//! Typed, asynchronous client for DOSO robot services.
#![doc = include_str!("../../docs/usage.md")]
#[deprecated(
    since = "0.3.0",
    note = "Use `RobotClient` for external robot control, or `doso_sdk::internal::motion_board` for onboard Board-A diagnostics."
)]
pub mod brainstem;
mod client;
mod common;
mod error;
pub mod internal;
mod localization;
mod maps;
mod motion;
mod navigation;
mod observations;
mod system;
mod tasks;
mod voice;

pub mod rpc {
    tonic::include_proto!("doso.v1");
}

#[allow(deprecated)]
#[deprecated(
    since = "0.3.0",
    note = "Use `RobotClient` for external robot control, or `internal::motion_board::MotionBoardClient` for onboard Board-A diagnostics."
)]
pub use brainstem::{
    BrainstemClient, BrainstemConfig, BrainstemExternalClient,
    DEFAULT_ENDPOINT as BRAINSTEM_DEFAULT_ENDPOINT,
};
pub use client::{Config, DosoClient, RobotClient, RobotConfig};
pub use common::new_request_id;
pub use error::{Error, ErrorKind, Result, Terminal};
pub use internal::motion_board::{
    MotionBoardClient, MotionBoardConfig, MotionBoardExternalClient, MotionBoardRecoveryClient,
    DEFAULT_ENDPOINT as MOTION_BOARD_DEFAULT_ENDPOINT,
};
pub use localization::Localization;
pub use maps::Maps;
pub use motion::{ExternalControl, Motion, Teleop};
pub use navigation::{GoalOptions, Navigation};
pub use observations::{CloudStreamKind, Observations, PointCloud};
pub use rpc::{
    BatteryState, BodyControlOwner, BodyControlStatus, BodyHeightStatus, BodyKind, BodyState,
    BodyTransition, CameraCalibration, CameraFrame, CameraInfo, CameraSample,
    ClearMotorFaultRequest, CloudFrame, CloudKind, ControlState, ControllerMode, ControllerState,
    Fault, FaultReport, FaultSeverity, FootContact, GeographicFix, GeographicLocation,
    GeographicState, GestureInfo, GestureList, HealthStatus, ImuSample, InspectionBinding,
    InspectionRoute, InspectionRoutes, InspectionTask, InspectionTaskListQuery, InspectionTasks,
    JointCommand, JointSample, JointState, LightSample, LocalizationQuality, LocalizationState,
    MapContent, MapContentInfo, MapGrid, MapOperation, MapState, MotorState, MotorStates,
    MotorVoltagesResponse, NavigationPath, OdometrySample, PlanPreview, PlayGestureRequest, Pose,
    ProductOperation, ProductState, ProductStatus, ProfileInfo, Quaternion, ReadinessState,
    Receipt as CommandReceipt, RecoveryActionReply, RecoveryCancelRequest, RecoverySessionRequest,
    RecoveryStartRequest, RecoveryStatus, RemoteState, RobotStatus, SampleTime,
    SetBodyHeightRequest, SystemInfo, Task, TaskRunState, TaskRunStatus, TaskState, TeleopReply,
    TeleopVelocity, Vector3, VelocitySample, VoiceChatReply, VoiceChatRequest, VoicePlayback,
    VoicePlaybackCancelRequest, VoicePlaybackQuery, VoiceProfileInfo, VoiceProfileList,
    VoiceSpeakRequest, VoiceSwitchComponentRequest, VoiceSwitchProfileRequest, VoiceSystemStatus,
    VoiceUpdatePromptRequest,
};
pub use system::System;
pub use tasks::Tasks;
pub use tonic::Code;
pub use voice::{Voice, VoiceSpeakOptions};
