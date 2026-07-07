/// Counts consecutive handshake failures. When the count reaches the
/// threshold the watchdog "trips": record_failure returns true and the
/// counter resets, so the caller restarts the pipeline exactly once.
pub struct Watchdog {
    consecutive_failures: u32,
    threshold: u32,
}

impl Watchdog {
    pub fn new(threshold: u32) -> Self {
        Self {
            consecutive_failures: 0,
            threshold,
        }
    }

    pub fn record_failure(&mut self) -> bool {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= self.threshold {
            self.consecutive_failures = 0;
            true
        } else {
            false
        }
    }

    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::Watchdog;

    #[test]
    fn trips_exactly_at_threshold_and_resets() {
        let mut dog = Watchdog::new(3);
        assert!(!dog.record_failure());
        assert!(!dog.record_failure());
        assert!(dog.record_failure()); // third consecutive failure trips
        assert!(!dog.record_failure()); // counter restarted after trip
    }

    #[test]
    fn success_resets_the_counter() {
        let mut dog = Watchdog::new(2);
        assert!(!dog.record_failure());
        dog.record_success();
        assert!(!dog.record_failure()); // would have tripped without the success
        assert!(dog.record_failure());
    }
}
