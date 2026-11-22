use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreshnessDecision {
    Accepted,
    BootMismatch,
    ProducerBootMissing,
    Future,
    Expired,
}

#[derive(Debug, Clone)]
pub struct CmdVelFreshnessSample {
    pub host_boot_id: String,
    pub producer_boot_id: String,
    pub source_boottime_ns: u64,
    pub receive_boottime_ns: u64,
    pub received_at: Instant,
    pub vx: f64,
    pub vy: f64,
    pub wz: f64,
}

/// Cross-board monotonic freshness gate for dual-board (`Board A <-> Board B`) DDS transport.
///
/// Supports either an exact `expected_host_boot_id` or `"*"` in dual-board mode, and
/// estimates sample age across separate Linux kernels without requiring NTP by anchoring a
/// monotonic minimum clock offset per active `producer_boot_id`.
#[derive(Debug)]
pub struct CrossBoardFreshnessGate {
    max_age: Duration,
    expected_host_boot_id: String,
    cross_board_clock: bool,
    anchored_producer_boot_id: Option<String>,
    min_offset_ns: i128,
    last_source_ns: u64,
}

impl CrossBoardFreshnessGate {
    pub fn new(
        max_age: Duration,
        expected_host_boot_id: impl Into<String>,
        cross_board_clock: bool,
    ) -> Self {
        Self {
            max_age,
            expected_host_boot_id: expected_host_boot_id.into(),
            cross_board_clock,
            anchored_producer_boot_id: None,
            min_offset_ns: 0,
            last_source_ns: 0,
        }
    }

    pub fn evaluate(&mut self, sample: &CmdVelFreshnessSample) -> FreshnessDecision {
        let wildcard_host = self.cross_board_clock && self.expected_host_boot_id == "*";
        if !wildcard_host
            && (sample.host_boot_id.is_empty() || sample.host_boot_id != self.expected_host_boot_id)
        {
            return FreshnessDecision::BootMismatch;
        }
        if sample.producer_boot_id.is_empty() {
            return FreshnessDecision::ProducerBootMissing;
        }

        if !self.cross_board_clock {
            if sample.source_boottime_ns > sample.receive_boottime_ns {
                return FreshnessDecision::Future;
            }
            let age_ns = sample.receive_boottime_ns - sample.source_boottime_ns;
            if age_ns >= self.max_age.as_nanos() as u64 {
                return FreshnessDecision::Expired;
            }
            return FreshnessDecision::Accepted;
        }

        let offset_ns =
            i128::from(sample.receive_boottime_ns) - i128::from(sample.source_boottime_ns);
        let producer_changed =
            self.anchored_producer_boot_id.as_deref() != Some(sample.producer_boot_id.as_str());
        if producer_changed {
            self.anchored_producer_boot_id = Some(sample.producer_boot_id.clone());
            self.min_offset_ns = offset_ns;
            self.last_source_ns = sample.source_boottime_ns;
            return FreshnessDecision::Accepted;
        }

        if sample.source_boottime_ns < self.last_source_ns {
            return FreshnessDecision::Expired;
        }

        const ALLOWED_CLOCK_LEAD_NS: i128 = 50_000_000;
        if offset_ns < self.min_offset_ns {
            if self.min_offset_ns - offset_ns > ALLOWED_CLOCK_LEAD_NS {
                return FreshnessDecision::Future;
            }
            self.min_offset_ns = offset_ns;
        }

        let estimated_age_ns = offset_ns - self.min_offset_ns;
        if estimated_age_ns >= self.max_age.as_nanos() as i128 {
            return FreshnessDecision::Expired;
        }
        self.last_source_ns = sample.source_boottime_ns;
        FreshnessDecision::Accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(host: &str, prod: &str, src_ms: u64, rx_ms: u64) -> CmdVelFreshnessSample {
        CmdVelFreshnessSample {
            host_boot_id: host.into(),
            producer_boot_id: prod.into(),
            source_boottime_ns: src_ms * 1_000_000,
            receive_boottime_ns: rx_ms * 1_000_000,
            received_at: Instant::now(),
            vx: 0.3,
            vy: 0.0,
            wz: 0.0,
        }
    }

    #[test]
    fn cross_board_offset_anchors_and_catches_stale_packets() {
        let mut gate = CrossBoardFreshnessGate::new(Duration::from_millis(200), "*", true);
        assert_eq!(
            gate.evaluate(&mk("board-b", "prod-b", 500_000, 20_010)),
            FreshnessDecision::Accepted
        );
        assert_eq!(
            gate.evaluate(&mk("board-b", "prod-b", 500_020, 20_022)),
            FreshnessDecision::Accepted
        );
        assert_eq!(
            gate.evaluate(&mk("board-b", "prod-b", 500_040, 20_260)),
            FreshnessDecision::Expired
        );
        assert_eq!(
            gate.evaluate(&mk("board-b", "prod-b", 500_010, 20_040)),
            FreshnessDecision::Expired
        );
    }

    #[test]
    fn single_board_mode_enforces_exact_boot_id() {
        let mut gate = CrossBoardFreshnessGate::new(Duration::from_millis(200), "host-1", false);
        assert_eq!(
            gate.evaluate(&mk("host-2", "prod-1", 100, 120)),
            FreshnessDecision::BootMismatch
        );
        assert_eq!(
            gate.evaluate(&mk("host-1", "", 100, 120)),
            FreshnessDecision::ProducerBootMissing
        );
        assert_eq!(
            gate.evaluate(&mk("host-1", "prod-1", 100, 120)),
            FreshnessDecision::Accepted
        );
    }
}
