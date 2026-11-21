"""ctypes binding over the shared Rust doso_sdk_ffi core."""

from __future__ import annotations

import ctypes
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

    repo_root = Path(__file__).resolve().parents[2]
    candidates = [
        repo_root / "ffi" / "target" / "release" / lib_name,
        repo_root / "ffi" / "target" / "debug" / lib_name,
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
        )

    def sport(self) -> SportClient:
        return SportClient(self._handle)

    def motion(self) -> SportClient:
        return SportClient(self._handle)

    def observations(self) -> ObservationsClient:
        return ObservationsClient(self._handle)

    def navigation(self) -> NavigationClient:
        return NavigationClient(self._handle)

    def low_level(self) -> LowLevelSession:
        handle = _lib().doso_low_level_enter(self._handle)
        if not handle:
            raise DosoError(_last_error())
        return LowLevelSession(handle)


DosoClient = RobotClient
