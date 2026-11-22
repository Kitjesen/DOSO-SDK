use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const NUM_JOINTS: usize = 16;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ObsScales {
    pub lin_vel: f64,
    pub ang_vel: f64,
    pub dof_pos: f64,
    pub dof_vel: f64,
}

impl Default for ObsScales {
    fn default() -> Self {
        Self {
            lin_vel: 2.0,
            ang_vel: 0.25,
            dof_pos: 1.0,
            dof_vel: 0.05,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeployConfig {
    pub policy_path: String,
    pub step_dt: f64,
    pub joint_ids_map: [usize; NUM_JOINTS],
    pub default_joint_pos: [f64; NUM_JOINTS],
    pub stiffness: [f64; NUM_JOINTS],
    pub damping: [f64; NUM_JOINTS],
    pub action_scale: [f64; NUM_JOINTS],
    #[serde(default = "default_action_clip")]
    pub action_clip: f64,
    #[serde(default = "default_wheel_indices")]
    pub wheel_indices: Vec<usize>,
    #[serde(default)]
    pub obs_scales: ObsScales,
    #[serde(default = "default_history_length")]
    pub history_length: usize,
    #[serde(default = "default_fix_stand_duration_s")]
    pub fix_stand_duration_s: f64,
    #[serde(default = "default_max_tilt_cos")]
    pub max_tilt_cos: f64,
    #[serde(default = "default_max_telemetry_age_ms")]
    pub max_telemetry_age_ms: f64,
    #[serde(default = "default_cmd_max_vx")]
    pub cmd_max_vx: f64,
    #[serde(default = "default_cmd_max_vy")]
    pub cmd_max_vy: f64,
    #[serde(default = "default_cmd_max_wz")]
    pub cmd_max_wz: f64,
}

fn default_action_clip() -> f64 {
    10.0
}

fn default_wheel_indices() -> Vec<usize> {
    vec![12, 13, 14, 15]
}

fn default_history_length() -> usize {
    1
}

fn default_fix_stand_duration_s() -> f64 {
    2.0
}

fn default_max_tilt_cos() -> f64 {
    0.5
}

fn default_max_telemetry_age_ms() -> f64 {
    150.0
}

fn default_cmd_max_vx() -> f64 {
    1.5
}

fn default_cmd_max_vy() -> f64 {
    0.8
}

fn default_cmd_max_wz() -> f64 {
    1.5
}

impl Default for DeployConfig {
    fn default() -> Self {
        Self {
            policy_path: "policy.onnx".to_string(),
            step_dt: 0.02,
            joint_ids_map: std::array::from_fn(|i| i),
            default_joint_pos: [
                0.0, 0.8, -1.5, 0.0, 0.8, -1.5, 0.0, 0.8, -1.5, 0.0, 0.8, -1.5, 0.0, 0.0, 0.0, 0.0,
            ],
            stiffness: [
                35.0, 35.0, 35.0, 35.0, 35.0, 35.0, 35.0, 35.0, 35.0, 35.0, 35.0, 35.0, 0.0, 0.0,
                0.0, 0.0,
            ],
            damping: [
                1.2, 1.2, 1.2, 1.2, 1.2, 1.2, 1.2, 1.2, 1.2, 1.2, 1.2, 1.2, 0.6, 0.6, 0.6, 0.6,
            ],
            action_scale: [
                0.25, 0.25, 0.25, 0.25, 0.25, 0.25, 0.25, 0.25, 0.25, 0.25, 0.25, 0.25, 5.0, 5.0,
                5.0, 5.0,
            ],
            action_clip: default_action_clip(),
            wheel_indices: default_wheel_indices(),
            obs_scales: ObsScales::default(),
            history_length: default_history_length(),
            fix_stand_duration_s: default_fix_stand_duration_s(),
            max_tilt_cos: default_max_tilt_cos(),
            max_telemetry_age_ms: default_max_telemetry_age_ms(),
            cmd_max_vx: default_cmd_max_vx(),
            cmd_max_vy: default_cmd_max_vy(),
            cmd_max_wz: default_cmd_max_wz(),
        }
    }
}

impl DeployConfig {
    pub fn from_yaml_str(raw: &str) -> Result<Self> {
        let cfg: Self = serde_yaml::from_str(raw).context("failed to parse deploy.yaml")?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let mut cfg = Self::from_yaml_str(&raw)?;
        let policy = PathBuf::from(&cfg.policy_path);
        if policy.is_relative() {
            if let Some(parent) = path.parent() {
                cfg.policy_path = parent.join(policy).to_string_lossy().into_owned();
            }
        }
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        if self.policy_path.trim().is_empty() {
            bail!("policy_path must not be empty");
        }
        if !self.step_dt.is_finite() || !(0.002..=0.1).contains(&self.step_dt) {
            bail!("step_dt must be within [0.002, 0.1], got {}", self.step_dt);
        }

        let mut seen = [false; NUM_JOINTS];
        for (p_idx, &hw_idx) in self.joint_ids_map.iter().enumerate() {
            if hw_idx >= NUM_JOINTS {
                bail!("joint_ids_map[{p_idx}] = {hw_idx} is out of range 0..16");
            }
            if seen[hw_idx] {
                bail!("joint_ids_map contains duplicate hardware index {hw_idx}");
            }
            seen[hw_idx] = true;
        }

        let mut wheel_seen = [false; NUM_JOINTS];
        for &w in &self.wheel_indices {
            if w >= NUM_JOINTS {
                bail!("wheel index {w} is out of range 0..16");
            }
            if wheel_seen[w] {
                bail!("duplicate wheel index {w}");
            }
            wheel_seen[w] = true;
            if self.stiffness[w] != 0.0 {
                bail!(
                    "wheel joint {w} must have zero stiffness kp=0.0, got {}",
                    self.stiffness[w]
                );
            }
        }

        for i in 0..NUM_JOINTS {
            if !self.default_joint_pos[i].is_finite() {
                bail!("default_joint_pos[{i}] is not finite");
            }
            if !self.stiffness[i].is_finite() || self.stiffness[i] < 0.0 {
                bail!("stiffness[{i}] must be finite and >= 0");
            }
            if !self.damping[i].is_finite() || self.damping[i] < 0.0 {
                bail!("damping[{i}] must be finite and >= 0");
            }
            if !self.action_scale[i].is_finite() || self.action_scale[i] <= 0.0 {
                bail!("action_scale[{i}] must be finite and > 0");
            }
        }

        if !self.action_clip.is_finite() || self.action_clip <= 0.0 {
            bail!("action_clip must be finite and > 0");
        }
        if self.history_length == 0 {
            bail!("history_length must be >= 1");
        }
        if !self.fix_stand_duration_s.is_finite() || self.fix_stand_duration_s < 0.1 {
            bail!("fix_stand_duration_s must be >= 0.1");
        }
        if !self.max_tilt_cos.is_finite() || self.max_tilt_cos <= 0.1 || self.max_tilt_cos >= 1.0 {
            bail!("max_tilt_cos must be in (0.1, 1.0)");
        }
        if !self.max_telemetry_age_ms.is_finite() || self.max_telemetry_age_ms <= 0.0 {
            bail!("max_telemetry_age_ms must be > 0");
        }

        for (label, v) in [
            ("obs_scales.lin_vel", self.obs_scales.lin_vel),
            ("obs_scales.ang_vel", self.obs_scales.ang_vel),
            ("obs_scales.dof_pos", self.obs_scales.dof_pos),
            ("obs_scales.dof_vel", self.obs_scales.dof_vel),
            ("cmd_max_vx", self.cmd_max_vx),
            ("cmd_max_vy", self.cmd_max_vy),
            ("cmd_max_wz", self.cmd_max_wz),
        ] {
            if !v.is_finite() || v < 0.0 {
                bail!("{label} must be finite and >= 0");
            }
        }

        Ok(())
    }

    pub fn policy_to_hw(&self) -> &[usize; NUM_JOINTS] {
        &self.joint_ids_map
    }

    pub fn hw_to_policy(&self) -> [usize; NUM_JOINTS] {
        let mut inv = [0usize; NUM_JOINTS];
        for (p_idx, &hw_idx) in self.joint_ids_map.iter().enumerate() {
            inv[hw_idx] = p_idx;
        }
        inv
    }

    pub fn is_wheel(&self, hw_idx: usize) -> bool {
        self.wheel_indices.contains(&hw_idx)
    }

    pub fn obs_dim(&self) -> usize {
        57 * self.history_length
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_and_validates_example_thunder_16dof_yaml() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join("thunder_16dof_deploy.yaml");
        let cfg = DeployConfig::from_path(&path).unwrap();
        assert_eq!(cfg.obs_dim(), 57);
        assert_eq!(cfg.wheel_indices, vec![12, 13, 14, 15]);
        let sibling_brainstem =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../brainstem");
        if sibling_brainstem.is_dir() {
            assert!(
                Path::new(&cfg.policy_path).exists(),
                "resolved policy_path {} should exist",
                cfg.policy_path
            );
        }
    }

    #[test]
    fn rejects_duplicate_joint_map_and_nonzero_wheel_kp() {
        let mut cfg = DeployConfig::default();
        cfg.joint_ids_map[1] = 0;
        assert!(cfg.validate().is_err());

        let mut cfg = DeployConfig::default();
        cfg.stiffness[12] = 10.0;
        assert!(cfg.validate().is_err());
    }
}
