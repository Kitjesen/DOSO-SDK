use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use doso_sdk::internal::motion_board::{rpc, MotionBoardClient, MotionBoardConfig};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

use crate::dds_bus::{
    DdsBus, DdsStamp, DriverControlStateMsg, ImuBodySample, JointStateSample, PostureAckMsg,
    ProfileStateMsg,
};
use crate::freshness::{CmdVelFreshnessSample, CrossBoardFreshnessGate, FreshnessDecision};

pub const POSTURE_ACTION_STAND_UP: i32 = 11;
pub const POSTURE_ACTION_SIT_DOWN: i32 = 12;
pub const POSTURE_ACTION_ENABLE_MOTORS: i32 = 13;
pub const POSTURE_ACTION_DISABLE_MOTORS: i32 = 14;
pub const POSTURE_ACTION_SWITCH_PROFILE: i32 = 15;
pub const POSTURE_ACTION_DAMP: i32 = 16;

pub const POSTURE_PHASE_INSPECT: i32 = 0;
pub const POSTURE_PHASE_EXECUTE: i32 = 1;
pub const POSTURE_PHASE_CANCEL: i32 = 2;

pub const DEFAULT_LOOPBACK_CLIENT_ID: &str = "local-grpc@bsrl1";
const MIN_WALK_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Debug, Clone)]
pub struct BridgeConfig {
    pub brainstem_endpoint: String,
    pub client_id: String,
    pub expected_host_boot_id: String,
    pub cross_board_clock: bool,
    pub command_timeout: Duration,
    pub poll_period: Duration,
    pub state_publish_period: Duration,
    pub reconnect_backoff: Duration,
    pub max_linear_mps: f64,
    pub max_angular_rps: f64,
    pub producer_boot_id: String,
    pub robot_model: String,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            brainstem_endpoint: doso_sdk::internal::motion_board::DEFAULT_ENDPOINT.to_string(),
            client_id: DEFAULT_LOOPBACK_CLIENT_ID.to_string(),
            expected_host_boot_id: "*".to_string(),
            cross_board_clock: true,
            command_timeout: Duration::from_millis(200),
            poll_period: Duration::from_millis(10),
            state_publish_period: Duration::from_millis(50),
            reconnect_backoff: Duration::from_millis(250),
            max_linear_mps: 2.5,
            max_angular_rps: 2.5,
            producer_boot_id: default_producer_boot_id(),
            robot_model: "thunder".to_string(),
        }
    }
}

fn default_producer_boot_id() -> String {
    #[cfg(target_os = "linux")]
    if let Ok(raw) = std::fs::read_to_string("/proc/sys/kernel/random/boot_id") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    "board-a".to_string()
}

pub fn current_boottime_ns() -> u64 {
    #[cfg(target_os = "linux")]
    {
        #[repr(C)]
        struct Timespec {
            tv_sec: i64,
            tv_nsec: i64,
        }
        extern "C" {
            fn clock_gettime(clk_id: i32, tp: *mut Timespec) -> i32;
        }
        const CLOCK_BOOTTIME: i32 = 7;
        let mut ts = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if unsafe { clock_gettime(CLOCK_BOOTTIME, &mut ts) } == 0
            && ts.tv_sec >= 0
            && ts.tv_nsec >= 0
        {
            return (ts.tv_sec as u64)
                .saturating_mul(1_000_000_000)
                .saturating_add(ts.tv_nsec as u64);
        }
    }
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    1_000_000_000_u64.saturating_add(start.elapsed().as_nanos() as u64)
}

fn now_wall_stamp() -> DdsStamp {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    DdsStamp {
        sec: dur.as_secs().min(i32::MAX as u64) as i32,
        nanosec: dur.subsec_nanos(),
    }
}

fn map_imu_sample(imu: &rpc::Imu) -> ImuBodySample {
    let q = imu.quaternion.unwrap_or(rpc::Quaternion {
        w: 1.0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    });
    let gyro = imu.gyroscope.unwrap_or(rpc::Vector3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    });
    let acc = imu.linear_acceleration.unwrap_or(rpc::Vector3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    });
    ImuBodySample {
        stamp: now_wall_stamp(),
        orientation_xyzw: [q.x, q.y, q.z, q.w],
        angular_velocity_xyz: [gyro.x, gyro.y, gyro.z],
        linear_acceleration_xyz: [acc.x, acc.y, acc.z],
    }
}

#[derive(Default)]
struct JointAssembler {
    position: [f64; 16],
    velocity: [f64; 16],
    effort: [f64; 16],
    ever_seen_mask: u16,
    cycle_mask: u16,
}

impl JointAssembler {
    fn apply(&mut self, msg: &rpc::Joint, robot_model: &str) -> Option<JointStateSample> {
        let data = msg.data.as_ref()?;
        match data {
            rpc::joint::Data::AllJoints(all) => {
                if let Some(pos) = &all.position {
                    for (dst, &src) in self.position.iter_mut().zip(pos.values.iter()) {
                        *dst = src;
                    }
                }
                if let Some(vel) = &all.velocity {
                    for (dst, &src) in self.velocity.iter_mut().zip(vel.values.iter()) {
                        *dst = src;
                    }
                }
                if let Some(tau) = &all.torque {
                    for (dst, &src) in self.effort.iter_mut().zip(tau.values.iter()) {
                        *dst = src;
                    }
                }
                self.ever_seen_mask = u16::MAX;
                self.cycle_mask = 0;
                Some(self.snapshot(robot_model))
            }
            rpc::joint::Data::SingleJoint(sj) => {
                let idx = sj.id as usize;
                if idx >= 16 {
                    return None;
                }
                self.position[idx] = sj.position;
                self.velocity[idx] = sj.velocity;
                self.effort[idx] = sj.torque;
                let bit = 1_u16 << idx;
                self.ever_seen_mask |= bit;
                self.cycle_mask |= bit;
                if self.cycle_mask == u16::MAX || (self.ever_seen_mask == u16::MAX && idx == 15) {
                    self.cycle_mask = 0;
                    return Some(self.snapshot(robot_model));
                }
                None
            }
        }
    }

    fn snapshot(&self, robot_model: &str) -> JointStateSample {
        JointStateSample {
            stamp: now_wall_stamp(),
            robot_model: robot_model.to_string(),
            position: self.position,
            velocity: self.velocity,
            effort: self.effort,
        }
    }
}

fn cms_kind_label(kind: i32) -> &'static str {
    match rpc::CmsStateKind::try_from(kind) {
        Ok(rpc::CmsStateKind::Zero) => "Zero",
        Ok(rpc::CmsStateKind::Grounded) => "Grounded",
        Ok(rpc::CmsStateKind::Standing) => "Standing",
        Ok(rpc::CmsStateKind::Walking) => "Walking",
        Ok(rpc::CmsStateKind::Transitioning) => "Transitioning",
        Ok(rpc::CmsStateKind::Damping) => "Damping",
        Err(_) => "Unknown",
    }
}

fn owner_label(owner: i32) -> &'static str {
    match rpc::ControlOwner::try_from(owner) {
        Ok(rpc::ControlOwner::None) => "None",
        Ok(rpc::ControlOwner::Grpc) => "Grpc",
        Ok(rpc::ControlOwner::Yunzhuo) => "Yunzhuo",
        Err(_) => "Unknown",
    }
}

fn reject_reason_label(reason: i32) -> &'static str {
    match rpc::CommandRejectReason::try_from(reason) {
        Ok(rpc::CommandRejectReason::None) => "",
        Ok(other) => other.as_str_name(),
        Err(_) => "UNKNOWN_REJECT_REASON",
    }
}

fn map_control_state(status: &rpc::ControlStatus, zero_output: bool) -> DriverControlStateMsg {
    let fsm = status
        .fsm
        .as_ref()
        .map(|s| cms_kind_label(s.kind))
        .unwrap_or("Unknown")
        .to_string();
    DriverControlStateMsg {
        stamp: now_wall_stamp(),
        connected: true,
        ready: status.ready_for_walk,
        motors_enabled: status.motor_output_enabled,
        critical_fault: status.critical_motor_fault,
        control_assured: status.grpc_lease_active && !status.critical_motor_fault,
        lease_valid: status.grpc_lease_active,
        lease_remaining_ms: status.lease_remaining_ms,
        accepted_sequence: status.last_accepted_sequence,
        zero_output,
        fsm,
        owner: owner_label(status.owner).to_string(),
        owner_id: status.owner_id.clone(),
        reason: reject_reason_label(status.last_decision).to_string(),
    }
}

fn disconnected_control_state(reason: String) -> DriverControlStateMsg {
    DriverControlStateMsg {
        stamp: now_wall_stamp(),
        connected: false,
        ready: false,
        motors_enabled: false,
        critical_fault: false,
        control_assured: false,
        lease_valid: false,
        lease_remaining_ms: 0,
        accepted_sequence: 0,
        zero_output: true,
        fsm: "Disconnected".to_string(),
        owner: "None".to_string(),
        owner_id: String::new(),
        reason,
    }
}

pub struct BridgeRuntime {
    config: BridgeConfig,
    bus: Arc<dyn DdsBus>,
}

impl BridgeRuntime {
    pub fn new(config: BridgeConfig, bus: Arc<dyn DdsBus>) -> Self {
        Self { config, bus }
    }

    pub async fn run_until(self, shutdown: oneshot::Receiver<()>) -> Result<()> {
        let (stop_tx, mut stop_rx) = watch::channel(false);
        tokio::spawn(async move {
            let _ = shutdown.await;
            let _ = stop_tx.send(true);
        });

        loop {
            if *stop_rx.borrow() {
                break;
            }

            let connect_res = tokio::select! {
                _ = stop_rx.changed() => break,
                res = MotionBoardClient::with_config(MotionBoardConfig::new(
                    self.config.brainstem_endpoint.clone(),
                )) => res,
            };

            let client = match connect_res {
                Ok(c) => Arc::new(c),
                Err(err) => {
                    tracing::warn!(
                        endpoint = %self.config.brainstem_endpoint,
                        error = %err,
                        "failed to connect to Brainstem MotionBoardClient; retrying"
                    );
                    let _ = self
                        .bus
                        .write_control_state(&disconnected_control_state(err.to_string()));
                    tokio::select! {
                        _ = stop_rx.changed() => break,
                        _ = tokio::time::sleep(self.config.reconnect_backoff) => continue,
                    }
                }
            };

            if let Err(err) = self.run_connected_session(client, &mut stop_rx).await {
                tracing::warn!(error = %err, "bridge session ended with error; will reconnect");
                let _ = self
                    .bus
                    .write_control_state(&disconnected_control_state(err.to_string()));
            }

            if *stop_rx.borrow() {
                break;
            }
            tokio::select! {
                _ = stop_rx.changed() => break,
                _ = tokio::time::sleep(self.config.reconnect_backoff) => {}
            }
        }

        Ok(())
    }

    async fn run_connected_session(
        &self,
        client: Arc<MotionBoardClient>,
        stop_rx: &mut watch::Receiver<bool>,
    ) -> Result<()> {
        let imu_ok = Arc::new(AtomicBool::new(true));
        let joint_ok = Arc::new(AtomicBool::new(true));

        let imu_task = spawn_imu_forwarder(client.clone(), self.bus.clone(), imu_ok.clone());
        let joint_task = spawn_joint_forwarder(
            client.clone(),
            self.bus.clone(),
            self.config.robot_model.clone(),
            joint_ok.clone(),
        );

        let result = self
            .control_and_state_loop(client.clone(), stop_rx, &imu_ok, &joint_ok)
            .await;

        imu_task.abort();
        joint_task.abort();
        result
    }

    async fn control_and_state_loop(
        &self,
        client: Arc<MotionBoardClient>,
        stop_rx: &mut watch::Receiver<bool>,
        imu_ok: &AtomicBool,
        joint_ok: &AtomicBool,
    ) -> Result<()> {
        let mut session = ControlSessionState::new(&self.config);
        let mut poll_tick = tokio::time::interval(self.config.poll_period);
        poll_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut state_tick = tokio::time::interval(self.config.state_publish_period);
        state_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let loop_res: Result<()> = loop {
            tokio::select! {
                _ = stop_rx.changed() => {
                    if *stop_rx.borrow() {
                        break Ok(());
                    }
                }
                _ = poll_tick.tick() => {
                    if !imu_ok.load(Ordering::Relaxed) || !joint_ok.load(Ordering::Relaxed) {
                        tracing::debug!("telemetry stream exited; keeping session alive while control RPC is healthy");
                        imu_ok.store(true, Ordering::Relaxed);
                        joint_ok.store(true, Ordering::Relaxed);
                    }
                    if let Err(err) = session.step_cmd_vel(&self.config, self.bus.as_ref(), &client).await {
                        break Err(err);
                    }
                    if let Err(err) = handle_posture_request(&self.config, self.bus.as_ref(), &client, &mut session).await {
                        break Err(err);
                    }
                    if let Err(err) = handle_profile_request(self.bus.as_ref(), &client).await {
                        break Err(err);
                    }
                }
                _ = state_tick.tick() => {
                    match client.control_status().await {
                        Ok(status) => {
                            let msg = map_control_state(&status, session.zero_output_active());
                            let _ = self.bus.write_control_state(&msg);
                        }
                        Err(err) => break Err(err.into()),
                    }
                    if let Ok(profile) = client.profile().await {
                        let _ = self.bus.write_profile_state(&ProfileStateMsg {
                            stamp: now_wall_stamp(),
                            task_id: String::new(),
                            requirement: String::new(),
                            phase: "active".to_string(),
                            requested_profile: String::new(),
                            active_profile: profile.current,
                            reason: String::new(),
                        });
                    }
                }
            }
        };

        session.shutdown_release(&client).await;
        loop_res
    }
}

struct ControlSessionState {
    freshness: CrossBoardFreshnessGate,
    lease_token: Option<String>,
    lease_renew_at: Option<Instant>,
    last_accepted_cmd_at: Option<Instant>,
    last_walk_sent_at: Option<Instant>,
    pending_cmd: Option<(f64, f64, f64)>,
    next_seq: u64,
    zero_output: bool,
}

impl ControlSessionState {
    fn new(cfg: &BridgeConfig) -> Self {
        Self {
            freshness: CrossBoardFreshnessGate::new(
                cfg.command_timeout,
                cfg.expected_host_boot_id.clone(),
                cfg.cross_board_clock,
            ),
            lease_token: None,
            lease_renew_at: None,
            last_accepted_cmd_at: None,
            last_walk_sent_at: None,
            pending_cmd: None,
            next_seq: 1,
            zero_output: true,
        }
    }

    fn zero_output_active(&self) -> bool {
        self.zero_output && self.pending_cmd.is_none()
    }

    async fn step_cmd_vel(
        &mut self,
        cfg: &BridgeConfig,
        bus: &dyn DdsBus,
        client: &MotionBoardClient,
    ) -> Result<()> {
        let writers = bus.matched_cmd_vel_writers()?;
        if let Some((cmd, _count)) = bus.take_cmd_vel()? {
            if !cmd.vx.is_finite() || !cmd.vy.is_finite() || !cmd.wz.is_finite() {
                tracing::warn!("ignoring non-finite rt/nav/cmd_vel sample");
            } else {
                let f_sample = CmdVelFreshnessSample {
                    host_boot_id: cmd.host_boot_id,
                    producer_boot_id: cmd.producer_boot_id,
                    source_boottime_ns: cmd.source_boottime_ns,
                    receive_boottime_ns: current_boottime_ns(),
                    received_at: Instant::now(),
                    vx: cmd.vx,
                    vy: cmd.vy,
                    wz: cmd.wz,
                };
                match self.freshness.evaluate(&f_sample) {
                    FreshnessDecision::Accepted => {
                        let vx = cmd.vx.clamp(-cfg.max_linear_mps, cfg.max_linear_mps);
                        let vy = cmd.vy.clamp(-cfg.max_linear_mps, cfg.max_linear_mps);
                        let wz = cmd.wz.clamp(-cfg.max_angular_rps, cfg.max_angular_rps);
                        self.last_accepted_cmd_at = Some(Instant::now());
                        self.pending_cmd = Some((vx, vy, wz));
                    }
                    decision => {
                        tracing::warn!(?decision, "rejected stale/invalid rt/nav/cmd_vel");
                    }
                }
            }
        }

        let timed_out = self
            .last_accepted_cmd_at
            .is_some_and(|t| t.elapsed() >= cfg.command_timeout);
        if self.lease_token.is_some()
            && (timed_out || (writers == 0 && self.zero_output && self.pending_cmd.is_none()))
        {
            self.send_zero_and_release(client).await;
            return Ok(());
        }

        if self.pending_cmd.is_some() {
            self.flush_pending_walk(cfg, client).await?;
        } else if let Some(renew_at) = self.lease_renew_at {
            if Instant::now() >= renew_at {
                if let Some(token) = &self.lease_token {
                    match client.renew_control_lease(token.clone()).await {
                        Ok(lease) if lease.accepted => {
                            let half_ms = (lease.lease_duration_ms / 2).max(100) as u64;
                            self.lease_renew_at =
                                Some(Instant::now() + Duration::from_millis(half_ms));
                        }
                        _ => {
                            self.clear_lease();
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn flush_pending_walk(
        &mut self,
        cfg: &BridgeConfig,
        client: &MotionBoardClient,
    ) -> Result<()> {
        if self.lease_token.is_none() && !self.try_acquire_lease(cfg, client).await? {
            self.pending_cmd = None;
            return Ok(());
        }

        if let Some(renew_at) = self.lease_renew_at {
            if Instant::now() >= renew_at {
                if let Some(token) = &self.lease_token {
                    match client.renew_control_lease(token.clone()).await {
                        Ok(lease) if lease.accepted => {
                            let half_ms = (lease.lease_duration_ms / 2).max(100) as u64;
                            self.lease_renew_at =
                                Some(Instant::now() + Duration::from_millis(half_ms));
                        }
                        _ => {
                            self.clear_lease();
                            if !self.try_acquire_lease(cfg, client).await? {
                                self.pending_cmd = None;
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }

        if self
            .last_walk_sent_at
            .is_some_and(|t| t.elapsed() < MIN_WALK_INTERVAL)
        {
            // Keep pending_cmd queued until Brainstem's 18ms WalkChecked rate window opens.
            return Ok(());
        }

        let Some((vx, vy, wz)) = self.pending_cmd.take() else {
            return Ok(());
        };
        let Some(token) = self.lease_token.clone() else {
            return Ok(());
        };
        let ack = client
            .walk_checked(
                token,
                self.next_seq,
                rpc::Vector3 {
                    x: vx,
                    y: vy,
                    z: wz,
                },
            )
            .await?;
        self.last_walk_sent_at = Some(Instant::now());

        if ack.accepted {
            self.next_seq = self.next_seq.saturating_add(1);
            self.zero_output = vx.abs() < 1e-6 && vy.abs() < 1e-6 && wz.abs() < 1e-6;
        } else {
            let expired = matches!(
                rpc::CommandRejectReason::try_from(ack.reason),
                Ok(rpc::CommandRejectReason::LeaseExpired)
                    | Ok(rpc::CommandRejectReason::LeaseMissing)
                    | Ok(rpc::CommandRejectReason::Preempted)
                    | Ok(rpc::CommandRejectReason::RateLimited)
            );
            if expired {
                self.clear_lease();
            }
            self.zero_output = true;
        }
        Ok(())
    }

    async fn try_acquire_lease(
        &mut self,
        cfg: &BridgeConfig,
        client: &MotionBoardClient,
    ) -> Result<bool> {
        let lease = client.acquire_control(cfg.client_id.clone()).await?;
        if !lease.accepted {
            self.clear_lease();
            return Ok(false);
        }
        let token = lease.token;
        let half_ms = (lease.lease_duration_ms / 2).max(100) as u64;
        // Real Brainstem UnifiedCmsServer requires an initial zero-velocity WalkChecked (seq=1)
        // on every newly acquired lease before accepting non-zero motion.
        let zero_ack = client
            .walk_checked(
                token.clone(),
                1,
                rpc::Vector3 {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
            )
            .await?;
        self.last_walk_sent_at = Some(Instant::now());
        if !zero_ack.accepted {
            let _ = client.release_control(token).await;
            self.clear_lease();
            return Ok(false);
        }
        self.lease_token = Some(token);
        self.lease_renew_at = Some(Instant::now() + Duration::from_millis(half_ms));
        self.next_seq = 2;
        self.zero_output = true;
        Ok(true)
    }

    fn clear_lease(&mut self) {
        self.lease_token = None;
        self.lease_renew_at = None;
        self.last_walk_sent_at = None;
        self.next_seq = 1;
    }

    async fn send_zero_and_release(&mut self, client: &MotionBoardClient) {
        self.pending_cmd = None;
        if let Some(token) = self.lease_token.take() {
            if !self.zero_output {
                if let Some(prev) = self.last_walk_sent_at {
                    let elapsed = prev.elapsed();
                    if elapsed < MIN_WALK_INTERVAL {
                        tokio::time::sleep(MIN_WALK_INTERVAL - elapsed).await;
                    }
                }
                let _ = client
                    .walk_checked(
                        token.clone(),
                        self.next_seq,
                        rpc::Vector3 {
                            x: 0.0,
                            y: 0.0,
                            z: 0.0,
                        },
                    )
                    .await;
            }
            let _ = client.release_control(token).await;
        }
        self.clear_lease();
        self.last_accepted_cmd_at = None;
        self.zero_output = true;
    }

    async fn shutdown_release(&mut self, client: &MotionBoardClient) {
        self.send_zero_and_release(client).await;
    }
}

async fn handle_posture_request(
    cfg: &BridgeConfig,
    bus: &dyn DdsBus,
    client: &MotionBoardClient,
    session: &mut ControlSessionState,
) -> Result<()> {
    let Some(req) = bus.take_posture_request()? else {
        return Ok(());
    };

    let valid_action = matches!(
        req.action,
        POSTURE_ACTION_STAND_UP
            | POSTURE_ACTION_SIT_DOWN
            | POSTURE_ACTION_ENABLE_MOTORS
            | POSTURE_ACTION_DISABLE_MOTORS
            | POSTURE_ACTION_SWITCH_PROFILE
            | POSTURE_ACTION_DAMP
    );

    let (final_result, accepted, reason) = if !valid_action {
        (
            true,
            false,
            format!("unsupported posture action {}", req.action),
        )
    } else if req.phase == POSTURE_PHASE_INSPECT {
        match client.control_status().await {
            Ok(status)
                if status.critical_motor_fault && req.action != POSTURE_ACTION_DISABLE_MOTORS =>
            {
                (false, false, "critical motor fault active".to_string())
            }
            Ok(_)
                if req.action == POSTURE_ACTION_SWITCH_PROFILE
                    && req.profile_name.trim().is_empty() =>
            {
                (false, false, "profile_name must not be empty".to_string())
            }
            Ok(_) => (false, true, String::new()),
            Err(err) => (true, false, err.to_string()),
        }
    } else if req.phase == POSTURE_PHASE_CANCEL {
        session.send_zero_and_release(client).await;
        (true, true, "cancelled".to_string())
    } else if req.phase == POSTURE_PHASE_EXECUTE {
        let rpc_res: Result<()> = match req.action {
            POSTURE_ACTION_STAND_UP => client.stand_up().await.map_err(anyhow::Error::from),
            POSTURE_ACTION_SIT_DOWN => client.sit_down().await.map_err(anyhow::Error::from),
            POSTURE_ACTION_ENABLE_MOTORS => {
                client.enable_motors().await.map_err(anyhow::Error::from)
            }
            POSTURE_ACTION_DISABLE_MOTORS => {
                client.disable_motors().await.map_err(anyhow::Error::from)
            }
            POSTURE_ACTION_SWITCH_PROFILE => client
                .switch_profile(req.profile_name.clone())
                .await
                .map(|_| ())
                .map_err(anyhow::Error::from),
            POSTURE_ACTION_DAMP => client.damp().await.map_err(anyhow::Error::from),
            _ => unreachable!(),
        };
        match rpc_res {
            Ok(()) => (true, true, String::new()),
            Err(err) => (true, false, err.to_string()),
        }
    } else {
        (
            true,
            false,
            format!("unsupported posture phase {}", req.phase),
        )
    };

    bus.write_posture_ack(&PostureAckMsg {
        stamp: now_wall_stamp(),
        producer_boot_id: cfg.producer_boot_id.clone(),
        client_id: req.client_id,
        request_id: req.request_id,
        action: req.action,
        phase: req.phase,
        profile_name: req.profile_name,
        external_session_id: req.external_session_id,
        final_result,
        accepted,
        reason,
    })
}

async fn handle_profile_request(bus: &dyn DdsBus, client: &MotionBoardClient) -> Result<()> {
    let Some(req) = bus.take_profile_request()? else {
        return Ok(());
    };
    let msg = match client.switch_profile(req.requirement.clone()).await {
        Ok(info) => ProfileStateMsg {
            stamp: now_wall_stamp(),
            task_id: req.task_id,
            requirement: req.requirement.clone(),
            phase: "succeeded".to_string(),
            requested_profile: req.requirement,
            active_profile: info.current,
            reason: String::new(),
        },
        Err(err) => ProfileStateMsg {
            stamp: now_wall_stamp(),
            task_id: req.task_id,
            requirement: req.requirement.clone(),
            phase: "failed".to_string(),
            requested_profile: req.requirement,
            active_profile: String::new(),
            reason: err.to_string(),
        },
    };
    bus.write_profile_state(&msg)
}

fn spawn_imu_forwarder(
    client: Arc<MotionBoardClient>,
    bus: Arc<dyn DdsBus>,
    healthy: Arc<AtomicBool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        match client.listen_imu().await {
            Ok(mut stream) => {
                while let Ok(Some(imu)) = stream.message().await {
                    let sample = map_imu_sample(&imu);
                    if let Err(err) = bus.write_imu_body(&sample) {
                        tracing::warn!(error = %err, "failed writing rt/imu/body");
                    }
                }
            }
            Err(err) => {
                tracing::debug!(error = %err, "listen_imu stopped");
            }
        }
        healthy.store(false, Ordering::Relaxed);
    })
}

fn spawn_joint_forwarder(
    client: Arc<MotionBoardClient>,
    bus: Arc<dyn DdsBus>,
    robot_model: String,
    healthy: Arc<AtomicBool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut assembler = JointAssembler::default();
        match client.listen_joint().await {
            Ok(mut stream) => {
                while let Ok(Some(joint)) = stream.message().await {
                    if let Some(sample) = assembler.apply(&joint, &robot_model) {
                        if let Err(err) = bus.write_joint_states(&sample) {
                            tracing::warn!(error = %err, "failed writing rt/robot/joint_states");
                        }
                    }
                }
            }
            Err(err) => {
                tracing::debug!(error = %err, "listen_joint stopped");
            }
        }
        healthy.store(false, Ordering::Relaxed);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dds_bus::{CmdVelWireMsg, InMemoryDdsBus, PostureRequestMsg, ProfileRequestMsg};
    use std::sync::Mutex;
    use tonic::{Request, Response, Status};

    struct MockBridgeRobotControl {
        walks: Mutex<Vec<rpc::WalkRequest>>,
        postures: Mutex<Vec<&'static str>>,
        profile: Mutex<String>,
        leases_released: Mutex<Vec<String>>,
        ready_for_walk: Mutex<bool>,
    }

    impl Default for MockBridgeRobotControl {
        fn default() -> Self {
            Self {
                walks: Mutex::new(Vec::new()),
                postures: Mutex::new(Vec::new()),
                profile: Mutex::new("flat_w5".into()),
                leases_released: Mutex::new(Vec::new()),
                ready_for_walk: Mutex::new(true),
            }
        }
    }

    #[tonic::async_trait]
    impl rpc::robot_control_server::RobotControl for MockBridgeRobotControl {
        type ListenHistoryStream =
            tokio_stream::wrappers::ReceiverStream<std::result::Result<rpc::History, Status>>;
        type ListenCmsStateStream =
            tokio_stream::wrappers::ReceiverStream<std::result::Result<rpc::CmsState, Status>>;
        type ListenImuStream =
            tokio_stream::wrappers::ReceiverStream<std::result::Result<rpc::Imu, Status>>;
        type ListenJointStream =
            tokio_stream::wrappers::ReceiverStream<std::result::Result<rpc::Joint, Status>>;

        async fn walk(
            &self,
            _: Request<rpc::Vector3>,
        ) -> std::result::Result<Response<()>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn walk_checked(
            &self,
            request: Request<rpc::WalkRequest>,
        ) -> std::result::Result<Response<rpc::CommandAck>, Status> {
            let inner = request.into_inner();
            let seq = inner.sequence;
            self.walks.lock().unwrap().push(inner);
            Ok(Response::new(rpc::CommandAck {
                accepted: true,
                reason: rpc::CommandRejectReason::None as i32,
                sequence: seq,
                status: None,
            }))
        }

        async fn enable(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
            self.postures.lock().unwrap().push("enable_motors");
            Ok(Response::new(()))
        }

        async fn disable(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
            self.postures.lock().unwrap().push("disable_motors");
            Ok(Response::new(()))
        }

        async fn acquire_control(
            &self,
            request: Request<rpc::AcquireControlRequest>,
        ) -> std::result::Result<Response<rpc::ControlLease>, Status> {
            let client_id = request.into_inner().client_id;
            if client_id != DEFAULT_LOOPBACK_CLIENT_ID {
                return Err(Status::permission_denied(format!(
                    "AcquireControl client_id '{client_id}' must match loopback principal '{DEFAULT_LOOPBACK_CLIENT_ID}'"
                )));
            }
            Ok(Response::new(rpc::ControlLease {
                accepted: true,
                token: "lease-tok-1".into(),
                lease_duration_ms: 500,
                reason: rpc::CommandRejectReason::None as i32,
                status: None,
            }))
        }

        async fn renew_control_lease(
            &self,
            request: Request<rpc::ControlLeaseRequest>,
        ) -> std::result::Result<Response<rpc::ControlLease>, Status> {
            Ok(Response::new(rpc::ControlLease {
                accepted: true,
                token: request.into_inner().token,
                lease_duration_ms: 500,
                reason: rpc::CommandRejectReason::None as i32,
                status: None,
            }))
        }

        async fn release_control(
            &self,
            request: Request<rpc::ControlLeaseRequest>,
        ) -> std::result::Result<Response<rpc::ControlStatus>, Status> {
            self.leases_released
                .lock()
                .unwrap()
                .push(request.into_inner().token);
            Ok(Response::new(self.current_status()))
        }

        async fn stand_up(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
            self.postures.lock().unwrap().push("stand_up");
            Ok(Response::new(()))
        }

        async fn sit_down(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
            self.postures.lock().unwrap().push("sit_down");
            Ok(Response::new(()))
        }

        async fn damp(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
            self.postures.lock().unwrap().push("damp");
            Ok(Response::new(()))
        }

        async fn set_speed_mode(
            &self,
            _: Request<rpc::SpeedModeRequest>,
        ) -> std::result::Result<Response<()>, Status> {
            Ok(Response::new(()))
        }

        async fn get_speed_mode(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::SpeedModeRequest>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn tick(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::History>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn step(
            &self,
            _: Request<rpc::SimState>,
        ) -> std::result::Result<Response<()>, Status> {
            Ok(Response::new(()))
        }

        async fn get_start_time(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<prost_types::Timestamp>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn get_cms_state(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::CmsState>, Status> {
            Ok(Response::new(rpc::CmsState {
                kind: rpc::CmsStateKind::Standing as i32,
                transition: rpc::CmsTransitionKind::None as i32,
                gesture_name: String::new(),
            }))
        }

        async fn get_control_status(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::ControlStatus>, Status> {
            Ok(Response::new(self.current_status()))
        }

        async fn listen_history(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<Self::ListenHistoryStream>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn listen_cms_state(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<Self::ListenCmsStateStream>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn listen_imu(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<Self::ListenImuStream>, Status> {
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tx.send(Ok(rpc::Imu {
                gyroscope: Some(rpc::Vector3 {
                    x: 0.1,
                    y: -0.2,
                    z: 0.3,
                }),
                quaternion: Some(rpc::Quaternion {
                    w: 0.995,
                    x: 0.01,
                    y: -0.02,
                    z: 0.09,
                }),
                linear_acceleration: Some(rpc::Vector3 {
                    x: 0.0,
                    y: 0.0,
                    z: 9.81,
                }),
                timestamp: Some(prost_types::Duration {
                    seconds: 1,
                    nanos: 0,
                }),
            }))
            .await
            .unwrap();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                drop(tx);
            });
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
                rx,
            )))
        }

        async fn listen_joint(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<Self::ListenJointStream>, Status> {
            let (tx, rx) = tokio::sync::mpsc::channel(32);
            let values: Vec<f64> = (0..16).map(|i| i as f64 * 0.1).collect();
            tx.send(Ok(rpc::Joint {
                timestamp: None,
                data: Some(rpc::joint::Data::AllJoints(rpc::AllJoints {
                    position: Some(rpc::Matrix4 {
                        values: values.clone(),
                    }),
                    velocity: Some(rpc::Matrix4 {
                        values: vec![0.5; 16],
                    }),
                    torque: Some(rpc::Matrix4 {
                        values: vec![1.0; 16],
                    }),
                    status: None,
                })),
            }))
            .await
            .unwrap();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                drop(tx);
            });
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
                rx,
            )))
        }

        async fn get_params(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::Params>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn switch_profile(
            &self,
            request: Request<rpc::ProfileRequest>,
        ) -> std::result::Result<Response<rpc::ProfileInfo>, Status> {
            let target = request.into_inner().name;
            *self.profile.lock().unwrap() = target.clone();
            Ok(Response::new(rpc::ProfileInfo {
                current: target,
                available: vec!["flat_w5".into(), "cts_v39".into()],
                descriptions: vec!["Flat".into(), "Stairs".into()],
                current_description: "Active".into(),
                mode_capabilities: Vec::new(),
                active_mode_id: "stairs".into(),
            }))
        }

        async fn get_profile(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::ProfileInfo>, Status> {
            let cur = self.profile.lock().unwrap().clone();
            Ok(Response::new(rpc::ProfileInfo {
                current: cur,
                available: vec!["flat_w5".into(), "cts_v39".into()],
                descriptions: vec!["Flat".into(), "Stairs".into()],
                current_description: "Flat".into(),
                mode_capabilities: Vec::new(),
                active_mode_id: "flat".into(),
            }))
        }

        async fn play_gesture(
            &self,
            _: Request<rpc::GestureRequest>,
        ) -> std::result::Result<Response<()>, Status> {
            Ok(Response::new(()))
        }

        async fn list_gestures(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::GestureList>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn get_voltage(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::Voltage>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn get_motor_status(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::MotorStatusResponse>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn clear_motor_fault(
            &self,
            _: Request<rpc::ClearFaultRequest>,
        ) -> std::result::Result<Response<()>, Status> {
            Ok(Response::new(()))
        }

        async fn set_zero(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
            Ok(Response::new(()))
        }
    }

    impl MockBridgeRobotControl {
        fn current_status(&self) -> rpc::ControlStatus {
            rpc::ControlStatus {
                fsm: Some(rpc::CmsState {
                    kind: rpc::CmsStateKind::Standing as i32,
                    transition: rpc::CmsTransitionKind::None as i32,
                    gesture_name: String::new(),
                }),
                motor_output_enabled: true,
                owner: rpc::ControlOwner::Grpc as i32,
                critical_motor_fault: false,
                grpc_lease_active: true,
                lease_remaining_ms: 420,
                last_accepted_sequence: self.walks.lock().unwrap().len() as u64,
                last_decision: rpc::CommandRejectReason::None as i32,
                ready_for_walk: *self.ready_for_walk.lock().unwrap(),
                owner_id: "brainstem-dds-bridge@board-a".into(),
                accepted_count: 1,
                rejected_count: 0,
            }
        }
    }

    async fn spawn_mock_server() -> (
        String,
        Arc<MockBridgeRobotControl>,
        oneshot::Sender<()>,
        JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let svc = Arc::new(MockBridgeRobotControl::default());
        let (tx, rx) = oneshot::channel();
        let server = tonic::transport::Server::builder()
            .add_service(rpc::robot_control_server::RobotControlServer::from_arc(
                svc.clone(),
            ))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async move {
                    let _ = rx.await;
                },
            );
        let handle = tokio::spawn(async move {
            let _ = server.await;
        });
        (format!("http://127.0.0.1:{port}"), svc, tx, handle)
    }

    #[tokio::test]
    async fn bridge_forwards_telemetry_posture_profile_and_watchdog_with_in_memory_bus() {
        let (endpoint, svc, srv_stop, srv_task) = spawn_mock_server().await;
        let bus = Arc::new(InMemoryDdsBus::default());
        *bus.cmd_vel_writers.lock().unwrap() = 1;

        let cfg = BridgeConfig {
            brainstem_endpoint: endpoint,
            poll_period: Duration::from_millis(5),
            state_publish_period: Duration::from_millis(15),
            command_timeout: Duration::from_millis(80),
            ..BridgeConfig::default()
        };

        let (stop_tx, stop_rx) = oneshot::channel();
        let bridge_bus = bus.clone();
        let bridge_task = tokio::spawn(async move {
            BridgeRuntime::new(cfg, bridge_bus)
                .run_until(stop_rx)
                .await
                .unwrap();
        });

        // Push a cross-board cmd_vel, two-phase standup, and profile request.
        bus.cmd_vel_rx.lock().unwrap().push_back(CmdVelWireMsg {
            host_boot_id: "board-b-boot".into(),
            producer_boot_id: "nav-b".into(),
            output_seq: 1,
            source_boottime_ns: 123_000_000,
            source_wall_ns: 1_000,
            vx: 0.45,
            vy: -0.1,
            wz: 0.2,
        });
        bus.posture_req_rx
            .lock()
            .unwrap()
            .push_back(PostureRequestMsg {
                host_boot_id: "board-b-boot".into(),
                producer_boot_id: "nav-b".into(),
                source_boottime_ns: 124_000_000,
                client_id: "gateway".into(),
                request_id: "req-stand-0".into(),
                action: POSTURE_ACTION_STAND_UP,
                phase: POSTURE_PHASE_INSPECT,
                profile_name: String::new(),
                external_session_id: String::new(),
            });
        bus.profile_req_rx
            .lock()
            .unwrap()
            .push_back(ProfileRequestMsg {
                task_id: "task-stairs".into(),
                requirement: "cts_v39".into(),
            });

        tokio::time::sleep(Duration::from_millis(60)).await;

        // Send execute phase after inspect succeeded.
        bus.posture_req_rx
            .lock()
            .unwrap()
            .push_back(PostureRequestMsg {
                host_boot_id: "board-b-boot".into(),
                producer_boot_id: "nav-b".into(),
                source_boottime_ns: 180_000_000,
                client_id: "gateway".into(),
                request_id: "req-stand-1".into(),
                action: POSTURE_ACTION_STAND_UP,
                phase: POSTURE_PHASE_EXECUTE,
                profile_name: String::new(),
                external_session_id: String::new(),
            });

        // Wait past watchdog timeout (80ms) to confirm zero-velocity + lease release.
        tokio::time::sleep(Duration::from_millis(140)).await;

        let imu_samples = bus.imu_tx.lock().unwrap().clone();
        assert!(!imu_samples.is_empty());
        assert!((imu_samples[0].orientation_xyzw[3] - 0.995).abs() < 1e-6);

        let joint_samples = bus.joints_tx.lock().unwrap().clone();
        assert!(!joint_samples.is_empty());
        assert!((joint_samples[0].position[15] - 1.5).abs() < 1e-6);

        let ctrl_states = bus.control_tx.lock().unwrap().clone();
        assert!(!ctrl_states.is_empty());
        assert!(ctrl_states
            .iter()
            .any(|s| s.connected && s.fsm == "Standing"));

        let posture_acks = bus.posture_ack_tx.lock().unwrap().clone();
        assert!(posture_acks
            .iter()
            .any(|a| a.request_id == "req-stand-0" && a.accepted && !a.final_result));
        assert!(posture_acks
            .iter()
            .any(|a| a.request_id == "req-stand-1" && a.accepted && a.final_result));
        assert!(svc.postures.lock().unwrap().contains(&"stand_up"));

        let prof_states = bus.profile_tx.lock().unwrap().clone();
        assert!(prof_states
            .iter()
            .any(|p| p.phase == "succeeded" && p.active_profile == "cts_v39"));

        let walks = svc.walks.lock().unwrap().clone();
        assert!(
            walks.len() >= 3,
            "expected initial zero ack + cmd_vel walk + watchdog zero walk, got {}",
            walks.len()
        );
        assert_eq!(walks[0].sequence, 1);
        let zero_ack_dir = walks[0].direction.unwrap();
        assert!(
            zero_ack_dir.x.abs() < 1e-6
                && zero_ack_dir.y.abs() < 1e-6
                && zero_ack_dir.z.abs() < 1e-6
        );
        assert_eq!(walks[1].sequence, 2);
        let cmd_dir = walks[1].direction.unwrap();
        assert!((cmd_dir.x - 0.45).abs() < 1e-6);
        let last_dir = walks.last().unwrap().direction.unwrap();
        assert!(last_dir.x.abs() < 1e-6 && last_dir.y.abs() < 1e-6 && last_dir.z.abs() < 1e-6);
        assert!(!svc.leases_released.lock().unwrap().is_empty());

        let _ = stop_tx.send(());
        bridge_task.await.unwrap();
        let _ = srv_stop.send(());
        srv_task.await.unwrap();
    }

    #[cfg(has_cyclonedds)]
    #[tokio::test]
    async fn cyclonedds_loopback_end_to_end_roundtrip() {
        use crate::dds_bus::cyclone::{CycloneDdsBus, CycloneTestPeer};

        if std::env::var_os("CYCLONEDDS_URI").is_none() {
            std::env::set_var(
                "CYCLONEDDS_URI",
                "<CycloneDDS><Domain><General><Interfaces><NetworkInterface address=\"127.0.0.1\"/></Interfaces><AllowMulticast>false</AllowMulticast></General><Discovery><Peers><Peer address=\"127.0.0.1\"/></Peers></Discovery></Domain></CycloneDDS>",
            );
        }

        let domain_id = 73;
        let bus = Arc::new(CycloneDdsBus::new(domain_id).unwrap());
        let peer = CycloneTestPeer::new(domain_id).unwrap();

        let (endpoint, svc, srv_stop, srv_task) = spawn_mock_server().await;
        let cfg = BridgeConfig {
            brainstem_endpoint: endpoint,
            poll_period: Duration::from_millis(10),
            state_publish_period: Duration::from_millis(20),
            command_timeout: Duration::from_millis(150),
            ..BridgeConfig::default()
        };

        let (stop_tx, stop_rx) = oneshot::channel();
        let bridge_bus = bus.clone();
        let bridge_task = tokio::spawn(async move {
            BridgeRuntime::new(cfg, bridge_bus)
                .run_until(stop_rx)
                .await
                .unwrap();
        });

        let mut matched = false;
        for _ in 0..80 {
            if peer.matched_bridge_readers() > 0 {
                matched = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            matched,
            "CycloneDDS test peer failed to match bridge readers"
        );

        peer.write_cmd_vel(&CmdVelWireMsg {
            host_boot_id: "board-b".into(),
            producer_boot_id: "locomotion".into(),
            output_seq: 1,
            source_boottime_ns: 900_000_000,
            source_wall_ns: 1_000,
            vx: 0.35,
            vy: 0.0,
            wz: -0.15,
        })
        .unwrap();

        peer.write_posture_request(&PostureRequestMsg {
            host_boot_id: "board-b".into(),
            producer_boot_id: "gateway".into(),
            source_boottime_ns: 901_000_000,
            client_id: "test-peer".into(),
            request_id: "dds-stand-1".into(),
            action: POSTURE_ACTION_STAND_UP,
            phase: POSTURE_PHASE_EXECUTE,
            profile_name: String::new(),
            external_session_id: String::new(),
        })
        .unwrap();

        let mut got_imu = false;
        let mut got_joints = false;
        let mut got_ctrl = false;
        let mut got_ack = false;
        for _ in 0..80 {
            if let Some(imu) = peer.take_imu_body() {
                if (imu.orientation_xyzw[3] - 0.995).abs() < 1e-6 {
                    got_imu = true;
                }
            }
            if let Some(js) = peer.take_joint_states() {
                if (js.position[15] - 1.5).abs() < 1e-6 {
                    got_joints = true;
                }
            }
            if let Some(st) = peer.take_control_state() {
                if st.connected && st.fsm == "Standing" {
                    got_ctrl = true;
                }
            }
            if let Some(ack) = peer.take_posture_ack() {
                if ack.request_id == "dds-stand-1" && ack.accepted && ack.final_result {
                    got_ack = true;
                }
            }
            if got_imu && got_joints && got_ctrl && got_ack && svc.walks.lock().unwrap().len() >= 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        assert!(got_imu, "expected rt/imu/body sample over CycloneDDS");
        assert!(
            got_joints,
            "expected rt/robot/joint_states sample over CycloneDDS"
        );
        assert!(
            got_ctrl,
            "expected rt/driver/control_state sample over CycloneDDS"
        );
        assert!(
            got_ack,
            "expected rt/driver/posture/ack sample over CycloneDDS"
        );
        let walks = svc.walks.lock().unwrap().clone();
        assert!(
            walks.len() >= 2,
            "expected initial zero ack + cmd_vel forwarded to Brainstem"
        );
        assert_eq!(walks[0].sequence, 1);
        assert_eq!(walks[1].sequence, 2);
        assert!((walks[1].direction.unwrap().x - 0.35).abs() < 1e-6);

        let _ = stop_tx.send(());
        bridge_task.await.unwrap();
        let _ = srv_stop.send(());
        srv_task.await.unwrap();
    }
}
