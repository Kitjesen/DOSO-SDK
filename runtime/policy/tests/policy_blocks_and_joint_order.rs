use std::path::PathBuf;

use anyhow::Result;
use doso_core::{JointVec16, WalkCommand, NUM_JOINTS, WHEEL_START_INDEX};
use doso_policy::{
    CompiledPolicyGraph, InferenceBackend, LoadedPolicyPackage, ObsTerm, PolicyEvalContext,
    PolicyManifest, RobotPreset, JOINT_ORDER_GO2_FL_FIRST_LEG_MAJOR, JOINT_ORDER_ISAACLAB_FL_FIRST,
    JOINT_ORDER_ISAACLAB_JOINT_MAJOR, JOINT_ORDER_THUNDER_LEG_MAJOR, THUNDER_V4_LOWER_LIMITS,
    THUNDER_V4_STANDING_POSE, THUNDER_V4_UPPER_LIMITS,
};

fn models_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("models")
}

#[test]
fn shorthand_yaml_robot_preset_role_gains_and_manifest_reexport() {
    // Verify backwards-compatible `doso_policy::manifest::PolicyManifest` module path still resolves!
    let shorthand_yaml = r#"
name: ultra_compact_test
robot: thunder_v4
joint_order: isaaclab_joint_major
model: "dummy.onnx"
inputs:
  - tensor: "@0"
    clip: [-50.0, 50.0]
    obs:
      - ang_vel
      - lin_acc
      - gravity
      - quat
      - rpy
      - cmd_vel
      - cmd_height
      - dof_pos_12
      - dof_pos_16
      - dof_pos_16_zero_wheels
      - dof_pos_abs_12
      - dof_pos_abs_16
      - dof_vel_16
      - dof_vel_12
      - dof_tau_16
      - dof_tau_12
      - last_action
      - last_action_12
      - "zeros(3)"
      - "const(2.0)"
      - "phase_clock(1.2)"
      - { source: gyro_rad_s, scale: 0.5, pre_clip: [-5.0, 5.0] }
action_decode:
  scale: [0.25, 0.25, 0.25, 5.0]
gains:
  kp: { hip: 90.0, thigh: 120.0, calf: 130.0 }
  kd: { hip: 7.0, thigh: 12.0, calf: 14.0, wheel: 1.5 }
"#;

    let m: doso_policy::manifest::PolicyManifest =
        PolicyManifest::from_yaml_str(shorthand_yaml).expect("shorthand YAML must parse cleanly");

    assert_eq!(m.schema_version, 2);
    assert_eq!(m.robot, Some(RobotPreset::ThunderV4));
    assert_eq!(m.kinematics.joint_map, JOINT_ORDER_ISAACLAB_JOINT_MAJOR);
    assert_eq!(m.kinematics.standing_pose, THUNDER_V4_STANDING_POSE);
    assert_eq!(m.kinematics.sitting_pose, [0.0; NUM_JOINTS]);
    assert_eq!(m.kinematics.stand_up_counts, 150);
    assert_eq!(m.kinematics.sit_down_counts, 150);
    assert_eq!(m.action_decode.lower_limits, THUNDER_V4_LOWER_LIMITS);
    assert_eq!(m.action_decode.upper_limits, THUNDER_V4_UPPER_LIMITS);
    assert_eq!(m.outputs.action_tensor, "@0");

    // Role gains expansion across 4 legs + wheel kp strictly 0.0
    assert_eq!(
        m.gains.infer_kp,
        [
            90.0, 120.0, 130.0, 90.0, 120.0, 130.0, 90.0, 120.0, 130.0, 90.0, 120.0, 130.0, 0.0,
            0.0, 0.0, 0.0
        ]
    );
    assert_eq!(
        m.gains.infer_kd,
        [
            7.0, 12.0, 14.0, 7.0, 12.0, 14.0, 7.0, 12.0, 14.0, 7.0, 12.0, 14.0, 1.5, 1.5, 1.5, 1.5
        ]
    );

    // Expected total single-frame dim:
    // 3+3+3+4+3+3+1 + 12+16+16 + 12+16 + 16+12 + 16+12 + 16+12 + 3+1+2 + 3 = 185
    assert_eq!(m.inputs[0].single_frame_dim(), 185);
    assert!(matches!(
        m.inputs[0].terms[0],
        ObsTerm::GyroRadS {
            scale: 0.25,
            pre_clip: None
        }
    ));
    assert!(matches!(
        m.inputs[0].terms[1],
        ObsTerm::AccelMS2 {
            scale: 0.1,
            pre_clip: None
        }
    ));

    // Also test canonical joint name list in `joint_order:`
    let names_yaml = r#"
name: custom_joint_names_test
joint_order:
  - FL_hip
  - FR_hip
  - RL_hip
  - RR_hip
  - FL_thigh
  - FR_thigh
  - RL_thigh
  - RR_thigh
  - FL_calf
  - FR_calf
  - RL_calf
  - RR_calf
  - FL_foot
  - FR_foot
  - RL_foot
  - RR_foot
model: "dummy.onnx"
inputs:
  - obs: [ang_vel, gravity, dof_pos_12]
action_decode: { scale_per_group: [0.25, 0.25, 0.25, 5.0] }
gains:
  kp: { hip: 80.0, thigh: 80.0, calf: 80.0 }
  kd: { hip: 5.0, thigh: 5.0, calf: 5.0, wheel: 1.0 }
"#;
    let m_names = PolicyManifest::from_yaml_str(names_yaml).expect("16 canonical joint names");
    assert_eq!(m_names.kinematics.joint_map, JOINT_ORDER_ISAACLAB_FL_FIRST);
}

#[test]
fn auto_at_index_input_binding_and_dim_breakdown_error_on_real_onnx() {
    let root = models_root();

    // 1. Single-input ONNX bound via `tensor: "@0"` matches Golden to < 1e-6
    let flat_onnx = std::fs::read(root.join("thunder_flat_w5_model89999_static_b1_arm64.onnx"))
        .expect("flat_w5 onnx");
    let flat_golden = std::fs::read_to_string(root.join("packages/flat_w5/golden.json"))
        .expect("flat_w5 golden");
    let flat_at0_yaml = r#"
schema_version: 2
name: flat_w5_at0_test
kind: locomotion
robot: thunder_v4
model: "flat.onnx"
inputs:
  - { tensor: "@0", clip: [-100.0, 100.0], obs: [ang_vel, gravity, cmd_vel, dof_pos_12, dof_vel_16, last_action] }
outputs: { action_tensor: "@0" }
action_decode:
  scale_per_group: [0.125, 0.25, 0.25, 5.0]
  lower_limits: [-0.39, -3.12, 0.02, -0.39, 0.02, -3.12, -0.39, 0.02, -3.12, -0.39, -3.12, -3.12, -37.69911184307752, -37.69911184307752, -37.69911184307752, -37.69911184307752]
  upper_limits: [0.39, -0.02, 3.12, 0.39, 3.12, -0.02, 0.39, 3.12, 3.12, 0.39, -0.02, 3.12, 37.69911184307752, 37.69911184307752, 37.69911184307752, 37.69911184307752]
gains:
  kp: { hip: 73.0, thigh: 80.0, calf: 80.0 }
  kd: { hip: 6.93, thigh: 6.93, calf: 6.93, wheel: 1.0 }
"#;
    let pkg_flat =
        LoadedPolicyPackage::load_and_verify(flat_at0_yaml, &flat_onnx, Some(&flat_golden), None)
            .expect("@0 input binding on flat_w5 must pass Golden gate");
    assert!(pkg_flat.report.golden_max_abs_err < 1e-6);

    // 2. Dual-input CTS ONNX bound via `tensor: "@0"` and `tensor: "@1"` + `action_tensor: "@0"`
    let cts_onnx = std::fs::read(root.join("thunder_cts_moe_v39_model61128_static_b1_arm64.onnx"))
        .expect("cts_v39 onnx");
    let cts_golden = std::fs::read_to_string(root.join("packages/stairs_cts_v39/golden.json"))
        .expect("cts_v39 golden");
    let cts_at01_yaml = r#"
schema_version: 2
name: cts_v39_at01_test
kind: locomotion
robot: thunder_v4
model: "cts.onnx"
inputs:
  - { tensor: "@0", history: { length: 10, layout: term_major }, clip: [-100.0, 100.0], obs: [ang_vel, gravity, cmd_vel, dof_pos_12, dof_vel_16, last_action] }
  - { tensor: "@1", clip: [-100.0, 100.0], obs: [ang_vel, gravity, cmd_vel, dof_pos_12, dof_vel_16, last_action] }
outputs: { action_tensor: "@0" }
action_decode:
  scale_per_group: [0.25, 0.25, 0.25, 5.0]
  raw_clip: [-100.0, 100.0]
  leg_rate_limit_rad_s: 12.0
  lower_limits: [-0.369, -2.983, 0.15700000000000003, -0.369, 0.15700000000000003, -2.983, -0.369, 0.15700000000000003, -2.826, -0.369, -2.983, -2.826, -30.0, -30.0, -30.0, -30.0]
  upper_limits: [0.369, -0.15700000000000003, 2.983, 0.369, 2.983, -0.15700000000000003, 0.369, 2.983, 2.826, 0.369, -0.15700000000000003, 2.826, 30.0, 30.0, 30.0, 30.0]
gains:
  kp: { hip: 90.0, thigh: 120.0, calf: 120.0 }
  kd: { hip: 7.0, thigh: 12.0, calf: 12.0, wheel: 1.0 }
"#;
    let pkg_cts =
        LoadedPolicyPackage::load_and_verify(cts_at01_yaml, &cts_onnx, Some(&cts_golden), None)
            .expect("@0/@1 dual input binding on cts_v39 must pass Golden gate");
    assert!(pkg_cts.report.golden_max_abs_err < 1e-5);

    // 3. Dimension mismatch reports crystal-clear block-by-block breakdown!
    let mismatch_yaml = flat_at0_yaml.replace("dof_pos_12", "dof_pos_16");
    let err = LoadedPolicyPackage::load_and_verify(&mismatch_yaml, &flat_onnx, None, None)
        .err()
        .expect("57D vs 53D mismatch must be rejected");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("policy.yaml=57") && msg.contains("ONNX=53") && msg.contains("dof_pos_16:16"),
        "expected block-by-block breakdown in error message, got: {msg}"
    );
}

/// Deterministic linear policy whose weights are permuted into Policy joint order `joint_map`,
/// simulating an RL network trained in IsaacLab (`isaaclab_joint_major` / `isaaclab_fl_first`)
/// versus Thunder native (`thunder_leg_major`).
struct JointOrderEquivariantBackend {
    joint_map: [usize; NUM_JOINTS],
    leg_hw_order: [usize; WHEEL_START_INDEX],
}

impl JointOrderEquivariantBackend {
    fn new(joint_map: [usize; NUM_JOINTS]) -> Self {
        let mut leg_hw_order = [0usize; WHEEL_START_INDEX];
        let mut k = 0;
        for &hw in &joint_map {
            if hw < WHEEL_START_INDEX {
                leg_hw_order[k] = hw;
                k += 1;
            }
        }
        assert_eq!(k, WHEEL_START_INDEX);
        Self {
            joint_map,
            leg_hw_order,
        }
    }
}

impl InferenceBackend for JointOrderEquivariantBackend {
    fn backend_name(&self) -> &'static str {
        "joint-order-equivariant-mock"
    }

    fn infer_into(&mut self, inputs: &[(&str, &[f32])], outputs: &mut [Vec<f32>]) -> Result<()> {
        // Observation layout (137D):
        // [0..3]: ang_vel (3)
        // [3..6]: lin_acc (3)
        // [6..10]: quat (4)
        // [10..13]: rpy (3)
        // [13..25]: dof_pos_12 (12)
        // [25..41]: dof_pos_16 (16)
        // [41..53]: dof_pos_abs_12 (12)
        // [53..69]: dof_vel_16 (16)
        // [69..81]: dof_vel_12 (12)
        // [81..97]: dof_tau_16 (16)
        // [97..109]: dof_tau_12 (12)
        // [109..125]: last_action (16)
        // [125..137]: last_action_12 (12)
        let obs = inputs[0].1;
        assert_eq!(obs.len(), 137);

        // Reconstruct canonical hardware vectors from policy-ordered observations so any bug in
        // input permutation immediately breaks hardware action equivalence!
        let mut pos12_hw = [0.0_f32; WHEEL_START_INDEX];
        let mut pos_abs12_hw = [0.0_f32; WHEEL_START_INDEX];
        let mut vel12_hw = [0.0_f32; WHEEL_START_INDEX];
        let mut tau12_hw = [0.0_f32; WHEEL_START_INDEX];
        let mut la12_hw = [0.0_f32; WHEEL_START_INDEX];
        for (l_idx, &hw) in self.leg_hw_order.iter().enumerate() {
            pos12_hw[hw] = obs[13 + l_idx];
            pos_abs12_hw[hw] = obs[41 + l_idx];
            vel12_hw[hw] = obs[69 + l_idx];
            tau12_hw[hw] = obs[97 + l_idx];
            la12_hw[hw] = obs[125 + l_idx];
        }

        let mut pos16_hw = [0.0_f32; NUM_JOINTS];
        let mut vel16_hw = [0.0_f32; NUM_JOINTS];
        let mut tau16_hw = [0.0_f32; NUM_JOINTS];
        let mut la16_hw = [0.0_f32; NUM_JOINTS];
        for (p_idx, &hw) in self.joint_map.iter().enumerate() {
            pos16_hw[hw] = obs[25 + p_idx];
            vel16_hw[hw] = obs[53 + p_idx];
            tau16_hw[hw] = obs[81 + p_idx];
            la16_hw[hw] = obs[109 + p_idx];
        }

        // Compute deterministic hardware action for each hardware joint, then emit in Policy order!
        let imu_bias = obs[0] * 0.2 + obs[3] * 0.1 + obs[10] * 0.3 + obs[11] * 0.4 + obs[12] * 0.5;
        let mut action_hw = [0.0_f32; NUM_JOINTS];
        for hw in 0..NUM_JOINTS {
            let leg_term = if hw < WHEEL_START_INDEX {
                0.7 * pos12_hw[hw]
                    + 0.2 * pos_abs12_hw[hw]
                    + 0.4 * vel12_hw[hw]
                    + 0.3 * tau12_hw[hw]
                    + 0.25 * la12_hw[hw]
            } else {
                0.0
            };
            action_hw[hw] = imu_bias
                + 0.5 * pos16_hw[hw]
                + 0.6 * vel16_hw[hw]
                + 0.35 * tau16_hw[hw]
                + 0.15 * la16_hw[hw]
                + leg_term
                + ((hw as f32 + 1.0) * 0.03125);
        }

        let out = &mut outputs[0];
        out.resize(NUM_JOINTS, 0.0);
        for (p_idx, &hw) in self.joint_map.iter().enumerate() {
            out[p_idx] = action_hw[hw];
        }
        Ok(())
    }
}

fn build_symmetry_graph(order_name: &str) -> CompiledPolicyGraph {
    let yaml = format!(
        r#"
schema_version: 2
name: sym_{order_name}
robot: thunder_v4
joint_order: {order_name}
model: "mock.onnx"
inputs:
  - tensor: "@0"
    clip: [-80.0, 80.0]
    obs:
      - ang_vel
      - lin_acc
      - quat
      - rpy
      - dof_pos_12
      - dof_pos_16
      - dof_pos_abs_12
      - {0}
      - {1}
      - dof_tau_16
      - dof_tau_12
      - last_action
      - last_action_12
action_decode:
  scale_per_group: [0.15, 0.25, 0.30, 4.0]
gains:
  kp: {{ hip: 90.0, thigh: 120.0, calf: 120.0 }}
  kd: {{ hip: 7.0, thigh: 12.0, calf: 12.0, wheel: 1.0 }}
"#,
        // Use distinct hip/thigh/calf/wheel velocity scales so role-indexing bugs are caught!
        "{ source: joint_vel, scale_per_group: [0.04, 0.06, 0.08, 0.02], legs_only: false }",
        "{ source: joint_vel, scale_per_group: [0.04, 0.06, 0.08, 0.02], legs_only: true }",
    );
    let manifest = PolicyManifest::from_yaml_str(&yaml).unwrap();
    CompiledPolicyGraph::new(manifest)
}

#[test]
fn bidirectional_joint_order_observation_and_action_symmetry_across_rollouts() {
    let mut g_thunder = build_symmetry_graph("thunder_leg_major");
    let mut b_thunder = JointOrderEquivariantBackend::new(JOINT_ORDER_THUNDER_LEG_MAJOR);

    let presets: [(&str, [usize; NUM_JOINTS]); 3] = [
        ("isaaclab_joint_major", JOINT_ORDER_ISAACLAB_JOINT_MAJOR),
        ("isaaclab_fl_first", JOINT_ORDER_ISAACLAB_FL_FIRST),
        ("go2_fl_first_leg_major", JOINT_ORDER_GO2_FL_FIRST_LEG_MAJOR),
    ];
    let mut runners: Vec<(&str, [usize; NUM_JOINTS], CompiledPolicyGraph, JointOrderEquivariantBackend)> =
        presets
            .into_iter()
            .map(|(name, map)| {
                (
                    name,
                    map,
                    build_symmetry_graph(name),
                    JointOrderEquivariantBackend::new(map),
                )
            })
            .collect();

    // Known non-trivial Roll=0.2, Pitch=-0.15, Yaw=0.35 quaternion (ZYX extrinsic XYZ)
    let (hr, hp, hy) = (0.1_f64, -0.075_f64, 0.175_f64);
    let (cr, sr) = (hr.cos(), hr.sin());
    let (cp, sp) = (hp.cos(), hp.sin());
    let (cy, sy) = (hy.cos(), hy.sin());
    let quat = [
        cr * cp * cy + sr * sp * sy,
        sr * cp * cy - cr * sp * sy,
        cr * sp * cy + sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
    ];

    for step in 0..6usize {
        let s = step as f64 * 0.07;
        // Distinct non-symmetric values on all 16 joints so every permutation index is uniquely tested!
        let ctx = PolicyEvalContext {
            gyro_rad_s: [0.15 + s, -0.22, 0.31],
            accel_m_s2: [0.4, -0.6, 9.75 + s],
            quat_wxyz: quat,
            projected_gravity: [0.149, 0.198, -0.969],
            velocity_cmd: WalkCommand {
                vx: 0.5,
                vy: -0.15,
                yaw: 0.25,
            },
            body_height_cmd: 0.38,
            joint_pos: JointVec16(std::array::from_fn(|i| {
                THUNDER_V4_STANDING_POSE[i] + ((i as f64 + 1.0) * 0.021 - 0.17) + s * 0.1
            })),
            joint_vel: JointVec16(std::array::from_fn(|i| {
                ((i as f64 + 1.0) * 0.37 - 2.9) + s
            })),
            joint_tau: JointVec16(std::array::from_fn(|i| {
                ((i as f64 + 1.0) * 1.15 - 9.0) - s * 0.5
            })),
            force_wheel_stop: false,
        };

        let ref_out = g_thunder.step(&ctx, &mut b_thunder).unwrap();
        let ref_obs = g_thunder.packed_input_f64(0);

        // Verify RPY extraction accuracy on step 0 (`[10..13]`)
        if step == 0 {
            assert!((ref_obs[10] - 0.2).abs() < 1e-12, "roll mismatch: {}", ref_obs[10]);
            assert!((ref_obs[11] - (-0.15)).abs() < 1e-12, "pitch mismatch: {}", ref_obs[11]);
            assert!((ref_obs[12] - 0.35).abs() < 1e-12, "yaw mismatch: {}", ref_obs[12]);
            assert!((ref_obs[3] - 0.04).abs() < 1e-12, "lin_acc[0] scale 0.1 mismatch");
        }

        for (name, map, graph, backend) in &mut runners {
            let out = graph.step(&ctx, backend).unwrap();
            let obs = graph.packed_input_f64(0);

            // Check 16-DOF observation blocks (`dof_pos_16` at 25..41, `dof_vel_16` at 53..69,
            // `dof_tau_16` at 81..97, `last_action` at 109..125) equal exact `map` permutation of `ref_obs`!
            for base in [25usize, 53, 81, 109] {
                for (p_idx, &hw_idx) in map.iter().enumerate() {
                    let diff = (obs[base + p_idx] - ref_obs[base + hw_idx]).abs();
                    assert!(
                        diff < 1e-12,
                        "{name} step {step} 16D block@{base}[{p_idx}->hw {hw_idx}] mismatch: {} vs {}",
                        obs[base + p_idx],
                        ref_obs[base + hw_idx]
                    );
                }
            }

            // Check 12-DOF leg observation blocks (`dof_pos_12` at 13..25, `dof_pos_abs_12` at 41..53,
            // `dof_vel_12` at 69..81, `dof_tau_12` at 97..109, `last_action_12` at 125..137)
            let leg_hw: Vec<usize> = map.iter().copied().filter(|&h| h < WHEEL_START_INDEX).collect();
            for base in [13usize, 41, 69, 97, 125] {
                for (l_idx, &hw_idx) in leg_hw.iter().enumerate() {
                    let diff = (obs[base + l_idx] - ref_obs[base + hw_idx]).abs();
                    assert!(
                        diff < 1e-12,
                        "{name} step {step} 12D block@{base}[{l_idx}->hw {hw_idx}] mismatch: {} vs {}",
                        obs[base + l_idx],
                        ref_obs[base + hw_idx]
                    );
                }
            }

            // Check hardware raw_action and decoded physical target_action match `thunder_leg_major`!
            for j in 0..NUM_JOINTS {
                assert!(
                    (out.raw_action[j] - ref_out.raw_action[j]).abs() < 1e-6,
                    "{name} step {step} raw_action[{j}] mismatch: {} vs {}",
                    out.raw_action[j],
                    ref_out.raw_action[j]
                );
                assert!(
                    (out.target_action.0[j] - ref_out.target_action.0[j]).abs() < 1e-6,
                    "{name} step {step} target_action[{j}] mismatch: {} vs {}",
                    out.target_action.0[j],
                    ref_out.target_action.0[j]
                );
            }
        }
    }
}
