use doso_sdk::{
    rpc, Code, Config, DosoClient, ErrorKind, MapState, Pose, ProductState, TaskState, Terminal,
};
#[path = "grpc/client.rs"]
mod client;
#[path = "grpc/control.rs"]
mod control;
#[path = "grpc/external.rs"]
mod external;
#[path = "grpc/inspection.rs"]
mod inspection;
#[path = "grpc/kinematics.rs"]
mod kinematics;
#[path = "grpc/location.rs"]
mod location;
#[path = "grpc/motors.rs"]
mod motors;
#[path = "grpc/navigation.rs"]
mod navigation;
#[path = "grpc/observation_metadata.rs"]
mod observation_metadata;
#[path = "grpc/posture.rs"]
mod posture;
#[path = "grpc/product.rs"]
mod product;
#[path = "grpc/profile.rs"]
mod profile;
#[path = "grpc/task_watch.rs"]
mod task_watch;
#[path = "grpc/teleop_cancel.rs"]
mod teleop_cancel;
#[path = "grpc/teleop_publish.rs"]
mod teleop_publish;
use prost::Message;
use prost_types::{value::Kind, Struct, Value};
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicU8, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_stream::{wrappers::TcpListenerStream, Stream, StreamExt};
use tonic::{Request, Response, Status};

#[path = "grpc/tls.rs"]
mod tls;

#[derive(Clone, Default)]
struct Service {
    mode: Arc<AtomicU8>,
    writes: Arc<AtomicUsize>,
    watches: Arc<AtomicUsize>,
}

impl Service {
    fn receipt(&self, command: rpc::Command) -> Result<Response<rpc::Receipt>, Status> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        if self.mode.load(Ordering::Relaxed) == 2 {
            return Err(Status::unavailable("owner acknowledgement unknown"));
        }
        if self.mode.load(Ordering::Relaxed) == 6 {
            let detail = rpc::ErrorDetail {
                reason_code: "safety_stop".into(),
                details: Some(evidence(&command.request_id)),
            };
            let mut status = Status::failed_precondition("safety stop is active");
            status.metadata_mut().insert_bin(
                "doso-error-bin",
                tonic::metadata::MetadataValue::from_bytes(&detail.encode_to_vec()),
            );
            return Err(status);
        }
        if self.mode.load(Ordering::Relaxed) == 7 {
            let mut status = Status::unavailable("owner acknowledgement unknown");
            status.metadata_mut().insert_bin(
                "doso-error-bin",
                tonic::metadata::MetadataValue::from_bytes(&[0xff]),
            );
            return Err(status);
        }
        let missing_identity = self.mode.load(Ordering::Relaxed) == 8;
        Ok(Response::new(rpc::Receipt {
            ok: !missing_identity,
            accepted: Some(true),
            request_id: if self.mode.load(Ordering::Relaxed) == 1 {
                "other".into()
            } else {
                command.request_id
            },
            task_id: (!missing_identity).then(|| "task-1".into()),
            operation_id: (!missing_identity).then(|| "map-1".into()),
            reason: if self.mode.load(Ordering::Relaxed) == 16 {
                "x".repeat(5 * 1024 * 1024)
            } else {
                String::new()
            },
            ..Default::default()
        }))
    }
    fn task_control_receipt(
        &self,
        request: rpc::TaskControl,
    ) -> Result<Response<rpc::Receipt>, Status> {
        assert_eq!(request.task_id, "task-1");
        assert!(!request.reason.is_empty());
        let mut response = self.receipt(request.command.unwrap())?;
        match self.mode.load(Ordering::Relaxed) {
            112 => response.get_mut().task_id = Some("other-task".into()),
            113 => response.get_mut().task_id = None,
            _ => {}
        }
        Ok(response)
    }
    fn task(&self, state: TaskState) -> rpc::Task {
        rpc::Task {
            task_id: if self.mode.load(Ordering::Relaxed) == 3 {
                "other".into()
            } else {
                "task-1".into()
            },
            found: true,
            state: if self.mode.load(Ordering::Relaxed) == 15 {
                999
            } else {
                state as i32
            },
            reason: "test evidence".into(),
            reason_code: "test_failure".into(),
            details: Some(evidence("task-1")),
        }
    }
}

#[tonic::async_trait]
impl rpc::system_server::System for Service {
    async fn handshake(
        &self,
        request: Request<rpc::Handshake>,
    ) -> Result<Response<rpc::HandshakeReply>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        let request = request.into_inner();
        assert_eq!(request.protocol_version, 2);
        assert_eq!(request.sdk_version, env!("CARGO_PKG_VERSION"));
        let mode = self.mode.load(Ordering::Relaxed);
        assert_eq!(
            request.client_id,
            if mode == 21 {
                "observer-test"
            } else {
                "doso-sdk"
            }
        );
        if mode == 18 {
            let detail = rpc::ErrorDetail {
                reason_code: "protocol_mismatch".into(),
                details: None,
            };
            let mut status = Status::failed_precondition("protocol is incompatible");
            status.metadata_mut().insert_bin(
                "doso-error-bin",
                tonic::metadata::MetadataValue::from_bytes(&detail.encode_to_vec()),
            );
            return Err(status);
        }
        if mode == 19 {
            return std::future::pending().await;
        }
        Ok(Response::new(rpc::HandshakeReply {
            protocol_version: if mode == 17 { 1 } else { 2 },
            info: (mode != 23).then(|| rpc::SystemInfo {
                firmware_version: (mode != 22).then(|| {
                    if mode == 120 {
                        "updated-firmware"
                    } else {
                        "test-firmware"
                    }
                    .into()
                }),
                robot_version: (mode != 22).then(|| "vendor/model".into()),
                robot_id: (mode != 22).then(|| {
                    if mode == 121 {
                        "2a0b75dc-a0d5-4dd5-a482-7f58fba4ab03"
                    } else {
                        "8f7e1c62-0a4d-4b89-96e2-6c47b03db1a5"
                    }
                    .into()
                }),
                low_level_endpoint: None,
            }),
        }))
    }
    async fn ping(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Ping>, Status> {
        Ok(Response::new(rpc::Ping {
            status: "ok".into(),
        }))
    }
    async fn state(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }
    async fn robot_status(
        &self,
        _: Request<rpc::Empty>,
    ) -> Result<Response<rpc::RobotStatus>, Status> {
        if self.mode.load(Ordering::Relaxed) == 100 {
            return Err(Status::unimplemented("typed status unavailable"));
        }
        Ok(Response::new(rpc::RobotStatus {
            mode: "manual".into(),
            ready: Some(false),
            software_emergency_stop: Some(false),
            ..Default::default()
        }))
    }
    async fn health(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }
    async fn faults(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<rpc::FaultReport>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        Ok(Response::new(control::faults()))
    }
    type WatchFaultsStream = Pin<Box<dyn Stream<Item = Result<rpc::FaultReport, Status>> + Send>>;
    async fn watch_faults(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<Self::WatchFaultsStream>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_none());
        Ok(Response::new(Box::pin(tokio_stream::iter([Ok(
            control::faults(),
        )]))))
    }
    async fn readiness(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot {
            fields: Some(Struct {
                fields: [
                    (
                        "status".into(),
                        Value {
                            kind: Some(Kind::StringValue("degraded".into())),
                        },
                    ),
                    (
                        "ready".into(),
                        Value {
                            kind: Some(Kind::BoolValue(false)),
                        },
                    ),
                    (
                        "data_ready".into(),
                        Value {
                            kind: Some(Kind::BoolValue(true)),
                        },
                    ),
                ]
                .into(),
            }),
        }))
    }
    async fn capabilities(
        &self,
        _: Request<rpc::Empty>,
    ) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }
    async fn session(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }
    async fn product_status(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<rpc::Snapshot>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        let mode = self.mode.load(Ordering::Relaxed);
        if mode == 140 {
            return Err(Status::unavailable("product service unavailable"));
        }
        Ok(Response::new(product::snapshot(mode)))
    }
}

#[tonic::async_trait]
impl rpc::localization_server::Localization for Service {
    async fn location(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<rpc::GeographicLocation>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_some());
        let mode = self.mode.load(Ordering::Relaxed);
        location::available(mode)?;
        Ok(Response::new(location::sample(if mode == 136 {
            rpc::GeographicState::Stale
        } else {
            rpc::GeographicState::Ready
        })))
    }

    type WatchLocationStream =
        Pin<Box<dyn Stream<Item = Result<rpc::GeographicLocation, Status>> + Send>>;
    async fn watch_location(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<Self::WatchLocationStream>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_none());
        location::available(self.mode.load(Ordering::Relaxed))?;
        Ok(Response::new(kinematics::track(
            Box::pin(
                tokio_stream::iter(
                    [
                        rpc::GeographicState::Ready,
                        rpc::GeographicState::NoFix,
                        rpc::GeographicState::Stale,
                        rpc::GeographicState::Ready,
                    ]
                    .map(|state| Ok(location::sample(state))),
                )
                .chain(tokio_stream::pending()),
            ),
            self.watches.clone(),
        )))
    }

    async fn state(
        &self,
        _: Request<rpc::Empty>,
    ) -> Result<Response<rpc::LocalizationState>, Status> {
        Ok(Response::new(rpc::LocalizationState {
            quality: rpc::LocalizationQuality::Lost as i32,
            source_state: "lost".into(),
            ready: Some(false),
            pose_fresh: Some(false),
            map_name: "lab".into(),
            reason: "localizer_health:lost".into(),
            ..Default::default()
        }))
    }
    async fn status(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }
    async fn pose(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::PoseSample>, Status> {
        Ok(Response::new(rpc::PoseSample {
            pose: Some(Pose {
                x: if self.mode.load(Ordering::Relaxed) == 110 {
                    f64::NAN
                } else {
                    1.0
                },
                y: 2.0,
                z: 3.0,
                yaw: 0.5,
            }),
            frame_id: "map".into(),
            details: None,
            received_age_ms: Some(10.0),
            fresh: Some(true),
        }))
    }
    type WatchPoseStream = Pin<Box<dyn Stream<Item = Result<rpc::PoseSample, Status>> + Send>>;
    async fn watch_pose(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<Self::WatchPoseStream>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_none());
        if self.mode.load(Ordering::Relaxed) == 70 {
            return Err(Status::unimplemented("pose stream unavailable"));
        }
        Ok(Response::new(Box::pin(tokio_stream::iter([
            Ok(self.pose(request).await?.into_inner()),
            Err(Status::unavailable("localization source unavailable")),
        ]))))
    }
    async fn relocalize(
        &self,
        request: Request<rpc::Relocalize>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        self.receipt(request.into_inner().command.unwrap())
    }
}

#[tonic::async_trait]
impl rpc::navigation_server::Navigation for Service {
    async fn go_with_options(
        &self,
        request: Request<rpc::Goal>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        if self.mode.load(Ordering::Relaxed) == 152 {
            return Err(Status::unimplemented(
                "goal options are not implemented by this server",
            ));
        }
        self.go(request).await
    }
    async fn go(&self, request: Request<rpc::Goal>) -> Result<Response<rpc::Receipt>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        let goal = request.into_inner();
        let pose = goal.pose.unwrap();
        assert_eq!((pose.x, pose.y, pose.z, pose.yaw), (1.0, 2.0, 3.0, 0.5));
        if self.mode.load(Ordering::Relaxed) == 150 {
            assert_eq!(goal.max_speed_mps, Some(0.8));
            assert_eq!(goal.acceptance_radius_m, Some(0.25));
        }
        self.receipt(goal.command.unwrap())
    }
    async fn preview_plan(
        &self,
        request: Request<rpc::PlanQuery>,
    ) -> Result<Response<rpc::PlanPreview>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        let request = request.into_inner();
        assert_eq!(request.acceptance_radius_m, Some(0.25));
        let pose = request.pose.unwrap();
        assert_eq!((pose.x, pose.y, pose.z), (1.0, 2.0, 3.0));
        Ok(Response::new(rpc::PlanPreview {
            ok: true,
            feasible: true,
            path: Some(rpc::NavigationPath {
                frame_id: "map".into(),
                points: vec![rpc::Vector3 {
                    x: pose.x,
                    y: pose.y,
                    z: pose.z,
                }],
                time: None,
            }),
            distance_m: Some(0.0),
            plan_ms: Some(2.5),
            planner: "native_global".into(),
            ..Default::default()
        }))
    }
    async fn set_speed_limit(
        &self,
        request: Request<rpc::TaskSpeedLimit>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        let request = request.into_inner();
        assert_eq!(request.task_id, "task-1");
        assert_eq!(request.max_speed_mps, 0.7);
        let mut response = self.receipt(request.command.unwrap())?;
        if self.mode.load(Ordering::Relaxed) == 151 {
            response.get_mut().task_id = Some("other-task".into());
        }
        Ok(response)
    }
    async fn status(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }
    async fn path(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }
    async fn route(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<rpc::NavigationPath>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        if self.mode.load(Ordering::Relaxed) == 71 {
            return Err(Status::unavailable("route unavailable"));
        }
        Ok(Response::new(rpc::NavigationPath {
            frame_id: "map".into(),
            points: vec![rpc::Vector3 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            }],
            time: None,
        }))
    }
    type WatchPathStream = Pin<Box<dyn Stream<Item = Result<rpc::NavigationPath, Status>> + Send>>;
    async fn watch_path(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<Self::WatchPathStream>, Status> {
        assert!(request.metadata().get("grpc-timeout").is_none());
        let route = self.route(request).await?.into_inner();
        Ok(Response::new(Box::pin(tokio_stream::iter([
            Ok(route),
            Ok(rpc::NavigationPath {
                frame_id: "map".into(),
                ..Default::default()
            }),
        ]))))
    }
    async fn task(&self, _: Request<rpc::TaskQuery>) -> Result<Response<rpc::Task>, Status> {
        let state = match self.mode.load(Ordering::Relaxed) {
            5 => TaskState::TaskFailed,
            9 => TaskState::TaskCancelled,
            14 => TaskState::TaskUnknown,
            _ => TaskState::TaskSuccess,
        };
        Ok(Response::new(self.task(state)))
    }
    async fn cancel(
        &self,
        request: Request<rpc::CancelTask>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        let request = request.into_inner();
        assert_eq!(request.task_id, "task-1");
        self.receipt(request.command.unwrap())
    }
    async fn pause(
        &self,
        request: Request<rpc::TaskControl>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        self.task_control_receipt(request.into_inner())
    }
    async fn resume(
        &self,
        request: Request<rpc::TaskControl>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        self.task_control_receipt(request.into_inner())
    }
    type WatchStream = Pin<Box<dyn Stream<Item = Result<rpc::Task, Status>> + Send>>;
    async fn watch(
        &self,
        request: Request<rpc::TaskQuery>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        assert_eq!(request.into_inner().task_id, "task-1");
        self.watches.fetch_add(1, Ordering::Relaxed);
        if self.mode.load(Ordering::Relaxed) == 4 {
            return Ok(Response::new(Box::pin(tokio_stream::pending())));
        }
        let end = match self.mode.load(Ordering::Relaxed) {
            5 => TaskState::TaskFailed,
            9 => TaskState::TaskCancelled,
            _ => TaskState::TaskSuccess,
        };
        Ok(Response::new(Box::pin(tokio_stream::iter([
            Ok(self.task(if self.mode.load(Ordering::Relaxed) == 14 {
                TaskState::TaskUnknown
            } else {
                TaskState::TaskPaused
            })),
            Ok(self.task(end)),
        ]))))
    }
}

#[tonic::async_trait]
impl rpc::motion_server::Motion for Service {
    async fn controller(
        &self,
        _: Request<rpc::Empty>,
    ) -> Result<Response<rpc::ControllerState>, Status> {
        Ok(Response::new(external::state(false, 0)))
    }
    type ExternalControlStream =
        Pin<Box<dyn Stream<Item = Result<rpc::ExternalControlReply, Status>> + Send>>;
    async fn external_control(
        &self,
        request: Request<tonic::Streaming<rpc::ExternalControlRequest>>,
    ) -> Result<Response<Self::ExternalControlStream>, Status> {
        external::serve(self.clone(), request).await
    }
    async fn velocity(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<rpc::VelocitySample>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_some());
        let mode = self.mode.load(Ordering::Relaxed);
        kinematics::source_available(mode)?;
        Ok(Response::new(kinematics::velocity(mode)))
    }

    type WatchVelocityStream =
        Pin<Box<dyn Stream<Item = Result<rpc::VelocitySample, Status>> + Send>>;
    async fn watch_velocity(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<Self::WatchVelocityStream>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_none());
        kinematics::source_available(self.mode.load(Ordering::Relaxed))?;
        Ok(Response::new(kinematics::track(
            Box::pin(
                tokio_stream::iter([Ok(kinematics::velocity(0))]).chain(tokio_stream::pending()),
            ),
            self.watches.clone(),
        )))
    }

    async fn odometry(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<rpc::OdometrySample>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_some());
        let mode = self.mode.load(Ordering::Relaxed);
        kinematics::source_available(mode)?;
        Ok(Response::new(kinematics::odometry(mode)))
    }

    type WatchOdometryStream =
        Pin<Box<dyn Stream<Item = Result<rpc::OdometrySample, Status>> + Send>>;
    async fn watch_odometry(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<Self::WatchOdometryStream>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_none());
        kinematics::source_available(self.mode.load(Ordering::Relaxed))?;
        Ok(Response::new(kinematics::track(
            Box::pin(
                tokio_stream::iter([
                    Ok(kinematics::odometry(0)),
                    Err(Status::unavailable("SLAM odometry stream lost")),
                ])
                .chain(tokio_stream::pending()),
            ),
            self.watches.clone(),
        )))
    }

    async fn stand_up(
        &self,
        request: Request<rpc::Command>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        let mut response = self.receipt(request.into_inner())?;
        response.get_mut().stage = "posture_requested".into();
        Ok(response)
    }
    async fn sit_down(
        &self,
        request: Request<rpc::Command>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        self.stand_up(request).await
    }
    async fn damp(&self, request: Request<rpc::Command>) -> Result<Response<rpc::Receipt>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        let mut response = self.receipt(request.into_inner())?;
        response.get_mut().stage = "damping_requested".into();
        Ok(response)
    }
    async fn clear_motor_fault(
        &self,
        request: Request<rpc::ClearMotorFaultRequest>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        let mut response = self.receipt(request.into_inner().command.unwrap())?;
        response.get_mut().stage = "fault_cleared".into();
        Ok(response)
    }
    async fn list_gestures(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<rpc::GestureList>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        Ok(Response::new(rpc::GestureList {
            gestures: vec![rpc::GestureInfo {
                name: "bow".into(),
                description: "Bow".into(),
                duration_ms: 1800,
            }],
        }))
    }
    async fn play_gesture(
        &self,
        request: Request<rpc::PlayGestureRequest>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        let inner = request.into_inner();
        assert!(!inner.name.trim().is_empty());
        let mut response = self.receipt(inner.command.unwrap())?;
        response.get_mut().stage = format!("gesture:{}", inner.name);
        Ok(response)
    }
    async fn enable_motors(
        &self,
        request: Request<rpc::Command>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_some());
        let mut response = self.receipt(request.into_inner())?;
        response.get_mut().stage = "motor_power_acknowledged".into();
        Ok(response)
    }
    async fn disable_motors(
        &self,
        request: Request<rpc::Command>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        self.enable_motors(request).await
    }
    async fn profile(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<rpc::ProfileInfo>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_some());
        if self.mode.load(Ordering::Relaxed) == 130 {
            return Err(Status::unimplemented("profile is unsupported"));
        }
        Ok(Response::new(profile::info()))
    }
    async fn switch_profile(
        &self,
        request: Request<rpc::SwitchProfileRequest>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_some());
        let request = request.into_inner();
        assert_eq!(request.name, "stairs_up");
        let mut response = self.receipt(request.command.unwrap())?;
        response.get_mut().stage = "profile_switch_acknowledged".into();
        Ok(response)
    }
    type TeleopStream = Pin<Box<dyn Stream<Item = Result<rpc::TeleopReply, Status>> + Send>>;
    async fn teleop(
        &self,
        request: Request<tonic::Streaming<rpc::TeleopRequest>>,
    ) -> Result<Response<Self::TeleopStream>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_none());
        let mut requests = request.into_inner();
        let service = self.clone();
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            let mut window = 0;
            while let Ok(Some(request)) = requests.message().await {
                let mode = service.mode.load(Ordering::Relaxed);
                let command = request.command.unwrap();
                assert_eq!(command.client_id, "doso-sdk");
                let mut reply = rpc::TeleopReply {
                    request_id: command.request_id,
                    sequence: request.sequence,
                    accepted: true,
                    reason: "accepted".into(),
                    control: Some(control::state(false)),
                    ..Default::default()
                };
                let close = matches!(request.action, Some(rpc::teleop_request::Action::Close(_)));
                match request.action.unwrap() {
                    rpc::teleop_request::Action::Claim(_) => {
                        if mode == 127 {
                            let _ = sender
                                .send(Err(Status::unavailable("control claim ACK unknown")))
                                .await;
                            break;
                        }
                        reply.accepted = mode != 126;
                        reply.control = Some(control::state(reply.accepted));
                        if !reply.accepted {
                            reply.reason = "authority_in_use".into();
                        }
                    }
                    rpc::teleop_request::Action::Input(_) => {
                        window += 1;
                        reply.input_window = window;
                        if mode == 125 {
                            reply.sequence = 0;
                        }
                    }
                    rpc::teleop_request::Action::Velocity(velocity) => {
                        assert_eq!(request.input_window, window);
                        assert!(velocity.deadman);
                        service.writes.fetch_add(1, Ordering::Relaxed);
                        reply.ingress_accepted = true;
                        if mode == 123 {
                            continue;
                        }
                        if mode == 120 {
                            reply.request_id = "wrong-request".into();
                        }
                        if mode == 121 {
                            let _ = requests.message().await;
                            break;
                        }
                    }
                    rpc::teleop_request::Action::Hold(_)
                    | rpc::teleop_request::Action::Close(_) => {
                        if !close {
                            window += 1;
                            reply.input_window = window;
                        }
                        reply.accepted = mode != 122;
                        reply.final_output_confirmed = reply.accepted;
                        if !reply.accepted {
                            reply.reason = "stop_unconfirmed".into();
                        }
                    }
                }
                if sender.send(Ok(reply)).await.is_err() || close {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(receiver),
        )))
    }
    async fn stop(&self, request: Request<rpc::Command>) -> Result<Response<rpc::Receipt>, Status> {
        self.receipt(request.into_inner())
    }
    async fn stop_motion(
        &self,
        request: Request<rpc::Command>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        let mut response = self.receipt(request.into_inner())?;
        response.get_mut().stage = "motion_stop_acknowledged".into();
        Ok(response)
    }
    async fn clear_emergency_stop(
        &self,
        request: Request<rpc::Command>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        let mut response = self.receipt(request.into_inner())?;
        response.get_mut().stage = "estop_reset_acknowledged".into();
        Ok(response)
    }
    async fn status(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }
    async fn control_status(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<rpc::BodyControlStatus>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_some());
        match self.mode.load(Ordering::Relaxed) {
            95 => Err(Status::unimplemented("control status unavailable")),
            96 => Err(Status::permission_denied("control observation denied")),
            _ => Ok(Response::new(rpc::BodyControlStatus {
                body: Some(rpc::BodyState {
                    kind: 999,
                    transition: 777,
                    gesture_name: "future".into(),
                }),
                motor_output_enabled: true,
                source_owner: 2,
                critical_motor_fault: true,
                grpc_lease_active: false,
                lease_remaining_ms: 0,
                last_accepted_sequence: 17,
                source_last_decision: 9,
                source_ready_for_walk: false,
                source_owner_id: "physical-remote".into(),
                accepted_count: 21,
                rejected_count: 3,
            })),
        }
    }
    async fn resume(
        &self,
        request: Request<rpc::Command>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        let mut response = self.receipt(request.into_inner())?;
        response.get_mut().stage = "native_acknowledged".into();
        Ok(response)
    }
    async fn control_state(
        &self,
        _: Request<rpc::Empty>,
    ) -> Result<Response<rpc::ControlState>, Status> {
        Ok(Response::new(control::state(false)))
    }
    type WatchControlStateStream =
        Pin<Box<dyn Stream<Item = Result<rpc::ControlState, Status>> + Send>>;
    async fn watch_control_state(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<Self::WatchControlStateStream>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_none());
        Ok(Response::new(Box::pin(tokio_stream::iter([Ok(
            control::state(false),
        )]))))
    }
    async fn body_state(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<rpc::BodyState>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_some());
        match self.mode.load(Ordering::Relaxed) {
            30 => Err(Status::unimplemented(
                "body state unavailable on this Gateway",
            )),
            32 => Err(Status::unavailable("body source disconnected")),
            33 => Ok(Response::new(rpc::BodyState {
                kind: 91,
                transition: 92,
                gesture_name: "future".into(),
            })),
            _ => Ok(Response::new(rpc::BodyState {
                kind: 4,
                transition: 3,
                gesture_name: "bow".into(),
            })),
        }
    }
    type WatchBodyStateStream = Pin<Box<dyn Stream<Item = Result<rpc::BodyState, Status>> + Send>>;
    async fn watch_body_state(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<Self::WatchBodyStateStream>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_none());
        match self.mode.load(Ordering::Relaxed) {
            30 => {
                return Err(Status::unimplemented(
                    "body state unavailable on this Gateway",
                ))
            }
            32 => return Err(Status::unavailable("body source disconnected")),
            _ => {}
        }
        let states = (0..=4).map(|kind| {
            Ok(rpc::BodyState {
                kind,
                transition: if kind == 4 { 1 } else { 0 },
                gesture_name: String::new(),
            })
        });
        let final_event = if self.mode.load(Ordering::Relaxed) == 31 {
            Err(Status::unavailable("body source disconnected"))
        } else {
            Ok(rpc::BodyState {
                kind: 91,
                transition: 92,
                gesture_name: "future".into(),
            })
        };
        Ok(Response::new(Box::pin(tokio_stream::iter(
            states.chain([final_event]),
        ))))
    }
    type WatchStateStream = Pin<Box<dyn Stream<Item = Result<rpc::Snapshot, Status>> + Send>>;
    async fn watch_state(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<Self::WatchStateStream>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_none());
        if self.mode.load(Ordering::Relaxed) == 24 {
            return Err(Status::resource_exhausted("observation capacity reached"));
        }
        let snapshots = tokio_stream::iter([
            Ok(rpc::Snapshot {
                fields: Some(evidence("state-1")),
            }),
            if self.mode.load(Ordering::Relaxed) == 25 {
                Err(Status::unavailable("state source unavailable"))
            } else {
                Ok(rpc::Snapshot {
                    fields: Some(evidence("state-2")),
                })
            },
        ]);
        Ok(Response::new(Box::pin(
            snapshots.chain(tokio_stream::pending()),
        )))
    }
    type WatchStateAtStream = Pin<Box<dyn Stream<Item = Result<rpc::Snapshot, Status>> + Send>>;
    async fn watch_state_at(
        &self,
        request: Request<rpc::WatchStateRequest>,
    ) -> Result<Response<Self::WatchStateAtStream>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_none());
        if self.mode.load(Ordering::Relaxed) == 26 {
            return Err(Status::unimplemented("frequency selection unavailable"));
        }
        let rate = request.into_inner().frequency_hz;
        if !(1..=100).contains(&rate) {
            return Err(Status::invalid_argument("frequency_hz must be 1 to 100"));
        }
        Ok(Response::new(Box::pin(tokio_stream::iter([Ok(
            rpc::Snapshot {
                fields: Some(evidence(&rate.to_string())),
            },
        )]))))
    }
}

#[tonic::async_trait]
impl rpc::observations_server::Observations for Service {
    async fn camera_calibration(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<rpc::CameraCalibration>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert_eq!(
            request.metadata().get("authorization").unwrap(),
            "Bearer viewer"
        );
        assert!(request.metadata().get("grpc-timeout").is_some());
        match self.mode.load(Ordering::Relaxed) {
            140 => {
                return Err(Status::unavailable(
                    "Source camera calibration is unavailable",
                ))
            }
            141 => return Err(Status::unimplemented("old server")),
            142 => return Err(Status::permission_denied("media_authorization_denied")),
            _ => {}
        }
        Ok(Response::new(rpc::CameraCalibration {
            width_px: 640,
            height_px: 480,
            fx_px: 410.0,
            fy_px: 411.0,
            cx_px: 319.5,
            cy_px: 239.5,
            frame_id: "source_color".into(),
            distortion_model: "plumb_bob".into(),
            distortion: vec![0.1, 0.2, 0.003, 0.004, 0.5],
        }))
    }

    type WatchPointCloudStream =
        Pin<Box<dyn Stream<Item = Result<rpc::CloudFrame, Status>> + Send>>;

    async fn watch_point_cloud(
        &self,
        request: Request<rpc::CloudRequest>,
    ) -> Result<Response<Self::WatchPointCloudStream>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_none());
        assert!(matches!(request.into_inner().kind, 1 | 2));
        let mode = self.mode.load(Ordering::Relaxed);
        if mode == 46 {
            return Err(Status::unimplemented("point-cloud service unavailable"));
        }
        let mut frame = include_bytes!("fixtures/cloud-v2.bin").to_vec();
        if mode == 45 {
            frame[5] = 0;
            frame[8..12].copy_from_slice(&1_000_000_u32.to_le_bytes());
            frame.truncate(52);
            frame.resize(52 + 1_000_000 * 6, 0);
        }
        if mode == 48 {
            let (send, receive) = tokio::sync::mpsc::channel(1);
            send.try_send(Ok(rpc::CloudFrame { pcld: frame })).unwrap();
            let watches = self.watches.clone();
            tokio::spawn(async move {
                send.closed().await;
                watches.fetch_add(1, Ordering::Relaxed);
            });
            return Ok(Response::new(Box::pin(
                tokio_stream::wrappers::ReceiverStream::new(receive),
            )));
        }
        let next = if mode == 47 {
            Err(Status::unavailable("point-cloud source disconnected"))
        } else {
            Ok(rpc::CloudFrame {
                pcld: include_bytes!("fixtures/cloud-reset.bin").to_vec(),
            })
        };
        Ok(Response::new(Box::pin(tokio_stream::iter([
            Ok(rpc::CloudFrame { pcld: frame }),
            next,
        ]))))
    }

    async fn scene(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }

    async fn camera_status(
        &self,
        _: Request<rpc::Empty>,
    ) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }

    async fn camera_frame(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<rpc::CameraFrame>, Status> {
        assert_eq!(
            request.metadata().get("authorization").unwrap(),
            "Bearer viewer"
        );
        assert!(request.metadata().get("grpc-timeout").is_some());
        if self.mode.load(Ordering::Relaxed) == 40 {
            return Err(Status::permission_denied("media_authorization_denied"));
        }
        Ok(Response::new(rpc::CameraFrame {
            jpeg: vec![1, 2, 3],
        }))
    }

    type WatchCameraStream = Pin<Box<dyn Stream<Item = Result<rpc::CameraFrame, Status>> + Send>>;
    async fn watch_camera(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<Self::WatchCameraStream>, Status> {
        assert_eq!(
            request.metadata().get("authorization").unwrap(),
            "Bearer viewer"
        );
        assert!(request.metadata().get("grpc-timeout").is_none());
        if self.mode.load(Ordering::Relaxed) == 40 {
            return Err(Status::permission_denied("media_authorization_denied"));
        }
        let next = if self.mode.load(Ordering::Relaxed) == 41 {
            Err(Status::unavailable("camera stalled"))
        } else {
            Ok(rpc::CameraFrame { jpeg: vec![4, 5] })
        };
        Ok(Response::new(Box::pin(tokio_stream::iter([
            Ok(rpc::CameraFrame {
                jpeg: vec![1, 2, 3],
            }),
            next,
        ]))))
    }

    async fn camera_sample(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<rpc::CameraSample>, Status> {
        let frame = self.camera_frame(request).await?.into_inner();
        Ok(Response::new(rpc::CameraSample {
            frame: Some(frame),
            info: Some(rpc::CameraInfo {
                encoding: "jpeg".into(),
                ..Default::default()
            }),
            ..Default::default()
        }))
    }

    type WatchCameraSamplesStream =
        Pin<Box<dyn Stream<Item = Result<rpc::CameraSample, Status>> + Send>>;
    async fn watch_camera_samples(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<Self::WatchCameraSamplesStream>, Status> {
        let frames = self.watch_camera(request).await?.into_inner();
        let mut sequence = 0;
        Ok(Response::new(Box::pin(frames.map(move |frame| {
            sequence += 1;
            frame.map(|frame| rpc::CameraSample {
                frame: Some(frame),
                info: Some(rpc::CameraInfo {
                    encoding: "jpeg".into(),
                    ..Default::default()
                }),
                sequence: Some(sequence),
                ..Default::default()
            })
        }))))
    }

    async fn motors(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<rpc::MotorStates>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_some());
        if self.mode.load(Ordering::Relaxed) == 29 {
            return Err(Status::unavailable("motor source unavailable"));
        }
        Ok(Response::new(rpc::MotorStates {
            motors: vec![
                rpc::MotorState {
                    id: 3,
                    online: true,
                    status_code: 7,
                    temperature_c: 42.5,
                    errors: vec![1, 9],
                },
                rpc::MotorState {
                    id: 12,
                    ..Default::default()
                },
            ],
        }))
    }

    async fn motor_voltages(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<rpc::MotorVoltagesResponse>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_some());
        if self.mode.load(Ordering::Relaxed) == 29 {
            return Err(Status::failed_precondition(
                "voltage requires hardware mode",
            ));
        }
        Ok(Response::new(rpc::MotorVoltagesResponse {
            volts: vec![48.25, 0.0, 47.75],
        }))
    }

    type WatchImuStream = Pin<Box<dyn Stream<Item = Result<rpc::ImuSample, Status>> + Send>>;
    async fn watch_imu(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<Self::WatchImuStream>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_none());
        if self.mode.load(Ordering::Relaxed) == 27 {
            return Err(Status::unavailable("IMU source unavailable"));
        }
        let sample = rpc::ImuSample {
            angular_velocity_rad_s: Some(rpc::Vector3 {
                x: 1.0,
                y: -2.0,
                z: 3.5,
            }),
            orientation_world_to_body: Some(rpc::Quaternion {
                w: 1.0,
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }),
            linear_acceleration_m_s2: None,
            source_elapsed: Some(prost_types::Duration {
                seconds: 12,
                nanos: 345,
            }),
        };
        let next = if self.mode.load(Ordering::Relaxed) == 28 {
            Err(Status::unavailable("IMU source disconnected"))
        } else {
            Ok(rpc::ImuSample {
                linear_acceleration_m_s2: Some(rpc::Vector3 {
                    x: 0.5,
                    y: -1.0,
                    z: 9.75,
                }),
                ..sample
            })
        };
        Ok(Response::new(Box::pin(
            tokio_stream::iter([Ok(sample), next]).chain(tokio_stream::pending()),
        )))
    }

    type WatchJointsStream = Pin<Box<dyn Stream<Item = Result<rpc::JointSample, Status>> + Send>>;
    async fn watch_joints(
        &self,
        request: Request<rpc::Empty>,
    ) -> Result<Response<Self::WatchJointsStream>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_none());
        if self.mode.load(Ordering::Relaxed) == 27 {
            return Err(Status::unavailable("joint source unavailable"));
        }
        let sample = rpc::JointSample {
            joints: vec![rpc::JointState {
                id: 12,
                position_rad: -0.5,
                velocity_rad_s: 0.75,
                torque_nm: -1.25,
                status_code: 7,
            }],
            source_elapsed: Some(prost_types::Duration {
                seconds: 20,
                nanos: 999,
            }),
        };
        let next = if self.mode.load(Ordering::Relaxed) == 28 {
            Err(Status::unavailable("joint source disconnected"))
        } else {
            Ok(rpc::JointSample {
                joints: (0..16)
                    .map(|id| rpc::JointState {
                        id,
                        position_rad: f64::from(id) + 0.1,
                        ..Default::default()
                    })
                    .collect(),
                source_elapsed: None,
            })
        };
        Ok(Response::new(Box::pin(
            tokio_stream::iter([Ok(sample), next]).chain(tokio_stream::pending()),
        )))
    }
}

#[tonic::async_trait]
impl rpc::maps_server::Maps for Service {
    async fn grid(
        &self,
        request: Request<rpc::MapContentRequest>,
    ) -> Result<Response<rpc::MapGrid>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert_eq!(request.into_inner().map_name, "warehouse");
        let mode = self.mode.load(Ordering::Relaxed);
        if mode == 90 {
            return Err(Status::not_found("saved grid missing"));
        }
        let mut grid = rpc::MapGrid {
            map_id: if mode == 92 {
                "other".into()
            } else {
                "warehouse".into()
            },
            content_epoch: Some(17),
            frame_id: "map".into(),
            width: 2500,
            height: 2000,
            resolution_m: 0.2,
            origin: Some(Pose {
                x: -3.0,
                y: 4.0,
                ..Default::default()
            }),
            cells: vec![255; 5_000_000],
            time: None,
        };
        if mode == 91 {
            grid.cells.pop();
        }
        if mode == 93 {
            grid.origin.as_mut().unwrap().x = f64::NAN;
        }
        if mode == 94 {
            grid.cells[0] = 101;
        }
        Ok(Response::new(grid))
    }

    type PointCloudStream = Pin<Box<dyn Stream<Item = Result<rpc::MapContent, Status>> + Send>>;

    async fn point_cloud(
        &self,
        request: Request<rpc::MapContentRequest>,
    ) -> Result<Response<Self::PointCloudStream>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_none());
        assert_eq!(request.into_inner().map_name, "warehouse");
        let mode = self.mode.load(Ordering::Relaxed);
        if mode == 42 {
            return Err(Status::not_found("map point cloud missing"));
        }
        let mut messages = vec![Ok(rpc::MapContent {
            value: Some(rpc::map_content::Value::Info(rpc::MapContentInfo {
                map_id: "warehouse".into(),
                content_epoch: (mode != 44).then_some(7),
                frame_id: (mode != 44).then(|| "map".into()),
                size_bytes: 5 * 1024 * 1024,
            })),
        })];
        if mode == 43 {
            messages.push(Err(Status::unavailable("map read interrupted")));
        } else {
            for index in 0..5 {
                messages.push(Ok(rpc::MapContent {
                    value: Some(rpc::map_content::Value::Chunk(vec![index; 1024 * 1024])),
                }));
            }
        }
        Ok(Response::new(Box::pin(tokio_stream::iter(messages))))
    }

    async fn list(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::MapList>, Status> {
        Ok(Response::new(rpc::MapList::default()))
    }
    async fn save(&self, request: Request<rpc::SaveMap>) -> Result<Response<rpc::Receipt>, Status> {
        self.receipt(request.into_inner().command.unwrap())
    }
    async fn operation(
        &self,
        request: Request<rpc::MapQuery>,
    ) -> Result<Response<rpc::MapOperation>, Status> {
        let state = match self.mode.load(Ordering::Relaxed) {
            10 => MapState::MapFailed,
            11 => MapState::MapCancelled,
            14 => MapState::MapUnknown,
            _ => MapState::MapSucceeded,
        };
        let operation_id = request.into_inner().operation_id;
        Ok(Response::new(rpc::MapOperation {
            state: if self.mode.load(Ordering::Relaxed) == 15 {
                999
            } else {
                state as i32
            },
            reason: "map evidence".into(),
            reason_code: "map_write_failed".into(),
            details: Some(evidence(&operation_id)),
            operation_id,
        }))
    }
    async fn switch(
        &self,
        request: Request<rpc::SwitchProduct>,
    ) -> Result<Response<rpc::ProductOperation>, Status> {
        self.product_operation(Request::new(rpc::ProductQuery {
            request_id: request.into_inner().command.unwrap().request_id,
        }))
        .await
    }
    async fn product_operation(
        &self,
        request: Request<rpc::ProductQuery>,
    ) -> Result<Response<rpc::ProductOperation>, Status> {
        let state = match self.mode.load(Ordering::Relaxed) {
            12 => ProductState::ProductFailed,
            13 => ProductState::ProductInterrupted,
            14 => ProductState::Unknown,
            _ => ProductState::ProductSucceeded,
        };
        let request_id = request.into_inner().request_id;
        Ok(Response::new(rpc::ProductOperation {
            state: if self.mode.load(Ordering::Relaxed) == 15 {
                999
            } else {
                state as i32
            },
            active: state == ProductState::ProductSucceeded,
            reason: "product evidence".into(),
            reason_code: "switch_failed".into(),
            details: Some(evidence(&request_id)),
            request_id,
        }))
    }
}

#[tonic::async_trait]
impl rpc::tasks_server::Tasks for Service {
    type WatchStream = Pin<Box<dyn Stream<Item = Result<rpc::Run, Status>> + Send>>;

    async fn inspection_routes(
        &self,
        request: Request<rpc::InspectionRouteQuery>,
    ) -> Result<Response<rpc::InspectionRoutes>, Status> {
        let request = request.into_inner();
        Ok(Response::new(rpc::InspectionRoutes {
            map_id: request.map_id.clone(),
            routes: vec![inspection::route(self, &request.map_id, "route-1")],
        }))
    }

    async fn inspection_route(
        &self,
        request: Request<rpc::InspectionRouteQuery>,
    ) -> Result<Response<rpc::InspectionRoute>, Status> {
        let request = request.into_inner();
        Ok(Response::new(inspection::route(
            self,
            &request.map_id,
            &request.route_id,
        )))
    }

    async fn put_inspection_route(
        &self,
        request: Request<rpc::PutInspectionRoute>,
    ) -> Result<Response<rpc::InspectionRoute>, Status> {
        let request = request.into_inner();
        assert!(request.route.is_some());
        let mut route = inspection::route(self, "map-1", "route-1");
        route.request_id = self
            .receipt(request.command.unwrap())?
            .into_inner()
            .request_id;
        Ok(Response::new(route))
    }

    async fn delete_inspection_route(
        &self,
        request: Request<rpc::DeleteInspectionRoute>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        let request = request.into_inner();
        assert_eq!(request.map_id, "map-1");
        assert_eq!(request.route_id, "route-1");
        self.receipt(request.command.unwrap())
    }

    async fn start_inspection(
        &self,
        request: Request<rpc::StartInspection>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        let request = request.into_inner();
        assert_eq!(
            (
                request.map_id.as_str(),
                request.route_id.as_str(),
                request.revision
            ),
            ("map-1", "route-1", 7)
        );
        assert_eq!(request.binding.unwrap().product_session_id, "session-1");
        if self.mode.load(Ordering::Relaxed) == 163 {
            return Err(Status::unavailable("inspection owner unavailable"));
        }
        let mut reply = self.receipt(request.command.unwrap())?;
        if self.mode.load(Ordering::Relaxed) == 162 {
            reply.get_mut().task_id = None;
        }
        Ok(reply)
    }

    async fn inspection_task(
        &self,
        request: Request<rpc::InspectionTaskQuery>,
    ) -> Result<Response<rpc::InspectionTask>, Status> {
        Ok(Response::new(inspection::task(
            self,
            &request.into_inner().task_id,
            false,
        )))
    }

    async fn inspection_tasks(
        &self,
        request: Request<rpc::InspectionTaskListQuery>,
    ) -> Result<Response<rpc::InspectionTasks>, Status> {
        let request = request.into_inner();
        assert_eq!(request.map_id.as_deref(), Some("map-1"));
        assert_eq!(request.route_id.as_deref(), Some("route-1"));
        assert_eq!(request.limit, None);
        let mut tasks = vec![inspection::task(self, "task-1", false)];
        if request.include_terminal {
            tasks.push(inspection::task(self, "task-2", true));
        }
        Ok(Response::new(rpc::InspectionTasks {
            count: tasks.len() as u32,
            tasks,
            retention: "process_local_gateway_projection".into(),
            observed_at_unix_s: 1234.0,
        }))
    }

    type WatchInspectionStream =
        Pin<Box<dyn Stream<Item = Result<rpc::InspectionTask, Status>> + Send>>;

    async fn watch_inspection(
        &self,
        request: Request<rpc::InspectionTaskQuery>,
    ) -> Result<Response<Self::WatchInspectionStream>, Status> {
        assert!(request.metadata().get("grpc-timeout").is_none());
        let task_id = request.into_inner().task_id;
        Ok(Response::new(Box::pin(tokio_stream::iter([
            Ok(inspection::task(self, &task_id, false)),
            Ok(inspection::task(self, &task_id, true)),
        ]))))
    }

    async fn pause_inspection(
        &self,
        request: Request<rpc::InspectionControl>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        inspection::control(self, request.into_inner())
    }

    async fn resume_inspection(
        &self,
        request: Request<rpc::InspectionControl>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        inspection::control(self, request.into_inner())
    }

    async fn cancel_inspection(
        &self,
        request: Request<rpc::InspectionControl>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        inspection::control(self, request.into_inner())
    }

    async fn inspection_report(
        &self,
        request: Request<rpc::InspectionTaskQuery>,
    ) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot {
            fields: Some(evidence(&request.into_inner().task_id)),
        }))
    }

    async fn watch(
        &self,
        request: Request<rpc::RunQuery>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        assert!(request.metadata().get("grpc-timeout").is_none());
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        if self.mode.load(Ordering::Relaxed) == 81 {
            return Err(Status::unavailable("task owner unavailable"));
        }
        let found = self.mode.load(Ordering::Relaxed) != 80;
        let id = request.into_inner().run_id;
        let updates = if found {
            vec!["running", "done"]
        } else {
            vec![""]
        };
        let stream = tokio_stream::iter(updates.into_iter().map(move |state| {
            Ok(rpc::Run {
                run_id: id.clone(),
                found,
                details: Some(Struct {
                    fields: [(
                        "state".into(),
                        Value {
                            kind: Some(Kind::StringValue(state.into())),
                        },
                    )]
                    .into(),
                }),
            })
        }));
        Ok(Response::new(Box::pin(stream)))
    }
    async fn send(&self, _: Request<rpc::Instruction>) -> Result<Response<rpc::Receipt>, Status> {
        Err(Status::unimplemented("task submission fixture unavailable"))
    }

    async fn get(&self, request: Request<rpc::RunQuery>) -> Result<Response<rpc::Run>, Status> {
        assert_eq!(
            request.metadata().get("x-api-key").unwrap(),
            "test-only-key"
        );
        assert!(request.metadata().get("grpc-timeout").is_some());
        let mode = self.mode.load(Ordering::Relaxed);
        if mode == 81 {
            return Err(Status::unavailable("task owner unavailable"));
        }
        let run_id = request.into_inner().run_id;
        let details = Struct {
            fields: [(
                "state".into(),
                Value {
                    kind: Some(Kind::StringValue("running".into())),
                },
            )]
            .into(),
        };
        Ok(Response::new(rpc::Run {
            run_id: if mode == 82 {
                "another-run".into()
            } else {
                run_id
            },
            found: mode != 80,
            details: Some(details),
        }))
    }

    async fn cancel(
        &self,
        request: Request<rpc::CancelRun>,
    ) -> Result<Response<rpc::Receipt>, Status> {
        let request = request.into_inner();
        let mut response = self.receipt(request.command.unwrap())?;
        response.get_mut().run_id = Some(if self.mode.load(Ordering::Relaxed) == 101 {
            "wrong-run".into()
        } else {
            request.run_id
        });
        Ok(response)
    }
}

struct Fixture {
    endpoint: String,
    client: DosoClient,
    service: Service,
    shutdown: Option<oneshot::Sender<()>>,
}

#[tokio::test]
async fn typed_run_state_preserves_query_results_and_failures() {
    let fixture = Fixture::new().await;
    let state = fixture
        .client
        .tasks()
        .get_run_state("run-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.run_id, "run-1");
    assert_eq!(state.status, rpc::TaskRunStatus::Running as i32);
    assert!(state.time.is_none());
    assert_eq!(state.progress, None);

    fixture.mode(80);
    assert!(fixture
        .client
        .tasks()
        .get_run_state("run-1")
        .await
        .unwrap()
        .is_none());
    fixture.mode(81);
    assert_eq!(
        fixture
            .client
            .tasks()
            .get_run_state("run-1")
            .await
            .unwrap_err()
            .code(),
        Code::Unavailable,
    );
    fixture.mode(82);
    assert_eq!(
        fixture
            .client
            .tasks()
            .get_run_state("run-1")
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Protocol,
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn localization_pose_stream_preserves_samples_and_failures() {
    let fixture = Fixture::new().await;
    let mut stream = fixture.client.localization().watch_pose().await.unwrap();
    let sample = stream.message().await.unwrap().unwrap();
    assert_eq!(sample.frame_id, "map");
    assert_eq!(sample.pose.unwrap().z, 3.0);
    assert_eq!(
        stream.message().await.unwrap_err().code(),
        Code::Unavailable
    );
    fixture.mode(70);
    assert_eq!(
        fixture
            .client
            .localization()
            .watch_pose()
            .await
            .unwrap_err()
            .code(),
        Code::Unimplemented
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn localization_pose_preserves_age_and_rejects_invalid_source() {
    let fixture = Fixture::new().await;
    let pose = fixture.client.localization().pose().await.unwrap();
    assert_eq!(pose.received_age_ms, Some(10.0));
    assert_eq!(pose.fresh, Some(true));
    fixture.mode(110);
    assert_eq!(
        fixture.client.localization().pose().await.unwrap_err().kind,
        ErrorKind::Protocol
    );
}

#[tokio::test]
async fn navigation_pause_resume_preserve_identity_and_unknown_submission() {
    let fixture = Fixture::new().await;
    assert!(fixture
        .client
        .navigation()
        .pause("task-1", "operator_pause", Some("pause-1"))
        .await
        .unwrap()
        .accepted
        .unwrap());
    assert!(fixture
        .client
        .navigation()
        .resume("task-1", "operator_resume", Some("resume-1"))
        .await
        .unwrap()
        .accepted
        .unwrap());
    let writes = fixture.service.writes.load(Ordering::Relaxed);
    assert!(fixture
        .client
        .navigation()
        .pause("", "pause", None)
        .await
        .is_err());
    assert!(fixture
        .client
        .navigation()
        .resume("task-1", "", None)
        .await
        .is_err());
    assert!(fixture
        .client
        .navigation()
        .pause("task-1", "pause", Some("task-1"))
        .await
        .is_err());
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), writes);
    for mode in [112, 113] {
        fixture.mode(mode);
        assert_eq!(
            fixture
                .client
                .navigation()
                .pause("task-1", "pause", None)
                .await
                .unwrap_err()
                .kind,
            ErrorKind::Protocol
        );
        assert_eq!(
            fixture
                .client
                .navigation()
                .resume("task-1", "resume", None)
                .await
                .unwrap_err()
                .kind,
            ErrorKind::Protocol
        );
    }
    fixture.mode(2);
    let writes = fixture.service.writes.load(Ordering::Relaxed);
    let error = fixture
        .client
        .navigation()
        .resume("task-1", "resume", None)
        .await
        .unwrap_err();
    assert!(error.submission_unknown);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), writes + 1);
}

#[tokio::test]
async fn navigation_route_stream_preserves_points_and_clear() {
    let fixture = Fixture::new().await;
    let route = fixture.client.navigation().route().await.unwrap();
    assert_eq!(route.frame_id, "map");
    assert_eq!(route.points[0].z, 3.0);
    assert!(route.time.is_none());
    let mut stream = fixture.client.navigation().watch_path().await.unwrap();
    assert_eq!(
        stream.message().await.unwrap().unwrap().points,
        route.points
    );
    assert!(stream.message().await.unwrap().unwrap().points.is_empty());
    fixture.mode(71);
    assert_eq!(
        fixture
            .client
            .navigation()
            .watch_path()
            .await
            .unwrap_err()
            .code(),
        Code::Unavailable
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn saved_grid_preserves_large_cells_and_identity() {
    let fixture = Fixture::new().await;
    let grid = fixture.client.maps().grid("warehouse").await.unwrap();
    assert_eq!((grid.width, grid.height), (2500, 2000));
    assert_eq!(grid.cells.len(), 5_000_000);
    assert_eq!(grid.cells[0], 255);
    assert_eq!(grid.content_epoch, Some(17));
    assert_eq!(grid.origin.unwrap().x, -3.0);
    assert!(grid.time.is_none());
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn saved_grid_rejects_invalid_replies_and_preserves_errors() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture.client.maps().grid(" ").await.unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    fixture.mode(90);
    assert_eq!(
        fixture
            .client
            .maps()
            .grid("warehouse")
            .await
            .unwrap_err()
            .code(),
        Code::NotFound
    );
    for mode in [91, 92, 93, 94] {
        fixture.mode(mode);
        assert_eq!(
            fixture
                .client
                .maps()
                .grid("warehouse")
                .await
                .unwrap_err()
                .kind,
            ErrorKind::Protocol
        );
    }
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}
impl Fixture {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let service = Service::default();
        let (send, recv) = oneshot::channel();
        let server = tonic::transport::Server::builder()
            .add_service(rpc::system_server::SystemServer::new(service.clone()))
            .add_service(rpc::localization_server::LocalizationServer::new(
                service.clone(),
            ))
            .add_service(rpc::navigation_server::NavigationServer::new(
                service.clone(),
            ))
            .add_service(rpc::motion_server::MotionServer::new(service.clone()))
            .add_service(rpc::observations_server::ObservationsServer::new(
                service.clone(),
            ))
            .add_service(rpc::maps_server::MapsServer::new(service.clone()))
            .add_service(rpc::tasks_server::TasksServer::new(service.clone()));
        tokio::spawn(async move {
            server
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                    let _ = recv.await;
                })
                .await
                .unwrap();
        });
        let mut config = Config::new(&endpoint);
        config.api_key = Some("test-only-key".into());
        Self {
            endpoint,
            client: DosoClient::with_config(config).await.unwrap(),
            service,
            shutdown: Some(send),
        }
    }
    fn mode(&self, value: u8) {
        self.service.mode.store(value, Ordering::Relaxed);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(send) = self.shutdown.take() {
            let _ = send.send(());
        }
    }
}
fn pose() -> Pose {
    Pose {
        x: 1.0,
        y: 2.0,
        z: 3.0,
        yaw: 0.5,
    }
}

fn evidence(request_id: &str) -> Struct {
    Struct {
        fields: [(
            "request_id".into(),
            Value {
                kind: Some(Kind::StringValue(request_id.into())),
            },
        )]
        .into(),
    }
}

#[tokio::test]
async fn live_point_cloud_preserves_large_frames_and_reset() {
    let fixture = Fixture::new().await;
    fixture.mode(45);
    let mut frames = fixture
        .client
        .observations()
        .watch_point_cloud(rpc::CloudKind::Scan)
        .await
        .unwrap();
    let cloud = frames.message().await.unwrap().unwrap().decode().unwrap();
    assert_eq!(cloud.points.len(), 1_000_000);
    assert_eq!(cloud.frame_id, "map");
    assert_eq!(cloud.sequence, 9);
    let reset = frames.message().await.unwrap().unwrap().decode().unwrap();
    assert_eq!(reset.stream_kind, doso_sdk::CloudStreamKind::Reset);
    assert!(frames.message().await.unwrap().is_none());
}

#[tokio::test]
async fn live_point_cloud_preserves_errors_and_cancellation() {
    let fixture = Fixture::new().await;
    assert!(fixture
        .client
        .observations()
        .watch_point_cloud(rpc::CloudKind::Unspecified)
        .await
        .is_err());
    fixture.mode(46);
    let error = match fixture
        .client
        .observations()
        .watch_point_cloud(rpc::CloudKind::Map)
        .await
    {
        Ok(_) => panic!("old service accepted point-cloud stream"),
        Err(error) => error,
    };
    assert_eq!(error.code(), Code::Unimplemented);
    fixture.mode(47);
    let mut frames = fixture
        .client
        .observations()
        .watch_point_cloud(rpc::CloudKind::Map)
        .await
        .unwrap();
    assert!(frames.message().await.unwrap().is_some());
    assert_eq!(
        frames.message().await.unwrap_err().code(),
        Code::Unavailable
    );
    fixture.mode(48);
    let mut frames = fixture
        .client
        .observations()
        .watch_point_cloud(rpc::CloudKind::Scan)
        .await
        .unwrap();
    assert!(frames.message().await.unwrap().is_some());
    drop(frames);
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.service.watches.load(Ordering::Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn map_point_cloud_streams_large_content_and_metadata() {
    let fixture = Fixture::new().await;
    let mut stream = fixture
        .client
        .maps()
        .point_cloud("warehouse")
        .await
        .unwrap();
    let Some(rpc::map_content::Value::Info(info)) = stream.message().await.unwrap().unwrap().value
    else {
        panic!("first map message must contain metadata");
    };
    assert_eq!(info.map_id, "warehouse");
    assert_eq!(info.content_epoch, Some(7));
    assert_eq!(info.frame_id.as_deref(), Some("map"));
    let mut received = 0;
    for index in 0..5 {
        let Some(rpc::map_content::Value::Chunk(bytes)) =
            stream.message().await.unwrap().unwrap().value
        else {
            panic!("subsequent map messages must contain bytes");
        };
        assert_eq!(bytes.len(), 1024 * 1024);
        assert!(bytes.iter().all(|byte| *byte == index));
        received += bytes.len() as u64;
    }
    assert_eq!(received, info.size_bytes);
    assert!(stream.message().await.unwrap().is_none());

    fixture.mode(44);
    let mut stream = fixture
        .client
        .maps()
        .point_cloud("warehouse")
        .await
        .unwrap();
    let Some(rpc::map_content::Value::Info(info)) = stream.message().await.unwrap().unwrap().value
    else {
        panic!("first map message must contain metadata");
    };
    assert_eq!(info.content_epoch, None);
    assert_eq!(info.frame_id, None);
}

#[tokio::test]
async fn map_point_cloud_preserves_failures_and_validates_name() {
    let fixture = Fixture::new().await;
    assert!(fixture.client.maps().point_cloud(" ").await.is_err());
    fixture.mode(42);
    let error = match fixture.client.maps().point_cloud("warehouse").await {
        Ok(_) => panic!("missing point cloud must fail"),
        Err(error) => error,
    };
    assert_eq!(error.code(), Code::NotFound);
    assert!(error.to_string().contains("map point cloud missing"));
    fixture.mode(43);
    let mut stream = fixture
        .client
        .maps()
        .point_cloud("warehouse")
        .await
        .unwrap();
    assert!(stream.message().await.unwrap().is_some());
    let error = stream.message().await.unwrap_err();
    assert_eq!(error.code(), Code::Unavailable);
    assert_eq!(error.message(), "map read interrupted");
}

#[tokio::test]
async fn camera_frames_preserve_jpeg_and_media_metadata() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture
            .client
            .observations()
            .camera_frame(Some("viewer"))
            .await
            .unwrap()
            .jpeg,
        [1, 2, 3]
    );
    let mut stream = fixture
        .client
        .observations()
        .watch_camera(Some("viewer"))
        .await
        .unwrap();
    assert_eq!(stream.message().await.unwrap().unwrap().jpeg, [1, 2, 3]);
    assert_eq!(stream.message().await.unwrap().unwrap().jpeg, [4, 5]);
    assert!(stream.message().await.unwrap().is_none());
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn camera_samples_preserve_source_metadata_and_errors() {
    let fixture = Fixture::new().await;
    let sample = fixture
        .client
        .observations()
        .camera_sample(Some("viewer"))
        .await
        .unwrap();
    assert_eq!(sample.frame.unwrap().jpeg, [1, 2, 3]);
    let info = sample.info.unwrap();
    assert_eq!(info.encoding, "jpeg");
    assert_eq!(info.camera_id, None);
    assert_eq!(info.width_px, None);
    assert_eq!(info.height_px, None);
    assert_eq!(info.frame_id, None);
    assert!(sample.time.is_none());
    assert_eq!(sample.sequence, None);

    let mut stream = fixture
        .client
        .observations()
        .watch_camera_samples(Some("viewer"))
        .await
        .unwrap();
    for (sequence, bytes) in [(1, vec![1, 2, 3]), (2, vec![4, 5])] {
        let sample = stream.message().await.unwrap().unwrap();
        assert_eq!(sample.frame.unwrap().jpeg, bytes);
        assert_eq!(sample.sequence, Some(sequence));
    }
    assert!(stream.message().await.unwrap().is_none());
    fixture.mode(40);
    assert_eq!(
        fixture
            .client
            .observations()
            .camera_sample(Some("viewer"))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied,
    );
    fixture.mode(41);
    let mut stream = fixture
        .client
        .observations()
        .watch_camera_samples(Some("viewer"))
        .await
        .unwrap();
    assert!(stream.message().await.unwrap().is_some());
    assert_eq!(
        stream.message().await.unwrap_err().code(),
        Code::Unavailable
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn camera_frames_preserve_errors_and_reject_invalid_metadata() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture
            .client
            .observations()
            .camera_frame(Some(""))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    );
    assert!(fixture
        .client
        .observations()
        .watch_camera(Some("bad\ntoken"))
        .await
        .is_err());
    fixture.mode(40);
    assert_eq!(
        fixture
            .client
            .observations()
            .camera_frame(Some("viewer"))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );
    assert_eq!(
        fixture
            .client
            .observations()
            .watch_camera(Some("viewer"))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );
    fixture.mode(41);
    let mut stream = fixture
        .client
        .observations()
        .watch_camera(Some("viewer"))
        .await
        .unwrap();
    assert!(stream.message().await.unwrap().is_some());
    assert_eq!(
        stream.message().await.unwrap_err().code(),
        Code::Unavailable
    );
}

#[tokio::test]
async fn body_states_preserve_values_and_unknown_enums() {
    let fixture = Fixture::new().await;
    let state = fixture.client.motion().body_state().await.unwrap();
    assert_eq!(state.kind, 4);
    assert_eq!(state.transition, 3);
    assert_eq!(state.gesture_name, "bow");
    let mut stream = fixture.client.motion().watch_body_state().await.unwrap();
    for kind in 0..=4 {
        let state = stream.message().await.unwrap().unwrap();
        assert_eq!(state.kind, kind);
        assert_eq!(state.transition, if kind == 4 { 1 } else { 0 });
    }
    let state = stream.message().await.unwrap().unwrap();
    assert_eq!(state.kind, 91);
    assert_eq!(state.transition, 92);
    assert_eq!(state.gesture_name, "future");
    assert!(stream.message().await.unwrap().is_none());
    fixture.mode(33);
    let state = fixture.client.motion().body_state().await.unwrap();
    assert_eq!((state.kind, state.transition), (91, 92));
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn control_status_preserves_source_values_and_errors() {
    let fixture = Fixture::new().await;
    let state = fixture.client.motion().control_status().await.unwrap();
    assert_eq!(state.source_owner, 2);
    assert_eq!(state.source_owner_id, "physical-remote");
    assert_eq!(state.body.unwrap().kind, 999);
    assert!(state.motor_output_enabled && state.critical_motor_fault);
    assert!(!state.grpc_lease_active && !state.source_ready_for_walk);
    assert_eq!(
        (
            state.last_accepted_sequence,
            state.accepted_count,
            state.rejected_count
        ),
        (17, 21, 3)
    );
    for (mode, code) in [(95, Code::Unimplemented), (96, Code::PermissionDenied)] {
        fixture.mode(mode);
        assert_eq!(
            fixture
                .client
                .motion()
                .control_status()
                .await
                .unwrap_err()
                .code(),
            code
        );
    }
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn body_states_preserve_source_errors() {
    let fixture = Fixture::new().await;
    for (mode, code) in [(30, Code::Unimplemented), (32, Code::Unavailable)] {
        fixture.mode(mode);
        assert_eq!(
            fixture
                .client
                .motion()
                .body_state()
                .await
                .unwrap_err()
                .code(),
            code
        );
        assert_eq!(
            fixture
                .client
                .motion()
                .watch_body_state()
                .await
                .unwrap_err()
                .code(),
            code
        );
    }
    fixture.mode(31);
    let mut stream = fixture.client.motion().watch_body_state().await.unwrap();
    for _ in 0..=4 {
        assert!(stream.message().await.unwrap().is_some());
    }
    let error = stream.message().await.unwrap_err();
    assert_eq!(error.code(), Code::Unavailable);
    assert_eq!(error.message(), "body source disconnected");
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn motor_queries_preserve_diagnostics_and_voltage_order() {
    let fixture = Fixture::new().await;
    let motors = fixture.client.observations().motors().await.unwrap().motors;
    assert_eq!(motors.len(), 2);
    assert_eq!(motors[0].id, 3);
    assert!(motors[0].online);
    assert_eq!(motors[0].status_code, 7);
    assert_eq!(motors[0].temperature_c, 42.5);
    assert_eq!(motors[0].errors, [1, 9]);
    assert_eq!(motors[1].id, 12);
    assert!(!motors[1].online);
    assert_eq!(motors[1].temperature_c, 0.0);
    assert!(motors[1].errors.is_empty());
    assert_eq!(
        fixture
            .client
            .observations()
            .motor_voltages()
            .await
            .unwrap()
            .volts,
        [48.25, 0.0, 47.75]
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn motor_queries_preserve_source_errors() {
    let fixture = Fixture::new().await;
    fixture.mode(29);
    let error = fixture.client.observations().motors().await.unwrap_err();
    assert_eq!(error.code(), Code::Unavailable);
    assert!(error.to_string().contains("motor source unavailable"));
    let error = fixture
        .client
        .observations()
        .motor_voltages()
        .await
        .unwrap_err();
    assert_eq!(error.code(), Code::FailedPrecondition);
    assert!(error.to_string().contains("voltage requires hardware mode"));
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn sensor_observations_preserve_samples_and_presence() {
    let fixture = Fixture::new().await;
    let mut imu = fixture.client.observations().watch_imu().await.unwrap();
    let sample = imu.message().await.unwrap().unwrap();
    assert_eq!(sample.angular_velocity_rad_s.unwrap().y, -2.0);
    assert_eq!(sample.orientation_world_to_body.unwrap().w, 1.0);
    assert_eq!(
        sample.source_elapsed,
        Some(prost_types::Duration {
            seconds: 12,
            nanos: 345
        })
    );
    assert!(sample.linear_acceleration_m_s2.is_none());
    assert_eq!(
        imu.message()
            .await
            .unwrap()
            .unwrap()
            .linear_acceleration_m_s2
            .unwrap()
            .z,
        9.75
    );

    let mut joints = fixture.client.observations().watch_joints().await.unwrap();
    let single = joints.message().await.unwrap().unwrap();
    assert_eq!(single.joints.len(), 1);
    assert_eq!(single.joints[0].id, 12);
    assert_eq!(single.joints[0].position_rad, -0.5);
    assert_eq!(single.joints[0].velocity_rad_s, 0.75);
    assert_eq!(single.joints[0].torque_nm, -1.25);
    assert_eq!(single.joints[0].status_code, 7);
    assert_eq!(
        single.source_elapsed,
        Some(prost_types::Duration {
            seconds: 20,
            nanos: 999
        })
    );
    let full = joints.message().await.unwrap().unwrap();
    assert_eq!(full.joints.len(), 16);
    assert_eq!(full.joints[15].position_rad, 15.1);
    assert!(full.source_elapsed.is_none());
    drop((imu, joints));
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.client.system().ping().await.unwrap().status, "ok");
}

#[tokio::test]
async fn sensor_observations_preserve_source_failures() {
    let fixture = Fixture::new().await;
    fixture.mode(27);
    assert_eq!(
        fixture
            .client
            .observations()
            .watch_imu()
            .await
            .unwrap_err()
            .code(),
        Code::Unavailable
    );
    assert_eq!(
        fixture
            .client
            .observations()
            .watch_joints()
            .await
            .unwrap_err()
            .code(),
        Code::Unavailable
    );
    fixture.mode(28);
    let mut imu = fixture.client.observations().watch_imu().await.unwrap();
    let mut joints = fixture.client.observations().watch_joints().await.unwrap();
    assert!(imu.message().await.unwrap().is_some());
    assert!(joints.message().await.unwrap().is_some());
    assert_eq!(imu.message().await.unwrap_err().code(), Code::Unavailable);
    assert_eq!(
        joints.message().await.unwrap_err().code(),
        Code::Unavailable
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn motion_observation_streams_without_motion_commands() {
    let fixture = Fixture::new().await;
    let mut stream = fixture.client.motion().watch_state().await.unwrap();
    for state in ["state-1", "state-2"] {
        let snapshot = stream.message().await.unwrap().unwrap();
        assert_eq!(snapshot.fields, Some(evidence(state)));
    }
    drop(stream);
    assert_eq!(fixture.client.system().ping().await.unwrap().status, "ok");
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn motion_observation_preserves_stream_errors() {
    let fixture = Fixture::new().await;
    fixture.mode(25);
    let mut stream = fixture.client.motion().watch_state().await.unwrap();
    assert!(stream.message().await.unwrap().is_some());
    assert_eq!(
        stream.message().await.unwrap_err().code(),
        Code::Unavailable
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn motion_observation_preserves_admission_errors() {
    let fixture = Fixture::new().await;
    fixture.mode(24);
    let error = fixture.client.motion().watch_state().await.unwrap_err();
    assert_eq!(error.code(), Code::ResourceExhausted);
    assert!(!error.submission_unknown);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn motion_observation_selects_frequency_and_preserves_errors() {
    let fixture = Fixture::new().await;
    for rate in [1, 2, 4, 100] {
        let mut stream = fixture.client.motion().watch_state_at(rate).await.unwrap();
        assert_eq!(
            stream.message().await.unwrap().unwrap().fields,
            Some(evidence(&rate.to_string()))
        );
    }
    for rate in [0, 101] {
        assert_eq!(
            fixture
                .client
                .motion()
                .watch_state_at(rate)
                .await
                .unwrap_err()
                .code(),
            Code::InvalidArgument
        );
    }
    fixture.mode(26);
    assert_eq!(
        fixture
            .client
            .motion()
            .watch_state_at(2)
            .await
            .unwrap_err()
            .code(),
        Code::Unimplemented
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn handshake_returns_initial_info() {
    let fixture = Fixture::new().await;
    fixture.mode(21);
    let mut config = Config::new(&fixture.endpoint);
    config.client_id = "observer-test".into();
    config.api_key = Some("test-only-key".into());
    let client = DosoClient::with_config(config).await.unwrap();
    let info = client.system().info();
    assert_eq!(info.firmware_version.as_deref(), Some("test-firmware"));
    assert_eq!(info.robot_version.as_deref(), Some("vendor/model"));
    assert_eq!(
        info.robot_id.as_deref(),
        Some("8f7e1c62-0a4d-4b89-96e2-6c47b03db1a5")
    );
    fixture.mode(18);
    assert_eq!(client.system().info(), info);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn readiness_state_reads_live_fields() {
    let fixture = Fixture::new().await;
    let state = fixture.client.system().readiness_state().await.unwrap();
    assert_eq!(state.status, "degraded");
    assert_eq!(state.ready, Some(false));
    assert_eq!(state.data_ready, Some(true));
    assert_eq!(state.motion_ready, None);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn robot_status_preserves_unknowns_and_errors() {
    let fixture = Fixture::new().await;
    let state = fixture.client.system().robot_status().await.unwrap();
    assert_eq!(state.ready, Some(false));
    assert_eq!(state.software_emergency_stop, Some(false));
    assert_eq!(state.hardware_emergency_stop, None);
    assert_eq!(state.health, rpc::HealthStatus::Unknown as i32);
    fixture.mode(100);
    assert_eq!(
        fixture
            .client
            .system()
            .robot_status()
            .await
            .unwrap_err()
            .code(),
        Code::Unimplemented
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn cancel_run_rejects_another_runs_receipt() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture
            .client
            .tasks()
            .cancel_run("run-1", Some("cancel-1"))
            .await
            .unwrap()
            .run_id
            .as_deref(),
        Some("run-1")
    );
    fixture.mode(101);
    let error = fixture
        .client
        .tasks()
        .cancel_run("run-1", Some("cancel-2"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Protocol);
    assert_eq!(error.request_id.as_deref(), Some("cancel-2"));
    assert!(error.submission_unknown);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn missing_identity_remains_absent() {
    let fixture = Fixture::new().await;
    fixture.mode(22);
    let mut config = Config::new(&fixture.endpoint);
    config.api_key = Some("test-only-key".into());
    let client = DosoClient::with_config(config).await.unwrap();
    assert_eq!(client.system().info().firmware_version, None);
    assert_eq!(client.system().info().robot_version, None);
    assert_eq!(client.system().info().robot_id, None);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn incompatible_handshakes_fail_connection() {
    let fixture = Fixture::new().await;
    for mode in [17, 18] {
        fixture.mode(mode);
        let mut config = Config::new(&fixture.endpoint);
        config.api_key = Some("test-only-key".into());
        let error = DosoClient::with_config(config)
            .await
            .err()
            .expect("connection must fail");
        if mode == 17 {
            assert_eq!(error.kind, ErrorKind::Protocol);
        } else {
            assert_eq!(error.code(), Code::FailedPrecondition);
            assert_eq!(error.reason_code(), Some("protocol_mismatch"));
        }
        assert!(!error.submission_unknown);
    }
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn missing_handshake_info_fails_connection() {
    let fixture = Fixture::new().await;
    fixture.mode(23);
    let mut config = Config::new(&fixture.endpoint);
    config.api_key = Some("test-only-key".into());
    let error = DosoClient::with_config(config)
        .await
        .err()
        .expect("missing system info must fail connection");
    assert_eq!(error.kind, ErrorKind::Protocol);
    assert!(!error.submission_unknown);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn handshake_obeys_timeout() {
    let fixture = Fixture::new().await;
    fixture.mode(19);
    let mut config = Config::new(&fixture.endpoint);
    config.api_key = Some("test-only-key".into());
    config.timeout = Duration::from_millis(50);
    let error = tokio::time::timeout(Duration::from_secs(2), DosoClient::with_config(config))
        .await
        .unwrap()
        .err()
        .expect("handshake must time out");
    assert!(matches!(
        error.code(),
        Code::Cancelled | Code::DeadlineExceeded
    ));
    assert!(!error.submission_unknown);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn service_without_handshake_fails_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (send, recv) = oneshot::channel();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(rpc::motion_server::MotionServer::new(Service::default()))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = recv.await;
            })
            .await
            .unwrap();
    });
    let result = DosoClient::connect(endpoint).await;
    let _ = send.send(());
    let error = result.err().expect("handshake is required");
    assert_eq!(error.code(), Code::Unimplemented);
    assert!(!error.submission_unknown);
}

#[tokio::test]
async fn typed_goal_and_admission() {
    let fixture = Fixture::new().await;
    let receipt = fixture
        .client
        .navigation()
        .go_to(pose(), Some("request-1"))
        .await
        .unwrap();
    assert!(receipt.ok);
    assert_eq!(receipt.accepted, Some(true));
    assert_eq!(receipt.task_id.as_deref(), Some("task-1"));
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 1);
    let mut invalid = pose();
    invalid.x = f64::NAN;
    assert_eq!(
        fixture
            .client
            .navigation()
            .go_to(invalid, None)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn motion_stop_and_reset_preserve_command_receipts() {
    let fixture = Fixture::new().await;
    let stopped = fixture.client.motion().stop(Some("hold-1")).await.unwrap();
    assert_eq!(stopped.request_id, "hold-1");
    assert_eq!(stopped.accepted, Some(true));
    assert_eq!(stopped.stage, "motion_stop_acknowledged");
    let cleared = fixture
        .client
        .motion()
        .clear_emergency_stop(Some("clear-1"))
        .await
        .unwrap();
    assert_eq!(cleared.request_id, "clear-1");
    assert_eq!(cleared.stage, "estop_reset_acknowledged");
    fixture.mode(2);
    let error = fixture
        .client
        .motion()
        .stop(Some("hold-2"))
        .await
        .unwrap_err();
    assert!(error.submission_unknown);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn teleop_preserves_velocity_hold_and_release_acknowledgements() {
    let fixture = Fixture::new().await;
    let mut session = fixture.client.motion().teleop().await.unwrap();
    for id in ["velocity-1", "velocity-2"] {
        let reply = session
            .send_velocity(
                rpc::TeleopVelocity {
                    vx_m_s: 0.2,
                    deadman: true,
                    ..Default::default()
                },
                Some(id),
            )
            .await
            .unwrap();
        assert_eq!(reply.request_id, id);
        assert!(reply.accepted && reply.ingress_accepted);
        assert!(!reply.final_output_confirmed);
    }
    let reply = session.hold(Some("hold-1")).await.unwrap();
    assert_eq!(reply.request_id, "hold-1");
    assert!(reply.accepted && reply.final_output_confirmed);
    assert!(!reply.ingress_accepted);
    assert!(
        session
            .send_velocity(
                rpc::TeleopVelocity {
                    vx_m_s: 0.1,
                    deadman: true,
                    ..Default::default()
                },
                Some("after-hold")
            )
            .await
            .unwrap()
            .accepted
    );
    let reply = session.close(Some("close-1")).await.unwrap();
    assert_eq!(reply.request_id, "close-1");
    assert!(reply.accepted && reply.final_output_confirmed);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn teleop_rejects_invalid_velocity_before_sending() {
    let fixture = Fixture::new().await;
    let mut session = fixture.client.motion().teleop().await.unwrap();
    let error = session
        .send_velocity(
            rpc::TeleopVelocity {
                vx_m_s: f64::NAN,
                deadman: true,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
    assert!(session.close(None).await.unwrap().accepted);
}

#[tokio::test]
async fn teleop_identity_mismatch_closes_session_without_replay() {
    let fixture = Fixture::new().await;
    fixture.mode(120);
    let mut session = fixture.client.motion().teleop().await.unwrap();
    let error = session
        .send_velocity(
            rpc::TeleopVelocity {
                vx_m_s: 0.2,
                deadman: true,
                ..Default::default()
            },
            Some("velocity-1"),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Protocol);
    assert_eq!(error.request_id.as_deref(), Some("velocity-1"));
    assert!(error.submission_unknown);
    assert_eq!(
        session.hold(None).await.unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn teleop_timeout_preserves_unknown_acceptance_without_replay() {
    let fixture = Fixture::new().await;
    fixture.mode(121);
    let mut config = Config::new(&fixture.endpoint);
    config.api_key = Some("test-only-key".into());
    config.timeout = Duration::from_millis(150);
    let client = DosoClient::with_config(config).await.unwrap();
    let mut session = client.motion().teleop().await.unwrap();
    let error = session
        .send_velocity(
            rpc::TeleopVelocity {
                vx_m_s: 0.2,
                deadman: true,
                ..Default::default()
            },
            Some("velocity-timeout"),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Timeout);
    assert!(error.submission_unknown);
    assert_eq!(error.request_id.as_deref(), Some("velocity-timeout"));
    assert_eq!(
        session.hold(None).await.unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn teleop_preserves_unconfirmed_release() {
    let fixture = Fixture::new().await;
    let session = fixture.client.motion().teleop().await.unwrap();
    fixture.mode(122);
    let reply = session.close(Some("close-1")).await.unwrap();
    assert!(!reply.accepted && !reply.final_output_confirmed);
    assert_eq!(reply.reason, "stop_unconfirmed");
}

#[tokio::test]
async fn unknown_write_is_not_replayed() {
    let fixture = Fixture::new().await;
    fixture.mode(2);
    let error = fixture
        .client
        .motion()
        .emergency_stop(Some("stop-1"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Rpc(tonic::Code::Unavailable));
    assert_eq!(error.code(), Code::Unavailable);
    assert_eq!(error.reason_code(), None);
    assert_eq!(error.details(), None);
    assert!(error.submission_unknown);
    assert_eq!(error.request_id.as_deref(), Some("stop-1"));
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn receipt_and_task_identity() {
    let fixture = Fixture::new().await;
    fixture.mode(1);
    let error = fixture
        .client
        .navigation()
        .go_to(pose(), Some("goal-1"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Protocol);
    assert!(error.submission_unknown);
    fixture.mode(3);
    assert_eq!(
        fixture
            .client
            .navigation()
            .task("task-1")
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Protocol
    );
}

#[tokio::test]
async fn wait_consumes_task_stream() {
    let fixture = Fixture::new().await;
    let task = fixture
        .client
        .navigation()
        .wait("task-1", Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(task.state, TaskState::TaskSuccess as i32);
    assert_eq!(fixture.service.watches.load(Ordering::Relaxed), 1);
    fixture.mode(5);
    assert_eq!(
        fixture
            .client
            .navigation()
            .task("task-1")
            .await
            .unwrap()
            .state,
        TaskState::TaskFailed as i32
    );
    let error = fixture
        .client
        .navigation()
        .wait("task-1", Duration::from_secs(2))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::OperationFailed);
    assert_eq!(error.code(), Code::Unknown);
    assert_eq!(error.reason_code(), Some("test_failure"));
    assert_eq!(error.details(), Some(&evidence("task-1")));
    let Some(Terminal::Navigation(task)) = error.terminal.as_deref() else {
        panic!("missing navigation terminal");
    };
    assert_eq!(task.task_id, "task-1");
    assert_eq!(task.state, TaskState::TaskFailed as i32);
    fixture.mode(9);
    assert_eq!(
        fixture
            .client
            .navigation()
            .task("task-1")
            .await
            .unwrap()
            .state,
        TaskState::TaskCancelled as i32
    );
    let error = fixture
        .client
        .navigation()
        .wait("task-1", Duration::from_secs(2))
        .await
        .unwrap_err();
    let Some(Terminal::Navigation(task)) = error.terminal.as_deref() else {
        panic!("missing cancelled navigation terminal");
    };
    assert_eq!(task.task_id, "task-1");
    assert_eq!(task.state, TaskState::TaskCancelled as i32);
    fixture.mode(3);
    assert_eq!(
        fixture
            .client
            .navigation()
            .wait("task-1", Duration::from_secs(2))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Protocol
    );
}

#[tokio::test]
async fn timeout_does_not_send_cancel() {
    let fixture = Fixture::new().await;
    fixture.mode(4);
    assert_eq!(
        fixture
            .client
            .navigation()
            .wait("task-1", Duration::from_millis(50))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Timeout
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.service.watches.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn rpc_error_details() {
    let fixture = Fixture::new().await;
    fixture.mode(6);
    let error = fixture
        .client
        .motion()
        .emergency_stop(Some("stop-1"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Rpc(Code::FailedPrecondition));
    assert_eq!(error.code(), Code::FailedPrecondition);
    assert_eq!(error.reason_code(), Some("safety_stop"));
    assert_eq!(error.details(), Some(&evidence("stop-1")));
    assert_eq!(error.request_id.as_deref(), Some("stop-1"));
    assert!(!error.submission_unknown);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn invalid_details_preserve_rpc_failure() {
    let fixture = Fixture::new().await;
    fixture.mode(7);
    let error = fixture
        .client
        .motion()
        .emergency_stop(Some("stop-1"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Rpc(Code::Unavailable));
    assert_eq!(error.code(), Code::Unavailable);
    assert!(error.message.contains("owner acknowledgement unknown"));
    assert!(error.message.contains("invalid DOSO error details"));
    assert_eq!(error.reason_code(), None);
    assert_eq!(error.request_id.as_deref(), Some("stop-1"));
    assert!(error.submission_unknown);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn accepted_receipts_require_identity() {
    let fixture = Fixture::new().await;
    fixture.mode(8);
    let error = fixture
        .client
        .navigation()
        .go_to(pose(), Some("goal-1"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Protocol);
    assert_eq!(error.code(), Code::Internal);
    assert_eq!(error.request_id.as_deref(), Some("goal-1"));
    assert!(error.submission_unknown);
    let error = fixture
        .client
        .maps()
        .save(None, Some("save-1"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Protocol);
    assert_eq!(error.request_id.as_deref(), Some("save-1"));
    assert!(error.submission_unknown);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn map_and_product_terminals() {
    let fixture = Fixture::new().await;
    for (mode, state) in [(10, MapState::MapFailed), (11, MapState::MapCancelled)] {
        fixture.mode(mode);
        let error = fixture
            .client
            .maps()
            .wait("map-1", Duration::from_secs(2), Duration::from_millis(10))
            .await
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::OperationFailed);
        assert_eq!(error.reason_code(), Some("map_write_failed"));
        assert_eq!(error.details(), Some(&evidence("map-1")));
        let Some(Terminal::Map(operation)) = error.terminal.as_deref() else {
            panic!("missing map terminal");
        };
        assert_eq!(operation.operation_id, "map-1");
        assert_eq!(operation.state, state as i32);
    }
    for (mode, state) in [
        (12, ProductState::ProductFailed),
        (13, ProductState::ProductInterrupted),
    ] {
        fixture.mode(mode);
        let error = fixture
            .client
            .maps()
            .wait_product_operation(
                "switch-1",
                Duration::from_secs(2),
                Duration::from_millis(10),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::OperationFailed);
        assert_eq!(error.reason_code(), Some("switch_failed"));
        assert_eq!(error.details(), Some(&evidence("switch-1")));
        let Some(Terminal::Product(operation)) = error.terminal.as_deref() else {
            panic!("missing product terminal");
        };
        assert_eq!(operation.request_id, "switch-1");
        assert_eq!(operation.state, state as i32);
    }
}

#[tokio::test]
async fn unknown_states_keep_waiting() {
    let fixture = Fixture::new().await;
    fixture.mode(14);
    assert_eq!(
        fixture
            .client
            .navigation()
            .task("task-1")
            .await
            .unwrap()
            .state,
        TaskState::TaskUnknown as i32
    );
    assert_eq!(
        fixture
            .client
            .navigation()
            .wait("task-1", Duration::from_secs(2))
            .await
            .unwrap()
            .state,
        TaskState::TaskSuccess as i32,
    );
    assert_eq!(
        fixture
            .client
            .maps()
            .operation("map-1")
            .await
            .unwrap()
            .state,
        MapState::MapUnknown as i32
    );
    assert_eq!(
        fixture
            .client
            .maps()
            .product_operation("switch-1")
            .await
            .unwrap()
            .state,
        ProductState::Unknown as i32
    );
    let timeout = Duration::from_millis(100);
    let interval = Duration::from_millis(10);
    assert_eq!(
        fixture
            .client
            .maps()
            .wait("map-1", timeout, interval)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Timeout
    );
    assert_eq!(
        fixture
            .client
            .maps()
            .wait_product_operation("switch-1", timeout, interval)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Timeout
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn undefined_states_are_protocol_errors() {
    let fixture = Fixture::new().await;
    fixture.mode(15);
    let timeout = Duration::from_secs(2);
    let interval = Duration::from_millis(10);
    assert_eq!(
        fixture
            .client
            .navigation()
            .task("task-1")
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Protocol
    );
    assert_eq!(
        fixture
            .client
            .navigation()
            .wait("task-1", timeout)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Protocol
    );
    assert_eq!(
        fixture
            .client
            .maps()
            .wait("map-1", timeout, interval)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Protocol
    );
    assert_eq!(
        fixture
            .client
            .maps()
            .wait_product_operation("switch-1", timeout, interval)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Protocol
    );
}

#[tokio::test]
async fn oversized_reply_leaves_submission_unknown() {
    let fixture = Fixture::new().await;
    fixture.mode(16);
    let error = fixture
        .client
        .motion()
        .emergency_stop(Some("large-1"))
        .await
        .unwrap_err();
    assert_eq!(error.code(), Code::OutOfRange);
    assert_eq!(error.request_id.as_deref(), Some("large-1"));
    assert!(error.submission_unknown);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn robot_client_supports_damp_fault_clear_gestures_and_stand_down() {
    let fixture = Fixture::new().await;
    let robot: &doso_sdk::RobotClient = &fixture.client;

    let damp = robot.sport().damp(Some("damp-1")).await.unwrap();
    assert_eq!(damp.request_id, "damp-1");
    assert_eq!(damp.stage, "damping_requested");

    let down = robot.sport().stand_down(Some("down-1")).await.unwrap();
    assert_eq!(down.request_id, "down-1");
    assert_eq!(down.stage, "posture_requested");

    let err = robot
        .sport()
        .clear_motor_fault([16], Some("bad-fault"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidInput);

    let cleared = robot
        .sport()
        .clear_motor_fault([0, 15], Some("clear-1"))
        .await
        .unwrap();
    assert_eq!(cleared.request_id, "clear-1");
    assert_eq!(cleared.stage, "fault_cleared");

    let gestures = robot.sport().list_gestures().await.unwrap();
    assert_eq!(gestures.gestures.len(), 1);
    assert_eq!(gestures.gestures[0].name, "bow");

    let played = robot
        .sport()
        .play_gesture("bow", Some("gesture-1"))
        .await
        .unwrap();
    assert_eq!(played.request_id, "gesture-1");
    assert_eq!(played.stage, "gesture:bow");

    let empty_err = robot.sport().play_gesture("   ", None).await.unwrap_err();
    assert_eq!(empty_err.kind, ErrorKind::InvalidInput);
}

#[derive(Clone)]
struct RedirectingSystem {
    low_level_endpoint: String,
}

#[tonic::async_trait]
impl rpc::system_server::System for RedirectingSystem {
    async fn handshake(
        &self,
        _: Request<rpc::Handshake>,
    ) -> Result<Response<rpc::HandshakeReply>, Status> {
        Ok(Response::new(rpc::HandshakeReply {
            protocol_version: 2,
            info: Some(rpc::SystemInfo {
                firmware_version: Some("v3-split".into()),
                robot_version: Some("doso-thunder".into()),
                robot_id: Some("8f7e1c62-0a4d-4b89-96e2-6c47b03db1a5".into()),
                low_level_endpoint: Some(self.low_level_endpoint.clone()),
            }),
        }))
    }
    async fn ping(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Ping>, Status> {
        Ok(Response::new(rpc::Ping {
            status: "ok".into(),
        }))
    }
    async fn state(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }
    async fn robot_status(
        &self,
        _: Request<rpc::Empty>,
    ) -> Result<Response<rpc::RobotStatus>, Status> {
        Ok(Response::new(rpc::RobotStatus::default()))
    }
    async fn health(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }
    async fn faults(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::FaultReport>, Status> {
        Ok(Response::new(rpc::FaultReport::default()))
    }
    type WatchFaultsStream = Pin<Box<dyn Stream<Item = Result<rpc::FaultReport, Status>> + Send>>;
    async fn watch_faults(
        &self,
        _: Request<rpc::Empty>,
    ) -> Result<Response<Self::WatchFaultsStream>, Status> {
        Ok(Response::new(Box::pin(tokio_stream::empty())))
    }
    async fn readiness(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }
    async fn capabilities(
        &self,
        _: Request<rpc::Empty>,
    ) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }
    async fn session(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }
    async fn product_status(
        &self,
        _: Request<rpc::Empty>,
    ) -> Result<Response<rpc::Snapshot>, Status> {
        Ok(Response::new(rpc::Snapshot::default()))
    }
}

#[tokio::test]
async fn robot_client_routes_low_level_to_advertised_direct_endpoint() {
    let fast_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fast_endpoint = format!("http://{}", fast_listener.local_addr().unwrap());
    let fast_service = Service::default();
    let (fast_tx, fast_rx) = oneshot::channel();
    let fast_server = tonic::transport::Server::builder()
        .add_service(rpc::motion_server::MotionServer::new(fast_service.clone()));
    tokio::spawn(async move {
        fast_server
            .serve_with_incoming_shutdown(TcpListenerStream::new(fast_listener), async {
                let _ = fast_rx.await;
            })
            .await
            .unwrap();
    });

    let main_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let main_endpoint = format!("http://{}", main_listener.local_addr().unwrap());
    let (main_tx, main_rx) = oneshot::channel();
    let main_server = tonic::transport::Server::builder().add_service(
        rpc::system_server::SystemServer::new(RedirectingSystem {
            low_level_endpoint: fast_endpoint.clone(),
        }),
    );
    tokio::spawn(async move {
        main_server
            .serve_with_incoming_shutdown(TcpListenerStream::new(main_listener), async {
                let _ = main_rx.await;
            })
            .await
            .unwrap();
    });

    let mut cfg = doso_sdk::RobotConfig::new(main_endpoint);
    cfg.api_key = Some("test-only-key".into());
    let robot = doso_sdk::RobotClient::with_config(cfg).await.unwrap();
    assert_eq!(
        robot.system().info().low_level_endpoint.as_deref(),
        Some(fast_endpoint.as_str())
    );

    let mut ll = robot.low_level(Some("ll-direct")).await.unwrap();
    let mut cmds = vec![rpc::JointCommand::leg_pd(0.1, 10.0, 1.0); 12];
    cmds.extend(std::iter::repeat_n(
        rpc::JointCommand::wheel_vel(2.0, 1.5),
        4,
    ));
    ll.publish_joints(cmds, Some("ll-frame")).unwrap();
    for _ in 0..100 {
        if ll.state().unwrap().unwrap().last_sequence == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(fast_service.writes.load(Ordering::Relaxed), 1);
    assert!(ll.close(Some("ll-close")).await.unwrap().accepted);

    let _ = fast_tx.send(());
    let _ = main_tx.send(());
}
