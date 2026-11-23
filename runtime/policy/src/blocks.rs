use std::fmt;

use anyhow::{bail, Context, Result};
use doso_core::{NUM_JOINTS, WHEEL_START_INDEX};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

pub const THUNDER_V4_STANDING_POSE: [f64; NUM_JOINTS] = [
    -0.12, -0.93, 1.96, 0.12, 0.93, -1.96, 0.12, 0.93, -1.96, -0.12, -0.93, 1.96, 0.0, 0.0, 0.0,
    0.0,
];

pub const THUNDER_V4_LOWER_LIMITS: [f64; NUM_JOINTS] = [
    -0.39, -3.12, 0.02, -0.39, 0.02, -3.12, -0.39, 0.02, -3.12, -0.39, -3.12, -3.12, -35.0, -35.0,
    -35.0, -35.0,
];

pub const THUNDER_V4_UPPER_LIMITS: [f64; NUM_JOINTS] = [
    0.39, -0.02, 3.12, 0.39, 3.12, -0.02, 0.39, 3.12, 3.12, 0.39, -0.02, 3.12, 35.0, 35.0, 35.0,
    35.0,
];

pub const CANONICAL_JOINT_NAMES: [&str; NUM_JOINTS] = [
    "FR_hip", "FR_thigh", "FR_calf", "FL_hip", "FL_thigh", "FL_calf", "RR_hip", "RR_thigh",
    "RR_calf", "RL_hip", "RL_thigh", "RL_calf", "FR_foot", "FL_foot", "RR_foot", "RL_foot",
];

pub const JOINT_ORDER_THUNDER_LEG_MAJOR: [usize; NUM_JOINTS] =
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

pub const JOINT_ORDER_ISAACLAB_JOINT_MAJOR: [usize; NUM_JOINTS] =
    [0, 3, 6, 9, 1, 4, 7, 10, 2, 5, 8, 11, 12, 13, 14, 15];

pub const JOINT_ORDER_ISAACLAB_FL_FIRST: [usize; NUM_JOINTS] =
    [3, 0, 9, 6, 4, 1, 10, 7, 5, 2, 11, 8, 13, 12, 15, 14];

pub const JOINT_ORDER_GO2_FL_FIRST_LEG_MAJOR: [usize; NUM_JOINTS] =
    [3, 4, 5, 0, 1, 2, 9, 10, 11, 6, 7, 8, 13, 12, 15, 14];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyKind {
    #[default]
    Locomotion,
    Recovery,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RobotPreset {
    #[default]
    #[serde(alias = "thunder")]
    ThunderV4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JointOrderPreset {
    #[default]
    ThunderLegMajor,
    IsaaclabJointMajor,
    IsaaclabFlFirst,
    Go2FlFirstLegMajor,
}

impl JointOrderPreset {
    pub const fn to_joint_map(self) -> [usize; NUM_JOINTS] {
        match self {
            Self::ThunderLegMajor => JOINT_ORDER_THUNDER_LEG_MAJOR,
            Self::IsaaclabJointMajor => JOINT_ORDER_ISAACLAB_JOINT_MAJOR,
            Self::IsaaclabFlFirst => JOINT_ORDER_ISAACLAB_FL_FIRST,
            Self::Go2FlFirstLegMajor => JOINT_ORDER_GO2_FL_FIRST_LEG_MAJOR,
        }
    }

    pub fn from_name(name: &str) -> Result<Self> {
        match name.trim() {
            "thunder_leg_major" => Ok(Self::ThunderLegMajor),
            "isaaclab_joint_major" => Ok(Self::IsaaclabJointMajor),
            "isaaclab_fl_first" => Ok(Self::IsaaclabFlFirst),
            "go2_fl_first_leg_major" => Ok(Self::Go2FlFirstLegMajor),
            other => bail!(
                "unknown joint_order preset '{other}' (expected thunder_leg_major, isaaclab_joint_major, isaaclab_fl_first, go2_fl_first_leg_major, [usize; 16], or 16 joint names)"
            ),
        }
    }
}

pub fn canonical_joint_index(raw: &str) -> Result<usize> {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    let stem = lower.strip_suffix("_joint").unwrap_or(&lower);
    match stem {
        "fr_hip" => Ok(0),
        "fr_thigh" => Ok(1),
        "fr_calf" => Ok(2),
        "fl_hip" => Ok(3),
        "fl_thigh" => Ok(4),
        "fl_calf" => Ok(5),
        "rr_hip" | "hr_hip" => Ok(6),
        "rr_thigh" | "hr_thigh" => Ok(7),
        "rr_calf" | "hr_calf" => Ok(8),
        "rl_hip" | "hl_hip" => Ok(9),
        "rl_thigh" | "hl_thigh" => Ok(10),
        "rl_calf" | "hl_calf" => Ok(11),
        "fr_foot" | "fr_wheel" => Ok(12),
        "fl_foot" | "fl_wheel" => Ok(13),
        "rr_foot" | "rr_wheel" | "hr_foot" | "hr_wheel" => Ok(14),
        "rl_foot" | "rl_wheel" | "hl_foot" | "hl_wheel" => Ok(15),
        _ => bail!(
            "unknown canonical joint name '{trimmed}' (expected one of {CANONICAL_JOINT_NAMES:?})"
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JointOrderSpec {
    Preset(JointOrderPreset),
    Indices([usize; NUM_JOINTS]),
    Names(Box<[String; NUM_JOINTS]>),
}

impl JointOrderSpec {
    pub fn to_joint_map(&self) -> Result<[usize; NUM_JOINTS]> {
        let map = match self {
            Self::Preset(p) => p.to_joint_map(),
            Self::Indices(arr) => *arr,
            Self::Names(names) => {
                let mut out = [0usize; NUM_JOINTS];
                for (i, n) in names.iter().enumerate() {
                    out[i] = canonical_joint_index(n)?;
                }
                out
            }
        };
        validate_joint_permutation(&map)?;
        Ok(map)
    }
}

impl<'de> Deserialize<'de> for JointOrderSpec {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct JointOrderVisitor;

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Elem {
            Idx(usize),
            Name(String),
        }

        impl<'de> Visitor<'de> for JointOrderVisitor {
            type Value = JointOrderSpec;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(
                    "a joint_order preset string, a 16-element index array, or 16 canonical joint names",
                )
            }

            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                JointOrderPreset::from_name(v)
                    .map(JointOrderSpec::Preset)
                    .map_err(E::custom)
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut elems = Vec::with_capacity(NUM_JOINTS);
                while let Some(item) = seq.next_element::<Elem>()? {
                    elems.push(item);
                }
                if elems.len() != NUM_JOINTS {
                    return Err(de::Error::custom(format!(
                        "joint_order sequence must have {NUM_JOINTS} entries, got {}",
                        elems.len()
                    )));
                }
                if elems.iter().all(|e| matches!(e, Elem::Idx(_))) {
                    let mut arr = [0usize; NUM_JOINTS];
                    for (i, e) in elems.into_iter().enumerate() {
                        if let Elem::Idx(v) = e {
                            arr[i] = v;
                        }
                    }
                    Ok(JointOrderSpec::Indices(arr))
                } else if elems.iter().all(|e| matches!(e, Elem::Name(_))) {
                    let names: [String; NUM_JOINTS] = elems
                        .into_iter()
                        .map(|e| match e {
                            Elem::Name(s) => s,
                            Elem::Idx(_) => unreachable!(),
                        })
                        .collect::<Vec<_>>()
                        .try_into()
                        .unwrap();
                    Ok(JointOrderSpec::Names(Box::new(names)))
                } else {
                    Err(de::Error::custom(
                        "joint_order array must contain either 16 integers or 16 joint name strings, not a mix",
                    ))
                }
            }
        }

        deserializer.deserialize_any(JointOrderVisitor)
    }
}

fn validate_joint_permutation(map: &[usize; NUM_JOINTS]) -> Result<()> {
    let mut seen = [false; NUM_JOINTS];
    for &hw in map {
        if hw >= NUM_JOINTS || seen[hw] {
            bail!("joint_order / kinematics.joint_map must be a permutation of 0..16, got {map:?}");
        }
        seen[hw] = true;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryLayout {
    FrameMajor,
    TermMajor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryOrder {
    OldestFirst,
    NewestFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryPadding {
    RepeatFirst,
    Zero,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistorySpec {
    pub length: usize,
    pub layout: HistoryLayout,
    pub order: HistoryOrder,
    pub padding: HistoryPadding,
}

impl Default for HistorySpec {
    fn default() -> Self {
        Self {
            length: 1,
            layout: HistoryLayout::FrameMajor,
            order: HistoryOrder::OldestFirst,
            padding: HistoryPadding::RepeatFirst,
        }
    }
}

impl<'de> Deserialize<'de> for HistorySpec {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawHistorySpec {
            #[serde(default = "default_one")]
            length: usize,
            #[serde(default = "default_frame_major")]
            layout: HistoryLayout,
            #[serde(default = "default_oldest_first")]
            order: HistoryOrder,
            #[serde(default = "default_repeat_first")]
            padding: HistoryPadding,
        }

        struct HistoryVisitor;

        impl<'de> Visitor<'de> for HistoryVisitor {
            type Value = HistorySpec;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a history length integer or a { length, layout, order, padding } map")
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(HistorySpec {
                    length: v as usize,
                    ..HistorySpec::default()
                })
            }

            fn visit_map<M: MapAccess<'de>>(
                self,
                map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let raw = RawHistorySpec::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(HistorySpec {
                    length: raw.length,
                    layout: raw.layout,
                    order: raw.order,
                    padding: raw.padding,
                })
            }
        }

        deserializer.deserialize_any(HistoryVisitor)
    }
}

fn default_schema_v2() -> u32 {
    2
}
fn default_one() -> usize {
    1
}
fn default_frame_major() -> HistoryLayout {
    HistoryLayout::FrameMajor
}
fn default_oldest_first() -> HistoryOrder {
    HistoryOrder::OldestFirst
}
fn default_repeat_first() -> HistoryPadding {
    HistoryPadding::RepeatFirst
}
fn default_f64_one() -> f64 {
    1.0
}
fn default_accel_scale() -> f64 {
    0.1
}
fn default_tau_scale() -> f64 {
    0.05
}
fn default_vel_scales() -> [f64; 4] {
    [0.05, 0.05, 0.05, 0.05]
}
fn default_at_zero() -> String {
    "@0".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JointPosSubset {
    Legs12,
    All16ZeroWheels,
    All16,
}

/// Consolidated observation block table supporting both 1-word string shorthands
/// (`ang_vel`, `gravity`, `dof_pos_12`, `"zeros(3)"`, `"phase_clock(1.2)"`, ...)
/// and explicit `{ source: ... }` YAML maps.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ObsTerm {
    GyroRadS {
        scale: f64,
        pre_clip: Option<[f64; 2]>,
    },
    AccelMS2 {
        scale: f64,
        pre_clip: Option<[f64; 2]>,
    },
    ProjectedGravity {
        pre_clip: Option<[f64; 2]>,
    },
    QuatWxyz {
        pre_clip: Option<[f64; 2]>,
    },
    RpyRad {
        scale: f64,
        pre_clip: Option<[f64; 2]>,
    },
    VelocityCmd {
        scale: f64,
        pre_clip: Option<[f64; 2]>,
    },
    BodyHeightCmd,
    JointPosRel {
        subset: JointPosSubset,
        pre_clip: Option<[f64; 2]>,
        scale: f64,
    },
    JointPosAbs {
        subset: JointPosSubset,
        pre_clip: Option<[f64; 2]>,
        scale: f64,
    },
    JointVel {
        scale_per_group: [f64; 4],
        pre_clip: Option<[f64; 2]>,
        #[serde(default)]
        legs_only: bool,
    },
    JointTau {
        scale: f64,
        pre_clip: Option<[f64; 2]>,
        #[serde(default)]
        legs_only: bool,
    },
    LastActionRaw {
        pre_clip: Option<[f64; 2]>,
        #[serde(default)]
        legs_only: bool,
    },
    Zeros {
        len: usize,
    },
    Constant {
        value: f64,
    },
    PhaseSinCos {
        period_s: f64,
    },
}

#[derive(Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
enum MapObsTerm {
    GyroRadS {
        #[serde(default = "default_f64_one")]
        scale: f64,
        #[serde(default)]
        pre_clip: Option<[f64; 2]>,
    },
    #[serde(alias = "lin_acc")]
    AccelMS2 {
        #[serde(default = "default_accel_scale")]
        scale: f64,
        #[serde(default)]
        pre_clip: Option<[f64; 2]>,
    },
    ProjectedGravity {
        #[serde(default)]
        pre_clip: Option<[f64; 2]>,
    },
    #[serde(alias = "quat")]
    QuatWxyz {
        #[serde(default)]
        pre_clip: Option<[f64; 2]>,
    },
    #[serde(alias = "rpy")]
    RpyRad {
        #[serde(default = "default_f64_one")]
        scale: f64,
        #[serde(default)]
        pre_clip: Option<[f64; 2]>,
    },
    VelocityCmd {
        #[serde(default = "default_f64_one")]
        scale: f64,
        #[serde(default)]
        pre_clip: Option<[f64; 2]>,
    },
    BodyHeightCmd,
    JointPosRel {
        subset: JointPosSubset,
        #[serde(default)]
        pre_clip: Option<[f64; 2]>,
        #[serde(default = "default_f64_one")]
        scale: f64,
    },
    JointPosAbs {
        subset: JointPosSubset,
        #[serde(default)]
        pre_clip: Option<[f64; 2]>,
        #[serde(default = "default_f64_one")]
        scale: f64,
    },
    JointVel {
        #[serde(default = "default_vel_scales")]
        scale_per_group: [f64; 4],
        #[serde(default)]
        pre_clip: Option<[f64; 2]>,
        #[serde(default)]
        legs_only: bool,
    },
    JointTau {
        #[serde(default = "default_tau_scale")]
        scale: f64,
        #[serde(default)]
        pre_clip: Option<[f64; 2]>,
        #[serde(default)]
        legs_only: bool,
    },
    LastActionRaw {
        #[serde(default)]
        pre_clip: Option<[f64; 2]>,
        #[serde(default)]
        legs_only: bool,
    },
    Zeros {
        len: usize,
    },
    Constant {
        value: f64,
    },
    PhaseSinCos {
        period_s: f64,
    },
}

impl From<MapObsTerm> for ObsTerm {
    fn from(m: MapObsTerm) -> Self {
        match m {
            MapObsTerm::GyroRadS { scale, pre_clip } => Self::GyroRadS { scale, pre_clip },
            MapObsTerm::AccelMS2 { scale, pre_clip } => Self::AccelMS2 { scale, pre_clip },
            MapObsTerm::ProjectedGravity { pre_clip } => Self::ProjectedGravity { pre_clip },
            MapObsTerm::QuatWxyz { pre_clip } => Self::QuatWxyz { pre_clip },
            MapObsTerm::RpyRad { scale, pre_clip } => Self::RpyRad { scale, pre_clip },
            MapObsTerm::VelocityCmd { scale, pre_clip } => Self::VelocityCmd { scale, pre_clip },
            MapObsTerm::BodyHeightCmd => Self::BodyHeightCmd,
            MapObsTerm::JointPosRel {
                subset,
                pre_clip,
                scale,
            } => Self::JointPosRel {
                subset,
                pre_clip,
                scale,
            },
            MapObsTerm::JointPosAbs {
                subset,
                pre_clip,
                scale,
            } => Self::JointPosAbs {
                subset,
                pre_clip,
                scale,
            },
            MapObsTerm::JointVel {
                scale_per_group,
                pre_clip,
                legs_only,
            } => Self::JointVel {
                scale_per_group,
                pre_clip,
                legs_only,
            },
            MapObsTerm::JointTau {
                scale,
                pre_clip,
                legs_only,
            } => Self::JointTau {
                scale,
                pre_clip,
                legs_only,
            },
            MapObsTerm::LastActionRaw {
                pre_clip,
                legs_only,
            } => Self::LastActionRaw {
                pre_clip,
                legs_only,
            },
            MapObsTerm::Zeros { len } => Self::Zeros { len },
            MapObsTerm::Constant { value } => Self::Constant { value },
            MapObsTerm::PhaseSinCos { period_s } => Self::PhaseSinCos { period_s },
        }
    }
}

impl ObsTerm {
    pub fn from_shorthand_str(raw: &str) -> Result<Self> {
        let s = raw.trim();
        if let Some(arg) = parse_fn_arg(s, "zeros") {
            let len: usize = arg
                .parse()
                .with_context(|| format!("invalid integer N in '{s}'"))?;
            if len == 0 {
                bail!("zeros(N) requires N >= 1, got '{s}'");
            }
            return Ok(Self::Zeros { len });
        }
        if let Some(arg) = parse_fn_arg(s, "const") {
            let value: f64 = arg
                .parse()
                .with_context(|| format!("invalid float V in '{s}'"))?;
            if !value.is_finite() {
                bail!("const(V) requires finite float, got '{s}'");
            }
            return Ok(Self::Constant { value });
        }
        if let Some(arg) = parse_fn_arg(s, "phase_clock") {
            let period_s: f64 = arg
                .parse()
                .with_context(|| format!("invalid float period P in '{s}'"))?;
            if !period_s.is_finite() || period_s <= 0.0 {
                bail!("phase_clock(P) requires positive finite period_s, got '{s}'");
            }
            return Ok(Self::PhaseSinCos { period_s });
        }

        match s {
            "ang_vel" => Ok(Self::GyroRadS {
                scale: 0.25,
                pre_clip: None,
            }),
            "lin_acc" => Ok(Self::AccelMS2 {
                scale: 0.1,
                pre_clip: None,
            }),
            "gravity" => Ok(Self::ProjectedGravity { pre_clip: None }),
            "quat" => Ok(Self::QuatWxyz { pre_clip: None }),
            "rpy" => Ok(Self::RpyRad {
                scale: 1.0,
                pre_clip: None,
            }),
            "cmd_vel" => Ok(Self::VelocityCmd {
                scale: 1.0,
                pre_clip: None,
            }),
            "cmd_height" => Ok(Self::BodyHeightCmd),
            "dof_pos_12" => Ok(Self::JointPosRel {
                subset: JointPosSubset::Legs12,
                pre_clip: None,
                scale: 1.0,
            }),
            "dof_pos_16" => Ok(Self::JointPosRel {
                subset: JointPosSubset::All16,
                pre_clip: None,
                scale: 1.0,
            }),
            "dof_pos_16_zero_wheels" => Ok(Self::JointPosRel {
                subset: JointPosSubset::All16ZeroWheels,
                pre_clip: None,
                scale: 1.0,
            }),
            "dof_pos_abs_12" => Ok(Self::JointPosAbs {
                subset: JointPosSubset::Legs12,
                pre_clip: None,
                scale: 1.0,
            }),
            "dof_pos_abs_16" => Ok(Self::JointPosAbs {
                subset: JointPosSubset::All16,
                pre_clip: None,
                scale: 1.0,
            }),
            "dof_pos_abs_16_zero_wheels" => Ok(Self::JointPosAbs {
                subset: JointPosSubset::All16ZeroWheels,
                pre_clip: None,
                scale: 1.0,
            }),
            "dof_vel_16" => Ok(Self::JointVel {
                scale_per_group: [0.05; 4],
                pre_clip: None,
                legs_only: false,
            }),
            "dof_vel_12" => Ok(Self::JointVel {
                scale_per_group: [0.05; 4],
                pre_clip: None,
                legs_only: true,
            }),
            "dof_tau_16" => Ok(Self::JointTau {
                scale: 0.05,
                pre_clip: None,
                legs_only: false,
            }),
            "dof_tau_12" => Ok(Self::JointTau {
                scale: 0.05,
                pre_clip: None,
                legs_only: true,
            }),
            "last_action" | "last_action_16" => Ok(Self::LastActionRaw {
                pre_clip: None,
                legs_only: false,
            }),
            "last_action_12" => Ok(Self::LastActionRaw {
                pre_clip: None,
                legs_only: true,
            }),
            other => bail!(
                "unknown observation block shorthand '{other}' (expected ang_vel, lin_acc, gravity, quat, rpy, cmd_vel, cmd_height, dof_pos_12/16/16_zero_wheels, dof_pos_abs_12/16, dof_vel_16/12, dof_tau_16/12, last_action/last_action_12, zeros(N), const(V), or phase_clock(P))"
            ),
        }
    }

    pub const fn width(&self) -> usize {
        match self {
            Self::GyroRadS { .. }
            | Self::AccelMS2 { .. }
            | Self::ProjectedGravity { .. }
            | Self::RpyRad { .. }
            | Self::VelocityCmd { .. } => 3,
            Self::QuatWxyz { .. } => 4,
            Self::BodyHeightCmd | Self::Constant { .. } => 1,
            Self::JointPosRel { subset, .. } | Self::JointPosAbs { subset, .. } => match subset {
                JointPosSubset::Legs12 => 12,
                JointPosSubset::All16ZeroWheels | JointPosSubset::All16 => 16,
            },
            Self::JointVel { legs_only, .. }
            | Self::JointTau { legs_only, .. }
            | Self::LastActionRaw { legs_only, .. } => {
                if *legs_only {
                    12
                } else {
                    16
                }
            }
            Self::Zeros { len } => *len,
            Self::PhaseSinCos { .. } => 2,
        }
    }

    pub fn block_label(&self) -> String {
        let name = match self {
            Self::GyroRadS { .. } => "ang_vel".to_string(),
            Self::AccelMS2 { .. } => "lin_acc".to_string(),
            Self::ProjectedGravity { .. } => "gravity".to_string(),
            Self::QuatWxyz { .. } => "quat".to_string(),
            Self::RpyRad { .. } => "rpy".to_string(),
            Self::VelocityCmd { .. } => "cmd_vel".to_string(),
            Self::BodyHeightCmd => "cmd_height".to_string(),
            Self::JointPosRel { subset, .. } => match subset {
                JointPosSubset::Legs12 => "dof_pos_12".to_string(),
                JointPosSubset::All16 => "dof_pos_16".to_string(),
                JointPosSubset::All16ZeroWheels => "dof_pos_16_zero_wheels".to_string(),
            },
            Self::JointPosAbs { subset, .. } => match subset {
                JointPosSubset::Legs12 => "dof_pos_abs_12".to_string(),
                JointPosSubset::All16 => "dof_pos_abs_16".to_string(),
                JointPosSubset::All16ZeroWheels => "dof_pos_abs_16_zero_wheels".to_string(),
            },
            Self::JointVel { legs_only, .. } => {
                if *legs_only {
                    "dof_vel_12".to_string()
                } else {
                    "dof_vel_16".to_string()
                }
            }
            Self::JointTau { legs_only, .. } => {
                if *legs_only {
                    "dof_tau_12".to_string()
                } else {
                    "dof_tau_16".to_string()
                }
            }
            Self::LastActionRaw { legs_only, .. } => {
                if *legs_only {
                    "last_action_12".to_string()
                } else {
                    "last_action".to_string()
                }
            }
            Self::Zeros { len } => format!("zeros({len})"),
            Self::Constant { value } => format!("const({value})"),
            Self::PhaseSinCos { period_s } => format!("phase_clock({period_s})"),
        };
        format!("{name}:{}", self.width())
    }
}

fn parse_fn_arg<'a>(s: &'a str, fn_name: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(fn_name)?.trim_start();
    let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
    Some(inner.trim())
}

impl<'de> Deserialize<'de> for ObsTerm {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ObsTermVisitor;

        impl<'de> Visitor<'de> for ObsTermVisitor {
            type Value = ObsTerm;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an observation shorthand string or a { source: ... } map")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                ObsTerm::from_shorthand_str(v).map_err(E::custom)
            }

            fn visit_map<M: MapAccess<'de>>(
                self,
                map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mapped = MapObsTerm::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(ObsTerm::from(mapped))
            }
        }

        deserializer.deserialize_any(ObsTermVisitor)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputTensorSpec {
    #[serde(default = "default_at_zero")]
    pub tensor: String,
    #[serde(default)]
    pub history: HistorySpec,
    #[serde(default)]
    pub clip: Option<[f64; 2]>,
    #[serde(alias = "obs")]
    pub terms: Vec<ObsTerm>,
}

impl InputTensorSpec {
    pub fn single_frame_dim(&self) -> usize {
        self.terms.iter().map(ObsTerm::width).sum()
    }

    pub fn total_dim(&self) -> usize {
        self.single_frame_dim() * self.history.length
    }

    pub fn dim_breakdown(&self) -> String {
        let blocks: Vec<String> = self.terms.iter().map(ObsTerm::block_label).collect();
        let single = self.single_frame_dim();
        if self.history.length == 1 {
            format!("single_frame={single} [{}]", blocks.join(" + "))
        } else {
            format!(
                "history={} * single_frame={single} [{}] = {}",
                self.history.length,
                blocks.join(" + "),
                self.total_dim()
            )
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelFileSpec {
    pub file: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default = "default_step_dt")]
    pub step_dt_s: f64,
}

impl<'de> Deserialize<'de> for ModelFileSpec {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawModelFileSpec {
            file: String,
            #[serde(default)]
            sha256: Option<String>,
            #[serde(default = "default_step_dt")]
            step_dt_s: f64,
        }

        struct ModelVisitor;

        impl<'de> Visitor<'de> for ModelVisitor {
            type Value = ModelFileSpec;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a model file path string or a { file, sha256, step_dt_s } map")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                Ok(ModelFileSpec {
                    file: v.to_string(),
                    sha256: None,
                    step_dt_s: default_step_dt(),
                })
            }

            fn visit_map<M: MapAccess<'de>>(
                self,
                map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let raw =
                    RawModelFileSpec::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(ModelFileSpec {
                    file: raw.file,
                    sha256: raw.sha256,
                    step_dt_s: raw.step_dt_s,
                })
            }
        }

        deserializer.deserialize_any(ModelVisitor)
    }
}

fn default_step_dt() -> f64 {
    0.02
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KinematicsSpec {
    #[serde(default = "identity_joint_map")]
    pub joint_map: [usize; NUM_JOINTS],
    #[serde(default = "thunder_v4_standing_pose")]
    pub standing_pose: [f64; NUM_JOINTS],
    #[serde(default = "zero_pose")]
    pub sitting_pose: [f64; NUM_JOINTS],
    #[serde(default = "default_counts_150")]
    pub stand_up_counts: usize,
    #[serde(default = "default_counts_150")]
    pub sit_down_counts: usize,
}

impl Default for KinematicsSpec {
    fn default() -> Self {
        Self {
            joint_map: JOINT_ORDER_THUNDER_LEG_MAJOR,
            standing_pose: THUNDER_V4_STANDING_POSE,
            sitting_pose: [0.0; NUM_JOINTS],
            stand_up_counts: 150,
            sit_down_counts: 150,
        }
    }
}

fn identity_joint_map() -> [usize; NUM_JOINTS] {
    JOINT_ORDER_THUNDER_LEG_MAJOR
}
fn thunder_v4_standing_pose() -> [f64; NUM_JOINTS] {
    THUNDER_V4_STANDING_POSE
}
fn thunder_v4_lower_limits() -> [f64; NUM_JOINTS] {
    THUNDER_V4_LOWER_LIMITS
}
fn thunder_v4_upper_limits() -> [f64; NUM_JOINTS] {
    THUNDER_V4_UPPER_LIMITS
}
fn zero_pose() -> [f64; NUM_JOINTS] {
    [0.0; NUM_JOINTS]
}
fn default_counts_150() -> usize {
    150
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputTensorSpec {
    #[serde(default = "default_at_zero")]
    pub action_tensor: String,
    #[serde(default)]
    pub aux_tensors: Vec<String>,
}

impl Default for OutputTensorSpec {
    fn default() -> Self {
        Self {
            action_tensor: default_at_zero(),
            aux_tensors: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionDecodeSpec {
    #[serde(alias = "scale")]
    pub scale_per_group: [f64; 4],
    #[serde(default)]
    pub raw_clip: Option<[f64; 2]>,
    #[serde(default)]
    pub max_abs_raw_guard: Option<f64>,
    #[serde(default = "thunder_v4_lower_limits")]
    pub lower_limits: [f64; NUM_JOINTS],
    #[serde(default = "thunder_v4_upper_limits")]
    pub upper_limits: [f64; NUM_JOINTS],
    #[serde(default)]
    pub leg_rate_limit_rad_s: Option<f64>,
    #[serde(default)]
    pub force_zero_wheel_on_stop: bool,
    #[serde(default)]
    pub zero_command_wheel_history: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GainsSpec {
    pub infer_kp: [f64; NUM_JOINTS],
    pub infer_kd: [f64; NUM_JOINTS],
    pub stand_kp: [f64; NUM_JOINTS],
    pub stand_kd: [f64; NUM_JOINTS],
}

#[derive(Deserialize)]
#[serde(untagged)]
enum GainVectorInput {
    Full([f64; NUM_JOINTS]),
    Roles {
        hip: f64,
        thigh: f64,
        calf: f64,
        #[serde(default)]
        wheel: Option<f64>,
    },
}

impl GainVectorInput {
    fn expand(self, default_wheel: f64) -> [f64; NUM_JOINTS] {
        match self {
            Self::Full(arr) => arr,
            Self::Roles {
                hip,
                thigh,
                calf,
                wheel,
            } => {
                let w = wheel.unwrap_or(default_wheel);
                [
                    hip, thigh, calf, hip, thigh, calf, hip, thigh, calf, hip, thigh, calf, w, w,
                    w, w,
                ]
            }
        }
    }
}

impl<'de> Deserialize<'de> for GainsSpec {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawGainsSpec {
            #[serde(default, alias = "kp")]
            infer_kp: Option<GainVectorInput>,
            #[serde(default, alias = "kd")]
            infer_kd: Option<GainVectorInput>,
            #[serde(default)]
            stand_kp: Option<GainVectorInput>,
            #[serde(default)]
            stand_kd: Option<GainVectorInput>,
        }

        let raw = RawGainsSpec::deserialize(deserializer)?;
        let infer_kp = raw
            .infer_kp
            .ok_or_else(|| de::Error::missing_field("kp / infer_kp"))?
            .expand(0.0);
        let infer_kd = raw
            .infer_kd
            .ok_or_else(|| de::Error::missing_field("kd / infer_kd"))?
            .expand(1.0);
        let stand_kp = raw
            .stand_kp
            .map_or_else(default_stand_kp, |g| g.expand(0.0));
        let stand_kd = raw
            .stand_kd
            .map_or_else(default_stand_kd, |g| g.expand(1.0));
        Ok(GainsSpec {
            infer_kp,
            infer_kd,
            stand_kp,
            stand_kd,
        })
    }
}

fn default_stand_kp() -> [f64; NUM_JOINTS] {
    [
        200.0, 200.0, 200.0, 200.0, 200.0, 200.0, 200.0, 200.0, 200.0, 200.0, 200.0, 200.0, 0.0,
        0.0, 0.0, 0.0,
    ]
}
fn default_stand_kd() -> [f64; NUM_JOINTS] {
    [
        8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 1.0, 1.0, 1.0, 1.0,
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BodyHeightSpec {
    pub default_m: f64,
    pub min_m: f64,
    pub max_m: f64,
    pub rate_mps: f64,
    #[serde(default = "default_handover_100")]
    pub handover_frames: usize,
}

fn default_handover_100() -> usize {
    100
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilitiesSpec {
    #[serde(default)]
    pub body_height: Option<BodyHeightSpec>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyManifest {
    pub schema_version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub kind: PolicyKind,
    #[serde(default)]
    pub robot: Option<RobotPreset>,
    pub model: ModelFileSpec,
    pub kinematics: KinematicsSpec,
    pub inputs: Vec<InputTensorSpec>,
    pub outputs: OutputTensorSpec,
    pub action_decode: ActionDecodeSpec,
    pub gains: GainsSpec,
    #[serde(default)]
    pub capabilities: CapabilitiesSpec,
}

impl<'de> Deserialize<'de> for PolicyManifest {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawKinematicsSpec {
            #[serde(default)]
            joint_map: Option<JointOrderSpec>,
            #[serde(default = "thunder_v4_standing_pose")]
            standing_pose: [f64; NUM_JOINTS],
            #[serde(default = "zero_pose")]
            sitting_pose: [f64; NUM_JOINTS],
            #[serde(default = "default_counts_150")]
            stand_up_counts: usize,
            #[serde(default = "default_counts_150")]
            sit_down_counts: usize,
        }

        impl Default for RawKinematicsSpec {
            fn default() -> Self {
                Self {
                    joint_map: None,
                    standing_pose: THUNDER_V4_STANDING_POSE,
                    sitting_pose: [0.0; NUM_JOINTS],
                    stand_up_counts: 150,
                    sit_down_counts: 150,
                }
            }
        }

        #[derive(Deserialize)]
        struct RawPolicyManifest {
            #[serde(default = "default_schema_v2")]
            schema_version: u32,
            name: String,
            #[serde(default)]
            description: String,
            #[serde(default)]
            kind: PolicyKind,
            #[serde(default)]
            robot: Option<RobotPreset>,
            #[serde(default)]
            joint_order: Option<JointOrderSpec>,
            model: ModelFileSpec,
            #[serde(default)]
            kinematics: RawKinematicsSpec,
            inputs: Vec<InputTensorSpec>,
            #[serde(default)]
            outputs: OutputTensorSpec,
            action_decode: ActionDecodeSpec,
            gains: GainsSpec,
            #[serde(default)]
            capabilities: CapabilitiesSpec,
        }

        let raw = RawPolicyManifest::deserialize(deserializer)?;
        let joint_map = match (&raw.joint_order, &raw.kinematics.joint_map) {
            (Some(jo), Some(km)) => {
                let jo_map = jo.to_joint_map().map_err(de::Error::custom)?;
                let km_map = km.to_joint_map().map_err(de::Error::custom)?;
                if jo_map != km_map {
                    return Err(de::Error::custom(
                        "conflicting top-level 'joint_order' and 'kinematics.joint_map'",
                    ));
                }
                jo_map
            }
            (Some(jo), None) => jo.to_joint_map().map_err(de::Error::custom)?,
            (None, Some(km)) => km.to_joint_map().map_err(de::Error::custom)?,
            (None, None) => JOINT_ORDER_THUNDER_LEG_MAJOR,
        };

        Ok(PolicyManifest {
            schema_version: raw.schema_version,
            name: raw.name,
            description: raw.description,
            kind: raw.kind,
            robot: raw.robot,
            model: raw.model,
            kinematics: KinematicsSpec {
                joint_map,
                standing_pose: raw.kinematics.standing_pose,
                sitting_pose: raw.kinematics.sitting_pose,
                stand_up_counts: raw.kinematics.stand_up_counts,
                sit_down_counts: raw.kinematics.sit_down_counts,
            },
            inputs: raw.inputs,
            outputs: raw.outputs,
            action_decode: raw.action_decode,
            gains: raw.gains,
            capabilities: raw.capabilities,
        })
    }
}

impl PolicyManifest {
    pub fn from_yaml_str(raw: &str) -> Result<Self> {
        let m: Self = serde_yaml::from_str(raw).context("failed to parse policy.yaml")?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 2 {
            bail!(
                "Unsupported policy.yaml schema_version={}, expected 2",
                self.schema_version
            );
        }
        if self.name.trim().is_empty() {
            bail!("policy name must not be empty");
        }
        if !(0.002..=0.1).contains(&self.model.step_dt_s) || !self.model.step_dt_s.is_finite() {
            bail!(
                "model.step_dt_s out of range [0.002, 0.1]: {}",
                self.model.step_dt_s
            );
        }
        if self.inputs.is_empty() {
            bail!("policy must declare at least 1 input tensor");
        }
        for inp in &self.inputs {
            if inp.tensor.trim().is_empty() || inp.history.length == 0 || inp.terms.is_empty() {
                bail!("invalid input tensor spec '{}'", inp.tensor);
            }
            if let Some([lo, hi]) = inp.clip {
                if lo > hi || !lo.is_finite() || !hi.is_finite() {
                    bail!("invalid clip bounds [{lo}, {hi}] on input '{}'", inp.tensor);
                }
            }
            for term in &inp.terms {
                match *term {
                    ObsTerm::Zeros { len: 0 } => {
                        bail!("Zeros block in '{}' must have len >= 1", inp.tensor);
                    }
                    ObsTerm::PhaseSinCos { period_s }
                        if !period_s.is_finite() || period_s <= 0.0 =>
                    {
                        bail!(
                            "PhaseSinCos block in '{}' requires positive finite period_s",
                            inp.tensor
                        );
                    }
                    _ => {}
                }
            }
        }
        validate_joint_permutation(&self.kinematics.joint_map)?;
        for w in WHEEL_START_INDEX..NUM_JOINTS {
            if self.gains.infer_kp[w] != 0.0 {
                bail!(
                    "wheel joint {w} infer_kp must be 0.0, got {}",
                    self.gains.infer_kp[w]
                );
            }
        }
        for i in 0..NUM_JOINTS {
            if self.action_decode.lower_limits[i] > self.action_decode.upper_limits[i] {
                bail!("action_decode lower_limits[{i}] > upper_limits[{i}]");
            }
        }
        if let Some(bh) = &self.capabilities.body_height {
            if bh.min_m <= 0.0 || bh.max_m <= bh.min_m || bh.rate_mps <= 0.0 {
                bail!("invalid capabilities.body_height bounds");
            }
        }
        Ok(())
    }
}

fn default_accel_m_s2() -> [f64; 3] {
    [0.0, 0.0, 9.80665]
}
fn default_quat_wxyz() -> [f64; 4] {
    [1.0, 0.0, 0.0, 0.0]
}
fn default_projected_gravity() -> [f64; 3] {
    [0.0, 0.0, -1.0]
}

/// Golden test suite serialized in `golden.json` (or YAML) alongside `policy.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenCase {
    pub gyro_rad_s: [f64; 3],
    #[serde(default = "default_accel_m_s2", alias = "lin_acc")]
    pub accel_m_s2: [f64; 3],
    #[serde(default = "default_quat_wxyz", alias = "quat")]
    pub quat_wxyz: [f64; 4],
    #[serde(default = "default_projected_gravity")]
    pub projected_gravity: [f64; 3],
    #[serde(default)]
    pub velocity_cmd: [f64; 3],
    #[serde(default)]
    pub body_height_cmd: f64,
    pub q: [f64; NUM_JOINTS],
    pub dq: [f64; NUM_JOINTS],
    #[serde(default = "zero_pose", alias = "joint_tau")]
    pub tau: [f64; NUM_JOINTS],
    pub expected_raw_action: [f64; NUM_JOINTS],
    #[serde(default)]
    pub expected_decoded_target: Option<[f64; NUM_JOINTS]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenSuite {
    #[serde(default = "default_tol")]
    pub max_abs_tolerance: f64,
    pub steps: Vec<GoldenCase>,
}

fn default_tol() -> f64 {
    1e-4
}
