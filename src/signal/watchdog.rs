use tokio::time::{Duration, Instant};

/// Counts handshake failures that happen close together in time. When the
/// count reaches the threshold *within the window* the watchdog "trips":
/// record_failure returns true and the counter resets, so the caller
/// restarts the pipeline exactly once.
///
/// The window matters: without it, a windowless counter trips on N failures
/// no matter how far apart, so a few abandoned client handshakes spread over
/// hours would eventually force a full pipeline restart and drop every
/// established viewer. A failure older than the window no longer counts
/// toward a trip — only a genuine burst (a wedged pipeline failing every
/// handshake) accumulates fast enough to trip.
pub struct Watchdog {
    consecutive_failures: u32,
    threshold: u32,
    window: Duration,
    last_failure: Option<Instant>,
}

impl Watchdog {
    pub fn new(threshold: u32, window: Duration) -> Self {
        Self {
            consecutive_failures: 0,
            threshold,
            window,
            last_failure: None,
        }
    }

    pub fn record_failure(&mut self) -> bool {
        let now = Instant::now();
        // Decay: a failure that arrives more than `window` after the previous
        // one starts the count over — stale failures don't accumulate.
        if let Some(last) = self.last_failure {
            if now.duration_since(last) > self.window {
                self.consecutive_failures = 0;
            }
        }
        self.last_failure = Some(now);
        self.consecutive_failures += 1;
        if self.consecutive_failures >= self.threshold {
            self.consecutive_failures = 0;
            self.last_failure = None;
            true
        } else {
            false
        }
    }

    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.last_failure = None;
    }
}

#[cfg(test)]
mod tests {
    use super::Watchdog;
    use tokio::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn trips_at_threshold_within_window_and_resets() {
        let mut dog = Watchdog::new(3, Duration::from_secs(60));
        assert!(!dog.record_failure());
        assert!(!dog.record_failure());
        assert!(dog.record_failure()); // third failure within the window trips
        assert!(!dog.record_failure()); // counter restarted after trip
    }

    #[tokio::test(start_paused = true)]
    async fn success_resets_the_counter() {
        let mut dog = Watchdog::new(2, Duration::from_secs(60));
        assert!(!dog.record_failure());
        dog.record_success();
        assert!(!dog.record_failure()); // would have tripped without the success
        assert!(dog.record_failure());
    }

    #[tokio::test(start_paused = true)]
    async fn failures_spread_beyond_the_window_never_accumulate() {
        let mut dog = Watchdog::new(3, Duration::from_secs(60));
        // Three failures, each more than a window apart: the counter decays
        // to zero between them, so it never reaches the threshold.
        for _ in 0..3 {
            assert!(!dog.record_failure());
            tokio::time::advance(Duration::from_secs(61)).await;
        }
        // A final failure still counts as the first in a fresh window.
        assert!(!dog.record_failure());
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_within_the_window_still_trips() {
        let mut dog = Watchdog::new(3, Duration::from_secs(60));
        assert!(!dog.record_failure());
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(!dog.record_failure());
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(dog.record_failure()); // all three inside 60s
    }
}
