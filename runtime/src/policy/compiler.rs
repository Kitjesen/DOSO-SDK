use anyhow::{bail, Result};

use crate::params::{NUM_JOINTS, WHEEL_START_INDEX};
use crate::policy::blocks::{
    HistoryLayout, HistoryOrder, HistoryPadding, InputTensorSpec, JointPosSubset, ObsTerm,
    PolicyManifest,
};
use crate::policy::ort::InferenceBackend;
use crate::types::{JointVec16, ProprioFrame, WalkCommand};

/// Zero-IO per-tick input snapshot passed to [`CompiledPolicyGraph::step`].
#[derive(Debug, Clone, Copy)]
pub struct PolicyEvalContext {
    pub gyro_rad_s: [f64; 3],
    pub accel_m_s2: [f64; 3],
    pub quat_wxyz: [f64; 4],
    pub projected_gravity: [f64; 3],
    pub velocity_cmd: WalkCommand,
    pub body_height_cmd: f64,
    pub joint_pos: JointVec16,
    pub joint_vel: JointVec16,
    pub joint_tau: JointVec16,
    pub force_wheel_stop: bool,
}

impl Default for PolicyEvalContext {
    fn default() -> Self {
        Self {
            gyro_rad_s: [0.0; 3],
            accel_m_s2: [0.0, 0.0, 9.80665],
            quat_wxyz: [1.0, 0.0, 0.0, 0.0],
            projected_gravity: [0.0, 0.0, -1.0],
            velocity_cmd: WalkCommand::default(),
            body_height_cmd: 0.0,
            joint_pos: JointVec16::zero(),
            joint_vel: JointVec16::zero(),
            joint_tau: JointVec16::zero(),
            force_wheel_stop: false,
        }
    }
}

impl PolicyEvalContext {
    #[inline]
    pub fn from_proprio(
        proprio: &ProprioFrame,
        velocity_cmd: WalkCommand,
        body_height_cmd: f64,
        force_wheel_stop: bool,
    ) -> Self {
        Self {
            gyro_rad_s: proprio.imu.gyro_rad_s,
            accel_m_s2: proprio.imu.accel_m_s2,
            quat_wxyz: proprio.imu.quat_wxyz,
            projected_gravity: proprio.imu.projected_gravity,
            velocity_cmd,
            body_height_cmd,
            joint_pos: proprio.positions(),
            joint_vel: proprio.velocities(),
            joint_tau: JointVec16(std::array::from_fn(|i| proprio.joints[i].tau)),
            force_wheel_stop,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PolicyStepOutput {
    pub raw_action: [f64; NUM_JOINTS],
    pub target_action: JointVec16,
    pub clipped_count: usize,
}

struct InputChannelState {
    spec: InputTensorSpec,
    single_dim: usize,
    term_widths: Vec<usize>,
    /// Ring of `history.length` frames, each `single_dim` doubles.
    frames: Vec<f64>,
    initialized: bool,
    single_scratch: Vec<f64>,
    packed_f64: Vec<f64>,
    packed_tensor: Vec<f32>,
}

impl InputChannelState {
    fn new(spec: InputTensorSpec) -> Self {
        let single_dim = spec.single_frame_dim();
        let total_dim = spec.total_dim();
        let term_widths = spec.terms.iter().map(ObsTerm::width).collect();
        Self {
            frames: vec![0.0; single_dim * spec.history.length],
            initialized: false,
            single_scratch: vec![0.0; single_dim],
            packed_f64: vec![0.0; total_dim],
            packed_tensor: vec![0.0; total_dim],
            spec,
            single_dim,
            term_widths,
        }
    }

    fn reset(&mut self) {
        self.frames.fill(0.0);
        self.single_scratch.fill(0.0);
        self.packed_f64.fill(0.0);
        self.packed_tensor.fill(0.0);
        self.initialized = false;
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_and_pack(
        &mut self,
        ctx: &PolicyEvalContext,
        joint_map: &[usize; NUM_JOINTS],
        standing_pose: &[f64; NUM_JOINTS],
        last_raw_action: &[f64; NUM_JOINTS],
        step_index: u64,
        step_dt_s: f64,
    ) {
        let default_clip = self.spec.clip;
        let mut offset = 0usize;
        for term in &self.spec.terms {
            match *term {
                ObsTerm::GyroRadS { scale, pre_clip } => {
                    let clip = pre_clip.or(default_clip);
                    for (i, &g) in ctx.gyro_rad_s.iter().enumerate() {
                        let v = apply_clip(g, clip) * scale;
                        self.single_scratch[offset + i] = sanitize_f64(v);
                    }
                    offset += 3;
                }
                ObsTerm::AccelMS2 { scale, pre_clip } => {
                    let clip = pre_clip.or(default_clip);
                    for (i, &a) in ctx.accel_m_s2.iter().enumerate() {
                        let v = apply_clip(a, clip) * scale;
                        self.single_scratch[offset + i] = sanitize_f64(v);
                    }
                    offset += 3;
                }
                ObsTerm::ProjectedGravity { pre_clip } => {
                    let clip = pre_clip.or(default_clip);
                    for (i, &pg) in ctx.projected_gravity.iter().enumerate() {
                        let v = apply_clip(pg, clip);
                        self.single_scratch[offset + i] = sanitize_f64(v);
                    }
                    offset += 3;
                }
                ObsTerm::QuatWxyz { pre_clip } => {
                    let clip = pre_clip.or(default_clip);
                    for (i, &q) in ctx.quat_wxyz.iter().enumerate() {
                        let v = apply_clip(q, clip);
                        self.single_scratch[offset + i] = sanitize_f64(v);
                    }
                    offset += 4;
                }
                ObsTerm::RpyRad { scale, pre_clip } => {
                    let clip = pre_clip.or(default_clip);
                    let rpy = quat_wxyz_to_rpy_rad(ctx.quat_wxyz);
                    for (i, &ang) in rpy.iter().enumerate() {
                        let v = apply_clip(ang, clip) * scale;
                        self.single_scratch[offset + i] = sanitize_f64(v);
                    }
                    offset += 3;
                }
                ObsTerm::VelocityCmd { scale, pre_clip } => {
                    let clip = pre_clip.or(default_clip);
                    let raw = [
                        ctx.velocity_cmd.vx,
                        ctx.velocity_cmd.vy,
                        ctx.velocity_cmd.yaw,
                    ];
                    for (i, &rv) in raw.iter().enumerate() {
                        let v = apply_clip(rv, clip) * scale;
                        self.single_scratch[offset + i] = sanitize_f64(v);
                    }
                    offset += 3;
                }
                ObsTerm::BodyHeightCmd => {
                    let v = apply_clip(ctx.body_height_cmd, default_clip);
                    self.single_scratch[offset] = sanitize_f64(v);
                    offset += 1;
                }
                ObsTerm::JointPosRel {
                    subset,
                    pre_clip,
                    scale,
                } => {
                    let clip = pre_clip.or(default_clip);
                    match subset {
                        JointPosSubset::Legs12 => {
                            for &hw_idx in joint_map {
                                if hw_idx < WHEEL_START_INDEX {
                                    let d = ctx.joint_pos.0[hw_idx] - standing_pose[hw_idx];
                                    self.single_scratch[offset] =
                                        sanitize_f64(apply_clip(d, clip) * scale);
                                    offset += 1;
                                }
                            }
                        }
                        JointPosSubset::All16ZeroWheels => {
                            for (p_idx, &hw_idx) in joint_map.iter().enumerate() {
                                self.single_scratch[offset + p_idx] = if hw_idx < WHEEL_START_INDEX
                                {
                                    let d = ctx.joint_pos.0[hw_idx] - standing_pose[hw_idx];
                                    sanitize_f64(apply_clip(d, clip) * scale)
                                } else {
                                    0.0
                                };
                            }
                            offset += 16;
                        }
                        JointPosSubset::All16 => {
                            for (p_idx, &hw_idx) in joint_map.iter().enumerate() {
                                let d = ctx.joint_pos.0[hw_idx] - standing_pose[hw_idx];
                                self.single_scratch[offset + p_idx] =
                                    sanitize_f64(apply_clip(d, clip) * scale);
                            }
                            offset += 16;
                        }
                    }
                }
                ObsTerm::JointPosAbs {
                    subset,
                    pre_clip,
                    scale,
                } => {
                    let clip = pre_clip.or(default_clip);
                    match subset {
                        JointPosSubset::Legs12 => {
                            for &hw_idx in joint_map {
                                if hw_idx < WHEEL_START_INDEX {
                                    let q = ctx.joint_pos.0[hw_idx];
                                    self.single_scratch[offset] =
                                        sanitize_f64(apply_clip(q, clip) * scale);
                                    offset += 1;
                                }
                            }
                        }
                        JointPosSubset::All16ZeroWheels => {
                            for (p_idx, &hw_idx) in joint_map.iter().enumerate() {
                                self.single_scratch[offset + p_idx] = if hw_idx < WHEEL_START_INDEX
                                {
                                    let q = ctx.joint_pos.0[hw_idx];
                                    sanitize_f64(apply_clip(q, clip) * scale)
                                } else {
                                    0.0
                                };
                            }
                            offset += 16;
                        }
                        JointPosSubset::All16 => {
                            for (p_idx, &hw_idx) in joint_map.iter().enumerate() {
                                let q = ctx.joint_pos.0[hw_idx];
                                self.single_scratch[offset + p_idx] =
                                    sanitize_f64(apply_clip(q, clip) * scale);
                            }
                            offset += 16;
                        }
                    }
                }
                ObsTerm::JointVel {
                    scale_per_group,
                    pre_clip,
                    legs_only,
                } => {
                    let clip = pre_clip.or(default_clip);
                    if legs_only {
                        for &hw_idx in joint_map {
                            if hw_idx < WHEEL_START_INDEX {
                                let r = hw_idx % 3;
                                let v =
                                    apply_clip(ctx.joint_vel.0[hw_idx], clip) * scale_per_group[r];
                                self.single_scratch[offset] = sanitize_f64(v);
                                offset += 1;
                            }
                        }
                    } else {
                        for (p_idx, &hw_idx) in joint_map.iter().enumerate() {
                            let r = if hw_idx < WHEEL_START_INDEX {
                                hw_idx % 3
                            } else {
                                3
                            };
                            let v = apply_clip(ctx.joint_vel.0[hw_idx], clip) * scale_per_group[r];
                            self.single_scratch[offset + p_idx] = sanitize_f64(v);
                        }
                        offset += 16;
                    }
                }
                ObsTerm::JointTau {
                    scale,
                    pre_clip,
                    legs_only,
                } => {
                    let clip = pre_clip.or(default_clip);
                    if legs_only {
                        for &hw_idx in joint_map {
                            if hw_idx < WHEEL_START_INDEX {
                                let v = apply_clip(ctx.joint_tau.0[hw_idx], clip) * scale;
                                self.single_scratch[offset] = sanitize_f64(v);
                                offset += 1;
                            }
                        }
                    } else {
                        for (p_idx, &hw_idx) in joint_map.iter().enumerate() {
                            let v = apply_clip(ctx.joint_tau.0[hw_idx], clip) * scale;
                            self.single_scratch[offset + p_idx] = sanitize_f64(v);
                        }
                        offset += 16;
                    }
                }
                ObsTerm::LastActionRaw {
                    pre_clip,
                    legs_only,
                } => {
                    let clip = pre_clip.or(default_clip);
                    if legs_only {
                        for &hw_idx in joint_map {
                            if hw_idx < WHEEL_START_INDEX {
                                let v = apply_clip(last_raw_action[hw_idx], clip);
                                self.single_scratch[offset] = sanitize_f64(v);
                                offset += 1;
                            }
                        }
                    } else {
                        for (p_idx, &hw_idx) in joint_map.iter().enumerate() {
                            let v = apply_clip(last_raw_action[hw_idx], clip);
                            self.single_scratch[offset + p_idx] = sanitize_f64(v);
                        }
                        offset += 16;
                    }
                }
                ObsTerm::Zeros { len } => {
                    self.single_scratch[offset..offset + len].fill(0.0);
                    offset += len;
                }
                ObsTerm::Constant { value } => {
                    self.single_scratch[offset] = sanitize_f64(value);
                    offset += 1;
                }
                ObsTerm::PhaseSinCos { period_s } => {
                    let phase =
                        2.0 * std::f64::consts::PI * ((step_index as f64) * step_dt_s) / period_s;
                    let (sin_v, cos_v) = (phase.sin(), phase.cos());
                    self.single_scratch[offset] = sanitize_f64(sin_v);
                    self.single_scratch[offset + 1] = sanitize_f64(cos_v);
                    offset += 2;
                }
            }
        }
        debug_assert_eq!(offset, self.single_dim);

        let h_len = self.spec.history.length;
        if !self.initialized {
            match self.spec.history.padding {
                HistoryPadding::RepeatFirst => {
                    for k in 0..h_len {
                        self.frames[k * self.single_dim..(k + 1) * self.single_dim]
                            .copy_from_slice(&self.single_scratch);
                    }
                }
                HistoryPadding::Zero => {
                    self.frames.fill(0.0);
                    self.frames[(h_len - 1) * self.single_dim..h_len * self.single_dim]
                        .copy_from_slice(&self.single_scratch);
                }
            }
            self.initialized = true;
        } else if h_len == 1 {
            self.frames.copy_from_slice(&self.single_scratch);
        } else {
            self.frames.copy_within(self.single_dim.., 0);
            self.frames[(h_len - 1) * self.single_dim..h_len * self.single_dim]
                .copy_from_slice(&self.single_scratch);
        }

        match self.spec.history.layout {
            HistoryLayout::FrameMajor => match self.spec.history.order {
                HistoryOrder::OldestFirst => {
                    self.packed_f64.copy_from_slice(&self.frames);
                }
                HistoryOrder::NewestFirst => {
                    for k in 0..h_len {
                        let src_k = h_len - 1 - k;
                        self.packed_f64[k * self.single_dim..(k + 1) * self.single_dim]
                            .copy_from_slice(
                                &self.frames
                                    [src_k * self.single_dim..(src_k + 1) * self.single_dim],
                            );
                    }
                }
            },
            HistoryLayout::TermMajor => {
                // Exact match to `CtsPolicyState.append`: for each term width, iterate frames!
                let mut src_term_offset = 0usize;
                let mut dst = 0usize;
                for &w in &self.term_widths {
                    for k in 0..h_len {
                        let frame_idx = match self.spec.history.order {
                            HistoryOrder::OldestFirst => k,
                            HistoryOrder::NewestFirst => h_len - 1 - k,
                        };
                        let base = frame_idx * self.single_dim + src_term_offset;
                        self.packed_f64[dst..dst + w].copy_from_slice(&self.frames[base..base + w]);
                        dst += w;
                    }
                    src_term_offset += w;
                }
            }
        }

        // Cast `packed_f64` -> `packed_tensor` (`f32`) as the final step right before ORT inference.
        for (dst, &v) in self.packed_tensor.iter_mut().zip(self.packed_f64.iter()) {
            *dst = v as f32;
        }
    }
}

/// Pre-allocated, zero-heap-allocation execution graph for any `PolicyManifest`.
pub struct CompiledPolicyGraph {
    manifest: PolicyManifest,
    channels: Vec<InputChannelState>,
    output_buffers: Vec<Vec<f32>>,
    last_raw_action: [f64; NUM_JOINTS],
    previous_target: JointVec16,
    step_index: u64,
}

impl CompiledPolicyGraph {
    pub fn new(manifest: PolicyManifest) -> Self {
        let channels = manifest
            .inputs
            .iter()
            .cloned()
            .map(InputChannelState::new)
            .collect();
        let num_outputs = 1 + manifest.outputs.aux_tensors.len();
        let mut output_buffers = Vec::with_capacity(num_outputs);
        output_buffers.push(vec![0.0_f32; NUM_JOINTS]);
        for _ in &manifest.outputs.aux_tensors {
            output_buffers.push(vec![0.0_f32; 64]);
        }
        let standing = JointVec16(manifest.posture.standing_pose);
        Self {
            manifest,
            channels,
            output_buffers,
            last_raw_action: [0.0; NUM_JOINTS],
            previous_target: standing,
            step_index: 0,
        }
    }

    pub fn manifest(&self) -> &PolicyManifest {
        &self.manifest
    }

    pub fn reset(&mut self) {
        for ch in &mut self.channels {
            ch.reset();
        }
        self.last_raw_action = [0.0; NUM_JOINTS];
        self.previous_target = JointVec16(self.manifest.posture.standing_pose);
        self.step_index = 0;
    }

    pub fn packed_input(&self, idx: usize) -> &[f32] {
        &self.channels[idx].packed_tensor
    }

    pub fn packed_input_f64(&self, idx: usize) -> &[f64] {
        &self.channels[idx].packed_f64
    }

    pub fn step(
        &mut self,
        ctx: &PolicyEvalContext,
        backend: &mut dyn InferenceBackend,
    ) -> Result<PolicyStepOutput> {
        let joint_map = &self.manifest.posture.joint_map;
        let standing = &self.manifest.posture.standing_pose;
        let step_dt = self.manifest.model.step_dt_s;
        for ch in &mut self.channels {
            ch.encode_and_pack(
                ctx,
                joint_map,
                standing,
                &self.last_raw_action,
                self.step_index,
                step_dt,
            );
        }

        // Build stack slice of `(&str, &[f32])` (typically 1 or 2 inputs).
        let mut input_refs: [(&str, &[f32]); 4] = [("", &[]); 4];
        if self.channels.len() > 4 {
            bail!("Policy exceeds 4 input tensors");
        }
        for (i, ch) in self.channels.iter().enumerate() {
            input_refs[i] = (ch.spec.tensor.as_str(), ch.packed_tensor.as_slice());
        }

        backend.infer_into(&input_refs[..self.channels.len()], &mut self.output_buffers)?;

        let raw_slice = &self.output_buffers[0];
        if raw_slice.len() != NUM_JOINTS {
            bail!("Expected 16 action outputs, got {}", raw_slice.len());
        }

        // Permute from policy joint order -> canonical Thunder hardware joint order.
        let mut raw_hw = [0.0_f64; NUM_JOINTS];
        for (p_idx, &v) in raw_slice.iter().enumerate() {
            let vf = v as f64;
            if !vf.is_finite() {
                bail!("ONNX output[{p_idx}] is non-finite ({vf})");
            }
            if let Some(max_guard) = self.manifest.action_decode.max_abs_raw_guard {
                if vf.abs() > max_guard {
                    bail!("ONNX output[{p_idx}]={vf} exceeds guard ±{max_guard}");
                }
            }
            let hw_idx = joint_map[p_idx];
            raw_hw[hw_idx] = vf;
        }

        // Update raw action history BEFORE target clipping/slew (exact match to Dart).
        let zero_cmd = ctx.velocity_cmd.is_zero();
        let mut next_hist_raw = raw_hw;
        if self.manifest.action_decode.zero_command_wheel_history && zero_cmd {
            for w in &mut next_hist_raw[WHEEL_START_INDEX..NUM_JOINTS] {
                *w = 0.0;
            }
        }
        self.last_raw_action = next_hist_raw;
        self.step_index = self.step_index.wrapping_add(1);

        // Decode into physical joint target (in canonical Thunder hardware order).
        let dec = &self.manifest.action_decode;
        let mut clipped_raw = JointVec16(raw_hw);
        if let Some([lo, hi]) = dec.raw_clip {
            for v in &mut clipped_raw.0 {
                *v = v.clamp(lo, hi);
            }
        }
        let scaled = clipped_raw.scale_by_role(dec.scale_per_group);
        let mut clipped_count = 0usize;
        let mut target: [f64; NUM_JOINTS] = std::array::from_fn(|i| {
            let req = scaled.0[i] + standing[i];
            let clamped = req.clamp(dec.lower_limits[i], dec.upper_limits[i]);
            if (clamped - req).abs() > 1e-9 {
                clipped_count += 1;
            }
            clamped
        });

        if let Some(rate_rad_s) = dec.leg_rate_limit_rad_s {
            let max_delta = rate_rad_s * step_dt;
            for (t, &prev) in target[..WHEEL_START_INDEX]
                .iter_mut()
                .zip(self.previous_target.0[..WHEEL_START_INDEX].iter())
            {
                *t = prev + (*t - prev).clamp(-max_delta, max_delta);
            }
        }

        let mut target_vec = JointVec16(target);
        self.previous_target = target_vec;
        if ctx.force_wheel_stop || (zero_cmd && dec.force_zero_wheel_on_stop) {
            target_vec = target_vec.discard_wheels();
        }

        Ok(PolicyStepOutput {
            raw_action: raw_hw,
            target_action: target_vec,
            clipped_count,
        })
    }
}

#[inline]
fn quat_wxyz_to_rpy_rad(q: [f64; 4]) -> [f64; 3] {
    let [mut w, mut x, mut y, mut z] = q;
    let norm_sq = w * w + x * x + y * y + z * z;
    if !norm_sq.is_finite() || norm_sq < 1e-24 {
        return [0.0; 3];
    }
    let inv = 1.0 / norm_sq.sqrt();
    w *= inv;
    x *= inv;
    y *= inv;
    z *= inv;

    let sinr_cosp = 2.0 * (w * x + y * z);
    let cosr_cosp = 1.0 - 2.0 * (x * x + y * y);
    let roll = sinr_cosp.atan2(cosr_cosp);

    let sinp = (2.0 * (w * y - z * x)).clamp(-1.0, 1.0);
    let pitch = sinp.asin();

    let siny_cosp = 2.0 * (w * z + x * y);
    let cosy_cosp = 1.0 - 2.0 * (y * y + z * z);
    let yaw = siny_cosp.atan2(cosy_cosp);

    [roll, pitch, yaw]
}

#[inline]
fn apply_clip(v: f64, clip: Option<[f64; 2]>) -> f64 {
    match clip {
        Some([lo, hi]) => v.clamp(lo, hi),
        None => v,
    }
}

#[inline]
fn sanitize_f64(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}
