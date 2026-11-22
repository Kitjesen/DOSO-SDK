use std::collections::VecDeque;

use anyhow::{bail, Result};
use doso_sdk::JointCommand;

use crate::config::{DeployConfig, NUM_JOINTS};

pub const SINGLE_OBS_DIM: usize = 57;

/// Computes gravity unit vector `[0, 0, -1]` expressed in the body frame from Hamilton quaternion `(w, x, y, z)`.
pub fn projected_gravity_from_quat_wxyz(w: f64, x: f64, y: f64, z: f64) -> Option<[f64; 3]> {
    if ![w, x, y, z].iter().all(|v| v.is_finite()) {
        return None;
    }
    let norm = (w * w + x * x + y * y + z * z).sqrt();
    if norm < 1e-6 {
        return None;
    }
    let (w, x, y, z) = (w / norm, x / norm, y / norm, z / norm);
    let gx = 2.0 * (w * y - x * z);
    let gy = -2.0 * (w * x + y * z);
    let gz = -1.0 + 2.0 * (x * x + y * y);
    Some([gx, gy, gz])
}

#[derive(Debug, Clone)]
pub struct ObservationBuilder {
    config: DeployConfig,
    history: VecDeque<[f32; SINGLE_OBS_DIM]>,
}

impl ObservationBuilder {
    pub fn new(config: DeployConfig) -> Self {
        let cap = config.history_length.max(1);
        Self {
            config,
            history: VecDeque::with_capacity(cap),
        }
    }

    pub fn reset(&mut self) {
        self.history.clear();
    }

    pub fn config(&self) -> &DeployConfig {
        &self.config
    }

    pub fn push_and_build(
        &mut self,
        ang_vel_body: [f64; 3],
        proj_gravity: [f64; 3],
        cmd_vel: [f64; 3],
        q_hw: &[f64; NUM_JOINTS],
        dq_hw: &[f64; NUM_JOINTS],
        last_action_policy: &[f32; NUM_JOINTS],
    ) -> Result<Vec<f32>> {
        if !ang_vel_body.iter().all(|v| v.is_finite())
            || !proj_gravity.iter().all(|v| v.is_finite())
            || !cmd_vel.iter().all(|v| v.is_finite())
            || !q_hw.iter().all(|v| v.is_finite())
            || !dq_hw.iter().all(|v| v.is_finite())
            || !last_action_policy.iter().all(|v| v.is_finite())
        {
            bail!("observation inputs contain non-finite value");
        }

        let mut frame = [0.0_f32; SINGLE_OBS_DIM];
        let scales = &self.config.obs_scales;

        frame[0] = (ang_vel_body[0] * scales.ang_vel) as f32;
        frame[1] = (ang_vel_body[1] * scales.ang_vel) as f32;
        frame[2] = (ang_vel_body[2] * scales.ang_vel) as f32;

        frame[3] = proj_gravity[0] as f32;
        frame[4] = proj_gravity[1] as f32;
        frame[5] = proj_gravity[2] as f32;

        frame[6] = (cmd_vel[0] * scales.lin_vel) as f32;
        frame[7] = (cmd_vel[1] * scales.lin_vel) as f32;
        frame[8] = (cmd_vel[2] * scales.ang_vel) as f32;

        for p_idx in 0..NUM_JOINTS {
            let hw_idx = self.config.joint_ids_map[p_idx];
            let pos_delta = if self.config.is_wheel(hw_idx) {
                0.0
            } else {
                q_hw[hw_idx] - self.config.default_joint_pos[hw_idx]
            };
            frame[9 + p_idx] = (pos_delta * scales.dof_pos) as f32;
            frame[25 + p_idx] = (dq_hw[hw_idx] * scales.dof_vel) as f32;
            frame[41 + p_idx] = last_action_policy[p_idx];
        }

        if self.history.is_empty() {
            for _ in 0..self.config.history_length {
                self.history.push_back(frame);
            }
        } else {
            while self.history.len() >= self.config.history_length {
                self.history.pop_front();
            }
            self.history.push_back(frame);
        }

        let mut out = Vec::with_capacity(self.config.obs_dim());
        for slot in &self.history {
            out.extend_from_slice(slot);
        }
        Ok(out)
    }
}

/// Converts 16 policy-order actions into hardware-order `JointCommand` frames (leg PD + wheel velocity).
pub fn policy_actions_to_joint_commands(
    raw_policy_action: &[f32],
    config: &DeployConfig,
) -> Result<([f32; NUM_JOINTS], Vec<JointCommand>)> {
    if raw_policy_action.len() != NUM_JOINTS {
        bail!(
            "expected {} policy actions, got {}",
            NUM_JOINTS,
            raw_policy_action.len()
        );
    }
    if raw_policy_action.iter().any(|v| !v.is_finite()) {
        bail!("policy produced non-finite action");
    }

    let mut clipped_policy = [0.0_f32; NUM_JOINTS];
    let mut hw_cmds = vec![JointCommand::damping(0.0); NUM_JOINTS];
    let clip = config.action_clip;

    for (p_idx, &raw) in raw_policy_action.iter().enumerate() {
        let clamped = (raw as f64).clamp(-clip, clip);
        clipped_policy[p_idx] = clamped as f32;
        let hw_idx = config.joint_ids_map[p_idx];
        hw_cmds[hw_idx] = if config.is_wheel(hw_idx) {
            JointCommand::wheel_vel(
                clamped * config.action_scale[hw_idx],
                config.damping[hw_idx],
            )
        } else {
            JointCommand::leg_pd(
                config.default_joint_pos[hw_idx] + clamped * config.action_scale[hw_idx],
                config.stiffness[hw_idx],
                config.damping[hw_idx],
            )
        };
    }

    Ok((clipped_policy, hw_cmds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projected_gravity_identity_inverted_and_tilted() {
        let id = projected_gravity_from_quat_wxyz(1.0, 0.0, 0.0, 0.0).unwrap();
        assert!((id[2] - (-1.0)).abs() < 1e-6);

        let inv = projected_gravity_from_quat_wxyz(0.0, 1.0, 0.0, 0.0).unwrap();
        assert!((inv[2] - 1.0).abs() < 1e-6);

        let angle = std::f64::consts::FRAC_PI_4;
        let half = angle * 0.5;
        let tilt = projected_gravity_from_quat_wxyz(half.cos(), half.sin(), 0.0, 0.0).unwrap();
        assert!(((-tilt[2]) - angle.cos()).abs() < 1e-6);
        assert!(projected_gravity_from_quat_wxyz(0.0, 0.0, 0.0, 0.0).is_none());
    }

    #[test]
    fn wheel_pos_delta_zeroed_and_permutation_applied() {
        let mut cfg = DeployConfig {
            history_length: 2,
            ..DeployConfig::default()
        };
        // Swap policy indices 0 and 12 (first hip vs first wheel).
        cfg.joint_ids_map.swap(0, 12);
        cfg.validate().unwrap();

        let mut builder = ObservationBuilder::new(cfg.clone());
        let mut q = cfg.default_joint_pos;
        q[0] += 0.2;
        q[12] = 42.0; // Large wrapped wheel angle must be zeroed.
        let dq = [2.0; 16];
        let mut last = [0.0_f32; 16];
        last[0] = 0.5;

        let obs = builder
            .push_and_build(
                [0.4, 0.0, 0.0],
                [0.0, 0.0, -1.0],
                [0.5, 0.0, 0.0],
                &q,
                &dq,
                &last,
            )
            .unwrap();
        assert_eq!(obs.len(), 114);
        // Policy index 0 maps to hardware wheel 12 -> dof_pos residual must be 0.
        assert_eq!(obs[9], 0.0);
        // Policy index 12 maps to hardware hip 0 -> residual = 0.2 * 1.0.
        assert!((obs[9 + 12] - 0.2).abs() < 1e-5);

        let mut action = [0.0_f32; 16];
        action[0] = 1.0; // wheel hw 12 -> scale 5.0 rad/s
        action[12] = 1.0; // leg hw 0 -> scale 0.25 rad
        let (_, cmds) = policy_actions_to_joint_commands(&action, &cfg).unwrap();
        assert_eq!(cmds[12].kp, 0.0);
        assert!((cmds[12].dq - 5.0).abs() < 1e-6);
        assert_eq!(cmds[0].kp, 35.0);
        assert!((cmds[0].q - 0.25).abs() < 1e-6);
    }
}
