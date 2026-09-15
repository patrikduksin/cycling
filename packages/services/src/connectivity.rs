/// Connection intent survives unexpected loss; explicit configuration/removal stops retries.
#[derive(Default)]
pub struct Reconnect {
    requested: bool,
    failures: u8,
}
impl Reconnect {
    pub fn connect(&mut self) {
        self.requested = true;
        self.failures = 0;
    }
    pub fn disconnect(&mut self) {
        self.requested = false;
        self.failures = 0;
    }
    pub fn requested(&self) -> bool {
        self.requested
    }
    pub fn recovered(&mut self) {
        self.failures = 0;
    }
    pub fn delay(&mut self) -> u64 {
        let delay = crate::network::retry_delay_secs(self.failures);
        self.failures = self.failures.saturating_add(1);
        delay
    }
}
#[cfg(test)]
mod recovery_tests {
    #[test]
    fn intent_survives_loss_with_capped_backoff_and_explicit_stop_cancels_it() {
        let mut retry = super::Reconnect::default();
        assert!(!retry.requested());
        retry.connect();
        assert!(retry.requested());
        let first = retry.delay();
        assert!(first > 0);
        for _ in 0..300 {
            assert!(retry.delay() <= 60);
            assert!(retry.requested());
        }
        retry.recovered();
        assert_eq!(retry.delay(), first);
        retry.disconnect();
        assert!(!retry.requested());
        retry.connect();
        assert_eq!(retry.delay(), first);
    }
}
