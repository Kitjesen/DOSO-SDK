use serde::{Deserialize, Serialize};

pub const NUM_JOINTS: usize = 16;
pub const NUM_LEGS: usize = 12;
pub const NUM_WHEELS: usize = 4;
pub const WHEEL_START_INDEX: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
pub enum MotorModel {
    Rs02,
    Rs04,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MotorProtocolLimits {
    pub position_max: f64,
    pub velocity_max: f64,
    pub torque_max: f64,
    pub kp_max: f64,
    pub kd_max: f64,
}

impl MotorModel {
    pub const fn limits(self) -> MotorProtocolLimits {
        match self {
            Self::Rs04 => MotorProtocolLimits {
                position_max: 12.57,
                velocity_max: 15.0,
                torque_max: 120.0,
                kp_max: 5000.0,
                kd_max: 100.0,
            },
            Self::Rs02 => MotorProtocolLimits {
                position_max: 12.57,
                velocity_max: 44.0,
                torque_max: 17.0,
                kp_max: 500.0,
                kd_max: 5.0,
            },
        }
    }
}

/// Canonical Thunder V4 motor model assignment per leg CAN bus (IDs 1..=4).
pub const fn thunder_motor_model_for_can_id(can_id: u8) -> Option<MotorModel> {
    match can_id {
        1..=3 => Some(MotorModel::Rs04),
        4 => Some(MotorModel::Rs02),
        _ => None,
    }
}

/// Converts canonical flat 16-joint index (`[FR_hip..RL_calf, FR_wheel..RL_wheel]`)
/// into `(LegId, can_id 1..=4)`.
///
/// Exact match with `_flatIndexToLegCan` in Dart `real_joint.dart`.
pub const fn flat_index_to_leg_can(flat_idx: usize) -> Option<(LegId, u8)> {
    if flat_idx < NUM_LEGS {
        let Some(leg) = LegId::from_index(flat_idx / 3) else {
            return None;
        };
        let can_id = (flat_idx % 3 + 1) as u8;
        Some((leg, can_id))
    } else if flat_idx < NUM_JOINTS {
        let Some(leg) = LegId::from_index(flat_idx - NUM_LEGS) else {
            return None;
        };
        Some((leg, 4))
    } else {
        None
    }
}

/// Inverse of [`flat_index_to_leg_can`].
pub const fn leg_can_to_flat_index(leg: LegId, can_id: u8) -> Option<usize> {
    let leg_idx = leg as usize;
    match can_id {
        1..=3 => Some(leg_idx * 3 + (can_id as usize - 1)),
        4 => Some(WHEEL_START_INDEX + leg_idx),
        _ => None,
    }
}

pub const fn motor_limits_for_flat_index(flat_idx: usize) -> Option<MotorProtocolLimits> {
    let Some((_, can_id)) = flat_index_to_leg_can(flat_idx) else {
        return None;
    };
    let Some(model) = thunder_motor_model_for_can_id(can_id) else {
        return None;
    };
    Some(model.limits())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_index_bijection_matches_thunder_layout() {
        for idx in 0..NUM_JOINTS {
            let (leg, can_id) = flat_index_to_leg_can(idx).unwrap();
            assert_eq!(leg_can_to_flat_index(leg, can_id), Some(idx));
            if idx < 12 {
                assert_ne!(can_id, 4);
                assert_eq!(thunder_motor_model_for_can_id(can_id), Some(MotorModel::Rs04));
            } else {
                assert_eq!(can_id, 4);
                assert_eq!(thunder_motor_model_for_can_id(can_id), Some(MotorModel::Rs02));
            }
        }
        assert_eq!(flat_index_to_leg_can(16), None);
    }
}
