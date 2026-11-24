use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const NUM_JOINTS: usize = 16;
pub const NUM_LEGS: usize = 12;
pub const NUM_WHEELS: usize = 4;
pub const WHEEL_START_INDEX: usize = 12;

/// Embedded canonical Thunder V4 hardware parameter configuration (`config/thunder_v4.yaml`).
pub const EMBEDDED_THUNDER_V4_YAML: &str = include_str!("../config/thunder_v4.yaml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum LegId {
    Fr = 0,
    Fl = 1,
    Rr = 2,
    Rl = 3,
}

impl LegId {
    pub const ALL: [Self; 4] = [Self::Fr, Self::Fl, Self::Rr, Self::Rl];

    pub const fn from_index(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(Self::Fr),
            1 => Some(Self::Fl),
            2 => Some(Self::Rr),
            3 => Some(Self::Rl),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Fr => "FR",
            Self::Fl => "FL",
            Self::Rr => "RR",
            Self::Rl => "RL",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotorRole {
    Hip,
    Thigh,
    Calf,
    Wheel,
}

impl MotorRole {
    pub const fn for_can_id(can_id: u8) -> Option<Self> {
        match can_id {
            1 => Some(Self::Hip),
            2 => Some(Self::Thigh),
            3 => Some(Self::Calf),
            4 => Some(Self::Wheel),
            _ => None,
        }
    }

    pub const fn group_index(self) -> usize {
        match self {
            Self::Hip => 0,
            Self::Thigh => 1,
            Self::Calf => 2,
            Self::Wheel => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotorModel {
    Rs02,
    Rs04,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MotorLimits {
    pub position_max: f64,
    pub velocity_max: f64,
    pub torque_max: f64,
    pub kp_max: f64,
    pub kd_max: f64,
}

impl MotorLimits {
    pub fn validate(&self, label: &str) -> Result<()> {
        let fields = [
            ("position_max", self.position_max),
            ("velocity_max", self.velocity_max),
            ("torque_max", self.torque_max),
            ("kp_max", self.kp_max),
            ("kd_max", self.kd_max),
        ];
        for (name, v) in fields {
            if !v.is_finite() || v <= 0.0 {
                bail!("motor '{label}' limit '{name}' must be finite and > 0, got {v}");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanBusMap {
    pub fr: String,
    pub fl: String,
    pub rr: String,
    pub rl: String,
}

impl CanBusMap {
    pub fn as_array(&self) -> [&str; 4] {
        [&self.fr, &self.fl, &self.rr, &self.rl]
    }

    pub fn for_leg(&self, leg: LegId) -> &str {
        match leg {
            LegId::Fr => &self.fr,
            LegId::Fl => &self.fl,
            LegId::Rr => &self.rr,
            LegId::Rl => &self.rl,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceEndpoints {
    pub imu_port: String,
    pub imu_baud: u32,
    pub remote_port: String,
    pub joystick_dev: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MotorCatalog {
    pub rs04: MotorLimits,
    pub rs02: MotorLimits,
}

impl MotorCatalog {
    pub const fn limits_for(&self, model: MotorModel) -> MotorLimits {
        match model {
            MotorModel::Rs04 => self.rs04,
            MotorModel::Rs02 => self.rs02,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JointParam {
    pub index: usize,
    pub name: String,
    pub leg: LegId,
    pub can_id: u8,
    pub motor: MotorModel,
    pub role: MotorRole,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RobotPostureParams {
    pub standing_pose: [f64; NUM_JOINTS],
    pub lower_limits: [f64; NUM_JOINTS],
    pub upper_limits: [f64; NUM_JOINTS],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardwareSafetyParams {
    pub min_upright_cos: f64,
    pub max_proprio_age_us: u64,
    pub max_temp_c: f32,
    pub damp_kd_leg: f64,
    pub damp_kd_wheel: f64,
    pub wheel_can_timeout_us: u32,
    pub deploy_action_min: [f64; NUM_JOINTS],
    pub deploy_action_max: [f64; NUM_JOINTS],
}

/// Single source of truth for robot bus topology, device endpoints, motor protocol envelopes,
/// 16-joint mapping, default posture/action limits, and safety thresholds loaded from
/// `config/thunder_v4.yaml` (or `--hardware-config`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RobotHardwareParams {
    pub robot_model: String,
    pub can_buses: CanBusMap,
    pub devices: DeviceEndpoints,
    pub motors: MotorCatalog,
    pub joints: Vec<JointParam>,
    pub posture: RobotPostureParams,
    pub safety: HardwareSafetyParams,
}

static DEFAULT_THUNDER_V4: OnceLock<RobotHardwareParams> = OnceLock::new();
static DEFAULT_THUNDER_V4_LOOKUP: OnceLock<JointLookupTable> = OnceLock::new();

/// O(1) stack-friendly lookup tables derived from a validated [`RobotHardwareParams`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JointLookupTable {
    leg_can_to_flat: [[Option<usize>; 4]; 4],
    flat_to_leg_can: [(LegId, u8); NUM_JOINTS],
    motor_limits: [MotorLimits; NUM_JOINTS],
    can_id_limits: [Option<MotorLimits>; 4],
}

impl JointLookupTable {
    /// Builds compact O(1) lookup arrays from `params` without scanning `params.joints` on every frame.
    pub fn from_params(params: &RobotHardwareParams) -> Result<Self> {
        let mut leg_can_to_flat = [[None; 4]; 4];
        let mut flat_to_leg_can = [(LegId::Fr, 1u8); NUM_JOINTS];
        let mut motor_limits = [params.motors.rs04; NUM_JOINTS];
        let mut can_id_limits = [None; 4];
        let mut seen = [false; NUM_JOINTS];

        for j in &params.joints {
            if j.index >= NUM_JOINTS {
                bail!(
                    "joint '{}' index {} out of range 0..{NUM_JOINTS}",
                    j.name,
                    j.index
                );
            }
            if !(1..=4).contains(&j.can_id) {
                bail!(
                    "joint '{}' has invalid can_id {}; expected 1..=4",
                    j.name,
                    j.can_id
                );
            }
            let leg_slot = j.leg as usize;
            let can_slot = (j.can_id - 1) as usize;
            if leg_can_to_flat[leg_slot][can_slot].is_some() {
                bail!("duplicate ({:?}, can_id={}) mapping", j.leg, j.can_id);
            }
            let limits = params.motors.limits_for(j.motor);
            leg_can_to_flat[leg_slot][can_slot] = Some(j.index);
            flat_to_leg_can[j.index] = (j.leg, j.can_id);
            motor_limits[j.index] = limits;
            if can_id_limits[can_slot].is_none() {
                can_id_limits[can_slot] = Some(limits);
            }
            seen[j.index] = true;
        }

        if seen.iter().any(|&ok| !ok) {
            bail!("hardware params must cover all {NUM_JOINTS} joint indices");
        }

        Ok(Self {
            leg_can_to_flat,
            flat_to_leg_can,
            motor_limits,
            can_id_limits,
        })
    }

    #[inline]
    pub const fn leg_can_to_flat_index(&self, leg: LegId, can_id: u8) -> Option<usize> {
        if can_id < 1 || can_id > 4 {
            return None;
        }
        self.leg_can_to_flat[leg as usize][(can_id - 1) as usize]
    }

    #[inline]
    pub const fn flat_index_to_leg_can(&self, flat_idx: usize) -> Option<(LegId, u8)> {
        if flat_idx >= NUM_JOINTS {
            return None;
        }
        Some(self.flat_to_leg_can[flat_idx])
    }

    #[inline]
    pub const fn motor_limits_for_flat_index(&self, flat_idx: usize) -> Option<MotorLimits> {
        if flat_idx >= NUM_JOINTS {
            return None;
        }
        Some(self.motor_limits[flat_idx])
    }

    #[inline]
    pub const fn motor_limits_for_can_id(&self, can_id: u8) -> Option<MotorLimits> {
        if can_id < 1 || can_id > 4 {
            return None;
        }
        self.can_id_limits[(can_id - 1) as usize]
    }
}

impl RobotHardwareParams {
    /// Returns the validated embedded `config/thunder_v4.yaml` configuration.
    pub fn thunder_v4() -> &'static Self {
        DEFAULT_THUNDER_V4.get_or_init(|| {
            Self::from_yaml_str(EMBEDDED_THUNDER_V4_YAML)
                .expect("embedded config/thunder_v4.yaml must be valid")
        })
    }

    /// Returns the cached O(1) lookup table for `config/thunder_v4.yaml`.
    pub fn thunder_v4_lookup() -> &'static JointLookupTable {
        DEFAULT_THUNDER_V4_LOOKUP.get_or_init(|| {
            JointLookupTable::from_params(Self::thunder_v4())
                .expect("embedded config/thunder_v4.yaml lookup table must be valid")
        })
    }

    pub fn build_lookup_table(&self) -> Result<JointLookupTable> {
        JointLookupTable::from_params(self)
    }

    pub fn from_yaml_str(raw: &str) -> Result<Self> {
        let mut params: Self =
            serde_yaml::from_str(raw).context("failed to parse robot hardware params YAML")?;
        params.validate()?;
        params.joints.sort_by_key(|j| j.index);
        Ok(params)
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read hardware config {}", path.display()))?;
        Self::from_yaml_str(&raw)
            .with_context(|| format!("invalid hardware config {}", path.display()))
    }

    pub fn load_or_default(path: Option<&Path>) -> Result<Self> {
        match path {
            Some(p) => Self::from_file(p),
            None => Ok(Self::thunder_v4().clone()),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.robot_model.trim().is_empty() {
            bail!("robot_model must not be empty");
        }

        let buses = self.can_buses.as_array();
        let mut seen_buses = HashSet::with_capacity(4);
        for bus in buses {
            let trimmed = bus.trim();
            if trimmed.is_empty() {
                bail!("can_buses interface name must not be empty");
            }
            if !seen_buses.insert(trimmed) {
                bail!("duplicate CAN interface '{trimmed}' in can_buses");
            }
        }

        if self.devices.imu_port.trim().is_empty()
            || self.devices.remote_port.trim().is_empty()
            || self.devices.joystick_dev.trim().is_empty()
        {
            bail!("device endpoints (imu_port, remote_port, joystick_dev) must not be empty");
        }
        if !matches!(self.devices.imu_baud, 115_200 | 460_800 | 921_600) {
            bail!(
                "unsupported devices.imu_baud {}; expected 115200, 460800, or 921600",
                self.devices.imu_baud
            );
        }

        self.motors.rs04.validate("rs04")?;
        self.motors.rs02.validate("rs02")?;

        if self.joints.len() != NUM_JOINTS {
            bail!(
                "joints must contain exactly {NUM_JOINTS} entries, got {}",
                self.joints.len()
            );
        }

        let mut seen_idx = [false; NUM_JOINTS];
        let mut seen_leg_can = [[false; 4]; 4];
        let mut seen_names = HashSet::with_capacity(NUM_JOINTS);

        for j in &self.joints {
            if j.index >= NUM_JOINTS {
                bail!(
                    "joint '{}' index {} out of range 0..{NUM_JOINTS}",
                    j.name,
                    j.index
                );
            }
            if seen_idx[j.index] {
                bail!("duplicate joint index {}", j.index);
            }
            seen_idx[j.index] = true;

            let name = j.name.trim();
            if name.is_empty() || !seen_names.insert(name) {
                bail!("invalid or duplicate joint name '{}'", j.name);
            }

            if !(1..=4).contains(&j.can_id) {
                bail!(
                    "joint '{}' has invalid can_id {}; expected 1..=4",
                    j.name,
                    j.can_id
                );
            }
            let leg_slot = j.leg as usize;
            let can_slot = (j.can_id - 1) as usize;
            if seen_leg_can[leg_slot][can_slot] {
                bail!(
                    "duplicate (leg={:?}, can_id={}) mapping on joint '{}'",
                    j.leg,
                    j.can_id,
                    j.name
                );
            }
            seen_leg_can[leg_slot][can_slot] = true;

            let is_wheel_idx = j.index >= WHEEL_START_INDEX;
            let is_wheel_role = j.role == MotorRole::Wheel;
            if is_wheel_idx != is_wheel_role {
                bail!(
                    "joint '{}' index {} and role {:?} disagree on wheel range (>= {WHEEL_START_INDEX})",
                    j.name,
                    j.index,
                    j.role
                );
            }
            if is_wheel_role && j.can_id != 4 {
                bail!(
                    "wheel joint '{}' must have can_id == 4, got {}",
                    j.name,
                    j.can_id
                );
            }
            if !is_wheel_role && j.can_id == 4 {
                bail!("leg joint '{}' must have can_id in 1..=3, got 4", j.name);
            }
        }

        let p = &self.posture;
        for i in 0..NUM_JOINTS {
            let sp = p.standing_pose[i];
            let lo = p.lower_limits[i];
            let hi = p.upper_limits[i];
            if !sp.is_finite() || !lo.is_finite() || !hi.is_finite() {
                bail!(
                    "posture values must be finite at joint {i} (standing_pose={sp}, lower_limits={lo}, upper_limits={hi})"
                );
            }
            if lo >= hi {
                bail!("posture.lower_limits[{i}] ({lo}) must be < upper_limits[{i}] ({hi})");
            }
            if i < WHEEL_START_INDEX && (sp < lo || sp > hi) {
                bail!(
                    "posture.standing_pose[{i}] ({sp}) outside [lower_limits, upper_limits] [{lo}, {hi}]"
                );
            }
        }

        let s = &self.safety;
        if !s.min_upright_cos.is_finite() || s.min_upright_cos <= 0.0 || s.min_upright_cos > 1.0 {
            bail!(
                "safety.min_upright_cos must be in (0, 1], got {}",
                s.min_upright_cos
            );
        }
        if s.max_proprio_age_us == 0 {
            bail!("safety.max_proprio_age_us must be > 0");
        }
        if !s.max_temp_c.is_finite() || s.max_temp_c <= 0.0 {
            bail!(
                "safety.max_temp_c must be finite and > 0, got {}",
                s.max_temp_c
            );
        }
        if !s.damp_kd_leg.is_finite() || s.damp_kd_leg < 0.0 {
            bail!(
                "safety.damp_kd_leg must be finite and >= 0, got {}",
                s.damp_kd_leg
            );
        }
        if !s.damp_kd_wheel.is_finite() || s.damp_kd_wheel < 0.0 {
            bail!(
                "safety.damp_kd_wheel must be finite and >= 0, got {}",
                s.damp_kd_wheel
            );
        }
        if s.wheel_can_timeout_us == 0 {
            bail!("safety.wheel_can_timeout_us must be > 0");
        }
        for i in 0..NUM_JOINTS {
            let lo = s.deploy_action_min[i];
            let hi = s.deploy_action_max[i];
            if !lo.is_finite() || !hi.is_finite() || lo > hi {
                bail!("safety.deploy_action bounds invalid at joint {i}: [{lo}, {hi}]");
            }
        }

        Ok(())
    }

    #[inline]
    pub fn joint(&self, flat_idx: usize) -> Option<&JointParam> {
        self.joints
            .get(flat_idx)
            .filter(|j| j.index == flat_idx)
            .or_else(|| self.joints.iter().find(|j| j.index == flat_idx))
    }

    #[inline]
    pub fn flat_index_to_leg_can(&self, flat_idx: usize) -> Option<(LegId, u8)> {
        self.joint(flat_idx).map(|j| (j.leg, j.can_id))
    }

    #[inline]
    pub fn leg_can_to_flat_index(&self, leg: LegId, can_id: u8) -> Option<usize> {
        self.joints
            .iter()
            .find(|j| j.leg == leg && j.can_id == can_id)
            .map(|j| j.index)
    }

    #[inline]
    pub fn motor_model_for_flat_index(&self, flat_idx: usize) -> Option<MotorModel> {
        self.joint(flat_idx).map(|j| j.motor)
    }

    #[inline]
    pub fn motor_limits_for_flat_index(&self, flat_idx: usize) -> Option<MotorLimits> {
        self.motor_model_for_flat_index(flat_idx)
            .map(|m| self.motors.limits_for(m))
    }

    #[inline]
    pub fn motor_limits_for_can_id(&self, can_id: u8) -> Option<MotorLimits> {
        self.joints
            .iter()
            .find(|j| j.can_id == can_id)
            .map(|j| self.motors.limits_for(j.motor))
    }

    pub fn motor_limits_array(&self) -> [MotorLimits; NUM_JOINTS] {
        std::array::from_fn(|i| {
            self.motor_limits_for_flat_index(i)
                .expect("validated RobotHardwareParams has all 16 joints")
        })
    }

    pub fn can_bus_interfaces(&self) -> [&str; 4] {
        self.can_buses.as_array()
    }
}

/// Converts canonical flat 16-joint index (`0..16`) into `(LegId, can_id 1..=4)` using `thunder_v4.yaml`.
#[inline]
pub fn flat_index_to_leg_can(flat_idx: usize) -> Option<(LegId, u8)> {
    RobotHardwareParams::thunder_v4_lookup().flat_index_to_leg_can(flat_idx)
}

/// Inverse of [`flat_index_to_leg_can`] using `thunder_v4.yaml`.
#[inline]
pub fn leg_can_to_flat_index(leg: LegId, can_id: u8) -> Option<usize> {
    RobotHardwareParams::thunder_v4_lookup().leg_can_to_flat_index(leg, can_id)
}

/// Looks up [`MotorLimits`] for a flat 16-joint index using `thunder_v4.yaml`.
#[inline]
pub fn motor_limits_for_flat_index(flat_idx: usize) -> Option<MotorLimits> {
    RobotHardwareParams::thunder_v4_lookup().motor_limits_for_flat_index(flat_idx)
}

/// Looks up [`MotorLimits`] for a RobStride CAN ID (`1..=4`) using `thunder_v4.yaml`.
#[inline]
pub fn motor_limits_for_can_id(can_id: u8) -> Option<MotorLimits> {
    RobotHardwareParams::thunder_v4_lookup().motor_limits_for_can_id(can_id)
}
