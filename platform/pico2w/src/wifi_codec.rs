//! Pure encode/decode helpers for WiFi credential flash storage and the
//! captive portal.
//!
//! This module has **no** `cfg(target_arch = "arm")` or `feature = "wifi"`
//! gate so that unit tests run on the host via `cargo test-host`.

// ---------------------------------------------------------------------------
// Config buffer codec (Finding A)
// ---------------------------------------------------------------------------

const MAGIC: &[u8; 4] = b"WIFY";
const SSID_OFFSET: usize = 4;
const SSID_LEN: usize = 32;
const PASSWORD_OFFSET: usize = 36;
const PASSWORD_LEN: usize = 64;
/// Total size of the credential record (magic + SSID + password).
pub const WIFI_CONFIG_BUF_LEN: usize = 100;

/// Serialize `(ssid, password)` into the 100-byte config buffer.
///
/// `buf` is **not** pre-erased here; the caller is responsible for
/// initialising it (typically to 0xFF for NOR flash).
pub fn encode_wifi_config(ssid: &str, password: &str, buf: &mut [u8; WIFI_CONFIG_BUF_LEN]) {
    buf.fill(0xFF);
    buf[..4].copy_from_slice(MAGIC);

    let ssid_bytes = ssid.as_bytes();
    let ssid_copy = ssid_bytes.len().min(SSID_LEN);
    buf[SSID_OFFSET..SSID_OFFSET + ssid_copy].copy_from_slice(&ssid_bytes[..ssid_copy]);
    // Null-terminate only when the SSID fits within the field — a full 32-byte
    // SSID leaves no room for a null and must be recovered by length.
    if ssid_copy < SSID_LEN {
        buf[SSID_OFFSET + ssid_copy] = 0;
    }

    let pw_bytes = password.as_bytes();
    let pw_copy = pw_bytes.len().min(PASSWORD_LEN);
    buf[PASSWORD_OFFSET..PASSWORD_OFFSET + pw_copy].copy_from_slice(&pw_bytes[..pw_copy]);
    if pw_copy < PASSWORD_LEN {
        buf[PASSWORD_OFFSET + pw_copy] = 0;
    }
}

/// Deserialize a 100-byte config buffer into `(ssid, password)`.
///
/// Returns `None` if the magic is absent/corrupt or the SSID is empty.
pub fn decode_wifi_config(
    buf: &[u8; WIFI_CONFIG_BUF_LEN],
) -> Option<(heapless::String<32>, heapless::String<64>)> {
    if &buf[..4] != MAGIC {
        return None;
    }

    let ssid_region = &buf[SSID_OFFSET..SSID_OFFSET + SSID_LEN];
    let ssid = fixed_field_str::<32>(ssid_region)?;

    let pw_region = &buf[PASSWORD_OFFSET..PASSWORD_OFFSET + PASSWORD_LEN];
    let password = fixed_field_str_optional::<64>(pw_region);

    Some((ssid, password))
}

// ---------------------------------------------------------------------------
// String helpers (Finding C)
// ---------------------------------------------------------------------------

/// Extract a UTF-8 string from a fixed-width field.
///
/// The field may be:
/// - Null-terminated (null byte found before the end) — use bytes before null.
/// - Full-width with no null — treat the entire field as the string.
///
/// Returns `None` if the resulting string is empty or not valid UTF-8.
pub fn null_terminated_str<const N: usize>(region: &[u8]) -> Option<heapless::String<N>> {
    let end = region.iter().position(|&b| b == 0).unwrap_or(region.len());
    if end == 0 {
        return None;
    }
    let s = core::str::from_utf8(&region[..end]).ok()?;
    heapless::String::try_from(s).ok()
}

/// Like [`null_terminated_str`] but returns an empty string instead of `None`
/// when the field is entirely absent / blank / has no null terminator and
/// decodes to an empty string.
pub fn null_terminated_str_optional<const N: usize>(region: &[u8]) -> heapless::String<N> {
    // For optional fields (password): if the whole region is 0xFF (erased) or
    // the first byte is 0, treat as empty.
    if region
        .first()
        .copied()
        .map_or(true, |b| b == 0 || b == 0xFF)
    {
        return heapless::String::new();
    }
    null_terminated_str(region).unwrap_or_default()
}

/// Internal alias used by `decode_wifi_config` for the SSID field.
fn fixed_field_str<const N: usize>(region: &[u8]) -> Option<heapless::String<N>> {
    null_terminated_str(region)
}

/// Internal alias used by `decode_wifi_config` for the password field.
fn fixed_field_str_optional<const N: usize>(region: &[u8]) -> heapless::String<N> {
    null_terminated_str_optional(region)
}

// ---------------------------------------------------------------------------
// URL percent-decode (Finding B / C)
// ---------------------------------------------------------------------------

/// Decode a URL percent-encoded string into `dst`.
///
/// Rules:
/// - `+` → space (application/x-www-form-urlencoded).
/// - `%XX` → decoded byte.
/// - Multi-byte UTF-8 sequences encoded as consecutive `%XX` escapes are
///   accumulated and validated as UTF-8 before being pushed.
/// - Truncated escapes (e.g. `%4` at end of string) are passed through as-is.
/// - Plain ASCII graphic characters and space are passed through.
/// - Bytes that are not valid in any context are dropped.
pub fn url_decode_into<const N: usize>(src: &str, dst: &mut heapless::String<N>) {
    let bytes = src.as_bytes();
    let mut i = 0;
    // Accumulator for multi-byte UTF-8 sequences decoded from %XX escapes.
    let mut utf8_buf = [0u8; 4];
    let mut utf8_len: usize = 0;

    while i < bytes.len() {
        let b = bytes[i];

        if b == b'+' {
            // Flush any pending UTF-8 bytes before emitting a literal space.
            flush_utf8(&utf8_buf, utf8_len, dst);
            utf8_len = 0;
            dst.push(' ').ok();
            i += 1;
        } else if b == b'%' && i + 2 < bytes.len() {
            if let Some(decoded) = decode_percent_escape(bytes[i + 1], bytes[i + 2]) {
                accumulate_utf8_byte(decoded, &mut utf8_buf, &mut utf8_len, dst);
                i += 3;
            } else {
                // Not a valid hex escape: flush pending buffer, pass '%' through.
                flush_utf8(&utf8_buf, utf8_len, dst);
                utf8_len = 0;
                dst.push('%').ok();
                i += 1;
            }
        } else {
            // Plain byte: flush any pending multi-byte sequence.
            flush_utf8(&utf8_buf, utf8_len, dst);
            utf8_len = 0;
            if b.is_ascii_graphic() || b == b' ' {
                // Safe: b is ASCII.
                dst.push(b as char).ok();
            }
            i += 1;
        }
    }

    // Flush any trailing bytes.
    flush_utf8(&utf8_buf, utf8_len, dst);
}

/// Decode a `%HI LO` percent-escape into a raw byte value.
///
/// Returns `None` if either nibble is not a valid hex digit.
fn decode_percent_escape(hi_ascii: u8, lo_ascii: u8) -> Option<u8> {
    let hi = hex_nibble(hi_ascii)?;
    let lo = hex_nibble(lo_ascii)?;
    Some((hi << 4) | lo)
}

/// Push a decoded byte into the UTF-8 accumulator buffer, flushing to `dst`
/// when a complete code point is ready.
///
/// ASCII bytes (< 0x80) flush any pending multi-byte buffer first and are
/// emitted immediately as a single char.  Non-ASCII bytes are accumulated; on
/// a complete and valid UTF-8 sequence the code point is pushed and the buffer
/// is cleared.  If the buffer fills without forming valid UTF-8 it is dropped.
fn accumulate_utf8_byte<const N: usize>(
    byte: u8,
    utf8_buf: &mut [u8; 4],
    utf8_len: &mut usize,
    dst: &mut heapless::String<N>,
) {
    if byte < 0x80 {
        // ASCII: flush pending multi-byte buffer first.
        flush_utf8(utf8_buf, *utf8_len, dst);
        *utf8_len = 0;
        // Safe: byte is ASCII.
        dst.push(byte as char).ok();
    } else {
        // Start or continuation of a multi-byte UTF-8 sequence.
        if *utf8_len < utf8_buf.len() {
            utf8_buf[*utf8_len] = byte;
            *utf8_len += 1;
            // Try to flush if we now have a complete code point.
            if let Ok(s) = core::str::from_utf8(&utf8_buf[..*utf8_len]) {
                for ch in s.chars() {
                    dst.push(ch).ok();
                }
                *utf8_len = 0;
            }
        }
        // If the buffer is full and still not valid UTF-8, drop it.
        if *utf8_len == utf8_buf.len() {
            *utf8_len = 0;
        }
    }
}

/// Flush the UTF-8 accumulator buffer to `dst`.
///
/// If the accumulated bytes form a valid UTF-8 string, each char is pushed.
/// Invalid UTF-8 bytes are silently dropped.
fn flush_utf8<const N: usize>(buf: &[u8; 4], len: usize, dst: &mut heapless::String<N>) {
    if len == 0 {
        return;
    }
    if let Ok(s) = core::str::from_utf8(&buf[..len]) {
        for ch in s.chars() {
            dst.push(ch).ok();
        }
    }
    // Invalid UTF-8 bytes are silently dropped.
}

// ---------------------------------------------------------------------------
// hex_nibble (Finding C)
// ---------------------------------------------------------------------------

/// Convert an ASCII hex digit byte to its numeric value (0–15), or `None`.
pub fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// find_body_start (Finding C)
// ---------------------------------------------------------------------------

/// Return the byte offset of the first character after the HTTP header/body
/// separator (`\r\n\r\n` or `\n\n`), or `None` if not found.
pub fn find_body_start(req: &str) -> Option<usize> {
    req.find("\r\n\r\n")
        .map(|i| i + 4)
        .or_else(|| req.find("\n\n").map(|i| i + 2))
}

// ---------------------------------------------------------------------------
// POST body credential parser (Finding C)
// ---------------------------------------------------------------------------

/// Parse an `application/x-www-form-urlencoded` body into `(ssid, password)`.
///
/// Returns `None` when the `ssid` field is absent or decodes to an empty
/// string.  The `password` field is optional; missing → empty string.
/// Unknown fields are ignored.
pub fn parse_credentials(body: &str) -> Option<(heapless::String<32>, heapless::String<64>)> {
    let mut ssid: heapless::String<32> = heapless::String::new();
    let mut password: heapless::String<64> = heapless::String::new();

    for pair in body.split('&') {
        let mut kv = pair.splitn(2, '=');
        let key = kv.next().unwrap_or("").trim();
        let val = kv.next().unwrap_or("").trim();
        match key {
            "ssid" => url_decode_into(val, &mut ssid),
            "password" => url_decode_into(val, &mut password),
            _ => {}
        }
    }

    if ssid.is_empty() {
        return None;
    }

    Some((ssid, password))
}

// ---------------------------------------------------------------------------
// DNS response builder (Finding D)
// ---------------------------------------------------------------------------

/// Build a minimal DNS A-record response into `out`, using `ip_octets` as the
/// answer address.
///
/// Returns the number of bytes written, or `None` if:
/// - `query` is shorter than 12 bytes (no valid DNS header).
/// - `out` is too small to hold the response.
pub fn build_dns_response(query: &[u8], ip_octets: [u8; 4], out: &mut [u8]) -> Option<usize> {
    if query.len() < 12 {
        return None;
    }

    let q_section_len = query.len() - 12;
    // Header (12) + question section + answer record (16).
    let needed = 12 + q_section_len + 16;
    if out.len() < needed {
        return None;
    }

    // Transaction ID.
    out[0] = query[0];
    out[1] = query[1];
    // Flags: QR=1 (response), AA=1, RD=1, RA=1 → 0x8180.
    out[2] = 0x81;
    out[3] = 0x80;
    // QDCOUNT = 1.
    out[4] = 0x00;
    out[5] = 0x01;
    // ANCOUNT = 1.
    out[6] = 0x00;
    out[7] = 0x01;
    // NSCOUNT = 0.
    out[8] = 0x00;
    out[9] = 0x00;
    // ARCOUNT = 0.
    out[10] = 0x00;
    out[11] = 0x00;

    // Copy question section verbatim.
    out[12..12 + q_section_len].copy_from_slice(&query[12..]);

    let mut pos = 12 + q_section_len;

    // Answer: pointer to question section (0xC00C).
    out[pos] = 0xC0;
    out[pos + 1] = 0x0C;
    // Type A.
    out[pos + 2] = 0x00;
    out[pos + 3] = 0x01;
    // Class IN.
    out[pos + 4] = 0x00;
    out[pos + 5] = 0x01;
    // TTL = 60.
    out[pos + 6] = 0x00;
    out[pos + 7] = 0x00;
    out[pos + 8] = 0x00;
    out[pos + 9] = 60;
    // RDLENGTH = 4.
    out[pos + 10] = 0x00;
    out[pos + 11] = 0x04;
    // RDATA — IP address.
    out[pos + 12] = ip_octets[0];
    out[pos + 13] = ip_octets[1];
    out[pos + 14] = ip_octets[2];
    out[pos + 15] = ip_octets[3];
    pos += 16;

    Some(pos)
}

// DHCP marshalling has been moved to `wifi::dhcp`.

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Finding A — round-trip tests for encode/decode
    // -----------------------------------------------------------------------

    fn encode_then_decode(
        ssid: &str,
        password: &str,
    ) -> Option<(heapless::String<32>, heapless::String<64>)> {
        let mut buf = [0u8; WIFI_CONFIG_BUF_LEN];
        encode_wifi_config(ssid, password, &mut buf);
        decode_wifi_config(&buf)
    }

    #[test]
    fn config_roundtrip_empty_ssid_returns_none() {
        assert!(encode_then_decode("", "password").is_none());
    }

    #[test]
    fn config_roundtrip_one_char_ssid() {
        let (ssid, pw) = encode_then_decode("A", "secret").unwrap();
        assert_eq!(ssid.as_str(), "A");
        assert_eq!(pw.as_str(), "secret");
    }

    #[test]
    fn config_roundtrip_31_char_ssid() {
        let s = "1234567890123456789012345678901"; // 31 chars
        let (ssid, pw) = encode_then_decode(s, "pass").unwrap();
        assert_eq!(ssid.as_str(), s);
        assert_eq!(pw.as_str(), "pass");
    }

    /// **Bug regression (Finding A)**: a 32-char SSID must survive the round-trip.
    #[test]
    fn config_roundtrip_32_char_ssid_the_bug() {
        let s = "12345678901234567890123456789012"; // exactly 32 chars
        let (ssid, pw) = encode_then_decode(s, "hunter2").unwrap();
        assert_eq!(ssid.as_str(), s);
        assert_eq!(pw.as_str(), "hunter2");
    }

    #[test]
    fn config_roundtrip_empty_password() {
        let (ssid, pw) = encode_then_decode("MyNet", "").unwrap();
        assert_eq!(ssid.as_str(), "MyNet");
        assert_eq!(pw.as_str(), "");
    }

    #[test]
    fn config_roundtrip_full_64_char_password() {
        let p = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"; // 64
        let (ssid, pw) = encode_then_decode("Net", p).unwrap();
        assert_eq!(ssid.as_str(), "Net");
        assert_eq!(pw.as_str(), p);
    }

    #[test]
    fn config_decode_bad_magic_returns_none() {
        let mut buf = [0u8; WIFI_CONFIG_BUF_LEN];
        encode_wifi_config("Net", "pw", &mut buf);
        buf[0] = b'X'; // corrupt magic
        assert!(decode_wifi_config(&buf).is_none());
    }

    #[test]
    fn config_decode_invalid_utf8_ssid_returns_none() {
        let mut buf = [0u8; WIFI_CONFIG_BUF_LEN];
        encode_wifi_config("valid", "pw", &mut buf);
        // Overwrite SSID region with invalid UTF-8.
        buf[SSID_OFFSET] = 0xFF;
        buf[SSID_OFFSET + 1] = 0xFE;
        buf[SSID_OFFSET + 2] = 0;
        assert!(decode_wifi_config(&buf).is_none());
    }

    // -----------------------------------------------------------------------
    // Finding C — null_terminated_str / null_terminated_str_optional
    // -----------------------------------------------------------------------

    #[test]
    fn null_str_normal() {
        let region = b"hello\x00\xFF\xFF";
        let s: heapless::String<32> = null_terminated_str(region).unwrap();
        assert_eq!(s.as_str(), "hello");
    }

    #[test]
    fn null_str_null_at_position_zero_returns_none() {
        let region = b"\x00hello";
        let s: Option<heapless::String<32>> = null_terminated_str(region);
        assert!(s.is_none());
    }

    #[test]
    fn null_str_no_null_uses_full_region() {
        let region = b"AB";
        let s: heapless::String<32> = null_terminated_str(region).unwrap();
        assert_eq!(s.as_str(), "AB");
    }

    #[test]
    fn null_str_invalid_utf8_returns_none() {
        let region = [0xFF, 0xFE, 0x00, 0x00];
        let s: Option<heapless::String<32>> = null_terminated_str(&region);
        assert!(s.is_none());
    }

    #[test]
    fn null_str_full_length_ascii() {
        let region = *b"12345678901234567890123456789012"; // 32 bytes, no null
        let s: heapless::String<32> = null_terminated_str(&region).unwrap();
        assert_eq!(s.as_str(), "12345678901234567890123456789012");
    }

    #[test]
    fn null_str_optional_empty_field_returns_empty() {
        let region = [0u8; 64];
        let s: heapless::String<64> = null_terminated_str_optional(&region);
        assert_eq!(s.as_str(), "");
    }

    #[test]
    fn null_str_optional_ff_region_returns_empty() {
        let region = [0xFFu8; 64];
        let s: heapless::String<64> = null_terminated_str_optional(&region);
        assert_eq!(s.as_str(), "");
    }

    #[test]
    fn null_str_optional_present_value() {
        let mut region = [0u8; 64];
        region[..5].copy_from_slice(b"hello");
        region[5] = 0;
        let s: heapless::String<64> = null_terminated_str_optional(&region);
        assert_eq!(s.as_str(), "hello");
    }

    // -----------------------------------------------------------------------
    // Finding C — hex_nibble
    // -----------------------------------------------------------------------

    #[test]
    fn hex_nibble_digits() {
        for (ch, val) in (b'0'..=b'9').zip(0u8..=9) {
            assert_eq!(hex_nibble(ch), Some(val));
        }
    }

    #[test]
    fn hex_nibble_lower() {
        assert_eq!(hex_nibble(b'a'), Some(10));
        assert_eq!(hex_nibble(b'f'), Some(15));
    }

    #[test]
    fn hex_nibble_upper() {
        assert_eq!(hex_nibble(b'A'), Some(10));
        assert_eq!(hex_nibble(b'F'), Some(15));
    }

    #[test]
    fn hex_nibble_invalid() {
        assert_eq!(hex_nibble(b'g'), None);
        assert_eq!(hex_nibble(b'G'), None);
        assert_eq!(hex_nibble(b'!'), None);
        assert_eq!(hex_nibble(b' '), None);
    }

    // -----------------------------------------------------------------------
    // Finding C — find_body_start
    // -----------------------------------------------------------------------

    #[test]
    fn find_body_start_crnl() {
        let req = "GET / HTTP/1.0\r\n\r\nbody";
        assert_eq!(find_body_start(req), Some(18));
    }

    #[test]
    fn find_body_start_nn() {
        let req = "GET / HTTP/1.0\n\nbody";
        assert_eq!(find_body_start(req), Some(16));
    }

    #[test]
    fn find_body_start_neither() {
        let req = "GET / HTTP/1.0\r\nbody";
        assert_eq!(find_body_start(req), None);
    }

    // -----------------------------------------------------------------------
    // Finding B — url_decode_into (UTF-8 / non-ASCII)
    // -----------------------------------------------------------------------

    fn url_decode(s: &str) -> heapless::String<128> {
        let mut out = heapless::String::new();
        url_decode_into(s, &mut out);
        out
    }

    #[test]
    fn url_decode_plain_ascii() {
        assert_eq!(url_decode("hello").as_str(), "hello");
    }

    #[test]
    fn url_decode_plus_to_space() {
        assert_eq!(url_decode("foo+bar").as_str(), "foo bar");
    }

    #[test]
    fn url_decode_percent20_to_space() {
        assert_eq!(url_decode("foo%20bar").as_str(), "foo bar");
    }

    /// **Bug regression (Finding B)**: `£` = U+00A3 = UTF-8 bytes C2 A3.
    #[test]
    fn url_decode_non_ascii_utf8_pound_sign() {
        // £ encoded as %C2%A3
        let decoded = url_decode("%C2%A3");
        assert_eq!(decoded.as_str(), "£");
    }

    #[test]
    fn url_decode_non_ascii_utf8_euro_sign() {
        // € = U+20AC = UTF-8 bytes E2 82 AC
        let decoded = url_decode("%E2%82%AC");
        assert_eq!(decoded.as_str(), "€");
    }

    #[test]
    fn url_decode_truncated_escape_passes_percent_through() {
        // '%4' at end — not a complete escape
        let decoded = url_decode("%4");
        assert_eq!(decoded.as_str(), "%4");
    }

    #[test]
    fn url_decode_bare_percent_passes_through() {
        let decoded = url_decode("100%");
        assert_eq!(decoded.as_str(), "100%");
    }

    // -----------------------------------------------------------------------
    // Finding C — parse_credentials (POST body parser)
    // -----------------------------------------------------------------------

    #[test]
    fn parse_credentials_basic() {
        let (ssid, pw) = parse_credentials("ssid=Foo&password=bar%20baz").unwrap();
        assert_eq!(ssid.as_str(), "Foo");
        assert_eq!(pw.as_str(), "bar baz");
    }

    #[test]
    fn parse_credentials_missing_ssid_returns_none() {
        assert!(parse_credentials("password=bar").is_none());
    }

    #[test]
    fn parse_credentials_empty_body_returns_none() {
        assert!(parse_credentials("").is_none());
    }

    #[test]
    fn parse_credentials_extra_fields_ignored() {
        let (ssid, pw) = parse_credentials("foo=bar&ssid=Net&baz=qux&password=pw").unwrap();
        assert_eq!(ssid.as_str(), "Net");
        assert_eq!(pw.as_str(), "pw");
    }

    #[test]
    fn parse_credentials_missing_password_defaults_empty() {
        let (ssid, pw) = parse_credentials("ssid=OnlySSID").unwrap();
        assert_eq!(ssid.as_str(), "OnlySSID");
        assert_eq!(pw.as_str(), "");
    }

    #[test]
    fn parse_credentials_url_encoded_ssid() {
        let (ssid, _) = parse_credentials("ssid=My%20Network&password=").unwrap();
        assert_eq!(ssid.as_str(), "My Network");
    }

    // -----------------------------------------------------------------------
    // Finding D — build_dns_response
    // -----------------------------------------------------------------------

    const AP_IP: [u8; 4] = [192, 168, 4, 1];

    fn make_minimal_query() -> [u8; 29] {
        // Minimal DNS query for "a.b" (type A, class IN).
        // Header: ID=0x1234, QR=0 (query), QDCOUNT=1, rest=0.
        let mut pkt = [0u8; 29];
        pkt[0] = 0x12;
        pkt[1] = 0x34;
        pkt[2] = 0x01; // RD flag
        pkt[3] = 0x00;
        pkt[4] = 0x00;
        pkt[5] = 0x01; // QDCOUNT=1
                       // Question: \x01a\x01b\x00 + QTYPE=1 + QCLASS=1
        pkt[12] = 0x01;
        pkt[13] = b'a';
        pkt[14] = 0x01;
        pkt[15] = b'b';
        pkt[16] = 0x00;
        pkt[17] = 0x00;
        pkt[18] = 0x01;
        pkt[19] = 0x00;
        pkt[20] = 0x01;
        // bytes 21..29 stay 0
        pkt
    }

    #[test]
    fn dns_response_short_query_returns_none() {
        let query = [0u8; 11];
        let mut out = [0u8; 256];
        assert!(build_dns_response(&query, AP_IP, &mut out).is_none());
    }

    #[test]
    fn dns_response_out_too_small_returns_none() {
        let query = make_minimal_query();
        let mut out = [0u8; 10]; // way too small
        assert!(build_dns_response(&query, AP_IP, &mut out).is_none());
    }

    #[test]
    fn dns_response_header_flags_correct() {
        let query = make_minimal_query();
        let mut out = [0u8; 256];
        let len = build_dns_response(&query, AP_IP, &mut out).unwrap();
        assert!(len > 12);
        // Transaction ID echoed.
        assert_eq!(out[0], 0x12);
        assert_eq!(out[1], 0x34);
        // Response flags: 0x8180.
        assert_eq!(out[2], 0x81);
        assert_eq!(out[3], 0x80);
        // QDCOUNT = 1.
        assert_eq!(out[4], 0x00);
        assert_eq!(out[5], 0x01);
        // ANCOUNT = 1.
        assert_eq!(out[6], 0x00);
        assert_eq!(out[7], 0x01);
    }

    #[test]
    fn dns_response_answer_record_correct() {
        let query = make_minimal_query();
        let q_section_len = query.len() - 12;
        let mut out = [0u8; 256];
        let len = build_dns_response(&query, AP_IP, &mut out).unwrap();
        let ans = &out[12 + q_section_len..len];
        // Name pointer 0xC00C.
        assert_eq!(ans[0], 0xC0);
        assert_eq!(ans[1], 0x0C);
        // Type A.
        assert_eq!(ans[2], 0x00);
        assert_eq!(ans[3], 0x01);
        // Class IN.
        assert_eq!(ans[4], 0x00);
        assert_eq!(ans[5], 0x01);
        // RDLENGTH = 4.
        assert_eq!(ans[10], 0x00);
        assert_eq!(ans[11], 0x04);
        // RDATA = AP IP.
        assert_eq!(&ans[12..16], &AP_IP);
    }

    #[test]
    fn dns_response_question_section_echoed() {
        let query = make_minimal_query();
        let q_section_len = query.len() - 12;
        let mut out = [0u8; 256];
        build_dns_response(&query, AP_IP, &mut out).unwrap();
        assert_eq!(&out[12..12 + q_section_len], &query[12..]);
    }
}
