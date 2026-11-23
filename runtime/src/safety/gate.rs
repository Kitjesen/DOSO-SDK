#![forbid(unsafe_code)]

use anyhow::{bail, Result};

use crate::hardware::HalBackend;
use crate::params::{MotorLimits, RobotHardwareParams, NUM_JOINTS, WHEEL_START_INDEX};
use crate::types::{MitCmd16, MitTarget, ProprioFrame};

/// Final software joint position lower envelope loaded from `config/thunder_v4.yaml`
/// (`safety.deploy_action_min`).
#[inline]
pub fn deploy_action_min() -> &'static [f64; NUM_JOINTS] {
    &RobotHardwareParams::thunder_v4().safety.deploy_action_min
}

/// Final software joint position upper envelope loaded from `config/thunder_v4.yaml`
/// (`safety.deploy_action_max`).
#[inline]
pub fn deploy_action_max() -> &'static [f64; NUM_JOINTS] {
    &RobotHardwareParams::thunder_v4().safety.deploy_action_max
}

#[derive(Debug, Clone)]
pub struct SafetyConfig {
    pub allow_motor_enable: bool,
    /// Minimum `-projected_gravity.z` allowed during enabled walking (`cos(max_tilt)`).
    pub min_upright_cos: f64,
    pub max_proprio_age_us: u64,
    pub max_temp_c: f32,
    pub damp_kd_leg: f64,
    pub damp_kd_wheel: f64,
    pub motor_limits: [MotorLimits; NUM_JOINTS],
    pub deploy_action_min: [f64; NUM_JOINTS],
    pub deploy_action_max: [f64; NUM_JOINTS],
}

impl SafetyConfig {
    /// Builds [`SafetyConfig`] directly from a validated [`RobotHardwareParams`] (`config/thunder_v4.yaml`).
    pub fn from_hardware_params(params: &RobotHardwareParams, allow_motor_enable: bool) -> Self {
        Self {
            allow_motor_enable,
            min_upright_cos: params.safety.min_upright_cos,
            max_proprio_age_us: params.safety.max_proprio_age_us,
            max_temp_c: params.safety.max_temp_c,
            damp_kd_leg: params.safety.damp_kd_leg,
            damp_kd_wheel: params.safety.damp_kd_wheel,
            motor_limits: params.motor_limits_array(),
            deploy_action_min: params.safety.deploy_action_min,
            deploy_action_max: params.safety.deploy_action_max,
        }
    }
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self::from_hardware_params(RobotHardwareParams::thunder_v4(), false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyTripReason {
    NonFiniteCommand,
    WheelNonZeroStiffness,
    HardwareLimitExceeded,
    ProfileBoundExceeded,
    TiltExceeded,
    MotorFaultOrOvertemp,
    StaleProprioception,
}

/// The single unbypassable gateway to `HalBackend::send_mit_verified`.
pub struct SafetyGate {
    cfg: SafetyConfig,
    motor_output_enabled: bool,
    latched_fault: Option<SafetyTripReason>,
    rejected_frames: u64,
}

impl SafetyGate {
    pub fn new(cfg: SafetyConfig) -> Self {
        Self {
            cfg,
            motor_output_enabled: false,
            latched_fault: None,
            rejected_frames: 0,
        }
    }

    pub fn config(&self) -> &SafetyConfig {
        &self.cfg
    }

    pub fn set_allow_motor_enable(&mut self, allow: bool) {
        self.cfg.allow_motor_enable = allow;
    }

    pub fn motor_output_enabled(&self) -> bool {
        self.motor_output_enabled
    }

    pub fn latched_fault(&self) -> Option<SafetyTripReason> {
        self.latched_fault
    }

    pub fn rejected_frames(&self) -> u64 {
        self.rejected_frames
    }

    pub fn clear_fault(&mut self, hal: &mut dyn HalBackend) -> Result<()> {
        self.latched_fault = None;
        hal.disable_motors(true)
    }

    pub fn enable_motors(&mut self, hal: &mut dyn HalBackend) -> Result<()> {
        if let Some(reason) = self.latched_fault {
            bail!("Motor enable refused: latched fault {reason:?}");
        }
        if !self.cfg.allow_motor_enable {
            hal.disable_motors(false)?;
            bail!("Motor enable refused: allow_motor_enable=false");
        }
        hal.enable_motors()?;
        self.motor_output_enabled = true;
        Ok(())
    }

    pub fn disable_motors(&mut self, hal: &mut dyn HalBackend, clear_errors: bool) -> Result<()> {
        self.motor_output_enabled = false;
        hal.disable_motors(clear_errors)
    }

    /// Strict 5-rule validation matching `RealJoint.validateExternalTargets` + wheel `kp == 0` invariant,
    /// using explicit per-joint [`MotorLimits`].
    pub fn validate_mit_frame_with_limits(
        cmd: &MitCmd16,
        leg_bounds: Option<(&[f64; NUM_JOINTS], &[f64; NUM_JOINTS])>,
        motor_limits: &[MotorLimits; NUM_JOINTS],
    ) -> std::result::Result<(), SafetyTripReason> {
        for (i, t) in cmd.0.iter().enumerate() {
            if !t.is_finite() || t.kp < 0.0 || t.kd < 0.0 {
                return Err(SafetyTripReason::NonFiniteCommand);
            }
            if i >= WHEEL_START_INDEX && t.kp != 0.0 {
                return Err(SafetyTripReason::WheelNonZeroStiffness);
            }
            let limits = motor_limits[i];
            if t.q.abs() > limits.position_max
                || t.dq.abs() > limits.velocity_max
                || t.tau.abs() > limits.torque_max
                || t.kp > limits.kp_max
                || t.kd > limits.kd_max
            {
                return Err(SafetyTripReason::HardwareLimitExceeded);
            }
            if i < WHEEL_START_INDEX && t.kp > 0.0 {
                if let Some((low, high)) = leg_bounds {
                    if t.q < low[i] || t.q > high[i] {
                        return Err(SafetyTripReason::ProfileBoundExceeded);
                    }
                }
            }
        }
        Ok(())
    }

    /// Validates `cmd` against the embedded canonical Thunder V4 hardware limits.
    pub fn validate_mit_frame(
        cmd: &MitCmd16,
        leg_bounds: Option<(&[f64; NUM_JOINTS], &[f64; NUM_JOINTS])>,
    ) -> std::result::Result<(), SafetyTripReason> {
        let limits = RobotHardwareParams::thunder_v4().motor_limits_array();
        Self::validate_mit_frame_with_limits(cmd, leg_bounds, &limits)
    }

    /// Validates `cmd` against this gate's configured hardware [`MotorLimits`].
    pub fn validate_frame(
        &self,
        cmd: &MitCmd16,
        leg_bounds: Option<(&[f64; NUM_JOINTS], &[f64; NUM_JOINTS])>,
    ) -> std::result::Result<(), SafetyTripReason> {
        Self::validate_mit_frame_with_limits(cmd, leg_bounds, &self.cfg.motor_limits)
    }

    /// Checks proprioception health; if violated while enabled, trips into protective damping.
    pub fn check_proprio_health(
        &self,
        proprio: &ProprioFrame,
        now_us: u64,
        enforce_tilt: bool,
    ) -> std::result::Result<(), SafetyTripReason> {
        if !proprio.imu.online
            || now_us.saturating_sub(proprio.imu.timestamp_us) > self.cfg.max_proprio_age_us
        {
            return Err(SafetyTripReason::StaleProprioception);
        }
        for j in &proprio.joints {
            if !j.online || now_us.saturating_sub(j.last_seen_us) > self.cfg.max_proprio_age_us {
                return Err(SafetyTripReason::StaleProprioception);
            }
            if j.error_bits != 0 || j.temp_c > self.cfg.max_temp_c {
                return Err(SafetyTripReason::MotorFaultOrOvertemp);
            }
        }
        if enforce_tilt && (-proprio.imu.projected_gravity[2]) < self.cfg.min_upright_cos {
            return Err(SafetyTripReason::TiltExceeded);
        }
        Ok(())
    }

    /// Pure passive damping frame (`kp=0`, legs hold current q with `kd=damp_kd_leg`, wheels `dq=0, kd=damp_kd_wheel`).
    pub fn passive_damping_frame(&self, proprio: &ProprioFrame) -> MitCmd16 {
        MitCmd16(std::array::from_fn(|i| {
            if i < WHEEL_START_INDEX {
                MitTarget::leg_pd(proprio.joints[i].q, 0.0, self.cfg.damp_kd_leg)
            } else {
                MitTarget::wheel_vel(0.0, self.cfg.damp_kd_wheel)
            }
        }))
    }

    /// Validates `cmd` and dispatches to `hal`. On any safety violation while enabled,
    /// immediately sends a passive damping frame and latches the trip reason!
    pub fn filter_and_dispatch(
        &mut self,
        hal: &mut dyn HalBackend,
        proprio: &ProprioFrame,
        now_us: u64,
        enforce_tilt: bool,
        leg_bounds: Option<(&[f64; NUM_JOINTS], &[f64; NUM_JOINTS])>,
        cmd: &MitCmd16,
    ) -> std::result::Result<(), SafetyTripReason> {
        if !self.motor_output_enabled {
            return Ok(());
        }
        if let Some(latched) = self.latched_fault {
            let damp = self.passive_damping_frame(proprio);
            let _ = hal.send_mit_verified(&damp);
            return Err(latched);
        }
        if let Err(reason) = self.check_proprio_health(proprio, now_us, enforce_tilt) {
            self.trip_into_damp(hal, proprio, reason);
            return Err(reason);
        }
        if let Err(reason) = self.validate_frame(cmd, leg_bounds) {
            self.trip_into_damp(hal, proprio, reason);
            return Err(reason);
        }
        let _ = hal.send_mit_verified(cmd);
        Ok(())
    }

    fn trip_into_damp(
        &mut self,
        hal: &mut dyn HalBackend,
        proprio: &ProprioFrame,
        reason: SafetyTripReason,
    ) {
        self.rejected_frames += 1;
        self.latched_fault = Some(reason);
        let damp = self.passive_damping_frame(proprio);
        let _ = hal.send_mit_verified(&damp);
        let _ = hal.disable_motors(false);
        self.motor_output_enabled = false;
    }
}
