//! Public HTTP bring-up response check, independent of the radio driver.
pub const PROBE_HOST: &str = "connectivitycheck.gstatic.com";
pub const PROBE_REQUEST: &[u8] =
    b"GET /generate_204 HTTP/1.0\r\nHost: connectivitycheck.gstatic.com\r\nConnection: close\r\n\r\n";

/// The endpoint answers 204 with no body. A captive portal or proxy that
/// rewrites the page answers 200 or 302 instead, so only 204 counts.
pub fn verified_response(response: &[u8]) -> bool {
    response.starts_with(b"HTTP/1.0 204 ") || response.starts_with(b"HTTP/1.1 204 ")
}

pub fn retry_delay_secs(failures: u8) -> u64 {
    1u64.checked_shl(u32::from(failures.min(5)))
        .unwrap_or(32)
        .min(30)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requires_no_content_status() {
        assert!(verified_response(b"HTTP/1.0 204 No Content\r\n\r\n"));
        assert!(verified_response(
            b"HTTP/1.1 204 No Content\r\nDate: x\r\n\r\n"
        ));
        for response in [
            b"HTTP/1.1 200 OK\r\n\r\n<html>portal</html>".as_slice(),
            b"HTTP/1.1 302 Found\r\nLocation: http://portal/\r\n\r\n",
            b"HTTP/1.1 2040 X\r\n\r\n",
            b"",
        ] {
            assert!(!verified_response(response));
        }
    }

    #[test]
    fn retry_backoff_is_bounded() {
        assert_eq!(
            core::array::from_fn::<_, 8, _>(|i| retry_delay_secs(i as u8)),
            [1, 2, 4, 8, 16, 30, 30, 30]
        );
    }
}
