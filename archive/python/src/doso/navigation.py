"""Map-frame navigation requests and task-correlated lifecycle queries."""

from __future__ import annotations

import math
import time
from typing import TYPE_CHECKING, Any
from urllib.parse import quote

from ._common import positive, require_text
from .errors import OperationFailed, ProtocolError

if TYPE_CHECKING:
    from .client import DosoClient
    from .models import CommandReceipt


class NavigationClient:
    def __init__(self, client: DosoClient) -> None:
        self._client = client

    def go_to(
        self,
        x: float,
        y: float,
        *,
        z: float = 0.0,
        yaw: float = 0.0,
        request_id: str | None = None,
    ) -> CommandReceipt:
        """Submit a map-frame target in metres and yaw in radians."""
        body: dict[str, Any] = {
            "x": x,
            "y": y,
            "z": z,
            "yaw": yaw,
            "frame_id": "map",
            "client_id": self._client.client_id,
        }
        for name in ("x", "y", "z", "yaw"):
            value = body[name]
            if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
                raise ValueError(f"{name} must be a finite number")
        receipt = self._client._command("/api/v1/goal", body, request_id=request_id)
        if receipt.ok and not receipt.task_id:
            raise ProtocolError(
                "Accepted navigation goal requires a task_id", request_id=receipt.request_id, submission_unknown=True
            )
        return receipt

    def status(self) -> dict[str, Any]:
        return self._client._get("/api/v1/navigation/status")

    def path(self) -> dict[str, Any]:
        return self._client._get("/api/v1/path")

    def task(self, task_id: str) -> dict[str, Any]:
        task_id = require_text(task_id, "task_id")
        return self._client._get(f"/api/v1/navigation/tasks/{quote(task_id, safe='')}")

    def cancel(
        self,
        task_id: str,
        reason: str = "client_cancel",
        request_id: str | None = None,
    ) -> CommandReceipt:
        """Request cancellation of one task; acceptance is not its terminal state."""
        task_id = require_text(task_id, "task_id")
        return self._client._command(
            f"/api/v1/navigation/tasks/{quote(task_id, safe='')}/cancel",
            {"task_id": task_id, "reason": require_text(reason, "reason"), "client_id": self._client.client_id},
            request_id=request_id,
        )

    def wait(
        self,
        task_id: str,
        timeout: float = 120.0,
        poll_interval: float = 0.5,
    ) -> dict[str, Any]:
        """Wait for this task's SUCCESS; failure and cancellation raise OperationFailed."""
        task_id = require_text(task_id, "task_id")
        timeout = positive(timeout, "timeout")
        poll_interval = positive(poll_interval, "poll_interval")
        deadline = time.monotonic() + timeout
        path = f"/api/v1/navigation/tasks/{quote(task_id, safe='')}"
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"Navigation task {task_id} did not finish within {timeout}s")
            payload = self._client._get(path, timeout=min(self._client.timeout, remaining))
            if payload.get("task_id") != task_id:
                raise ProtocolError("Navigation response task_id does not match the requested task")
            if type(payload.get("found")) is not bool:
                raise ProtocolError("Navigation response requires a boolean found field")
            if payload["found"]:
                status = payload.get("status")
                if not isinstance(status, dict) or status.get("task_id") != task_id:
                    raise ProtocolError("Navigation status does not identify the requested task")
                state = status.get("lifecycle_state_name") or status.get("state_name")
                if state == "SUCCESS":
                    if any(
                        item.get("ok") is False or item.get("success") is False or item.get("error")
                        for item in (payload, status)
                    ):
                        raise ProtocolError("Navigation SUCCESS contradicts explicit failure evidence")
                    return payload
                if state in {"FAILED", "CANCELLED"}:
                    raise OperationFailed(f"Navigation task {task_id} ended with {state}", payload)
                if state not in {"PLANNING", "EXECUTING", "PAUSED", "RECOVERING"}:
                    raise ProtocolError(f"Unsupported navigation lifecycle state: {state!r}")
            remaining = deadline - time.monotonic()
            if remaining > 0:
                time.sleep(min(poll_interval, remaining))
