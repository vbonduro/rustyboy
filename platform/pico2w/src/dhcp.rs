//! DHCP marshalling and unmarshalling for the captive-portal server.
//!
//! A captive-portal AP must answer DHCP so the joining phone gets an address in
//! our subnet; without it the phone associates but never obtains an IP and the
//! OS reports the network as unusable.  This is a deliberately minimal,
//! single-client server: it always offers the same address.

const DHCP_MAGIC_COOKIE: [u8; 4] = [0x63, 0x82, 0x53, 0x63];
const DHCP_OP_REPLY: u8 = 2;
const DHCP_DISCOVER: u8 = 1;
const DHCP_REQUEST: u8 = 3;
const DHCP_OFFER: u8 = 2;
const DHCP_ACK: u8 = 5;
const DHCP_NAK: u8 = 6;
/// Offset of the 4-byte magic cookie; the BOOTP fixed header precedes it.
pub const DHCP_COOKIE_OFFSET: usize = 236;
/// Smallest message we will parse: fixed header + magic cookie.
pub const DHCP_MIN_LEN: usize = DHCP_COOKIE_OFFSET + 4; // 240

/// Find a DHCP option by code and return its value slice.
///
/// Options are TLV (`code, len, value...`); `0` is a pad byte and `255` ends
/// the list.  Returns `None` if the option is absent or the list is malformed.
fn dhcp_option(req: &[u8], want: u8) -> Option<&[u8]> {
    let mut i = DHCP_MIN_LEN;
    while i < req.len() {
        let code = req[i];
        if code == 255 {
            break;
        }
        if code == 0 {
            i += 1;
            continue;
        }
        let len = *req.get(i + 1)? as usize;
        let val_start = i + 2;
        let val_end = val_start + len;
        if val_end > req.len() {
            return None;
        }
        if code == want {
            return Some(&req[val_start..val_end]);
        }
        i = val_end;
    }
    None
}

/// Parse option 53 (DHCP message type).
fn dhcp_message_type(req: &[u8]) -> Option<u8> {
    dhcp_option(req, 53).and_then(|v| v.first().copied())
}

/// The address the client wants confirmed in a REQUEST: option 50 (requested
/// IP) if present, otherwise `ciaddr` (used in RENEW/REBIND).
fn dhcp_requested_ip(req: &[u8]) -> Option<[u8; 4]> {
    if let Some(v) = dhcp_option(req, 50) {
        if v.len() == 4 {
            return Some([v[0], v[1], v[2], v[3]]);
        }
    }
    let ciaddr = [req[12], req[13], req[14], req[15]];
    if ciaddr != [0, 0, 0, 0] {
        return Some(ciaddr);
    }
    None
}

/// Length of the response this builder emits (fixed header + cookie + options).
pub const DHCP_RESPONSE_LEN: usize = 274;

/// Build a DHCP OFFER (reply to DISCOVER) or ACK (reply to REQUEST) into `out`.
///
/// `server_ip` is this AP's address — advertised as the server identifier,
/// router and DNS server.  `offer_ip` is the single address handed to the
/// client.  Returns the number of bytes written, or `None` when the request is
/// not a DISCOVER/REQUEST, is malformed, or `out` is smaller than
/// [`DHCP_RESPONSE_LEN`].
pub fn build_dhcp_response(
    req: &[u8],
    server_ip: [u8; 4],
    offer_ip: [u8; 4],
    out: &mut [u8],
) -> Option<usize> {
    if req.len() < DHCP_MIN_LEN {
        return None;
    }
    // Must be a BOOTREQUEST carrying the DHCP magic cookie.
    if req[0] != 1 || req[DHCP_COOKIE_OFFSET..DHCP_MIN_LEN] != DHCP_MAGIC_COOKIE {
        return None;
    }

    let resp_type = match dhcp_message_type(req)? {
        DHCP_DISCOVER => DHCP_OFFER,
        DHCP_REQUEST => {
            // We only have one address to give.  If the client is asking to
            // reconfirm a *different* address (a stale lease from another
            // network — the INIT-REBOOT case), NAK it so it discards that lease
            // and restarts with DISCOVER; otherwise an ACK with a mismatched
            // yiaddr is silently dropped by iOS/Android and the join stalls.
            match dhcp_requested_ip(req) {
                Some(ip) if ip != offer_ip => DHCP_NAK,
                _ => DHCP_ACK,
            }
        }
        _ => return None,
    };

    if out.len() < DHCP_RESPONSE_LEN {
        return None;
    }
    out[..DHCP_RESPONSE_LEN].fill(0);

    out[0] = DHCP_OP_REPLY; // op = BOOTREPLY
    out[1] = 1; // htype = ethernet
    out[2] = 6; // hlen = 6
                // hops (3) = 0
    out[4..8].copy_from_slice(&req[4..8]); // xid echo
                                           // secs (8..10) = 0
    out[10..12].copy_from_slice(&req[10..12]); // flags echo (broadcast bit)
                                               // ciaddr (12..16) = 0
    if resp_type != DHCP_NAK {
        // yiaddr is the offered address; a NAK leaves it zero.
        out[16..20].copy_from_slice(&offer_ip);
    }
    out[20..24].copy_from_slice(&server_ip); // siaddr
    out[24..28].copy_from_slice(&req[24..28]); // giaddr echo
    out[28..44].copy_from_slice(&req[28..44]); // chaddr echo (16 bytes)
                                               // sname/file zeroed already
    out[DHCP_COOKIE_OFFSET..DHCP_MIN_LEN].copy_from_slice(&DHCP_MAGIC_COOKIE);

    // Options.
    let mut p = DHCP_MIN_LEN;
    let opt = |out: &mut [u8], p: &mut usize, code: u8, val: &[u8]| {
        out[*p] = code;
        out[*p + 1] = val.len() as u8;
        out[*p + 2..*p + 2 + val.len()].copy_from_slice(val);
        *p += 2 + val.len();
    };
    opt(out, &mut p, 53, &[resp_type]); // message type
    opt(out, &mut p, 54, &server_ip); // server identifier
    if resp_type != DHCP_NAK {
        opt(out, &mut p, 51, &86_400u32.to_be_bytes()); // lease time (1 day)
        opt(out, &mut p, 1, &[255, 255, 255, 0]); // subnet mask
        opt(out, &mut p, 3, &server_ip); // router
        opt(out, &mut p, 6, &server_ip); // DNS server
    }
    out[p] = 255; // end
    p += 1;

    Some(p)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER_IP: [u8; 4] = [192, 168, 4, 1];
    const OFFER_IP: [u8; 4] = [192, 168, 4, 2];

    /// Build a minimal DHCP request of the given message type (option 53).
    fn make_dhcp_request(msg_type: u8) -> [u8; 244] {
        let mut pkt = [0u8; 244];
        pkt[0] = 1; // op = BOOTREQUEST
        pkt[1] = 1; // htype
        pkt[2] = 6; // hlen
        pkt[4..8].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]); // xid
                                                              // chaddr (client MAC)
        pkt[28..34].copy_from_slice(&[0x02, 0x11, 0x22, 0x33, 0x44, 0x55]);
        pkt[236..240].copy_from_slice(&DHCP_MAGIC_COOKIE);
        // option 53 (message type), then end.
        pkt[240] = 53;
        pkt[241] = 1;
        pkt[242] = msg_type;
        pkt[243] = 255;
        pkt
    }

    #[test]
    fn dhcp_discover_yields_offer() {
        let req = make_dhcp_request(DHCP_DISCOVER);
        let mut out = [0u8; 300];
        let len = build_dhcp_response(&req, SERVER_IP, OFFER_IP, &mut out).unwrap();
        assert_eq!(len, DHCP_RESPONSE_LEN);
        assert_eq!(out[0], DHCP_OP_REPLY);
        // xid echoed.
        assert_eq!(&out[4..8], &[0xDE, 0xAD, 0xBE, 0xEF]);
        // yiaddr = offered IP.
        assert_eq!(&out[16..20], &OFFER_IP);
        // chaddr echoed.
        assert_eq!(&out[28..34], &[0x02, 0x11, 0x22, 0x33, 0x44, 0x55]);
        // magic cookie + option 53 = OFFER.
        assert_eq!(&out[236..240], &DHCP_MAGIC_COOKIE);
        assert_eq!(out[240], 53);
        assert_eq!(out[242], DHCP_OFFER);
    }

    #[test]
    fn dhcp_request_no_requested_ip_yields_ack() {
        // No option 50 and ciaddr=0 (SELECTING after our OFFER) → ACK.
        let req = make_dhcp_request(DHCP_REQUEST);
        let mut out = [0u8; 300];
        build_dhcp_response(&req, SERVER_IP, OFFER_IP, &mut out).unwrap();
        assert_eq!(out[242], DHCP_ACK);
    }

    /// Build a REQUEST that carries option 50 (requested IP).
    fn make_dhcp_request_for(ip: [u8; 4]) -> [u8; 250] {
        let mut pkt = [0u8; 250];
        pkt[0] = 1;
        pkt[1] = 1;
        pkt[2] = 6;
        pkt[4..8].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        pkt[28..34].copy_from_slice(&[0x02, 0x11, 0x22, 0x33, 0x44, 0x55]);
        pkt[236..240].copy_from_slice(&DHCP_MAGIC_COOKIE);
        // option 53 = REQUEST
        pkt[240] = 53;
        pkt[241] = 1;
        pkt[242] = DHCP_REQUEST;
        // option 50 = requested IP
        pkt[243] = 50;
        pkt[244] = 4;
        pkt[245..249].copy_from_slice(&ip);
        pkt[249] = 255;
        pkt
    }

    #[test]
    fn dhcp_request_matching_ip_yields_ack() {
        let req = make_dhcp_request_for(OFFER_IP);
        let mut out = [0u8; 300];
        build_dhcp_response(&req, SERVER_IP, OFFER_IP, &mut out).unwrap();
        assert_eq!(out[242], DHCP_ACK);
        assert_eq!(&out[16..20], &OFFER_IP); // yiaddr set
    }

    #[test]
    fn dhcp_request_stale_ip_yields_nak() {
        // Phone reconfirming a lease from another network → NAK.
        let req = make_dhcp_request_for([192, 168, 1, 50]);
        let mut out = [0u8; 300];
        build_dhcp_response(&req, SERVER_IP, OFFER_IP, &mut out).unwrap();
        assert_eq!(out[242], DHCP_NAK);
        // NAK carries no offered address.
        assert_eq!(&out[16..20], &[0, 0, 0, 0]);
    }

    #[test]
    fn dhcp_response_advertises_server_options() {
        let req = make_dhcp_request(DHCP_DISCOVER);
        let mut out = [0u8; 300];
        let len = build_dhcp_response(&req, SERVER_IP, OFFER_IP, &mut out).unwrap();
        let opts = &out[240..len];
        // Walk the option list and collect (code -> value) for the ones we set.
        let mut server_id = None;
        let mut router = None;
        let mut dns = None;
        let mut mask = None;
        let mut i = 0;
        while i < opts.len() {
            let code = opts[i];
            if code == 255 {
                break;
            }
            let l = opts[i + 1] as usize;
            let v = &opts[i + 2..i + 2 + l];
            match code {
                54 => server_id = Some(v.to_vec()),
                3 => router = Some(v.to_vec()),
                6 => dns = Some(v.to_vec()),
                1 => mask = Some(v.to_vec()),
                _ => {}
            }
            i += 2 + l;
        }
        assert_eq!(server_id.unwrap(), SERVER_IP);
        assert_eq!(router.unwrap(), SERVER_IP);
        assert_eq!(dns.unwrap(), SERVER_IP);
        assert_eq!(mask.unwrap(), vec![255, 255, 255, 0]);
    }

    #[test]
    fn dhcp_ignores_non_discover_request() {
        // DHCPRELEASE (7) should produce no reply.
        let req = make_dhcp_request(7);
        let mut out = [0u8; 300];
        assert!(build_dhcp_response(&req, SERVER_IP, OFFER_IP, &mut out).is_none());
    }

    #[test]
    fn dhcp_rejects_short_packet() {
        let req = [0u8; 100];
        let mut out = [0u8; 300];
        assert!(build_dhcp_response(&req, SERVER_IP, OFFER_IP, &mut out).is_none());
    }

    #[test]
    fn dhcp_rejects_bad_cookie() {
        let mut req = make_dhcp_request(DHCP_DISCOVER);
        req[238] = 0x00; // corrupt magic cookie
        let mut out = [0u8; 300];
        assert!(build_dhcp_response(&req, SERVER_IP, OFFER_IP, &mut out).is_none());
    }

    #[test]
    fn dhcp_rejects_small_output() {
        let req = make_dhcp_request(DHCP_DISCOVER);
        let mut out = [0u8; 100];
        assert!(build_dhcp_response(&req, SERVER_IP, OFFER_IP, &mut out).is_none());
    }
}
