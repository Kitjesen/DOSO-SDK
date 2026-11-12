//! Typed, asynchronous gRPC client for DOSO robot services.
#![doc = include_str!("../docs/usage.md")]
mod client;
mod common;
mod error;
mod localization;
mod maps;
mod motion;
mod navigation;
mod observations;
mod system;
mod tasks;

pub mod rpc {
    tonic::include_proto!("doso.v1");
}

pub use client::{Config, DosoClient};
pub use common::new_request_id;
pub use error::{Error, ErrorKind, Result, Terminal};
pub use localization::Localization;
pub use maps::Maps;
pub use motion::{ExternalControl, Motion, Teleop};
pub use navigation::Navigation;
pub use observations::{CloudStreamKind, Observations, PointCloud};
pub use rpc::{
    BatteryState, BodyControlOwner, BodyControlStatus, BodyKind, BodyState, BodyTransition,
    CameraFrame, CameraInfo, CameraSample, CloudFrame, CloudKind, ControlState, ControllerMode,
    ControllerState, Fault, FaultReport, FaultSeverity, FootContact, GeographicFix,
    GeographicLocation, GeographicState, HealthStatus, ImuSample, JointCommand, JointSample,
    JointState, LightSample, LocalizationQuality, LocalizationState, MapContent, MapContentInfo,
    MapGrid, MapOperation, MapState, MotorState, MotorStates, MotorVoltagesResponse,
    NavigationPath, OdometrySample, Pose, ProductOperation, ProductState, ProductStatus,
    ProfileInfo, Quaternion, ReadinessState, Receipt as CommandReceipt, RobotStatus, SampleTime,
    SystemInfo, Task, TaskRunState, TaskRunStatus, TaskState, TeleopReply, TeleopVelocity, Vector3,
    VelocitySample,
};
pub use system::System;
pub use tasks::Tasks;
pub use tonic::Code;
