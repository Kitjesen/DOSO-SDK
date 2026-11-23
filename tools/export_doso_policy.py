#!/usr/bin/env python3
"""DOSO Policy Packager, Inspector & 1-Second Zero-Restart Hot Deployment Tool.

Usage examples:
  1. Inspect any exported ONNX checkpoint or policy package directory:
     python tools/export_doso_policy.py inspect ./policy.onnx
     python tools/export_doso_policy.py inspect ./models/packages/flat_w5

  2. 1-command end-to-end deploy (inspect + shorthand YAML + 2-step Golden + hot-push):
     python tools/export_doso_policy.py deploy ./policy.onnx \
         --preset flat_53d \
         --joint-order isaaclab_joint_major \
         --admin-url http://192.168.66.13:5054 \
         --slot flat \
         --activate

  3. Offline dry-run package + auto-golden generation without HTTP push:
     python tools/export_doso_policy.py deploy ./policy.onnx --preset cts_stairs --dry-run

  4. Legacy subcommands (pack / push / rollback):
     python tools/export_doso_policy.py pack --onnx ./policy.onnx --name parkour_v1 --template height --out-dir ./out/parkour_v1
     python tools/export_doso_policy.py push --pkg-dir ./models/packages/single_frame_height_v2 --admin-url http://192.168.66.13:5054 --slot height
     python tools/export_doso_policy.py rollback --admin-url http://192.168.66.13:5054 --slot height
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import math
import shutil
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any


THUNDER_V4_STANDING: list[float] = [
    -0.12, -0.93, 1.96,
    0.12, 0.93, -1.96,
    0.12, 0.93, -1.96,
    -0.12, -0.93, 1.96,
    0.0, 0.0, 0.0, 0.0,
]

HEIGHT_RECOVERY_STANDING: list[float] = [
    -0.1, -0.8, 1.8,
    0.1, 0.8, -1.8,
    0.1, 0.8, -1.8,
    -0.1, -0.8, 1.8,
    0.0, 0.0, 0.0, 0.0,
]

# Maps Policy index p_idx (0..16) -> Canonical Thunder Hardware index hw_idx (0..16).
JOINT_ORDERS: dict[str, list[int]] = {
    "thunder_leg_major": [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    "isaaclab_joint_major": [0, 3, 6, 9, 1, 4, 7, 10, 2, 5, 8, 11, 12, 13, 14, 15],
    "isaaclab_fl_first": [3, 0, 9, 6, 4, 1, 10, 7, 5, 2, 11, 8, 13, 12, 15, 14],
    "go2_fl_first_leg_major": [3, 4, 5, 0, 1, 2, 9, 10, 11, 6, 7, 8, 13, 12, 15, 14],
}

FLAT_53D_OBS = ["ang_vel", "gravity", "cmd_vel", "dof_pos_12", "dof_vel_16", "last_action"]
HEIGHT_58D_OBS = [
    "ang_vel",
    "gravity",
    "cmd_vel",
    "cmd_height",
    "dof_pos_16_zero_wheels",
    "dof_vel_16",
    "last_action",
]
RECOVERY_60D_OBS = [
    "ang_vel",
    "gravity",
    "zeros(3)",
    "dof_pos_12",
    "zeros(4)",
    "dof_vel_16",
    "last_action",
    "const(2.0)",
    "phase_clock(1.2)",
]


@dataclass(frozen=True)
class ChannelPreset:
    default_name: str
    total_dim: int
    history_len: int
    layout: str  # "frame_major" | "term_major"
    clip: tuple[float, float] | None
    obs: list[str]


@dataclass(frozen=True)
class DeployPreset:
    key: str
    kind: str
    description: str
    standing_pose: list[float]
    override_standing_pose: bool
    channels: list[ChannelPreset]
    breakdown: str
    action_decode_inline: str
    default_kp: tuple[float, float, float]
    default_kd: tuple[float, float, float, float]
    body_height_cmd: float
    capabilities_yaml: str | None = None


PRESETS: dict[str, DeployPreset] = {
    "flat_53d": DeployPreset(
        key="flat_53d",
        kind="locomotion",
        description="Thunder V4 53D single-frame flat locomotion policy",
        standing_pose=THUNDER_V4_STANDING,
        override_standing_pose=False,
        channels=[
            ChannelPreset("obs", 53, 1, "frame_major", (-100.0, 100.0), FLAT_53D_OBS),
        ],
        breakdown="53D = ang_vel(3) + gravity(3) + cmd_vel(3) + dof_pos_12(12) + dof_vel_16(16) + last_action(16)",
        action_decode_inline="{ scale_per_group: [0.125, 0.25, 0.25, 5.0] }",
        default_kp=(73.0, 80.0, 80.0),
        default_kd=(6.93, 6.93, 6.93, 1.0),
        body_height_cmd=0.0,
    ),
    "cts_stairs": DeployPreset(
        key="cts_stairs",
        kind="locomotion",
        description="Thunder V4 CTS stairs dual-input policy (530D term-major history + 53D single_obs)",
        standing_pose=THUNDER_V4_STANDING,
        override_standing_pose=False,
        channels=[
            ChannelPreset("history", 530, 10, "term_major", (-100.0, 100.0), FLAT_53D_OBS),
            ChannelPreset("single_obs", 53, 1, "frame_major", (-100.0, 100.0), FLAT_53D_OBS),
        ],
        breakdown="530D (10x53D term_major history) + 53D (single_obs)",
        action_decode_inline="{ scale_per_group: [0.25, 0.25, 0.25, 5.0], raw_clip: [-100.0, 100.0], leg_rate_limit_rad_s: 12.0 }",
        default_kp=(90.0, 120.0, 120.0),
        default_kd=(7.0, 12.0, 12.0, 1.0),
        body_height_cmd=0.0,
    ),
    "height_58d": DeployPreset(
        key="height_58d",
        kind="locomotion",
        description="Thunder V4 58D body-height locomotion policy",
        standing_pose=HEIGHT_RECOVERY_STANDING,
        override_standing_pose=True,
        channels=[
            ChannelPreset("obs", 58, 1, "frame_major", None, HEIGHT_58D_OBS),
        ],
        breakdown="58D = ang_vel(3) + gravity(3) + cmd_vel(3) + cmd_height(1) + dof_pos_16_zero_wheels(16) + dof_vel_16(16) + last_action(16)",
        action_decode_inline="{ scale_per_group: [0.125, 0.25, 0.25, 5.0], force_zero_wheel_on_stop: true }",
        default_kp=(100.0, 100.0, 100.0),
        default_kd=(5.0, 5.0, 5.0, 1.0),
        body_height_cmd=0.375,
        capabilities_yaml=(
            "capabilities:\n"
            "  body_height: { default_m: 0.375, min_m: 0.25, max_m: 0.50, rate_mps: 0.05, handover_frames: 100 }"
        ),
    ),
    "recovery_360d": DeployPreset(
        key="recovery_360d",
        kind="recovery",
        description="Thunder V4 360D (6x60D) fall-recovery policy",
        standing_pose=HEIGHT_RECOVERY_STANDING,
        override_standing_pose=True,
        channels=[
            ChannelPreset("history", 360, 6, "frame_major", (-100.0, 100.0), RECOVERY_60D_OBS),
        ],
        breakdown="360D = 6 x [ang_vel(3) + gravity(3) + zeros(3) + dof_pos_12(12) + zeros(4) + dof_vel_16(16) + last_action(16) + const(1) + phase_clock(2)]",
        action_decode_inline="{ scale_per_group: [0.125, 0.25, 0.25, 0.0], max_abs_raw_guard: 4.0, force_zero_wheel_on_stop: true }",
        default_kp=(80.0, 80.0, 80.0),
        default_kd=(10.0, 10.0, 10.0, 1.0),
        body_height_cmd=0.0,
    ),
}


# Legacy `pack` templates preserved for backwards compatibility.
TEMPLATES = {
    "flat": """schema_version: 2
name: {name}
description: "Auto-packaged Thunder 53D flat locomotion policy"
kind: locomotion
robot: thunder_v4
model:
  file: "model.onnx"
  sha256: "{sha256}"
  step_dt_s: 0.02
inputs:
  - tensor: "@0"
    clip: [-100.0, 100.0]
    obs: [ang_vel, gravity, cmd_vel, dof_pos_12, dof_vel_16, last_action]
outputs:
  action_tensor: "@0"
action_decode: {{ scale_per_group: [0.125, 0.25, 0.25, 5.0] }}
gains:
  kp: {{ hip: 73.0, thigh: 80.0, calf: 80.0 }}
  kd: {{ hip: 6.93, thigh: 6.93, calf: 6.93, wheel: 1.0 }}
""",
    "height": """schema_version: 2
name: {name}
description: "Auto-packaged Thunder 58D body-height locomotion policy"
kind: locomotion
robot: thunder_v4
model:
  file: "model.onnx"
  sha256: "{sha256}"
  step_dt_s: 0.02
posture:
  standing_pose: [-0.1, -0.8, 1.8, 0.1, 0.8, -1.8, 0.1, 0.8, -1.8, -0.1, -0.8, 1.8, 0.0, 0.0, 0.0, 0.0]
inputs:
  - tensor: "@0"
    obs: [ang_vel, gravity, cmd_vel, cmd_height, dof_pos_16_zero_wheels, dof_vel_16, last_action]
outputs:
  action_tensor: "@0"
action_decode: {{ scale_per_group: [0.125, 0.25, 0.25, 5.0], force_zero_wheel_on_stop: true }}
gains:
  kp: {{ hip: 100.0, thigh: 100.0, calf: 100.0 }}
  kd: {{ hip: 5.0, thigh: 5.0, calf: 5.0, wheel: 1.0 }}
capabilities:
  body_height: {{ default_m: 0.375, min_m: 0.25, max_m: 0.50, rate_mps: 0.05, handover_frames: 100 }}
""",
}


@dataclass
class TensorMeta:
    name: str
    shape: list[int | str]
    elem_type: str

    @property
    def feature_dim(self) -> int | None:
        if not self.shape:
            return None
        last = self.shape[-1]
        return last if isinstance(last, int) and last > 0 else None


@dataclass
class OnnxInspection:
    sha256: str
    size_bytes: int
    backend: str
    inputs: list[TensorMeta]
    outputs: list[TensorMeta]
    session: Any = None


# ─── Pure-Python ONNX Protobuf Wire-Format Fallback ─────────────────────────

_ONNX_ELEM_TYPES = {
    1: "tensor(float)",
    2: "tensor(uint8)",
    3: "tensor(int8)",
    6: "tensor(int32)",
    7: "tensor(int64)",
    10: "tensor(float16)",
    11: "tensor(double)",
}


def _read_varint(buf: memoryview, pos: int) -> tuple[int, int]:
    result = 0
    shift = 0
    n = len(buf)
    while pos < n:
        b = buf[pos]
        pos += 1
        result |= (b & 0x7F) << shift
        if not (b & 0x80):
            return result, pos
        shift += 7
        if shift > 70:
            raise ValueError("corrupted protobuf varint")
    raise ValueError("truncated protobuf varint")


def _iter_pb_fields(buf: memoryview):
    pos = 0
    n = len(buf)
    while pos < n:
        tag, pos = _read_varint(buf, pos)
        field_num = tag >> 3
        wire_type = tag & 0x07
        if wire_type == 0:
            val, pos = _read_varint(buf, pos)
            yield field_num, wire_type, val
        elif wire_type == 1:
            if pos + 8 > n:
                raise ValueError("truncated fixed64")
            yield field_num, wire_type, buf[pos : pos + 8]
            pos += 8
        elif wire_type == 2:
            length, pos = _read_varint(buf, pos)
            if pos + length > n:
                raise ValueError("truncated length-delimited field")
            yield field_num, wire_type, buf[pos : pos + length]
            pos += length
        elif wire_type == 5:
            if pos + 4 > n:
                raise ValueError("truncated fixed32")
            yield field_num, wire_type, buf[pos : pos + 4]
            pos += 4
        else:
            raise ValueError(f"unsupported protobuf wire_type={wire_type}")


def _parse_value_info_proto(raw: memoryview) -> TensorMeta:
    name = ""
    elem_type = "tensor(unknown)"
    shape: list[int | str] = []
    for fnum, wtype, val in _iter_pb_fields(raw):
        if fnum == 1 and wtype == 2:
            name = bytes(val).decode("utf-8", errors="replace")
        elif fnum == 2 and wtype == 2:  # TypeProto
            for tfnum, twtype, tval in _iter_pb_fields(val):
                if tfnum == 1 and twtype == 2:  # TypeProto.Tensor
                    for ttf, ttw, ttv in _iter_pb_fields(tval):
                        if ttf == 1 and ttw == 0:
                            elem_type = _ONNX_ELEM_TYPES.get(int(ttv), f"tensor(type={ttv})")
                        elif ttf == 2 and ttw == 2:  # TensorShapeProto
                            for sf, sw, sv in _iter_pb_fields(ttv):
                                if sf == 1 and sw == 2:  # Dimension
                                    dim_item: int | str = "?"
                                    for df, dw, dv in _iter_pb_fields(sv):
                                        if df == 1 and dw == 0:
                                            # Handle signed int64 (-1 dynamic dim)
                                            iv = int(dv)
                                            if iv >= (1 << 63):
                                                iv -= 1 << 64
                                            dim_item = iv if iv > 0 else "dynamic"
                                        elif df == 2 and dw == 2:
                                            dim_item = bytes(dv).decode("utf-8", errors="replace") or "dynamic"
                                    shape.append(dim_item)
    return TensorMeta(name=name, shape=shape, elem_type=elem_type)


def _parse_tensor_proto_name(raw: memoryview) -> str | None:
    for fnum, wtype, val in _iter_pb_fields(raw):
        if fnum == 8 and wtype == 2:
            return bytes(val).decode("utf-8", errors="replace")
    return None


def _inspect_onnx_protobuf_fallback(onnx_bytes: bytes, sha256: str) -> OnnxInspection:
    view = memoryview(onnx_bytes)
    graph_view: memoryview | None = None
    for fnum, wtype, val in _iter_pb_fields(view):
        if fnum == 7 and wtype == 2:  # ModelProto.graph
            graph_view = val
            break
    if graph_view is None:
        raise ValueError("Invalid ONNX file: missing ModelProto.graph (field 7)")

    initializer_names: set[str] = set()
    raw_inputs: list[TensorMeta] = []
    outputs: list[TensorMeta] = []
    for fnum, wtype, val in _iter_pb_fields(graph_view):
        if fnum == 5 and wtype == 2:  # GraphProto.initializer
            init_name = _parse_tensor_proto_name(val)
            if init_name:
                initializer_names.add(init_name)
        elif fnum == 11 and wtype == 2:  # GraphProto.input
            raw_inputs.append(_parse_value_info_proto(val))
        elif fnum == 12 and wtype == 2:  # GraphProto.output
            outputs.append(_parse_value_info_proto(val))

    inputs = [m for m in raw_inputs if m.name not in initializer_names]
    return OnnxInspection(
        sha256=sha256,
        size_bytes=len(onnx_bytes),
        backend="protobuf-wire-fallback",
        inputs=inputs,
        outputs=outputs,
        session=None,
    )


def inspect_onnx_bytes(onnx_bytes: bytes, require_ort: bool = False) -> OnnxInspection:
    sha = hashlib.sha256(onnx_bytes).hexdigest()
    try:
        import onnxruntime as ort  # type: ignore
    except ImportError as exc:
        if require_ort:
            raise RuntimeError(
                "onnxruntime is required for 'deploy' auto-golden generation (install with: pip install onnxruntime)"
            ) from exc
        return _inspect_onnx_protobuf_fallback(onnx_bytes, sha)

    sess_opts = ort.SessionOptions()
    sess_opts.intra_op_num_threads = 1
    sess_opts.log_severity_level = 3
    sess = ort.InferenceSession(
        onnx_bytes,
        sess_options=sess_opts,
        providers=["CPUExecutionProvider"],
    )
    inputs = [
        TensorMeta(
            name=inp.name,
            shape=[d if (isinstance(d, int) and d > 0) else (str(d) if d is not None else "dynamic") for d in inp.shape],
            elem_type=inp.type,
        )
        for inp in sess.get_inputs()
    ]
    outputs = [
        TensorMeta(
            name=out.name,
            shape=[d if (isinstance(d, int) and d > 0) else (str(d) if d is not None else "dynamic") for d in out.shape],
            elem_type=out.type,
        )
        for out in sess.get_outputs()
    ]
    return OnnxInspection(
        sha256=sha,
        size_bytes=len(onnx_bytes),
        backend=f"onnxruntime {ort.__version__}",
        inputs=inputs,
        outputs=outputs,
        session=sess,
    )


def suggest_preset_for_dims(in_dims: list[int | None]) -> str | None:
    if in_dims == [53]:
        return "flat_53d"
    if sorted(d for d in in_dims if d is not None) == [53, 530] and len(in_dims) == 2:
        return "cts_stairs"
    if in_dims == [58]:
        return "height_58d"
    if in_dims == [360]:
        return "recovery_360d"
    return None


def match_channels_to_onnx(preset: DeployPreset, info: OnnxInspection) -> tuple[list[tuple[str, ChannelPreset]], str, list[str]]:
    in_dims = [t.feature_dim for t in info.inputs]
    in_desc = ", ".join(f"'{t.name}' shape={t.shape}" for t in info.inputs) or "<none>"

    if len(info.inputs) != len(preset.channels):
        hint = suggest_preset_for_dims(in_dims)
        hint_msg = f" (Hint: this model matches `--preset {hint}`!)" if hint and hint != preset.key else ""
        raise ValueError(
            f"Preset '{preset.key}' expects {len(preset.channels)} input tensor(s) ({preset.breakdown}), "
            f"but ONNX has {len(info.inputs)}: [{in_desc}]{hint_msg}"
        )

    for t in info.inputs:
        if len(t.shape) >= 2 and isinstance(t.shape[0], int) and t.shape[0] != 1:
            raise ValueError(
                f"ONNX input '{t.name}' has static batch size {t.shape[0]} (shape={t.shape}); expected batch=1 or dynamic"
            )

    bound_channels: list[tuple[str, ChannelPreset]] = []
    if len(preset.channels) == 1:
        ch = preset.channels[0]
        actual_dim = info.inputs[0].feature_dim
        if actual_dim is not None and actual_dim != ch.total_dim:
            hint = suggest_preset_for_dims(in_dims)
            hint_msg = f" (Hint: this model matches `--preset {hint}`!)" if hint and hint != preset.key else ""
            raise ValueError(
                f"Preset '{preset.key}' input dim mismatch: expected {ch.total_dim} ({preset.breakdown}), "
                f"got ONNX input [{in_desc}]{hint_msg}"
            )
        tensor_name = info.inputs[0].name or "@0"
        bound_channels.append((tensor_name, ch))
    else:
        # Match multi-input channels (e.g. cts_stairs 530D + 53D) in exact ONNX input order.
        remaining = list(preset.channels)
        for inp in info.inputs:
            matched: ChannelPreset | None = None
            for cand in remaining:
                if inp.feature_dim == cand.total_dim:
                    matched = cand
                    break
            if matched is None:
                hint = suggest_preset_for_dims(in_dims)
                hint_msg = f" (Hint: this model matches `--preset {hint}`!)" if hint and hint != preset.key else ""
                raise ValueError(
                    f"Preset '{preset.key}' input dim mismatch: expected ({preset.breakdown}), "
                    f"got ONNX inputs [{in_desc}]{hint_msg}"
                )
            remaining.remove(matched)
            bound_channels.append((inp.name, matched))

    if not info.outputs:
        raise ValueError("ONNX model declares 0 output tensors")

    action_out: TensorMeta | None = None
    for out in info.outputs:
        if out.feature_dim == 16 or out.feature_dim is None:
            action_out = out
            break
    if action_out is None:
        out_desc = ", ".join(f"'{o.name}' shape={o.shape}" for o in info.outputs)
        raise ValueError(
            f"Expected a 16-DOF action output tensor in ONNX outputs, got [{out_desc}]"
        )

    action_tensor = action_out.name or "@0"
    aux_tensors = [
        o.name
        for o in info.outputs
        if o.name != action_out.name and o.name and (o.feature_dim is not None and 0 < o.feature_dim <= 64)
    ]
    return bound_channels, action_tensor, aux_tensors


def parse_kp_arg(raw: str | None, default: tuple[float, float, float]) -> tuple[float, float, float]:
    if raw is None:
        return default
    parts = [p.strip() for p in raw.split(",") if p.strip()]
    if len(parts) != 3:
        raise ValueError(f"--kp expects 3 comma-separated floats 'hip,thigh,calf' (e.g. 90,120,120), got '{raw}'")
    vals = tuple(float(x) for x in parts)
    if any((not math.isfinite(v) or v < 0.0) for v in vals):
        raise ValueError(f"--kp values must be finite and >= 0, got {vals}")
    return (vals[0], vals[1], vals[2])


def parse_kd_arg(raw: str | None, default: tuple[float, float, float, float]) -> tuple[float, float, float, float]:
    if raw is None:
        return default
    parts = [p.strip() for p in raw.split(",") if p.strip()]
    if len(parts) != 4:
        raise ValueError(
            f"--kd expects 4 comma-separated floats 'hip,thigh,calf,wheel' (e.g. 7,12,12,1), got '{raw}'"
        )
    vals = tuple(float(x) for x in parts)
    if any((not math.isfinite(v) or v < 0.0) for v in vals):
        raise ValueError(f"--kd values must be finite and >= 0, got {vals}")
    return (vals[0], vals[1], vals[2], vals[3])


def _fmt_num(v: float) -> str:
    return f"{v:g}" if ("." in f"{v:g}" or "e" in f"{v:g}") else f"{v:.1f}"


def _fmt_obs_item(tok: str) -> str:
    return f'"{tok}"' if ("(" in tok or ")" in tok) else tok


def build_shorthand_yaml(
    name: str,
    sha256: str,
    preset: DeployPreset,
    joint_order: str,
    bound_channels: list[tuple[str, ChannelPreset]],
    action_tensor: str,
    aux_tensors: list[str],
    kp: tuple[float, float, float],
    kd: tuple[float, float, float, float],
) -> str:
    lines = [
        "schema_version: 2",
        f"name: {name}",
        f'description: "{preset.description}"',
        f"kind: {preset.kind}",
        "robot: thunder_v4",
        f"joint_order: {joint_order}",
        "model:",
        '  file: "model.onnx"',
        f'  sha256: "{sha256}"',
        "  step_dt_s: 0.02",
    ]
    if preset.override_standing_pose:
        sp_str = ", ".join(_fmt_num(x) for x in preset.standing_pose)
        lines.append("posture:")
        lines.append(f"  standing_pose: [{sp_str}]")

    lines.append("inputs:")
    for tensor_name, ch in bound_channels:
        lines.append(f'  - tensor: "{tensor_name}"')
        if ch.history_len > 1 or ch.layout != "frame_major":
            lines.append(
                f"    history: {{ length: {ch.history_len}, layout: {ch.layout}, order: oldest_first, padding: repeat_first }}"
            )
        if ch.clip is not None:
            lines.append(f"    clip: [{_fmt_num(ch.clip[0])}, {_fmt_num(ch.clip[1])}]")
        obs_str = ", ".join(_fmt_obs_item(t) for t in ch.obs)
        lines.append(f"    obs: [{obs_str}]")

    if aux_tensors:
        aux_str = ", ".join(f'"{a}"' for a in aux_tensors)
        lines.append(f'outputs: {{ action_tensor: "{action_tensor}", aux_tensors: [{aux_str}] }}')
    else:
        lines.append(f'outputs: {{ action_tensor: "{action_tensor}" }}')

    lines.append(f"action_decode: {preset.action_decode_inline}")
    lines.append("gains:")
    lines.append(f"  kp: {{ hip: {_fmt_num(kp[0])}, thigh: {_fmt_num(kp[1])}, calf: {_fmt_num(kp[2])} }}")
    lines.append(
        f"  kd: {{ hip: {_fmt_num(kd[0])}, thigh: {_fmt_num(kd[1])}, calf: {_fmt_num(kd[2])}, wheel: {_fmt_num(kd[3])} }}"
    )
    if preset.capabilities_yaml:
        lines.append(preset.capabilities_yaml)

    return "\n".join(lines) + "\n"


# ─── Exact Bidirectional Python Golden Trajectory Generator ──────────────────

def _apply_clip(v: float, clip: tuple[float, float] | None) -> float:
    if clip is None:
        return v
    lo, hi = clip
    return lo if v < lo else (hi if v > hi else v)


def _encode_single_frame(
    obs_tokens: list[str],
    clip: tuple[float, float] | None,
    joint_map: list[int],
    standing_pose: list[float],
    gyro_rad_s: list[float],
    projected_gravity: list[float],
    velocity_cmd: list[float],
    body_height_cmd: float,
    q_hw: list[float],
    dq_hw: list[float],
    last_raw_hw: list[float],
    step_index: int,
    step_dt_s: float = 0.02,
) -> tuple[list[float], list[int]]:
    frame: list[float] = []
    widths: list[int] = []
    for tok in obs_tokens:
        if tok == "ang_vel":
            frame.extend(_apply_clip(g, clip) * 0.25 for g in gyro_rad_s)
            widths.append(3)
        elif tok == "gravity":
            frame.extend(_apply_clip(pg, clip) for pg in projected_gravity)
            widths.append(3)
        elif tok == "cmd_vel":
            frame.extend(_apply_clip(c, clip) * 1.0 for c in velocity_cmd)
            widths.append(3)
        elif tok == "cmd_height":
            frame.append(body_height_cmd)
            widths.append(1)
        elif tok == "dof_pos_12":
            for p_idx in range(16):
                hw_idx = joint_map[p_idx]
                if hw_idx < 12:
                    frame.append(_apply_clip(q_hw[hw_idx] - standing_pose[hw_idx], clip))
            widths.append(12)
        elif tok == "dof_pos_16_zero_wheels":
            for p_idx in range(16):
                hw_idx = joint_map[p_idx]
                if hw_idx < 12:
                    frame.append(_apply_clip(q_hw[hw_idx] - standing_pose[hw_idx], clip))
                else:
                    frame.append(0.0)
            widths.append(16)
        elif tok == "dof_vel_16":
            for p_idx in range(16):
                hw_idx = joint_map[p_idx]
                frame.append(_apply_clip(dq_hw[hw_idx], clip) * 0.05)
            widths.append(16)
        elif tok == "last_action":
            for p_idx in range(16):
                hw_idx = joint_map[p_idx]
                frame.append(_apply_clip(last_raw_hw[hw_idx], clip))
            widths.append(16)
        elif tok.startswith("zeros(") and tok.endswith(")"):
            n = int(tok[6:-1])
            frame.extend([0.0] * n)
            widths.append(n)
        elif tok.startswith("const(") and tok.endswith(")"):
            v = float(tok[6:-1])
            frame.append(v)
            widths.append(1)
        elif tok.startswith("phase_clock(") and tok.endswith(")"):
            period_s = float(tok[12:-1])
            phase = 2.0 * math.pi * (step_index * step_dt_s) / period_s
            frame.append(math.sin(phase))
            frame.append(math.cos(phase))
            widths.append(2)
        else:
            raise ValueError(f"Unsupported obs block '{tok}' in Python Golden generator")
    return frame, widths


def _pack_history(frames: list[list[float]], widths: list[int], layout: str) -> list[float]:
    if layout == "frame_major":
        out: list[float] = []
        for f in frames:
            out.extend(f)
        return out
    if layout == "term_major":
        out = []
        offset = 0
        for w in widths:
            for f in frames:
                out.extend(f[offset : offset + w])
            offset += w
        return out
    raise ValueError(f"Unsupported history layout '{layout}'")


def generate_golden_suite(
    session: Any,
    preset: DeployPreset,
    joint_order: str,
    bound_channels: list[tuple[str, ChannelPreset]],
    action_tensor: str,
) -> dict[str, Any]:
    import numpy as np  # Installed as a core dependency of onnxruntime

    joint_map = JOINT_ORDERS[joint_order]
    synthetic_inputs = [
        {
            "gyro_rad_s": [0.12, -0.08, 0.05],
            "projected_gravity": [0.03, -0.04, -0.9987492],
            "velocity_cmd": [0.4, -0.1, 0.2],
            "body_height_cmd": preset.body_height_cmd,
            "q": [-0.10, -0.88, 1.90, 0.11, 0.89, -1.92, 0.13, 0.91, -1.94, -0.09, -0.87, 1.88, 1.2, -0.5, 0.8, -0.3],
            "dq": [0.2, -0.3, 0.4, -0.1, 0.25, -0.35, 0.15, -0.2, 0.3, -0.15, 0.2, -0.25, 3.5, -2.0, 4.0, -1.5],
        },
        {
            "gyro_rad_s": [0.14, -0.09, 0.06],
            "projected_gravity": [0.03, -0.04, -0.9987492],
            "velocity_cmd": [0.4, -0.1, 0.2],
            "body_height_cmd": preset.body_height_cmd,
            "q": [-0.11, -0.89, 1.91, 0.115, 0.90, -1.93, 0.125, 0.92, -1.95, -0.10, -0.88, 1.89, 1.3, -0.4, 0.9, -0.2],
            "dq": [0.22, -0.28, 0.38, -0.12, 0.24, -0.32, 0.14, -0.18, 0.28, -0.14, 0.18, -0.22, 3.6, -1.8, 3.9, -1.4],
        },
    ]

    channel_histories: list[list[list[float]]] = [[] for _ in bound_channels]
    last_raw_hw = [0.0] * 16
    steps_out: list[dict[str, Any]] = []

    out_name = session.get_outputs()[0].name if action_tensor == "@0" else action_tensor

    for step_idx, sc in enumerate(synthetic_inputs):
        feed: dict[str, Any] = {}
        for ch_idx, (tensor_name, ch) in enumerate(bound_channels):
            single_f64, widths = _encode_single_frame(
                obs_tokens=ch.obs,
                clip=ch.clip,
                joint_map=joint_map,
                standing_pose=preset.standing_pose,
                gyro_rad_s=sc["gyro_rad_s"],
                projected_gravity=sc["projected_gravity"],
                velocity_cmd=sc["velocity_cmd"],
                body_height_cmd=sc["body_height_cmd"],
                q_hw=sc["q"],
                dq_hw=sc["dq"],
                last_raw_hw=last_raw_hw,
                step_index=step_idx,
            )
            hist = channel_histories[ch_idx]
            if not hist:
                # RepeatFirst history initialization
                hist.extend(list(single_f64) for _ in range(ch.history_len))
            elif ch.history_len == 1:
                hist[0] = list(single_f64)
            else:
                hist.pop(0)
                hist.append(list(single_f64))

            packed_f64 = _pack_history(hist, widths, ch.layout)
            actual_in_name = session.get_inputs()[ch_idx].name if tensor_name.startswith("@") else tensor_name
            feed[actual_in_name] = np.asarray(packed_f64, dtype=np.float32).reshape(1, ch.total_dim)

        raw_policy = [float(x) for x in session.run([out_name], feed)[0].reshape(-1)]
        if len(raw_policy) != 16:
            raise RuntimeError(f"Expected 16 action outputs from '{out_name}', got {len(raw_policy)}")
        if any(not math.isfinite(x) for x in raw_policy):
            raise RuntimeError(f"ONNX produced non-finite output at synthetic step {step_idx}: {raw_policy}")

        # Permute Policy order -> Canonical Thunder Hardware order
        raw_hw = [0.0] * 16
        for p_idx, val in enumerate(raw_policy):
            raw_hw[joint_map[p_idx]] = val

        last_raw_hw = list(raw_hw)
        steps_out.append(
            {
                "gyro_rad_s": sc["gyro_rad_s"],
                "projected_gravity": sc["projected_gravity"],
                "velocity_cmd": sc["velocity_cmd"],
                "body_height_cmd": sc["body_height_cmd"],
                "q": sc["q"],
                "dq": sc["dq"],
                "expected_raw_action": raw_hw,
            }
        )

    return {
        "max_abs_tolerance": 1e-4,
        "steps": steps_out,
    }


def post_json(url: str, payload: dict) -> dict:
    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(req, timeout=15) as resp:
            return json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        err_text = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"HTTP {exc.code}: {err_text}") from exc


# ─── CLI Subcommand Handlers ─────────────────────────────────────────────────

def _parse_manifest_field(yaml_text: str, key: str) -> str | None:
    prefix = f"{key}:"
    for line in yaml_text.splitlines():
        s = line.strip()
        if s.startswith(prefix):
            return s.split(":", 1)[1].strip().strip('"').strip("'")
    return None


def cmd_inspect(args: argparse.Namespace) -> int:
    target = Path(args.onnx).resolve()
    pkg_dir: Path | None = None
    manifest_name: str | None = None
    manifest_sha: str | None = None

    if target.is_dir():
        pkg_dir = target
        manifest_path = pkg_dir / "policy.yaml"
        model_rel = "model.onnx"
        if manifest_path.is_file():
            yaml_text = manifest_path.read_text(encoding="utf-8")
            manifest_name = _parse_manifest_field(yaml_text, "name")
            manifest_sha = _parse_manifest_field(yaml_text, "sha256")
            parsed_file = _parse_manifest_field(yaml_text, "file")
            if parsed_file:
                model_rel = parsed_file
        onnx_src = (pkg_dir / model_rel).resolve()
    else:
        onnx_src = target

    if not onnx_src.is_file():
        print(f"ERROR: ONNX file not found: {onnx_src}", file=sys.stderr)
        return 1
    onnx_bytes = onnx_src.read_bytes()
    info = inspect_onnx_bytes(onnx_bytes, require_ort=False)
    in_dims = [t.feature_dim for t in info.inputs]
    suggested = suggest_preset_for_dims(in_dims)

    if pkg_dir is not None:
        print(f"Package: {pkg_dir}" + (f" (name={manifest_name})" if manifest_name else ""))
    print(f"File:    {onnx_src}")
    print(f"Size:    {info.size_bytes:,} bytes ({info.size_bytes / 1024.0:.1f} KiB)")
    print(f"SHA256:  {info.sha256}")
    if manifest_sha:
        status = "MATCH" if manifest_sha.lower() == info.sha256.lower() else f"MISMATCH (manifest={manifest_sha})"
        print(f"Verify:  policy.yaml sha256 {status}")
    print(f"Backend: {info.backend}")
    print(f"Inputs ({len(info.inputs)}):")
    for idx, t in enumerate(info.inputs):
        print(f"  [{idx}] {t.name}: shape={t.shape} type={t.elem_type}")
    print(f"Outputs ({len(info.outputs)}):")
    for idx, t in enumerate(info.outputs):
        print(f"  [{idx}] {t.name}: shape={t.shape} type={t.elem_type}")
    if suggested:
        print(f"Preset:  matches --preset {suggested}")
    return 0


def cmd_deploy(args: argparse.Namespace) -> int:
    t0 = time.perf_counter()
    onnx_src = Path(args.onnx).resolve()
    if not onnx_src.is_file():
        print(f"ERROR: ONNX file not found: {onnx_src}", file=sys.stderr)
        return 1

    preset = PRESETS[args.preset]
    try:
        kp = parse_kp_arg(args.kp, preset.default_kp)
        kd = parse_kd_arg(args.kd, preset.default_kd)
        onnx_bytes = onnx_src.read_bytes()
        info = inspect_onnx_bytes(onnx_bytes, require_ort=True)
        bound_channels, action_tensor, aux_tensors = match_channels_to_onnx(preset, info)
    except (ValueError, RuntimeError) as exc:
        print(f"DEPLOY_CHECK_FAILED: {exc}", file=sys.stderr)
        return 2

    policy_name = (args.name.strip() if args.name else "") or onnx_src.stem
    out_dir = Path(args.out_dir).resolve() if args.out_dir else (Path("models/packages") / policy_name).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    yaml_text = build_shorthand_yaml(
        name=policy_name,
        sha256=info.sha256,
        preset=preset,
        joint_order=args.joint_order,
        bound_channels=bound_channels,
        action_tensor=action_tensor,
        aux_tensors=aux_tensors,
        kp=kp,
        kd=kd,
    )
    golden_suite = generate_golden_suite(
        session=info.session,
        preset=preset,
        joint_order=args.joint_order,
        bound_channels=bound_channels,
        action_tensor=action_tensor,
    )
    golden_text = json.dumps(golden_suite, indent=2) + "\n"

    dst_onnx = out_dir / "model.onnx"
    if onnx_src != dst_onnx:
        shutil.copy2(onnx_src, dst_onnx)
    (out_dir / "policy.yaml").write_text(yaml_text, encoding="utf-8")
    (out_dir / "golden.json").write_text(golden_text, encoding="utf-8")

    yaml_lines = len(yaml_text.strip().splitlines())
    in_summary = ", ".join(f"{name}:{ch.total_dim}D" for name, ch in bound_channels)

    if args.dry_run:
        wall_ms = (time.perf_counter() - t0) * 1000.0
        print(
            f"DRY_RUN_OK in {wall_ms:.1f}ms:\n"
            f"  package:      {out_dir}\n"
            f"  name:         {policy_name} (preset={preset.key}, joint_order={args.joint_order})\n"
            f"  sha256:       {info.sha256[:16]}...\n"
            f"  tensors:      inputs=[{in_summary}] -> output='{action_tensor}'\n"
            f"  policy.yaml:  {yaml_lines} lines\n"
            f"  golden.json:  {len(golden_suite['steps'])} synthetic steps verified (tol={golden_suite['max_abs_tolerance']:.0e})"
        )
        return 0

    resp = post_json(
        f"{args.admin_url.rstrip('/')}/v1/policies/push",
        {
            "manifest_yaml": yaml_text,
            "onnx_base64": base64.b64encode(onnx_bytes).decode("ascii"),
            "golden_json": golden_text,
            "slot": args.slot,
            "shadow_frames": args.shadow_frames,
            "activate": args.activate,
        },
    )
    wall_ms = (time.perf_counter() - t0) * 1000.0
    warmup_us = resp.get("warmup_us", "n/a")
    golden_err = resp.get("golden_max_err", "n/a")
    shadow_obj = resp.get("shadow")
    shadow_p99 = shadow_obj.get("p99_us", "n/a") if isinstance(shadow_obj, dict) else "skipped"
    err_str = f"{golden_err:.2e}" if isinstance(golden_err, (int, float)) else str(golden_err)

    print(
        f"DEPLOY_OK in {wall_ms:.1f}ms:\n"
        f"  name:           {resp.get('name', policy_name)} (slot={resp.get('bound_slot')}, active={resp.get('active_profile')})\n"
        f"  warmup_us:      {warmup_us} µs\n"
        f"  golden_max_err: {err_str} ({resp.get('golden_steps', 2)} steps)\n"
        f"  shadow_p99_us:  {shadow_p99} µs\n"
        f"  wall_clock_ms:  {wall_ms:.1f} ms (server={resp.get('elapsed_ms', 'n/a')} ms)"
    )
    return 0


def cmd_pack(args: argparse.Namespace) -> int:
    onnx_src = Path(args.onnx).resolve()
    out_dir = Path(args.out_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    onnx_bytes = onnx_src.read_bytes()
    sha = hashlib.sha256(onnx_bytes).hexdigest()
    shutil.copy2(onnx_src, out_dir / "model.onnx")
    yaml_text = TEMPLATES[args.template].format(name=args.name, sha256=sha)
    (out_dir / "policy.yaml").write_text(yaml_text, encoding="utf-8")
    print(f"Packaged '{args.name}' -> {out_dir} (sha256={sha[:16]}...)")
    return 0


def cmd_push(args: argparse.Namespace) -> int:
    t0 = time.perf_counter()
    pkg_dir = Path(args.pkg_dir).resolve()
    yaml_text = (pkg_dir / "policy.yaml").read_text(encoding="utf-8")
    model_rel = "model.onnx"
    for line in yaml_text.splitlines():
        s = line.strip()
        if s.startswith("file:"):
            model_rel = s.split(":", 1)[1].strip().strip('"').strip("'")
            break
    onnx_bytes = (pkg_dir / model_rel).read_bytes()
    golden_path = pkg_dir / "golden.json"
    golden_text = golden_path.read_text(encoding="utf-8") if golden_path.is_file() else None

    resp = post_json(
        f"{args.admin_url.rstrip('/')}/v1/policies/push",
        {
            "manifest_yaml": yaml_text,
            "onnx_base64": base64.b64encode(onnx_bytes).decode("ascii"),
            "golden_json": golden_text,
            "slot": args.slot,
            "shadow_frames": args.shadow_frames,
            "activate": args.activate,
        },
    )
    dt_ms = (time.perf_counter() - t0) * 1000.0
    print(f"HOT_PUSH_OK in {dt_ms:.1f}ms:\n" + json.dumps(resp, indent=2, ensure_ascii=False))
    return 0


def cmd_rollback(args: argparse.Namespace) -> int:
    resp = post_json(
        f"{args.admin_url.rstrip('/')}/v1/policies/rollback",
        {"slot": args.slot},
    )
    print(json.dumps(resp, indent=2, ensure_ascii=False))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="DOSO zero-restart policy inspector, packager & 1s deployer")
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_insp = sub.add_parser("inspect", help="Inspect an .onnx file or policy package directory")
    p_insp.add_argument("onnx", help="Path to .onnx checkpoint file or policy package directory")
    p_insp.set_defaults(func=cmd_inspect)

    p_dep = sub.add_parser("deploy", help="1-command inspect + shorthand YAML + auto-Golden + hot-push")
    p_dep.add_argument("onnx", help="Path to .onnx checkpoint file")
    p_dep.add_argument("--preset", choices=list(PRESETS), default="flat_53d")
    p_dep.add_argument("--name", default=None, help="Policy name (defaults to <onnx_stem>)")
    p_dep.add_argument("--joint-order", choices=list(JOINT_ORDERS), default="thunder_leg_major")
    p_dep.add_argument("--kp", default=None, help="Override leg kp 'hip,thigh,calf' (e.g. 90,120,120)")
    p_dep.add_argument("--kd", default=None, help="Override kd 'hip,thigh,calf,wheel' (e.g. 7,12,12,1)")
    p_dep.add_argument("--out-dir", default=None, help="Output package directory (defaults to models/packages/<name>)")
    p_dep.add_argument("--admin-url", default="http://127.0.0.1:5054")
    p_dep.add_argument("--slot", default=None, help="Operator slot to bind (e.g. flat, stairs, height)")
    p_dep.add_argument("--shadow-frames", type=int, default=50)
    p_dep.add_argument("--activate", action="store_true", help="Immediately switch active locomotion profile")
    p_dep.add_argument("--dry-run", action="store_true", help="Generate package + golden.json locally without HTTP push")
    p_dep.set_defaults(func=cmd_deploy)

    p_pack = sub.add_parser("pack", help="Create a self-contained policy package directory")
    p_pack.add_argument("--onnx", required=True)
    p_pack.add_argument("--name", required=True)
    p_pack.add_argument("--template", choices=sorted(TEMPLATES), default="height")
    p_pack.add_argument("--out-dir", required=True)
    p_pack.set_defaults(func=cmd_pack)

    p_push = sub.add_parser("push", help="Hot-push, Golden-verify, Shadow-eval, and bind a policy")
    p_push.add_argument("--pkg-dir", required=True)
    p_push.add_argument("--admin-url", default="http://127.0.0.1:5054")
    p_push.add_argument("--slot", default=None)
    p_push.add_argument("--shadow-frames", type=int, default=50)
    p_push.add_argument("--activate", action="store_true")
    p_push.set_defaults(func=cmd_push)

    p_rb = sub.add_parser("rollback", help="Instant rollback of an operator slot")
    p_rb.add_argument("--admin-url", default="http://127.0.0.1:5054")
    p_rb.add_argument("--slot", required=True)
    p_rb.set_defaults(func=cmd_rollback)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
