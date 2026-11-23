use std::collections::VecDeque;

use anyhow::Result;
use doso_core::{JointVec16, WalkCommand, NUM_JOINTS, WHEEL_START_INDEX};
use doso_policy::{
    CompiledPolicyGraph, HistoryLayout, HistoryOrder, HistoryPadding, InferenceBackend,
    PolicyEvalContext, PolicyManifest,
};

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
    for rv in [ctx.velocity_cmd.vx, ctx.velocity_cmd.vy, ctx.velocity_cmd.yaw] {
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
        // Include values > 100.0 on specific steps to exercise pre_clip in f64
        gyro_rad_s: [
            if step == 4 { 125.7 } else { 0.123_456_789_012_345 + t * 0.11 },
            -0.087_654_321_098_765 - t * 0.07,
            0.054_321_987_654_321 + t * 0.03,
        ],
        projected_gravity: [0.031_415_926_535, -0.041_421_356_237, -0.998_645_123_456],
        velocity_cmd: WalkCommand {
            vx: if step == 7 { 14.2 } else { 0.412_345_678_901 + 0.02 * t },
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
    let standing = manifest.kinematics.standing_pose;
    let step_dt = manifest.model.step_dt_s;
    let specs = manifest.inputs.clone();

    let mut graph = CompiledPolicyGraph::new(manifest);
    let mut backend = DeterministicActionBackend::new();

    // Record pre-allocated buffer pointers and lengths before any step runs.
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

    // Execute 25 steps (> 20 steps) and verify precision & pointer stability at every single tick.
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

            // 1. Zero heap reallocation check: backing slice pointer & length unchanged.
            assert_eq!(
                actual_f64.as_ptr(),
                initial_f64_ptrs[c],
                "packed_f64 reallocated on channel {c} at step {step}"
            );
            assert_eq!(actual_f64.len(), initial_f64_lens[c]);
            assert_eq!(
                actual_f32.as_ptr(),
                initial_f32_ptrs[c],
                "packed_tensor reallocated on channel {c} at step {step}"
            );
            assert_eq!(actual_f32.len(), initial_f32_lens[c]);

            // 2. Full f64 analytical precision check (< 1e-15).
            let expected_f64 =
                analytical_pack_history(&ref_rings[c], spec.history.layout, spec.history.order);
            assert_eq!(actual_f64.len(), expected_f64.len());

            for (idx, (&got, &exp)) in actual_f64.iter().zip(expected_f64.iter()).enumerate() {
                let err = (got - exp).abs();
                max_f64_err_overall = max_f64_err_overall.max(err);
                assert!(
                    err < 1e-15,
                    "channel {} ({:?}/{:?}) step {} elem {}: got {:.17e}, expected {:.17e}, err={:.3e}",
                    c,
                    spec.history.layout,
                    spec.history.order,
                    step,
                    idx,
                    got,
                    exp,
                    err
                );

                // 3. Verify packed_tensor (f32) is exact final cast of packed_f64.
                let got_f32 = actual_f32[idx];
                assert_eq!(
                    got_f32.to_bits(),
                    (got as f32).to_bits(),
                    "packed_tensor[{idx}] must equal `packed_f64[{idx}] as f32`"
                );
                max_f32_drift_observed =
                    max_f32_drift_observed.max(((got_f32 as f64) - exp).abs());
            }
        }

        ref_last_raw_action = step_out.raw_action;
    }

    // Confirm our test inputs genuinely exercise sub-f32 precision where f32 loses ~1e-7!
    assert!(
        max_f32_drift_observed > 1e-8,
        "test inputs must exercise non-dyadic values with >1e-8 f32 rounding (got {max_f32_drift_observed:.3e})"
    );

    // Also verify reset() preserves buffer pointers with zero reallocation across 20 post-reset steps.
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

    let _ = graph.step(&ctx, &mut backend).expect("sanitized step succeeds");
    for c in 0..4 {
        assert!(
            graph.packed_input_f64(c).iter().all(|v| v.is_finite()),
            "channel {c} packed_f64 contains non-finite value"
        );
        assert!(
            graph.packed_input(c).iter().all(|v| v.is_finite()),
            "channel {c} packed_tensor contains non-finite value"
        );
    }
}
