"""ctypes binding over the shared Rust doso_sdk_ffi core."""

from __future__ import annotations

import ctypes
import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, List, Optional, Tuple


class DosoError(RuntimeError):
    """Raised when an underlying DOSO SDK operation fails."""


class _CJointCmd(ctypes.Structure):
    _fields_ = [
        ("q", ctypes.c_double),
        ("dq", ctypes.c_double),
        ("kp", ctypes.c_double),
        ("kd", ctypes.c_double),
        ("tau", ctypes.c_double),
    ]


class _CControllerSnapshot(ctypes.Structure):
    _fields_ = [
        ("mode", ctypes.c_int32),
        ("active", ctypes.c_uint8),
        ("motor_output_enabled", ctypes.c_uint8),
        ("fresh", ctypes.c_uint8),
        ("remote_active", ctypes.c_uint8),
        ("last_sequence", ctypes.c_uint64),
        ("joint_count", ctypes.c_uint32),
        ("remote_buttons", ctypes.c_uint32),
        ("remote_lx", ctypes.c_double),
        ("remote_ly", ctypes.c_double),
        ("remote_rx", ctypes.c_double),
        ("remote_ry", ctypes.c_double),
    ]


class _CCapabilities(ctypes.Structure):
    _fields_ = [
        ("motion", ctypes.c_uint8),
        ("low_level", ctypes.c_uint8),
        ("observations", ctypes.c_uint8),
        ("navigation", ctypes.c_uint8),
        ("maps", ctypes.c_uint8),
        ("localization", ctypes.c_uint8),
        ("tasks", ctypes.c_uint8),
        ("board_b_autonomy", ctypes.c_uint8),
        ("voice", ctypes.c_uint8),
    ]


class _CBodyControlStatus(ctypes.Structure):
    _fields_ = [
        ("body_kind", ctypes.c_int32),
        ("body_transition", ctypes.c_int32),
        ("source_owner", ctypes.c_int32),
        ("motor_output_enabled", ctypes.c_uint8),
        ("critical_motor_fault", ctypes.c_uint8),
        ("grpc_lease_active", ctypes.c_uint8),
        ("source_ready_for_walk", ctypes.c_uint8),
        ("lease_remaining_ms", ctypes.c_uint32),
        ("_pad", ctypes.c_uint32),
        ("last_accepted_sequence", ctypes.c_uint64),
        ("accepted_count", ctypes.c_uint64),
        ("rejected_count", ctypes.c_uint64),
    ]


class _CBodyHeightStatus(ctypes.Structure):
    _fields_ = [
        ("supported", ctypes.c_uint8),
        ("_pad", ctypes.c_uint8 * 7),
        ("current_height_m", ctypes.c_double),
        ("target_height_m", ctypes.c_double),
        ("min_height_m", ctypes.c_double),
        ("max_height_m", ctypes.c_double),
        ("rate_mps", ctypes.c_double),
    ]


class _CImuSnapshot(ctypes.Structure):
    _fields_ = [
        ("available", ctypes.c_uint8),
        ("fresh", ctypes.c_uint8),
        ("stream_active", ctypes.c_uint8),
        ("_pad", ctypes.c_uint8 * 5),
        ("age_ms", ctypes.c_double),
        ("gyro_rad_s", ctypes.c_double * 3),
        ("accel_m_s2", ctypes.c_double * 3),
        ("quat_wxyz", ctypes.c_double * 4),
        ("source_elapsed_ns", ctypes.c_int64),
    ]


class _CJointsSnapshot(ctypes.Structure):
    _fields_ = [
        ("available", ctypes.c_uint8),
        ("complete", ctypes.c_uint8),
        ("fresh", ctypes.c_uint8),
        ("stream_active", ctypes.c_uint8),
        ("valid_mask", ctypes.c_uint16),
        ("_pad", ctypes.c_uint16),
        ("age_ms", ctypes.c_double),
        ("q", ctypes.c_double * 16),
        ("dq", ctypes.c_double * 16),
        ("tau", ctypes.c_double * 16),
        ("status_code", ctypes.c_uint32 * 16),
        ("source_elapsed_ns", ctypes.c_int64),
    ]


class _CMotorInfo(ctypes.Structure):
    _fields_ = [
        ("id", ctypes.c_uint32),
        ("online", ctypes.c_uint8),
        ("_pad", ctypes.c_uint8 * 3),
        ("status_code", ctypes.c_uint32),
        ("error_count", ctypes.c_uint32),
        ("temperature_c", ctypes.c_double),
    ]


class _CMotorsSnapshot(ctypes.Structure):
    _fields_ = [
        ("count", ctypes.c_uint32),
        ("_pad", ctypes.c_uint32),
        ("motors", _CMotorInfo * 16),
    ]


@dataclass(frozen=True)
class JointCmd:
    q: float = 0.0
    dq: float = 0.0
    kp: float = 0.0
    kd: float = 0.0
    tau: float = 0.0

    @classmethod
    def leg_pd(cls, q: float, kp: float, kd: float) -> "JointCmd":
        return cls(q=q, dq=0.0, kp=kp, kd=kd, tau=0.0)

    @classmethod
    def wheel_vel(cls, dq: float, kd: float) -> "JointCmd":
        return cls(q=0.0, dq=dq, kp=0.0, kd=kd, tau=0.0)

    @classmethod
    def damping(cls, kd: float) -> "JointCmd":
        return cls(q=0.0, dq=0.0, kp=0.0, kd=kd, tau=0.0)


@dataclass(frozen=True)
class ControllerSnapshot:
    mode: int
    active: bool
    motor_output_enabled: bool
    fresh: bool
    last_sequence: int
    joint_count: int
    remote_active: bool
    remote_buttons: int
    remote_lx: float
    remote_ly: float
    remote_rx: float
    remote_ry: float


@dataclass(frozen=True)
class Capabilities:
    motion: bool
    low_level: bool
    observations: bool
    navigation: bool
    maps: bool
    localization: bool
    tasks: bool
    board_b_autonomy: bool
    voice: bool = False


@dataclass(frozen=True)
class VoiceStatus:
    status: str
    service: str
    active_profile: str
    llm_provider: str
    llm_model: str
    asr_provider: str
    tts_provider: str


@dataclass(frozen=True)
class VoiceProfileInfo:
    profile_id: str
    label: str
    voice_id: str
    category: str
    speed: float
    volume: float
    pitch: float
    emotion: str


@dataclass(frozen=True)
class VoiceProfileList:
    active_profile: str
    profiles: Tuple[VoiceProfileInfo, ...]
    sound_cues_enabled: bool
    available_sound_cues: Tuple[str, ...]


@dataclass(frozen=True)
class VoicePlayback:
    playback_id: str
    state: str
    priority: str
    text_chars: int
    message: str


@dataclass(frozen=True)
class VoiceChatReply:
    reply_text: str
    thread_id: str
    turn_id: str
    latency_ms: float


@dataclass(frozen=True)
class BodyControlStatus:
    body_kind: int
    body_transition: int
    source_owner: int
    motor_output_enabled: bool
    critical_motor_fault: bool
    grpc_lease_active: bool
    source_ready_for_walk: bool
    lease_remaining_ms: int
    last_accepted_sequence: int
    accepted_count: int
    rejected_count: int


@dataclass(frozen=True)
class BodyHeightStatus:
    supported: bool
    current_height_m: float
    target_height_m: float
    min_height_m: float
    max_height_m: float
    rate_mps: float


@dataclass(frozen=True)
class RecoveryStatus:
    availability: int
    phase: int
    operation_id: str
    unavailable_reason: str
    selected: bool
    session_token: str
    motor_disable_confirmed: bool


@dataclass(frozen=True)
class RecoveryActionReply:
    operation_id: str
    decision: int
    phase: int
    reason: str


@dataclass(frozen=True)
class ImuSnapshot:
    available: bool
    fresh: bool
    stream_active: bool
    age_ms: float
    gyro_rad_s: Tuple[float, float, float]
    accel_m_s2: Tuple[float, float, float]
    quat_wxyz: Tuple[float, float, float, float]
    source_elapsed_ns: int


@dataclass(frozen=True)
class JointsSnapshot:
    available: bool
    complete: bool
    fresh: bool
    stream_active: bool
    valid_mask: int
    age_ms: float
    q: Tuple[float, ...]
    dq: Tuple[float, ...]
    tau: Tuple[float, ...]
    status_code: Tuple[int, ...]
    source_elapsed_ns: int


@dataclass(frozen=True)
class MotorInfo:
    id: int
    online: bool
    status_code: int
    error_count: int
    temperature_c: float


def _find_library() -> Path:
    env_path = os.environ.get("DOSO_SDK_FFI_PATH")
    if env_path:
        p = Path(env_path)
        if p.is_file():
            return p
        raise DosoError(f"DOSO_SDK_FFI_PATH does not exist: {env_path}")

    if sys.platform == "win32":
        lib_name = "doso_sdk_ffi.dll"
    elif sys.platform == "darwin":
        lib_name = "libdoso_sdk_ffi.dylib"
    else:
        lib_name = "libdoso_sdk_ffi.so"

    repo_root = Path(__file__).resolve().parents[3]
    candidates = [
        repo_root / "target" / "release" / lib_name,
        repo_root / "target" / "debug" / lib_name,
        repo_root / "bindings" / "ffi" / "target" / "release" / lib_name,
        repo_root / "bindings" / "ffi" / "target" / "debug" / lib_name,
        Path(__file__).resolve().parent / lib_name,
    ]
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    raise DosoError(f"Could not find {lib_name}; build ffi crate or set DOSO_SDK_FFI_PATH")


_LIB: Optional[ctypes.CDLL] = None


def _lib() -> ctypes.CDLL:
    global _LIB
    if _LIB is not None:
        return _LIB
    lib = ctypes.CDLL(str(_find_library()))

    lib.doso_last_error.argtypes = [ctypes.c_char_p, ctypes.c_size_t]
    lib.doso_last_error.restype = ctypes.c_size_t

    lib.doso_robot_connect.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint32]
    lib.doso_robot_connect.restype = ctypes.c_void_p
    lib.doso_robot_free.argtypes = [ctypes.c_void_p]
    lib.doso_robot_free.restype = None
    lib.doso_robot_ping.argtypes = [ctypes.c_void_p]
    lib.doso_robot_ping.restype = ctypes.c_int32

    for fn_name in (
        "doso_robot_id",
        "doso_firmware_version",
        "doso_robot_version",
        "doso_low_level_endpoint",
    ):
        fn = getattr(lib, fn_name)
        fn.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
        fn.restype = ctypes.c_size_t

    for fn_name in (
        "doso_sport_enable_motors",
        "doso_sport_disable_motors",
        "doso_sport_stand_up",
        "doso_sport_stand_down",
        "doso_sport_damp",
        "doso_sport_stop",
        "doso_sport_clear_motor_fault",
    ):
        fn = getattr(lib, fn_name)
        fn.argtypes = [ctypes.c_void_p]
        fn.restype = ctypes.c_int32

    lib.doso_sport_switch_profile.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
    lib.doso_sport_switch_profile.restype = ctypes.c_int32
    lib.doso_sport_play_gesture.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
    lib.doso_sport_play_gesture.restype = ctypes.c_int32
    lib.doso_sport_clear_joint_fault.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
    lib.doso_sport_clear_joint_fault.restype = ctypes.c_int32

    lib.doso_teleop_take_control.argtypes = [ctypes.c_void_p]
    lib.doso_teleop_take_control.restype = ctypes.c_void_p
    lib.doso_teleop_send_velocity.argtypes = [
        ctypes.c_void_p,
        ctypes.c_double,
        ctypes.c_double,
        ctypes.c_double,
        ctypes.c_uint8,
    ]
    lib.doso_teleop_send_velocity.restype = ctypes.c_int32
    lib.doso_teleop_publish_velocity.argtypes = lib.doso_teleop_send_velocity.argtypes
    lib.doso_teleop_publish_velocity.restype = ctypes.c_int32
    lib.doso_teleop_release.argtypes = [ctypes.c_void_p]
    lib.doso_teleop_release.restype = ctypes.c_int32
    lib.doso_teleop_free.argtypes = [ctypes.c_void_p]
    lib.doso_teleop_free.restype = None

    lib.doso_low_level_enter.argtypes = [ctypes.c_void_p]
    lib.doso_low_level_enter.restype = ctypes.c_void_p
    lib.doso_low_level_enable.argtypes = [ctypes.c_void_p]
    lib.doso_low_level_enable.restype = ctypes.c_int32
    lib.doso_low_level_disable.argtypes = [ctypes.c_void_p]
    lib.doso_low_level_disable.restype = ctypes.c_int32
    lib.doso_low_level_publish_joints16.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(_CJointCmd),
    ]
    lib.doso_low_level_publish_joints16.restype = ctypes.c_int32
    lib.doso_low_level_get_state.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(_CControllerSnapshot),
    ]
    lib.doso_low_level_get_state.restype = ctypes.c_int32
    lib.doso_low_level_close.argtypes = [ctypes.c_void_p]
    lib.doso_low_level_close.restype = ctypes.c_int32
    lib.doso_low_level_free.argtypes = [ctypes.c_void_p]
    lib.doso_low_level_free.restype = None

    lib.doso_robot_capabilities.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(_CCapabilities),
    ]
    lib.doso_robot_capabilities.restype = ctypes.c_int32

    lib.doso_sport_control_status.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(_CBodyControlStatus),
    ]
    lib.doso_sport_control_status.restype = ctypes.c_int32

    lib.doso_sport_body_height_status.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(_CBodyHeightStatus),
    ]
    lib.doso_sport_body_height_status.restype = ctypes.c_int32
    lib.doso_sport_set_body_height.argtypes = [
        ctypes.c_void_p,
        ctypes.c_double,
        ctypes.POINTER(_CBodyHeightStatus),
    ]
    lib.doso_sport_set_body_height.restype = ctypes.c_int32

    lib.doso_sport_recovery_status_json.argtypes = [
        ctypes.c_void_p,
        ctypes.c_char_p,
        ctypes.c_size_t,
    ]
    lib.doso_sport_recovery_status_json.restype = ctypes.c_int32
    for fn_name in (
        "doso_sport_recovery_enter_json",
        "doso_sport_recovery_enable_json",
        "doso_sport_recovery_heartbeat_json",
        "doso_sport_recovery_exit_json",
    ):
        fn = getattr(lib, fn_name)
        fn.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_size_t]
        fn.restype = ctypes.c_int32
    for fn_name in (
        "doso_sport_recovery_start_json",
        "doso_sport_recovery_cancel_json",
    ):
        fn = getattr(lib, fn_name)
        fn.argtypes = [
            ctypes.c_void_p,
            ctypes.c_char_p,
            ctypes.c_char_p,
            ctypes.c_char_p,
            ctypes.c_size_t,
        ]
        fn.restype = ctypes.c_int32

    for fn_name in (
        "doso_sport_current_profile",
        "doso_sport_list_profiles",
        "doso_sport_list_gestures",
    ):
        fn = getattr(lib, fn_name)
        fn.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
        fn.restype = ctypes.c_int32

    lib.doso_observations_start_telemetry.argtypes = [
        ctypes.c_void_p,
        ctypes.c_uint8,
        ctypes.c_uint8,
        ctypes.c_uint32,
    ]
    lib.doso_observations_start_telemetry.restype = ctypes.c_void_p
    lib.doso_observations_latest_imu.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(_CImuSnapshot),
    ]
    lib.doso_observations_latest_imu.restype = ctypes.c_int32
    lib.doso_observations_latest_joints.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(_CJointsSnapshot),
    ]
    lib.doso_observations_latest_joints.restype = ctypes.c_int32
    lib.doso_observations_stop_telemetry.argtypes = [ctypes.c_void_p]
    lib.doso_observations_stop_telemetry.restype = None
    lib.doso_observations_motors.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(_CMotorsSnapshot),
    ]
    lib.doso_observations_motors.restype = ctypes.c_int32
    lib.doso_observations_motor_voltages.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(ctypes.c_double),
        ctypes.POINTER(ctypes.c_uint32),
    ]
    lib.doso_observations_motor_voltages.restype = ctypes.c_int32

    lib.doso_nav_go_to.argtypes = [
        ctypes.c_void_p,
        ctypes.c_double,
        ctypes.c_double,
        ctypes.c_double,
        ctypes.c_double,
        ctypes.c_char_p,
        ctypes.c_size_t,
    ]
    lib.doso_nav_go_to.restype = ctypes.c_int32
    lib.doso_nav_cancel.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p]
    lib.doso_nav_cancel.restype = ctypes.c_int32

    lib.doso_voice_status_json.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
    lib.doso_voice_status_json.restype = ctypes.c_int32
    lib.doso_voice_profiles_json.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
    lib.doso_voice_profiles_json.restype = ctypes.c_int32
    lib.doso_voice_switch_profile.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
    lib.doso_voice_switch_profile.restype = ctypes.c_int32
    lib.doso_voice_speak.argtypes = [
        ctypes.c_void_p,
        ctypes.c_char_p,
        ctypes.c_char_p,
        ctypes.c_char_p,
        ctypes.c_char_p,
        ctypes.c_char_p,
        ctypes.c_size_t,
    ]
    lib.doso_voice_speak.restype = ctypes.c_int32
    lib.doso_voice_playback_status_json.argtypes = [
        ctypes.c_void_p,
        ctypes.c_char_p,
        ctypes.c_char_p,
        ctypes.c_size_t,
    ]
    lib.doso_voice_playback_status_json.restype = ctypes.c_int32
    lib.doso_voice_cancel_playback.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p]
    lib.doso_voice_cancel_playback.restype = ctypes.c_int32
    lib.doso_voice_switch_component.argtypes = [
        ctypes.c_void_p,
        ctypes.c_char_p,
        ctypes.c_char_p,
        ctypes.c_char_p,
        ctypes.c_uint8,
    ]
    lib.doso_voice_switch_component.restype = ctypes.c_int32
    lib.doso_voice_update_prompt.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
    lib.doso_voice_update_prompt.restype = ctypes.c_int32
    lib.doso_voice_chat_json.argtypes = [
        ctypes.c_void_p,
        ctypes.c_char_p,
        ctypes.c_char_p,
        ctypes.c_uint8,
        ctypes.c_char_p,
        ctypes.c_size_t,
    ]
    lib.doso_voice_chat_json.restype = ctypes.c_int32

    _LIB = lib
    return lib


def _last_error() -> str:
    buf = ctypes.create_string_buffer(512)
    n = _lib().doso_last_error(buf, len(buf))
    if n == 0:
        return "unknown DOSO SDK error"
    return buf.value.decode("utf-8", errors="replace")


def _check(code: int) -> None:
    if code != 0:
        raise DosoError(_last_error())


def _read_str(fn_name: str, handle: int) -> str:
    fn = getattr(_lib(), fn_name)
    buf = ctypes.create_string_buffer(256)
    n = fn(handle, buf, len(buf))
    if n < len(buf):
        return buf.value.decode("utf-8", errors="replace")
    big = ctypes.create_string_buffer(n + 1)
    fn(handle, big, len(big))
    return big.value.decode("utf-8", errors="replace")


class TeleopSession:
    def __init__(self, handle: int) -> None:
        self._handle: Optional[int] = handle

    def send_velocity(
        self, vx_mps: float, vy_mps: float = 0.0, yaw_rps: float = 0.0, deadman: bool = True
    ) -> None:
        _check(_lib().doso_teleop_send_velocity(self._handle, vx_mps, vy_mps, yaw_rps, int(deadman)))

    def publish_velocity(
        self, vx_mps: float, vy_mps: float = 0.0, yaw_rps: float = 0.0, deadman: bool = True
    ) -> None:
        _check(
            _lib().doso_teleop_publish_velocity(
                self._handle, vx_mps, vy_mps, yaw_rps, int(deadman)
            )
        )

    def release(self) -> None:
        if self._handle is not None:
            _check(_lib().doso_teleop_release(self._handle))

    def close(self) -> None:
        if self._handle is not None:
            _lib().doso_teleop_free(self._handle)
            self._handle = None

    def __enter__(self) -> "TeleopSession":
        return self

    def __exit__(self, *_: object) -> None:
        try:
            self.release()
        except DosoError:
            pass
        finally:
            self.close()

    def __del__(self) -> None:
        self.close()


class LowLevelSession:
    def __init__(self, handle: int) -> None:
        self._handle: Optional[int] = handle

    def enable(self) -> None:
        _check(_lib().doso_low_level_enable(self._handle))

    def disable(self) -> None:
        _check(_lib().doso_low_level_disable(self._handle))

    def publish_joints(self, joints: Iterable[JointCmd]) -> None:
        items = list(joints)
        if len(items) != 16:
            raise DosoError(f"publish_joints requires exactly 16 JointCmd items, got {len(items)}")
        arr = (_CJointCmd * 16)(
            *(_CJointCmd(q=c.q, dq=c.dq, kp=c.kp, kd=c.kd, tau=c.tau) for c in items)
        )
        _check(_lib().doso_low_level_publish_joints16(self._handle, arr))

    def state(self) -> ControllerSnapshot:
        raw = _CControllerSnapshot()
        _check(_lib().doso_low_level_get_state(self._handle, ctypes.byref(raw)))
        return ControllerSnapshot(
            mode=raw.mode,
            active=bool(raw.active),
            motor_output_enabled=bool(raw.motor_output_enabled),
            fresh=bool(raw.fresh),
            last_sequence=raw.last_sequence,
            joint_count=raw.joint_count,
            remote_active=bool(raw.remote_active),
            remote_buttons=raw.remote_buttons,
            remote_lx=raw.remote_lx,
            remote_ly=raw.remote_ly,
            remote_rx=raw.remote_rx,
            remote_ry=raw.remote_ry,
        )

    def close(self) -> None:
        if self._handle is not None:
            try:
                _check(_lib().doso_low_level_close(self._handle))
            finally:
                _lib().doso_low_level_free(self._handle)
                self._handle = None

    def __enter__(self) -> "LowLevelSession":
        return self

    def __exit__(self, *_: object) -> None:
        if self._handle is not None:
            try:
                self.close()
            except DosoError:
                pass

    def __del__(self) -> None:
        if self._handle is not None:
            _lib().doso_low_level_free(self._handle)
            self._handle = None


class TelemetryStream:
    """Background IMU & 16-joint telemetry cache powered by the Rust Tokio runtime."""

    def __init__(self, handle: int) -> None:
        self._handle: Optional[int] = handle

    def latest_imu(self) -> ImuSnapshot:
        raw = _CImuSnapshot()
        _check(_lib().doso_observations_latest_imu(self._handle, ctypes.byref(raw)))
        return ImuSnapshot(
            available=bool(raw.available),
            fresh=bool(raw.fresh),
            stream_active=bool(raw.stream_active),
            age_ms=raw.age_ms,
            gyro_rad_s=(raw.gyro_rad_s[0], raw.gyro_rad_s[1], raw.gyro_rad_s[2]),
            accel_m_s2=(raw.accel_m_s2[0], raw.accel_m_s2[1], raw.accel_m_s2[2]),
            quat_wxyz=(raw.quat_wxyz[0], raw.quat_wxyz[1], raw.quat_wxyz[2], raw.quat_wxyz[3]),
            source_elapsed_ns=raw.source_elapsed_ns,
        )

    def latest_joints(self) -> JointsSnapshot:
        raw = _CJointsSnapshot()
        _check(_lib().doso_observations_latest_joints(self._handle, ctypes.byref(raw)))
        return JointsSnapshot(
            available=bool(raw.available),
            complete=bool(raw.complete),
            fresh=bool(raw.fresh),
            stream_active=bool(raw.stream_active),
            valid_mask=raw.valid_mask,
            age_ms=raw.age_ms,
            q=tuple(raw.q),
            dq=tuple(raw.dq),
            tau=tuple(raw.tau),
            status_code=tuple(raw.status_code),
            source_elapsed_ns=raw.source_elapsed_ns,
        )

    def close(self) -> None:
        if self._handle is not None:
            _lib().doso_observations_stop_telemetry(self._handle)
            self._handle = None

    def __enter__(self) -> "TelemetryStream":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    def __del__(self) -> None:
        self.close()


class ObservationsClient:
    def __init__(self, robot_handle: int) -> None:
        self._robot = robot_handle

    def telemetry(
        self, *, imu: bool = True, joints: bool = True, stale_after_ms: int = 500
    ) -> TelemetryStream:
        handle = _lib().doso_observations_start_telemetry(
            self._robot, int(imu), int(joints), stale_after_ms
        )
        if not handle:
            raise DosoError(_last_error())
        return TelemetryStream(handle)

    def motors(self) -> List[MotorInfo]:
        raw = _CMotorsSnapshot()
        _check(_lib().doso_observations_motors(self._robot, ctypes.byref(raw)))
        out: List[MotorInfo] = []
        for i in range(min(raw.count, 16)):
            m = raw.motors[i]
            out.append(
                MotorInfo(
                    id=m.id,
                    online=bool(m.online),
                    status_code=m.status_code,
                    error_count=m.error_count,
                    temperature_c=m.temperature_c,
                )
            )
        return out

    def motor_voltages(self) -> List[float]:
        arr = (ctypes.c_double * 16)()
        count = ctypes.c_uint32(0)
        _check(_lib().doso_observations_motor_voltages(self._robot, arr, ctypes.byref(count)))
        return [arr[i] for i in range(min(count.value, 16))]


class SportClient:
    def __init__(self, robot_handle: int) -> None:
        self._robot = robot_handle

    def enable_motors(self) -> None:
        _check(_lib().doso_sport_enable_motors(self._robot))

    def disable_motors(self) -> None:
        _check(_lib().doso_sport_disable_motors(self._robot))

    def stand_up(self) -> None:
        _check(_lib().doso_sport_stand_up(self._robot))

    def stand_down(self) -> None:
        _check(_lib().doso_sport_stand_down(self._robot))

    def damp(self) -> None:
        _check(_lib().doso_sport_damp(self._robot))

    def stop(self) -> None:
        _check(_lib().doso_sport_stop(self._robot))

    def switch_profile(self, name: str) -> None:
        _check(_lib().doso_sport_switch_profile(self._robot, name.encode("utf-8")))

    def play_gesture(self, name: str) -> None:
        _check(_lib().doso_sport_play_gesture(self._robot, name.encode("utf-8")))

    def clear_motor_fault(self, joint_id: Optional[int] = None) -> None:
        if joint_id is None:
            _check(_lib().doso_sport_clear_motor_fault(self._robot))
        else:
            _check(_lib().doso_sport_clear_joint_fault(self._robot, joint_id))

    def control_status(self) -> BodyControlStatus:
        raw = _CBodyControlStatus()
        _check(_lib().doso_sport_control_status(self._robot, ctypes.byref(raw)))
        return BodyControlStatus(
            body_kind=raw.body_kind,
            body_transition=raw.body_transition,
            source_owner=raw.source_owner,
            motor_output_enabled=bool(raw.motor_output_enabled),
            critical_motor_fault=bool(raw.critical_motor_fault),
            grpc_lease_active=bool(raw.grpc_lease_active),
            source_ready_for_walk=bool(raw.source_ready_for_walk),
            lease_remaining_ms=raw.lease_remaining_ms,
            last_accepted_sequence=raw.last_accepted_sequence,
            accepted_count=raw.accepted_count,
            rejected_count=raw.rejected_count,
        )

    def current_profile(self) -> str:
        buf = ctypes.create_string_buffer(256)
        _check(_lib().doso_sport_current_profile(self._robot, buf, len(buf)))
        return buf.value.decode("utf-8", errors="replace")

    def list_profiles(self) -> List[str]:
        buf = ctypes.create_string_buffer(4096)
        _check(_lib().doso_sport_list_profiles(self._robot, buf, len(buf)))
        raw = buf.value.decode("utf-8", errors="replace")
        return [line for line in raw.splitlines() if line]

    def list_gestures(self) -> List[str]:
        buf = ctypes.create_string_buffer(4096)
        _check(_lib().doso_sport_list_gestures(self._robot, buf, len(buf)))
        raw = buf.value.decode("utf-8", errors="replace")
        return [line for line in raw.splitlines() if line]

    def body_height_status(self) -> BodyHeightStatus:
        raw = _CBodyHeightStatus()
        _check(_lib().doso_sport_body_height_status(self._robot, ctypes.byref(raw)))
        return BodyHeightStatus(
            supported=bool(raw.supported),
            current_height_m=raw.current_height_m,
            target_height_m=raw.target_height_m,
            min_height_m=raw.min_height_m,
            max_height_m=raw.max_height_m,
            rate_mps=raw.rate_mps,
        )

    def set_body_height(self, height_m: float) -> BodyHeightStatus:
        raw = _CBodyHeightStatus()
        _check(
            _lib().doso_sport_set_body_height(
                self._robot, float(height_m), ctypes.byref(raw)
            )
        )
        return BodyHeightStatus(
            supported=bool(raw.supported),
            current_height_m=raw.current_height_m,
            target_height_m=raw.target_height_m,
            min_height_m=raw.min_height_m,
            max_height_m=raw.max_height_m,
            rate_mps=raw.rate_mps,
        )

    @staticmethod
    def _parse_recovery_status(raw_json: str) -> RecoveryStatus:
        d = json.loads(raw_json)
        return RecoveryStatus(
            availability=int(d.get("availability", 0)),
            phase=int(d.get("phase", 0)),
            operation_id=str(d.get("operation_id", "")),
            unavailable_reason=str(d.get("unavailable_reason", "")),
            selected=bool(d.get("selected", False)),
            session_token=str(d.get("session_token", "")),
            motor_disable_confirmed=bool(d.get("motor_disable_confirmed", False)),
        )

    @staticmethod
    def _parse_recovery_action(raw_json: str) -> RecoveryActionReply:
        d = json.loads(raw_json)
        return RecoveryActionReply(
            operation_id=str(d.get("operation_id", "")),
            decision=int(d.get("decision", 0)),
            phase=int(d.get("phase", 0)),
            reason=str(d.get("reason", "")),
        )

    def recovery_status(self) -> RecoveryStatus:
        buf = ctypes.create_string_buffer(2048)
        _check(_lib().doso_sport_recovery_status_json(self._robot, buf, len(buf)))
        return self._parse_recovery_status(buf.value.decode("utf-8", errors="replace"))

    def recovery_enter(self, operation_id: str) -> RecoveryStatus:
        buf = ctypes.create_string_buffer(2048)
        _check(
            _lib().doso_sport_recovery_enter_json(
                self._robot, operation_id.encode("utf-8"), buf, len(buf)
            )
        )
        return self._parse_recovery_status(buf.value.decode("utf-8", errors="replace"))

    def recovery_enable(self, session_token: str) -> RecoveryStatus:
        buf = ctypes.create_string_buffer(2048)
        _check(
            _lib().doso_sport_recovery_enable_json(
                self._robot, session_token.encode("utf-8"), buf, len(buf)
            )
        )
        return self._parse_recovery_status(buf.value.decode("utf-8", errors="replace"))

    def recovery_heartbeat(self, session_token: str) -> RecoveryStatus:
        buf = ctypes.create_string_buffer(2048)
        _check(
            _lib().doso_sport_recovery_heartbeat_json(
                self._robot, session_token.encode("utf-8"), buf, len(buf)
            )
        )
        return self._parse_recovery_status(buf.value.decode("utf-8", errors="replace"))

    def recovery_start(
        self, operation_id: str, session_token: str
    ) -> RecoveryActionReply:
        buf = ctypes.create_string_buffer(2048)
        _check(
            _lib().doso_sport_recovery_start_json(
                self._robot,
                operation_id.encode("utf-8"),
                session_token.encode("utf-8"),
                buf,
                len(buf),
            )
        )
        return self._parse_recovery_action(buf.value.decode("utf-8", errors="replace"))

    def recovery_cancel(
        self, operation_id: str, session_token: str
    ) -> RecoveryActionReply:
        buf = ctypes.create_string_buffer(2048)
        _check(
            _lib().doso_sport_recovery_cancel_json(
                self._robot,
                operation_id.encode("utf-8"),
                session_token.encode("utf-8"),
                buf,
                len(buf),
            )
        )
        return self._parse_recovery_action(buf.value.decode("utf-8", errors="replace"))

    def recovery_exit(self, session_token: str) -> RecoveryStatus:
        buf = ctypes.create_string_buffer(2048)
        _check(
            _lib().doso_sport_recovery_exit_json(
                self._robot, session_token.encode("utf-8"), buf, len(buf)
            )
        )
        return self._parse_recovery_status(buf.value.decode("utf-8", errors="replace"))

    def take_control(self) -> TeleopSession:
        handle = _lib().doso_teleop_take_control(self._robot)
        if not handle:
            raise DosoError(_last_error())
        return TeleopSession(handle)


class NavigationClient:
    def __init__(self, robot_handle: int) -> None:
        self._robot = robot_handle

    def go_to(self, x: float, y: float, z: float = 0.0, yaw: float = 0.0) -> str:
        buf = ctypes.create_string_buffer(128)
        _check(_lib().doso_nav_go_to(self._robot, x, y, z, yaw, buf, len(buf)))
        return buf.value.decode("utf-8", errors="replace")

    def cancel(self, task_id: str, reason: str = "cancelled") -> None:
        _check(
            _lib().doso_nav_cancel(
                self._robot, task_id.encode("utf-8"), reason.encode("utf-8")
            )
        )


class VoiceClient:
    """Unified voice runtime & dialogue management client (`doso.v1.Voice`)."""

    def __init__(self, robot_handle: int) -> None:
        self._robot = robot_handle

    def status(self) -> VoiceStatus:
        buf = ctypes.create_string_buffer(8192)
        _check(_lib().doso_voice_status_json(self._robot, buf, len(buf)))
        data = json.loads(buf.value.decode("utf-8", errors="replace"))
        return VoiceStatus(
            status=str(data.get("status", "")),
            service=str(data.get("service", "")),
            active_profile=str(data.get("active_profile", "")),
            llm_provider=str(data.get("llm_provider", "")),
            llm_model=str(data.get("llm_model", "")),
            asr_provider=str(data.get("asr_provider", "")),
            tts_provider=str(data.get("tts_provider", "")),
        )

    def list_profiles(self) -> VoiceProfileList:
        buf = ctypes.create_string_buffer(16384)
        _check(_lib().doso_voice_profiles_json(self._robot, buf, len(buf)))
        data = json.loads(buf.value.decode("utf-8", errors="replace"))
        profiles = tuple(
            VoiceProfileInfo(
                profile_id=str(p.get("profile_id", "")),
                label=str(p.get("label", "")),
                voice_id=str(p.get("voice_id", "")),
                category=str(p.get("category", "")),
                speed=float(p.get("speed", 1.0)),
                volume=float(p.get("volume", 1.0)),
                pitch=float(p.get("pitch", 0.0)),
                emotion=str(p.get("emotion", "")),
            )
            for p in data.get("profiles", [])
        )
        cues = tuple(str(c) for c in data.get("available_sound_cues", []))
        return VoiceProfileList(
            active_profile=str(data.get("active_profile", "")),
            profiles=profiles,
            sound_cues_enabled=bool(data.get("sound_cues_enabled", False)),
            available_sound_cues=cues,
        )

    def switch_profile(self, profile_id: str) -> None:
        _check(_lib().doso_voice_switch_profile(self._robot, profile_id.encode("utf-8")))

    def speak(
        self,
        text: str,
        *,
        priority: str = "normal",
        queue_policy: str = "enqueue",
        voice_profile_id: Optional[str] = None,
    ) -> str:
        pb_buf = ctypes.create_string_buffer(128)
        prof_bytes = voice_profile_id.encode("utf-8") if voice_profile_id else None
        _check(
            _lib().doso_voice_speak(
                self._robot,
                text.encode("utf-8"),
                priority.encode("utf-8"),
                queue_policy.encode("utf-8"),
                prof_bytes,
                pb_buf,
                len(pb_buf),
            )
        )
        return pb_buf.value.decode("utf-8", errors="replace")

    def playback_status(self, playback_id: str) -> VoicePlayback:
        buf = ctypes.create_string_buffer(4096)
        _check(
            _lib().doso_voice_playback_status_json(
                self._robot, playback_id.encode("utf-8"), buf, len(buf)
            )
        )
        data = json.loads(buf.value.decode("utf-8", errors="replace"))
        return VoicePlayback(
            playback_id=str(data.get("playback_id", playback_id)),
            state=str(data.get("state", "")),
            priority=str(data.get("priority", "")),
            text_chars=int(data.get("text_chars", 0)),
            message=str(data.get("message", "")),
        )

    def cancel_playback(self, playback_id: str, reason: str = "operator_cancelled") -> None:
        _check(
            _lib().doso_voice_cancel_playback(
                self._robot, playback_id.encode("utf-8"), reason.encode("utf-8")
            )
        )

    def switch_component(
        self,
        component: str,
        provider: str,
        model: Optional[str] = None,
        *,
        validate: bool = False,
    ) -> None:
        model_bytes = model.encode("utf-8") if model else None
        _check(
            _lib().doso_voice_switch_component(
                self._robot,
                component.encode("utf-8"),
                provider.encode("utf-8"),
                model_bytes,
                1 if validate else 0,
            )
        )

    def update_prompt(self, system_prompt: str) -> None:
        _check(_lib().doso_voice_update_prompt(self._robot, system_prompt.encode("utf-8")))

    def chat(
        self,
        text: str,
        *,
        thread_id: Optional[str] = None,
        speak_reply: bool = False,
    ) -> VoiceChatReply:
        buf = ctypes.create_string_buffer(16384)
        tid_bytes = thread_id.encode("utf-8") if thread_id else None
        _check(
            _lib().doso_voice_chat_json(
                self._robot,
                text.encode("utf-8"),
                tid_bytes,
                1 if speak_reply else 0,
                buf,
                len(buf),
            )
        )
        data = json.loads(buf.value.decode("utf-8", errors="replace"))
        return VoiceChatReply(
            reply_text=str(data.get("reply_text", "")),
            thread_id=str(data.get("thread_id", "")),
            turn_id=str(data.get("turn_id", "")),
            latency_ms=float(data.get("latency_ms", 0.0)),
        )


class RobotClient:
    """Unified customer entry point representing the entire DOSO robot."""

    def __init__(self, handle: int) -> None:
        self._handle: Optional[int] = handle

    @classmethod
    def connect(
        cls, endpoint: str, api_key: Optional[str] = None, timeout_ms: int = 10000
    ) -> "RobotClient":
        key_bytes = api_key.encode("utf-8") if api_key else None
        handle = _lib().doso_robot_connect(endpoint.encode("utf-8"), key_bytes, timeout_ms)
        if not handle:
            raise DosoError(_last_error())
        return cls(handle)

    def close(self) -> None:
        if self._handle is not None:
            _lib().doso_robot_free(self._handle)
            self._handle = None

    def __enter__(self) -> "RobotClient":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    def __del__(self) -> None:
        self.close()

    def ping(self) -> None:
        _check(_lib().doso_robot_ping(self._handle))

    @property
    def robot_id(self) -> str:
        return _read_str("doso_robot_id", self._handle)

    @property
    def firmware_version(self) -> str:
        return _read_str("doso_firmware_version", self._handle)

    @property
    def robot_version(self) -> str:
        return _read_str("doso_robot_version", self._handle)

    @property
    def low_level_endpoint(self) -> str:
        return _read_str("doso_low_level_endpoint", self._handle)

    def capabilities(self) -> Capabilities:
        raw = _CCapabilities()
        _check(_lib().doso_robot_capabilities(self._handle, ctypes.byref(raw)))
        return Capabilities(
            motion=bool(raw.motion),
            low_level=bool(raw.low_level),
            observations=bool(raw.observations),
            navigation=bool(raw.navigation),
            maps=bool(raw.maps),
            localization=bool(raw.localization),
            tasks=bool(raw.tasks),
            board_b_autonomy=bool(raw.board_b_autonomy),
            voice=bool(raw.voice),
        )

    def sport(self) -> SportClient:
        return SportClient(self._handle)

    def motion(self) -> SportClient:
        return SportClient(self._handle)

    def observations(self) -> ObservationsClient:
        return ObservationsClient(self._handle)

    def navigation(self) -> NavigationClient:
        return NavigationClient(self._handle)

    def voice(self) -> VoiceClient:
        return VoiceClient(self._handle)

    def low_level(self) -> LowLevelSession:
        handle = _lib().doso_low_level_enter(self._handle)
        if not handle:
            raise DosoError(_last_error())
        return LowLevelSession(handle)


DosoClient = RobotClient
