"""Saved maps, durable save operations, and ProductControl map activation."""

from __future__ import annotations

import math
import time
from typing import TYPE_CHECKING, Any
from urllib.parse import quote

from ._common import new_request_id, positive, require_text
from .errors import OperationFailed, ProtocolError

if TYPE_CHECKING:
    from .client import DosoClient
    from .models import CommandReceipt


class MapsClient:
    def __init__(self, client: DosoClient) -> None:
        self._client = client

    def list(self) -> dict[str, Any]:
        return self._client._get("/api/v1/slam/maps")

    def save(self, name: str | None = None, request_id: str | None = None) -> CommandReceipt:
        """Submit one save request; inspect its operation for durable completion."""
        body = {} if name is None else {"name": require_text(name, "name")}
        receipt = self._client._command("/api/v1/map/save", body, request_id=request_id)
        if receipt.ok and not receipt.operation_id:
            raise ProtocolError(
                "Accepted map save requires an operation_id", request_id=receipt.request_id, submission_unknown=True
            )
        return receipt

    def operation(self, operation_id: str) -> dict[str, Any]:
        operation_id = require_text(operation_id, "operation_id")
        return self._client._get(f"/api/v1/maps/operations/{quote(operation_id, safe='')}")

    def wait(
        self,
        operation_id: str,
        timeout: float = 300.0,
        poll_interval: float = 1.0,
    ) -> dict[str, Any]:
        """Wait for an existing save operation without resubmitting the write."""
        operation_id = require_text(operation_id, "operation_id")
        timeout = positive(timeout, "timeout")
        poll_interval = positive(poll_interval, "poll_interval")
        deadline = time.monotonic() + timeout
        path = f"/api/v1/maps/operations/{quote(operation_id, safe='')}"
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"Map operation {operation_id} did not finish within {timeout}s")
            payload = self._client._get(path, timeout=min(self._client.timeout, remaining))
            operation = payload.get("operation")
            if not isinstance(operation, dict) or operation.get("operation_id") != operation_id:
                raise ProtocolError("Map response operation does not identify the requested operation")
            if "operation_id" in payload and payload["operation_id"] != operation_id:
                raise ProtocolError("Map response operation_id does not match the requested operation")
            state = operation.get("state")
            if state == "SUCCEEDED":
                if any(
                    item.get("ok") is False or item.get("success") is False or item.get("error")
                    for item in (payload, operation)
                ):
                    raise ProtocolError("Map SUCCEEDED contradicts explicit failure evidence")
                return payload
            if state in {"FAILED", "CANCELLED", "CANCELED"}:
                raise OperationFailed(f"Map operation {operation_id} ended with {state}", payload)
            if state not in {"WAITING_SNAPSHOT", "QUEUED", "RUNNING"}:
                raise ProtocolError(f"Unsupported map operation state: {state!r}")
            remaining = deadline - time.monotonic()
            if remaining > 0:
                time.sleep(min(poll_interval, remaining))

    def load(
        self,
        map_name: str,
        *,
        expected_product_session_id: str,
        initial_pose: dict[str, float] | None = None,
        request_id: str | None = None,
    ) -> dict[str, Any]:
        """Activate a saved map via ProductControl, retaining its switch receipt."""
        body: dict[str, Any] = {"product": "nav", "map_name": require_text(map_name, "map_name")}
        if initial_pose is not None:
            if not isinstance(initial_pose, dict) or set(initial_pose) != {"x", "y", "z", "yaw"}:
                raise ValueError("initial_pose requires exactly x, y, z and yaw")
            for value in initial_pose.values():
                if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
                    raise ValueError("initial_pose values must be finite numbers")
            body["initial_pose"] = dict(initial_pose)
        return self._switch(body, expected_product_session_id, request_id)

    def start_mapping(
        self,
        *,
        expected_product_session_id: str,
        request_id: str | None = None,
    ) -> dict[str, Any]:
        """Start a new mapping Product; this does not resume a saved mapping graph."""
        return self._switch({"product": "map"}, expected_product_session_id, request_id)

    def _switch(self, body: dict[str, Any], expected_product_session_id: str, request_id: str | None) -> dict[str, Any]:
        if not isinstance(expected_product_session_id, str):
            raise ValueError("expected_product_session_id must be a string")
        resolved_request_id = new_request_id() if request_id is None else require_text(request_id, "request_id")
        body.update(request_id=resolved_request_id, expected_product_session_id=expected_product_session_id)
        payload = self._client._post("/api/v1/product-control/switch", body)
        if payload.get("request_id") != resolved_request_id:
            raise ProtocolError(
                "ProductControl response request_id does not match the switch request",
                request_id=resolved_request_id,
                submission_unknown=True,
            )
        return payload

    def product_operation(self, request_id: str) -> dict[str, Any]:
        request_id = require_text(request_id, "request_id")
        return self._client._get(f"/api/v1/product-control/operations/{quote(request_id, safe='')}")

    def wait_product_operation(
        self,
        request_id: str,
        timeout: float = 120.0,
        poll_interval: float = 0.5,
    ) -> dict[str, Any]:
        """Poll a switch receipt; transport errors propagate with no write retry."""
        request_id = require_text(request_id, "request_id")
        timeout = positive(timeout, "timeout")
        poll_interval = positive(poll_interval, "poll_interval")
        deadline = time.monotonic() + timeout
        path = f"/api/v1/product-control/operations/{quote(request_id, safe='')}"
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"Product operation {request_id} did not finish within {timeout}s")
            payload = self._client._get(path, timeout=min(self._client.timeout, remaining))
            if payload.get("request_id") != request_id:
                raise ProtocolError("ProductControl operation request_id does not match the requested operation")
            request = payload.get("request")
            if isinstance(request, dict) and request.get("request_id") != request_id:
                raise ProtocolError("ProductControl operation contains a conflicting request_id")
            state = payload.get("state")
            if state == "succeeded":
                result = payload.get("result")
                if (
                    not isinstance(result, dict)
                    or result.get("ok") is not True
                    or result.get("status") not in {"active", "already_active"}
                ):
                    raise ProtocolError("ProductControl succeeded requires a confirmed active result")
                if any(
                    item.get("ok") is False or item.get("success") is False or item.get("error")
                    for item in (payload, result)
                ):
                    raise ProtocolError("ProductControl succeeded contradicts explicit failure evidence")
                return payload
            if state in {"failed", "interrupted"}:
                raise OperationFailed(f"Product operation {request_id} ended with {state}", payload)
            if state != "running":
                raise ProtocolError(f"Unsupported ProductControl operation state: {state!r}")
            remaining = deadline - time.monotonic()
            if remaining > 0:
                time.sleep(min(poll_interval, remaining))
