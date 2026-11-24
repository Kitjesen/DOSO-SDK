use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use doso_sdk::internal::motion_board::rpc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt as _;
use tonic::{Request, Response, Status};

use crate::control::{
    BodyTransitionKind, BrainEngine, BrainStatusSnapshot, FsmState, HistoryFrameSnapshot,
    RecoverySnapshot, SpeedMode,
};
use crate::hardware::hipnuc::rotate_unit_neg_z_by_quat;
use crate::params::NUM_JOINTS;
use crate::types::{ImuState, JointFeedback, MitCmd16, MitTarget, ProprioFrame, WalkCommand};

#[derive(Debug, Clone)]
pub struct TelemetrySnapshot {
    pub updated_us: u64,
    pub refresh_count: u64,
    pub proprio: ProprioFrame,
    pub status: BrainStatusSnapshot,
    pub history_seq: u64,
    pub latest_history: Option<HistoryFrameSnapshot>,
    pub can_read_only: bool,
    pub hal_backend: &'static str,
    pub hal_rx_frames: u64,
}

impl TelemetrySnapshot {
    pub fn from_engine(engine: &BrainEngine, now_us: u64) -> Self {
        let diag = engine.hal.diagnostics();
        Self {
            updated_us: now_us,
            refresh_count: 0,
            proprio: *engine.latest_proprio(),
            status: engine.status_snapshot(now_us),
            history_seq: engine.history_seq(),
            latest_history: engine.latest_history().cloned(),
            can_read_only: !engine.hal.tx_enabled(),
            hal_backend: diag.backend_name,
            hal_rx_frames: diag.rx_frames,
        }
    }
}

pub struct SharedBrain {
    pub engine: Mutex<BrainEngine>,
    pub telemetry: RwLock<TelemetrySnapshot>,
    pub boot_instant: Instant,
    pub rt_tick_max_us: AtomicU64,
    pub rt_jitter_max_us: AtomicU64,
    pub rt_mutex_wait_max_us: AtomicU64,
    pub rt_tick_count: AtomicU64,
}

impl SharedBrain {
    pub fn new(engine: BrainEngine) -> Self {
        let initial = TelemetrySnapshot::from_engine(&engine, 0);
        Self {
            engine: Mutex::new(engine),
            telemetry: RwLock::new(initial),
            boot_instant: Instant::now(),
            rt_tick_max_us: AtomicU64::new(0),
            rt_jitter_max_us: AtomicU64::new(0),
            rt_mutex_wait_max_us: AtomicU64::new(0),
            rt_tick_count: AtomicU64::new(0),
        }
    }

    pub fn now_us(&self) -> u64 {
        self.boot_instant.elapsed().as_micros() as u64
    }

    /// Refreshes the decoupled `RwLock<TelemetrySnapshot>` cache while holding `&BrainEngine`.
    /// Lock hierarchy is strictly `engine -> telemetry` (telemetry readers never lock `engine`),
    /// reusing existing `String`/`Vec` buffers in place to keep steady-state 200Hz ticks zero-alloc.
    pub fn refresh_telemetry(&self, guard: &BrainEngine, now_us: u64) {
        let proprio = *guard.latest_proprio();
        let new_hist_seq = guard.history_seq();
        let can_read_only = !guard.hal.tx_enabled();
        let diag = guard.hal.diagnostics();
        let mut cache = self
            .telemetry
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.copy_status_into(now_us, &mut cache.status);
        if cache.history_seq != new_hist_seq
            || (cache.latest_history.is_none() && guard.latest_history().is_some())
        {
            if let Some(src) = guard.latest_history() {
                if let Some(dst) = cache.latest_history.as_mut() {
                    dst.monotonic_us = src.monotonic_us;
                    dst.gyro_rad_s = src.gyro_rad_s;
                    dst.projected_gravity = src.projected_gravity;
                    dst.walk_cmd = src.walk_cmd;
                    dst.joint_pos = src.joint_pos;
                    dst.joint_vel = src.joint_vel;
                    dst.action = src.action;
                    dst.next_action = src.next_action;
                    dst.kp = src.kp;
                    dst.kd = src.kd;
                    dst.observation.clear();
                    dst.observation.extend_from_slice(&src.observation);
                } else {
                    cache.latest_history = Some(src.clone());
                }
            }
            cache.history_seq = new_hist_seq;
        }
        cache.proprio = proprio;
        cache.can_read_only = can_read_only;
        cache.hal_backend = diag.backend_name;
        cache.hal_rx_frames = diag.rx_frames;
        cache.updated_us = now_us;
        cache.refresh_count = cache.refresh_count.wrapping_add(1);
    }

    #[inline]
    pub fn record_rt_sample(&self, mutex_wait_us: u64, tick_us: u64, jitter_us: u64) {
        self.rt_mutex_wait_max_us
            .fetch_max(mutex_wait_us, Ordering::Relaxed);
        self.rt_tick_max_us.fetch_max(tick_us, Ordering::Relaxed);
        self.rt_jitter_max_us
            .fetch_max(jitter_us, Ordering::Relaxed);
        self.rt_tick_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn reset_rt_metrics(&self) {
        self.rt_mutex_wait_max_us.store(0, Ordering::Relaxed);
        self.rt_tick_max_us.store(0, Ordering::Relaxed);
        self.rt_jitter_max_us.store(0, Ordering::Relaxed);
    }
}

pub struct BrainstemGrpcServer {
    pub shared: Arc<SharedBrain>,
}

type RpcStream<T> = ReceiverStream<std::result::Result<T, Status>>;

fn map_cms_kind(state: FsmState) -> i32 {
    match state {
        FsmState::Grounded => rpc::CmsStateKind::Grounded as i32,
        FsmState::Standing => rpc::CmsStateKind::Standing as i32,
        FsmState::Walking => rpc::CmsStateKind::Walking as i32,
        FsmState::Damping => rpc::CmsStateKind::Damping as i32,
        FsmState::Transitioning | FsmState::Recovery | FsmState::External => {
            rpc::CmsStateKind::Transitioning as i32
        }
    }
}

fn control_status_from_guard(
    guard: &BrainEngine,
    now_us: u64,
    last_decision: i32,
) -> rpc::ControlStatus {
    let st = guard.status_snapshot(now_us);
    rpc::ControlStatus {
        fsm: Some(rpc::CmsState {
            kind: map_cms_kind(st.fsm_state),
            transition: st.transition as i32,
            gesture_name: st.gesture_name,
        }),
        motor_output_enabled: st.motor_output_enabled,
        owner: st.source_owner as i32,
        critical_motor_fault: st.critical_motor_fault,
        grpc_lease_active: st.grpc_lease_active,
        lease_remaining_ms: st.lease_remaining_ms,
        last_accepted_sequence: st.last_accepted_sequence,
        last_decision,
        ready_for_walk: st.fsm_state == FsmState::Standing || st.fsm_state == FsmState::Walking,
        owner_id: st.source_owner_id,
        accepted_count: st.accepted_count,
        rejected_count: st.rejected_count,
    }
}

fn build_control_status(shared: &SharedBrain) -> rpc::ControlStatus {
    let now = shared.now_us();
    let guard = shared.engine.lock().unwrap();
    control_status_from_guard(&guard, now, rpc::CommandRejectReason::None as i32)
}

fn history_to_rpc(h: &HistoryFrameSnapshot, st: &BrainStatusSnapshot) -> rpc::History {
    let cmd_data = if st.fsm_state == FsmState::Damping {
        rpc::command::Data::Damp(())
    } else if st.transition == BodyTransitionKind::StandUp {
        rpc::command::Data::StandUp(())
    } else if st.transition == BodyTransitionKind::SitDown {
        rpc::command::Data::SitDown(())
    } else if st.fsm_state == FsmState::Walking || !h.walk_cmd.is_zero() {
        rpc::command::Data::Walk(rpc::Vector3 {
            x: h.walk_cmd.vx,
            y: h.walk_cmd.vy,
            z: h.walk_cmd.yaw,
        })
    } else {
        rpc::command::Data::Idle(())
    };
    let secs = (h.monotonic_us / 1_000_000) as i64;
    let nanos = ((h.monotonic_us % 1_000_000) * 1_000) as i32;
    rpc::History {
        gyroscope: Some(rpc::Vector3 {
            x: h.gyro_rad_s[0],
            y: h.gyro_rad_s[1],
            z: h.gyro_rad_s[2],
        }),
        projected_gravity: Some(rpc::Vector3 {
            x: h.projected_gravity[0],
            y: h.projected_gravity[1],
            z: h.projected_gravity[2],
        }),
        command: Some(rpc::Command {
            data: Some(cmd_data),
        }),
        joint_position: Some(rpc::Matrix4 {
            values: h.joint_pos.0.to_vec(),
        }),
        joint_velocity: Some(rpc::Matrix4 {
            values: h.joint_vel.0.to_vec(),
        }),
        action: Some(rpc::Matrix4 {
            values: h.action.0.to_vec(),
        }),
        next_action: Some(rpc::Matrix4 {
            values: h.next_action.0.to_vec(),
        }),
        timestamp: Some(prost_types::Duration {
            seconds: secs,
            nanos,
        }),
        kp: Some(rpc::Matrix4 {
            values: h.kp.0.to_vec(),
        }),
        kd: Some(rpc::Matrix4 {
            values: h.kd.0.to_vec(),
        }),
        observation: h.observation.clone(),
    }
}

fn build_profile_info(guard: &BrainEngine) -> rpc::ProfileInfo {
    let available = guard.policies.locomotion_names();
    let current = guard.policies.active_name().to_string();
    let mut descriptions = Vec::with_capacity(available.len());
    let mut current_description = String::new();
    for name in &available {
        let desc = guard
            .policies
            .get(name)
            .map(|p| p.graph.manifest().description.clone())
            .unwrap_or_default();
        if name == &current {
            current_description = desc.clone();
        }
        descriptions.push(desc);
    }

    let mut caps = Vec::new();
    let mut active_mode_id = current.clone();
    for (slot, target) in guard.policies.slots() {
        if target == &current {
            active_mode_id = slot.clone();
        }
        let bh = guard
            .policies
            .get(target)
            .and_then(|p| p.graph.manifest().capabilities.body_height.as_ref())
            .map(|b| rpc::BodyHeightCapability {
                default_height_m: b.default_m,
                min_height_m: b.min_m,
                max_height_m: b.max_m,
                rate_mps: b.rate_mps,
            });
        let kind = if bh.is_some() {
            rpc::RobotModeKind::BodyHeightProfile as i32
        } else {
            rpc::RobotModeKind::LocomotionProfile as i32
        };
        caps.push(rpc::RobotModeCapability {
            id: slot.clone(),
            kind,
            availability: rpc::RobotModeAvailability::Available as i32,
            profile_id: target.clone(),
            unavailable_reason: String::new(),
            body_height: bh,
        });
    }

    if guard.recovery.policy_name().is_some() {
        let rec = guard.recovery_status();
        caps.push(rpc::RobotModeCapability {
            id: "recovery".into(),
            kind: rpc::RobotModeKind::RecoveryAction as i32,
            availability: rec.availability,
            profile_id: String::new(),
            unavailable_reason: rec.unavailable_reason,
            body_height: None,
        });
    }

    rpc::ProfileInfo {
        current,
        available,
        descriptions,
        current_description,
        mode_capabilities: caps,
        active_mode_id,
    }
}

#[tonic::async_trait]
impl rpc::robot_control_server::RobotControl for BrainstemGrpcServer {
    type ListenHistoryStream = RpcStream<rpc::History>;
    type ListenCmsStateStream = RpcStream<rpc::CmsState>;
    type ListenImuStream = RpcStream<rpc::Imu>;
    type ListenJointStream = RpcStream<rpc::Joint>;

    async fn enable(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        guard
            .enable_motors()
            .map_err(|e| Status::failed_precondition(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(()))
    }

    async fn disable(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        guard
            .disable_motors(false)
            .map_err(|e| Status::internal(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(()))
    }

    async fn walk(&self, req: Request<rpc::Vector3>) -> std::result::Result<Response<()>, Status> {
        let v = req.into_inner();
        if !v.x.is_finite() || !v.y.is_finite() || !v.z.is_finite() {
            return Ok(Response::new(()));
        }
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        let _ = guard.set_walk_velocity(WalkCommand {
            vx: v.x.clamp(-3.0, 3.0),
            vy: v.y.clamp(-3.0, 3.0),
            yaw: v.z.clamp(-3.0, 3.0),
        });
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(()))
    }

    async fn walk_checked(
        &self,
        req: Request<rpc::WalkRequest>,
    ) -> std::result::Result<Response<rpc::CommandAck>, Status> {
        let inner = req.into_inner();
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        let caller = if inner.token.is_empty() {
            "local-grpc@bsrl1"
        } else {
            inner.token.as_str()
        };
        if guard
            .arbiter
            .renew_grpc_seq(caller, inner.sequence, now)
            .is_err()
        {
            self.shared.refresh_telemetry(&guard, now);
            let reason = rpc::CommandRejectReason::LeaseMissing as i32;
            let status = control_status_from_guard(&guard, now, reason);
            return Ok(Response::new(rpc::CommandAck {
                accepted: false,
                reason,
                sequence: inner.sequence,
                status: Some(status),
            }));
        }
        if let Some(d) = inner.direction {
            let _ = guard.set_walk_velocity(WalkCommand {
                vx: d.x.clamp(-3.0, 3.0),
                vy: d.y.clamp(-3.0, 3.0),
                yaw: d.z.clamp(-3.0, 3.0),
            });
        }
        self.shared.refresh_telemetry(&guard, now);
        let reason = rpc::CommandRejectReason::None as i32;
        let status = control_status_from_guard(&guard, now, reason);
        Ok(Response::new(rpc::CommandAck {
            accepted: true,
            reason,
            sequence: inner.sequence,
            status: Some(status),
        }))
    }

    async fn acquire_control(
        &self,
        req: Request<rpc::AcquireControlRequest>,
    ) -> std::result::Result<Response<rpc::ControlLease>, Status> {
        let inner = req.into_inner();
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        let caller = if inner.client_id.is_empty() {
            "local-grpc@bsrl1"
        } else {
            inner.client_id.as_str()
        };
        let accepted = guard.arbiter.acquire_grpc(caller, now).is_ok();
        self.shared.refresh_telemetry(&guard, now);
        let reason = if accepted {
            rpc::CommandRejectReason::None as i32
        } else {
            rpc::CommandRejectReason::ControlBusy as i32
        };
        let status = control_status_from_guard(&guard, now, reason);
        Ok(Response::new(rpc::ControlLease {
            accepted,
            token: if accepted {
                caller.to_string()
            } else {
                String::new()
            },
            lease_duration_ms: 350,
            reason,
            status: Some(status),
        }))
    }

    async fn renew_control_lease(
        &self,
        req: Request<rpc::ControlLeaseRequest>,
    ) -> std::result::Result<Response<rpc::ControlLease>, Status> {
        let inner = req.into_inner();
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        let caller = if inner.token.is_empty() {
            "local-grpc@bsrl1"
        } else {
            inner.token.as_str()
        };
        let accepted = guard.arbiter.acquire_grpc(caller, now).is_ok();
        self.shared.refresh_telemetry(&guard, now);
        let reason = if accepted {
            rpc::CommandRejectReason::None as i32
        } else {
            rpc::CommandRejectReason::LeaseExpired as i32
        };
        let status = control_status_from_guard(&guard, now, reason);
        Ok(Response::new(rpc::ControlLease {
            accepted,
            token: if accepted {
                caller.to_string()
            } else {
                String::new()
            },
            lease_duration_ms: 350,
            reason,
            status: Some(status),
        }))
    }

    async fn release_control(
        &self,
        req: Request<rpc::ControlLeaseRequest>,
    ) -> std::result::Result<Response<rpc::ControlStatus>, Status> {
        let inner = req.into_inner();
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        guard.arbiter.release_grpc(&inner.token);
        let _ = guard.set_walk_velocity(WalkCommand::default());
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(control_status_from_guard(
            &guard,
            now,
            rpc::CommandRejectReason::None as i32,
        )))
    }

    async fn stand_up(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        guard
            .stand_up()
            .map_err(|e| Status::failed_precondition(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(()))
    }

    async fn sit_down(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        guard
            .sit_down()
            .map_err(|e| Status::failed_precondition(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(()))
    }

    async fn damp(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        guard.damp().map_err(|e| Status::aborted(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(()))
    }

    async fn set_speed_mode(
        &self,
        req: Request<rpc::SpeedModeRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        let raw = req.into_inner().mode;
        let mode = match rpc::SpeedMode::try_from(raw) {
            Ok(rpc::SpeedMode::Normal) => SpeedMode::Normal,
            Ok(rpc::SpeedMode::High) => SpeedMode::High,
            Err(_) => return Err(Status::invalid_argument(format!("Unknown SpeedMode {raw}"))),
        };
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        guard.set_speed_mode(mode);
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(()))
    }

    async fn get_speed_mode(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::SpeedModeRequest>, Status> {
        let guard = self.shared.engine.lock().unwrap();
        let mode = match guard.speed_mode() {
            SpeedMode::Normal => rpc::SpeedMode::Normal as i32,
            SpeedMode::High => rpc::SpeedMode::High as i32,
        };
        Ok(Response::new(rpc::SpeedModeRequest { mode }))
    }

    async fn tick(&self, _: Request<()>) -> std::result::Result<Response<rpc::History>, Status> {
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        if guard.hal.name().starts_with("socketcan") {
            return Err(Status::failed_precondition(
                "tick is disallowed on physical socketcan backend",
            ));
        }
        let snap = guard.tick_sim(now);
        let st = guard.status_snapshot(now);
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(history_to_rpc(&snap, &st)))
    }

    async fn step(&self, req: Request<rpc::SimState>) -> std::result::Result<Response<()>, Status> {
        let sim = req.into_inner();
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        if guard.hal.name().starts_with("socketcan") {
            return Err(Status::failed_precondition(
                "step is disallowed on physical socketcan backend",
            ));
        }

        let gyro = sim.gyroscope.map(|g| [g.x, g.y, g.z]).unwrap_or([0.0; 3]);
        let quat = sim
            .quaternion
            .map(|q| [q.w, q.x, q.y, q.z])
            .unwrap_or([1.0, 0.0, 0.0, 0.0]);
        if !gyro.iter().all(|v| v.is_finite()) || !quat.iter().all(|v| v.is_finite()) {
            return Err(Status::invalid_argument("non-finite SimState IMU values"));
        }

        let mut pos = [0.0; NUM_JOINTS];
        if let Some(jp) = sim.joint_position {
            if jp.values.len() != NUM_JOINTS || !jp.values.iter().all(|v| v.is_finite()) {
                return Err(Status::invalid_argument(
                    "SimState.joint_position must contain 16 finite values",
                ));
            }
            pos.copy_from_slice(&jp.values);
        }

        let mut vel = [0.0; NUM_JOINTS];
        if let Some(jv) = sim.joint_velocity {
            if jv.values.len() != NUM_JOINTS || !jv.values.iter().all(|v| v.is_finite()) {
                return Err(Status::invalid_argument(
                    "SimState.joint_velocity must contain 16 finite values",
                ));
            }
            vel.copy_from_slice(&jv.values);
        }

        let ts_us = sim
            .timestamp
            .and_then(|d| {
                if d.seconds >= 0 && d.nanos >= 0 {
                    Some((d.seconds as u64) * 1_000_000 + (d.nanos as u64) / 1_000)
                } else {
                    None
                }
            })
            .unwrap_or(now);

        let projected_gravity = rotate_unit_neg_z_by_quat(quat[0], quat[1], quat[2], quat[3]);
        let next_seq = guard.latest_proprio().sequence.wrapping_add(1);
        let status_code = if guard.safety.motor_output_enabled() {
            2
        } else {
            0
        };
        let joints = std::array::from_fn(|i| JointFeedback {
            q: pos[i],
            dq: vel[i],
            tau: 0.0,
            temp_c: 25.0,
            status_code,
            error_bits: 0,
            vbus_v: 50.0,
            last_seen_us: ts_us,
            online: true,
        });

        guard.inject_sim_state(ProprioFrame {
            sequence: next_seq,
            monotonic_us: ts_us,
            imu: ImuState {
                gyro_rad_s: gyro,
                accel_m_s2: [0.0, 0.0, 9.80665],
                quat_wxyz: quat,
                projected_gravity,
                timestamp_us: ts_us,
                online: true,
            },
            joints,
        });
        self.shared.refresh_telemetry(&guard, ts_us);
        Ok(Response::new(()))
    }

    async fn get_start_time(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<prost_types::Timestamp>, Status> {
        Ok(Response::new(prost_types::Timestamp {
            seconds: 0,
            nanos: 0,
        }))
    }

    async fn get_cms_state(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::CmsState>, Status> {
        let snap = self
            .shared
            .telemetry
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let st = &snap.status;
        Ok(Response::new(rpc::CmsState {
            kind: map_cms_kind(st.fsm_state),
            transition: st.transition as i32,
            gesture_name: st.gesture_name.clone(),
        }))
    }

    async fn get_control_status(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::ControlStatus>, Status> {
        Ok(Response::new(build_control_status(&self.shared)))
    }

    async fn listen_history(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<Self::ListenHistoryStream>, Status> {
        let (tx, rx) = mpsc::channel(32);
        let shared = self.shared.clone();
        tokio::spawn(async move {
            let mut last_seq = 0u64;
            loop {
                let msg = {
                    let snap = shared
                        .telemetry
                        .read()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if snap.history_seq != last_seq {
                        last_seq = snap.history_seq;
                        snap.latest_history
                            .as_ref()
                            .map(|h| history_to_rpc(h, &snap.status))
                    } else {
                        None
                    }
                };
                if let Some(history) = msg {
                    if tx.send(Ok(history)).await.is_err() {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn listen_cms_state(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<Self::ListenCmsStateStream>, Status> {
        let (tx, rx) = mpsc::channel(16);
        let shared = self.shared.clone();
        tokio::spawn(async move {
            let mut last = (-1, -1, String::new());
            loop {
                let msg = {
                    let snap = shared
                        .telemetry
                        .read()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let st = &snap.status;
                    rpc::CmsState {
                        kind: map_cms_kind(st.fsm_state),
                        transition: st.transition as i32,
                        gesture_name: st.gesture_name.clone(),
                    }
                };
                let key = (msg.kind, msg.transition, msg.gesture_name.clone());
                if key != last {
                    last = key;
                    if tx.send(Ok(msg)).await.is_err() {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn listen_imu(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<Self::ListenImuStream>, Status> {
        let (tx, rx) = mpsc::channel(32);
        let shared = self.shared.clone();
        tokio::spawn(async move {
            loop {
                let msg = {
                    let snap = shared
                        .telemetry
                        .read()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let p = &snap.proprio;
                    let secs = (p.monotonic_us / 1_000_000) as i64;
                    let nanos = ((p.monotonic_us % 1_000_000) * 1_000) as i32;
                    rpc::Imu {
                        gyroscope: Some(rpc::Vector3 {
                            x: p.imu.gyro_rad_s[0],
                            y: p.imu.gyro_rad_s[1],
                            z: p.imu.gyro_rad_s[2],
                        }),
                        quaternion: Some(rpc::Quaternion {
                            w: p.imu.quat_wxyz[0],
                            x: p.imu.quat_wxyz[1],
                            y: p.imu.quat_wxyz[2],
                            z: p.imu.quat_wxyz[3],
                        }),
                        linear_acceleration: Some(rpc::Vector3 {
                            x: p.imu.accel_m_s2[0],
                            y: p.imu.accel_m_s2[1],
                            z: p.imu.accel_m_s2[2],
                        }),
                        timestamp: Some(prost_types::Duration {
                            seconds: secs,
                            nanos,
                        }),
                    }
                };
                if tx.send(Ok(msg)).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn listen_joint(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<Self::ListenJointStream>, Status> {
        let (tx, rx) = mpsc::channel(32);
        let shared = self.shared.clone();
        tokio::spawn(async move {
            loop {
                let msg = {
                    let snap = shared
                        .telemetry
                        .read()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let p = &snap.proprio;
                    let secs = (p.monotonic_us / 1_000_000) as i64;
                    let nanos = ((p.monotonic_us % 1_000_000) * 1_000) as i32;
                    rpc::Joint {
                        data: Some(rpc::joint::Data::AllJoints(rpc::AllJoints {
                            position: Some(rpc::Matrix4 {
                                values: p.joints.iter().map(|j| j.q).collect(),
                            }),
                            velocity: Some(rpc::Matrix4 {
                                values: p.joints.iter().map(|j| j.dq).collect(),
                            }),
                            torque: Some(rpc::Matrix4 {
                                values: p.joints.iter().map(|j| j.tau).collect(),
                            }),
                            status: Some(rpc::Matrix4Int32 {
                                values: p.joints.iter().map(|j| j.status_code as u32).collect(),
                            }),
                        })),
                        timestamp: Some(prost_types::Duration {
                            seconds: secs,
                            nanos,
                        }),
                    }
                };
                if tx.send(Ok(msg)).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn get_params(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::Params>, Status> {
        let guard = self.shared.engine.lock().unwrap();
        let stand_pose = guard
            .policies
            .active()
            .map(|p| p.graph.manifest().posture.standing_pose.to_vec())
            .unwrap_or_else(|| vec![0.0; NUM_JOINTS]);
        Ok(Response::new(rpc::Params {
            robot: Some(rpc::RobotModel {
                r#type: rpc::RobotType::Han as i32,
                initial_joint_position: Some(rpc::Matrix4 { values: stand_pose }),
                initial_joint_velocity: Some(rpc::Matrix4 {
                    values: vec![0.0; NUM_JOINTS],
                }),
            }),
        }))
    }

    async fn switch_profile(
        &self,
        req: Request<rpc::ProfileRequest>,
    ) -> std::result::Result<Response<rpc::ProfileInfo>, Status> {
        let target = req.into_inner().name;
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        guard
            .switch_profile(&target)
            .map_err(|e| Status::failed_precondition(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(build_profile_info(&guard)))
    }

    async fn set_body_height(
        &self,
        req: Request<rpc::BodyHeightCommand>,
    ) -> std::result::Result<Response<rpc::BodyHeightStatus>, Status> {
        let h = req.into_inner().height_m;
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        let st = guard
            .set_body_height(h)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(rpc::BodyHeightStatus {
            supported: st.supported,
            current_height_m: st.current_height_m,
            target_height_m: st.target_height_m,
            min_height_m: st.min_height_m,
            max_height_m: st.max_height_m,
            rate_mps: st.rate_mps,
        }))
    }

    async fn get_body_height_status(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::BodyHeightStatus>, Status> {
        let guard = self.shared.engine.lock().unwrap();
        let st = guard.body_height_status();
        Ok(Response::new(rpc::BodyHeightStatus {
            supported: st.supported,
            current_height_m: st.current_height_m,
            target_height_m: st.target_height_m,
            min_height_m: st.min_height_m,
            max_height_m: st.max_height_m,
            rate_mps: st.rate_mps,
        }))
    }

    async fn get_profile(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::ProfileInfo>, Status> {
        let guard = self.shared.engine.lock().unwrap();
        Ok(Response::new(build_profile_info(&guard)))
    }

    async fn play_gesture(
        &self,
        req: Request<rpc::GestureRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        let name = req.into_inner().name;
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        guard
            .play_gesture(&name)
            .map_err(|e| Status::failed_precondition(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(()))
    }

    async fn list_gestures(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::GestureList>, Status> {
        let guard = self.shared.engine.lock().unwrap();
        let gestures = guard
            .gestures
            .names()
            .into_iter()
            .filter_map(|n| {
                let c = guard.gestures.get(&n)?;
                let counts: usize = c.keyframes.iter().map(|k| k.counts).sum();
                Some(rpc::GestureInfo {
                    name: c.name.clone(),
                    description: c.description.clone(),
                    duration_ms: (counts * 20) as i32,
                })
            })
            .collect();
        Ok(Response::new(rpc::GestureList { gestures }))
    }

    async fn get_voltage(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::Voltage>, Status> {
        let values = {
            let snap = self
                .shared
                .telemetry
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            snap.proprio
                .joints
                .iter()
                .map(|j| j.vbus_v as f64)
                .collect()
        };
        Ok(Response::new(rpc::Voltage { values }))
    }

    async fn get_motor_status(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::MotorStatusResponse>, Status> {
        let motors = {
            let snap = self
                .shared
                .telemetry
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            snap.proprio
                .joints
                .iter()
                .enumerate()
                .map(|(idx, j)| rpc::MotorState {
                    id: idx as u32,
                    online: j.online,
                    status_code: j.status_code as u32,
                    temperature: j.temp_c as f64,
                    voltage: j.vbus_v as f64,
                    position: j.q,
                    velocity: j.dq,
                    torque: j.tau,
                    errors: if j.error_bits == 0 {
                        Vec::new()
                    } else {
                        vec![j.error_bits as u32]
                    },
                })
                .collect()
        };
        Ok(Response::new(rpc::MotorStatusResponse { motors }))
    }

    async fn clear_motor_fault(
        &self,
        _: Request<rpc::ClearFaultRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        guard
            .clear_motor_fault()
            .map_err(|e| Status::internal(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(()))
    }

    async fn set_zero(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
        if std::env::var("THUNDER_ALLOW_SET_ZERO").as_deref() != Ok("true") {
            return Err(Status::permission_denied(
                "Factory calibration set_zero requires THUNDER_ALLOW_SET_ZERO=true",
            ));
        }
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        guard
            .calibrate_set_zero()
            .map_err(|e| Status::failed_precondition(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(()))
    }
}

// ─── RobotExternalControl Server ──────────────────────────────────────────────

fn ext_status(guard: &BrainEngine, include_tok: bool) -> rpc::ExternalStatus {
    rpc::ExternalStatus {
        selected: guard.external.active,
        active: guard.external.active,
        session_id: guard.external.session_id.clone(),
        session_token: if include_tok {
            guard.external.session_id.clone()
        } else {
            String::new()
        },
        motor_output_enabled: guard.safety.motor_output_enabled(),
        last_sequence: guard.external.last_sequence,
        reason: if guard.external.active {
            "external_active".into()
        } else {
            "builtin".into()
        },
        motor_disable_confirmed: !guard.safety.motor_output_enabled(),
        client_id: "external@sdk".into(),
        input_window: 1,
        joint_count: 16,
    }
}

#[tonic::async_trait]
impl rpc::robot_external_control_server::RobotExternalControl for BrainstemGrpcServer {
    type ControlStream = RpcStream<rpc::ExternalStatus>;

    async fn enter(
        &self,
        req: Request<rpc::ExternalEnterRequest>,
    ) -> std::result::Result<Response<rpc::ExternalStatus>, Status> {
        let inner = req.into_inner();
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        let sid = if !inner.request_id.is_empty() {
            inner.request_id
        } else if !inner.client_id.is_empty() {
            inner.client_id
        } else {
            format!("ext-{now:x}")
        };
        guard
            .external_enter(&sid, now)
            .map_err(|e| Status::failed_precondition(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(ext_status(&guard, true)))
    }

    async fn enable(
        &self,
        _: Request<rpc::ExternalSession>,
    ) -> std::result::Result<Response<rpc::ExternalStatus>, Status> {
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        guard
            .external_enable_motors()
            .map_err(|e| Status::failed_precondition(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(ext_status(&guard, false)))
    }

    async fn disable(
        &self,
        _: Request<rpc::ExternalSession>,
    ) -> std::result::Result<Response<rpc::ExternalStatus>, Status> {
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        guard
            .disable_motors(false)
            .map_err(|e| Status::internal(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(ext_status(&guard, false)))
    }

    async fn heartbeat(
        &self,
        _: Request<rpc::ExternalSession>,
    ) -> std::result::Result<Response<rpc::ExternalStatus>, Status> {
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        if guard.external.active {
            guard.external.expires_us = now + 500_000;
        }
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(ext_status(&guard, false)))
    }

    async fn exit(
        &self,
        _: Request<rpc::ExternalSession>,
    ) -> std::result::Result<Response<rpc::ExternalStatus>, Status> {
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        let _ = guard.external_exit();
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(ext_status(&guard, false)))
    }

    async fn get_status(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::ExternalStatus>, Status> {
        let guard = self.shared.engine.lock().unwrap();
        Ok(Response::new(ext_status(&guard, false)))
    }

    async fn control(
        &self,
        req: Request<tonic::Streaming<rpc::ExternalFrame>>,
    ) -> std::result::Result<Response<Self::ControlStream>, Status> {
        let mut inbound = req.into_inner();
        let (tx, rx) = mpsc::channel(16);
        let shared = self.shared.clone();
        tokio::spawn(async move {
            while let Some(Ok(frame)) = inbound.next().await {
                let status = {
                    let now = shared.now_us();
                    let mut guard = shared.engine.lock().unwrap();
                    if frame.targets.len() == NUM_JOINTS {
                        let mut targets = [MitTarget::zero(); NUM_JOINTS];
                        for (i, t) in frame.targets.iter().enumerate() {
                            targets[i] = MitTarget {
                                q: t.q,
                                dq: t.dq,
                                kp: t.kp,
                                kd: t.kd,
                                tau: t.tau,
                            };
                        }
                        let _ = guard.external_submit_frame(frame.sequence, MitCmd16(targets), now);
                    } else if guard.external.active {
                        guard.external.expires_us = now + 500_000;
                    }
                    shared.refresh_telemetry(&guard, now);
                    ext_status(&guard, false)
                };
                if tx.send(Ok(status)).await.is_err() {
                    break;
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

// ─── RobotRecovery Server ─────────────────────────────────────────────────────

fn to_rpc_rec_status(s: RecoverySnapshot, include_tok: bool) -> rpc::RecoveryStatus {
    rpc::RecoveryStatus {
        availability: s.availability,
        phase: s.phase,
        operation_id: s.operation_id,
        unavailable_reason: s.unavailable_reason,
        selected: s.selected,
        session_token: if include_tok {
            s.session_token
        } else {
            String::new()
        },
        motor_disable_confirmed: s.motor_disable_confirmed,
    }
}

#[tonic::async_trait]
impl rpc::robot_recovery_server::RobotRecovery for BrainstemGrpcServer {
    async fn enter(
        &self,
        req: Request<rpc::RecoveryStartRequest>,
    ) -> std::result::Result<Response<rpc::RecoveryStatus>, Status> {
        let inner = req.into_inner();
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        let grounded_free = guard.recovery_status().availability == 2 && !guard.recovery.selected();
        let _ = guard
            .recovery
            .enter(&inner.operation_id, grounded_free, now)
            .map_err(|e| Status::failed_precondition(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(to_rpc_rec_status(
            guard.recovery_status(),
            true,
        )))
    }

    async fn enable(
        &self,
        req: Request<rpc::RecoverySessionRequest>,
    ) -> std::result::Result<Response<rpc::RecoveryStatus>, Status> {
        let tok = req.into_inner().session_token;
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        guard
            .recovery_enable(&tok, now)
            .map_err(|e| Status::failed_precondition(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(to_rpc_rec_status(
            guard.recovery_status(),
            false,
        )))
    }

    async fn heartbeat(
        &self,
        req: Request<rpc::RecoverySessionRequest>,
    ) -> std::result::Result<Response<rpc::RecoveryStatus>, Status> {
        let tok = req.into_inner().session_token;
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        guard
            .recovery
            .heartbeat(&tok, now)
            .map_err(|e| Status::failed_precondition(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(to_rpc_rec_status(
            guard.recovery_status(),
            false,
        )))
    }

    async fn exit(
        &self,
        req: Request<rpc::RecoverySessionRequest>,
    ) -> std::result::Result<Response<rpc::RecoveryStatus>, Status> {
        let tok = req.into_inner().session_token;
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        let _ = guard.disable_motors(false);
        guard
            .recovery
            .exit(&tok)
            .map_err(|e| Status::failed_precondition(e.to_string()))?;
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(to_rpc_rec_status(
            guard.recovery_status(),
            false,
        )))
    }

    async fn get_status(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::RecoveryStatus>, Status> {
        let guard = self.shared.engine.lock().unwrap();
        Ok(Response::new(to_rpc_rec_status(
            guard.recovery_status(),
            false,
        )))
    }

    async fn start(
        &self,
        req: Request<rpc::RecoveryStartRequest>,
    ) -> std::result::Result<Response<rpc::RecoveryActionReply>, Status> {
        let inner = req.into_inner();
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        let Some(rec_name) = guard.recovery.policy_name().map(str::to_owned) else {
            return Err(Status::failed_precondition("Recovery policy not loaded"));
        };
        let start_res = {
            let BrainEngine {
                recovery, policies, ..
            } = &mut *guard;
            let Some(pkg) = policies.get_mut(&rec_name) else {
                return Err(Status::failed_precondition("Recovery package missing"));
            };
            recovery.start(&inner.session_token, &inner.operation_id, pkg, now)
        };
        self.shared.refresh_telemetry(&guard, now);
        match start_res {
            Ok(()) => Ok(Response::new(rpc::RecoveryActionReply {
                operation_id: inner.operation_id,
                decision: rpc::RecoveryActionDecision::Accepted as i32,
                phase: rpc::RecoveryPhase::Recovering as i32,
                reason: String::new(),
            })),
            Err(e) => Ok(Response::new(rpc::RecoveryActionReply {
                operation_id: inner.operation_id,
                decision: rpc::RecoveryActionDecision::Rejected as i32,
                phase: rpc::RecoveryPhase::Failed as i32,
                reason: e.to_string(),
            })),
        }
    }

    async fn cancel(
        &self,
        req: Request<rpc::RecoveryCancelRequest>,
    ) -> std::result::Result<Response<rpc::RecoveryActionReply>, Status> {
        let inner = req.into_inner();
        let now = self.shared.now_us();
        let mut guard = self.shared.engine.lock().unwrap();
        let _ = guard.disable_motors(false);
        let _ = guard.recovery.cancel(&inner.session_token);
        self.shared.refresh_telemetry(&guard, now);
        Ok(Response::new(rpc::RecoveryActionReply {
            operation_id: inner.operation_id,
            decision: rpc::RecoveryActionDecision::Accepted as i32,
            phase: rpc::RecoveryPhase::Failed as i32,
            reason: "cancelled".into(),
        }))
    }
}

// ─── Read-Only Local Status Server (:13146 compat for doso-cloud-link) ───────

/// Read-only `brainstem.api.v1.RobotControl` server bound to loopback (`127.0.0.1:13146`).
/// Serves `GetControlStatus` and read-only queries while rejecting every mutating RPC
/// with `Status::permission_denied`.
pub struct LocalStatusGrpcServer {
    pub shared: Arc<SharedBrain>,
}

fn readonly_denied() -> Status {
    Status::permission_denied("local status endpoint (:13146) is read-only")
}

#[tonic::async_trait]
impl rpc::robot_control_server::RobotControl for LocalStatusGrpcServer {
    type ListenHistoryStream = RpcStream<rpc::History>;
    type ListenCmsStateStream = RpcStream<rpc::CmsState>;
    type ListenImuStream = RpcStream<rpc::Imu>;
    type ListenJointStream = RpcStream<rpc::Joint>;

    async fn enable(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
        Err(readonly_denied())
    }

    async fn disable(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
        Err(readonly_denied())
    }

    async fn walk(&self, _: Request<rpc::Vector3>) -> std::result::Result<Response<()>, Status> {
        Err(readonly_denied())
    }

    async fn walk_checked(
        &self,
        _: Request<rpc::WalkRequest>,
    ) -> std::result::Result<Response<rpc::CommandAck>, Status> {
        Err(readonly_denied())
    }

    async fn acquire_control(
        &self,
        _: Request<rpc::AcquireControlRequest>,
    ) -> std::result::Result<Response<rpc::ControlLease>, Status> {
        Err(readonly_denied())
    }

    async fn renew_control_lease(
        &self,
        _: Request<rpc::ControlLeaseRequest>,
    ) -> std::result::Result<Response<rpc::ControlLease>, Status> {
        Err(readonly_denied())
    }

    async fn release_control(
        &self,
        _: Request<rpc::ControlLeaseRequest>,
    ) -> std::result::Result<Response<rpc::ControlStatus>, Status> {
        Err(readonly_denied())
    }

    async fn stand_up(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
        Err(readonly_denied())
    }

    async fn sit_down(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
        Err(readonly_denied())
    }

    async fn damp(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
        Err(readonly_denied())
    }

    async fn set_speed_mode(
        &self,
        _: Request<rpc::SpeedModeRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        Err(readonly_denied())
    }

    async fn get_speed_mode(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::SpeedModeRequest>, Status> {
        let guard = self.shared.engine.lock().unwrap();
        let mode = match guard.speed_mode() {
            SpeedMode::Normal => rpc::SpeedMode::Normal as i32,
            SpeedMode::High => rpc::SpeedMode::High as i32,
        };
        Ok(Response::new(rpc::SpeedModeRequest { mode }))
    }

    async fn tick(&self, _: Request<()>) -> std::result::Result<Response<rpc::History>, Status> {
        Err(readonly_denied())
    }

    async fn step(&self, _: Request<rpc::SimState>) -> std::result::Result<Response<()>, Status> {
        Err(readonly_denied())
    }

    async fn get_start_time(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<prost_types::Timestamp>, Status> {
        Ok(Response::new(prost_types::Timestamp {
            seconds: 0,
            nanos: 0,
        }))
    }

    async fn get_cms_state(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::CmsState>, Status> {
        let now = self.shared.now_us();
        let guard = self.shared.engine.lock().unwrap();
        let st = guard.status_snapshot(now);
        Ok(Response::new(rpc::CmsState {
            kind: map_cms_kind(st.fsm_state),
            transition: st.transition as i32,
            gesture_name: st.gesture_name,
        }))
    }

    async fn get_control_status(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::ControlStatus>, Status> {
        Ok(Response::new(build_control_status(&self.shared)))
    }

    async fn listen_history(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<Self::ListenHistoryStream>, Status> {
        Err(readonly_denied())
    }

    async fn listen_cms_state(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<Self::ListenCmsStateStream>, Status> {
        Err(readonly_denied())
    }

    async fn listen_imu(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<Self::ListenImuStream>, Status> {
        Err(readonly_denied())
    }

    async fn listen_joint(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<Self::ListenJointStream>, Status> {
        Err(readonly_denied())
    }

    async fn get_params(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::Params>, Status> {
        Err(readonly_denied())
    }

    async fn switch_profile(
        &self,
        _: Request<rpc::ProfileRequest>,
    ) -> std::result::Result<Response<rpc::ProfileInfo>, Status> {
        Err(readonly_denied())
    }

    async fn set_body_height(
        &self,
        _: Request<rpc::BodyHeightCommand>,
    ) -> std::result::Result<Response<rpc::BodyHeightStatus>, Status> {
        Err(readonly_denied())
    }

    async fn get_body_height_status(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::BodyHeightStatus>, Status> {
        let guard = self.shared.engine.lock().unwrap();
        let st = guard.body_height_status();
        Ok(Response::new(rpc::BodyHeightStatus {
            supported: st.supported,
            current_height_m: st.current_height_m,
            target_height_m: st.target_height_m,
            min_height_m: st.min_height_m,
            max_height_m: st.max_height_m,
            rate_mps: st.rate_mps,
        }))
    }

    async fn get_profile(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::ProfileInfo>, Status> {
        let guard = self.shared.engine.lock().unwrap();
        Ok(Response::new(build_profile_info(&guard)))
    }

    async fn play_gesture(
        &self,
        _: Request<rpc::GestureRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        Err(readonly_denied())
    }

    async fn list_gestures(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::GestureList>, Status> {
        Err(readonly_denied())
    }

    async fn get_voltage(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::Voltage>, Status> {
        Err(readonly_denied())
    }

    async fn get_motor_status(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<rpc::MotorStatusResponse>, Status> {
        Err(readonly_denied())
    }

    async fn clear_motor_fault(
        &self,
        _: Request<rpc::ClearFaultRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        Err(readonly_denied())
    }

    async fn set_zero(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
        Err(readonly_denied())
    }
}
