"""Read existing scene and camera status without starting sensors."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from .client import DosoClient


class ObservationsClient:
    def __init__(self, client: DosoClient):
        self._client = client

    def scene(self) -> dict[str, Any]:
        return self._client._get("/api/v1/scene_graph")

    def camera_status(self) -> dict[str, Any]:
        return self._client._get("/api/v1/camera/status")
