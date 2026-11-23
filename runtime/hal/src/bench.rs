use anyhow::Result;
use doso_core::{ImuState, JointFeedback, MitCmd16, ProprioFrame, RemotePadState, NUM_JOINTS};

use crate::traits::{HalBackend, HalDiagnostics};

/// Safe zero-hardware benchtop backend (used on S100P `.13` test board or CI).
/// Keeps synthetic fresh proprioception at nominal standing/grounded pose while tracking commands in memory.
pub struct BenchNullHal {
    seq: u64,
    tx_enabled: bool,
    motors_enabled: bool,
    positions: [f64; NUM_JOINTS],
    velocities: [f64; NUM_JOINTS],
    last_cmd: MitCmd16,
    tx_frames: u64,
}

impl Default for BenchNullHal {
    fn default() -> Self {
        Self::new(false)
    }
}

impl BenchNullHal {
    pub fn new(tx_enabled: bool) -> Self {
        Self {
            seq: 0,
            tx_enabled,
            motors_enabled: false,
            positions: [0.0; NUM_JOINTS],
            velocities: [0.0; NUM_JOINTS],
            last_cmd: MitCmd16::default(),
            tx_frames: 0,
        }
    }

    pub fn last_cmd(&self) -> &MitCmd16 {
        &self.last_cmd
    }
}

impl HalBackend for BenchNullHal {
    fn name(&self) -> &'static str {
        "bench-null"
    }

    fn sample_proprio(&mut self, now_us: u64) -> ProprioFrame {
        self.seq = self.seq.wrapping_add(1);
        let joints = std::array::from_fn(|i| JointFeedback {
            q: self.positions[i],
            dq: self.velocities[i],
            tau: 0.0,
            temp_c: 28.0,
            status_code: if self.motors_enabled { 2 } else { 0 },
            error_bits: 0,
            vbus_v: 50.4,
            last_seen_us: now_us,
            online: true,
        });
        ProprioFrame {
            sequence: self.seq,
            monotonic_us: now_us,
            imu: ImuState {
                gyro_rad_s: [0.0, 0.0, 0.0],
                accel_m_s2: [0.0, 0.0, 9.80665],
                quat_wxyz: [1.0, 0.0, 0.0, 0.0],
                projected_gravity: [0.0, 0.0, -1.0],
                timestamp_us: now_us,
                online: true,
            },
            joints,
        }
    }

    fn sample_remote(&mut self, _now_us: u64) -> RemotePadState {
        RemotePadState::default()
    }

    fn set_tx_enabled(&mut self, enabled: bool) {
        self.tx_enabled = enabled;
    }

    fn tx_enabled(&self) -> bool {
        self.tx_enabled
    }

    fn enable_motors(&mut self) -> Result<()> {
        if self.tx_enabled {
            self.motors_enabled = true;
        }
        Ok(())
    }

    fn disable_motors(&mut self, _clear_errors: bool) -> Result<()> {
        self.motors_enabled = false;
        self.velocities = [0.0; NUM_JOINTS];
        Ok(())
    }

    fn send_mit_verified(&mut self, cmd: &MitCmd16) -> Result<()> {
        self.last_cmd = *cmd;
        if self.tx_enabled && self.motors_enabled {
            // Simple first-order benchtop kinematic tracking so posture & height transitions are observable!
            for i in 0..12 {
                if cmd.0[i].kp > 0.0 {
                    self.positions[i] += (cmd.0[i].q - self.positions[i]) * 0.25;
                }
            }
            for i in 12..16 {
                self.velocities[i] = cmd.0[i].dq;
            }
            self.tx_frames += 16;
        }
        Ok(())
    }

    fn diagnostics(&self) -> HalDiagnostics {
        HalDiagnostics {
            backend_name: self.name(),
            tx_enabled: self.tx_enabled,
            tx_frames: self.tx_frames,
            rx_frames: self.seq * 16,
            tx_dropped: 0,
        }
    }
}

/// Alias for external physics simulator injection.
pub type SimHal = BenchNullHal;
