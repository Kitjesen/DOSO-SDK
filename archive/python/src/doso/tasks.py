"""Agent run identities remain separate from navigation tasks."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any
from urllib.parse import quote

from ._common import require_text
from .models import CommandReceipt

if TYPE_CHECKING:
    from .client import DosoClient


class TasksClient:
    def __init__(self, client: DosoClient):
        self._client = client

    def send_instruction(
        self, text: str, *, run_id: str | None = None, control_only: bool = False, request_id: str | None = None
    ) -> CommandReceipt:
        """May start robot motion through the task owner; not a read-only chat API."""
        if type(control_only) is not bool:
            raise ValueError("control_only must be boolean")
        body: dict[str, Any] = {
            "text": require_text(text, "text"),
            "control_only": control_only,
            "client_id": self._client.client_id,
        }
        if run_id is not None:
            body["run_id"] = require_text(run_id, "run_id")
        return self._client._command("/api/v1/instruction", body, request_id=request_id)

    def get_run(self, run_id: str) -> dict[str, Any]:
        return self._client._get(f"/api/v1/agent/runs/{quote(require_text(run_id, 'run_id'), safe='')}")

    def cancel_run(self, run_id: str, *, request_id: str | None = None) -> CommandReceipt:
        return self._client._command(
            f"/api/v1/agent/runs/{quote(require_text(run_id, 'run_id'), safe='')}/cancel",
            {"client_id": self._client.client_id},
            request_id=request_id,
        )
