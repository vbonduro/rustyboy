//! Captive portal Embassy tasks.
//!
//! Five concurrent tasks bring up the full WiFi portal:
//!
//! 1. [`cyw43_task`]  — keeps the CYW43 runner alive.
//! 2. [`net_task`]    — runs the embassy-net stack.
//! 3. [`dhcp_task`]   — UDP/67 DHCP server; hands the joining phone an IP in
//!                      192.168.4.0/24 (without it the phone can't connect at all).
//! 4. [`dns_task`]    — UDP/53 catch-all; redirects all DNS queries to
//!                      192.168.4.1 (triggers iOS/Android captive-portal detection).
//! 5. [`http_task`]   — minimal HTTP/1.0 server on port 80:
//!                      `GET /`             → HTML config form with scanned SSIDs.
//!                      `POST /configure`   → parses credentials; signals result via
//!                                           [`PORTAL_RESULT`].
//!
//! The UI task (WifiPortalScreen) polls [`PORTAL_RESULT.try_take()`] each tick.
//! On receipt it saves the credentials to flash and triggers a system reset.

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpEndpoint, IpListenEndpoint, Ipv4Address, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embedded_io_async::Write as _;

use super::driver::{Cyw43Runner, AP_IP_OCTETS};
use crate::dhcp::{build_dhcp_response, DHCP_RESPONSE_LEN};
use crate::wifi_codec::{build_dns_response, find_body_start, parse_credentials};

// ---------------------------------------------------------------------------
// Shared result signal
// ---------------------------------------------------------------------------

/// Credentials received via the captive portal POST.
///
/// Polled from the UI task each tick; on signal arrival the UI saves to flash
/// and calls `cortex_m::peripheral::SCB::sys_reset()`.
pub static PORTAL_RESULT: Signal<CriticalSectionRawMutex, PortalCredentials> = Signal::new();

/// WiFi credentials captured from the portal form submission.
pub struct PortalCredentials {
    pub ssid: heapless::String<32>,
    pub password: heapless::String<64>,
}

// ---------------------------------------------------------------------------
// Task: cyw43 runner (must always be alive while WiFi is active)
// ---------------------------------------------------------------------------

#[embassy_executor::task]
pub async fn cyw43_task(runner: Cyw43Runner) -> ! {
    runner.run().await
}

// ---------------------------------------------------------------------------
// Task: embassy-net stack runner
// ---------------------------------------------------------------------------

#[embassy_executor::task]
pub async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

// ---------------------------------------------------------------------------
// Task: DNS catch-all (UDP/53)
// ---------------------------------------------------------------------------

/// Answer every DNS query with 192.168.4.1, triggering captive-portal
/// detection on iOS and Android devices.
#[embassy_executor::task]
pub async fn dns_task(stack: Stack<'static>) {
    let mut rx_meta = [PacketMetadata::EMPTY; 4];
    let mut tx_meta = [PacketMetadata::EMPTY; 4];
    let mut rx_buf = [0u8; 512];
    let mut tx_buf = [0u8; 512];
    let mut pkt_buf = [0u8; 512];

    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    socket
        .bind(IpListenEndpoint {
            addr: None,
            port: 53,
        })
        .ok();

    loop {
        let Ok((n, remote)) = socket.recv_from(&mut pkt_buf).await else {
            continue;
        };

        let mut resp = [0u8; 512];
        if let Some(resp_len) = build_dns_response(&pkt_buf[..n], AP_IP_OCTETS, &mut resp) {
            socket.send_to(&resp[..resp_len], remote).await.ok();
        }
    }
}

// ---------------------------------------------------------------------------
// Task: DHCP server (UDP/67)
// ---------------------------------------------------------------------------

/// Minimal single-client DHCP server.
///
/// Without this, a phone that joins the open AP never gets an IP address (the
/// OS reports the network as unusable and won't load the portal page).  We
/// hand every client the same address (`192.168.4.2`) and advertise ourselves
/// (`192.168.4.1`) as router + DNS so all traffic funnels to the portal.
#[embassy_executor::task]
pub async fn dhcp_task(stack: Stack<'static>) {
    let mut rx_meta = [PacketMetadata::EMPTY; 4];
    let mut tx_meta = [PacketMetadata::EMPTY; 4];
    let mut rx_buf = [0u8; 1024];
    let mut tx_buf = [0u8; 1024];
    let mut pkt_buf = [0u8; 1024];

    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    if socket
        .bind(IpListenEndpoint {
            addr: None,
            port: 67,
        })
        .is_err()
    {
        warn!("dhcp: bind failed");
        return;
    }

    info!("dhcp: server listening on UDP/67");

    let server_ip = AP_IP_OCTETS;
    let offer_ip = [AP_IP_OCTETS[0], AP_IP_OCTETS[1], AP_IP_OCTETS[2], 2];

    loop {
        let Ok((n, remote)) = socket.recv_from(&mut pkt_buf).await else {
            continue;
        };
        info!("dhcp: rx {} bytes from {:?}", n, remote);

        let mut resp = [0u8; DHCP_RESPONSE_LEN];
        if let Some(len) = build_dhcp_response(&pkt_buf[..n], server_ip, offer_ip, &mut resp) {
            // The client has no IP yet, so broadcast the reply to 255.255.255.255:68.
            let dest = IpEndpoint::new(Ipv4Address::new(255, 255, 255, 255).into(), 68);
            match socket.send_to(&resp[..len], dest).await {
                Ok(()) => info!("dhcp: sent {}-byte reply (type {})", len, resp[242]),
                Err(e) => warn!("dhcp: send failed: {:?}", e),
            }
        } else {
            warn!("dhcp: ignored packet (not a DISCOVER/REQUEST)");
        }
    }
}

// ---------------------------------------------------------------------------
// Task: HTTP captive portal (TCP/80)
// ---------------------------------------------------------------------------

/// Minimal HTTP/1.0 server.
///
/// - `GET /`           → HTML form with SSID dropdown and password field.
/// - `POST /configure` → parses `ssid=&password=`, signals [`PORTAL_RESULT`].
/// - All other paths   → 302 redirect to `/` (triggers iOS/Android portal).
#[embassy_executor::task]
pub async fn http_task(stack: Stack<'static>, ssids: heapless::Vec<heapless::String<32>, 16>) {
    let mut rx_buf = [0u8; 2048];
    let mut tx_buf = [0u8; 4096];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(embassy_time::Duration::from_secs(10)));

        if let Err(e) = socket.accept(80).await {
            warn!("http: accept error: {:?}", e);
            continue;
        }

        handle_http_connection(&mut socket, &ssids).await;
        socket.flush().await.ok();
        socket.close();
        // Wait for the close to propagate.
        embassy_time::Timer::after(embassy_time::Duration::from_millis(50)).await;
        socket.abort();
    }
}

async fn handle_http_connection(
    socket: &mut TcpSocket<'_>,
    ssids: &heapless::Vec<heapless::String<32>, 16>,
) {
    // Read the first 1 KiB of the request — enough for headers + small POST body.
    let mut req_buf = [0u8; 1024];
    let mut total = 0usize;
    loop {
        match socket.read(&mut req_buf[total..]).await {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                if total >= req_buf.len() {
                    break;
                }
                let so_far = &req_buf[..total];
                if so_far.windows(4).any(|w| w == b"\r\n\r\n")
                    || so_far.windows(2).any(|w| w == b"\n\n")
                {
                    break;
                }
            }
            Err(_) => return,
        }
    }

    let req = &req_buf[..total];
    let Ok(req_str) = core::str::from_utf8(req) else {
        send_redirect(socket).await;
        return;
    };

    // Parse the request line.
    let mut lines = req_str.lines();
    let Some(request_line) = lines.next() else {
        send_redirect(socket).await;
        return;
    };

    let mut parts = request_line.splitn(3, ' ');
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");

    match (method, path) {
        ("GET", "/") | ("GET", "/?") => {
            send_portal_page(socket, ssids).await;
        }
        ("POST", "/configure") => {
            let body = if let Some(idx) = find_body_start(req_str) {
                &req_str[idx..]
            } else {
                ""
            };
            handle_configure(socket, body).await;
        }
        _ => {
            send_redirect(socket).await;
        }
    }
}

async fn send_redirect(socket: &mut TcpSocket<'_>) {
    let response =
        b"HTTP/1.0 302 Found\r\nLocation: http://192.168.4.1/\r\nContent-Length: 0\r\n\r\n";
    socket.write_all(response).await.ok();
}

async fn send_portal_page(
    socket: &mut TcpSocket<'_>,
    ssids: &heapless::Vec<heapless::String<32>, 16>,
) {
    let header = b"HTTP/1.0 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n";
    socket.write_all(header).await.ok();

    let page_top = br#"<!DOCTYPE html><html><head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>RustyBoy WiFi Setup</title>
<style>
body{font-family:sans-serif;max-width:480px;margin:2em auto;padding:0 1em;background:#f5f5f5}
h1{font-size:1.4em;color:#333}
label{display:block;margin:1em 0 .3em;font-weight:bold}
select,input{width:100%;padding:.7em;font-size:1em;border:1px solid #ccc;border-radius:4px;box-sizing:border-box}
button{width:100%;padding:.8em;margin-top:1.5em;font-size:1.1em;background:#0078d4;color:#fff;border:none;border-radius:4px;cursor:pointer}
#manual{display:none;margin-top:.5em}
</style>
</head><body>
<h1>&#x1F4F6; RustyBoy WiFi Setup</h1>
<form method="POST" action="/configure">
<label for="ssid_sel">Select Network</label>
<select id="ssid_sel" onchange="m(this)">
"#;
    socket.write_all(page_top).await.ok();

    // SSID options.
    for ssid in ssids.iter() {
        let mut opt: heapless::String<128> = heapless::String::new();
        let _ = core::fmt::write(
            &mut opt,
            format_args!("<option value=\"{}\">{}</option>\n", ssid, ssid),
        );
        socket.write_all(opt.as_bytes()).await.ok();
    }
    socket
        .write_all(b"<option value=\"__other__\">Other (enter below)...</option>\n")
        .await
        .ok();

    let page_mid = br#"</select>
<div id="manual">
<label for="ssid_man">Network name (SSID)</label>
<input type="text" id="ssid_man" name="ssid_manual" maxlength="32">
</div>
<input type="hidden" id="ssid_hid" name="ssid" value="">
<label for="pw">Password</label>
<input type="password" id="pw" name="password" maxlength="64">
<button type="submit">Connect</button>
</form>
<script>
function m(s){
  var manual=document.getElementById('manual');
  var h=document.getElementById('ssid_hid');
  if(s.value==='__other__'){manual.style.display='block';h.value='';}
  else{manual.style.display='none';h.value=s.value;}
}
(function(){var s=document.getElementById('ssid_sel');m(s);})();
document.querySelector('form').onsubmit=function(){
  var sel=document.getElementById('ssid_sel');
  var h=document.getElementById('ssid_hid');
  if(sel.value==='__other__'){
    h.value=document.getElementById('ssid_man').value;
  }
};
</script>
</body></html>"#;
    socket.write_all(page_mid).await.ok();
}

async fn handle_configure(socket: &mut TcpSocket<'_>, body: &str) {
    match parse_credentials(body) {
        None => {
            let resp = b"HTTP/1.0 400 Bad Request\r\nContent-Length: 9\r\n\r\nBad SSID";
            socket.write_all(resp).await.ok();
        }
        Some((ssid, password)) => {
            info!("portal: credentials received for SSID '{}'", ssid.as_str());

            // Signal the UI task.
            PORTAL_RESULT.signal(PortalCredentials { ssid, password });

            let resp = br#"HTTP/1.0 200 OK
Content-Type: text/html; charset=utf-8

<!DOCTYPE html><html><head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Saved</title>
</head><body>
<h2>&#x2705; Credentials saved!</h2>
<p>RustyBoy is restarting. Reconnect to your WiFi network.</p>
</body></html>"#;
            socket.write_all(resp).await.ok();
        }
    }
}

// ---------------------------------------------------------------------------
// Spawn helper (called from WifiPortalScreen::start_portal)
// ---------------------------------------------------------------------------

/// Spawn the cyw43 runner task.
///
/// Must be done **before** any `control` ioctl (CLM download, scan, AP start) —
/// those calls block on the runner servicing them.
pub fn spawn_cyw43_runner(spawner: &Spawner, runner: Cyw43Runner) {
    // Tasks return Result<SpawnToken, SpawnError> at this embassy revision;
    // use defmt::unwrap! to panic loudly if spawning fails (out of task slots).
    spawner.spawn(defmt::unwrap!(cyw43_task(runner)));
}

/// Spawn the network stack + DNS + HTTP portal tasks.
///
/// Call after the AP is up so the net stack has a link to run on.
pub fn spawn_net_tasks(
    spawner: &Spawner,
    net_runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>,
    stack: Stack<'static>,
    ssids: heapless::Vec<heapless::String<32>, 16>,
) {
    spawner.spawn(defmt::unwrap!(net_task(net_runner)));
    spawner.spawn(defmt::unwrap!(dhcp_task(stack)));
    spawner.spawn(defmt::unwrap!(dns_task(stack)));
    spawner.spawn(defmt::unwrap!(http_task(stack, ssids)));
}
