//! Public HTTP bring-up response check, independent of the radio driver.
pub fn verified_response(response: &[u8]) -> bool {
    let ok_status =
        response.starts_with(b"HTTP/1.0 200 ") || response.starts_with(b"HTTP/1.1 200 ");
    let body = response
        .windows(4)
        .position(|v| v == b"\r\n\r\n")
        .map(|n| &response[n + 4..]);
    ok_status
        && body.is_some_and(|body| {
            body.windows(b"<h1>Example Domain</h1>".len())
                .any(|w| w == b"<h1>Example Domain</h1>")
        })
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
    fn requires_success_status_and_expected_page() {
        assert!(verified_response(
            b"HTTP/1.0 200 OK\r\n\r\n<h1>Example Domain</h1>"
        ));
        assert!(verified_response(
            b"HTTP/1.1 200 OK\r\n\r\n<h1>Example Domain</h1>"
        ));
        for response in [
            b"HTTP/1.0 404 Missing\r\n\r\n<h1>Example Domain</h1>".as_slice(),
            b"HTTP/1.0 200 OK\r\n\r\nwrong",
            b"HTTP/1.0 200 Example Domain\r\n\r\nwrong",
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
