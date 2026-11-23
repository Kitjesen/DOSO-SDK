use serde::{Deserialize, Serialize};

use crate::kinematics::{NUM_JOINTS, WHEEL_START_INDEX};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct JointVec16(pub [f64; NUM_JOINTS]);

impl Default for JointVec16 {
    fn default() -> Self {
        Self::zero()
    }
}

impl JointVec16 {
    pub const fn zero() -> Self {
        Self([0.0; NUM_JOINTS])
    }

    pub fn is_all_finite(&self) -> bool {
        self.0.iter().all(|v| v.is_finite())
    }

    pub fn discard_wheels(mut self) -> Self {
        for w in &mut self.0[WHEEL_START_INDEX..NUM_JOINTS] {
            *w = 0.0;
        }
        self
    }

    pub fn lerp(a: &Self, b: &Self, t: f64) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self(std::array::from_fn(|i| a.0[i] + (b.0[i] - a.0[i]) * t))
    }

    /// Group-wise scale `[hip, thigh, calf, wheel]` matching `JointsMatrix * (h, t, c, w)`.
    pub fn scale_by_role(&self, group_scales: [f64; 4]) -> Self {
        let mut out = [0.0; NUM_JOINTS];
        for leg in 0..4 {
            out[leg * 3] = self.0[leg * 3] * group_scales[0];
            out[leg * 3 + 1] = self.0[leg * 3 + 1] * group_scales[1];
            out[leg * 3 + 2] = self.0[leg * 3 + 2] * group_scales[2];
            out[WHEEL_START_INDEX + leg] = self.0[WHEEL_START_INDEX + leg] * group_scales[3];
        }
        Self(out)
    }

    /// Group-wise divide `(self - offset) / group_scales`.
    pub fn normalize_from_pose(&self, pose: &Self, group_scales: [f64; 4]) -> Self {
        let mut out = [0.0; NUM_JOINTS];
        for leg in 0..4 {
            out[leg * 3] = (self.0[leg * 3] - pose.0[leg * 3]) / group_scales[0];
            out[leg * 3 + 1] = (self.0[leg * 3 + 1] - pose.0[leg * 3 + 1]) / group_scales[1];
            out[leg * 3 + 2] = (self.0[leg * 3 + 2] - pose.0[leg * 3 + 2]) / group_scales[2];
            out[WHEEL_START_INDEX + leg] =
                (self.0[WHEEL_START_INDEX + leg] - pose.0[WHEEL_START_INDEX + leg]) / group_scales[3];
        }
        Self(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MitTarget {
    pub q: f64,
    pub dq: f64,
    pub kp: f64,
    pub kd: f64,
    pub tau: f64,
}

impl Default for MitTarget {
    fn default() -> Self {
        Self::zero()
    }
}

impl MitTarget {
    pub const fn zero() -> Self {
        Self {
            q: 0.0,
            dq: 0.0,
            kp: 0.0,
            kd: 0.0,
            tau: 0.0,
        }
    }

    pub const fn leg_pd(q: f64, kp: f64, kd: f64) -> Self {
        Self {
            q,
            dq: 0.0,
            kp,
            kd,
            tau: 0.0,
        }
    }

    pub const fn wheel_vel(dq: f64, kd: f64) -> Self {
        Self {
            q: 0.0,
            dq,
            kp: 0.0,
            kd,
            tau: 0.0,
        }
    }

    pub fn is_finite(&self) -> bool {
        self.q.is_finite()
            && self.dq.is_finite()
            && self.kp.is_finite()
            && self.kd.is_finite()
            && self.tau.is_finite()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MitCmd16(pub [MitTarget; NUM_JOINTS]);

impl Default for MitCmd16 {
    fn default() -> Self {
        Self([MitTarget::zero(); NUM_JOINTS])
    }
}

impl MitCmd16 {
    /// Builds canonical Thunder 16-DOF MIT frame from target vector + kp/kd vectors:
    /// indices `0..12` drive leg `position` (`dq=0, tau=0`), indices `12..16` drive wheel `velocity` (`q=0, kp=0, tau=0`).
    pub fn from_action_and_gains(target: &JointVec16, kp: &JointVec16, kd: &JointVec16) -> Self {
        Self(std::array::from_fn(|i| {
            if i < WHEEL_START_INDEX {
                MitTarget::leg_pd(target.0[i], kp.0[i], kd.0[i])
            } else {
                MitTarget::wheel_vel(target.0[i], kd.0[i])
            }
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ImuState {
    pub gyro_rad_s: [f64; 3],
    pub accel_m_s2: [f64; 3],
    pub quat_wxyz: [f64; 4],
    pub projected_gravity: [f64; 3],
    pub timestamp_us: u64,
    pub online: bool,
}

impl Default for ImuState {
    fn default() -> Self {
        Self {
            gyro_rad_s: [0.0; 3],
            accel_m_s2: [0.0, 0.0, 9.80665],
            quat_wxyz: [1.0, 0.0, 0.0, 0.0],
            projected_gravity: [0.0, 0.0, -1.0],
            timestamp_us: 0,
            online: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct JointFeedback {
    pub q: f64,
    pub dq: f64,
    pub tau: f64,
    pub temp_c: f32,
    pub status_code: u8,
    pub error_bits: u8,
    pub vbus_v: f32,
    pub last_seen_us: u64,
    pub online: bool,
}

impl Default for JointFeedback {
    fn default() -> Self {
        Self {
            q: 0.0,
            dq: 0.0,
            tau: 0.0,
            temp_c: 25.0,
            status_code: 0,
            error_bits: 0,
            vbus_v: 48.0,
            last_seen_us: 0,
            online: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProprioFrame {
    pub sequence: u64,
    pub monotonic_us: u64,
    pub imu: ImuState,
    pub joints: [JointFeedback; NUM_JOINTS],
}

impl Default for ProprioFrame {
    fn default() -> Self {
        Self {
            sequence: 0,
            monotonic_us: 0,
            imu: ImuState::default(),
            joints: [JointFeedback::default(); NUM_JOINTS],
        }
    }
}

impl ProprioFrame {
    pub fn positions(&self) -> JointVec16 {
        JointVec16(std::array::from_fn(|i| self.joints[i].q))
    }

    pub fn velocities(&self) -> JointVec16 {
        JointVec16(std::array::from_fn(|i| self.joints[i].dq))
    }

    pub fn online_mask(&self) -> u16 {
        let mut mask = 0u16;
        for (i, j) in self.joints.iter().enumerate() {
            if j.online {
                mask |= 1u16 << i;
            }
        }
        mask
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct WalkCommand {
    pub vx: f64,
    pub vy: f64,
    pub yaw: f64,
}

impl WalkCommand {
    pub fn is_zero(&self) -> bool {
        self.vx.abs() < 1e-6 && self.vy.abs() < 1e-6 && self.yaw.abs() < 1e-6
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct RemotePadState {
    pub active: bool,
    pub lx: f64,
    pub ly: f64,
    pub rx: f64,
    pub ry: f64,
    pub l1: bool,
    pub l2: bool,
    pub r1: bool,
    pub r2: bool,
    pub damp_btn: bool,
    pub timestamp_us: u64,
}
