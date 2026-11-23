use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::policy::{LoadedPolicyPackage, PolicyEvalContext};
use crate::types::{JointVec16, MitCmd16, ProprioFrame, WalkCommand};

/// 1-to-1 discriminant match with `brainstem.api.v1.RecoveryPhase` in `recovery.proto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum RecoveryPhase {
    Unknown = 0,
    Disabled = 1,
    Preparing = 2,
    Waiting = 3,
    Recovering = 4,
    Balanced = 5,
    Lowering = 6,
    ProneHold = 7,
    StandUp = 8,
    Standing = 9,
    SitDown = 10,
    Grounded = 11,
    Failed = 12,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoverySnapshot {
    pub availability: i32,
    pub phase: i32,
    pub operation_id: String,
    pub unavailable_reason: String,
    pub selected: bool,
    pub session_token: String,
    pub motor_disable_confirmed: bool,
}

/// Decoupled 11-phase RobotRecovery state machine backed by the declarative policy engine.
pub struct RecoveryRuntime {
    policy_name: Option<String>,
    verified: bool,
    selected: bool,
    phase: RecoveryPhase,
    session_token: String,
    operation_id: String,
    expires_us: u64,
    hold_pose: JointVec16,
    lerp_start_pose: JointVec16,
    upright_ticks: usize,
    recovering_ticks: usize,
    phase_ticks: usize,
    motor_disable_confirmed: bool,
}

impl Default for RecoveryRuntime {
    fn default() -> Self {
        Self::new(None, false)
    }
}

impl RecoveryRuntime {
    pub const LEASE_TIMEOUT_US: u64 = 300_000;
    pub const UPRIGHT_REQUIRED_TICKS: usize = 25;
    pub const RECOVERING_TIMEOUT_TICKS: usize = 500;
    pub const BALANCED_HOLD_TICKS: usize = 15;
    pub const LOWERING_TICKS: usize = 50;
    pub const PRONE_HOLD_TICKS: usize = 15;
    pub const STAND_UP_TICKS: usize = 75;
    pub const SIT_DOWN_TICKS: usize = 50;

    pub fn new(policy_name: Option<String>, verified: bool) -> Self {
        Self {
            policy_name,
            verified,
            selected: false,
            phase: RecoveryPhase::Disabled,
            session_token: String::new(),
            operation_id: String::new(),
            expires_us: 0,
            hold_pose: JointVec16::zero(),
            lerp_start_pose: JointVec16::zero(),
            upright_ticks: 0,
            recovering_ticks: 0,
            phase_ticks: 0,
            motor_disable_confirmed: true,
        }
    }

    pub fn set_policy(&mut self, policy_name: String, verified: bool) {
        self.policy_name = Some(policy_name);
        self.verified = verified;
    }

    pub fn policy_name(&self) -> Option<&str> {
        self.policy_name.as_deref()
    }

    pub fn selected(&self) -> bool {
        self.selected
    }

    pub fn phase(&self) -> RecoveryPhase {
        self.phase
    }

    pub fn validate_op_id(op_id: &str) -> Result<()> {
        if op_id.is_empty()
            || op_id.len() > 80
            || !op_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            bail!("Invalid recovery operation_id '{op_id}' (expected 1..=80 ASCII [A-Za-z0-9_-])");
        }
        Ok(())
    }

    pub fn snapshot(&self, grounded_and_free: bool) -> RecoverySnapshot {
        let (availability, reason) = if self.policy_name.is_none() || !self.verified {
            (1, "recovery_not_configured".to_string())
        } else if self.selected {
            (2, String::new())
        } else if !grounded_and_free {
            (1, "requires_grounded_and_idle".to_string())
        } else {
            (2, String::new())
        };
        RecoverySnapshot {
            availability,
            phase: self.phase as i32,
            operation_id: self.operation_id.clone(),
            unavailable_reason: reason,
            selected: self.selected,
            session_token: self.session_token.clone(),
            motor_disable_confirmed: self.motor_disable_confirmed,
        }
    }

    pub fn enter(&mut self, op_id: &str, grounded_and_free: bool, now_us: u64) -> Result<String> {
        Self::validate_op_id(op_id)?;
        if self.policy_name.is_none() || !self.verified {
            bail!("Recovery model is not configured or verified");
        }
        if self.selected
            && self.operation_id == op_id
            && now_us < self.expires_us
            && self.phase != RecoveryPhase::Failed
        {
            return Ok(self.session_token.clone());
        }
        if self.selected {
            bail!("Recovery session already selected");
        }
        if !grounded_and_free {
            bail!("Recovery requires Grounded state with motors disabled and no active owner");
        }
        self.selected = true;
        self.phase = RecoveryPhase::Preparing;
        self.operation_id = op_id.to_string();
        self.session_token = format!("rec-{now_us:x}");
        self.expires_us = now_us + Self::LEASE_TIMEOUT_US;
        self.hold_pose = JointVec16::zero();
        self.lerp_start_pose = JointVec16::zero();
        self.upright_ticks = 0;
        self.recovering_ticks = 0;
        self.phase_ticks = 0;
        self.motor_disable_confirmed = false;
        Ok(self.session_token.clone())
    }

    pub fn heartbeat(&mut self, token: &str, now_us: u64) -> Result<()> {
        self.authorize(token, now_us)?;
        self.expires_us = now_us + Self::LEASE_TIMEOUT_US;
        Ok(())
    }

    pub fn enable(&mut self, token: &str, current_pos: JointVec16, now_us: u64) -> Result<()> {
        self.authorize(token, now_us)?;
        if self.phase != RecoveryPhase::Preparing {
            bail!(
                "Recovery enable requires Preparing phase (current={:?})",
                self.phase
            );
        }
        self.hold_pose = current_pos.discard_wheels();
        self.phase = RecoveryPhase::Waiting;
        self.phase_ticks = 0;
        self.expires_us = now_us + Self::LEASE_TIMEOUT_US;
        Ok(())
    }

    pub fn start(
        &mut self,
        token: &str,
        op_id: &str,
        pkg: &mut LoadedPolicyPackage,
        now_us: u64,
    ) -> Result<()> {
        self.authorize(token, now_us)?;
        Self::validate_op_id(op_id)?;
        if self.phase != RecoveryPhase::Waiting {
            bail!("Recovery must be in Waiting phase before Start");
        }
        self.operation_id = op_id.to_string();
        pkg.reset();
        self.phase = RecoveryPhase::Recovering;
        self.upright_ticks = 0;
        self.recovering_ticks = 0;
        self.phase_ticks = 0;
        self.expires_us = now_us + Self::LEASE_TIMEOUT_US;
        Ok(())
    }

    pub fn sit_down(&mut self, token: &str, now_us: u64) -> Result<()> {
        self.authorize(token, now_us)?;
        if self.phase != RecoveryPhase::Standing {
            bail!("Recovery sit_down requires Standing phase");
        }
        self.lerp_start_pose = self.hold_pose.discard_wheels();
        self.phase = RecoveryPhase::SitDown;
        self.phase_ticks = 0;
        self.expires_us = now_us + Self::LEASE_TIMEOUT_US;
        Ok(())
    }

    pub fn cancel(&mut self, token: &str) -> Result<()> {
        if !self.selected || token.is_empty() || token != self.session_token {
            bail!("Recovery session mismatch");
        }
        self.phase = RecoveryPhase::Failed;
        self.motor_disable_confirmed = true;
        Ok(())
    }

    pub fn exit(&mut self, token: &str) -> Result<()> {
        if !self.selected || token.is_empty() || token != self.session_token {
            bail!("Recovery session mismatch");
        }
        self.selected = false;
        self.phase = RecoveryPhase::Disabled;
        self.session_token.clear();
        self.operation_id.clear();
        self.upright_ticks = 0;
        self.recovering_ticks = 0;
        self.phase_ticks = 0;
        self.motor_disable_confirmed = true;
        Ok(())
    }

    /// 50Hz tick while selected; returns `Some(MitCmd16)` when motors should be driven, or `Err` on expiry/timeout/fault.
    pub fn tick(
        &mut self,
        proprio: &ProprioFrame,
        pkg: &mut LoadedPolicyPackage,
        now_us: u64,
    ) -> Result<Option<MitCmd16>> {
        if !self.selected || self.phase == RecoveryPhase::Failed {
            return Ok(None);
        }
        if now_us >= self.expires_us {
            self.phase = RecoveryPhase::Failed;
            self.motor_disable_confirmed = true;
            bail!("Recovery session heartbeat expired");
        }

        let manifest = pkg.graph.manifest();
        let rec_kp = JointVec16(manifest.gains.infer_kp).discard_wheels();
        let rec_kd = JointVec16(manifest.gains.infer_kd);
        let stand_kp = JointVec16(manifest.gains.stand_kp).discard_wheels();
        let stand_kd = JointVec16(manifest.gains.stand_kd);
        let sitting_pose = JointVec16(manifest.posture.sitting_pose).discard_wheels();
        let standing_pose = JointVec16(manifest.posture.standing_pose).discard_wheels();

        match self.phase {
            RecoveryPhase::Unknown
            | RecoveryPhase::Disabled
            | RecoveryPhase::Preparing
            | RecoveryPhase::Failed => Ok(None),

            RecoveryPhase::Waiting => {
                let pose = self.hold_pose.discard_wheels();
                Ok(Some(MitCmd16::from_action_and_gains(
                    &pose, &rec_kp, &rec_kd,
                )))
            }

            RecoveryPhase::Recovering => {
                if self.recovering_ticks >= Self::RECOVERING_TIMEOUT_TICKS {
                    self.phase = RecoveryPhase::Failed;
                    self.motor_disable_confirmed = true;
                    bail!(
                        "Recovery timed out after {} ticks (10s)",
                        Self::RECOVERING_TIMEOUT_TICKS
                    );
                }

                let ctx =
                    PolicyEvalContext::from_proprio(proprio, WalkCommand::default(), 0.0, true);
                let step_out = match pkg.step(&ctx) {
                    Ok(out) => out,
                    Err(e) => {
                        self.phase = RecoveryPhase::Failed;
                        self.motor_disable_confirmed = true;
                        return Err(e);
                    }
                };
                let target = step_out.target_action.discard_wheels();
                self.hold_pose = target;
                self.recovering_ticks += 1;

                // Upright stability check: `-projected_gravity[2] >= 0.94` and `gyro_norm_sq <= 0.04` for 25 consecutive ticks.
                let gyro_norm_sq: f64 = proprio.imu.gyro_rad_s.iter().map(|w| w * w).sum();
                let upright = (-proprio.imu.projected_gravity[2]) >= 0.94 && gyro_norm_sq <= 0.04;
                if upright {
                    self.upright_ticks += 1;
                    if self.upright_ticks >= Self::UPRIGHT_REQUIRED_TICKS {
                        self.phase = RecoveryPhase::Balanced;
                        self.phase_ticks = 0;
                    }
                } else {
                    self.upright_ticks = 0;
                }
                Ok(Some(MitCmd16::from_action_and_gains(
                    &target, &rec_kp, &rec_kd,
                )))
            }

            RecoveryPhase::Balanced => {
                let pose = self.hold_pose.discard_wheels();
                self.phase_ticks += 1;
                if self.phase_ticks >= Self::BALANCED_HOLD_TICKS {
                    self.lerp_start_pose = pose;
                    self.phase = RecoveryPhase::Lowering;
                    self.phase_ticks = 0;
                }
                Ok(Some(MitCmd16::from_action_and_gains(
                    &pose, &rec_kp, &rec_kd,
                )))
            }

            RecoveryPhase::Lowering => {
                self.phase_ticks += 1;
                let s = smoothstep(self.phase_ticks, Self::LOWERING_TICKS);
                let pose =
                    JointVec16::lerp(&self.lerp_start_pose, &sitting_pose, s).discard_wheels();
                self.hold_pose = pose;
                if self.phase_ticks >= Self::LOWERING_TICKS {
                    self.phase = RecoveryPhase::ProneHold;
                    self.phase_ticks = 0;
                }
                Ok(Some(MitCmd16::from_action_and_gains(
                    &pose, &rec_kp, &rec_kd,
                )))
            }

            RecoveryPhase::ProneHold => {
                let pose = sitting_pose;
                self.hold_pose = pose;
                self.phase_ticks += 1;
                if self.phase_ticks >= Self::PRONE_HOLD_TICKS {
                    self.lerp_start_pose = pose;
                    self.phase = RecoveryPhase::StandUp;
                    self.phase_ticks = 0;
                }
                Ok(Some(MitCmd16::from_action_and_gains(
                    &pose, &rec_kp, &rec_kd,
                )))
            }

            RecoveryPhase::StandUp => {
                self.phase_ticks += 1;
                let s = smoothstep(self.phase_ticks, Self::STAND_UP_TICKS);
                let pose =
                    JointVec16::lerp(&self.lerp_start_pose, &standing_pose, s).discard_wheels();
                let kp = JointVec16::lerp(&rec_kp, &stand_kp, s).discard_wheels();
                let kd = JointVec16::lerp(&rec_kd, &stand_kd, s);
                self.hold_pose = pose;
                if self.phase_ticks >= Self::STAND_UP_TICKS {
                    self.phase = RecoveryPhase::Standing;
                    self.phase_ticks = 0;
                }
                Ok(Some(MitCmd16::from_action_and_gains(&pose, &kp, &kd)))
            }

            RecoveryPhase::Standing => {
                let pose = standing_pose;
                self.hold_pose = pose;
                Ok(Some(MitCmd16::from_action_and_gains(
                    &pose, &stand_kp, &stand_kd,
                )))
            }

            RecoveryPhase::SitDown => {
                self.phase_ticks += 1;
                let s = smoothstep(self.phase_ticks, Self::SIT_DOWN_TICKS);
                let pose =
                    JointVec16::lerp(&self.lerp_start_pose, &sitting_pose, s).discard_wheels();
                self.hold_pose = pose;
                if self.phase_ticks >= Self::SIT_DOWN_TICKS {
                    self.phase = RecoveryPhase::Grounded;
                    self.phase_ticks = 0;
                }
                Ok(Some(MitCmd16::from_action_and_gains(
                    &pose, &stand_kp, &stand_kd,
                )))
            }

            RecoveryPhase::Grounded => {
                let pose = sitting_pose;
                self.hold_pose = pose;
                Ok(Some(MitCmd16::from_action_and_gains(
                    &pose, &stand_kp, &stand_kd,
                )))
            }
        }
    }

    fn authorize(&self, token: &str, now_us: u64) -> Result<()> {
        if !self.selected || token.is_empty() || token != self.session_token {
            bail!("Recovery session mismatch");
        }
        if now_us >= self.expires_us {
            bail!("Recovery authority expired");
        }
        Ok(())
    }
}

#[inline]
fn smoothstep(step: usize, total: usize) -> f64 {
    if total == 0 {
        return 1.0;
    }
    let t = (step as f64 / total as f64).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
