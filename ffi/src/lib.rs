//! Stable C ABI wrapper over [`doso_sdk::RobotClient`] for C++, Python, and mobile bindings.

#![allow(clippy::missing_safety_doc)]

use doso_sdk::{
    ExternalControl, JointCommand, Pose, RobotClient, RobotConfig, Teleop, TeleopVelocity,
    VoiceSpeakOptions,
};
use std::cell::RefCell;
use std::ffi::{c_char, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DosoCapabilities {
    pub motion: u8,
    pub low_level: u8,
    pub observations: u8,
    pub navigation: u8,
    pub maps: u8,
    pub localization: u8,
    pub tasks: u8,
    pub board_b_autonomy: u8,
    pub voice: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DosoBodyControlStatus {
    pub body_kind: i32,
    pub body_transition: i32,
    pub source_owner: i32,
    pub motor_output_enabled: u8,
    pub critical_motor_fault: u8,
    pub grpc_lease_active: u8,
    pub source_ready_for_walk: u8,
    pub lease_remaining_ms: u32,
    pub _pad: u32,
    pub last_accepted_sequence: u64,
    pub accepted_count: u64,
    pub rejected_count: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DosoBodyHeightStatus {
    pub supported: u8,
    pub _pad: [u8; 7],
    pub current_height_m: f64,
    pub target_height_m: f64,
    pub min_height_m: f64,
    pub max_height_m: f64,
    pub rate_mps: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DosoImuSnapshot {
    pub available: u8,
    pub fresh: u8,
    pub stream_active: u8,
    pub _pad: [u8; 5],
    pub age_ms: f64,
    pub gyro_rad_s: [f64; 3],
    pub accel_m_s2: [f64; 3],
    pub quat_wxyz: [f64; 4],
    pub source_elapsed_ns: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DosoJointsSnapshot {
    pub available: u8,
    pub complete: u8,
    pub fresh: u8,
    pub stream_active: u8,
    pub valid_mask: u16,
    pub _pad: u16,
    pub age_ms: f64,
    pub q: [f64; 16],
    pub dq: [f64; 16],
    pub tau: [f64; 16],
    pub status_code: [u32; 16],
    pub source_elapsed_ns: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DosoMotorInfo {
    pub id: u32,
    pub online: u8,
    pub _pad: [u8; 3],
    pub status_code: u32,
    pub error_count: u32,
    pub temperature_c: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DosoMotorsSnapshot {
    pub count: u32,
    pub _pad: u32,
    pub motors: [DosoMotorInfo; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DosoJointCmd {
    pub q: f64,
    pub dq: f64,
    pub kp: f64,
    pub kd: f64,
    pub tau: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DosoControllerSnapshot {
    pub mode: i32,
    pub active: u8,
    pub motor_output_enabled: u8,
    pub fresh: u8,
    pub remote_active: u8,
    pub last_sequence: u64,
    pub joint_count: u32,
    pub remote_buttons: u32,
    pub remote_lx: f64,
    pub remote_ly: f64,
    pub remote_rx: f64,
    pub remote_ry: f64,
}

pub struct DosoRobot {
    client: Arc<RobotClient>,
}

pub struct DosoTeleop {
    // Field declaration order matters: Rust drops `session` before `_owner`,
    // keeping the `Arc<RobotClient>` pointee pinned while `Teleop` lives.
    session: Option<Teleop<'static>>,
    _owner: Arc<RobotClient>,
}

pub struct DosoLowLevel {
    // Field declaration order matters: Rust drops `session` before `_owner`,
    // keeping the `Arc<RobotClient>` pointee pinned while `ExternalControl` lives.
    session: Option<ExternalControl<'static>>,
    _owner: Arc<RobotClient>,
}

#[derive(Default)]
struct ImuCache {
    available: bool,
    stream_active: bool,
    received_at: Option<Instant>,
    gyro_rad_s: [f64; 3],
    accel_m_s2: [f64; 3],
    quat_wxyz: [f64; 4],
    source_elapsed_ns: i64,
}

#[derive(Default)]
struct JointsCache {
    available: bool,
    stream_active: bool,
    valid_mask: u16,
    received_at: Option<Instant>,
    q: [f64; 16],
    dq: [f64; 16],
    tau: [f64; 16],
    status_code: [u32; 16],
    source_elapsed_ns: i64,
}

#[derive(Default)]
struct TelemetryState {
    imu: ImuCache,
    joints: JointsCache,
}

pub struct DosoTelemetry {
    state: Arc<Mutex<TelemetryState>>,
    stale_after: Duration,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for DosoTelemetry {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn duration_to_ns(duration: Option<prost_types::Duration>) -> i64 {
    duration.map_or(0, |d| {
        d.seconds
            .saturating_mul(1_000_000_000)
            .saturating_add(i64::from(d.nanos))
    })
}

fn snapshot_bool(
    fields: &std::collections::BTreeMap<String, prost_types::Value>,
    key: &str,
) -> Option<bool> {
    match fields.get(key).and_then(|v| v.kind.as_ref()) {
        Some(prost_types::value::Kind::BoolValue(value)) => Some(*value),
        _ => None,
    }
}

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to initialize doso-sdk-ffi tokio runtime")
    })
}

fn set_error(message: impl Into<String>) {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = message.into());
}

fn clear_error() {
    LAST_ERROR.with(|slot| slot.borrow_mut().clear());
}

fn write_c_string(src: &str, buf: *mut c_char, cap: usize) -> usize {
    if buf.is_null() || cap == 0 {
        return src.len();
    }
    let bytes = src.as_bytes();
    let n = bytes.len().min(cap - 1);
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast::<u8>(), n);
        *buf.add(n) = 0;
    }
    src.len()
}

unsafe fn required_str<'a>(ptr: *const c_char, name: &str) -> Result<&'a str, String> {
    if ptr.is_null() {
        return Err(format!("{name} is null"));
    }
    CStr::from_ptr(ptr)
        .to_str()
        .map_err(|e| format!("{name} must be valid UTF-8: {e}"))
}

unsafe fn optional_str<'a>(ptr: *const c_char) -> Result<Option<&'a str>, String> {
    if ptr.is_null() {
        Ok(None)
    } else {
        CStr::from_ptr(ptr)
            .to_str()
            .map(|s| (!s.is_empty()).then_some(s))
            .map_err(|e| format!("string must be valid UTF-8: {e}"))
    }
}

fn ffi_status(op: impl FnOnce() -> Result<(), String>) -> i32 {
    clear_error();
    match catch_unwind(AssertUnwindSafe(op)) {
        Ok(Ok(())) => 0,
        Ok(Err(msg)) => {
            set_error(msg);
            -1
        }
        Err(_) => {
            set_error("doso-sdk-ffi panicked");
            -2
        }
    }
}

fn ffi_ptr<T>(op: impl FnOnce() -> Result<*mut T, String>) -> *mut T {
    clear_error();
    match catch_unwind(AssertUnwindSafe(op)) {
        Ok(Ok(p)) => p,
        Ok(Err(msg)) => {
            set_error(msg);
            ptr::null_mut()
        }
        Err(_) => {
            set_error("doso-sdk-ffi panicked");
            ptr::null_mut()
        }
    }
}

fn map_sdk_err(err: doso_sdk::Error) -> String {
    match err.reason_code() {
        Some(code) => format!("{err} (reason_code={code}, code={:?})", err.code()),
        None => format!("{err} (code={:?})", err.code()),
    }
}

fn map_receipt(receipt: doso_sdk::CommandReceipt) -> Result<(), String> {
    if receipt.accepted == Some(false) || (!receipt.ok && receipt.accepted.is_none()) {
        return Err(if receipt.reason.is_empty() {
            "command rejected".to_owned()
        } else {
            receipt.reason
        });
    }
    Ok(())
}

#[no_mangle]
pub extern "C" fn doso_last_error(buf: *mut c_char, cap: usize) -> usize {
    LAST_ERROR.with(|slot| write_c_string(&slot.borrow(), buf, cap))
}

#[no_mangle]
pub unsafe extern "C" fn doso_robot_connect(
    endpoint: *const c_char,
    api_key: *const c_char,
    timeout_ms: u32,
) -> *mut DosoRobot {
    ffi_ptr(|| {
        let ep = required_str(endpoint, "endpoint")?;
        let key = optional_str(api_key)?;
        let mut config = RobotConfig::new(ep);
        config.api_key = key.map(str::to_owned);
        if timeout_ms > 0 {
            config.timeout = Duration::from_millis(u64::from(timeout_ms));
        }
        let client = runtime()
            .block_on(RobotClient::with_config(config))
            .map_err(map_sdk_err)?;
        Ok(Box::into_raw(Box::new(DosoRobot {
            client: Arc::new(client),
        })))
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_robot_free(robot: *mut DosoRobot) {
    if !robot.is_null() {
        drop(Box::from_raw(robot));
    }
}

#[no_mangle]
pub unsafe extern "C" fn doso_robot_ping(robot: *const DosoRobot) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let ping = runtime()
            .block_on(robot.client.system().ping())
            .map_err(map_sdk_err)?;
        if ping.status.is_empty() {
            return Err("empty ping status".to_owned());
        }
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_robot_id(
    robot: *const DosoRobot,
    buf: *mut c_char,
    cap: usize,
) -> usize {
    let Some(robot) = robot.as_ref() else {
        return 0;
    };
    write_c_string(
        robot
            .client
            .system()
            .info()
            .robot_id
            .as_deref()
            .unwrap_or(""),
        buf,
        cap,
    )
}

#[no_mangle]
pub unsafe extern "C" fn doso_firmware_version(
    robot: *const DosoRobot,
    buf: *mut c_char,
    cap: usize,
) -> usize {
    let Some(robot) = robot.as_ref() else {
        return 0;
    };
    write_c_string(
        robot
            .client
            .system()
            .info()
            .firmware_version
            .as_deref()
            .unwrap_or(""),
        buf,
        cap,
    )
}

#[no_mangle]
pub unsafe extern "C" fn doso_robot_version(
    robot: *const DosoRobot,
    buf: *mut c_char,
    cap: usize,
) -> usize {
    let Some(robot) = robot.as_ref() else {
        return 0;
    };
    write_c_string(
        robot
            .client
            .system()
            .info()
            .robot_version
            .as_deref()
            .unwrap_or(""),
        buf,
        cap,
    )
}

#[no_mangle]
pub unsafe extern "C" fn doso_low_level_endpoint(
    robot: *const DosoRobot,
    buf: *mut c_char,
    cap: usize,
) -> usize {
    let Some(robot) = robot.as_ref() else {
        return 0;
    };
    write_c_string(
        robot
            .client
            .system()
            .info()
            .low_level_endpoint
            .as_deref()
            .unwrap_or(""),
        buf,
        cap,
    )
}

#[no_mangle]
pub unsafe extern "C" fn doso_robot_capabilities(
    robot: *const DosoRobot,
    out_caps: *mut DosoCapabilities,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let out = out_caps.as_mut().ok_or("out_caps pointer is null")?;
        let snap = runtime()
            .block_on(robot.client.system().capabilities())
            .map_err(map_sdk_err)?;
        let empty = std::collections::BTreeMap::new();
        let fields = snap.fields.as_ref().map_or(&empty, |s| &s.fields);
        let motion = snapshot_bool(fields, "motion").unwrap_or(false);
        let low_level = snapshot_bool(fields, "low_level").unwrap_or(false);
        let observations = snapshot_bool(fields, "observations").unwrap_or(false);
        let navigation = snapshot_bool(fields, "navigation").unwrap_or(false);
        let maps = snapshot_bool(fields, "maps").unwrap_or(false);
        let localization = snapshot_bool(fields, "localization").unwrap_or(false);
        let tasks = snapshot_bool(fields, "tasks").unwrap_or(false);
        let board_b_autonomy =
            snapshot_bool(fields, "board_b_autonomy").unwrap_or(navigation || maps || localization);
        let voice = snapshot_bool(fields, "voice").unwrap_or(false);
        *out = DosoCapabilities {
            motion: u8::from(motion),
            low_level: u8::from(low_level),
            observations: u8::from(observations),
            navigation: u8::from(navigation),
            maps: u8::from(maps),
            localization: u8::from(localization),
            tasks: u8::from(tasks),
            board_b_autonomy: u8::from(board_b_autonomy),
            voice: u8::from(voice),
        };
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_enable_motors(robot: *const DosoRobot) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let receipt = runtime()
            .block_on(robot.client.sport().enable_motors(None))
            .map_err(map_sdk_err)?;
        map_receipt(receipt)
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_disable_motors(robot: *const DosoRobot) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let receipt = runtime()
            .block_on(robot.client.sport().disable_motors(None))
            .map_err(map_sdk_err)?;
        map_receipt(receipt)
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_stand_up(robot: *const DosoRobot) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let receipt = runtime()
            .block_on(robot.client.sport().stand_up(None))
            .map_err(map_sdk_err)?;
        map_receipt(receipt)
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_stand_down(robot: *const DosoRobot) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let receipt = runtime()
            .block_on(robot.client.sport().stand_down(None))
            .map_err(map_sdk_err)?;
        map_receipt(receipt)
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_damp(robot: *const DosoRobot) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let receipt = runtime()
            .block_on(robot.client.sport().damp(None))
            .map_err(map_sdk_err)?;
        map_receipt(receipt)
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_stop(robot: *const DosoRobot) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let receipt = runtime()
            .block_on(robot.client.sport().stop(None))
            .map_err(map_sdk_err)?;
        map_receipt(receipt)
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_switch_profile(
    robot: *const DosoRobot,
    name: *const c_char,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let name = required_str(name, "name")?;
        let receipt = runtime()
            .block_on(robot.client.sport().switch_profile(name, None))
            .map_err(map_sdk_err)?;
        map_receipt(receipt)
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_play_gesture(
    robot: *const DosoRobot,
    name: *const c_char,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let name = required_str(name, "name")?;
        let receipt = runtime()
            .block_on(robot.client.sport().play_gesture(name, None))
            .map_err(map_sdk_err)?;
        map_receipt(receipt)
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_clear_motor_fault(robot: *const DosoRobot) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let receipt = runtime()
            .block_on(robot.client.sport().clear_motor_fault([], None))
            .map_err(map_sdk_err)?;
        map_receipt(receipt)
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_clear_joint_fault(
    robot: *const DosoRobot,
    joint_id: u32,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let receipt = runtime()
            .block_on(robot.client.sport().clear_motor_fault([joint_id], None))
            .map_err(map_sdk_err)?;
        map_receipt(receipt)
    })
}

fn map_body_height_status(s: doso_sdk::BodyHeightStatus) -> DosoBodyHeightStatus {
    DosoBodyHeightStatus {
        supported: u8::from(s.supported),
        _pad: [0; 7],
        current_height_m: s.current_height_m,
        target_height_m: s.target_height_m,
        min_height_m: s.min_height_m,
        max_height_m: s.max_height_m,
        rate_mps: s.rate_mps,
    }
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_body_height_status(
    robot: *const DosoRobot,
    out_status: *mut DosoBodyHeightStatus,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let out = out_status.as_mut().ok_or("out_status pointer is null")?;
        let status = runtime()
            .block_on(robot.client.sport().body_height_status())
            .map_err(map_sdk_err)?;
        *out = map_body_height_status(status);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_set_body_height(
    robot: *const DosoRobot,
    height_m: f64,
    out_status: *mut DosoBodyHeightStatus,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let status = runtime()
            .block_on(robot.client.sport().set_body_height(height_m, None))
            .map_err(map_sdk_err)?;
        if let Some(out) = out_status.as_mut() {
            *out = map_body_height_status(status);
        }
        Ok(())
    })
}

fn recovery_status_to_json(s: &doso_sdk::RecoveryStatus) -> String {
    format!(
        "{{\"availability\":{},\"phase\":{},\"operation_id\":{},\"unavailable_reason\":{},\"selected\":{},\"session_token\":{},\"motor_disable_confirmed\":{}}}",
        s.availability,
        s.phase,
        json_quote(&s.operation_id),
        json_quote(&s.unavailable_reason),
        s.selected,
        json_quote(&s.session_token),
        s.motor_disable_confirmed,
    )
}

fn recovery_action_to_json(r: &doso_sdk::RecoveryActionReply) -> String {
    format!(
        "{{\"operation_id\":{},\"decision\":{},\"phase\":{},\"reason\":{}}}",
        json_quote(&r.operation_id),
        r.decision,
        r.phase,
        json_quote(&r.reason),
    )
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_recovery_status_json(
    robot: *const DosoRobot,
    buf: *mut c_char,
    cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let status = runtime()
            .block_on(robot.client.sport().recovery_status())
            .map_err(map_sdk_err)?;
        write_c_string(&recovery_status_to_json(&status), buf, cap);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_recovery_enter_json(
    robot: *const DosoRobot,
    operation_id: *const c_char,
    buf: *mut c_char,
    cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let operation_id = required_str(operation_id, "operation_id")?;
        let status = runtime()
            .block_on(robot.client.sport().recovery_enter(operation_id, None))
            .map_err(map_sdk_err)?;
        write_c_string(&recovery_status_to_json(&status), buf, cap);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_recovery_enable_json(
    robot: *const DosoRobot,
    session_token: *const c_char,
    buf: *mut c_char,
    cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let session_token = required_str(session_token, "session_token")?;
        let status = runtime()
            .block_on(robot.client.sport().recovery_enable(session_token, None))
            .map_err(map_sdk_err)?;
        write_c_string(&recovery_status_to_json(&status), buf, cap);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_recovery_heartbeat_json(
    robot: *const DosoRobot,
    session_token: *const c_char,
    buf: *mut c_char,
    cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let session_token = required_str(session_token, "session_token")?;
        let status = runtime()
            .block_on(robot.client.sport().recovery_heartbeat(session_token, None))
            .map_err(map_sdk_err)?;
        write_c_string(&recovery_status_to_json(&status), buf, cap);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_recovery_start_json(
    robot: *const DosoRobot,
    operation_id: *const c_char,
    session_token: *const c_char,
    buf: *mut c_char,
    cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let operation_id = required_str(operation_id, "operation_id")?;
        let session_token = required_str(session_token, "session_token")?;
        let reply = runtime()
            .block_on(
                robot
                    .client
                    .sport()
                    .recovery_start(operation_id, session_token, None),
            )
            .map_err(map_sdk_err)?;
        write_c_string(&recovery_action_to_json(&reply), buf, cap);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_recovery_cancel_json(
    robot: *const DosoRobot,
    operation_id: *const c_char,
    session_token: *const c_char,
    buf: *mut c_char,
    cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let operation_id = required_str(operation_id, "operation_id")?;
        let session_token = required_str(session_token, "session_token")?;
        let reply = runtime()
            .block_on(
                robot
                    .client
                    .sport()
                    .recovery_cancel(operation_id, session_token, None),
            )
            .map_err(map_sdk_err)?;
        write_c_string(&recovery_action_to_json(&reply), buf, cap);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_recovery_exit_json(
    robot: *const DosoRobot,
    session_token: *const c_char,
    buf: *mut c_char,
    cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let session_token = required_str(session_token, "session_token")?;
        let status = runtime()
            .block_on(robot.client.sport().recovery_exit(session_token, None))
            .map_err(map_sdk_err)?;
        write_c_string(&recovery_status_to_json(&status), buf, cap);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_control_status(
    robot: *const DosoRobot,
    out_status: *mut DosoBodyControlStatus,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let out = out_status.as_mut().ok_or("out_status pointer is null")?;
        let status = runtime()
            .block_on(robot.client.sport().control_status())
            .map_err(map_sdk_err)?;
        let body = status.body.unwrap_or_default();
        *out = DosoBodyControlStatus {
            body_kind: body.kind,
            body_transition: body.transition,
            source_owner: status.source_owner,
            motor_output_enabled: u8::from(status.motor_output_enabled),
            critical_motor_fault: u8::from(status.critical_motor_fault),
            grpc_lease_active: u8::from(status.grpc_lease_active),
            source_ready_for_walk: u8::from(status.source_ready_for_walk),
            lease_remaining_ms: status.lease_remaining_ms,
            _pad: 0,
            last_accepted_sequence: status.last_accepted_sequence,
            accepted_count: status.accepted_count,
            rejected_count: status.rejected_count,
        };
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_current_profile(
    robot: *const DosoRobot,
    buf: *mut c_char,
    cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let info = runtime()
            .block_on(robot.client.sport().profile())
            .map_err(map_sdk_err)?;
        write_c_string(&info.current, buf, cap);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_list_profiles(
    robot: *const DosoRobot,
    buf: *mut c_char,
    cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let info = runtime()
            .block_on(robot.client.sport().profile())
            .map_err(map_sdk_err)?;
        write_c_string(&info.available.join("\n"), buf, cap);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_sport_list_gestures(
    robot: *const DosoRobot,
    buf: *mut c_char,
    cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let list = runtime()
            .block_on(robot.client.sport().list_gestures())
            .map_err(map_sdk_err)?;
        let names: Vec<String> = list.gestures.into_iter().map(|g| g.name).collect();
        write_c_string(&names.join("\n"), buf, cap);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_observations_start_telemetry(
    robot: *const DosoRobot,
    imu: u8,
    joints: u8,
    stale_after_ms: u32,
) -> *mut DosoTelemetry {
    ffi_ptr(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        if imu == 0 && joints == 0 {
            return Err("at least one telemetry stream (imu or joints) must be enabled".to_owned());
        }
        let stale_after = Duration::from_millis(if stale_after_ms == 0 {
            500
        } else {
            u64::from(stale_after_ms)
        });
        let (imu_stream, joints_stream) = runtime().block_on(async {
            let imu_stream = if imu != 0 {
                Some(
                    robot
                        .client
                        .observations()
                        .watch_imu()
                        .await
                        .map_err(map_sdk_err)?,
                )
            } else {
                None
            };
            let joints_stream = if joints != 0 {
                Some(
                    robot
                        .client
                        .observations()
                        .watch_joints()
                        .await
                        .map_err(map_sdk_err)?,
                )
            } else {
                None
            };
            Ok::<_, String>((imu_stream, joints_stream))
        })?;

        let state = Arc::new(Mutex::new(TelemetryState::default()));
        {
            let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
            guard.imu.stream_active = imu_stream.is_some();
            guard.joints.stream_active = joints_stream.is_some();
        }
        let mut tasks = Vec::new();
        if let Some(mut stream) = imu_stream {
            let state_clone = Arc::clone(&state);
            tasks.push(runtime().spawn(async move {
                while let Ok(Some(sample)) = stream.message().await {
                    let mut guard = state_clone.lock().unwrap_or_else(|e| e.into_inner());
                    let gyro = sample.angular_velocity_rad_s.unwrap_or_default();
                    let accel = sample.linear_acceleration_m_s2.unwrap_or_default();
                    let quat = sample.orientation_world_to_body.unwrap_or_default();
                    guard.imu.available = true;
                    guard.imu.received_at = Some(Instant::now());
                    guard.imu.gyro_rad_s = [gyro.x, gyro.y, gyro.z];
                    guard.imu.accel_m_s2 = [accel.x, accel.y, accel.z];
                    guard.imu.quat_wxyz = [quat.w, quat.x, quat.y, quat.z];
                    guard.imu.source_elapsed_ns = duration_to_ns(sample.source_elapsed);
                }
                let mut guard = state_clone.lock().unwrap_or_else(|e| e.into_inner());
                guard.imu.stream_active = false;
            }));
        }
        if let Some(mut stream) = joints_stream {
            let state_clone = Arc::clone(&state);
            tasks.push(runtime().spawn(async move {
                while let Ok(Some(sample)) = stream.message().await {
                    let mut guard = state_clone.lock().unwrap_or_else(|e| e.into_inner());
                    let mut updated = false;
                    for joint in sample.joints {
                        if let Ok(idx) = usize::try_from(joint.id) {
                            if idx < 16 {
                                guard.joints.q[idx] = joint.position_rad;
                                guard.joints.dq[idx] = joint.velocity_rad_s;
                                guard.joints.tau[idx] = joint.torque_nm;
                                guard.joints.status_code[idx] = joint.status_code;
                                guard.joints.valid_mask |= 1u16 << idx;
                                updated = true;
                            }
                        }
                    }
                    if updated {
                        guard.joints.available = true;
                        guard.joints.received_at = Some(Instant::now());
                        guard.joints.source_elapsed_ns = duration_to_ns(sample.source_elapsed);
                    }
                }
                let mut guard = state_clone.lock().unwrap_or_else(|e| e.into_inner());
                guard.joints.stream_active = false;
            }));
        }
        Ok(Box::into_raw(Box::new(DosoTelemetry {
            state,
            stale_after,
            tasks,
        })))
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_observations_latest_imu(
    telemetry: *const DosoTelemetry,
    out_imu: *mut DosoImuSnapshot,
) -> i32 {
    ffi_status(|| {
        let handle = telemetry.as_ref().ok_or("telemetry handle is null")?;
        let out = out_imu.as_mut().ok_or("out_imu pointer is null")?;
        let guard = handle.state.lock().unwrap_or_else(|e| e.into_inner());
        let elapsed = guard.imu.received_at.map(|t| t.elapsed());
        let age_ms = elapsed.map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let fresh = guard.imu.available && elapsed.is_some_and(|d| d <= handle.stale_after);
        *out = DosoImuSnapshot {
            available: u8::from(guard.imu.available),
            fresh: u8::from(fresh),
            stream_active: u8::from(guard.imu.stream_active),
            _pad: [0; 5],
            age_ms,
            gyro_rad_s: guard.imu.gyro_rad_s,
            accel_m_s2: guard.imu.accel_m_s2,
            quat_wxyz: guard.imu.quat_wxyz,
            source_elapsed_ns: guard.imu.source_elapsed_ns,
        };
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_observations_latest_joints(
    telemetry: *const DosoTelemetry,
    out_joints: *mut DosoJointsSnapshot,
) -> i32 {
    ffi_status(|| {
        let handle = telemetry.as_ref().ok_or("telemetry handle is null")?;
        let out = out_joints.as_mut().ok_or("out_joints pointer is null")?;
        let guard = handle.state.lock().unwrap_or_else(|e| e.into_inner());
        let elapsed = guard.joints.received_at.map(|t| t.elapsed());
        let age_ms = elapsed.map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let fresh = guard.joints.available && elapsed.is_some_and(|d| d <= handle.stale_after);
        *out = DosoJointsSnapshot {
            available: u8::from(guard.joints.available),
            complete: u8::from(guard.joints.valid_mask == u16::MAX),
            fresh: u8::from(fresh),
            stream_active: u8::from(guard.joints.stream_active),
            valid_mask: guard.joints.valid_mask,
            _pad: 0,
            age_ms,
            q: guard.joints.q,
            dq: guard.joints.dq,
            tau: guard.joints.tau,
            status_code: guard.joints.status_code,
            source_elapsed_ns: guard.joints.source_elapsed_ns,
        };
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_observations_stop_telemetry(telemetry: *mut DosoTelemetry) {
    if !telemetry.is_null() {
        drop(Box::from_raw(telemetry));
    }
}

#[no_mangle]
pub unsafe extern "C" fn doso_observations_motors(
    robot: *const DosoRobot,
    out_motors: *mut DosoMotorsSnapshot,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let out = out_motors.as_mut().ok_or("out_motors pointer is null")?;
        let resp = runtime()
            .block_on(robot.client.observations().motors())
            .map_err(map_sdk_err)?;
        let mut snap = DosoMotorsSnapshot::default();
        let n = resp.motors.len().min(16);
        snap.count = n as u32;
        for (slot, m) in snap.motors.iter_mut().zip(resp.motors.into_iter().take(n)) {
            *slot = DosoMotorInfo {
                id: m.id,
                online: u8::from(m.online),
                _pad: [0; 3],
                status_code: m.status_code,
                error_count: m.errors.len() as u32,
                temperature_c: m.temperature_c,
            };
        }
        *out = snap;
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_observations_motor_voltages(
    robot: *const DosoRobot,
    out_volts16: *mut f64,
    out_count: *mut u32,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        if out_volts16.is_null() {
            return Err("out_volts16 pointer is null".to_owned());
        }
        let resp = runtime()
            .block_on(robot.client.observations().motor_voltages())
            .map_err(map_sdk_err)?;
        let dst = std::slice::from_raw_parts_mut(out_volts16, 16);
        dst.fill(0.0);
        let n = resp.volts.len().min(16);
        dst[..n].copy_from_slice(&resp.volts[..n]);
        if let Some(count_ptr) = out_count.as_mut() {
            *count_ptr = n as u32;
        }
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_teleop_take_control(robot: *const DosoRobot) -> *mut DosoTeleop {
    ffi_ptr(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let owner = Arc::clone(&robot.client);
        let ptr: *const RobotClient = Arc::as_ptr(&owner);
        // SAFETY: `owner` keeps the `RobotClient` pinned at `ptr`; `session` drops before `_owner`.
        let session: Teleop<'static> = runtime()
            .block_on((*ptr).sport().take_control(None))
            .map_err(map_sdk_err)?;
        Ok(Box::into_raw(Box::new(DosoTeleop {
            session: Some(session),
            _owner: owner,
        })))
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_teleop_send_velocity(
    teleop: *mut DosoTeleop,
    vx_mps: f64,
    vy_mps: f64,
    yaw_rps: f64,
    deadman: u8,
) -> i32 {
    ffi_status(|| {
        let handle = teleop.as_mut().ok_or("teleop handle is null")?;
        let session = handle
            .session
            .as_mut()
            .ok_or("teleop session is already closed")?;
        let reply = runtime()
            .block_on(session.send_velocity(
                TeleopVelocity {
                    vx_m_s: vx_mps,
                    vy_m_s: vy_mps,
                    yaw_rad_s: yaw_rps,
                    deadman: deadman != 0,
                    manual_mode: false,
                },
                None,
            ))
            .map_err(map_sdk_err)?;
        if !reply.accepted {
            return Err(reply.reason);
        }
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_teleop_publish_velocity(
    teleop: *mut DosoTeleop,
    vx_mps: f64,
    vy_mps: f64,
    yaw_rps: f64,
    deadman: u8,
) -> i32 {
    ffi_status(|| {
        let handle = teleop.as_mut().ok_or("teleop handle is null")?;
        let session = handle
            .session
            .as_mut()
            .ok_or("teleop session is already closed")?;
        session
            .publish_velocity(
                TeleopVelocity {
                    vx_m_s: vx_mps,
                    vy_m_s: vy_mps,
                    yaw_rad_s: yaw_rps,
                    deadman: deadman != 0,
                    manual_mode: false,
                },
                None,
            )
            .map(|_| ())
            .map_err(map_sdk_err)
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_teleop_release(teleop: *mut DosoTeleop) -> i32 {
    ffi_status(|| {
        let handle = teleop.as_mut().ok_or("teleop handle is null")?;
        let session = handle
            .session
            .take()
            .ok_or("teleop session is already closed")?;
        let reply = runtime()
            .block_on(session.release_control(None))
            .map_err(map_sdk_err)?;
        if !reply.accepted {
            return Err(reply.reason);
        }
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_teleop_free(teleop: *mut DosoTeleop) {
    if !teleop.is_null() {
        drop(Box::from_raw(teleop));
    }
}

#[no_mangle]
pub unsafe extern "C" fn doso_low_level_enter(robot: *const DosoRobot) -> *mut DosoLowLevel {
    ffi_ptr(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let owner = Arc::clone(&robot.client);
        let ptr: *const RobotClient = Arc::as_ptr(&owner);
        // SAFETY: `owner` keeps the `RobotClient` pinned at `ptr`; `session` drops before `_owner`.
        let session: ExternalControl<'static> = runtime()
            .block_on((*ptr).low_level(None))
            .map_err(map_sdk_err)?;
        Ok(Box::into_raw(Box::new(DosoLowLevel {
            session: Some(session),
            _owner: owner,
        })))
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_low_level_enable(session_ptr: *mut DosoLowLevel) -> i32 {
    ffi_status(|| {
        let handle = session_ptr.as_mut().ok_or("low_level handle is null")?;
        let session = handle
            .session
            .as_mut()
            .ok_or("low_level session is closed")?;
        let reply = runtime()
            .block_on(session.enable(None))
            .map_err(map_sdk_err)?;
        if !reply.accepted {
            return Err(reply.reason);
        }
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_low_level_disable(session_ptr: *mut DosoLowLevel) -> i32 {
    ffi_status(|| {
        let handle = session_ptr.as_mut().ok_or("low_level handle is null")?;
        let session = handle
            .session
            .as_mut()
            .ok_or("low_level session is closed")?;
        let reply = runtime()
            .block_on(session.disable(None))
            .map_err(map_sdk_err)?;
        if !reply.accepted {
            return Err(reply.reason);
        }
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_low_level_publish_joints16(
    session_ptr: *mut DosoLowLevel,
    joints16: *const DosoJointCmd,
) -> i32 {
    ffi_status(|| {
        let handle = session_ptr.as_mut().ok_or("low_level handle is null")?;
        if joints16.is_null() {
            return Err("joints16 pointer is null".to_owned());
        }
        let session = handle
            .session
            .as_mut()
            .ok_or("low_level session is closed")?;
        let slice = std::slice::from_raw_parts(joints16, 16);
        let cmds: Vec<JointCommand> = slice
            .iter()
            .map(|c| JointCommand {
                q: c.q,
                dq: c.dq,
                kp: c.kp,
                kd: c.kd,
                tau: c.tau,
            })
            .collect();
        session
            .publish_joints(cmds, None)
            .map(|_| ())
            .map_err(map_sdk_err)
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_low_level_get_state(
    session_ptr: *const DosoLowLevel,
    out_state: *mut DosoControllerSnapshot,
) -> i32 {
    ffi_status(|| {
        let handle = session_ptr.as_ref().ok_or("low_level handle is null")?;
        let out = out_state.as_mut().ok_or("out_state pointer is null")?;
        let session = handle
            .session
            .as_ref()
            .ok_or("low_level session is closed")?;
        let state = session
            .state()
            .map_err(map_sdk_err)?
            .ok_or("controller state missing")?;
        let remote = state.remote.as_ref();
        *out = DosoControllerSnapshot {
            mode: state.mode,
            active: u8::from(state.active),
            motor_output_enabled: u8::from(state.motor_output_enabled),
            fresh: u8::from(state.fresh),
            remote_active: u8::from(remote.is_some_and(|r| r.active)),
            last_sequence: state.last_sequence,
            joint_count: state.joint_count,
            remote_buttons: remote.map_or(0, |r| r.buttons),
            remote_lx: remote.map_or(0.0, |r| r.lx),
            remote_ly: remote.map_or(0.0, |r| r.ly),
            remote_rx: remote.map_or(0.0, |r| r.rx),
            remote_ry: remote.map_or(0.0, |r| r.ry),
        };
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_low_level_close(session_ptr: *mut DosoLowLevel) -> i32 {
    ffi_status(|| {
        let handle = session_ptr.as_mut().ok_or("low_level handle is null")?;
        let session = handle.session.take().ok_or("low_level session is closed")?;
        let reply = runtime()
            .block_on(session.close(None))
            .map_err(map_sdk_err)?;
        if !reply.accepted {
            return Err(reply.reason);
        }
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_low_level_free(session_ptr: *mut DosoLowLevel) {
    if !session_ptr.is_null() {
        drop(Box::from_raw(session_ptr));
    }
}

#[no_mangle]
pub unsafe extern "C" fn doso_nav_go_to(
    robot: *const DosoRobot,
    x: f64,
    y: f64,
    z: f64,
    yaw: f64,
    task_id_buf: *mut c_char,
    task_id_cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let receipt = runtime()
            .block_on(robot.client.navigation().go_to(Pose { x, y, z, yaw }, None))
            .map_err(map_sdk_err)?;
        map_receipt(receipt.clone())?;
        if let Some(task_id) = receipt.task_id {
            write_c_string(&task_id, task_id_buf, task_id_cap);
        } else if !task_id_buf.is_null() && task_id_cap > 0 {
            *task_id_buf = 0;
        }
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_nav_cancel(
    robot: *const DosoRobot,
    task_id: *const c_char,
    reason: *const c_char,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let task_id = required_str(task_id, "task_id")?;
        let reason = optional_str(reason)?.unwrap_or("cancelled");
        let receipt = runtime()
            .block_on(robot.client.navigation().cancel(task_id, reason, None))
            .map_err(map_sdk_err)?;
        map_receipt(receipt)
    })
}

fn json_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[no_mangle]
pub unsafe extern "C" fn doso_voice_status_json(
    robot: *const DosoRobot,
    buf: *mut c_char,
    cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let st = runtime()
            .block_on(robot.client.voice().status())
            .map_err(map_sdk_err)?;
        let json = format!(
            "{{\"status\":{},\"service\":{},\"active_profile\":{},\"llm_provider\":{},\"llm_model\":{},\"asr_provider\":{},\"tts_provider\":{}}}",
            json_quote(&st.status),
            json_quote(&st.service),
            json_quote(&st.active_profile),
            json_quote(&st.llm_provider),
            json_quote(&st.llm_model),
            json_quote(&st.asr_provider),
            json_quote(&st.tts_provider),
        );
        write_c_string(&json, buf, cap);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_voice_profiles_json(
    robot: *const DosoRobot,
    buf: *mut c_char,
    cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let resp = runtime()
            .block_on(robot.client.voice().list_profiles())
            .map_err(map_sdk_err)?;
        let items: Vec<String> = resp
            .profiles
            .iter()
            .map(|p| {
                format!(
                    "{{\"profile_id\":{},\"label\":{},\"voice_id\":{},\"category\":{},\"speed\":{},\"volume\":{},\"pitch\":{},\"emotion\":{}}}",
                    json_quote(&p.profile_id),
                    json_quote(&p.label),
                    json_quote(&p.voice_id),
                    json_quote(&p.category),
                    p.speed,
                    p.volume,
                    p.pitch,
                    json_quote(&p.emotion),
                )
            })
            .collect();
        let cues: Vec<String> = resp
            .available_sound_cues
            .iter()
            .map(|c| json_quote(c))
            .collect();
        let json = format!(
            "{{\"active_profile\":{},\"sound_cues_enabled\":{},\"available_sound_cues\":[{}],\"profiles\":[{}]}}",
            json_quote(&resp.active_profile),
            if resp.sound_cues_enabled { "true" } else { "false" },
            cues.join(","),
            items.join(","),
        );
        write_c_string(&json, buf, cap);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_voice_switch_profile(
    robot: *const DosoRobot,
    profile_id: *const c_char,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let profile_id = required_str(profile_id, "profile_id")?;
        let receipt = runtime()
            .block_on(robot.client.voice().switch_profile(profile_id, None))
            .map_err(map_sdk_err)?;
        map_receipt(receipt)
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_voice_speak(
    robot: *const DosoRobot,
    text: *const c_char,
    priority: *const c_char,
    queue_policy: *const c_char,
    voice_profile_id: *const c_char,
    playback_id_buf: *mut c_char,
    playback_id_cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let text = required_str(text, "text")?;
        let opts = VoiceSpeakOptions {
            priority: optional_str(priority)?.unwrap_or("normal").to_owned(),
            queue_policy: optional_str(queue_policy)?.unwrap_or("enqueue").to_owned(),
            voice_profile_id: optional_str(voice_profile_id)?.unwrap_or("").to_owned(),
            ..VoiceSpeakOptions::default()
        };
        let pb = runtime()
            .block_on(robot.client.voice().speak(text, opts, None))
            .map_err(map_sdk_err)?;
        write_c_string(&pb.playback_id, playback_id_buf, playback_id_cap);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_voice_playback_status_json(
    robot: *const DosoRobot,
    playback_id: *const c_char,
    buf: *mut c_char,
    cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let playback_id = required_str(playback_id, "playback_id")?;
        let pb = runtime()
            .block_on(robot.client.voice().playback_status(playback_id))
            .map_err(map_sdk_err)?;
        let json = format!(
            "{{\"playback_id\":{},\"state\":{},\"priority\":{},\"text_chars\":{},\"message\":{}}}",
            json_quote(&pb.playback_id),
            json_quote(&pb.state),
            json_quote(&pb.priority),
            pb.text_chars,
            json_quote(&pb.message),
        );
        write_c_string(&json, buf, cap);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_voice_cancel_playback(
    robot: *const DosoRobot,
    playback_id: *const c_char,
    reason: *const c_char,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let playback_id = required_str(playback_id, "playback_id")?;
        let reason = optional_str(reason)?.unwrap_or("operator_cancelled");
        let _ = runtime()
            .block_on(robot.client.voice().cancel_playback(playback_id, reason, None))
            .map_err(map_sdk_err)?;
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_voice_switch_component(
    robot: *const DosoRobot,
    component: *const c_char,
    provider: *const c_char,
    model: *const c_char,
    validate: u8,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let component = required_str(component, "component")?;
        let provider = required_str(provider, "provider")?;
        let model = optional_str(model)?;
        let receipt = runtime()
            .block_on(
                robot
                    .client
                    .voice()
                    .switch_component(component, provider, model, validate != 0, None),
            )
            .map_err(map_sdk_err)?;
        map_receipt(receipt)
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_voice_update_prompt(
    robot: *const DosoRobot,
    system_prompt: *const c_char,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let system_prompt = required_str(system_prompt, "system_prompt")?;
        let receipt = runtime()
            .block_on(robot.client.voice().update_prompt(system_prompt, None))
            .map_err(map_sdk_err)?;
        map_receipt(receipt)
    })
}

#[no_mangle]
pub unsafe extern "C" fn doso_voice_chat_json(
    robot: *const DosoRobot,
    text: *const c_char,
    thread_id: *const c_char,
    speak_reply: u8,
    buf: *mut c_char,
    cap: usize,
) -> i32 {
    ffi_status(|| {
        let robot = robot.as_ref().ok_or("robot handle is null")?;
        let text = required_str(text, "text")?;
        let thread_id = optional_str(thread_id)?;
        let reply = runtime()
            .block_on(
                robot
                    .client
                    .voice()
                    .chat(text, thread_id, speak_reply != 0, None),
            )
            .map_err(map_sdk_err)?;
        let json = format!(
            "{{\"reply_text\":{},\"thread_id\":{},\"turn_id\":{},\"latency_ms\":{}}}",
            json_quote(&reply.reply_text),
            json_quote(&reply.thread_id),
            json_quote(&reply.turn_id),
            reply.latency_ms,
        );
        write_c_string(&json, buf, cap);
        Ok(())
    })
}

#[cfg(any(test, feature = "test-mock"))]
mod mock_server {
    use super::*;
    use doso_sdk::rpc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::net::TcpListener;
    use tokio::sync::{mpsc, oneshot};
    use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
    use tonic::{Request, Response, Status};

    #[derive(Clone, Default)]
    struct MockSvc {
        writes: Arc<AtomicU64>,
    }

    impl MockSvc {
        fn ok_receipt(&self, command: Option<rpc::Command>, stage: &str) -> rpc::Receipt {
            self.writes.fetch_add(1, Ordering::Relaxed);
            rpc::Receipt {
                ok: true,
                accepted: Some(true),
                request_id: command.map_or_else(String::new, |c| c.request_id),
                task_id: Some("mock-task-1".into()),
                stage: stage.into(),
                ..Default::default()
            }
        }
    }

    #[tonic::async_trait]
    impl rpc::system_server::System for MockSvc {
        async fn handshake(
            &self,
            _: Request<rpc::Handshake>,
        ) -> Result<Response<rpc::HandshakeReply>, Status> {
            Ok(Response::new(rpc::HandshakeReply {
                protocol_version: 2,
                info: Some(rpc::SystemInfo {
                    firmware_version: Some("doso-ffi-test".into()),
                    robot_version: Some("thunder-16".into()),
                    robot_id: Some("8f7e1c62-0a4d-4b89-96e2-6c47b03db1a5".into()),
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
            Ok(Response::new(rpc::RobotStatus::default()))
        }
        async fn health(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
            Ok(Response::new(rpc::Snapshot::default()))
        }
        async fn faults(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::FaultReport>, Status> {
            Ok(Response::new(rpc::FaultReport::default()))
        }
        type WatchFaultsStream = ReceiverStream<Result<rpc::FaultReport, Status>>;
        async fn watch_faults(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<Self::WatchFaultsStream>, Status> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(Response::new(ReceiverStream::new(rx)))
        }
        async fn readiness(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::Snapshot>, Status> {
            Ok(Response::new(rpc::Snapshot::default()))
        }
        async fn capabilities(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::Snapshot>, Status> {
            let mut fields = std::collections::BTreeMap::new();
            for key in [
                "motion",
                "low_level",
                "observations",
                "navigation",
                "maps",
                "localization",
                "tasks",
                "board_b_autonomy",
                "voice",
            ] {
                fields.insert(
                    key.to_owned(),
                    prost_types::Value {
                        kind: Some(prost_types::value::Kind::BoolValue(true)),
                    },
                );
            }
            Ok(Response::new(rpc::Snapshot {
                fields: Some(prost_types::Struct { fields }),
            }))
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

    #[tonic::async_trait]
    impl rpc::motion_server::Motion for MockSvc {
        async fn controller(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::ControllerState>, Status> {
            Ok(Response::new(rpc::ControllerState::default()))
        }
        type ExternalControlStream = ReceiverStream<Result<rpc::ExternalControlReply, Status>>;
        async fn external_control(
            &self,
            request: Request<tonic::Streaming<rpc::ExternalControlRequest>>,
        ) -> Result<Response<Self::ExternalControlStream>, Status> {
            let mut inbound = request.into_inner();
            let (tx, rx) = mpsc::channel(8);
            let writes = Arc::clone(&self.writes);
            tokio::spawn(async move {
                let mut enabled = false;
                let mut last_seq = 0;
                while let Ok(Some(req)) = inbound.message().await {
                    let close = matches!(
                        req.action,
                        Some(rpc::external_control_request::Action::Close(_))
                    );
                    let heartbeat = matches!(
                        req.action,
                        Some(rpc::external_control_request::Action::Heartbeat(_))
                    );
                    if let Some(rpc::external_control_request::Action::Frame(frame)) = req.action {
                        if frame.joints.len() == 16 {
                            writes.fetch_add(1, Ordering::Relaxed);
                            last_seq = req.sequence;
                        }
                    } else if let Some(rpc::external_control_request::Action::Enable(_)) =
                        req.action
                    {
                        enabled = true;
                    } else if matches!(
                        req.action,
                        Some(rpc::external_control_request::Action::Disable(_))
                            | Some(rpc::external_control_request::Action::Close(_))
                    ) {
                        enabled = false;
                    }
                    let reply = rpc::ExternalControlReply {
                        request_id: if heartbeat {
                            String::new()
                        } else {
                            req.command.map_or_else(String::new, |c| c.request_id)
                        },
                        sequence: req.sequence,
                        accepted: true,
                        reason: String::new(),
                        ingress_accepted: true,
                        input_window: if close { 0 } else { 42 },
                        state: Some(rpc::ControllerState {
                            mode: if close {
                                rpc::ControllerMode::Builtin
                            } else {
                                rpc::ControllerMode::External
                            } as i32,
                            session_id: "ffi-ext".into(),
                            active: !close,
                            motor_output_enabled: enabled,
                            last_sequence: last_seq,
                            reason: "ok".into(),
                            received_age_ms: Some(1.0),
                            fresh: true,
                            motor_disable_confirmed: !enabled,
                            joint_count: 16,
                            remote: Some(rpc::RemoteState {
                                lx: 0.25,
                                ly: -0.1,
                                rx: 0.0,
                                ry: 0.5,
                                buttons: 3,
                                active: true,
                            }),
                        }),
                    };
                    if tx.send(Ok(reply)).await.is_err() || close {
                        break;
                    }
                }
            });
            Ok(Response::new(ReceiverStream::new(rx)))
        }
        async fn stand_up(
            &self,
            req: Request<rpc::Command>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(Some(req.into_inner()), "stand"),
            ))
        }
        async fn sit_down(
            &self,
            req: Request<rpc::Command>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(Some(req.into_inner()), "sit"),
            ))
        }
        async fn damp(&self, req: Request<rpc::Command>) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(Some(req.into_inner()), "damp"),
            ))
        }
        async fn enable_motors(
            &self,
            req: Request<rpc::Command>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(Some(req.into_inner()), "enable"),
            ))
        }
        async fn disable_motors(
            &self,
            req: Request<rpc::Command>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(Some(req.into_inner()), "disable"),
            ))
        }
        async fn clear_motor_fault(
            &self,
            req: Request<rpc::ClearMotorFaultRequest>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(req.into_inner().command, "fault"),
            ))
        }
        async fn list_gestures(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::GestureList>, Status> {
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
            req: Request<rpc::PlayGestureRequest>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(req.into_inner().command, "gesture"),
            ))
        }
        async fn profile(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::ProfileInfo>, Status> {
            Ok(Response::new(rpc::ProfileInfo {
                current: "flat".into(),
                available: vec!["flat".into(), "stairs".into()],
                descriptions: vec!["Flat terrain".into(), "Stairs".into()],
                current_description: "Flat terrain".into(),
            }))
        }
        async fn switch_profile(
            &self,
            req: Request<rpc::SwitchProfileRequest>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(req.into_inner().command, "profile"),
            ))
        }
        async fn body_height_status(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::BodyHeightStatus>, Status> {
            Ok(Response::new(rpc::BodyHeightStatus {
                supported: true,
                current_height_m: 0.375,
                target_height_m: 0.375,
                min_height_m: 0.25,
                max_height_m: 0.50,
                rate_mps: 0.05,
            }))
        }
        async fn set_body_height(
            &self,
            req: Request<rpc::SetBodyHeightRequest>,
        ) -> Result<Response<rpc::BodyHeightStatus>, Status> {
            let inner = req.into_inner();
            if !inner.height_m.is_finite() || !(0.25..=0.50).contains(&inner.height_m) {
                return Err(Status::invalid_argument(
                    "height_m is outside the active profile range",
                ));
            }
            self.writes.fetch_add(1, Ordering::Relaxed);
            Ok(Response::new(rpc::BodyHeightStatus {
                supported: true,
                current_height_m: inner.height_m,
                target_height_m: inner.height_m,
                min_height_m: 0.25,
                max_height_m: 0.50,
                rate_mps: 0.05,
            }))
        }
        async fn recovery_status(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::RecoveryStatus>, Status> {
            Ok(Response::new(rpc::RecoveryStatus {
                availability: 2,
                phase: 1,
                ..Default::default()
            }))
        }
        async fn recovery_enter(
            &self,
            req: Request<rpc::RecoveryStartRequest>,
        ) -> Result<Response<rpc::RecoveryStatus>, Status> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            Ok(Response::new(rpc::RecoveryStatus {
                availability: 2,
                phase: 2,
                operation_id: req.into_inner().operation_id,
                selected: true,
                session_token: "rec-mock-tok".into(),
                ..Default::default()
            }))
        }
        async fn recovery_enable(
            &self,
            _: Request<rpc::RecoverySessionRequest>,
        ) -> Result<Response<rpc::RecoveryStatus>, Status> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            Ok(Response::new(rpc::RecoveryStatus {
                availability: 2,
                phase: 3,
                selected: true,
                ..Default::default()
            }))
        }
        async fn recovery_heartbeat(
            &self,
            _: Request<rpc::RecoverySessionRequest>,
        ) -> Result<Response<rpc::RecoveryStatus>, Status> {
            Ok(Response::new(rpc::RecoveryStatus {
                availability: 2,
                phase: 3,
                selected: true,
                ..Default::default()
            }))
        }
        async fn recovery_start(
            &self,
            req: Request<rpc::RecoveryStartRequest>,
        ) -> Result<Response<rpc::RecoveryActionReply>, Status> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            Ok(Response::new(rpc::RecoveryActionReply {
                operation_id: req.into_inner().operation_id,
                decision: 1,
                phase: 4,
                reason: String::new(),
            }))
        }
        async fn recovery_cancel(
            &self,
            req: Request<rpc::RecoveryCancelRequest>,
        ) -> Result<Response<rpc::RecoveryActionReply>, Status> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            Ok(Response::new(rpc::RecoveryActionReply {
                operation_id: req.into_inner().operation_id,
                decision: 1,
                phase: 1,
                reason: "Motor disable confirmed".into(),
            }))
        }
        async fn recovery_exit(
            &self,
            _: Request<rpc::RecoverySessionRequest>,
        ) -> Result<Response<rpc::RecoveryStatus>, Status> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            Ok(Response::new(rpc::RecoveryStatus {
                availability: 2,
                phase: 1,
                selected: false,
                motor_disable_confirmed: true,
                ..Default::default()
            }))
        }
        async fn stop(&self, req: Request<rpc::Command>) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(Some(req.into_inner()), "stop"),
            ))
        }
        async fn stop_motion(
            &self,
            req: Request<rpc::Command>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(Some(req.into_inner()), "stop_motion"),
            ))
        }
        async fn clear_emergency_stop(
            &self,
            req: Request<rpc::Command>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(Some(req.into_inner()), "clear_estop"),
            ))
        }
        async fn resume(
            &self,
            req: Request<rpc::Command>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(Some(req.into_inner()), "resume"),
            ))
        }
        async fn control_status(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::BodyControlStatus>, Status> {
            Ok(Response::new(rpc::BodyControlStatus {
                body: Some(rpc::BodyState {
                    kind: rpc::BodyKind::Grounded as i32,
                    transition: rpc::BodyTransition::None as i32,
                    gesture_name: String::new(),
                }),
                motor_output_enabled: false,
                source_owner: rpc::BodyControlOwner::None as i32,
                critical_motor_fault: false,
                grpc_lease_active: false,
                lease_remaining_ms: 0,
                last_accepted_sequence: 7,
                source_last_decision: 0,
                source_ready_for_walk: false,
                source_owner_id: String::new(),
                accepted_count: 11,
                rejected_count: 0,
            }))
        }
        async fn control_state(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::ControlState>, Status> {
            Ok(Response::new(rpc::ControlState::default()))
        }
        type WatchControlStateStream = ReceiverStream<Result<rpc::ControlState, Status>>;
        async fn watch_control_state(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<Self::WatchControlStateStream>, Status> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(Response::new(ReceiverStream::new(rx)))
        }
        async fn velocity(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::VelocitySample>, Status> {
            Ok(Response::new(rpc::VelocitySample::default()))
        }
        type WatchVelocityStream = ReceiverStream<Result<rpc::VelocitySample, Status>>;
        async fn watch_velocity(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<Self::WatchVelocityStream>, Status> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(Response::new(ReceiverStream::new(rx)))
        }
        async fn odometry(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::OdometrySample>, Status> {
            Ok(Response::new(rpc::OdometrySample::default()))
        }
        type WatchOdometryStream = ReceiverStream<Result<rpc::OdometrySample, Status>>;
        async fn watch_odometry(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<Self::WatchOdometryStream>, Status> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(Response::new(ReceiverStream::new(rx)))
        }
        async fn status(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
            Ok(Response::new(rpc::Snapshot::default()))
        }
        type WatchStateStream = ReceiverStream<Result<rpc::Snapshot, Status>>;
        async fn watch_state(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<Self::WatchStateStream>, Status> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(Response::new(ReceiverStream::new(rx)))
        }
        type WatchStateAtStream = ReceiverStream<Result<rpc::Snapshot, Status>>;
        async fn watch_state_at(
            &self,
            _: Request<rpc::WatchStateRequest>,
        ) -> Result<Response<Self::WatchStateAtStream>, Status> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(Response::new(ReceiverStream::new(rx)))
        }
        async fn body_state(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::BodyState>, Status> {
            Ok(Response::new(rpc::BodyState::default()))
        }
        type WatchBodyStateStream = ReceiverStream<Result<rpc::BodyState, Status>>;
        async fn watch_body_state(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<Self::WatchBodyStateStream>, Status> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(Response::new(ReceiverStream::new(rx)))
        }
        type TeleopStream = ReceiverStream<Result<rpc::TeleopReply, Status>>;
        async fn teleop(
            &self,
            request: Request<tonic::Streaming<rpc::TeleopRequest>>,
        ) -> Result<Response<Self::TeleopStream>, Status> {
            let mut inbound = request.into_inner();
            let (tx, rx) = mpsc::channel(8);
            let writes = Arc::clone(&self.writes);
            tokio::spawn(async move {
                while let Ok(Some(req)) = inbound.message().await {
                    let close = matches!(req.action, Some(rpc::teleop_request::Action::Close(_)));
                    if matches!(req.action, Some(rpc::teleop_request::Action::Velocity(_))) {
                        writes.fetch_add(1, Ordering::Relaxed);
                    }
                    let reply = rpc::TeleopReply {
                        request_id: req.command.map_or_else(String::new, |c| c.request_id),
                        accepted: true,
                        reason: "ok".into(),
                        input_window: 9,
                        ingress_accepted: true,
                        final_output_confirmed: true,
                        sequence: req.sequence,
                        control: Some(rpc::ControlState {
                            held_by_client: Some(!close),
                            fresh: true,
                            ..Default::default()
                        }),
                    };
                    if tx.send(Ok(reply)).await.is_err() || close {
                        break;
                    }
                }
            });
            Ok(Response::new(ReceiverStream::new(rx)))
        }
    }

    #[tonic::async_trait]
    impl rpc::navigation_server::Navigation for MockSvc {
        async fn go(&self, req: Request<rpc::Goal>) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(req.into_inner().command, "nav_go"),
            ))
        }
        async fn go_with_options(
            &self,
            req: Request<rpc::Goal>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(req.into_inner().command, "nav_go"),
            ))
        }
        async fn preview_plan(
            &self,
            _: Request<rpc::PlanQuery>,
        ) -> Result<Response<rpc::PlanPreview>, Status> {
            Ok(Response::new(rpc::PlanPreview::default()))
        }
        async fn set_speed_limit(
            &self,
            req: Request<rpc::TaskSpeedLimit>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(req.into_inner().command, "limit"),
            ))
        }
        async fn status(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
            Ok(Response::new(rpc::Snapshot::default()))
        }
        async fn path(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
            Ok(Response::new(rpc::Snapshot::default()))
        }
        async fn route(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::NavigationPath>, Status> {
            Ok(Response::new(rpc::NavigationPath::default()))
        }
        type WatchPathStream = ReceiverStream<Result<rpc::NavigationPath, Status>>;
        async fn watch_path(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<Self::WatchPathStream>, Status> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(Response::new(ReceiverStream::new(rx)))
        }
        async fn task(&self, _: Request<rpc::TaskQuery>) -> Result<Response<rpc::Task>, Status> {
            Ok(Response::new(rpc::Task::default()))
        }
        async fn cancel(
            &self,
            req: Request<rpc::CancelTask>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(req.into_inner().command, "cancel"),
            ))
        }
        async fn pause(
            &self,
            req: Request<rpc::TaskControl>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(req.into_inner().command, "pause"),
            ))
        }
        async fn resume(
            &self,
            req: Request<rpc::TaskControl>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(req.into_inner().command, "resume"),
            ))
        }
        type WatchStream = ReceiverStream<Result<rpc::Task, Status>>;
        async fn watch(
            &self,
            _: Request<rpc::TaskQuery>,
        ) -> Result<Response<Self::WatchStream>, Status> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(Response::new(ReceiverStream::new(rx)))
        }
    }

    #[tonic::async_trait]
    impl rpc::observations_server::Observations for MockSvc {
        async fn scene(&self, _: Request<rpc::Empty>) -> Result<Response<rpc::Snapshot>, Status> {
            Ok(Response::new(rpc::Snapshot::default()))
        }
        async fn camera_status(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::Snapshot>, Status> {
            Ok(Response::new(rpc::Snapshot::default()))
        }
        async fn camera_calibration(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::CameraCalibration>, Status> {
            Ok(Response::new(rpc::CameraCalibration::default()))
        }
        type WatchImuStream = ReceiverStream<Result<rpc::ImuSample, Status>>;
        async fn watch_imu(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<Self::WatchImuStream>, Status> {
            let (tx, rx) = mpsc::channel(8);
            tokio::spawn(async move {
                let mut tick = 0i64;
                loop {
                    tick += 1;
                    let sample = rpc::ImuSample {
                        angular_velocity_rad_s: Some(rpc::Vector3 {
                            x: 0.01,
                            y: -0.02,
                            z: 0.03,
                        }),
                        orientation_world_to_body: Some(rpc::Quaternion {
                            w: 1.0,
                            x: 0.0,
                            y: 0.0,
                            z: 0.0,
                        }),
                        linear_acceleration_m_s2: Some(rpc::Vector3 {
                            x: 0.0,
                            y: 0.0,
                            z: 9.81,
                        }),
                        source_elapsed: Some(prost_types::Duration {
                            seconds: 1,
                            nanos: (tick * 10_000_000) as i32,
                        }),
                    };
                    if tx.send(Ok(sample)).await.is_err() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            });
            Ok(Response::new(ReceiverStream::new(rx)))
        }
        type WatchJointsStream = ReceiverStream<Result<rpc::JointSample, Status>>;
        async fn watch_joints(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<Self::WatchJointsStream>, Status> {
            let (tx, rx) = mpsc::channel(8);
            tokio::spawn(async move {
                let mut tick = 0i64;
                loop {
                    tick += 1;
                    let joints = (0..16u32)
                        .map(|id| rpc::JointState {
                            id,
                            position_rad: 0.05 * f64::from(id),
                            velocity_rad_s: 0.1 * f64::from(id),
                            torque_nm: 0.25,
                            status_code: 0,
                        })
                        .collect();
                    let sample = rpc::JointSample {
                        joints,
                        source_elapsed: Some(prost_types::Duration {
                            seconds: 2,
                            nanos: (tick * 10_000_000) as i32,
                        }),
                    };
                    if tx.send(Ok(sample)).await.is_err() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            });
            Ok(Response::new(ReceiverStream::new(rx)))
        }
        async fn motors(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::MotorStates>, Status> {
            let motors = (0..16u32)
                .map(|id| rpc::MotorState {
                    id,
                    online: true,
                    status_code: 0,
                    temperature_c: 28.5,
                    errors: Vec::new(),
                })
                .collect();
            Ok(Response::new(rpc::MotorStates { motors }))
        }
        async fn motor_voltages(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::MotorVoltagesResponse>, Status> {
            Ok(Response::new(rpc::MotorVoltagesResponse {
                volts: vec![50.2; 16],
            }))
        }
        async fn camera_frame(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::CameraFrame>, Status> {
            Err(Status::unimplemented("camera mock unimplemented"))
        }
        type WatchCameraStream = ReceiverStream<Result<rpc::CameraFrame, Status>>;
        async fn watch_camera(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<Self::WatchCameraStream>, Status> {
            Err(Status::unimplemented("camera mock unimplemented"))
        }
        async fn camera_sample(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::CameraSample>, Status> {
            Err(Status::unimplemented("camera mock unimplemented"))
        }
        type WatchCameraSamplesStream = ReceiverStream<Result<rpc::CameraSample, Status>>;
        async fn watch_camera_samples(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<Self::WatchCameraSamplesStream>, Status> {
            Err(Status::unimplemented("camera mock unimplemented"))
        }
        type WatchPointCloudStream = ReceiverStream<Result<rpc::CloudFrame, Status>>;
        async fn watch_point_cloud(
            &self,
            _: Request<rpc::CloudRequest>,
        ) -> Result<Response<Self::WatchPointCloudStream>, Status> {
            Err(Status::unimplemented("pointcloud mock unimplemented"))
        }
    }

    #[tonic::async_trait]
    impl rpc::voice_server::Voice for MockSvc {
        async fn status(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::VoiceSystemStatus>, Status> {
            Ok(Response::new(rpc::VoiceSystemStatus {
                status: "ready".into(),
                service: "askme".into(),
                active_profile: "patrol_default".into(),
                llm_provider: "deepseek".into(),
                llm_model: "deepseek-v4-flash".into(),
                asr_provider: "cloud+local".into(),
                tts_provider: "minimax".into(),
                raw_json: "{}".into(),
            }))
        }
        async fn list_profiles(
            &self,
            _: Request<rpc::Empty>,
        ) -> Result<Response<rpc::VoiceProfileList>, Status> {
            Ok(Response::new(rpc::VoiceProfileList {
                active_profile: "patrol_default".into(),
                profiles: vec![rpc::VoiceProfileInfo {
                    profile_id: "patrol_default".into(),
                    label: "巡检播报".into(),
                    voice_id: "male-qn-qingse".into(),
                    category: "operations".into(),
                    speed: 1.0,
                    volume: 1.0,
                    pitch: 0.0,
                    emotion: String::new(),
                }],
                sound_cues_enabled: true,
                available_sound_cues: vec!["soft_chime".into()],
                raw_json: "{}".into(),
            }))
        }
        async fn switch_profile(
            &self,
            req: Request<rpc::VoiceSwitchProfileRequest>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(req.into_inner().command, "voice_profile_switched"),
            ))
        }
        async fn speak(
            &self,
            req: Request<rpc::VoiceSpeakRequest>,
        ) -> Result<Response<rpc::VoicePlayback>, Status> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            let body = req.into_inner();
            Ok(Response::new(rpc::VoicePlayback {
                playback_id: "pb-mock-1".into(),
                state: "queued".into(),
                priority: body.priority,
                text_chars: body.text.chars().count() as u32,
                message: "accepted".into(),
                raw_json: "{}".into(),
            }))
        }
        async fn playback_status(
            &self,
            req: Request<rpc::VoicePlaybackQuery>,
        ) -> Result<Response<rpc::VoicePlayback>, Status> {
            Ok(Response::new(rpc::VoicePlayback {
                playback_id: req.into_inner().playback_id,
                state: "completed".into(),
                priority: "normal".into(),
                text_chars: 6,
                message: "done".into(),
                raw_json: "{}".into(),
            }))
        }
        async fn cancel_playback(
            &self,
            req: Request<rpc::VoicePlaybackCancelRequest>,
        ) -> Result<Response<rpc::VoicePlayback>, Status> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            let body = req.into_inner();
            Ok(Response::new(rpc::VoicePlayback {
                playback_id: body.playback_id,
                state: "cancelled".into(),
                priority: "normal".into(),
                text_chars: 0,
                message: body.reason,
                raw_json: "{}".into(),
            }))
        }
        async fn switch_component(
            &self,
            req: Request<rpc::VoiceSwitchComponentRequest>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(req.into_inner().command, "voice_component_switched"),
            ))
        }
        async fn update_prompt(
            &self,
            req: Request<rpc::VoiceUpdatePromptRequest>,
        ) -> Result<Response<rpc::Receipt>, Status> {
            Ok(Response::new(
                self.ok_receipt(req.into_inner().command, "voice_prompt_updated"),
            ))
        }
        async fn chat(
            &self,
            req: Request<rpc::VoiceChatRequest>,
        ) -> Result<Response<rpc::VoiceChatReply>, Status> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            let body = req.into_inner();
            Ok(Response::new(rpc::VoiceChatReply {
                reply_text: format!("收到: {}", body.text),
                thread_id: if body.thread_id.is_empty() {
                    "conv-mock-1".into()
                } else {
                    body.thread_id
                },
                turn_id: "turn-1".into(),
                latency_ms: 42.5,
                raw_json: "{}".into(),
            }))
        }
    }

    pub struct DosoMockServerHandle {
        writes: Arc<AtomicU64>,
        shutdown: Option<oneshot::Sender<()>>,
    }

    pub fn start() -> Result<(String, Box<DosoMockServerHandle>), String> {
        let (endpoint, shutdown, writes) = runtime().block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|e| e.to_string())?;
            let addr = listener.local_addr().map_err(|e| e.to_string())?;
            let svc = MockSvc::default();
            let writes = Arc::clone(&svc.writes);
            let (tx, rx) = oneshot::channel::<()>();
            let server = tonic::transport::Server::builder()
                .add_service(rpc::system_server::SystemServer::new(svc.clone()))
                .add_service(rpc::motion_server::MotionServer::new(svc.clone()))
                .add_service(rpc::observations_server::ObservationsServer::new(
                    svc.clone(),
                ))
                .add_service(rpc::navigation_server::NavigationServer::new(svc.clone()))
                .add_service(rpc::voice_server::VoiceServer::new(svc));
            tokio::spawn(async move {
                let _ = server
                    .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                        let _ = rx.await;
                    })
                    .await;
            });
            Ok::<_, String>((format!("http://{addr}"), tx, writes))
        })?;
        Ok((
            endpoint,
            Box::new(DosoMockServerHandle {
                writes,
                shutdown: Some(shutdown),
            }),
        ))
    }

    pub fn writes(handle: &DosoMockServerHandle) -> u64 {
        handle.writes.load(Ordering::Relaxed)
    }

    pub fn stop(mut handle: DosoMockServerHandle) {
        if let Some(tx) = handle.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

#[cfg(any(test, feature = "test-mock"))]
#[repr(C)]
pub struct DosoMockServer {
    _private: [u8; 0],
}

#[cfg(any(test, feature = "test-mock"))]
#[no_mangle]
pub unsafe extern "C" fn doso_test_mock_server_start(
    endpoint_buf: *mut c_char,
    endpoint_cap: usize,
) -> *mut DosoMockServer {
    ffi_ptr(|| {
        let (endpoint, handle) = mock_server::start()?;
        write_c_string(&endpoint, endpoint_buf, endpoint_cap);
        Ok(Box::into_raw(handle).cast::<DosoMockServer>())
    })
}

#[cfg(any(test, feature = "test-mock"))]
#[no_mangle]
pub unsafe extern "C" fn doso_test_mock_server_writes(server: *const DosoMockServer) -> u64 {
    let Some(handle) = server.cast::<mock_server::DosoMockServerHandle>().as_ref() else {
        return 0;
    };
    mock_server::writes(handle)
}

#[cfg(any(test, feature = "test-mock"))]
#[no_mangle]
pub unsafe extern "C" fn doso_test_mock_server_stop(server: *mut DosoMockServer) {
    if !server.is_null() {
        let boxed = Box::from_raw(server.cast::<mock_server::DosoMockServerHandle>());
        mock_server::stop(*boxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn c_abi_round_trips_robot_sport_teleop_and_low_level() {
        unsafe {
            let mut ep_buf = [0 as c_char; 128];
            let mock = doso_test_mock_server_start(ep_buf.as_mut_ptr(), ep_buf.len());
            assert!(!mock.is_null());
            let endpoint = CStr::from_ptr(ep_buf.as_ptr()).to_str().unwrap().to_owned();

            let c_ep = CString::new(endpoint).unwrap();
            let robot = doso_robot_connect(c_ep.as_ptr(), ptr::null(), 3000);
            assert!(!robot.is_null());
            assert_eq!(doso_robot_ping(robot), 0);

            let mut caps = DosoCapabilities::default();
            assert_eq!(doso_robot_capabilities(robot, &mut caps), 0);
            assert_eq!(caps.motion, 1);
            assert_eq!(caps.low_level, 1);
            assert_eq!(caps.observations, 1);
            assert_eq!(caps.board_b_autonomy, 1);
            assert_eq!(caps.voice, 1);

            let mut vbuf = [0 as c_char; 2048];
            assert_eq!(doso_voice_status_json(robot, vbuf.as_mut_ptr(), vbuf.len()), 0);
            let vstatus = CStr::from_ptr(vbuf.as_ptr()).to_str().unwrap();
            assert!(vstatus.contains("\"service\":\"askme\""));
            assert!(vstatus.contains("\"llm_model\":\"deepseek-v4-flash\""));

            assert_eq!(doso_voice_profiles_json(robot, vbuf.as_mut_ptr(), vbuf.len()), 0);
            let vprofs = CStr::from_ptr(vbuf.as_ptr()).to_str().unwrap();
            assert!(vprofs.contains("\"active_profile\":\"patrol_default\""));

            let c_prof = CString::new("patrol_default").unwrap();
            assert_eq!(doso_voice_switch_profile(robot, c_prof.as_ptr()), 0);
            let c_text = CString::new("系统自检通过").unwrap();
            let mut pb_buf = [0 as c_char; 128];
            assert_eq!(
                doso_voice_speak(
                    robot,
                    c_text.as_ptr(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    pb_buf.as_mut_ptr(),
                    pb_buf.len()
                ),
                0
            );
            assert_eq!(CStr::from_ptr(pb_buf.as_ptr()).to_str().unwrap(), "pb-mock-1");
            assert_eq!(
                doso_voice_playback_status_json(robot, pb_buf.as_ptr(), vbuf.as_mut_ptr(), vbuf.len()),
                0
            );
            assert!(CStr::from_ptr(vbuf.as_ptr())
                .to_str()
                .unwrap()
                .contains("\"state\":\"completed\""));
            assert_eq!(doso_voice_cancel_playback(robot, pb_buf.as_ptr(), ptr::null()), 0);
            let c_comp = CString::new("llm").unwrap();
            let c_prov = CString::new("deepseek").unwrap();
            let c_model = CString::new("deepseek-v4-flash").unwrap();
            assert_eq!(
                doso_voice_switch_component(robot, c_comp.as_ptr(), c_prov.as_ptr(), c_model.as_ptr(), 0),
                0
            );
            let c_prompt = CString::new("你是小算机器狗。").unwrap();
            assert_eq!(doso_voice_update_prompt(robot, c_prompt.as_ptr()), 0);
            let c_msg = CString::new("你好").unwrap();
            assert_eq!(
                doso_voice_chat_json(robot, c_msg.as_ptr(), ptr::null(), 0, vbuf.as_mut_ptr(), vbuf.len()),
                0
            );
            assert!(CStr::from_ptr(vbuf.as_ptr())
                .to_str()
                .unwrap()
                .contains("收到: 你好"));

            let mut ctrl = DosoBodyControlStatus::default();
            assert_eq!(doso_sport_control_status(robot, &mut ctrl), 0);
            assert_eq!(ctrl.body_kind, 1); // Grounded
            assert_eq!(ctrl.motor_output_enabled, 0);
            assert_eq!(ctrl.accepted_count, 11);

            let mut str_buf = [0 as c_char; 256];
            assert_eq!(
                doso_sport_current_profile(robot, str_buf.as_mut_ptr(), str_buf.len()),
                0
            );
            assert_eq!(CStr::from_ptr(str_buf.as_ptr()).to_str().unwrap(), "flat");
            assert_eq!(
                doso_sport_list_profiles(robot, str_buf.as_mut_ptr(), str_buf.len()),
                0
            );
            assert_eq!(
                CStr::from_ptr(str_buf.as_ptr()).to_str().unwrap(),
                "flat\nstairs"
            );
            assert_eq!(
                doso_sport_list_gestures(robot, str_buf.as_mut_ptr(), str_buf.len()),
                0
            );
            assert_eq!(CStr::from_ptr(str_buf.as_ptr()).to_str().unwrap(), "bow");

            let telemetry = doso_observations_start_telemetry(robot, 1, 1, 500);
            assert!(!telemetry.is_null());
            let mut imu = DosoImuSnapshot::default();
            let mut joints = DosoJointsSnapshot::default();
            for _ in 0..50 {
                assert_eq!(doso_observations_latest_imu(telemetry, &mut imu), 0);
                assert_eq!(doso_observations_latest_joints(telemetry, &mut joints), 0);
                if imu.available == 1 && joints.complete == 1 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(imu.available, 1);
            assert_eq!(imu.fresh, 1);
            assert!((imu.accel_m_s2[2] - 9.81).abs() < 1e-6);
            assert!((imu.quat_wxyz[0] - 1.0).abs() < 1e-6);
            assert_eq!(joints.available, 1);
            assert_eq!(joints.complete, 1);
            assert_eq!(joints.valid_mask, u16::MAX);
            assert!((joints.q[10] - 0.5).abs() < 1e-6);
            doso_observations_stop_telemetry(telemetry);

            let mut motors = DosoMotorsSnapshot::default();
            assert_eq!(doso_observations_motors(robot, &mut motors), 0);
            assert_eq!(motors.count, 16);
            assert_eq!(motors.motors[0].online, 1);
            assert!((motors.motors[0].temperature_c - 28.5).abs() < 1e-6);

            let mut volts = [0.0f64; 16];
            let mut vcount = 0u32;
            assert_eq!(
                doso_observations_motor_voltages(robot, volts.as_mut_ptr(), &mut vcount),
                0
            );
            assert_eq!(vcount, 16);
            assert!((volts[0] - 50.2).abs() < 1e-6);

            assert_eq!(doso_sport_stand_up(robot), 0);
            assert_eq!(doso_sport_damp(robot), 0);
            let bow = CString::new("bow").unwrap();
            assert_eq!(doso_sport_play_gesture(robot, bow.as_ptr()), 0);
            assert_eq!(doso_sport_clear_motor_fault(robot), 0);

            let mut bh = DosoBodyHeightStatus::default();
            assert_eq!(doso_sport_body_height_status(robot, &mut bh), 0);
            assert_eq!(bh.supported, 1);
            assert!((bh.current_height_m - 0.375).abs() < 1e-6);
            assert!((bh.min_height_m - 0.25).abs() < 1e-6);
            assert!((bh.max_height_m - 0.50).abs() < 1e-6);
            assert_eq!(doso_sport_set_body_height(robot, 0.42, &mut bh), 0);
            assert!((bh.target_height_m - 0.42).abs() < 1e-6);
            assert_ne!(doso_sport_set_body_height(robot, 0.99, ptr::null_mut()), 0);

            assert_eq!(
                doso_sport_recovery_status_json(robot, vbuf.as_mut_ptr(), vbuf.len()),
                0
            );
            assert!(CStr::from_ptr(vbuf.as_ptr())
                .to_str()
                .unwrap()
                .contains("\"availability\":2"));
            let c_op = CString::new("rec-op-1").unwrap();
            assert_eq!(
                doso_sport_recovery_enter_json(robot, c_op.as_ptr(), vbuf.as_mut_ptr(), vbuf.len()),
                0
            );
            assert!(CStr::from_ptr(vbuf.as_ptr())
                .to_str()
                .unwrap()
                .contains("\"session_token\":\"rec-mock-tok\""));
            let c_tok = CString::new("rec-mock-tok").unwrap();
            assert_eq!(
                doso_sport_recovery_heartbeat_json(robot, c_tok.as_ptr(), vbuf.as_mut_ptr(), vbuf.len()),
                0
            );
            assert_eq!(
                doso_sport_recovery_exit_json(robot, c_tok.as_ptr(), vbuf.as_mut_ptr(), vbuf.len()),
                0
            );

            let teleop = doso_teleop_take_control(robot);
            assert!(!teleop.is_null());
            assert_eq!(doso_teleop_send_velocity(teleop, 0.3, 0.0, 0.1, 1), 0);
            assert_eq!(doso_teleop_release(teleop), 0);
            doso_teleop_free(teleop);

            let ll = doso_low_level_enter(robot);
            assert!(!ll.is_null());
            let mut cmds = [DosoJointCmd::default(); 16];
            for (idx, cmd) in cmds.iter_mut().enumerate() {
                if idx < 12 {
                    *cmd = DosoJointCmd {
                        q: 0.1,
                        dq: 0.0,
                        kp: 40.0,
                        kd: 2.0,
                        tau: 0.0,
                    };
                } else {
                    *cmd = DosoJointCmd {
                        q: 0.0,
                        dq: 3.0,
                        kp: 0.0,
                        kd: 1.5,
                        tau: 0.0,
                    };
                }
            }
            assert_eq!(doso_low_level_publish_joints16(ll, cmds.as_ptr()), 0);
            assert_eq!(doso_low_level_enable(ll), 0);
            let mut snap = DosoControllerSnapshot::default();
            assert_eq!(doso_low_level_get_state(ll, &mut snap), 0);
            assert_eq!(snap.active, 1);
            assert_eq!(snap.motor_output_enabled, 1);
            assert_eq!(snap.remote_active, 1);
            assert_eq!(snap.remote_buttons, 3);
            assert_eq!(doso_low_level_close(ll), 0);
            doso_low_level_free(ll);

            assert!(doso_test_mock_server_writes(mock) >= 6);
            doso_robot_free(robot);
            doso_test_mock_server_stop(mock);
        }
    }
}
