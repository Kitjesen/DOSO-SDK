use anyhow::{bail, Result};

use crate::policy::BodyHeightSpec;
use crate::types::JointVec16;

/// Smoothstep entry blend (`3t^2 - 2t^3`) matching `blendWalkEntryAction` in `behaviour.dart`.
pub fn blend_walk_entry(
    entry: &JointVec16,
    policy: &JointVec16,
    frame: usize,
    total_frames: usize,
) -> JointVec16 {
    if total_frames == 0 || frame >= total_frames {
        return *policy;
    }
    let linear = (frame as f64 / total_frames as f64).clamp(0.0, 1.0);
    let smooth = linear * linear * (3.0 - 2.0 * linear);
    JointVec16::lerp(entry, policy, smooth)
}

/// Rate-limited body-height controller + 100-frame handover blender matching `body_height_handover.dart`.
#[derive(Debug, Clone)]
pub struct BodyHeightController {
    pub supported: bool,
    pub current_m: f64,
    pub target_m: f64,
    pub min_m: f64,
    pub max_m: f64,
    pub rate_mps: f64,
    handover_frames: usize,
    handover_progress: usize,
    entry_pose: JointVec16,
    entry_kp: JointVec16,
    entry_kd: JointVec16,
}

impl Default for BodyHeightController {
    fn default() -> Self {
        Self {
            supported: false,
            current_m: 0.0,
            target_m: 0.0,
            min_m: 0.25,
            max_m: 0.50,
            rate_mps: 0.05,
            handover_frames: 100,
            handover_progress: 100,
            entry_pose: JointVec16::zero(),
            entry_kp: JointVec16::zero(),
            entry_kd: JointVec16::zero(),
        }
    }
}

impl BodyHeightController {
    pub fn configure_from_spec(&mut self, spec: Option<&BodyHeightSpec>) {
        match spec {
            Some(s) => {
                self.supported = true;
                self.min_m = s.min_m;
                self.max_m = s.max_m;
                self.rate_mps = s.rate_mps;
                self.handover_frames = s.handover_frames;
                if self.target_m < s.min_m || self.target_m > s.max_m {
                    self.current_m = s.default_m;
                    self.target_m = s.default_m;
                }
            }
            None => {
                self.supported = false;
                self.current_m = 0.0;
                self.target_m = 0.0;
            }
        }
    }

    pub fn set_target(&mut self, height_m: f64, grounded: bool) -> Result<()> {
        if !self.supported {
            bail!("Active profile does not support body-height control");
        }
        if !height_m.is_finite() || height_m < self.min_m || height_m > self.max_m {
            bail!(
                "Target body height {:.3}m outside [{:.3}, {:.3}]m",
                height_m,
                self.min_m,
                self.max_m
            );
        }
        self.target_m = height_m;
        if grounded {
            self.current_m = height_m;
        }
        Ok(())
    }

    pub fn start_handover(&mut self, pose: JointVec16, kp: JointVec16, kd: JointVec16) {
        self.entry_pose = pose;
        self.entry_kp = kp;
        self.entry_kd = kd;
        self.handover_progress = 0;
    }

    pub fn reset_handover(&mut self) {
        self.handover_progress = self.handover_frames;
    }

    /// Advances `current_m` toward `target_m` at `rate_mps * dt_s`.
    pub fn tick_height(&mut self, dt_s: f64) -> f64 {
        if !self.supported {
            return 0.0;
        }
        let max_step = self.rate_mps * dt_s;
        let diff = self.target_m - self.current_m;
        self.current_m += diff.clamp(-max_step, max_step);
        self.current_m
    }

    /// Blends `(pose, kp, kd)` smoothly across `handover_frames` ticks.
    pub fn apply_handover(
        &mut self,
        target_pose: JointVec16,
        target_kp: JointVec16,
        target_kd: JointVec16,
    ) -> (JointVec16, JointVec16, JointVec16) {
        if self.handover_frames == 0 || self.handover_progress >= self.handover_frames {
            return (target_pose, target_kp, target_kd);
        }
        let t = (self.handover_progress as f64 / self.handover_frames as f64).clamp(0.0, 1.0);
        let smooth = t * t * (3.0 - 2.0 * t);
        self.handover_progress += 1;
        (
            JointVec16::lerp(&self.entry_pose, &target_pose, smooth),
            JointVec16::lerp(&self.entry_kp, &target_kp, smooth),
            JointVec16::lerp(&self.entry_kd, &target_kd, smooth),
        )
    }
}
