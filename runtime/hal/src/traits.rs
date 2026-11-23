use anyhow::Result;
use doso_core::{LegId, MitCmd16, ProprioFrame, RemotePadState};

use crate::robostride::RawCanFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanBusFrame {
    pub leg: LegId,
    pub frame: RawCanFrame,
}

#[derive(Debug, Clone, Default)]
pub struct HalDiagnostics {
    pub backend_name: &'static str,
    pub tx_enabled: bool,
    pub tx_frames: u64,
    pub rx_frames: u64,
    pub tx_dropped: u64,
}

/// Strictly decoupled hardware interface. Neither `doso-brain` nor `doso-policy` ever sees CAN/serial FDs.
pub trait HalBackend: Send {
    fn name(&self) -> &'static str;

    /// Read latest proprioception snapshot (`ImuState` + 16 `JointFeedback`). Zero heap allocation.
    fn sample_proprio(&mut self, now_us: u64) -> ProprioFrame;

    /// Read latest physical remote state (if present).
    fn sample_remote(&mut self, now_us: u64) -> RemotePadState;

    /// Toggle hardware transmission gate (false = 100% read-only shadow listener mode on real dog!).
    fn set_tx_enabled(&mut self, enabled: bool);

    fn tx_enabled(&self) -> bool;

    fn enable_motors(&mut self) -> Result<()>;

    fn disable_motors(&mut self, clear_errors: bool) -> Result<()>;

    /// Send 16-joint MIT targets (only invoked by `doso-safety::SafetyGate`!).
    fn send_mit_verified(&mut self, cmd: &MitCmd16) -> Result<()>;

    fn calibrate_set_zero(&mut self) -> anyhow::Result<()> {
        anyhow::bail!("set_zero unsupported on this backend")
    }

    fn motor_disable_confirmed(&self) -> bool {
        true
    }

    fn diagnostics(&self) -> HalDiagnostics;
}
