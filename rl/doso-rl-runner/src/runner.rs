use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use doso_sdk::{rpc, ExternalControl, JointCommand, RobotClient};
use doso_shm_ipc::{DosoExternalCmdPayload, ExternalCmdWriter, ProprioRingReader};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

use crate::config::{DeployConfig, NUM_JOINTS};
use crate::obs::{
    policy_actions_to_joint_commands_into, projected_gravity_from_quat_wxyz, ObservationBuilder,
};
use crate::ort_backend::PolicyBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerMode {
    Passive,
    FixStand,
    VelocityPolicy,
    SoftDamp,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LatencyStats {
    pub samples: usize,
    pub p50_us: u64,
    pub p99_us: u64,
    pub p999_us: u64,
    pub max_us: u64,
}

#[derive(Default)]
struct LatencyRecorder {
    samples_us: Vec<u64>,
}

impl LatencyRecorder {
    fn record(&mut self, us: u64) {
        self.samples_us.push(us);
    }

    fn summarize(&mut self) -> LatencyStats {
        if self.samples_us.is_empty() {
            return LatencyStats::default();
        }
        self.samples_us.sort_unstable();
        let n = self.samples_us.len();
        let at = |pct: f64| -> u64 {
            let idx = (((n - 1) as f64) * pct).round() as usize;
            self.samples_us[idx.min(n - 1)]
        };
        LatencyStats {
            samples: n,
            p50_us: at(0.50),
            p99_us: at(0.99),
            p999_us: at(0.999),
            max_us: self.samples_us[n - 1],
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub deploy: DeployConfig,
    pub target_cmd: [f64; 3],
    pub max_steps: Option<u64>,
    pub skip_fix_stand: bool,
    pub telemetry_wait_timeout: Duration,
    pub shm_enabled: bool,
    pub shm_proprio_name: String,
    pub shm_cmd_name: String,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            deploy: DeployConfig::default(),
            target_cmd: [0.0, 0.0, 0.0],
            max_steps: None,
            skip_fix_stand: false,
            telemetry_wait_timeout: Duration::from_secs(3),
            shm_enabled: false,
            shm_proprio_name: "/doso_proprio_v1".to_string(),
            shm_cmd_name: "/doso_ext_cmd_v1".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunnerReport {
    pub final_mode: RunnerMode,
    pub fix_stand_steps: u64,
    pub policy_steps: u64,
    pub damp_triggered: bool,
    pub stop_reason: String,
    pub shm_data_plane_active: bool,
    pub step_latency_us: LatencyStats,
    pub infer_latency_us: LatencyStats,
    pub tick_jitter_us: LatencyStats,
}

struct ShmDataPlane {
    proprio_reader: ProprioRingReader,
    cmd_writer: ExternalCmdWriter,
    last_proprio_seq: u64,
    cached_imu: ImuSnapshot,
    cached_joints: JointsSnapshot,
    cached_session_id: String,
    cached_session_hash: u64,
}

impl ShmDataPlane {
    fn try_open(proprio_name: &str, cmd_name: &str) -> Result<Self> {
        let proprio_reader = ProprioRingReader::open(proprio_name)?;
        let cmd_writer = ExternalCmdWriter::open(cmd_name)?;
        Ok(Self {
            proprio_reader,
            cmd_writer,
            last_proprio_seq: 0,
            cached_imu: ImuSnapshot::default(),
            cached_joints: JointsSnapshot::default(),
            cached_session_id: String::new(),
            cached_session_hash: 0,
        })
    }

    fn poll_telemetry(&mut self) {
        if let Some((seq, p)) = self.proprio_reader.read_latest(self.last_proprio_seq) {
            self.last_proprio_seq = seq;
            let now = Instant::now();
            self.cached_imu = ImuSnapshot {
                quat_wxyz: p.imu_quat_wxyz,
                ang_vel: p.imu_gyro_rad_s,
                received_at: Some(now),
            };
            self.cached_joints = JointsSnapshot {
                q: p.joint_q,
                dq: p.joint_dq,
                seen_mask: u16::MAX,
                received_at: Some(now),
            };
        }
    }

    fn read_fresh(
        &mut self,
        max_age_ms: f64,
    ) -> std::result::Result<(ImuSnapshot, JointsSnapshot), String> {
        self.poll_telemetry();
        let Some(at) = self.cached_imu.received_at else {
            return Err("SHM proprioception telemetry not received".to_string());
        };
        if at.elapsed().as_secs_f64() * 1000.0 > max_age_ms {
            return Err("SHM proprioception telemetry stale".to_string());
        }
        Ok((self.cached_imu.clone(), self.cached_joints.clone()))
    }

    fn publish_commands(
        &mut self,
        ext: &mut ExternalControl<'_>,
        cmds: &[JointCommand],
    ) -> Result<()> {
        ExternalControl::validate_joint_commands(cmds, NUM_JOINTS)?;
        let (session_id, input_window, sequence) = ext.shm_frame_token()?;
        if self.cached_session_id != session_id {
            self.cached_session_hash = doso_shm_ipc::fnv1a_64(session_id.as_bytes());
            self.cached_session_id = session_id;
        }
        let mut payload = DosoExternalCmdPayload {
            session_id_hash: self.cached_session_hash,
            input_window,
            sequence,
            monotonic_us: doso_shm_ipc::monotonic_us(),
            ..Default::default()
        };
        for (i, j) in cmds.iter().enumerate() {
            payload.q[i] = j.q;
            payload.dq[i] = j.dq;
            payload.kp[i] = j.kp;
            payload.kd[i] = j.kd;
            payload.tau[i] = j.tau;
        }
        self.cmd_writer.publish(&payload)?;
        Ok(())
    }
}

#[derive(Clone, Default)]
struct ImuSnapshot {
    quat_wxyz: [f64; 4],
    ang_vel: [f64; 3],
    received_at: Option<Instant>,
}

#[derive(Clone)]
struct JointsSnapshot {
    q: [f64; NUM_JOINTS],
    dq: [f64; NUM_JOINTS],
    seen_mask: u16,
    received_at: Option<Instant>,
}

impl Default for JointsSnapshot {
    fn default() -> Self {
        Self {
            q: [0.0; NUM_JOINTS],
            dq: [0.0; NUM_JOINTS],
            seen_mask: 0,
            received_at: None,
        }
    }
}

struct TaskAbortGuard(Vec<JoinHandle<()>>);

impl Drop for TaskAbortGuard {
    fn drop(&mut self) {
        for h in &self.0 {
            h.abort();
        }
    }
}

pub struct RlRunner {
    config: RunnerConfig,
    obs_builder: ObservationBuilder,
    backend: Box<dyn PolicyBackend>,
}

impl RlRunner {
    pub fn new(config: RunnerConfig, backend: Box<dyn PolicyBackend>) -> Result<Self> {
        config.deploy.validate()?;
        if backend.input_dim() != config.deploy.obs_dim() {
            bail!(
                "policy input_dim {} != deploy obs_dim {}",
                backend.input_dim(),
                config.deploy.obs_dim()
            );
        }
        if backend.output_dim() != NUM_JOINTS {
            bail!(
                "policy output_dim {} != {}",
                backend.output_dim(),
                NUM_JOINTS
            );
        }
        let obs_builder = ObservationBuilder::new(config.deploy.clone());
        Ok(Self {
            config,
            obs_builder,
            backend,
        })
    }

    pub async fn run(
        &mut self,
        robot: &RobotClient,
        mut shutdown: oneshot::Receiver<()>,
    ) -> Result<RunnerReport> {
        let mut shm_plane = if self.config.shm_enabled {
            match ShmDataPlane::try_open(&self.config.shm_proprio_name, &self.config.shm_cmd_name) {
                Ok(plane) => {
                    tracing::info!(
                        proprio = %self.config.shm_proprio_name,
                        cmd = %self.config.shm_cmd_name,
                        "doso-rl-runner using direct POSIX SHM zero-copy data plane (control plane stays on gRPC)"
                    );
                    Some(plane)
                }
                Err(err) => {
                    tracing::warn!(
                        proprio = %self.config.shm_proprio_name,
                        cmd = %self.config.shm_cmd_name,
                        error = %err,
                        "POSIX SHM unavailable; falling back to gRPC telemetry and command streams"
                    );
                    None
                }
            }
        } else {
            None
        };
        let shm_active = shm_plane.is_some();

        let (grpc_watches, _tasks) = if shm_active {
            (None, TaskAbortGuard(Vec::new()))
        } else {
            let imu_stream = robot.observations().watch_imu().await?;
            let joint_stream = robot.observations().watch_joints().await?;
            let (imu_tx, imu_rx) = watch::channel(ImuSnapshot::default());
            let (joints_tx, joints_rx) = watch::channel(JointsSnapshot::default());
            let guard = TaskAbortGuard(vec![
                spawn_imu_watcher(imu_stream, imu_tx),
                spawn_joint_watcher(joint_stream, joints_tx),
            ]);
            (Some((imu_rx, joints_rx)), guard)
        };

        let mut ext = robot.low_level(None).await?;

        let q_initial = match wait_for_initial_telemetry(
            shm_plane.as_mut(),
            grpc_watches.as_ref(),
            self.config.telemetry_wait_timeout,
            &mut shutdown,
        )
        .await
        {
            Ok(q) => q,
            Err(reason) => {
                safe_damp_and_close(Some(ext), shm_plane.as_mut(), robot, &self.config.deploy)
                    .await;
                return Ok(RunnerReport {
                    final_mode: RunnerMode::SoftDamp,
                    fix_stand_steps: 0,
                    policy_steps: 0,
                    damp_triggered: true,
                    stop_reason: reason,
                    shm_data_plane_active: shm_active,
                    step_latency_us: LatencyStats::default(),
                    infer_latency_us: LatencyStats::default(),
                    tick_jitter_us: LatencyStats::default(),
                });
            }
        };

        // Submit safe initial hold frame before enabling motor outputs.
        let mut initial_hold = [JointCommand::damping(0.0); NUM_JOINTS];
        interpolate_fix_stand_into(&q_initial, &self.config.deploy, 0.0, &mut initial_hold);
        if let Err(err) = submit_commands(&mut ext, shm_plane.as_mut(), &initial_hold) {
            safe_damp_and_close(Some(ext), shm_plane.as_mut(), robot, &self.config.deploy).await;
            return Err(err);
        }
        if shm_active {
            // Allow on-board HardwareExternal 200Hz SHM poller to ingest the priming frame.
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        if let Err(err) = ext.enable(None).await {
            safe_damp_and_close(Some(ext), shm_plane.as_mut(), robot, &self.config.deploy).await;
            return Err(err.into());
        }

        let mut mode = if self.config.skip_fix_stand {
            RunnerMode::VelocityPolicy
        } else {
            RunnerMode::FixStand
        };
        let mut fix_stand_started = Instant::now();
        let mut fix_stand_steps = 0_u64;
        let mut policy_steps = 0_u64;
        let mut last_action_policy = [0.0_f32; NUM_JOINTS];
        let mut obs_buf = vec![0.0_f32; self.config.deploy.obs_dim()];
        let mut raw_action_buf = [0.0_f32; NUM_JOINTS];
        let mut clipped_buf = [0.0_f32; NUM_JOINTS];
        let mut cmd_buf = [JointCommand::damping(0.0); NUM_JOINTS];
        let mut step_latency = LatencyRecorder::default();
        let mut infer_latency = LatencyRecorder::default();
        let mut tick_jitter = LatencyRecorder::default();
        self.obs_builder.reset();

        let step_dur = Duration::from_secs_f64(self.config.deploy.step_dt);
        let nominal_step_us = step_dur.as_micros() as u64;
        let mut ticker = tokio::time::interval(step_dur);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut prev_tick_at: Option<Instant> = None;

        let mut ext_opt = Some(ext);
        let max_age_ms = self.config.deploy.max_telemetry_age_ms;

        let (damp_triggered, stop_reason) = loop {
            tokio::select! {
                _ = &mut shutdown => {
                    mode = RunnerMode::SoftDamp;
                    break (true, "shutdown signal received".to_string());
                }
                _ = ticker.tick() => {
                    let tick_start = Instant::now();
                    if let Some(prev) = prev_tick_at {
                        let actual_us = tick_start.saturating_duration_since(prev).as_micros() as u64;
                        tick_jitter.record(actual_us.abs_diff(nominal_step_us));
                    }
                    prev_tick_at = Some(tick_start);

                    let Some(ext_ref) = ext_opt.as_mut() else {
                        mode = RunnerMode::SoftDamp;
                        break (true, "external session missing".to_string());
                    };

                    let ctrl = match ext_ref.state() {
                        Ok(Some(c)) => c,
                        Ok(None) => {
                            mode = RunnerMode::SoftDamp;
                            break (true, "controller state missing".to_string());
                        }
                        Err(err) => {
                            mode = RunnerMode::SoftDamp;
                            break (true, format!("controller stream error: {err}"));
                        }
                    };
                    if !ctrl.fresh || !ctrl.active {
                        mode = RunnerMode::SoftDamp;
                        break (true, "controller state stale or inactive".to_string());
                    }

                    let (imu, joints) = match read_fresh_telemetry(
                        shm_plane.as_mut(),
                        grpc_watches.as_ref(),
                        max_age_ms,
                    ) {
                        Ok(pair) => pair,
                        Err(reason) => {
                            mode = RunnerMode::SoftDamp;
                            break (true, reason);
                        }
                    };

                    let Some(proj_grav) = projected_gravity_from_quat_wxyz(
                        imu.quat_wxyz[0],
                        imu.quat_wxyz[1],
                        imu.quat_wxyz[2],
                        imu.quat_wxyz[3],
                    ) else {
                        mode = RunnerMode::SoftDamp;
                        break (true, "invalid IMU quaternion".to_string());
                    };

                    if -proj_grav[2] < self.config.deploy.max_tilt_cos {
                        mode = RunnerMode::SoftDamp;
                        break (
                            true,
                            format!(
                                "tilt safety limit exceeded: upright_cos={:.3} < {:.3}",
                                -proj_grav[2], self.config.deploy.max_tilt_cos
                            ),
                        );
                    }

                    match mode {
                        RunnerMode::Passive => {
                            mode = RunnerMode::FixStand;
                            fix_stand_started = Instant::now();
                        }
                        RunnerMode::FixStand => {
                            let alpha = (fix_stand_started.elapsed().as_secs_f64()
                                / self.config.deploy.fix_stand_duration_s)
                                .clamp(0.0, 1.0);
                            interpolate_fix_stand_into(
                                &q_initial,
                                &self.config.deploy,
                                alpha,
                                &mut cmd_buf,
                            );
                            if let Err(err) =
                                submit_commands(ext_ref, shm_plane.as_mut(), &cmd_buf)
                            {
                                mode = RunnerMode::SoftDamp;
                                break (true, format!("FixStand publish failed: {err}"));
                            }
                            step_latency.record(tick_start.elapsed().as_micros() as u64);
                            fix_stand_steps += 1;
                            if alpha >= 1.0 {
                                mode = RunnerMode::VelocityPolicy;
                            }
                        }
                        RunnerMode::VelocityPolicy => {
                            let cmd_vel = resolve_cmd_vel(
                                ctrl.remote.as_ref(),
                                self.config.target_cmd,
                                &self.config.deploy,
                            );
                            if let Err(err) = self.obs_builder.push_and_build_into(
                                imu.ang_vel,
                                proj_grav,
                                cmd_vel,
                                &joints.q,
                                &joints.dq,
                                &last_action_policy,
                                &mut obs_buf,
                            ) {
                                mode = RunnerMode::SoftDamp;
                                break (true, format!("observation error: {err}"));
                            }
                            let infer_start = Instant::now();
                            if let Err(err) = self.backend.infer_into(&obs_buf, &mut raw_action_buf)
                            {
                                mode = RunnerMode::SoftDamp;
                                break (true, format!("inference error: {err}"));
                            }
                            infer_latency.record(infer_start.elapsed().as_micros() as u64);
                            if let Err(err) = policy_actions_to_joint_commands_into(
                                &raw_action_buf,
                                &self.config.deploy,
                                &mut clipped_buf,
                                &mut cmd_buf,
                            ) {
                                mode = RunnerMode::SoftDamp;
                                break (true, format!("action safety error: {err}"));
                            }
                            if let Err(err) =
                                submit_commands(ext_ref, shm_plane.as_mut(), &cmd_buf)
                            {
                                mode = RunnerMode::SoftDamp;
                                break (true, format!("policy publish failed: {err}"));
                            }
                            step_latency.record(tick_start.elapsed().as_micros() as u64);
                            last_action_policy = clipped_buf;
                            policy_steps += 1;
                            if let Some(limit) = self.config.max_steps {
                                if fix_stand_steps + policy_steps >= limit {
                                    break (false, format!("reached max_steps {limit}"));
                                }
                            }
                        }
                        RunnerMode::SoftDamp => {
                            break (true, "entered SoftDamp".to_string());
                        }
                    }

                    if let Some(limit) = self.config.max_steps {
                        if mode == RunnerMode::FixStand && fix_stand_steps + policy_steps >= limit {
                            break (false, format!("reached max_steps {limit}"));
                        }
                    }
                }
            }
        };

        safe_damp_and_close(
            ext_opt.take(),
            shm_plane.as_mut(),
            robot,
            &self.config.deploy,
        )
        .await;

        Ok(RunnerReport {
            final_mode: mode,
            fix_stand_steps,
            policy_steps,
            damp_triggered,
            stop_reason,
            shm_data_plane_active: shm_active,
            step_latency_us: step_latency.summarize(),
            infer_latency_us: infer_latency.summarize(),
            tick_jitter_us: tick_jitter.summarize(),
        })
    }
}

fn resolve_cmd_vel(
    remote: Option<&rpc::RemoteState>,
    fallback: [f64; 3],
    cfg: &DeployConfig,
) -> [f64; 3] {
    if let Some(rc) = remote {
        if rc.active && rc.lx.is_finite() && rc.ly.is_finite() && rc.rx.is_finite() {
            return [
                rc.ly.clamp(-1.0, 1.0) * cfg.cmd_max_vx,
                -rc.lx.clamp(-1.0, 1.0) * cfg.cmd_max_vy,
                -rc.rx.clamp(-1.0, 1.0) * cfg.cmd_max_wz,
            ];
        }
    }
    fallback
}

fn interpolate_fix_stand_into(
    q_initial: &[f64; NUM_JOINTS],
    cfg: &DeployConfig,
    alpha: f64,
    out: &mut [JointCommand; NUM_JOINTS],
) {
    for (hw_idx, slot) in out.iter_mut().enumerate() {
        *slot = if cfg.is_wheel(hw_idx) {
            JointCommand::wheel_vel(0.0, cfg.damping[hw_idx])
        } else {
            let q_target =
                (1.0 - alpha) * q_initial[hw_idx] + alpha * cfg.default_joint_pos[hw_idx];
            JointCommand::leg_pd(q_target, cfg.stiffness[hw_idx], cfg.damping[hw_idx])
        };
    }
}

fn submit_commands(
    ext: &mut ExternalControl<'_>,
    shm: Option<&mut ShmDataPlane>,
    cmds: &[JointCommand],
) -> Result<()> {
    if let Some(plane) = shm {
        plane.publish_commands(ext, cmds)
    } else {
        ext.publish_joints(cmds.to_vec(), None)?;
        Ok(())
    }
}

fn read_fresh_telemetry(
    shm: Option<&mut ShmDataPlane>,
    grpc: Option<&(
        watch::Receiver<ImuSnapshot>,
        watch::Receiver<JointsSnapshot>,
    )>,
    max_age_ms: f64,
) -> std::result::Result<(ImuSnapshot, JointsSnapshot), String> {
    if let Some(plane) = shm {
        return plane.read_fresh(max_age_ms);
    }
    let Some((imu_rx, joints_rx)) = grpc else {
        return Err("no telemetry source active".to_string());
    };
    let imu = imu_rx.borrow().clone();
    let Some(imu_at) = imu.received_at else {
        return Err("IMU telemetry not received".to_string());
    };
    if imu_at.elapsed().as_secs_f64() * 1000.0 > max_age_ms {
        return Err("IMU telemetry stale".to_string());
    }

    let joints = joints_rx.borrow().clone();
    let Some(j_at) = joints.received_at else {
        return Err("joint telemetry not received".to_string());
    };
    if joints.seen_mask != u16::MAX {
        return Err("incomplete 16-joint state".to_string());
    }
    if j_at.elapsed().as_secs_f64() * 1000.0 > max_age_ms {
        return Err("joint telemetry stale".to_string());
    }
    Ok((imu, joints))
}

async fn wait_for_initial_telemetry(
    mut shm: Option<&mut ShmDataPlane>,
    grpc: Option<&(
        watch::Receiver<ImuSnapshot>,
        watch::Receiver<JointsSnapshot>,
    )>,
    timeout: Duration,
    shutdown: &mut oneshot::Receiver<()>,
) -> std::result::Result<[f64; NUM_JOINTS], String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if shutdown.try_recv().is_ok() {
            return Err("shutdown requested during initial telemetry wait".to_string());
        }
        if let Some(plane) = shm.as_deref_mut() {
            plane.poll_telemetry();
            if plane.cached_imu.received_at.is_some()
                && plane.cached_joints.received_at.is_some()
                && plane.cached_joints.seen_mask == u16::MAX
            {
                return Ok(plane.cached_joints.q);
            }
        } else if let Some((imu_rx, joints_rx)) = grpc {
            let imu = imu_rx.borrow();
            let joints = joints_rx.borrow();
            if imu.received_at.is_some()
                && joints.received_at.is_some()
                && joints.seen_mask == u16::MAX
            {
                return Ok(joints.q);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("timed out waiting for initial IMU and 16-joint telemetry".to_string());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn safe_damp_and_close(
    mut ext_opt: Option<ExternalControl<'_>>,
    shm: Option<&mut ShmDataPlane>,
    robot: &RobotClient,
    cfg: &DeployConfig,
) {
    if let Some(ext) = ext_opt.as_mut() {
        let mut damp_frame = [JointCommand::damping(0.0); NUM_JOINTS];
        for (i, slot) in damp_frame.iter_mut().enumerate() {
            *slot = JointCommand::damping(cfg.damping[i]);
        }
        let _ = submit_commands(ext, shm, &damp_frame);
        let _ = ext.disable(None).await;
    }
    if let Some(ext) = ext_opt.take() {
        let _ = ext.close(None).await;
    }
    let _ = robot.sport().damp(None).await;
}

fn spawn_imu_watcher(
    mut stream: tonic::Streaming<rpc::ImuSample>,
    tx: watch::Sender<ImuSnapshot>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Ok(Some(sample)) = stream.message().await {
            let q = sample.orientation_world_to_body.unwrap_or(rpc::Quaternion {
                w: 1.0,
                x: 0.0,
                y: 0.0,
                z: 0.0,
            });
            let gyro = sample.angular_velocity_rad_s.unwrap_or(rpc::Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            });
            let _ = tx.send(ImuSnapshot {
                quat_wxyz: [q.w, q.x, q.y, q.z],
                ang_vel: [gyro.x, gyro.y, gyro.z],
                received_at: Some(Instant::now()),
            });
        }
    })
}

fn spawn_joint_watcher(
    mut stream: tonic::Streaming<rpc::JointSample>,
    tx: watch::Sender<JointsSnapshot>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Ok(Some(sample)) = stream.message().await {
            tx.send_modify(|state| {
                for j in &sample.joints {
                    let idx = j.id as usize;
                    if idx < NUM_JOINTS {
                        state.q[idx] = j.position_rad;
                        state.dq[idx] = j.velocity_rad_s;
                        state.seen_mask |= 1_u16 << idx;
                    }
                }
                state.received_at = Some(Instant::now());
            });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ort_backend::GainScheduleTestBackend;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio_stream::Stream;
    use tonic::{Request, Response, Status};

    type EmptyStream<T> = Pin<Box<dyn Stream<Item = std::result::Result<T, Status>> + Send>>;

    #[derive(Clone, Default)]
    struct MockDosoGateway {
        flip_imu: Arc<AtomicBool>,
        damp_calls: Arc<AtomicUsize>,
        frames: Arc<Mutex<Vec<Vec<JointCommand>>>>,
    }

    #[tonic::async_trait]
    impl rpc::system_server::System for MockDosoGateway {
        async fn handshake(
            &self,
            _: Request<rpc::Handshake>,
        ) -> std::result::Result<Response<rpc::HandshakeReply>, Status> {
            Ok(Response::new(rpc::HandshakeReply {
                protocol_version: 2,
                info: Some(rpc::SystemInfo {
                    robot_id: Some("thunder-test-1".into()),
                    firmware_version: Some("0.1.0".into()),
                    robot_version: Some("thunder".into()),
                    low_level_endpoint: None,
                }),
            }))
        }

        async fn ping(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::Ping>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn state(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::Snapshot>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn robot_status(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::RobotStatus>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn health(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::Snapshot>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn faults(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::FaultReport>, Status> {
            Err(Status::unimplemented("unused"))
        }

        type WatchFaultsStream = EmptyStream<rpc::FaultReport>;
        async fn watch_faults(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<Self::WatchFaultsStream>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn readiness(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::Snapshot>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn capabilities(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::Snapshot>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn session(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::Snapshot>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn product_status(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::Snapshot>, Status> {
            Err(Status::unimplemented("unused"))
        }
    }

    #[tonic::async_trait]
    impl rpc::observations_server::Observations for MockDosoGateway {
        async fn scene(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::Snapshot>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn camera_status(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::Snapshot>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn camera_calibration(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::CameraCalibration>, Status> {
            Err(Status::unimplemented("unused"))
        }

        type WatchImuStream =
            tokio_stream::wrappers::ReceiverStream<std::result::Result<rpc::ImuSample, Status>>;
        async fn watch_imu(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<Self::WatchImuStream>, Status> {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let flip = self.flip_imu.clone();
            tokio::spawn(async move {
                loop {
                    let inverted = flip.load(Ordering::Relaxed);
                    let q = if inverted {
                        rpc::Quaternion {
                            w: 0.0,
                            x: 1.0,
                            y: 0.0,
                            z: 0.0,
                        }
                    } else {
                        rpc::Quaternion {
                            w: 1.0,
                            x: 0.0,
                            y: 0.0,
                            z: 0.0,
                        }
                    };
                    if tx
                        .send(Ok(rpc::ImuSample {
                            angular_velocity_rad_s: Some(rpc::Vector3 {
                                x: 0.0,
                                y: 0.0,
                                z: 0.0,
                            }),
                            orientation_world_to_body: Some(q),
                            linear_acceleration_m_s2: Some(rpc::Vector3 {
                                x: 0.0,
                                y: 0.0,
                                z: 9.81,
                            }),
                            source_elapsed: None,
                        }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            });
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
                rx,
            )))
        }

        type WatchJointsStream =
            tokio_stream::wrappers::ReceiverStream<std::result::Result<rpc::JointSample, Status>>;
        async fn watch_joints(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<Self::WatchJointsStream>, Status> {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tokio::spawn(async move {
                let default_pos = DeployConfig::default().default_joint_pos;
                loop {
                    let joints: Vec<rpc::JointState> = (0..16)
                        .map(|id| rpc::JointState {
                            id: id as u32,
                            position_rad: default_pos[id],
                            velocity_rad_s: 0.0,
                            torque_nm: 0.0,
                            status_code: 0,
                        })
                        .collect();
                    if tx
                        .send(Ok(rpc::JointSample {
                            joints,
                            source_elapsed: None,
                        }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            });
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
                rx,
            )))
        }

        async fn motors(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::MotorStates>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn motor_voltages(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::MotorVoltagesResponse>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn camera_frame(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::CameraFrame>, Status> {
            Err(Status::unimplemented("unused"))
        }

        type WatchCameraStream = EmptyStream<rpc::CameraFrame>;
        async fn watch_camera(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<Self::WatchCameraStream>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn camera_sample(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::CameraSample>, Status> {
            Err(Status::unimplemented("unused"))
        }

        type WatchCameraSamplesStream = EmptyStream<rpc::CameraSample>;
        async fn watch_camera_samples(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<Self::WatchCameraSamplesStream>, Status> {
            Err(Status::unimplemented("unused"))
        }

        type WatchPointCloudStream = EmptyStream<rpc::CloudFrame>;
        async fn watch_point_cloud(
            &self,
            _: Request<rpc::CloudRequest>,
        ) -> std::result::Result<Response<Self::WatchPointCloudStream>, Status> {
            Err(Status::unimplemented("unused"))
        }
    }

    #[tonic::async_trait]
    impl rpc::motion_server::Motion for MockDosoGateway {
        async fn controller(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::ControllerState>, Status> {
            Err(Status::unimplemented("unused"))
        }

        type ExternalControlStream = tokio_stream::wrappers::ReceiverStream<
            std::result::Result<rpc::ExternalControlReply, Status>,
        >;

        async fn external_control(
            &self,
            request: Request<tonic::Streaming<rpc::ExternalControlRequest>>,
        ) -> std::result::Result<Response<Self::ExternalControlStream>, Status> {
            let mut requests = request.into_inner();
            let (tx, rx) = tokio::sync::mpsc::channel(32);
            let frames = self.frames.clone();
            tokio::spawn(async move {
                let mut enabled = false;
                let mut last_seq = 0_u64;
                while let Ok(Some(req)) = requests.message().await {
                    let cmd = req.command.unwrap_or_default();
                    let is_hb = matches!(
                        req.action,
                        Some(rpc::external_control_request::Action::Heartbeat(_))
                    );
                    let is_close = matches!(
                        req.action,
                        Some(rpc::external_control_request::Action::Close(_))
                    );
                    if let Some(action) = req.action {
                        match action {
                            rpc::external_control_request::Action::Enable(_) => enabled = true,
                            rpc::external_control_request::Action::Disable(_)
                            | rpc::external_control_request::Action::Close(_) => enabled = false,
                            rpc::external_control_request::Action::Frame(f) => {
                                last_seq = req.sequence;
                                frames.lock().unwrap().push(f.joints);
                            }
                            rpc::external_control_request::Action::Enter(_)
                            | rpc::external_control_request::Action::Heartbeat(_) => {}
                        }
                    }
                    let state = rpc::ControllerState {
                        mode: rpc::ControllerMode::External as i32,
                        session_id: "rl-test-session".into(),
                        active: !is_close,
                        motor_output_enabled: enabled,
                        last_sequence: last_seq,
                        reason: String::new(),
                        received_age_ms: Some(1.0),
                        fresh: true,
                        motor_disable_confirmed: !enabled,
                        joint_count: 16,
                        remote: Some(rpc::RemoteState {
                            lx: 0.0,
                            ly: 0.5,
                            rx: 0.0,
                            ry: 0.0,
                            buttons: 0,
                            active: true,
                        }),
                    };
                    if tx
                        .send(Ok(rpc::ExternalControlReply {
                            request_id: if is_hb { String::new() } else { cmd.request_id },
                            sequence: req.sequence,
                            accepted: true,
                            reason: String::new(),
                            ingress_accepted: true,
                            input_window: if is_close { 0 } else { 42 },
                            state: Some(state),
                        }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
                rx,
            )))
        }

        async fn stand_up(
            &self,
            _: Request<rpc::Command>,
        ) -> std::result::Result<Response<rpc::Receipt>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn sit_down(
            &self,
            _: Request<rpc::Command>,
        ) -> std::result::Result<Response<rpc::Receipt>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn damp(
            &self,
            request: Request<rpc::Command>,
        ) -> std::result::Result<Response<rpc::Receipt>, Status> {
            self.damp_calls.fetch_add(1, Ordering::Relaxed);
            let cmd = request.into_inner();
            Ok(Response::new(rpc::Receipt {
                ok: true,
                accepted: Some(true),
                request_id: cmd.request_id,
                ..Default::default()
            }))
        }

        async fn enable_motors(
            &self,
            _: Request<rpc::Command>,
        ) -> std::result::Result<Response<rpc::Receipt>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn disable_motors(
            &self,
            _: Request<rpc::Command>,
        ) -> std::result::Result<Response<rpc::Receipt>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn clear_motor_fault(
            &self,
            _: Request<rpc::ClearMotorFaultRequest>,
        ) -> std::result::Result<Response<rpc::Receipt>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn list_gestures(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::GestureList>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn body_height_status(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::BodyHeightStatus>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn set_body_height(
            &self,
            _: Request<rpc::SetBodyHeightRequest>,
        ) -> std::result::Result<Response<rpc::BodyHeightStatus>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn recovery_status(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::RecoveryStatus>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn recovery_enter(
            &self,
            _: Request<rpc::RecoveryStartRequest>,
        ) -> std::result::Result<Response<rpc::RecoveryStatus>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn recovery_enable(
            &self,
            _: Request<rpc::RecoverySessionRequest>,
        ) -> std::result::Result<Response<rpc::RecoveryStatus>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn recovery_heartbeat(
            &self,
            _: Request<rpc::RecoverySessionRequest>,
        ) -> std::result::Result<Response<rpc::RecoveryStatus>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn recovery_start(
            &self,
            _: Request<rpc::RecoveryStartRequest>,
        ) -> std::result::Result<Response<rpc::RecoveryActionReply>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn recovery_cancel(
            &self,
            _: Request<rpc::RecoveryCancelRequest>,
        ) -> std::result::Result<Response<rpc::RecoveryActionReply>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn recovery_exit(
            &self,
            _: Request<rpc::RecoverySessionRequest>,
        ) -> std::result::Result<Response<rpc::RecoveryStatus>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn play_gesture(
            &self,
            _: Request<rpc::PlayGestureRequest>,
        ) -> std::result::Result<Response<rpc::Receipt>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn profile(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::ProfileInfo>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn switch_profile(
            &self,
            _: Request<rpc::SwitchProfileRequest>,
        ) -> std::result::Result<Response<rpc::Receipt>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn stop(
            &self,
            _: Request<rpc::Command>,
        ) -> std::result::Result<Response<rpc::Receipt>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn stop_motion(
            &self,
            _: Request<rpc::Command>,
        ) -> std::result::Result<Response<rpc::Receipt>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn clear_emergency_stop(
            &self,
            _: Request<rpc::Command>,
        ) -> std::result::Result<Response<rpc::Receipt>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn resume(
            &self,
            _: Request<rpc::Command>,
        ) -> std::result::Result<Response<rpc::Receipt>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn control_status(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::BodyControlStatus>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn control_state(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::ControlState>, Status> {
            Err(Status::unimplemented("unused"))
        }

        type WatchControlStateStream = EmptyStream<rpc::ControlState>;
        async fn watch_control_state(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<Self::WatchControlStateStream>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn velocity(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::VelocitySample>, Status> {
            Err(Status::unimplemented("unused"))
        }

        type WatchVelocityStream = EmptyStream<rpc::VelocitySample>;
        async fn watch_velocity(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<Self::WatchVelocityStream>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn odometry(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::OdometrySample>, Status> {
            Err(Status::unimplemented("unused"))
        }

        type WatchOdometryStream = EmptyStream<rpc::OdometrySample>;
        async fn watch_odometry(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<Self::WatchOdometryStream>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn status(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::Snapshot>, Status> {
            Err(Status::unimplemented("unused"))
        }

        type WatchStateStream = EmptyStream<rpc::Snapshot>;
        async fn watch_state(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<Self::WatchStateStream>, Status> {
            Err(Status::unimplemented("unused"))
        }

        type WatchStateAtStream = EmptyStream<rpc::Snapshot>;
        async fn watch_state_at(
            &self,
            _: Request<rpc::WatchStateRequest>,
        ) -> std::result::Result<Response<Self::WatchStateAtStream>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn body_state(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<rpc::BodyState>, Status> {
            Err(Status::unimplemented("unused"))
        }

        type WatchBodyStateStream = EmptyStream<rpc::BodyState>;
        async fn watch_body_state(
            &self,
            _: Request<rpc::Empty>,
        ) -> std::result::Result<Response<Self::WatchBodyStateStream>, Status> {
            Err(Status::unimplemented("unused"))
        }

        type TeleopStream = EmptyStream<rpc::TeleopReply>;
        async fn teleop(
            &self,
            _: Request<tonic::Streaming<rpc::TeleopRequest>>,
        ) -> std::result::Result<Response<Self::TeleopStream>, Status> {
            Err(Status::unimplemented("unused"))
        }
    }

    #[tokio::test]
    async fn closed_loop_fix_stand_to_policy_and_tilt_soft_damp() {
        let gw = MockDosoGateway::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (srv_tx, srv_rx) = oneshot::channel();
        let server = tonic::transport::Server::builder()
            .add_service(rpc::system_server::SystemServer::new(gw.clone()))
            .add_service(rpc::observations_server::ObservationsServer::new(
                gw.clone(),
            ))
            .add_service(rpc::motion_server::MotionServer::new(gw.clone()))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async move {
                    let _ = srv_rx.await;
                },
            );
        let srv_handle = tokio::spawn(async move {
            let _ = server.await;
        });

        let robot = RobotClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();

        let deploy = DeployConfig {
            step_dt: 0.01,
            fix_stand_duration_s: 0.1,
            ..DeployConfig::default()
        };
        let backend = Box::new(GainScheduleTestBackend::new(&deploy));
        let cfg = RunnerConfig {
            deploy,
            target_cmd: [0.0, 0.0, 0.0],
            max_steps: None,
            skip_fix_stand: false,
            telemetry_wait_timeout: Duration::from_secs(2),
            ..RunnerConfig::default()
        };
        let mut runner = RlRunner::new(cfg, backend).unwrap();
        let (_stop_tx, stop_rx) = oneshot::channel();

        let flip_trigger = gw.flip_imu.clone();
        let frames_ref = gw.frames.clone();
        let flipper = tokio::spawn(async move {
            // Wait until both FixStand and non-zero VelocityPolicy wheel frames have been recorded.
            for _ in 0..100 {
                let count = frames_ref.lock().unwrap().len();
                if count >= 14 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            flip_trigger.store(true, Ordering::Relaxed);
        });

        let report = runner.run(&robot, stop_rx).await.unwrap();
        let _ = flipper.await;

        assert_eq!(report.final_mode, RunnerMode::SoftDamp);
        assert!(report.damp_triggered);
        assert!(report.fix_stand_steps >= 1);
        assert!(report.policy_steps >= 1);
        assert!(gw.damp_calls.load(Ordering::Relaxed) >= 1);

        {
            let frames = gw.frames.lock().unwrap();
            // Verify wheel velocity (kp == 0, non-zero dq from active RC ly=0.5) appeared during VelocityPolicy.
            assert!(
                frames.iter().any(|f| f[12].kp == 0.0 && f[12].dq > 0.5),
                "expected active wheel velocity frame during VelocityPolicy"
            );
            // Verify final damping frame has kp == 0 on all 16 axes.
            let last = frames.last().unwrap();
            assert!(last.iter().all(|j| j.kp == 0.0 && j.dq == 0.0));
        }

        let _ = srv_tx.send(());
        srv_handle.await.unwrap();
    }

    #[tokio::test]
    async fn direct_shm_closed_loop_runs_without_grpc_data_plane_and_records_latency_percentiles() {
        let suffix = format!("{}_{}", std::process::id(), doso_shm_ipc::monotonic_us());
        let proprio_name = format!("/doso_rl_proprio_{suffix}");
        let cmd_name = format!("/doso_rl_cmd_{suffix}");
        let mut proprio_writer = doso_shm_ipc::ProprioRingWriter::open(&proprio_name).unwrap();
        let default_pos = DeployConfig::default().default_joint_pos;

        // Publish continuous upright proprioception into /dev/shm (simulating Brainstem 200Hz writer).
        let stop_feed = Arc::new(AtomicBool::new(false));
        let stop_feed_bg = stop_feed.clone();
        let feeder = tokio::spawn(async move {
            let mut seq = 0_u64;
            while !stop_feed_bg.load(Ordering::Relaxed) {
                seq += 1;
                let payload = doso_shm_ipc::DosoProprioPayload {
                    sequence: seq,
                    monotonic_us: doso_shm_ipc::monotonic_us(),
                    imu_quat_wxyz: [1.0, 0.0, 0.0, 0.0],
                    imu_gyro_rad_s: [0.0, 0.0, 0.0],
                    imu_accel_m_s2: [0.0, 0.0, 9.81],
                    joint_q: default_pos,
                    imu_fresh: 1,
                    ..Default::default()
                };
                let _ = proprio_writer.publish(&payload);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });

        let gw = MockDosoGateway::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (srv_tx, srv_rx) = oneshot::channel();
        let server = tonic::transport::Server::builder()
            .add_service(rpc::system_server::SystemServer::new(gw.clone()))
            .add_service(rpc::observations_server::ObservationsServer::new(
                gw.clone(),
            ))
            .add_service(rpc::motion_server::MotionServer::new(gw.clone()))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async move {
                    let _ = srv_rx.await;
                },
            );
        let srv_handle = tokio::spawn(async move {
            let _ = server.await;
        });

        let robot = RobotClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();
        let deploy = DeployConfig {
            step_dt: 0.005, // 200Hz
            fix_stand_duration_s: 0.1,
            ..DeployConfig::default()
        };
        let backend = Box::new(GainScheduleTestBackend::new(&deploy));
        let cfg = RunnerConfig {
            deploy,
            target_cmd: [0.0, 0.0, 0.0],
            max_steps: Some(30),
            skip_fix_stand: false,
            telemetry_wait_timeout: Duration::from_secs(2),
            shm_enabled: true,
            shm_proprio_name: proprio_name,
            shm_cmd_name: cmd_name.clone(),
        };
        let mut runner = RlRunner::new(cfg, backend).unwrap();
        let (_stop_tx, stop_rx) = oneshot::channel();

        // Simulate Board-A HardwareExternal concurrently polling /dev/shm/doso_ext_cmd_v1
        // while RlRunner is active (keeps the shared memory mapping alive on Windows too).
        let stop_reader = Arc::new(AtomicBool::new(false));
        let stop_reader_bg = stop_reader.clone();
        let cmd_name_reader = cmd_name.clone();
        let captured_cmds = Arc::new(Mutex::new(
            Vec::<doso_shm_ipc::DosoExternalCmdPayload>::new(),
        ));
        let captured_bg = captured_cmds.clone();
        let reader_task = tokio::spawn(async move {
            let mut reader: Option<doso_shm_ipc::ExternalCmdReader> = None;
            let mut last_seq = 0_u64;
            while !stop_reader_bg.load(Ordering::Relaxed) {
                if reader.is_none() {
                    if let Ok(r) = doso_shm_ipc::ExternalCmdReader::open(&cmd_name_reader) {
                        reader = Some(r);
                    }
                }
                if let Some(r) = &reader {
                    if let Some((seq, cmd)) = r.read_latest(last_seq) {
                        last_seq = seq;
                        captured_bg.lock().unwrap().push(cmd);
                    }
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            if let Some(r) = &reader {
                if let Some((_, cmd)) = r.read_latest(last_seq) {
                    captured_bg.lock().unwrap().push(cmd);
                }
            }
        });

        let report = runner.run(&robot, stop_rx).await.unwrap();
        stop_feed.store(true, Ordering::Relaxed);
        let _ = feeder.await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        stop_reader.store(true, Ordering::Relaxed);
        let _ = reader_task.await;

        assert!(report.shm_data_plane_active);
        assert!(!report.damp_triggered);
        assert_eq!(report.fix_stand_steps + report.policy_steps, 30);
        assert!(report.policy_steps >= 1);
        assert_eq!(report.step_latency_us.samples, 30);
        assert_eq!(report.infer_latency_us.samples as u64, report.policy_steps);
        assert!(report.tick_jitter_us.samples >= 29);

        // Zero gRPC data-plane joint frames were sent!
        assert!(
            gw.frames.lock().unwrap().is_empty(),
            "expected zero gRPC Action::Frame messages in direct SHM mode"
        );
        // Verify live SHM MIT frames received by concurrent reader (session binding, active window, wheel velocity, and final damping frame).
        {
            let cmds = captured_cmds.lock().unwrap();
            assert!(!cmds.is_empty(), "expected live SHM MIT frames captured");
            let expected_hash = doso_shm_ipc::fnv1a_64(b"rl-test-session");
            assert!(cmds.iter().all(|c| c.session_id_hash == expected_hash));
            assert!(cmds.iter().all(|c| c.input_window == 42));
            assert!(
                cmds.iter().any(|c| c.kp[12] == 0.0 && c.dq[12] > 0.5),
                "expected VelocityPolicy wheel command delivered over SHM"
            );
            let last_cmd = cmds.last().unwrap();
            assert!(last_cmd.sequence >= 30);
            assert!(last_cmd.kp.iter().all(|&v| v == 0.0));
            assert!(last_cmd.dq.iter().all(|&v| v == 0.0));
        }

        let _ = srv_tx.send(());
        srv_handle.await.unwrap();
    }

    #[tokio::test]
    async fn shm_enabled_falls_back_cleanly_to_grpc_when_ring_absent() {
        let gw = MockDosoGateway::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (srv_tx, srv_rx) = oneshot::channel();
        let server = tonic::transport::Server::builder()
            .add_service(rpc::system_server::SystemServer::new(gw.clone()))
            .add_service(rpc::observations_server::ObservationsServer::new(
                gw.clone(),
            ))
            .add_service(rpc::motion_server::MotionServer::new(gw.clone()))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async move {
                    let _ = srv_rx.await;
                },
            );
        let srv_handle = tokio::spawn(async move {
            let _ = server.await;
        });

        let robot = RobotClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();
        let deploy = DeployConfig {
            step_dt: 0.01,
            fix_stand_duration_s: 0.1,
            ..DeployConfig::default()
        };
        let backend = Box::new(GainScheduleTestBackend::new(&deploy));
        let cfg = RunnerConfig {
            deploy,
            target_cmd: [0.0, 0.0, 0.0],
            max_steps: Some(6),
            skip_fix_stand: true,
            telemetry_wait_timeout: Duration::from_secs(2),
            shm_enabled: true,
            shm_proprio_name: format!("/doso_nonexistent_proprio_{}", std::process::id()),
            shm_cmd_name: format!("/doso_nonexistent_cmd_{}", std::process::id()),
        };
        let mut runner = RlRunner::new(cfg, backend).unwrap();
        let (_stop_tx, stop_rx) = oneshot::channel();

        let report = runner.run(&robot, stop_rx).await.unwrap();
        assert!(!report.shm_data_plane_active);
        assert_eq!(report.fix_stand_steps + report.policy_steps, 6);
        assert!(!gw.frames.lock().unwrap().is_empty());

        let _ = srv_tx.send(());
        srv_handle.await.unwrap();
    }
}
