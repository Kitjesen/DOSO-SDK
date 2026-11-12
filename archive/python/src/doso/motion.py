"""Software stop and native navigation evidence; no direct velocity bypass."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from .models import CommandReceipt

if TYPE_CHECKING:
    from .client import DosoClient


class MotionClient:
    def __init__(self, client: DosoClient):
        self._client = client

    def emergency_stop(self, *, request_id: str | None = None) -> CommandReceipt:
        """Request a software emergency stop; ACK does not prove physical standstill."""
        return self._client._command("/api/v1/stop", {"client_id": self._client.client_id}, request_id=request_id)

    def status(self) -> dict[str, Any]:
        """Return navigation control/motion evidence with the server's original fields."""
        return self._client._get("/api/v1/navigation/status")
