use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use doso_sdk::internal::motion_board::rpc as mb;
use doso_sdk::internal::motion_board::{MotionBoardClient, MotionBoardConfig};
use doso_sdk::rpc as doso;
use doso_shm_ipc::{
    DosoExternalCmdPayload, DosoProprioPayload, ExternalCmdWriter, ProprioRingReader,
};
use prost_types::{value::Kind, ListValue, Struct, Value};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_stream::{wrappers::TcpListenerStream, Stream};
use tonic::{Request, Response, Status};

const BOARD_B_MISSING_MSG: &str =
    "Board B autonomy (LingTu) is not installed on standalone motion chassis";

type BoxStream<T> = Pin<Box<dyn Stream<Item = std::result::Result<T, Status>> + Send>>;

#[derive(Debug, Clone)]
pub struct BaseGatewayConfig {
    pub brainstem_endpoint: String,
    pub brainstem_client_id: String,
    pub listen_addr: String,
    pub robot_id: Option<String>,
    pub robot_model: String,
    pub firmware_version: String,
    pub api_key: Option<String>,
    pub low_level_endpoint_override: Option<String>,
    pub teleop_watchdog: Duration,
    pub shm_enabled: bool,
    pub shm_proprio_name: String,
    pub shm_cmd_name: String,
    pub shm_poll_period: Duration,
}

impl Default for BaseGatewayConfig {
    fn default() -> Self {
        Self {
            brainstem_endpoint: doso_sdk::internal::motion_board::DEFAULT_ENDPOINT.to_string(),
            brainstem_client_id: crate::bridge::DEFAULT_LOOPBACK_CLIENT_ID.to_string(),
            listen_addr: "0.0.0.0:5052".to_string(),
            robot_id: None,
            robot_model: "thunder".to_string(),
            firmware_version: "0.1.0".to_string(),
            api_key: None,
            low_level_endpoint_override: None,
            teleop_watchdog: Duration::from_millis(250),
            shm_enabled: false,
            shm_proprio_name: "/doso_proprio_v1".to_string(),
            shm_cmd_name: "/doso_ext_cmd_v1".to_string(),
            shm_poll_period: Duration::from_millis(5),
        }
    }
}

#[derive(Clone)]
pub struct BaseGatewayService {
    config: Arc<BaseGatewayConfig>,
    brainstem: Arc<Mutex<Option<Arc<MotionBoardClient>>>>,
}

impl BaseGatewayService {
    pub fn new(config: BaseGatewayConfig) -> Self {
        Self {
            config: Arc::new(config),
            brainstem: Arc::new(Mutex::new(None)),
        }
    }

    fn check_auth<T>(&self, request: &Request<T>) -> std::result::Result<(), Status> {
        let Some(expected) = &self.config.api_key else {
            return Ok(());
        };
        let actual = request
            .metadata()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok());
        if actual == Some(expected.as_str()) {
            Ok(())
        } else {
            Err(Status::unauthenticated("invalid or missing x-api-key"))
        }
    }

    async fn client(&self) -> std::result::Result<Arc<MotionBoardClient>, Status> {
        let mut guard = self.brainstem.lock().await;
        if let Some(existing) = &*guard {
            return Ok(existing.clone());
        }
        let cfg = MotionBoardConfig::new(self.config.brainstem_endpoint.clone());
        let client = MotionBoardClient::with_config(cfg)
            .await
            .map_err(|err| Status::unavailable(format!("Brainstem unavailable: {err}")))?;
        let arc = Arc::new(client);
        *guard = Some(arc.clone());
        Ok(arc)
    }

    async fn drop_cached_client_if_transport(&self, err: &doso_sdk::Error) {
        if err.kind == doso_sdk::ErrorKind::Transport {
            let mut guard = self.brainstem.lock().await;
            *guard = None;
        }
    }

    fn require_command(cmd: &doso::Command) -> std::result::Result<&str, Status> {
        let id = cmd.request_id.trim();
        if id.is_empty() {
            return Err(Status::invalid_argument("request_id must not be empty"));
        }
        Ok(&cmd.request_id)
    }

    fn receipt(request_id: impl Into<String>, stage: impl Into<String>) -> doso::Receipt {
        doso::Receipt {
            ok: true,
            accepted: Some(true),
            request_id: request_id.into(),
            stage: stage.into(),
            ..Default::default()
        }
    }
}

pub async fn spawn_base_gateway_on_listener(
    service: BaseGatewayService,
    listener: TcpListener,
    shutdown: oneshot::Receiver<()>,
) -> Result<(SocketAddr, tokio::task::JoinHandle<Result<()>>)> {
    let local_addr = listener.local_addr()?;
    let router = tonic::transport::Server::builder()
        .tcp_nodelay(true)
        .add_service(doso::system_server::SystemServer::new(service.clone()))
        .add_service(doso::motion_server::MotionServer::new(service.clone()))
        .add_service(doso::observations_server::ObservationsServer::new(
            service.clone(),
        ))
        .add_service(doso::localization_server::LocalizationServer::new(
            service.clone(),
        ))
        .add_service(doso::maps_server::MapsServer::new(service.clone()))
        .add_service(doso::navigation_server::NavigationServer::new(
            service.clone(),
        ))
        .add_service(doso::tasks_server::TasksServer::new(service));

    let handle = tokio::spawn(async move {
        router
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async move {
                let _ = shutdown.await;
            })
            .await?;
        Ok(())
    });
    Ok((local_addr, handle))
}

pub async fn serve_base_gateway(
    config: BaseGatewayConfig,
    shutdown: oneshot::Receiver<()>,
) -> Result<()> {
    let listener = TcpListener::bind(&config.listen_addr).await?;
    let service = BaseGatewayService::new(config);
    let (addr, handle) = spawn_base_gateway_on_listener(service, listener, shutdown).await?;
    tracing::info!(%addr, "Board-A doso.v1 Base Gateway listening");
    handle.await?
}

fn unimplemented_board_b() -> Status {
    Status::unimplemented(BOARD_B_MISSING_MSG)
}

fn map_sdk_err(err: doso_sdk::Error) -> Status {
    match err.kind {
        doso_sdk::ErrorKind::InvalidInput => Status::invalid_argument(err.message),
        doso_sdk::ErrorKind::Timeout => Status::deadline_exceeded(err.message),
        doso_sdk::ErrorKind::Transport | doso_sdk::ErrorKind::Protocol => {
            Status::unavailable(err.message)
        }
        doso_sdk::ErrorKind::OperationFailed => Status::failed_precondition(err.message),
        doso_sdk::ErrorKind::Rpc(code) => Status::new(code, err.message),
    }
}

fn sval(k: Kind) -> Value {
    Value { kind: Some(k) }
}

fn snapshot(pairs: Vec<(&str, Value)>) -> doso::Snapshot {
    let mut fields = BTreeMap::new();
    for (k, v) in pairs {
        fields.insert(k.to_string(), v);
    }
    doso::Snapshot {
        fields: Some(Struct { fields }),
    }
}

fn cms_kind_label(kind: i32) -> &'static str {
    match mb::CmsStateKind::try_from(kind) {
        Ok(mb::CmsStateKind::Zero) => "zero",
        Ok(mb::CmsStateKind::Grounded) => "grounded",
        Ok(mb::CmsStateKind::Standing) => "standing",
        Ok(mb::CmsStateKind::Walking) => "walking",
        Ok(mb::CmsStateKind::Transitioning) => "transitioning",
        Ok(mb::CmsStateKind::Damping) => "damping",
        Err(_) => "unknown",
    }
}

fn reject_label(reason: i32) -> String {
    match mb::CommandRejectReason::try_from(reason) {
        Ok(mb::CommandRejectReason::None) => String::new(),
        Ok(other) => other.as_str_name().to_string(),
        Err(_) => format!("unknown_reason_{reason}"),
    }
}

fn owner_label(owner: i32) -> &'static str {
    match mb::ControlOwner::try_from(owner) {
        Ok(mb::ControlOwner::None) => "none",
        Ok(mb::ControlOwner::Grpc) => "grpc",
        Ok(mb::ControlOwner::Yunzhuo) => "yunzhuo",
        Err(_) => "unknown",
    }
}

fn map_cms_to_body(cms: &mb::CmsState) -> doso::BodyState {
    doso::BodyState {
        kind: cms.kind,
        transition: cms.transition,
        gesture_name: cms.gesture_name.clone(),
    }
}

fn map_status_to_control_state(st: &mb::ControlStatus, held_by_client: bool) -> doso::ControlState {
    let owner = if st.owner_id.is_empty() {
        owner_label(st.owner).to_string()
    } else {
        st.owner_id.clone()
    };
    doso::ControlState {
        available: Some(!st.critical_motor_fault),
        held_by_client: Some(held_by_client),
        owner,
        time: None,
        owner_epoch: 0,
        holding: Some(!st.grpc_lease_active),
        reason: reject_label(st.last_decision),
        received_age_ms: Some(0.0),
        fresh: true,
    }
}

fn map_status_to_body_control(st: &mb::ControlStatus) -> doso::BodyControlStatus {
    doso::BodyControlStatus {
        body: st.fsm.as_ref().map(map_cms_to_body),
        motor_output_enabled: st.motor_output_enabled,
        source_owner: st.owner,
        critical_motor_fault: st.critical_motor_fault,
        grpc_lease_active: st.grpc_lease_active,
        lease_remaining_ms: st.lease_remaining_ms,
        last_accepted_sequence: st.last_accepted_sequence,
        source_last_decision: st.last_decision,
        source_ready_for_walk: st.ready_for_walk,
        source_owner_id: st.owner_id.clone(),
        accepted_count: st.accepted_count,
        rejected_count: st.rejected_count,
    }
}

fn map_ext_status_to_controller(st: &mb::ExternalStatus) -> doso::ControllerState {
    doso::ControllerState {
        mode: if st.selected {
            doso::ControllerMode::External as i32
        } else {
            doso::ControllerMode::Builtin as i32
        },
        session_id: st.session_id.clone(),
        active: st.active,
        motor_output_enabled: st.motor_output_enabled,
        last_sequence: st.last_sequence,
        reason: st.reason.clone(),
        received_age_ms: Some(0.0),
        fresh: st.active,
        motor_disable_confirmed: st.motor_disable_confirmed,
        joint_count: if st.joint_count == 0 {
            16
        } else {
            st.joint_count
        },
        remote: None,
    }
}

fn map_vec3(v: mb::Vector3) -> doso::Vector3 {
    doso::Vector3 {
        x: v.x,
        y: v.y,
        z: v.z,
    }
}

fn map_imu(imu: mb::Imu) -> doso::ImuSample {
    doso::ImuSample {
        angular_velocity_rad_s: imu.gyroscope.map(map_vec3),
        orientation_world_to_body: imu.quaternion.map(|q| doso::Quaternion {
            w: q.w,
            x: q.x,
            y: q.y,
            z: q.z,
        }),
        linear_acceleration_m_s2: imu.linear_acceleration.map(map_vec3),
        source_elapsed: imu.timestamp,
    }
}

fn shm_elapsed_duration(monotonic_us: u64) -> prost_types::Duration {
    prost_types::Duration {
        seconds: (monotonic_us / 1_000_000) as i64,
        nanos: ((monotonic_us % 1_000_000) * 1_000) as i32,
    }
}

fn map_shm_imu(p: &DosoProprioPayload) -> doso::ImuSample {
    doso::ImuSample {
        angular_velocity_rad_s: Some(doso::Vector3 {
            x: p.imu_gyro_rad_s[0],
            y: p.imu_gyro_rad_s[1],
            z: p.imu_gyro_rad_s[2],
        }),
        orientation_world_to_body: Some(doso::Quaternion {
            w: p.imu_quat_wxyz[0],
            x: p.imu_quat_wxyz[1],
            y: p.imu_quat_wxyz[2],
            z: p.imu_quat_wxyz[3],
        }),
        linear_acceleration_m_s2: Some(doso::Vector3 {
            x: p.imu_accel_m_s2[0],
            y: p.imu_accel_m_s2[1],
            z: p.imu_accel_m_s2[2],
        }),
        source_elapsed: Some(shm_elapsed_duration(p.monotonic_us)),
    }
}

fn map_shm_joint(p: &DosoProprioPayload) -> doso::JointSample {
    let mut joints = Vec::with_capacity(16);
    for i in 0..16 {
        joints.push(doso::JointState {
            id: i as u32,
            position_rad: p.joint_q[i],
            velocity_rad_s: p.joint_dq[i],
            torque_nm: p.joint_tau[i],
            status_code: u32::from(p.joint_status[i]),
        });
    }
    doso::JointSample {
        joints,
        source_elapsed: Some(shm_elapsed_duration(p.monotonic_us)),
    }
}

fn map_joint(j: mb::Joint) -> doso::JointSample {
    let mut joints = Vec::new();
    match j.data {
        Some(mb::joint::Data::SingleJoint(sj)) => {
            joints.push(doso::JointState {
                id: sj.id,
                position_rad: sj.position,
                velocity_rad_s: sj.velocity,
                torque_nm: sj.torque,
                status_code: sj.status,
            });
        }
        Some(mb::joint::Data::AllJoints(all)) => {
            let pos = all.position.map(|m| m.values).unwrap_or_default();
            let vel = all.velocity.map(|m| m.values).unwrap_or_default();
            let tau = all.torque.map(|m| m.values).unwrap_or_default();
            let st = all.status.map(|m| m.values).unwrap_or_default();
            for i in 0..16 {
                joints.push(doso::JointState {
                    id: i as u32,
                    position_rad: *pos.get(i).unwrap_or(&0.0),
                    velocity_rad_s: *vel.get(i).unwrap_or(&0.0),
                    torque_nm: *tau.get(i).unwrap_or(&0.0),
                    status_code: *st.get(i).unwrap_or(&0),
                });
            }
        }
        None => {}
    }
    doso::JointSample {
        joints,
        source_elapsed: j.timestamp,
    }
}

async fn collect_fault_report(
    mb: &MotionBoardClient,
) -> std::result::Result<doso::FaultReport, Status> {
    let status = mb.control_status().await.map_err(map_sdk_err)?;
    let mut faults = Vec::new();
    let mut health = doso::HealthStatus::Ok as i32;
    if status.critical_motor_fault {
        health = doso::HealthStatus::Error as i32;
        faults.push(doso::Fault {
            source: "brainstem".to_string(),
            code: "critical_motor_fault".to_string(),
            severity: doso::FaultSeverity::Fatal as i32,
            message: "Brainstem reported critical motor fault".to_string(),
            active: Some(true),
            time: None,
        });
    }
    if let Ok(motors) = mb.motor_status().await {
        for m in motors.motors {
            if !m.online || !m.errors.is_empty() {
                if health < doso::HealthStatus::Warning as i32 {
                    health = doso::HealthStatus::Warning as i32;
                }
                faults.push(doso::Fault {
                    source: "brainstem_motor".to_string(),
                    code: format!("motor_{}", m.id),
                    severity: doso::FaultSeverity::Error as i32,
                    message: format!("online={} errors={:?}", m.online, m.errors),
                    active: Some(true),
                    time: None,
                });
            }
        }
    }
    Ok(doso::FaultReport {
        health,
        faults,
        observed_sources: vec!["brainstem".to_string()],
        unavailable_sources: vec!["lingtu".to_string()],
    })
}

#[tonic::async_trait]
impl doso::system_server::System for BaseGatewayService {
    async fn handshake(
        &self,
        request: Request<doso::Handshake>,
    ) -> std::result::Result<Response<doso::HandshakeReply>, Status> {
        self.check_auth(&request)?;
        let req = request.into_inner();
        if req.protocol_version != 2 {
            return Err(Status::failed_precondition(format!(
                "unsupported protocol version {}, expected 2",
                req.protocol_version
            )));
        }
        Ok(Response::new(doso::HandshakeReply {
            protocol_version: 2,
            info: Some(doso::SystemInfo {
                firmware_version: Some(self.config.firmware_version.clone()),
                robot_version: Some(self.config.robot_model.clone()),
                robot_id: self.config.robot_id.clone(),
                low_level_endpoint: self.config.low_level_endpoint_override.clone(),
            }),
        }))
    }

    async fn ping(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::Ping>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        if let Err(err) = mb.control_status().await {
            self.drop_cached_client_if_transport(&err).await;
            return Err(map_sdk_err(err));
        }
        Ok(Response::new(doso::Ping {
            status: "ok".to_string(),
        }))
    }

    async fn state(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::Snapshot>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let st = mb.control_status().await.map_err(map_sdk_err)?;
        let mode = st
            .fsm
            .as_ref()
            .map(|f| cms_kind_label(f.kind))
            .unwrap_or("unknown");
        Ok(Response::new(snapshot(vec![
            (
                "deployment_mode",
                sval(Kind::StringValue("standalone_board_a".into())),
            ),
            ("mode", sval(Kind::StringValue(mode.into()))),
            (
                "motors_enabled",
                sval(Kind::BoolValue(st.motor_output_enabled)),
            ),
            ("ready_for_walk", sval(Kind::BoolValue(st.ready_for_walk))),
        ])))
    }

    async fn robot_status(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::RobotStatus>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let st = mb.control_status().await.map_err(map_sdk_err)?;
        let mode = st
            .fsm
            .as_ref()
            .map(|f| cms_kind_label(f.kind))
            .unwrap_or("unknown")
            .to_string();
        let health = if st.critical_motor_fault {
            doso::HealthStatus::Error as i32
        } else {
            doso::HealthStatus::Ok as i32
        };
        Ok(Response::new(doso::RobotStatus {
            mode,
            ready: Some(st.ready_for_walk),
            time: None,
            health,
            software_emergency_stop: Some(false),
            hardware_emergency_stop: Some(st.critical_motor_fault),
            reason: reject_label(st.last_decision),
        }))
    }

    async fn health(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::Snapshot>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let st = mb.control_status().await.map_err(map_sdk_err)?;
        Ok(Response::new(snapshot(vec![
            (
                "status",
                sval(Kind::StringValue(
                    if st.critical_motor_fault {
                        "error"
                    } else {
                        "ok"
                    }
                    .into(),
                )),
            ),
            ("brainstem_connected", sval(Kind::BoolValue(true))),
            (
                "critical_motor_fault",
                sval(Kind::BoolValue(st.critical_motor_fault)),
            ),
        ])))
    }

    async fn faults(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::FaultReport>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        Ok(Response::new(collect_fault_report(&mb).await?))
    }

    type WatchFaultsStream = BoxStream<doso::FaultReport>;
    async fn watch_faults(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<Self::WatchFaultsStream>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(200));
            loop {
                interval.tick().await;
                let item = collect_fault_report(&mb).await;
                let is_err = item.is_err();
                if tx.send(item).await.is_err() || is_err {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    async fn readiness(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::Snapshot>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let st = mb.control_status().await.map_err(map_sdk_err)?;
        let ready = !st.critical_motor_fault;
        Ok(Response::new(snapshot(vec![
            (
                "status",
                sval(Kind::StringValue(
                    if ready { "ready" } else { "degraded" }.into(),
                )),
            ),
            ("ready", sval(Kind::BoolValue(ready))),
            ("data_ready", sval(Kind::BoolValue(true))),
            ("motion_ready", sval(Kind::BoolValue(st.ready_for_walk))),
            (
                "non_motion_safe",
                sval(Kind::BoolValue(!st.motor_output_enabled)),
            ),
            (
                "advisories",
                sval(Kind::ListValue(ListValue {
                    values: vec![sval(Kind::StringValue(BOARD_B_MISSING_MSG.into()))],
                })),
            ),
        ])))
    }

    async fn capabilities(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::Snapshot>, Status> {
        self.check_auth(&request)?;
        Ok(Response::new(snapshot(vec![
            (
                "deployment_mode",
                sval(Kind::StringValue("standalone_board_a".into())),
            ),
            ("board_a_motion", sval(Kind::BoolValue(true))),
            ("board_b_autonomy", sval(Kind::BoolValue(false))),
            (
                "shm_ipc_enabled",
                sval(Kind::BoolValue(self.config.shm_enabled)),
            ),
            ("motion", sval(Kind::BoolValue(true))),
            ("sport", sval(Kind::BoolValue(true))),
            ("teleop", sval(Kind::BoolValue(true))),
            ("low_level", sval(Kind::BoolValue(true))),
            ("observations", sval(Kind::BoolValue(true))),
            ("localization", sval(Kind::BoolValue(false))),
            ("maps", sval(Kind::BoolValue(false))),
            ("navigation", sval(Kind::BoolValue(false))),
            ("tasks", sval(Kind::BoolValue(false))),
        ])))
    }

    async fn session(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::Snapshot>, Status> {
        self.check_auth(&request)?;
        Ok(Response::new(snapshot(vec![(
            "mode",
            sval(Kind::StringValue("standalone_board_a".into())),
        )])))
    }

    async fn product_status(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::Snapshot>, Status> {
        self.check_auth(&request)?;
        let current = snapshot(vec![
            (
                "status",
                sval(Kind::StringValue("standalone_board_a".into())),
            ),
            ("product", sval(Kind::StringValue("motion_chassis".into()))),
        ]);
        Ok(Response::new(snapshot(vec![
            ("available", sval(Kind::BoolValue(true))),
            (
                "current",
                sval(Kind::StructValue(current.fields.unwrap_or_default())),
            ),
        ])))
    }
}

#[tonic::async_trait]
impl doso::observations_server::Observations for BaseGatewayService {
    async fn scene(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::Snapshot>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }

    async fn camera_status(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::Snapshot>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }

    async fn camera_calibration(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::CameraCalibration>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }

    type WatchImuStream = BoxStream<doso::ImuSample>;
    async fn watch_imu(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<Self::WatchImuStream>, Status> {
        self.check_auth(&request)?;
        if self.config.shm_enabled {
            if let Ok(reader) = ProprioRingReader::open(&self.config.shm_proprio_name) {
                let poll = self.config.shm_poll_period;
                let (tx, rx) = mpsc::channel(64);
                tokio::spawn(async move {
                    let mut last_seq = 0_u64;
                    let mut tick = tokio::time::interval(poll);
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        tick.tick().await;
                        if let Some((seq, payload)) = reader.read_latest(last_seq) {
                            last_seq = seq;
                            if tx.send(Ok(map_shm_imu(&payload))).await.is_err() {
                                break;
                            }
                        }
                    }
                });
                return Ok(Response::new(Box::pin(
                    tokio_stream::wrappers::ReceiverStream::new(rx),
                )));
            }
        }
        let mb = self.client().await?;
        let mut upstream = mb.listen_imu().await.map_err(map_sdk_err)?;
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            while let Ok(Some(imu)) = upstream.message().await {
                if tx.send(Ok(map_imu(imu))).await.is_err() {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    type WatchJointsStream = BoxStream<doso::JointSample>;
    async fn watch_joints(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<Self::WatchJointsStream>, Status> {
        self.check_auth(&request)?;
        if self.config.shm_enabled {
            if let Ok(reader) = ProprioRingReader::open(&self.config.shm_proprio_name) {
                let poll = self.config.shm_poll_period;
                let (tx, rx) = mpsc::channel(64);
                tokio::spawn(async move {
                    let mut last_seq = 0_u64;
                    let mut tick = tokio::time::interval(poll);
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        tick.tick().await;
                        if let Some((seq, payload)) = reader.read_latest(last_seq) {
                            last_seq = seq;
                            if tx.send(Ok(map_shm_joint(&payload))).await.is_err() {
                                break;
                            }
                        }
                    }
                });
                return Ok(Response::new(Box::pin(
                    tokio_stream::wrappers::ReceiverStream::new(rx),
                )));
            }
        }
        let mb = self.client().await?;
        let mut upstream = mb.listen_joint().await.map_err(map_sdk_err)?;
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            while let Ok(Some(joint)) = upstream.message().await {
                if tx.send(Ok(map_joint(joint))).await.is_err() {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    async fn motors(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::MotorStates>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let resp = mb.motor_status().await.map_err(map_sdk_err)?;
        let motors = resp
            .motors
            .into_iter()
            .map(|m| doso::MotorState {
                id: m.id,
                online: m.online,
                status_code: m.status_code,
                temperature_c: m.temperature,
                errors: m.errors,
            })
            .collect();
        Ok(Response::new(doso::MotorStates { motors }))
    }

    async fn motor_voltages(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::MotorVoltagesResponse>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let resp = mb.voltage().await.map_err(map_sdk_err)?;
        Ok(Response::new(doso::MotorVoltagesResponse {
            volts: resp.values,
        }))
    }

    async fn camera_frame(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::CameraFrame>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }

    type WatchCameraStream = BoxStream<doso::CameraFrame>;
    async fn watch_camera(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<Self::WatchCameraStream>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }

    async fn camera_sample(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::CameraSample>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }

    type WatchCameraSamplesStream = BoxStream<doso::CameraSample>;
    async fn watch_camera_samples(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<Self::WatchCameraSamplesStream>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }

    type WatchPointCloudStream = BoxStream<doso::CloudFrame>;
    async fn watch_point_cloud(
        &self,
        request: Request<doso::CloudRequest>,
    ) -> std::result::Result<Response<Self::WatchPointCloudStream>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
}

#[tonic::async_trait]
impl doso::motion_server::Motion for BaseGatewayService {
    async fn controller(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::ControllerState>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let ext = mb.external().status().await.map_err(map_sdk_err)?;
        Ok(Response::new(map_ext_status_to_controller(&ext)))
    }

    type ExternalControlStream = BoxStream<doso::ExternalControlReply>;
    async fn external_control(
        &self,
        request: Request<tonic::Streaming<doso::ExternalControlRequest>>,
    ) -> std::result::Result<Response<Self::ExternalControlStream>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let mut requests = request.into_inner();
        let first = requests.message().await?.ok_or_else(|| {
            Status::invalid_argument("external control stream ended before Enter")
        })?;
        let first_cmd = first.command.unwrap_or_default();
        let req_id = Self::require_command(&first_cmd)?.to_string();
        if !matches!(
            first.action,
            Some(doso::external_control_request::Action::Enter(_))
        ) {
            return Err(Status::invalid_argument(
                "first external_control request must be Enter",
            ));
        }

        let ext_client = mb.external();
        let enter_status = ext_client
            .enter(&first_cmd.client_id, &req_id)
            .await
            .map_err(map_sdk_err)?;

        let (tx, rx) = mpsc::channel(32);
        if !enter_status.selected || !enter_status.active || enter_status.session_token.is_empty() {
            let _ = tx
                .send(Ok(doso::ExternalControlReply {
                    request_id: req_id,
                    sequence: first.sequence,
                    accepted: false,
                    reason: enter_status.reason.clone(),
                    ingress_accepted: false,
                    input_window: enter_status.input_window,
                    state: Some(map_ext_status_to_controller(&enter_status)),
                }))
                .await;
            return Ok(Response::new(Box::pin(
                tokio_stream::wrappers::ReceiverStream::new(rx),
            )));
        }

        let session_token = enter_status.session_token.clone();
        // Latest-wins relay: send the empty session handshake frame first, then coalesce any
        // bursts via a watch slot so Brainstem never executes stale queued joint targets.
        let (latest_tx, mut latest_rx) =
            tokio::sync::watch::channel::<Option<mb::ExternalFrame>>(None);
        let (frame_tx, frame_rx) = mpsc::channel::<mb::ExternalFrame>(2);
        // Real Brainstem ExternalService.control() requires an initial empty handshake frame
        // (sequence == 0, input_window == 0, targets.is_empty()) within 300ms before granting
        // the active input_window.
        let _ = frame_tx
            .send(mb::ExternalFrame {
                session_token: session_token.clone(),
                sequence: 0,
                targets: Vec::new(),
                input_window: 0,
            })
            .await;
        tokio::spawn(async move {
            while latest_rx.changed().await.is_ok() {
                let maybe_frame = latest_rx.borrow_and_update().clone();
                if let Some(frame) = maybe_frame {
                    if frame_tx.send(frame).await.is_err() {
                        break;
                    }
                }
            }
        });
        let mut status_stream = match ext_client
            .control(tokio_stream::wrappers::ReceiverStream::new(frame_rx))
            .await
        {
            Ok(stream) => stream,
            Err(err) => {
                let _ = ext_client.exit(&session_token).await;
                return Err(map_sdk_err(err));
            }
        };

        let initial_status = if enter_status.input_window > 0 {
            enter_status
        } else {
            match tokio::time::timeout(Duration::from_secs(2), status_stream.message()).await {
                Ok(Ok(Some(st))) => st,
                Ok(Ok(None)) => {
                    let _ = ext_client.exit(&session_token).await;
                    return Err(Status::unavailable(
                        "brainstem external control stream closed before granting input_window",
                    ));
                }
                Ok(Err(st)) => {
                    let _ = ext_client.exit(&session_token).await;
                    return Err(st);
                }
                Err(_) => {
                    let _ = ext_client.exit(&session_token).await;
                    return Err(Status::deadline_exceeded(
                        "timed out waiting for brainstem external input_window grant",
                    ));
                }
            }
        };

        let session_id_for_hash = if !initial_status.session_id.is_empty() {
            initial_status.session_id.clone()
        } else {
            req_id.clone()
        };
        let session_id_hash = doso_shm_ipc::fnv1a_64(session_id_for_hash.as_bytes());
        let mut shm_cmd_writer = if self.config.shm_enabled {
            match ExternalCmdWriter::open(&self.config.shm_cmd_name) {
                Ok(writer) => {
                    tracing::info!(
                        shm = %self.config.shm_cmd_name,
                        session_id = %session_id_for_hash,
                        "opened POSIX SHM external MIT command writer (control plane stays on gRPC)"
                    );
                    Some(writer)
                }
                Err(err) => {
                    tracing::warn!(
                        shm = %self.config.shm_cmd_name,
                        error = %err,
                        "failed to open SHM external command writer; falling back to gRPC frame relay"
                    );
                    None
                }
            }
        } else {
            None
        };

        let init_reply = doso::ExternalControlReply {
            request_id: req_id,
            sequence: first.sequence,
            accepted: initial_status.selected && initial_status.active,
            reason: initial_status.reason.clone(),
            ingress_accepted: initial_status.active && initial_status.input_window > 0,
            input_window: initial_status.input_window,
            state: Some(map_ext_status_to_controller(&initial_status)),
        };
        if tx.send(Ok(init_reply)).await.is_err() {
            let _ = ext_client.exit(&session_token).await;
            return Err(Status::cancelled("downstream external stream closed"));
        }

        tokio::spawn(async move {
            let mut closed_cleanly = false;
            loop {
                tokio::select! {
                    upstream = status_stream.message() => {
                        match upstream {
                            Ok(Some(up)) => {
                                let update = doso::ExternalControlReply {
                                    request_id: String::new(),
                                    sequence: 0,
                                    accepted: true,
                                    reason: up.reason.clone(),
                                    ingress_accepted: up.input_window > 0,
                                    input_window: up.input_window,
                                    state: Some(map_ext_status_to_controller(&up)),
                                };
                                if tx.send(Ok(update)).await.is_err() {
                                    break;
                                }
                            }
                            Ok(None) | Err(_) => break,
                        }
                    }
                    msg = requests.message() => {
                        let Ok(Some(req)) = msg else { break; };
                        let cmd = req.command.unwrap_or_default();
                        match req.action {
                            Some(doso::external_control_request::Action::Heartbeat(_)) => {
                                if let Ok(hb) = ext_client.heartbeat(&session_token).await {
                                    let _ = tx.try_send(Ok(doso::ExternalControlReply {
                                        request_id: String::new(),
                                        sequence: 0,
                                        accepted: true,
                                        reason: hb.reason.clone(),
                                        ingress_accepted: hb.input_window > 0,
                                        input_window: hb.input_window,
                                        state: Some(map_ext_status_to_controller(&hb)),
                                    }));
                                }
                            }
                            Some(doso::external_control_request::Action::Enable(_)) => {
                                match ext_client.enable(&session_token).await {
                                    Ok(st) => {
                                        let _ = tx.send(Ok(doso::ExternalControlReply {
                                            request_id: cmd.request_id,
                                            sequence: req.sequence,
                                            accepted: st.motor_output_enabled || st.reason.is_empty(),
                                            reason: st.reason.clone(),
                                            ingress_accepted: st.input_window > 0,
                                            input_window: st.input_window,
                                            state: Some(map_ext_status_to_controller(&st)),
                                        })).await;
                                    }
                                    Err(err) => {
                                        let _ = tx.send(Err(map_sdk_err(err))).await;
                                        break;
                                    }
                                }
                            }
                            Some(doso::external_control_request::Action::Disable(_)) => {
                                match ext_client.disable(&session_token).await {
                                    Ok(st) => {
                                        let _ = tx.send(Ok(doso::ExternalControlReply {
                                            request_id: cmd.request_id,
                                            sequence: req.sequence,
                                            accepted: true,
                                            reason: st.reason.clone(),
                                            ingress_accepted: st.input_window > 0,
                                            input_window: st.input_window,
                                            state: Some(map_ext_status_to_controller(&st)),
                                        })).await;
                                    }
                                    Err(err) => {
                                        let _ = tx.send(Err(map_sdk_err(err))).await;
                                        break;
                                    }
                                }
                            }
                            Some(doso::external_control_request::Action::Frame(targets)) => {
                                if let (Some(shm_writer), true) =
                                    (&mut shm_cmd_writer, targets.joints.len() == 16)
                                {
                                    let mut payload = DosoExternalCmdPayload {
                                        session_id_hash,
                                        input_window: req.input_window,
                                        sequence: req.sequence,
                                        monotonic_us: doso_shm_ipc::monotonic_us(),
                                        ..Default::default()
                                    };
                                    for (i, j) in targets.joints.iter().enumerate() {
                                        payload.q[i] = j.q;
                                        payload.dq[i] = j.dq;
                                        payload.kp[i] = j.kp;
                                        payload.kd[i] = j.kd;
                                        payload.tau[i] = j.tau;
                                    }
                                    if let Err(err) = shm_writer.publish(&payload) {
                                        tracing::warn!(
                                            error = %err,
                                            "SHM external command publish failed"
                                        );
                                    }
                                } else {
                                    let mapped: Vec<mb::ExternalJointCommand> = targets
                                        .joints
                                        .into_iter()
                                        .map(|j| mb::ExternalJointCommand {
                                            q: j.q,
                                            dq: j.dq,
                                            kp: j.kp,
                                            kd: j.kd,
                                            tau: j.tau,
                                        })
                                        .collect();
                                    let frame = mb::ExternalFrame {
                                        session_token: session_token.clone(),
                                        sequence: req.sequence,
                                        targets: mapped,
                                        input_window: req.input_window,
                                    };
                                    // Latest-wins overwrite: never queue stale 200Hz joint targets.
                                    let _ = latest_tx.send_replace(Some(frame));
                                }
                            }
                            Some(doso::external_control_request::Action::Close(_)) => {
                                let _ = ext_client.disable(&session_token).await;
                                match ext_client.exit(&session_token).await {
                                    Ok(st) => {
                                        let _ = tx.send(Ok(doso::ExternalControlReply {
                                            request_id: cmd.request_id,
                                            sequence: req.sequence,
                                            accepted: true,
                                            reason: st.reason.clone(),
                                            ingress_accepted: false,
                                            input_window: 0,
                                            state: Some(map_ext_status_to_controller(&st)),
                                        })).await;
                                    }
                                    Err(err) => {
                                        let _ = tx.send(Err(map_sdk_err(err))).await;
                                    }
                                }
                                closed_cleanly = true;
                                break;
                            }
                            Some(doso::external_control_request::Action::Enter(_)) | None => {}
                        }
                    }
                }
            }
            if !closed_cleanly {
                let _ = ext_client.disable(&session_token).await;
                let _ = ext_client.exit(&session_token).await;
            }
        });

        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    async fn stand_up(
        &self,
        request: Request<doso::Command>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        let cmd = request.into_inner();
        let req_id = Self::require_command(&cmd)?.to_string();
        let mb = self.client().await?;
        mb.stand_up().await.map_err(map_sdk_err)?;
        Ok(Response::new(Self::receipt(req_id, "posture_requested")))
    }

    async fn sit_down(
        &self,
        request: Request<doso::Command>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        let cmd = request.into_inner();
        let req_id = Self::require_command(&cmd)?.to_string();
        let mb = self.client().await?;
        mb.sit_down().await.map_err(map_sdk_err)?;
        Ok(Response::new(Self::receipt(req_id, "posture_requested")))
    }

    async fn damp(
        &self,
        request: Request<doso::Command>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        let cmd = request.into_inner();
        let req_id = Self::require_command(&cmd)?.to_string();
        let mb = self.client().await?;
        mb.damp().await.map_err(map_sdk_err)?;
        Ok(Response::new(Self::receipt(req_id, "damp_requested")))
    }

    async fn enable_motors(
        &self,
        request: Request<doso::Command>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        let cmd = request.into_inner();
        let req_id = Self::require_command(&cmd)?.to_string();
        let mb = self.client().await?;
        mb.enable_motors().await.map_err(map_sdk_err)?;
        Ok(Response::new(Self::receipt(
            req_id,
            "motor_power_acknowledged",
        )))
    }

    async fn disable_motors(
        &self,
        request: Request<doso::Command>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        let cmd = request.into_inner();
        let req_id = Self::require_command(&cmd)?.to_string();
        let mb = self.client().await?;
        mb.disable_motors().await.map_err(map_sdk_err)?;
        Ok(Response::new(Self::receipt(
            req_id,
            "motor_power_acknowledged",
        )))
    }

    async fn clear_motor_fault(
        &self,
        request: Request<doso::ClearMotorFaultRequest>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        let req = request.into_inner();
        let cmd = req
            .command
            .ok_or_else(|| Status::invalid_argument("command is required"))?;
        let req_id = Self::require_command(&cmd)?.to_string();
        if req.joint_ids.iter().any(|&id| id >= 16) {
            return Err(Status::invalid_argument(
                "joint_ids must be between 0 and 15",
            ));
        }
        let mb = self.client().await?;
        mb.clear_motor_fault(req.joint_ids)
            .await
            .map_err(map_sdk_err)?;
        Ok(Response::new(Self::receipt(
            req_id,
            "fault_clear_requested",
        )))
    }

    async fn list_gestures(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::GestureList>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let resp = mb.list_gestures().await.map_err(map_sdk_err)?;
        let gestures = resp
            .gestures
            .into_iter()
            .map(|g| doso::GestureInfo {
                name: g.name,
                description: g.description,
                duration_ms: g.duration_ms,
            })
            .collect();
        Ok(Response::new(doso::GestureList { gestures }))
    }

    async fn play_gesture(
        &self,
        request: Request<doso::PlayGestureRequest>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        let req = request.into_inner();
        let cmd = req
            .command
            .ok_or_else(|| Status::invalid_argument("command is required"))?;
        let req_id = Self::require_command(&cmd)?.to_string();
        if req.name.trim().is_empty() {
            return Err(Status::invalid_argument("gesture name must not be empty"));
        }
        let mb = self.client().await?;
        mb.play_gesture(req.name).await.map_err(map_sdk_err)?;
        Ok(Response::new(Self::receipt(req_id, "gesture_requested")))
    }

    async fn profile(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::ProfileInfo>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let info = mb.profile().await.map_err(map_sdk_err)?;
        Ok(Response::new(doso::ProfileInfo {
            current: info.current,
            available: info.available,
            descriptions: info.descriptions,
            current_description: info.current_description,
        }))
    }

    async fn switch_profile(
        &self,
        request: Request<doso::SwitchProfileRequest>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        let req = request.into_inner();
        let cmd = req
            .command
            .ok_or_else(|| Status::invalid_argument("command is required"))?;
        let req_id = Self::require_command(&cmd)?.to_string();
        if req.name.trim().is_empty() {
            return Err(Status::invalid_argument("profile name must not be empty"));
        }
        let mb = self.client().await?;
        mb.switch_profile(req.name).await.map_err(map_sdk_err)?;
        Ok(Response::new(Self::receipt(req_id, "profile_switched")))
    }

    async fn stop(
        &self,
        request: Request<doso::Command>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        let cmd = request.into_inner();
        let req_id = Self::require_command(&cmd)?.to_string();
        let mb = self.client().await?;
        mb.damp().await.map_err(map_sdk_err)?;
        Ok(Response::new(Self::receipt(req_id, "stop_requested")))
    }

    async fn stop_motion(
        &self,
        request: Request<doso::Command>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.stop(request).await
    }

    async fn clear_emergency_stop(
        &self,
        request: Request<doso::Command>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        let cmd = request.into_inner();
        let req_id = Self::require_command(&cmd)?.to_string();
        Ok(Response::new(Self::receipt(req_id, "acknowledged")))
    }

    async fn resume(
        &self,
        request: Request<doso::Command>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.clear_emergency_stop(request).await
    }

    async fn control_status(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::BodyControlStatus>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let st = mb.control_status().await.map_err(map_sdk_err)?;
        Ok(Response::new(map_status_to_body_control(&st)))
    }

    async fn control_state(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::ControlState>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let st = mb.control_status().await.map_err(map_sdk_err)?;
        Ok(Response::new(map_status_to_control_state(&st, false)))
    }

    type WatchControlStateStream = BoxStream<doso::ControlState>;
    async fn watch_control_state(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<Self::WatchControlStateStream>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(50));
            loop {
                interval.tick().await;
                let item = mb
                    .control_status()
                    .await
                    .map(|st| map_status_to_control_state(&st, false))
                    .map_err(map_sdk_err);
                let is_err = item.is_err();
                if tx.send(item).await.is_err() || is_err {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    async fn velocity(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::VelocitySample>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }

    type WatchVelocityStream = BoxStream<doso::VelocitySample>;
    async fn watch_velocity(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<Self::WatchVelocityStream>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }

    async fn odometry(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::OdometrySample>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }

    type WatchOdometryStream = BoxStream<doso::OdometrySample>;
    async fn watch_odometry(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<Self::WatchOdometryStream>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }

    async fn status(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::Snapshot>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let st = mb.control_status().await.map_err(map_sdk_err)?;
        Ok(Response::new(snapshot(vec![
            (
                "body_kind",
                sval(Kind::StringValue(
                    st.fsm
                        .as_ref()
                        .map(|f| cms_kind_label(f.kind))
                        .unwrap_or("unknown")
                        .into(),
                )),
            ),
            (
                "motors_enabled",
                sval(Kind::BoolValue(st.motor_output_enabled)),
            ),
            ("ready_for_walk", sval(Kind::BoolValue(st.ready_for_walk))),
        ])))
    }

    type WatchStateStream = BoxStream<doso::Snapshot>;
    async fn watch_state(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<Self::WatchStateStream>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(50));
            loop {
                interval.tick().await;
                let item = mb
                    .control_status()
                    .await
                    .map(|st| {
                        snapshot(vec![(
                            "ready_for_walk",
                            sval(Kind::BoolValue(st.ready_for_walk)),
                        )])
                    })
                    .map_err(map_sdk_err);
                let is_err = item.is_err();
                if tx.send(item).await.is_err() || is_err {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    type WatchStateAtStream = BoxStream<doso::Snapshot>;
    async fn watch_state_at(
        &self,
        request: Request<doso::WatchStateRequest>,
    ) -> std::result::Result<Response<Self::WatchStateAtStream>, Status> {
        self.check_auth(&request)?;
        let hz = request.into_inner().frequency_hz.clamp(1, 50);
        let mb = self.client().await?;
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis((1000 / hz) as u64));
            loop {
                interval.tick().await;
                let item = mb
                    .control_status()
                    .await
                    .map(|st| {
                        snapshot(vec![(
                            "ready_for_walk",
                            sval(Kind::BoolValue(st.ready_for_walk)),
                        )])
                    })
                    .map_err(map_sdk_err);
                let is_err = item.is_err();
                if tx.send(item).await.is_err() || is_err {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    async fn body_state(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::BodyState>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let cms = mb.cms_state().await.map_err(map_sdk_err)?;
        Ok(Response::new(map_cms_to_body(&cms)))
    }

    type WatchBodyStateStream = BoxStream<doso::BodyState>;
    async fn watch_body_state(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<Self::WatchBodyStateStream>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let initial = mb.cms_state().await.map_err(map_sdk_err)?;
        let mut upstream = mb.listen_cms_state().await.map_err(map_sdk_err)?;
        let (tx, rx) = mpsc::channel(32);
        tokio::spawn(async move {
            if tx.send(Ok(map_cms_to_body(&initial))).await.is_err() {
                return;
            }
            while let Ok(Some(state)) = upstream.message().await {
                if tx.send(Ok(map_cms_to_body(&state))).await.is_err() {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    type TeleopStream = BoxStream<doso::TeleopReply>;
    async fn teleop(
        &self,
        request: Request<tonic::Streaming<doso::TeleopRequest>>,
    ) -> std::result::Result<Response<Self::TeleopStream>, Status> {
        self.check_auth(&request)?;
        let mb = self.client().await?;
        let watchdog = self.config.teleop_watchdog;
        let brainstem_client_id = self.config.brainstem_client_id.clone();
        let mut requests = request.into_inner();
        let (tx, rx) = mpsc::channel(32);

        tokio::spawn(async move {
            const MIN_WALK_INTERVAL: Duration = Duration::from_millis(20);
            struct ActiveLease {
                token: String,
                next_seq: u64,
                last_walk_at: Option<Instant>,
            }
            async fn paced_walk(
                mb: &MotionBoardClient,
                lease: &mut ActiveLease,
                dir: mb::Vector3,
            ) -> std::result::Result<mb::CommandAck, doso_sdk::Error> {
                if let Some(prev) = lease.last_walk_at {
                    let elapsed = prev.elapsed();
                    if elapsed < MIN_WALK_INTERVAL {
                        tokio::time::sleep(MIN_WALK_INTERVAL - elapsed).await;
                    }
                }
                let ack = mb
                    .walk_checked(lease.token.clone(), lease.next_seq, dir)
                    .await?;
                lease.last_walk_at = Some(Instant::now());
                if ack.accepted {
                    lease.next_seq = lease.next_seq.saturating_add(1);
                }
                Ok(ack)
            }
            async fn acquire_and_ack_zero(
                mb: &MotionBoardClient,
                principal: &str,
            ) -> std::result::Result<(mb::ControlLease, Option<ActiveLease>), doso_sdk::Error>
            {
                let mut lease = mb.acquire_control(principal.to_string()).await?;
                if !lease.accepted {
                    return Ok((lease, None));
                }
                let mut active = ActiveLease {
                    token: lease.token.clone(),
                    next_seq: 1,
                    last_walk_at: None,
                };
                let zero = paced_walk(
                    mb,
                    &mut active,
                    mb::Vector3 {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                    },
                )
                .await?;
                if !zero.accepted {
                    let _ = mb.release_control(active.token).await;
                    lease.accepted = false;
                    lease.reason = zero.reason;
                    lease.status = zero.status;
                    return Ok((lease, None));
                }
                if zero.status.is_some() {
                    lease.status = zero.status;
                }
                Ok((lease, Some(active)))
            }

            let window_id: u64 = 1;
            let mut active_lease: Option<ActiveLease> = None;
            let mut last_vel_at: Option<Instant> = None;
            let mut moving = false;
            let mut tick = tokio::time::interval(Duration::from_millis(100));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        if let Some(lease) = &mut active_lease {
                            if moving && last_vel_at.is_some_and(|t| t.elapsed() >= watchdog) {
                                let _ = paced_walk(&mb, lease, mb::Vector3 { x: 0.0, y: 0.0, z: 0.0 }).await;
                                moving = false;
                            }
                            let _ = mb.renew_control_lease(lease.token.clone()).await;
                        }
                        if let Ok(st) = mb.control_status().await {
                            let _ = tx.try_send(Ok(doso::TeleopReply {
                                request_id: String::new(),
                                accepted: true,
                                reason: String::new(),
                                input_window: window_id,
                                ingress_accepted: false,
                                final_output_confirmed: false,
                                sequence: 0,
                                control: Some(map_status_to_control_state(&st, active_lease.is_some())),
                            }));
                        }
                    }
                    msg = requests.message() => {
                        let Ok(Some(req)) = msg else { break; };
                        let cmd = req.command.unwrap_or_default();
                        let mut reply = doso::TeleopReply {
                            request_id: cmd.request_id,
                            accepted: true,
                            reason: String::new(),
                            input_window: window_id,
                            ingress_accepted: false,
                            final_output_confirmed: false,
                            sequence: req.sequence,
                            control: None,
                        };
                        let mut is_close = false;
                        match req.action {
                            Some(doso::teleop_request::Action::Input(_)) => {
                                if let Ok(st) = mb.control_status().await {
                                    reply.control = Some(map_status_to_control_state(&st, active_lease.is_some()));
                                }
                            }
                            Some(doso::teleop_request::Action::Claim(_)) => {
                                match acquire_and_ack_zero(&mb, &brainstem_client_id).await {
                                    Ok((lease, maybe_active)) => {
                                        reply.accepted = lease.accepted;
                                        reply.reason = reject_label(lease.reason);
                                        if let Some(act) = maybe_active {
                                            active_lease = Some(act);
                                        }
                                        reply.control = lease.status.as_ref().map(|s| map_status_to_control_state(s, active_lease.is_some()));
                                    }
                                    Err(err) => {
                                        reply.accepted = false;
                                        reply.reason = err.message;
                                    }
                                }
                            }
                            Some(doso::teleop_request::Action::Velocity(vel)) => {
                                if active_lease.is_none() {
                                    match acquire_and_ack_zero(&mb, &brainstem_client_id).await {
                                        Ok((_, Some(act))) => active_lease = Some(act),
                                        Ok((lease, None)) => {
                                            reply.accepted = false;
                                            reply.reason = reject_label(lease.reason);
                                        }
                                        Err(err) => {
                                            reply.accepted = false;
                                            reply.reason = err.message;
                                        }
                                    }
                                }
                                if let Some(lease) = &mut active_lease {
                                    let (vx, vy, wz) = if vel.deadman {
                                        (vel.vx_m_s, vel.vy_m_s, vel.yaw_rad_s)
                                    } else {
                                        (0.0, 0.0, 0.0)
                                    };
                                    match paced_walk(&mb, lease, mb::Vector3 { x: vx, y: vy, z: wz }).await {
                                        Ok(ack) => {
                                            reply.accepted = ack.accepted;
                                            reply.ingress_accepted = ack.accepted;
                                            reply.reason = reject_label(ack.reason);
                                            if ack.accepted {
                                                last_vel_at = Some(Instant::now());
                                                moving = vx.abs() > 1e-6 || vy.abs() > 1e-6 || wz.abs() > 1e-6;
                                            }
                                            reply.control = ack.status.as_ref().map(|s| map_status_to_control_state(s, true));
                                        }
                                        Err(err) => {
                                            reply.accepted = false;
                                            reply.reason = err.message;
                                        }
                                    }
                                }
                            }
                            Some(doso::teleop_request::Action::Hold(_)) => {
                                if let Some(lease) = &mut active_lease {
                                    let _ = paced_walk(&mb, lease, mb::Vector3 { x: 0.0, y: 0.0, z: 0.0 }).await;
                                    moving = false;
                                }
                                reply.final_output_confirmed = true;
                                if let Ok(st) = mb.control_status().await {
                                    reply.control = Some(map_status_to_control_state(&st, active_lease.is_some()));
                                }
                            }
                            Some(doso::teleop_request::Action::Close(_)) => {
                                is_close = true;
                                reply.input_window = 0;
                                if let Some(mut lease) = active_lease.take() {
                                    let _ = paced_walk(&mb, &mut lease, mb::Vector3 { x: 0.0, y: 0.0, z: 0.0 }).await;
                                    moving = false;
                                    if let Ok(st) = mb.release_control(lease.token).await {
                                        reply.control = Some(map_status_to_control_state(&st, false));
                                    }
                                }
                                reply.final_output_confirmed = true;
                            }
                            None => {}
                        }
                        if tx.send(Ok(reply)).await.is_err() || is_close {
                            break;
                        }
                    }
                }
            }

            if let Some(mut lease) = active_lease.take() {
                let _ = paced_walk(
                    &mb,
                    &mut lease,
                    mb::Vector3 {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                    },
                )
                .await;
                let _ = mb.release_control(lease.token).await;
            }
        });

        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }
}

#[tonic::async_trait]
impl doso::localization_server::Localization for BaseGatewayService {
    async fn status(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::Snapshot>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn state(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::LocalizationState>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn pose(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::PoseSample>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    type WatchPoseStream = BoxStream<doso::PoseSample>;
    async fn watch_pose(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<Self::WatchPoseStream>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn location(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::GeographicLocation>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    type WatchLocationStream = BoxStream<doso::GeographicLocation>;
    async fn watch_location(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<Self::WatchLocationStream>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn relocalize(
        &self,
        request: Request<doso::Relocalize>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
}

#[tonic::async_trait]
impl doso::maps_server::Maps for BaseGatewayService {
    async fn list(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::MapList>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    type PointCloudStream = BoxStream<doso::MapContent>;
    async fn point_cloud(
        &self,
        request: Request<doso::MapContentRequest>,
    ) -> std::result::Result<Response<Self::PointCloudStream>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn grid(
        &self,
        request: Request<doso::MapContentRequest>,
    ) -> std::result::Result<Response<doso::MapGrid>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn save(
        &self,
        request: Request<doso::SaveMap>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn operation(
        &self,
        request: Request<doso::MapQuery>,
    ) -> std::result::Result<Response<doso::MapOperation>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn switch(
        &self,
        request: Request<doso::SwitchProduct>,
    ) -> std::result::Result<Response<doso::ProductOperation>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn product_operation(
        &self,
        request: Request<doso::ProductQuery>,
    ) -> std::result::Result<Response<doso::ProductOperation>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
}

#[tonic::async_trait]
impl doso::navigation_server::Navigation for BaseGatewayService {
    async fn go(
        &self,
        request: Request<doso::Goal>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn go_with_options(
        &self,
        request: Request<doso::Goal>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn preview_plan(
        &self,
        request: Request<doso::PlanQuery>,
    ) -> std::result::Result<Response<doso::PlanPreview>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn set_speed_limit(
        &self,
        request: Request<doso::TaskSpeedLimit>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn status(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::Snapshot>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn path(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::Snapshot>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn route(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<doso::NavigationPath>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    type WatchPathStream = BoxStream<doso::NavigationPath>;
    async fn watch_path(
        &self,
        request: Request<doso::Empty>,
    ) -> std::result::Result<Response<Self::WatchPathStream>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn task(
        &self,
        request: Request<doso::TaskQuery>,
    ) -> std::result::Result<Response<doso::Task>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn cancel(
        &self,
        request: Request<doso::CancelTask>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn pause(
        &self,
        request: Request<doso::TaskControl>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn resume(
        &self,
        request: Request<doso::TaskControl>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    type WatchStream = BoxStream<doso::Task>;
    async fn watch(
        &self,
        request: Request<doso::TaskQuery>,
    ) -> std::result::Result<Response<Self::WatchStream>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
}

#[tonic::async_trait]
impl doso::tasks_server::Tasks for BaseGatewayService {
    async fn send(
        &self,
        request: Request<doso::Instruction>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn get(
        &self,
        request: Request<doso::RunQuery>,
    ) -> std::result::Result<Response<doso::Run>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    type WatchStream = BoxStream<doso::Run>;
    async fn watch(
        &self,
        request: Request<doso::RunQuery>,
    ) -> std::result::Result<Response<Self::WatchStream>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn cancel(
        &self,
        request: Request<doso::CancelRun>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn inspection_routes(
        &self,
        request: Request<doso::InspectionRouteQuery>,
    ) -> std::result::Result<Response<doso::InspectionRoutes>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn inspection_route(
        &self,
        request: Request<doso::InspectionRouteQuery>,
    ) -> std::result::Result<Response<doso::InspectionRoute>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn put_inspection_route(
        &self,
        request: Request<doso::PutInspectionRoute>,
    ) -> std::result::Result<Response<doso::InspectionRoute>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn delete_inspection_route(
        &self,
        request: Request<doso::DeleteInspectionRoute>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn start_inspection(
        &self,
        request: Request<doso::StartInspection>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn inspection_task(
        &self,
        request: Request<doso::InspectionTaskQuery>,
    ) -> std::result::Result<Response<doso::InspectionTask>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn inspection_tasks(
        &self,
        request: Request<doso::InspectionTaskListQuery>,
    ) -> std::result::Result<Response<doso::InspectionTasks>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    type WatchInspectionStream = BoxStream<doso::InspectionTask>;
    async fn watch_inspection(
        &self,
        request: Request<doso::InspectionTaskQuery>,
    ) -> std::result::Result<Response<Self::WatchInspectionStream>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn pause_inspection(
        &self,
        request: Request<doso::InspectionControl>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn resume_inspection(
        &self,
        request: Request<doso::InspectionControl>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn cancel_inspection(
        &self,
        request: Request<doso::InspectionControl>,
    ) -> std::result::Result<Response<doso::Receipt>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
    async fn inspection_report(
        &self,
        request: Request<doso::InspectionTaskQuery>,
    ) -> std::result::Result<Response<doso::Snapshot>, Status> {
        self.check_auth(&request)?;
        Err(unimplemented_board_b())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doso_sdk::{Code, JointCommand, RobotClient};
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct MockBrainstem {
        calls: StdMutex<Vec<String>>,
        walks: StdMutex<Vec<mb::WalkRequest>>,
        ext_events: StdMutex<Vec<String>>,
        ext_frames: Arc<StdMutex<Vec<mb::ExternalFrame>>>,
        profile: StdMutex<String>,
    }

    #[tonic::async_trait]
    impl mb::robot_control_server::RobotControl for MockBrainstem {
        type ListenHistoryStream =
            tokio_stream::wrappers::ReceiverStream<std::result::Result<mb::History, Status>>;
        type ListenCmsStateStream =
            tokio_stream::wrappers::ReceiverStream<std::result::Result<mb::CmsState, Status>>;
        type ListenImuStream =
            tokio_stream::wrappers::ReceiverStream<std::result::Result<mb::Imu, Status>>;
        type ListenJointStream =
            tokio_stream::wrappers::ReceiverStream<std::result::Result<mb::Joint, Status>>;

        async fn walk(&self, _: Request<mb::Vector3>) -> std::result::Result<Response<()>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn walk_checked(
            &self,
            request: Request<mb::WalkRequest>,
        ) -> std::result::Result<Response<mb::CommandAck>, Status> {
            let inner = request.into_inner();
            let seq = inner.sequence;
            self.walks.lock().unwrap().push(inner);
            Ok(Response::new(mb::CommandAck {
                accepted: true,
                reason: mb::CommandRejectReason::None as i32,
                sequence: seq,
                status: None,
            }))
        }

        async fn enable(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
            self.calls.lock().unwrap().push("enable_motors".into());
            Ok(Response::new(()))
        }

        async fn disable(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
            self.calls.lock().unwrap().push("disable_motors".into());
            Ok(Response::new(()))
        }

        async fn acquire_control(
            &self,
            request: Request<mb::AcquireControlRequest>,
        ) -> std::result::Result<Response<mb::ControlLease>, Status> {
            let client_id = request.into_inner().client_id;
            if client_id != crate::bridge::DEFAULT_LOOPBACK_CLIENT_ID {
                return Err(Status::permission_denied(format!(
                    "AcquireControl client_id '{client_id}' must match authenticated loopback principal"
                )));
            }
            Ok(Response::new(mb::ControlLease {
                accepted: true,
                token: "lease-a".into(),
                lease_duration_ms: 500,
                reason: mb::CommandRejectReason::None as i32,
                status: None,
            }))
        }

        async fn renew_control_lease(
            &self,
            request: Request<mb::ControlLeaseRequest>,
        ) -> std::result::Result<Response<mb::ControlLease>, Status> {
            Ok(Response::new(mb::ControlLease {
                accepted: true,
                token: request.into_inner().token,
                lease_duration_ms: 500,
                reason: mb::CommandRejectReason::None as i32,
                status: None,
            }))
        }

        async fn release_control(
            &self,
            _: Request<mb::ControlLeaseRequest>,
        ) -> std::result::Result<Response<mb::ControlStatus>, Status> {
            self.calls.lock().unwrap().push("release_control".into());
            Ok(Response::new(self.status_msg()))
        }

        async fn stand_up(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
            self.calls.lock().unwrap().push("stand_up".into());
            Ok(Response::new(()))
        }

        async fn sit_down(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
            self.calls.lock().unwrap().push("sit_down".into());
            Ok(Response::new(()))
        }

        async fn damp(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
            self.calls.lock().unwrap().push("damp".into());
            Ok(Response::new(()))
        }

        async fn set_speed_mode(
            &self,
            _: Request<mb::SpeedModeRequest>,
        ) -> std::result::Result<Response<()>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn get_speed_mode(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<mb::SpeedModeRequest>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn tick(&self, _: Request<()>) -> std::result::Result<Response<mb::History>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn step(
            &self,
            _: Request<mb::SimState>,
        ) -> std::result::Result<Response<()>, Status> {
            Err(Status::unimplemented("unused"))
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
        ) -> std::result::Result<Response<mb::CmsState>, Status> {
            Ok(Response::new(mb::CmsState {
                kind: mb::CmsStateKind::Standing as i32,
                transition: mb::CmsTransitionKind::None as i32,
                gesture_name: String::new(),
            }))
        }

        async fn get_control_status(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<mb::ControlStatus>, Status> {
            Ok(Response::new(self.status_msg()))
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
            let (tx, rx) = mpsc::channel(4);
            tokio::spawn(async move {
                let _ = tx
                    .send(Ok(mb::CmsState {
                        kind: mb::CmsStateKind::Standing as i32,
                        transition: mb::CmsTransitionKind::None as i32,
                        gesture_name: String::new(),
                    }))
                    .await;
                tokio::time::sleep(Duration::from_secs(2)).await;
            });
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
                rx,
            )))
        }

        async fn listen_imu(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<Self::ListenImuStream>, Status> {
            let (tx, rx) = mpsc::channel(4);
            let _ = tx
                .send(Ok(mb::Imu {
                    gyroscope: Some(mb::Vector3 {
                        x: 0.01,
                        y: -0.01,
                        z: 0.02,
                    }),
                    quaternion: Some(mb::Quaternion {
                        w: 1.0,
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                    }),
                    linear_acceleration: Some(mb::Vector3 {
                        x: 0.0,
                        y: 0.0,
                        z: 9.81,
                    }),
                    timestamp: None,
                }))
                .await;
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
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
            let (tx, rx) = mpsc::channel(4);
            let _ = tx
                .send(Ok(mb::Joint {
                    timestamp: None,
                    data: Some(mb::joint::Data::AllJoints(mb::AllJoints {
                        position: Some(mb::Matrix4 {
                            values: (0..16).map(|i| i as f64 * 0.05).collect(),
                        }),
                        velocity: Some(mb::Matrix4 {
                            values: vec![0.2; 16],
                        }),
                        torque: Some(mb::Matrix4 {
                            values: vec![0.1; 16],
                        }),
                        status: Some(mb::Matrix4Int32 {
                            values: vec![0; 16],
                        }),
                    })),
                }))
                .await;
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                drop(tx);
            });
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
                rx,
            )))
        }

        async fn get_params(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<mb::Params>, Status> {
            Err(Status::unimplemented("unused"))
        }

        async fn switch_profile(
            &self,
            request: Request<mb::ProfileRequest>,
        ) -> std::result::Result<Response<mb::ProfileInfo>, Status> {
            let name = request.into_inner().name;
            *self.profile.lock().unwrap() = name.clone();
            self.calls.lock().unwrap().push(format!("profile:{name}"));
            Ok(Response::new(mb::ProfileInfo {
                current: name,
                available: vec!["flat_w5".into(), "cts_v39".into()],
                descriptions: vec!["Flat".into(), "Stairs".into()],
                current_description: "Active".into(),
                mode_capabilities: Vec::new(),
                active_mode_id: "flat".into(),
            }))
        }

        async fn get_profile(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<mb::ProfileInfo>, Status> {
            let cur = self.profile.lock().unwrap().clone();
            Ok(Response::new(mb::ProfileInfo {
                current: if cur.is_empty() {
                    "flat_w5".into()
                } else {
                    cur
                },
                available: vec!["flat_w5".into(), "cts_v39".into()],
                descriptions: vec!["Flat".into(), "Stairs".into()],
                current_description: "Flat".into(),
                mode_capabilities: Vec::new(),
                active_mode_id: "flat".into(),
            }))
        }

        async fn play_gesture(
            &self,
            request: Request<mb::GestureRequest>,
        ) -> std::result::Result<Response<()>, Status> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("gesture:{}", request.into_inner().name));
            Ok(Response::new(()))
        }

        async fn list_gestures(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<mb::GestureList>, Status> {
            Ok(Response::new(mb::GestureList {
                gestures: vec![mb::GestureInfo {
                    name: "bow".into(),
                    description: "Bow".into(),
                    duration_ms: 1500,
                }],
            }))
        }

        async fn get_voltage(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<mb::Voltage>, Status> {
            Ok(Response::new(mb::Voltage {
                values: vec![48.0; 16],
            }))
        }

        async fn get_motor_status(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<mb::MotorStatusResponse>, Status> {
            Ok(Response::new(mb::MotorStatusResponse {
                motors: (0..16)
                    .map(|id| mb::MotorState {
                        id,
                        online: true,
                        status_code: 0,
                        temperature: 32.5,
                        voltage: 48.0,
                        position: 0.0,
                        velocity: 0.0,
                        torque: 0.0,
                        errors: Vec::new(),
                    })
                    .collect(),
            }))
        }

        async fn clear_motor_fault(
            &self,
            request: Request<mb::ClearFaultRequest>,
        ) -> std::result::Result<Response<()>, Status> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("clear_fault:{:?}", request.into_inner().joint_ids));
            Ok(Response::new(()))
        }

        async fn set_zero(&self, _: Request<()>) -> std::result::Result<Response<()>, Status> {
            Err(Status::permission_denied("factory zero locked"))
        }
    }

    #[tonic::async_trait]
    impl mb::robot_external_control_server::RobotExternalControl for MockBrainstem {
        type ControlStream =
            tokio_stream::wrappers::ReceiverStream<std::result::Result<mb::ExternalStatus, Status>>;

        async fn enter(
            &self,
            _: Request<mb::ExternalEnterRequest>,
        ) -> std::result::Result<Response<mb::ExternalStatus>, Status> {
            self.ext_events.lock().unwrap().push("enter".into());
            // Real Brainstem ExternalService.enter() returns input_window = 0 until control()
            // completes the empty-frame session handshake.
            Ok(Response::new(self.ext_msg(true, false, 0)))
        }

        async fn enable(
            &self,
            _: Request<mb::ExternalSession>,
        ) -> std::result::Result<Response<mb::ExternalStatus>, Status> {
            self.ext_events.lock().unwrap().push("enable".into());
            Ok(Response::new(self.ext_msg(true, true, 77)))
        }

        async fn disable(
            &self,
            _: Request<mb::ExternalSession>,
        ) -> std::result::Result<Response<mb::ExternalStatus>, Status> {
            self.ext_events.lock().unwrap().push("disable".into());
            Ok(Response::new(self.ext_msg(true, false, 77)))
        }

        async fn heartbeat(
            &self,
            _: Request<mb::ExternalSession>,
        ) -> std::result::Result<Response<mb::ExternalStatus>, Status> {
            Ok(Response::new(self.ext_msg(true, true, 78)))
        }

        async fn control(
            &self,
            request: Request<tonic::Streaming<mb::ExternalFrame>>,
        ) -> std::result::Result<Response<Self::ControlStream>, Status> {
            let mut incoming = request.into_inner();
            let (tx, rx) = mpsc::channel(16);
            let frames = self.ext_frames.clone();
            let status_base = self.ext_msg(true, true, 77);
            tokio::spawn(async move {
                let Ok(Some(handshake)) = incoming.message().await else {
                    let _ = tx
                        .send(Err(Status::invalid_argument(
                            "missing external session handshake frame",
                        )))
                        .await;
                    return;
                };
                if handshake.sequence != 0
                    || handshake.input_window != 0
                    || !handshake.targets.is_empty()
                {
                    let _ = tx
                        .send(Err(Status::invalid_argument(
                            "external control stream must begin with an empty session handshake",
                        )))
                        .await;
                    return;
                }
                if tx.send(Ok(status_base.clone())).await.is_err() {
                    return;
                }
                while let Ok(Some(f)) = incoming.message().await {
                    let seq = f.sequence;
                    frames.lock().unwrap().push(f);
                    let mut st = status_base.clone();
                    st.last_sequence = seq;
                    let _ = tx.send(Ok(st)).await;
                }
            });
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
                rx,
            )))
        }

        async fn exit(
            &self,
            _: Request<mb::ExternalSession>,
        ) -> std::result::Result<Response<mb::ExternalStatus>, Status> {
            self.ext_events.lock().unwrap().push("exit".into());
            Ok(Response::new(self.ext_msg(false, false, 0)))
        }

        async fn get_status(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<mb::ExternalStatus>, Status> {
            Ok(Response::new(self.ext_msg(false, false, 0)))
        }
    }

    impl MockBrainstem {
        fn status_msg(&self) -> mb::ControlStatus {
            mb::ControlStatus {
                fsm: Some(mb::CmsState {
                    kind: mb::CmsStateKind::Standing as i32,
                    transition: mb::CmsTransitionKind::None as i32,
                    gesture_name: String::new(),
                }),
                motor_output_enabled: true,
                owner: mb::ControlOwner::Grpc as i32,
                critical_motor_fault: false,
                grpc_lease_active: true,
                lease_remaining_ms: 400,
                last_accepted_sequence: 1,
                last_decision: mb::CommandRejectReason::None as i32,
                ready_for_walk: true,
                owner_id: "base-gw".into(),
                accepted_count: 1,
                rejected_count: 0,
            }
        }

        fn ext_msg(&self, active: bool, enabled: bool, window: u64) -> mb::ExternalStatus {
            mb::ExternalStatus {
                selected: active,
                active,
                session_id: "ext-sess-a".into(),
                session_token: "ext-tok-a".into(),
                motor_output_enabled: enabled,
                last_sequence: 0,
                reason: String::new(),
                motor_disable_confirmed: !enabled,
                client_id: "rl-client".into(),
                input_window: window,
                joint_count: 16,
            }
        }
    }

    async fn spawn_mock_brainstem() -> (
        String,
        Arc<MockBrainstem>,
        oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let svc = Arc::new(MockBrainstem::default());
        let (tx, rx) = oneshot::channel();
        let server = tonic::transport::Server::builder()
            .add_service(mb::robot_control_server::RobotControlServer::from_arc(
                svc.clone(),
            ))
            .add_service(
                mb::robot_external_control_server::RobotExternalControlServer::from_arc(
                    svc.clone(),
                ),
            )
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async move {
                let _ = rx.await;
            });
        let h = tokio::spawn(async move {
            let _ = server.await;
        });
        (format!("http://{addr}"), svc, tx, h)
    }

    #[tokio::test]
    async fn standalone_board_a_without_board_b_runs_l1_l2_and_reports_l3_uninstalled() {
        let (mb_url, mb_svc, mb_stop, mb_task) = spawn_mock_brainstem().await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (gw_stop, gw_rx) = oneshot::channel();
        let gw_cfg = BaseGatewayConfig {
            brainstem_endpoint: mb_url,
            robot_id: Some("thunder-chassis-001".into()),
            ..BaseGatewayConfig::default()
        };
        let (gw_addr, gw_task) =
            spawn_base_gateway_on_listener(BaseGatewayService::new(gw_cfg), listener, gw_rx)
                .await
                .unwrap();

        let robot = RobotClient::connect(format!("http://{gw_addr}"))
            .await
            .unwrap();
        assert_eq!(robot.system().ping().await.unwrap().status, "ok");
        assert_eq!(
            robot.system().info().robot_id.as_deref(),
            Some("thunder-chassis-001")
        );
        let caps = robot.system().capabilities().await.unwrap().fields.unwrap();
        assert_eq!(
            caps.fields["board_b_autonomy"].kind,
            Some(Kind::BoolValue(false))
        );
        assert_eq!(caps.fields["low_level"].kind, Some(Kind::BoolValue(true)));

        // L2 Sport + Observations without Board B.
        robot.sport().enable_motors(None).await.unwrap();
        robot.sport().stand_up(None).await.unwrap();
        robot.sport().switch_profile("cts_v39", None).await.unwrap();
        let gestures = robot.sport().list_gestures().await.unwrap();
        assert_eq!(gestures.gestures[0].name, "bow");
        robot.sport().play_gesture("bow", None).await.unwrap();
        robot.sport().clear_motor_fault([0, 1], None).await.unwrap();
        robot.sport().damp(None).await.unwrap();

        let motors = robot.observations().motors().await.unwrap();
        assert_eq!(motors.motors.len(), 16);
        let volts = robot.observations().motor_voltages().await.unwrap();
        assert_eq!(volts.volts.len(), 16);
        let imu_sample = robot
            .observations()
            .watch_imu()
            .await
            .unwrap()
            .message()
            .await
            .unwrap()
            .unwrap();
        assert!((imu_sample.angular_velocity_rad_s.unwrap().x - 0.01).abs() < 1e-6);
        let j_sample = robot
            .observations()
            .watch_joints()
            .await
            .unwrap()
            .message()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(j_sample.joints.len(), 16);

        // L2 Continuous Teleop without Board B.
        let mut teleop = robot.motion().claim_control(None).await.unwrap();
        let rep = teleop
            .send_velocity(
                doso::TeleopVelocity {
                    vx_m_s: 0.4,
                    deadman: true,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert!(rep.accepted && rep.ingress_accepted);
        assert!(teleop.hold(None).await.unwrap().final_output_confirmed);
        assert!(teleop.close(None).await.unwrap().accepted);
        {
            let walks = mb_svc.walks.lock().unwrap();
            assert!(
                walks.len() >= 3,
                "expected initial zero ack + velocity + hold/close zero"
            );
            assert_eq!(walks[0].sequence, 1);
            assert!(walks[0].direction.unwrap().x.abs() < 1e-6);
            assert_eq!(walks[1].sequence, 2);
            assert!((walks[1].direction.unwrap().x - 0.4).abs() < 1e-6);
        }

        // L1 Low-level 16-axis MIT RL without Board B.
        let mut ext = robot.low_level(None).await.unwrap();
        assert!(ext.enable(None).await.unwrap().accepted);
        let mut cmds: Vec<JointCommand> = (0..12)
            .map(|_| JointCommand::leg_pd(0.1, 35.0, 1.2))
            .collect();
        cmds.extend((0..4).map(|_| JointCommand::wheel_vel(2.5, 0.6)));
        ext.publish_joints(cmds, None).unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(ext.close(None).await.unwrap().accepted);
        {
            let frames = mb_svc.ext_frames.lock().unwrap();
            assert_eq!(frames.len(), 1);
            assert_eq!(frames[0].input_window, 77);
            assert!((frames[0].targets[12].dq - 2.5).abs() < 1e-6);
            assert_eq!(
                *mb_svc.ext_events.lock().unwrap(),
                vec!["enter", "enable", "disable", "exit"]
            );
        }

        // L3 Autonomy cleanly fails with Unimplemented on standalone chassis.
        let err_map = robot.maps().list().await.unwrap_err();
        assert_eq!(err_map.code(), Code::Unimplemented);
        assert!(err_map
            .message
            .contains("Board B autonomy (LingTu) is not installed"));
        let err_nav = robot.navigation().status().await.unwrap_err();
        assert_eq!(err_nav.code(), Code::Unimplemented);

        let _ = gw_stop.send(());
        gw_task.await.unwrap().unwrap();
        let _ = mb_stop.send(());
        mb_task.await.unwrap();
    }

    #[derive(Clone)]
    struct MockBoardBHandshakeOnly {
        low_level_endpoint: String,
    }

    #[tonic::async_trait]
    impl doso::system_server::System for MockBoardBHandshakeOnly {
        async fn handshake(
            &self,
            _: Request<doso::Handshake>,
        ) -> std::result::Result<Response<doso::HandshakeReply>, Status> {
            Ok(Response::new(doso::HandshakeReply {
                protocol_version: 2,
                info: Some(doso::SystemInfo {
                    robot_id: Some("full-robot-ab".into()),
                    firmware_version: Some("0.1.0".into()),
                    robot_version: Some("thunder".into()),
                    low_level_endpoint: Some(self.low_level_endpoint.clone()),
                }),
            }))
        }
        async fn ping(
            &self,
            _: Request<doso::Empty>,
        ) -> std::result::Result<Response<doso::Ping>, Status> {
            Ok(Response::new(doso::Ping {
                status: "ok".into(),
            }))
        }
        async fn state(
            &self,
            _: Request<doso::Empty>,
        ) -> std::result::Result<Response<doso::Snapshot>, Status> {
            Err(Status::unimplemented("unused"))
        }
        async fn robot_status(
            &self,
            _: Request<doso::Empty>,
        ) -> std::result::Result<Response<doso::RobotStatus>, Status> {
            Err(Status::unimplemented("unused"))
        }
        async fn health(
            &self,
            _: Request<doso::Empty>,
        ) -> std::result::Result<Response<doso::Snapshot>, Status> {
            Err(Status::unimplemented("unused"))
        }
        async fn faults(
            &self,
            _: Request<doso::Empty>,
        ) -> std::result::Result<Response<doso::FaultReport>, Status> {
            Err(Status::unimplemented("unused"))
        }
        type WatchFaultsStream = BoxStream<doso::FaultReport>;
        async fn watch_faults(
            &self,
            _: Request<doso::Empty>,
        ) -> std::result::Result<Response<Self::WatchFaultsStream>, Status> {
            Err(Status::unimplemented("unused"))
        }
        async fn readiness(
            &self,
            _: Request<doso::Empty>,
        ) -> std::result::Result<Response<doso::Snapshot>, Status> {
            Err(Status::unimplemented("unused"))
        }
        async fn capabilities(
            &self,
            _: Request<doso::Empty>,
        ) -> std::result::Result<Response<doso::Snapshot>, Status> {
            Err(Status::unimplemented("unused"))
        }
        async fn session(
            &self,
            _: Request<doso::Empty>,
        ) -> std::result::Result<Response<doso::Snapshot>, Status> {
            Err(Status::unimplemented("unused"))
        }
        async fn product_status(
            &self,
            _: Request<doso::Empty>,
        ) -> std::result::Result<Response<doso::Snapshot>, Status> {
            Err(Status::unimplemented("unused"))
        }
    }

    #[tokio::test]
    async fn dual_board_handshake_redirects_low_level_directly_to_board_a_gateway() {
        let (mb_url, mb_svc, mb_stop, mb_task) = spawn_mock_brainstem().await;
        let listener_a = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (gw_stop, gw_rx) = oneshot::channel();
        let gw_cfg = BaseGatewayConfig {
            brainstem_endpoint: mb_url,
            ..BaseGatewayConfig::default()
        };
        let (addr_a, gw_task) =
            spawn_base_gateway_on_listener(BaseGatewayService::new(gw_cfg), listener_a, gw_rx)
                .await
                .unwrap();

        // Board B serves :5052 System WITHOUT any MotionServer, advertising Board A on :5053.
        let listener_b = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_b = listener_b.local_addr().unwrap();
        let (b_stop, b_rx) = oneshot::channel();
        let b_server = tonic::transport::Server::builder()
            .add_service(doso::system_server::SystemServer::new(
                MockBoardBHandshakeOnly {
                    low_level_endpoint: format!("http://{addr_a}"),
                },
            ))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener_b), async move {
                let _ = b_rx.await;
            });
        let b_task = tokio::spawn(async move {
            let _ = b_server.await;
        });

        let robot = RobotClient::connect(format!("http://{addr_b}"))
            .await
            .unwrap();
        let mut ext = robot.low_level(None).await.unwrap();
        assert!(ext.enable(None).await.unwrap().accepted);
        let cmds = vec![JointCommand::leg_pd(0.25, 40.0, 1.5); 16];
        ext.publish_joints(cmds, None).unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(ext.close(None).await.unwrap().accepted);

        {
            let frames = mb_svc.ext_frames.lock().unwrap();
            assert_eq!(frames.len(), 1);
            assert!((frames[0].targets[0].q - 0.25).abs() < 1e-6);
        }

        let _ = b_stop.send(());
        b_task.await.unwrap();
        let _ = gw_stop.send(());
        gw_task.await.unwrap().unwrap();
        let _ = mb_stop.send(());
        mb_task.await.unwrap();
    }

    #[tokio::test]
    async fn shm_data_plane_serves_proprio_and_writes_external_mit_frames() {
        let suffix = format!("{}_{}", std::process::id(), doso_shm_ipc::monotonic_us());
        let proprio_name = format!("/doso_gw_proprio_{suffix}");
        let cmd_name = format!("/doso_gw_cmd_{suffix}");
        let mut proprio_writer = doso_shm_ipc::ProprioRingWriter::open(&proprio_name).unwrap();
        let mut proprio = DosoProprioPayload {
            sequence: 11,
            monotonic_us: 2_500_000,
            imu_quat_wxyz: [0.97, 0.02, -0.01, 0.04],
            imu_gyro_rad_s: [0.11, -0.22, 0.33],
            imu_accel_m_s2: [0.0, 0.0, 9.81],
            imu_fresh: 1,
            ..Default::default()
        };
        for i in 0..16 {
            proprio.joint_q[i] = 0.04 * (i as f64);
            proprio.joint_dq[i] = -0.02 * (i as f64);
            proprio.joint_tau[i] = 0.1 * (i as f64);
            proprio.joint_status[i] = 2;
        }
        proprio_writer.publish(&proprio).unwrap();

        let (mb_url, mb_svc, mb_stop, mb_task) = spawn_mock_brainstem().await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (gw_stop, gw_rx) = oneshot::channel();
        let gw_cfg = BaseGatewayConfig {
            brainstem_endpoint: mb_url,
            shm_enabled: true,
            shm_proprio_name: proprio_name,
            shm_cmd_name: cmd_name.clone(),
            shm_poll_period: Duration::from_millis(2),
            ..BaseGatewayConfig::default()
        };
        let (addr, gw_task) =
            spawn_base_gateway_on_listener(BaseGatewayService::new(gw_cfg), listener, gw_rx)
                .await
                .unwrap();

        let robot = RobotClient::connect(format!("http://{addr}"))
            .await
            .unwrap();

        // 1. Verify IMU and Joints streams are served directly from SHM (not gRPC).
        let imu_sample = {
            let mut imu_stream = robot.observations().watch_imu().await.unwrap();
            tokio::time::timeout(Duration::from_secs(2), imu_stream.message())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        };
        assert!(
            (imu_sample.orientation_world_to_body.unwrap().w - 0.97).abs() < 1e-6,
            "expected IMU quaternion from SHM ring"
        );
        let joint_sample = {
            let mut joint_stream = robot.observations().watch_joints().await.unwrap();
            tokio::time::timeout(Duration::from_secs(2), joint_stream.message())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        };
        assert_eq!(joint_sample.joints.len(), 16);
        assert!(
            (joint_sample.joints[15].position_rad - 0.60).abs() < 1e-6,
            "expected joint positions from SHM ring"
        );

        // 2. Verify ExternalControl uses gRPC for Enter/Enable/Close while writing 200Hz MIT frames to SHM.
        let mut ext = robot.low_level(None).await.unwrap();
        assert!(ext.enable(None).await.unwrap().accepted);
        let cmds = vec![JointCommand::leg_pd(0.35, 50.0, 2.0); 16];
        ext.publish_joints(cmds, None).unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;

        let cmd_reader = doso_shm_ipc::ExternalCmdReader::open(&cmd_name).unwrap();
        let (_, written) = cmd_reader
            .read_latest(0)
            .expect("expected MIT command written into SHM ring");
        assert!(written.session_id_hash != 0);
        assert!(written.input_window > 0);
        assert!((written.q[0] - 0.35).abs() < 1e-6);
        assert!((written.kp[0] - 50.0).abs() < 1e-6);
        assert!((written.kd[0] - 2.0).abs() < 1e-6);
        // Zero gRPC ExternalFrame payloads sent on data plane!
        assert_eq!(mb_svc.ext_frames.lock().unwrap().len(), 0);

        assert!(ext.close(None).await.unwrap().accepted);
        let _ = gw_stop.send(());
        gw_task.await.unwrap().unwrap();
        let _ = mb_stop.send(());
        mb_task.await.unwrap();
    }
}
