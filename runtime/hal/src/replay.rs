use anyhow::Result;
use doso_core::{flat_index_to_leg_can, LegId, MitCmd16, ProprioFrame, RemotePadState};

use crate::robostride::RsEvent;
use crate::traits::{CanBusFrame, HalBackend, HalDiagnostics};

/// Deterministic offline replay backend for bit-exact regression testing.
pub struct ReplayHal {
    frames: Vec<ProprioFrame>,
    cursor: usize,
    tx_enabled: bool,
    recorded_tx: Vec<CanBusFrame>,
}

impl ReplayHal {
    pub fn new(frames: Vec<ProprioFrame>) -> Self {
        Self {
            frames,
            cursor: 0,
            tx_enabled: true,
            recorded_tx: Vec::new(),
        }
    }

    pub fn recorded_can_tx(&self) -> &[CanBusFrame] {
        &self.recorded_tx
    }
}

impl HalBackend for ReplayHal {
    fn name(&self) -> &'static str {
        "replay"
    }

    fn sample_proprio(&mut self, now_us: u64) -> ProprioFrame {
        if self.frames.is_empty() {
            return ProprioFrame {
                monotonic_us: now_us,
                ..ProprioFrame::default()
            };
        }
        let idx = self.cursor.min(self.frames.len() - 1);
        self.cursor += 1;
        self.frames[idx]
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
        if !self.tx_enabled {
            return Ok(());
        }
        for leg in LegId::ALL {
            for can_id in 1..=4 {
                self.recorded_tx.push(CanBusFrame {
                    leg,
                    frame: RsEvent::enable(can_id).encode()?,
                });
            }
        }
        Ok(())
    }

    fn disable_motors(&mut self, clear_errors: bool) -> Result<()> {
        if !self.tx_enabled {
            return Ok(());
        }
        for leg in LegId::ALL {
            for can_id in 1..=4 {
                self.recorded_tx.push(CanBusFrame {
                    leg,
                    frame: RsEvent::disable(can_id, clear_errors).encode()?,
                });
            }
        }
        Ok(())
    }

    fn send_mit_verified(&mut self, cmd: &MitCmd16) -> Result<()> {
        if !self.tx_enabled {
            return Ok(());
        }
        for (idx, t) in cmd.0.iter().enumerate() {
            let (leg, can_id) = flat_index_to_leg_can(idx).unwrap();
            let frame = RsEvent::Control {
                can_id,
                torque: t.tau,
                position: t.q,
                velocity: t.dq,
                kp: t.kp,
                kd: t.kd,
            }
            .encode()?;
            self.recorded_tx.push(CanBusFrame { leg, frame });
        }
        Ok(())
    }

    fn diagnostics(&self) -> HalDiagnostics {
        HalDiagnostics {
            backend_name: self.name(),
            tx_enabled: self.tx_enabled,
            tx_frames: self.recorded_tx.len() as u64,
            rx_frames: self.cursor as u64,
            tx_dropped: 0,
        }
    }
}
