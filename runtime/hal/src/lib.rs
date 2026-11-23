pub mod robostride;
pub mod hipnuc;
pub mod yunzhuo;
pub mod traits;
pub mod bench;
pub mod replay;
pub mod socketcan;
pub mod serial_imu;
pub mod remote_input;

pub use traits::*;
pub use bench::*;
pub use replay::*;
pub use remote_input::{
    GSState, JoystickPadMapper, LinuxJsEvent, YunZhuoControlMode, YunZhuoModeSnapshot,
    YunZhuoModeStateMachine, YunZhuoState, JS_EVENT_AXIS, JS_EVENT_BUTTON, JS_EVENT_INIT,
};
pub use serial_imu::{
    parse_imu_baud, resolve_imu_baud_from_env, ImuSerialState, DEFAULT_IMU_BAUD,
    IMU_STALE_TIMEOUT_US,
};
pub use socketcan::{build_can_startup_frames, build_leg_set_zero_frames, SocketCanCore};
#[cfg(target_os = "linux")]
pub use remote_input::LinuxRemoteInputReader;
#[cfg(target_os = "linux")]
pub use serial_imu::SerialImuReader;
#[cfg(target_os = "linux")]
pub use socketcan::SocketCanHal;
