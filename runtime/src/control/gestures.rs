use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::params::NUM_JOINTS;
use crate::types::JointVec16;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GestureKeyframe {
    pub target_pose: [f64; NUM_JOINTS],
    pub counts: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GestureClip {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub keeps_wheels: bool,
    pub keyframes: Vec<GestureKeyframe>,
}

#[derive(Debug, Clone, Default)]
pub struct GestureCatalog {
    clips: Vec<GestureClip>,
}

impl GestureCatalog {
    /// Installs the 5 built-in Thunder gestures (`bow`, `nod`, `wiggle`, `stretch`, `dance`)
    /// anchored around `standing_pose`, matching `installOnboardGestures` in `gesture.dart`.
    pub fn with_builtins(standing: &[f64; NUM_JOINTS]) -> Self {
        let off = |deltas12: [f64; 12]| -> [f64; NUM_JOINTS] {
            let mut p = *standing;
            for (dst, &d) in p[..12].iter_mut().zip(deltas12.iter()) {
                *dst += d;
            }
            p[12..16].fill(0.0);
            p
        };
        let stand = {
            let mut p = *standing;
            p[12..16].fill(0.0);
            p
        };

        let bow_down = off([
            0.0, -0.35, 0.55, 0.0, 0.35, -0.55, 0.0, -0.1, 0.15, 0.0, 0.1, -0.15,
        ]);
        let nod_down = off([0.0, -0.2, 0.3, 0.0, 0.2, -0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let wiggle_l = off([
            0.22, 0.0, 0.0, 0.22, 0.0, 0.0, -0.22, 0.0, 0.0, -0.22, 0.0, 0.0,
        ]);
        let wiggle_r = off([
            -0.22, 0.0, 0.0, -0.22, 0.0, 0.0, 0.22, 0.0, 0.0, 0.22, 0.0, 0.0,
        ]);
        let stretch_front = off([
            0.0, 0.35, -0.55, 0.0, -0.35, 0.55, 0.0, -0.25, 0.4, 0.0, 0.25, -0.4,
        ]);
        let dance_crouch = off([
            0.0, -0.45, 0.68, 0.0, 0.45, -0.68, 0.0, 0.45, -0.68, 0.0, -0.45, 0.68,
        ]);

        let clips = vec![
            GestureClip {
                name: "bow".into(),
                description: "鞠躬".into(),
                keeps_wheels: false,
                keyframes: vec![
                    GestureKeyframe {
                        target_pose: bow_down,
                        counts: 35,
                    },
                    GestureKeyframe {
                        target_pose: bow_down,
                        counts: 25,
                    },
                    GestureKeyframe {
                        target_pose: stand,
                        counts: 35,
                    },
                ],
            },
            GestureClip {
                name: "nod".into(),
                description: "点头".into(),
                keeps_wheels: false,
                keyframes: vec![
                    GestureKeyframe {
                        target_pose: nod_down,
                        counts: 15,
                    },
                    GestureKeyframe {
                        target_pose: stand,
                        counts: 15,
                    },
                    GestureKeyframe {
                        target_pose: nod_down,
                        counts: 15,
                    },
                    GestureKeyframe {
                        target_pose: stand,
                        counts: 15,
                    },
                ],
            },
            GestureClip {
                name: "wiggle".into(),
                description: "摇摆".into(),
                keeps_wheels: false,
                keyframes: vec![
                    GestureKeyframe {
                        target_pose: wiggle_l,
                        counts: 15,
                    },
                    GestureKeyframe {
                        target_pose: wiggle_r,
                        counts: 20,
                    },
                    GestureKeyframe {
                        target_pose: wiggle_l,
                        counts: 20,
                    },
                    GestureKeyframe {
                        target_pose: stand,
                        counts: 15,
                    },
                ],
            },
            GestureClip {
                name: "stretch".into(),
                description: "伸展".into(),
                keeps_wheels: false,
                keyframes: vec![
                    GestureKeyframe {
                        target_pose: stretch_front,
                        counts: 45,
                    },
                    GestureKeyframe {
                        target_pose: stretch_front,
                        counts: 25,
                    },
                    GestureKeyframe {
                        target_pose: stand,
                        counts: 35,
                    },
                ],
            },
            GestureClip {
                name: "dance".into(),
                description: "跳舞".into(),
                keeps_wheels: false,
                keyframes: vec![
                    GestureKeyframe {
                        target_pose: dance_crouch,
                        counts: 12,
                    },
                    GestureKeyframe {
                        target_pose: wiggle_l,
                        counts: 15,
                    },
                    GestureKeyframe {
                        target_pose: wiggle_r,
                        counts: 15,
                    },
                    GestureKeyframe {
                        target_pose: stand,
                        counts: 25,
                    },
                ],
            },
        ];
        Self { clips }
    }

    pub fn names(&self) -> Vec<String> {
        self.clips.iter().map(|c| c.name.clone()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&GestureClip> {
        self.clips.iter().find(|c| c.name == name)
    }

    pub fn upsert(&mut self, clip: GestureClip) -> Result<()> {
        if clip.name.trim().is_empty() || clip.keyframes.is_empty() {
            bail!("Invalid gesture clip");
        }
        if let Some(existing) = self.clips.iter_mut().find(|c| c.name == clip.name) {
            *existing = clip;
        } else {
            self.clips.push(clip);
        }
        Ok(())
    }
}

/// Active keyframe player matching `Gesture.doing` in `behaviour.dart`.
pub struct GesturePlayer {
    clip: GestureClip,
    kf_idx: usize,
    tick_in_kf: usize,
    segment_start: JointVec16,
}

impl GesturePlayer {
    pub fn start(clip: GestureClip, entry_pose: JointVec16) -> Self {
        let first_tick = if clip.keyframes.first().is_some_and(|k| k.counts > 0) {
            1
        } else {
            0
        };
        Self {
            clip,
            kf_idx: 0,
            tick_in_kf: first_tick,
            segment_start: entry_pose,
        }
    }

    pub fn name(&self) -> &str {
        &self.clip.name
    }

    /// Returns `(pose, finished)`.
    pub fn step(&mut self) -> (JointVec16, bool) {
        if self.kf_idx >= self.clip.keyframes.len() {
            return (self.segment_start, true);
        }
        let kf = &self.clip.keyframes[self.kf_idx];
        let target = JointVec16(kf.target_pose);
        let t = if kf.counts == 0 {
            1.0
        } else {
            (self.tick_in_kf as f64 / kf.counts as f64).clamp(0.0, 1.0)
        };
        let mut blended = JointVec16::lerp(&self.segment_start, &target, t);
        if !self.clip.keeps_wheels {
            blended = blended.discard_wheels();
        }
        if self.tick_in_kf >= kf.counts {
            self.segment_start = target;
            self.kf_idx += 1;
            self.tick_in_kf = if self
                .clip
                .keyframes
                .get(self.kf_idx)
                .is_some_and(|next_kf| next_kf.counts > 0)
            {
                1
            } else {
                0
            };
            let done = self.kf_idx >= self.clip.keyframes.len();
            (blended, done)
        } else {
            self.tick_in_kf += 1;
            (blended, false)
        }
    }
}
