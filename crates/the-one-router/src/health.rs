use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderStatus {
    Healthy,
    Unhealthy,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct ProviderHealth {
    pub status: ProviderStatus,
    pub last_check_epoch_ms: u64,
    pub consecutive_failures: u32,
    pub cooldown_until_epoch_ms: u64,
    pub latency_samples: Vec<u64>,
    pub total_calls: u64,
    pub total_errors: u64,
}

impl ProviderHealth {
    pub fn new() -> Self {
        Self {
            status: ProviderStatus::Unknown,
            last_check_epoch_ms: 0,
            consecutive_failures: 0,
            cooldown_until_epoch_ms: 0,
            latency_samples: Vec::new(),
            total_calls: 0,
            total_errors: 0,
        }
    }

    /// Median latency of recent samples, or 0 if no samples.
    pub fn p50_latency_ms(&self) -> u64 {
        if self.latency_samples.is_empty() {
            return 0;
        }
        let mut sorted = self.latency_samples.clone();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    }

    /// Record a successful call. Resets to healthy, clears cooldown.
    pub fn record_success(&mut self, latency_ms: u64) {
        self.status = ProviderStatus::Healthy;
        self.consecutive_failures = 0;
        self.cooldown_until_epoch_ms = 0;
        self.total_calls += 1;
        self.last_check_epoch_ms = now_epoch_ms();
        // Rolling window: keep last 20 samples
        self.latency_samples.push(latency_ms);
        if self.latency_samples.len() > 20 {
            self.latency_samples.remove(0);
        }
    }

    /// Record a failed call. Increments failures, sets cooldown.
    pub fn record_failure(&mut self) {
        self.status = ProviderStatus::Unhealthy;
        self.consecutive_failures += 1;
        self.total_calls += 1;
        self.total_errors += 1;
        self.last_check_epoch_ms = now_epoch_ms();
        let cooldown = Self::cooldown_duration_ms(self.consecutive_failures);
        self.cooldown_until_epoch_ms = now_epoch_ms() + cooldown;
    }

    /// Is this provider available for use right now?
    pub fn is_available(&self) -> bool {
        match self.status {
            ProviderStatus::Unhealthy => {
                // Check if cooldown has expired
                now_epoch_ms() >= self.cooldown_until_epoch_ms
            }
            _ => true, // Healthy or Unknown are available
        }
    }

    /// Cooldown duration based on consecutive failure count.
    fn cooldown_duration_ms(consecutive_failures: u32) -> u64 {
        match consecutive_failures {
            0 => 0,
            1 => 5_000,
            2 => 15_000,
            _ => 60_000,
        }
    }
}

impl Default for ProviderHealth {
    fn default() -> Self {
        Self::new()
    }
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_provider_is_unknown_and_available() {
        let health = ProviderHealth::new();
        assert_eq!(health.status, ProviderStatus::Unknown);
        assert!(health.is_available());
    }

    #[test]
    fn test_success_sets_healthy_and_tracks_latency() {
        let mut health = ProviderHealth::new();
        health.record_success(42);
        assert_eq!(health.status, ProviderStatus::Healthy);
        assert_eq!(health.latency_samples.len(), 1);
        assert_eq!(health.latency_samples[0], 42);
        assert_eq!(health.total_calls, 1);
    }

    #[test]
    fn test_failure_sets_unhealthy_with_cooldown() {
        let mut health = ProviderHealth::new();
        health.record_failure();
        assert_eq!(health.status, ProviderStatus::Unhealthy);
        assert_eq!(health.consecutive_failures, 1);
        assert!(health.cooldown_until_epoch_ms > 0);
        assert_eq!(health.total_calls, 1);
        assert_eq!(health.total_errors, 1);
    }

    #[test]
    fn test_success_after_failure_resets() {
        let mut health = ProviderHealth::new();
        health.record_failure();
        assert_eq!(health.status, ProviderStatus::Unhealthy);
        assert_eq!(health.consecutive_failures, 1);

        health.record_success(50);
        assert_eq!(health.status, ProviderStatus::Healthy);
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.cooldown_until_epoch_ms, 0);
        assert_eq!(health.total_calls, 2);
        assert_eq!(health.total_errors, 1);
    }

    #[test]
    fn test_p50_latency_calculation() {
        let mut health = ProviderHealth::new();
        // Empty returns 0
        assert_eq!(health.p50_latency_ms(), 0);

        // Single sample
        health.record_success(100);
        assert_eq!(health.p50_latency_ms(), 100);

        // Multiple samples: [100, 200, 300] -> sorted median at index 1 = 200
        health.record_success(200);
        health.record_success(300);
        assert_eq!(health.p50_latency_ms(), 200);

        // Even number: [100, 200, 300, 400] -> sorted median at index 2 = 300
        health.record_success(400);
        assert_eq!(health.p50_latency_ms(), 300);

        // Unsorted input: [200, 300, 400, 50] -> sorted [50, 200, 300, 400] median at index 2 = 300
        health.latency_samples = vec![200, 300, 400, 50];
        assert_eq!(health.p50_latency_ms(), 300);
    }

    #[test]
    fn test_cooldown_progression() {
        // 1 failure = 5s
        assert_eq!(ProviderHealth::cooldown_duration_ms(1), 5_000);
        // 2 failures = 15s
        assert_eq!(ProviderHealth::cooldown_duration_ms(2), 15_000);
        // 3+ failures = 60s
        assert_eq!(ProviderHealth::cooldown_duration_ms(3), 60_000);
        assert_eq!(ProviderHealth::cooldown_duration_ms(10), 60_000);
        // 0 failures = 0
        assert_eq!(ProviderHealth::cooldown_duration_ms(0), 0);
    }

    #[test]
    fn test_rolling_window_caps_at_20() {
        let mut health = ProviderHealth::new();
        for i in 0..25 {
            health.record_success(i as u64);
        }
        assert_eq!(health.latency_samples.len(), 20);
        // Should contain samples 5..=24 (the last 20)
        assert_eq!(health.latency_samples[0], 5);
        assert_eq!(health.latency_samples[19], 24);
    }

    #[test]
    fn test_is_available_respects_cooldown() {
        let mut health = ProviderHealth::new();
        health.record_failure();
        // Cooldown is set in the future, so not available
        assert!(!health.is_available());

        // Manually set cooldown to past to simulate expiry
        health.cooldown_until_epoch_ms = 0;
        assert!(health.is_available());
    }
}
