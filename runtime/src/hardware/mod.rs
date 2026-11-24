pub mod bench;
pub mod hipnuc;
pub mod remote_dev;
pub mod robostride;
pub mod serial_imu;
pub mod socketcan;
pub mod yunzhuo;

use anyhow::Result;

use crate::hardware::robostride::RawCanFrame;
use crate::params::LegId;
use crate::types::{MitCmd16, ProprioFrame, RemotePadState};

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

/// Strictly decoupled hardware interface. Control and policy modules never touch CAN/serial FDs.
pub trait HalBackend: Send {
    fn name(&self) -> &'static str;

    /// Read latest proprioception snapshot (`ImuState` + 16 `JointFeedback`). Zero heap allocation.
    fn sample_proprio(&mut self, now_us: u64) -> ProprioFrame;

    /// Read latest physical remote state (if present).
    fn sample_remote(&mut self, now_us: u64) -> RemotePadState;

    /// Toggle hardware transmission gate (`false` = 100% read-only shadow listener mode on real dog).
    fn set_tx_enabled(&mut self, enabled: bool);

    fn tx_enabled(&self) -> bool;

    fn enable_motors(&mut self) -> Result<()>;

    fn disable_motors(&mut self, clear_errors: bool) -> Result<()>;

    /// Send 16-joint MIT targets (only invoked by `crate::safety::SafetyGate`).
    fn send_mit_verified(&mut self, cmd: &MitCmd16) -> Result<()>;

    fn calibrate_set_zero(&mut self) -> Result<()> {
        anyhow::bail!("set_zero unsupported on this backend")
    }

    fn motor_disable_confirmed(&self) -> bool {
        true
    }

    fn diagnostics(&self) -> HalDiagnostics;
}

pub use bench::{BenchNullHal, ReplayHal, SimHal};
#[cfg(target_os = "linux")]
pub use remote_dev::LinuxRemoteInputReader;
#[cfg(target_os = "linux")]
pub use serial_imu::SerialImuReader;
pub use serial_imu::{
    parse_imu_baud, resolve_imu_baud_from_env, resolve_imu_baud_from_env_or, ImuSerialState,
    DEFAULT_IMU_BAUD, IMU_STALE_TIMEOUT_US,
};
#[cfg(target_os = "linux")]
pub use socketcan::SocketCanHal;
pub use socketcan::{
    build_can_startup_frames, build_can_startup_frames_with_timeout, build_leg_set_zero_frames,
    parse_can_ifaces_override, partition_mit_cmd_by_leg, partition_mit_cmd_by_leg_with_lookup,
    LegCanBatch, MaintenanceFrameBuf, SocketCanCore, MAX_MAINTENANCE_FRAMES_PER_TICK,
    MOTORS_PER_LEG, RX_MMSG_BATCH,
};
