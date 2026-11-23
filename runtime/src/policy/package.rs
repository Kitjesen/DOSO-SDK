use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use ring::digest::{digest, SHA256};
use serde::{Deserialize, Serialize};

use crate::policy::blocks::{GoldenSuite, PolicyKind, PolicyManifest};
use crate::policy::compiler::{CompiledPolicyGraph, PolicyEvalContext, PolicyStepOutput};
use crate::policy::ort::{InferenceBackend, OrtDynamicMultiBackend};
use crate::types::{JointVec16, ProprioFrame, WalkCommand};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyLoadReport {
    pub name: String,
    pub sha256: String,
    pub backend: String,
    pub warmup_last_us: u64,
    pub warmup_max_us: u64,
    pub golden_steps_checked: usize,
    pub golden_max_abs_err: f64,
}

pub struct LoadedPolicyPackage {
    pub graph: CompiledPolicyGraph,
    pub backend: Box<dyn InferenceBackend>,
    pub report: PolicyLoadReport,
}

impl LoadedPolicyPackage {
    /// Loads, SHA256-verifies, warms up (10 steps), and Golden-verifies a policy package from bytes.
    /// Rejects immediately if SHA256 mismatches, Golden error exceeds tolerance, or warmup produces NaN.
    pub fn load_and_verify(
        manifest_yaml: &str,
        onnx_bytes: &[u8],
        golden_yaml_or_json: Option<&str>,
        custom_backend: Option<Box<dyn InferenceBackend>>,
    ) -> Result<Self> {
        let manifest = PolicyManifest::from_yaml_str(manifest_yaml)?;
        let actual_sha = hex_sha256(onnx_bytes);
        if let Some(expected_sha) = &manifest.model.sha256 {
            let clean = expected_sha.trim().to_ascii_lowercase();
            if !clean.is_empty() && clean != actual_sha {
                bail!(
                    "Policy '{}' ONNX SHA256 mismatch: expected {}, got {}",
                    manifest.name,
                    clean,
                    actual_sha
                );
            }
        }

        let mut backend: Box<dyn InferenceBackend> = match custom_backend {
            Some(b) => b,
            None => Box::new(OrtDynamicMultiBackend::load_for_manifest(
                &manifest, onnx_bytes,
            )?),
        };
        let backend_name = backend.backend_name().to_string();
        let mut graph = CompiledPolicyGraph::new(manifest.clone());

        // 1. Warmup 10 nominal steps before allowing motor/golden evaluation
        let nominal_ctx = PolicyEvalContext {
            body_height_cmd: manifest
                .capabilities
                .body_height
                .as_ref()
                .map_or(0.0, |b| b.default_m),
            joint_pos: JointVec16(manifest.posture.standing_pose),
            ..PolicyEvalContext::default()
        };
        let mut warmup_last_us = 0u64;
        let mut warmup_max_us = 0u64;
        for _ in 0..10 {
            let t0 = Instant::now();
            let _ = graph.step(&nominal_ctx, backend.as_mut())?;
            let us = t0.elapsed().as_micros() as u64;
            warmup_last_us = us;
            warmup_max_us = warmup_max_us.max(us);
        }
        graph.reset();

        // 2. Optional Golden trajectory gate
        let mut golden_steps_checked = 0usize;
        let mut golden_max_abs_err = 0.0_f64;
        if let Some(raw_golden) = golden_yaml_or_json {
            let suite: GoldenSuite =
                serde_yaml::from_str(raw_golden).context("failed to parse golden suite")?;
            graph.reset();
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
                let out = graph.step(&ctx, backend.as_mut())?;
                for (j, &exp_raw) in case.expected_raw_action.iter().enumerate() {
                    let err = (out.raw_action[j] - exp_raw).abs();
                    golden_max_abs_err = golden_max_abs_err.max(err);
                    if err > suite.max_abs_tolerance {
                        bail!(
                            "Golden gate REJECTED policy '{}' at step {} joint {}: expected raw {}, got {} (err {:.6e} > tol {:.6e})",
                            manifest.name,
                            step_idx,
                            j,
                            exp_raw,
                            out.raw_action[j],
                            err,
                            suite.max_abs_tolerance
                        );
                    }
                    if let Some(exp_dec) = &case.expected_decoded_target {
                        let derr = (out.target_action.0[j] - exp_dec[j]).abs();
                        golden_max_abs_err = golden_max_abs_err.max(derr);
                        if derr > suite.max_abs_tolerance {
                            bail!(
                                "Golden gate REJECTED policy '{}' decoded target at step {} joint {}: expected {}, got {} (err {:.6e})",
                                manifest.name,
                                step_idx,
                                j,
                                exp_dec[j],
                                out.target_action.0[j],
                                derr
                            );
                        }
                    }
                }
                golden_steps_checked += 1;
            }
            graph.reset();
        }

        let report = PolicyLoadReport {
            name: manifest.name,
            sha256: actual_sha,
            backend: backend_name,
            warmup_last_us,
            warmup_max_us,
            golden_steps_checked,
            golden_max_abs_err,
        };
        Ok(Self {
            graph,
            backend,
            report,
        })
    }

    pub fn load_from_dir(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let yaml_path = dir.join("policy.yaml");
        let manifest_yaml = std::fs::read_to_string(&yaml_path)
            .with_context(|| format!("missing {}", yaml_path.display()))?;
        let manifest = PolicyManifest::from_yaml_str(&manifest_yaml)?;
        let onnx_path = dir.join(&manifest.model.file);
        let onnx_bytes = std::fs::read(&onnx_path)
            .with_context(|| format!("missing ONNX file {}", onnx_path.display()))?;
        let golden_path = dir.join("golden.json");
        let golden_str = if golden_path.is_file() {
            Some(std::fs::read_to_string(&golden_path)?)
        } else {
            None
        };
        Self::load_and_verify(&manifest_yaml, &onnx_bytes, golden_str.as_deref(), None)
    }

    #[inline]
    pub fn step(&mut self, ctx: &PolicyEvalContext) -> Result<PolicyStepOutput> {
        self.graph.step(ctx, self.backend.as_mut())
    }

    pub fn reset(&mut self) {
        self.graph.reset();
    }
}

/// Zero-risk Shadow Mode evaluator: runs a candidate policy against live proprioception
/// without ever touching motor commands, collecting latency, clipping, and smoothness metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowEvalReport {
    pub policy_name: String,
    pub steps: usize,
    pub p50_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
    pub total_clipped_joints: usize,
    pub max_leg_step_delta_rad: f64,
}

pub struct ShadowEvaluator;

impl ShadowEvaluator {
    pub fn evaluate_frames(
        pkg: &mut LoadedPolicyPackage,
        frames: &[ProprioFrame],
        cmd: WalkCommand,
        body_height_m: f64,
    ) -> Result<ShadowEvalReport> {
        pkg.reset();
        let mut latencies = Vec::with_capacity(frames.len());
        let mut total_clipped = 0usize;
        let mut prev_leg: Option<[f64; 12]> = None;
        let mut max_delta = 0.0_f64;

        for f in frames {
            let ctx = PolicyEvalContext::from_proprio(f, cmd, body_height_m, false);
            let t0 = Instant::now();
            let out = pkg.step(&ctx)?;
            latencies.push(t0.elapsed().as_micros() as u64);
            total_clipped += out.clipped_count;
            let cur_leg: [f64; 12] = std::array::from_fn(|i| out.target_action.0[i]);
            if let Some(prev) = prev_leg {
                for (&c, &p) in cur_leg.iter().zip(prev.iter()) {
                    max_delta = max_delta.max((c - p).abs());
                }
            }
            prev_leg = Some(cur_leg);
        }
        pkg.reset();
        latencies.sort_unstable();
        let n = latencies.len();
        let p50 = if n == 0 { 0 } else { latencies[n / 2] };
        let p99 = if n == 0 {
            0
        } else {
            latencies[((n * 99) / 100).min(n - 1)]
        };
        let max_us = latencies.last().copied().unwrap_or(0);
        Ok(ShadowEvalReport {
            policy_name: pkg.report.name.clone(),
            steps: n,
            p50_us: p50,
            p99_us: p99,
            max_us,
            total_clipped_joints: total_clipped,
            max_leg_step_delta_rad: max_delta,
        })
    }
}

/// Hot-swappable registry of verified policies + named operator slots (`flat`, `stairs`, `height`, `recovery`, ...).
#[derive(Default)]
pub struct PolicyRegistry {
    packages: HashMap<String, LoadedPolicyPackage>,
    slots: HashMap<String, String>,
    previous_slot_target: HashMap<String, String>,
    active_locomotion: String,
}

impl PolicyRegistry {
    pub fn register(&mut self, pkg: LoadedPolicyPackage) -> PolicyLoadReport {
        let report = pkg.report.clone();
        if self.active_locomotion.is_empty() && pkg.graph.manifest().kind == PolicyKind::Locomotion
        {
            self.active_locomotion = report.name.clone();
        }
        self.packages.insert(report.name.clone(), pkg);
        report
    }

    pub fn bind_slot(&mut self, slot: &str, policy_name: &str) -> Result<()> {
        if !self.packages.contains_key(policy_name) {
            bail!("Policy '{policy_name}' is not registered");
        }
        if let Some(prev) = self.slots.insert(slot.to_string(), policy_name.to_string()) {
            if prev != policy_name {
                self.previous_slot_target.insert(slot.to_string(), prev);
            }
        }
        Ok(())
    }

    /// Instant 1-command rollback of an operator slot to its previous verified policy.
    pub fn rollback_slot(&mut self, slot: &str) -> Result<String> {
        let Some(prev) = self.previous_slot_target.get(slot).cloned() else {
            bail!("No previous policy recorded for slot '{slot}'");
        };
        if !self.packages.contains_key(&prev) {
            bail!("Previous policy '{prev}' is no longer loaded");
        }
        self.slots.insert(slot.to_string(), prev.clone());
        if self.slots.values().all(|v| v != &self.active_locomotion) {
            self.active_locomotion = prev.clone();
        }
        Ok(prev)
    }

    pub fn switch_active(&mut self, name_or_slot: &str) -> Result<String> {
        let resolved = self
            .slots
            .get(name_or_slot)
            .cloned()
            .unwrap_or_else(|| name_or_slot.to_string());
        let Some(pkg) = self.packages.get_mut(&resolved) else {
            bail!("Unknown profile or slot '{name_or_slot}'");
        };
        if pkg.graph.manifest().kind != PolicyKind::Locomotion {
            bail!("Policy '{resolved}' is not a locomotion profile");
        }
        pkg.reset();
        self.active_locomotion = resolved.clone();
        Ok(resolved)
    }

    pub fn active_name(&self) -> &str {
        &self.active_locomotion
    }

    pub fn active_mut(&mut self) -> Option<&mut LoadedPolicyPackage> {
        self.packages.get_mut(&self.active_locomotion)
    }

    pub fn active(&self) -> Option<&LoadedPolicyPackage> {
        self.packages.get(&self.active_locomotion)
    }

    pub fn get(&self, name: &str) -> Option<&LoadedPolicyPackage> {
        self.packages.get(name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut LoadedPolicyPackage> {
        self.packages.get_mut(name)
    }

    pub fn locomotion_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .packages
            .values()
            .filter(|p| p.graph.manifest().kind == PolicyKind::Locomotion)
            .map(|p| p.report.name.clone())
            .collect();
        names.sort();
        names
    }

    pub fn slots(&self) -> &HashMap<String, String> {
        &self.slots
    }

    pub fn reports(&self) -> Vec<PolicyLoadReport> {
        let mut out: Vec<_> = self.packages.values().map(|p| p.report.clone()).collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    let d = digest(&SHA256, bytes);
    let mut s = String::with_capacity(64);
    for b in d.as_ref() {
        use std::fmt::Write;
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}
