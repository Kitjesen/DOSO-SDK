use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use doso_runtime::params::{RobotHardwareParams, NUM_JOINTS, WHEEL_START_INDEX};
use doso_runtime::policy::{
    thunder_v4_lower_limits, thunder_v4_standing_pose, thunder_v4_upper_limits,
    AnalyticalMockBackend, CompiledPolicyGraph, GoldenSuite, HistoryLayout, HistoryOrder,
    HistoryPadding, InferenceBackend, LoadedPolicyPackage, ObsTerm, OrtDynamicMultiBackend,
    PolicyEvalContext, PolicyManifest, RobotPreset, ShadowEvaluator,
    JOINT_ORDER_GO2_FL_FIRST_LEG_MAJOR, JOINT_ORDER_ISAACLAB_FL_FIRST,
    JOINT_ORDER_ISAACLAB_JOINT_MAJOR, JOINT_ORDER_THUNDER_LEG_MAJOR,
};
use doso_runtime::types::{JointVec16, ProprioFrame, WalkCommand};

fn models_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("models")
}

fn packages_root() -> PathBuf {
    models_root().join("packages")
}

// ─── 1. Four Real ONNX Models Dart Golden Parity (< 1e-5) ─────────────────────

#[test]
fn all_four_real_onnx_models_match_dart_brainstem_goldens_to_1e_5() {
    let root = packages_root();
    for pkg_name in [
        "flat_w5",
        "single_frame_height_v2",
        "stairs_cts_v39",
        "recovery_v89",
    ] {
        let dir = root.join(pkg_name);
        let mut loaded = LoadedPolicyPackage::load_from_dir(&dir)
            .unwrap_or_else(|e| panic!("Failed to load & Golden-verify {pkg_name}: {e:#}"));
        assert!(
            loaded.report.golden_steps_checked >= 1,
            "{pkg_name} must execute >= 1 Golden verification step"
        );
        assert!(
            loaded.report.golden_max_abs_err < 1e-5,
            "{pkg_name} max_abs_err={:.3e} exceeded 1e-5!",
            loaded.report.golden_max_abs_err
        );

        let dummy_frames = vec![ProprioFrame::default(); 20];
        let shadow = ShadowEvaluator::evaluate_frames(
            &mut loaded,
            &dummy_frames,
            WalkCommand {
                vx: 0.3,
                vy: 0.0,
                yaw: 0.0,
            },
            0.38,
        )
        .unwrap();
        assert_eq!(shadow.steps, 20);
        println!(
            "[PASS] {:<24} golden_steps={} max_err={:.6e} warmup={}us shadow_p99={}us",
            loaded.report.name,
            loaded.report.golden_steps_checked,
            loaded.report.golden_max_abs_err,
            loaded.report.warmup_last_us,
            shadow.p99_us
        );
    }
}

#[test]
fn golden_gate_immediately_rejects_corrupted_action_expectations() {
    let dir = packages_root().join("single_frame_height_v2");
    let yaml = std::fs::read_to_string(dir.join("policy.yaml")).unwrap();
    let bad_golden = r#"
max_abs_tolerance: 1e-4
steps:
  - gyro_rad_s: [0.0, 0.0, 0.0]
    projected_gravity: [0.0, 0.0, -1.0]
    velocity_cmd: [0.5, 0.0, 0.0]
    body_height_cmd: 0.38
    q: [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
    dq: [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
    expected_raw_action: [99.0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
"#;
    let res = LoadedPolicyPackage::load_and_verify(
        &yaml.replace("sha256:", "# sha256:"),
        b"dummy",
        Some(bad_golden),
        Some(Box::new(AnalyticalMockBackend::default())),
    );
    assert!(res.is_err());
    let msg = format!("{:#}", res.err().unwrap());
    assert!(
        msg.contains("Golden gate REJECTED"),
        "unexpected error: {msg}"
    );
}

// ─── 2. Pure f64 25-Step Ring History Precision & Zero-Reallocation ────────────

struct DeterministicActionBackend {
    step_count: u64,
}

impl DeterministicActionBackend {
    fn new() -> Self {
        Self { step_count: 0 }
    }
}

impl InferenceBackend for DeterministicActionBackend {
    fn backend_name(&self) -> &'static str {
        "deterministic-action-mock"
    }

    fn infer_into(&mut self, inputs: &[(&str, &[f32])], outputs: &mut [Vec<f32>]) -> Result<()> {
        assert!(!inputs.is_empty());
        let s = self.step_count as f32;
        let out = &mut outputs[0];
        out.resize(NUM_JOINTS, 0.0);
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = ((i as f32 + 1.0) * 0.137_931_03) + s * 0.071_428_57 + inputs[0].1[0] * 0.01;
        }
        self.step_count += 1;
        Ok(())
    }
}

fn build_precision_manifest() -> PolicyManifest {
    let yaml = r#"
schema_version: 2
name: f64_precision_multi_channel_test
description: "Verifies pure f64 observation encoding and ring history packing"
kind: locomotion

model:
  file: "dummy.onnx"
  step_dt_s: 0.02

kinematics:
  standing_pose: [-0.12, -0.93, 1.96, 0.12, 0.93, -1.96, 0.12, 0.93, -1.96, -0.12, -0.93, 1.96, 0.0, 0.0, 0.0, 0.0]
  sitting_pose: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]

inputs:
  - tensor: cts_term_major_oldest
    history:
      length: 10
      layout: term_major
      order: oldest_first
      padding: repeat_first
    terms: &shared_terms
      - source: gyro_rad_s
        scale: 0.25
        pre_clip: [-100.0, 100.0]
      - source: projected_gravity
        pre_clip: [-100.0, 100.0]
      - source: velocity_cmd
        scale: 0.3333333333333333
        pre_clip: [-10.0, 10.0]
      - source: body_height_cmd
      - source: joint_pos_rel
        subset: legs12
        pre_clip: [-100.0, 100.0]
        scale: 0.7
      - source: zeros
        len: 3
      - source: joint_vel
        scale_per_group: [0.05, 0.05, 0.05, 0.05]
        pre_clip: [-100.0, 100.0]
      - source: last_action_raw
        pre_clip: [-100.0, 100.0]
      - source: constant
        value: 1.4142135623730951
      - source: phase_sin_cos
        period_s: 1.2
  - tensor: frame_major_oldest
    history:
      length: 6
      layout: frame_major
      order: oldest_first
      padding: repeat_first
    terms: *shared_terms
  - tensor: frame_major_newest_zero
    history:
      length: 5
      layout: frame_major
      order: newest_first
      padding: zero
    terms: *shared_terms
  - tensor: term_major_newest_zero
    history:
      length: 4
      layout: term_major
      order: newest_first
      padding: zero
    terms: *shared_terms

outputs:
  action_tensor: actions

action_decode:
  scale_per_group: [0.25, 0.25, 0.25, 5.0]
  raw_clip: [-100.0, 100.0]
  lower_limits: [-0.39, -3.12, 0.02, -0.39, 0.02, -3.12, -0.39, 0.02, -3.12, -0.39, -3.12, -3.12, -35.0, -35.0, -35.0, -35.0]
  upper_limits: [0.39, -0.02, 3.12, 0.39, 3.12, -0.02, 0.39, 3.12, 3.12, 0.39, -0.02, 3.12, 35.0, 35.0, 35.0, 35.0]
  leg_rate_limit_rad_s: 12.0
  force_zero_wheel_on_stop: false

gains:
  infer_kp: [90.0, 120.0, 120.0, 90.0, 120.0, 120.0, 90.0, 120.0, 120.0, 90.0, 120.0, 120.0, 0.0, 0.0, 0.0, 0.0]
  infer_kd: [7.0, 12.0, 12.0, 7.0, 12.0, 12.0, 7.0, 12.0, 12.0, 7.0, 12.0, 12.0, 1.0, 1.0, 1.0, 1.0]
"#;
    PolicyManifest::from_yaml_str(yaml).expect("valid test manifest")
}

const TERM_WIDTHS: [usize; 10] = [3, 3, 3, 1, 12, 3, 16, 16, 1, 2];
const SINGLE_DIM: usize = 60;

fn analytical_single_frame_f64(
    ctx: &PolicyEvalContext,
    standing_pose: &[f64; NUM_JOINTS],
    last_raw_action: &[f64; NUM_JOINTS],
    step_index: u64,
    step_dt_s: f64,
) -> Vec<f64> {
    let mut out = Vec::with_capacity(SINGLE_DIM);
    for &g in &ctx.gyro_rad_s {
        out.push(g.clamp(-100.0, 100.0) * 0.25);
    }
    for &pg in &ctx.projected_gravity {
        out.push(pg.clamp(-100.0, 100.0));
    }
    for rv in [
        ctx.velocity_cmd.vx,
        ctx.velocity_cmd.vy,
        ctx.velocity_cmd.yaw,
    ] {
        out.push(rv.clamp(-10.0, 10.0) * (1.0_f64 / 3.0_f64));
    }
    out.push(ctx.body_height_cmd);
    for (&q, &sp) in ctx.joint_pos.0[..12].iter().zip(standing_pose[..12].iter()) {
        out.push((q - sp).clamp(-100.0, 100.0) * 0.7);
    }
    out.extend_from_slice(&[0.0; 3]);
    let mut jv_scaled = [0.0_f64; NUM_JOINTS];
    for leg in 0..4 {
        for r in 0..3 {
            let idx = leg * 3 + r;
            jv_scaled[idx] = ctx.joint_vel.0[idx].clamp(-100.0, 100.0) * 0.05;
        }
        let w_idx = WHEEL_START_INDEX + leg;
        jv_scaled[w_idx] = ctx.joint_vel.0[w_idx].clamp(-100.0, 100.0) * 0.05;
    }
    out.extend_from_slice(&jv_scaled);
    for &la in last_raw_action {
        out.push(la.clamp(-100.0, 100.0));
    }
    out.push(std::f64::consts::SQRT_2);
    let phase = 2.0 * std::f64::consts::PI * ((step_index as f64) * step_dt_s) / 1.2;
    out.push(phase.sin());
    out.push(phase.cos());
    assert_eq!(out.len(), SINGLE_DIM);
    out
}

fn analytical_pack_history(
    ring: &VecDeque<Vec<f64>>,
    layout: HistoryLayout,
    order: HistoryOrder,
) -> Vec<f64> {
    let h_len = ring.len();
    let mut packed = Vec::with_capacity(h_len * SINGLE_DIM);
    match layout {
        HistoryLayout::FrameMajor => match order {
            HistoryOrder::OldestFirst => {
                for frame in ring {
                    packed.extend_from_slice(frame);
                }
            }
            HistoryOrder::NewestFirst => {
                for frame in ring.iter().rev() {
                    packed.extend_from_slice(frame);
                }
            }
        },
        HistoryLayout::TermMajor => {
            let mut src_offset = 0usize;
            for &w in &TERM_WIDTHS {
                for k in 0..h_len {
                    let frame = match order {
                        HistoryOrder::OldestFirst => &ring[k],
                        HistoryOrder::NewestFirst => &ring[h_len - 1 - k],
                    };
                    packed.extend_from_slice(&frame[src_offset..src_offset + w]);
                }
                src_offset += w;
            }
        }
    }
    packed
}

fn make_eval_context(step: usize) -> PolicyEvalContext {
    let t = (step as f64 + 1.0) * 0.173_205_080_756_887_7;
    PolicyEvalContext {
        gyro_rad_s: [
            if step == 4 {
                125.7
            } else {
                0.123_456_789_012_345 + t * 0.11
            },
            -0.087_654_321_098_765 - t * 0.07,
            0.054_321_987_654_321 + t * 0.03,
        ],
        projected_gravity: [0.031_415_926_535, -0.041_421_356_237, -0.998_645_123_456],
        velocity_cmd: WalkCommand {
            vx: if step == 7 {
                14.2
            } else {
                0.412_345_678_901 + 0.02 * t
            },
            vy: -0.113_579_246_802,
            yaw: 0.219_876_543_210 - 0.01 * t,
        },
        body_height_cmd: 0.375_123_456_789 + 0.002 * (t.sin()),
        joint_pos: JointVec16(std::array::from_fn(|i| {
            -0.11 + (i as f64) * 0.091_234_567_890_123 + t * 0.013
        })),
        joint_vel: JointVec16(std::array::from_fn(|i| {
            if i == 13 && step == 9 {
                -142.5
            } else {
                0.234_567_890_123_456 * ((i as f64) - 7.5) + t * 0.09
            }
        })),
        force_wheel_stop: false,
        ..PolicyEvalContext::default()
    }
}

#[test]
fn packed_input_f64_preserves_double_precision_and_zero_reallocations_across_20_steps() {
    let manifest = build_precision_manifest();
    let standing = manifest.posture.standing_pose;
    let step_dt = manifest.model.step_dt_s;
    let specs = manifest.inputs.clone();

    let mut graph = CompiledPolicyGraph::new(manifest);
    let mut backend = DeterministicActionBackend::new();

    let initial_f64_ptrs: Vec<*const f64> = (0..specs.len())
        .map(|c| graph.packed_input_f64(c).as_ptr())
        .collect();
    let initial_f64_lens: Vec<usize> = (0..specs.len())
        .map(|c| graph.packed_input_f64(c).len())
        .collect();
    let initial_f32_ptrs: Vec<*const f32> = (0..specs.len())
        .map(|c| graph.packed_input(c).as_ptr())
        .collect();
    let initial_f32_lens: Vec<usize> = (0..specs.len())
        .map(|c| graph.packed_input(c).len())
        .collect();

    let mut ref_rings: Vec<VecDeque<Vec<f64>>> = vec![VecDeque::new(); specs.len()];
    let mut ref_last_raw_action = [0.0_f64; NUM_JOINTS];
    let mut max_f64_err_overall = 0.0_f64;
    let mut max_f32_drift_observed = 0.0_f64;

    for step in 0..25usize {
        let ctx = make_eval_context(step);
        let single_ref = analytical_single_frame_f64(
            &ctx,
            &standing,
            &ref_last_raw_action,
            step as u64,
            step_dt,
        );

        for (c, spec) in specs.iter().enumerate() {
            let h_len = spec.history.length;
            if ref_rings[c].is_empty() {
                match spec.history.padding {
                    HistoryPadding::RepeatFirst => {
                        for _ in 0..h_len {
                            ref_rings[c].push_back(single_ref.clone());
                        }
                    }
                    HistoryPadding::Zero => {
                        for _ in 0..(h_len - 1) {
                            ref_rings[c].push_back(vec![0.0; SINGLE_DIM]);
                        }
                        ref_rings[c].push_back(single_ref.clone());
                    }
                }
            } else {
                ref_rings[c].pop_front();
                ref_rings[c].push_back(single_ref.clone());
            }
        }

        let step_out = graph.step(&ctx, &mut backend).expect("step must succeed");

        for (c, spec) in specs.iter().enumerate() {
            let actual_f64 = graph.packed_input_f64(c);
            let actual_f32 = graph.packed_input(c);

            assert_eq!(actual_f64.as_ptr(), initial_f64_ptrs[c]);
            assert_eq!(actual_f64.len(), initial_f64_lens[c]);
            assert_eq!(actual_f32.as_ptr(), initial_f32_ptrs[c]);
            assert_eq!(actual_f32.len(), initial_f32_lens[c]);

            let expected_f64 =
                analytical_pack_history(&ref_rings[c], spec.history.layout, spec.history.order);
            assert_eq!(actual_f64.len(), expected_f64.len());

            for (idx, (&got, &exp)) in actual_f64.iter().zip(expected_f64.iter()).enumerate() {
                let err = (got - exp).abs();
                max_f64_err_overall = max_f64_err_overall.max(err);
                assert!(
                    err < 1e-15,
                    "channel {c} step {step} elem {idx}: got {got:.17e}, expected {exp:.17e}, err={err:.3e}"
                );

                let got_f32 = actual_f32[idx];
                assert_eq!(got_f32.to_bits(), (got as f32).to_bits());
                max_f32_drift_observed = max_f32_drift_observed.max(((got_f32 as f64) - exp).abs());
            }
        }

        ref_last_raw_action = step_out.raw_action;
    }

    assert!(max_f32_drift_observed > 1e-8);

    graph.reset();
    for (c, (&init_f64, &init_f32)) in initial_f64_ptrs
        .iter()
        .zip(initial_f32_ptrs.iter())
        .enumerate()
    {
        assert_eq!(graph.packed_input_f64(c).as_ptr(), init_f64);
        assert_eq!(graph.packed_input(c).as_ptr(), init_f32);
        assert!(graph.packed_input_f64(c).iter().all(|&v| v == 0.0));
    }
    for step in 0..20usize {
        let ctx = make_eval_context(step);
        let _ = graph.step(&ctx, &mut backend).expect("post-reset step");
        for (c, (&init_f64, &init_f32)) in initial_f64_ptrs
            .iter()
            .zip(initial_f32_ptrs.iter())
            .enumerate()
        {
            assert_eq!(graph.packed_input_f64(c).as_ptr(), init_f64);
            assert_eq!(graph.packed_input(c).as_ptr(), init_f32);
        }
    }

    println!(
        "[PASS] packed_input_f64 25-step TermMajor+FrameMajor max_f64_err={max_f64_err_overall:.3e} (vs f32 cast drift {max_f32_drift_observed:.3e}), 0 reallocations"
    );
}

#[test]
fn non_finite_inputs_are_sanitized_to_zero_in_f64_and_f32() {
    let manifest = build_precision_manifest();
    let mut graph = CompiledPolicyGraph::new(manifest);
    let mut backend = DeterministicActionBackend::new();

    let mut ctx = make_eval_context(0);
    ctx.gyro_rad_s[0] = f64::NAN;
    ctx.projected_gravity[1] = f64::INFINITY;
    ctx.body_height_cmd = f64::NEG_INFINITY;
    ctx.joint_vel.0[2] = f64::NAN;

    let _ = graph
        .step(&ctx, &mut backend)
        .expect("sanitized step succeeds");
    for c in 0..4 {
        assert!(graph.packed_input_f64(c).iter().all(|v| v.is_finite()));
        assert!(graph.packed_input(c).iter().all(|v| v.is_finite()));
    }
}

// ─── 3. Modular Blocks, `posture:` Alias, `@0/@1`, and Bidirectional Joint Order ──

#[test]
fn shorthand_yaml_robot_preset_posture_alias_role_gains_and_manifest_reexport() {
    let shorthand_yaml = r#"
name: ultra_compact_test
robot: thunder_v4
joint_order: isaaclab_joint_major
model: "dummy.onnx"
posture:
  stand_up_counts: 120
  sit_down_counts: 100
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

    let m: doso_runtime::policy::manifest::PolicyManifest =
        PolicyManifest::from_yaml_str(shorthand_yaml).expect("shorthand YAML must parse cleanly");

    let hw_posture = &RobotHardwareParams::thunder_v4().posture;
    assert_eq!(m.schema_version, 2);
    assert_eq!(m.robot, Some(RobotPreset::ThunderV4));
    assert_eq!(m.posture.joint_map, JOINT_ORDER_ISAACLAB_JOINT_MAJOR);
    assert_eq!(m.posture.standing_pose, thunder_v4_standing_pose());
    assert_eq!(m.posture.standing_pose, hw_posture.standing_pose);
    assert_eq!(m.posture.sitting_pose, [0.0; NUM_JOINTS]);
    assert_eq!(m.posture.stand_up_counts, 120);
    assert_eq!(m.posture.sit_down_counts, 100);
    assert_eq!(m.action_decode.lower_limits, thunder_v4_lower_limits());
    assert_eq!(m.action_decode.lower_limits, hw_posture.lower_limits);
    assert_eq!(m.action_decode.upper_limits, thunder_v4_upper_limits());
    assert_eq!(m.action_decode.upper_limits, hw_posture.upper_limits);
    assert_eq!(m.outputs.action_tensor, "@0");

    assert_eq!(
        m.gains.infer_kp,
        [
            90.0, 120.0, 130.0, 90.0, 120.0, 130.0, 90.0, 120.0, 130.0, 90.0, 120.0, 130.0, 0.0,
            0.0, 0.0, 0.0
        ]
    );
    assert_eq!(
        m.gains.infer_kd,
        [7.0, 12.0, 14.0, 7.0, 12.0, 14.0, 7.0, 12.0, 14.0, 7.0, 12.0, 14.0, 1.5, 1.5, 1.5, 1.5]
    );

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
    assert_eq!(m_names.posture.joint_map, JOINT_ORDER_ISAACLAB_FL_FIRST);
}

#[test]
fn auto_at_index_input_binding_and_dim_breakdown_error_on_real_onnx() {
    let root = models_root();

    let flat_onnx = std::fs::read(root.join("thunder_flat_w5_model89999_static_b1_arm64.onnx"))
        .expect("flat_w5 onnx");
    let flat_golden =
        std::fs::read_to_string(root.join("packages/flat_w5/golden.json")).expect("flat_w5 golden");
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
        let obs = inputs[0].1;
        assert_eq!(obs.len(), 137);

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
    let mut runners: Vec<(
        &str,
        [usize; NUM_JOINTS],
        CompiledPolicyGraph,
        JointOrderEquivariantBackend,
    )> = presets
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
                thunder_v4_standing_pose()[i] + ((i as f64 + 1.0) * 0.021 - 0.17) + s * 0.1
            })),
            joint_vel: JointVec16(std::array::from_fn(|i| ((i as f64 + 1.0) * 0.37 - 2.9) + s)),
            joint_tau: JointVec16(std::array::from_fn(|i| {
                ((i as f64 + 1.0) * 1.15 - 9.0) - s * 0.5
            })),
            force_wheel_stop: false,
        };

        let ref_out = g_thunder.step(&ctx, &mut b_thunder).unwrap();
        let ref_obs = g_thunder.packed_input_f64(0);

        if step == 0 {
            assert!((ref_obs[10] - 0.2).abs() < 1e-12);
            assert!((ref_obs[11] - (-0.15)).abs() < 1e-12);
            assert!((ref_obs[12] - 0.35).abs() < 1e-12);
            assert!((ref_obs[3] - 0.04).abs() < 1e-12);
        }

        for (name, map, graph, backend) in &mut runners {
            let out = graph.step(&ctx, backend).unwrap();
            let obs = graph.packed_input_f64(0);

            for base in [25usize, 53, 81, 109] {
                for (p_idx, &hw_idx) in map.iter().enumerate() {
                    let diff = (obs[base + p_idx] - ref_obs[base + hw_idx]).abs();
                    assert!(
                        diff < 1e-12,
                        "{name} step {step} 16D block@{base}[{p_idx}->hw {hw_idx}] mismatch"
                    );
                }
            }

            let leg_hw: Vec<usize> = map
                .iter()
                .copied()
                .filter(|&h| h < WHEEL_START_INDEX)
                .collect();
            for base in [13usize, 41, 69, 97, 125] {
                for (l_idx, &hw_idx) in leg_hw.iter().enumerate() {
                    let diff = (obs[base + l_idx] - ref_obs[base + hw_idx]).abs();
                    assert!(
                        diff < 1e-12,
                        "{name} step {step} 12D block@{base}[{l_idx}->hw {hw_idx}] mismatch"
                    );
                }
            }

            for j in 0..NUM_JOINTS {
                assert!((out.raw_action[j] - ref_out.raw_action[j]).abs() < 1e-6);
                assert!((out.target_action.0[j] - ref_out.target_action.0[j]).abs() < 1e-6);
            }
        }
    }
}

// ─── 4. 200-Iter Zero-Alloc Pre-Bound ORT Microbenchmark ──────────────────────

#[test]
fn ort_zero_alloc_hot_path_200_iters_all_four_models_golden_and_latency() {
    const WARMUP_ITERS: usize = 200;
    const TIMED_ITERS: usize = 200;

    let root = packages_root();
    println!(
        "\n=== ORT Pre-Bound Zero-Alloc Microbenchmark ({} warmup + {} timed iters) ===",
        WARMUP_ITERS, TIMED_ITERS
    );

    for pkg_name in [
        "flat_w5",
        "single_frame_height_v2",
        "stairs_cts_v39",
        "recovery_v89",
    ] {
        let dir = root.join(pkg_name);

        let loaded = LoadedPolicyPackage::load_from_dir(&dir)
            .unwrap_or_else(|e| panic!("Failed to load & Golden-verify {pkg_name}: {e:#}"));
        assert!(loaded.report.golden_steps_checked >= 1);
        assert!(loaded.report.golden_max_abs_err < 1e-5);
        drop(loaded);

        let yaml_str = std::fs::read_to_string(dir.join("policy.yaml")).unwrap();
        let manifest = PolicyManifest::from_yaml_str(&yaml_str).unwrap();
        let onnx_bytes = std::fs::read(dir.join(&manifest.model.file)).unwrap();
        let golden_str = std::fs::read_to_string(dir.join("golden.json")).unwrap();
        let suite: GoldenSuite = serde_yaml::from_str(&golden_str).unwrap();

        let mut backend = OrtDynamicMultiBackend::load_for_manifest(&manifest, &onnx_bytes)
            .unwrap_or_else(|e| panic!("ORT load_for_manifest failed for {pkg_name}: {e:#}"));
        assert!(backend.has_prebound_io());

        let (init_in_bufs, init_out_bufs) = backend.pinned_buffer_addrs();
        let (init_in_vals, init_out_vals) = backend.bound_value_addrs();

        let mut graph = CompiledPolicyGraph::new(manifest.clone());
        let default_h = manifest
            .capabilities
            .body_height
            .as_ref()
            .map_or(0.38, |b| b.default_m);

        for step in 0..WARMUP_ITERS {
            let phase = (step as f64) * 0.05;
            let ctx = PolicyEvalContext {
                gyro_rad_s: [0.05 * phase.sin(), -0.03 * phase.cos(), 0.02],
                projected_gravity: [0.01, -0.01, -0.999],
                velocity_cmd: WalkCommand {
                    vx: 0.35,
                    vy: 0.0,
                    yaw: 0.1,
                },
                body_height_cmd: default_h,
                joint_pos: JointVec16(manifest.posture.standing_pose),
                joint_vel: JointVec16::zero(),
                ..PolicyEvalContext::default()
            };
            let out = graph.step(&ctx, &mut backend).unwrap();
            assert!(out.raw_action.iter().all(|v| v.is_finite()));
        }

        let mut samples_us = Vec::with_capacity(TIMED_ITERS);
        for step in 0..TIMED_ITERS {
            let phase = (step as f64) * 0.03;
            let ctx = PolicyEvalContext {
                gyro_rad_s: [0.04 * phase.cos(), 0.02 * phase.sin(), -0.01],
                projected_gravity: [0.0, 0.0, -1.0],
                velocity_cmd: WalkCommand {
                    vx: 0.4,
                    vy: -0.05,
                    yaw: 0.05,
                },
                body_height_cmd: default_h,
                joint_pos: JointVec16(manifest.posture.standing_pose),
                joint_vel: JointVec16::zero(),
                ..PolicyEvalContext::default()
            };
            let t0 = Instant::now();
            let out = graph.step(&ctx, &mut backend).unwrap();
            let elapsed_us = t0.elapsed().as_secs_f64() * 1_000_000.0;
            samples_us.push(elapsed_us);
            assert!(out.target_action.0.iter().all(|v| v.is_finite()));
        }

        assert_eq!(backend.pinned_buffer_addrs(), (init_in_bufs, init_out_bufs));
        assert_eq!(backend.bound_value_addrs(), (init_in_vals, init_out_vals));

        graph.reset();
        let mut post_bench_max_err = 0.0_f64;
        for (step_idx, case) in suite.steps.iter().enumerate() {
            let ctx = PolicyEvalContext {
                gyro_rad_s: case.gyro_rad_s,
                accel_m_s2: case.accel_m_s2,
                quat_wxyz: case.quat_wxyz,
                projected_gravity: case.projected_gravity,
                velocity_cmd: WalkCommand {
                    vx: case.velocity_cmd[0],
                    vy: case.velocity_cmd[1],
                    yaw: case.velocity_cmd[2],
                },
                body_height_cmd: case.body_height_cmd,
                joint_pos: JointVec16(case.q),
                joint_vel: JointVec16(case.dq),
                joint_tau: JointVec16(case.tau),
                force_wheel_stop: false,
            };
            let out = graph.step(&ctx, &mut backend).unwrap();
            for (j, &exp_raw) in case.expected_raw_action.iter().enumerate() {
                let err = (out.raw_action[j] - exp_raw).abs();
                post_bench_max_err = post_bench_max_err.max(err);
                assert!(
                    err < 1e-5,
                    "{pkg_name} post-bench Golden drift at step {step_idx} joint {j}: err={err:.3e}"
                );
            }
            if let Some(exp_dec) = &case.expected_decoded_target {
                for (j, &exp_t) in exp_dec.iter().enumerate() {
                    let derr = (out.target_action.0[j] - exp_t).abs();
                    post_bench_max_err = post_bench_max_err.max(derr);
                    assert!(
                        derr < 1e-5,
                        "{pkg_name} post-bench decoded target drift at step {step_idx} joint {j}: err={derr:.3e}"
                    );
                }
            }
        }

        samples_us.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p50_us = samples_us[TIMED_ITERS / 2];
        let p99_us = samples_us[((TIMED_ITERS * 99) / 100).min(TIMED_ITERS - 1)];
        let max_us = samples_us[TIMED_ITERS - 1];

        println!(
            "[BENCH] {:<24} p50={:>6.1}us  p99={:>6.1}us  max={:>6.1}us  post_golden_err={:.3e}",
            pkg_name, p50_us, p99_us, max_us, post_bench_max_err
        );
    }
}
