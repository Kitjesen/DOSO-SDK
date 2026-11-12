"""Receipts describe admission, never physical execution."""

from dataclasses import dataclass
from typing import Any

from .errors import ProtocolError


@dataclass(frozen=True)
class CommandReceipt:
    ok: bool
    accepted: bool | None
    request_id: str
    task_id: str | None
    operation_id: str | None
    run_id: str | None
    raw: dict[str, Any]

    @classmethod
    def from_payload(cls, payload: dict[str, Any], request_id: str) -> "CommandReceipt":
        command = payload.get("command")
        if command is None:
            command = {}
        if not isinstance(command, dict):
            raise ProtocolError("command must be an object", request_id=request_id, submission_unknown=True)
        for item in (payload, command):
            returned_id = item.get("request_id")
            if returned_id is not None and returned_id != request_id:
                raise ProtocolError("request_id mismatch", request_id=request_id, submission_unknown=True)
        signals = [item[key] for item in (payload, command) for key in ("ok", "success", "accepted") if key in item]
        if any(signal is not None and type(signal) is not bool for signal in signals):
            raise ProtocolError("receipt flags must be boolean", request_id=request_id, submission_unknown=True)
        admission = [item["accepted"] for item in (payload, command) if item.get("accepted") is not None]
        accepted = False if any(value is False for value in admission) else (True if admission else None)
        ok = any(value is True for value in signals) and not any(value is False for value in signals)
        if payload.get("error") or command.get("error"):
            ok = False

        def identity(key: str) -> str | None:
            values = [item[key] for item in (payload, command) if item.get(key) is not None]
            if any(not isinstance(value, str) or not value for value in values) or len(set(values)) > 1:
                raise ProtocolError(f"invalid or conflicting {key}", request_id=request_id, submission_unknown=True)
            return values[0] if values else None

        return cls(ok, accepted, request_id, identity("task_id"), identity("operation_id"), identity("run_id"), payload)
