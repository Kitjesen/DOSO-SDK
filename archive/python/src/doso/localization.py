"""Map-frame observations and explicit requests to the existing localization owner."""

from __future__ import annotations

import math
from typing import TYPE_CHECKING, Any

from ._common import require_text
from .errors import ProtocolError
from .models import CommandReceipt

if TYPE_CHECKING:
    from .client import DosoClient


class LocalizationClient:
    def __init__(self, client: DosoClient):
        self._client = client

    def status(self) -> dict[str, Any]:
        return self._client._get("/api/v1/localization/status")

    def pose(self) -> dict[str, Any]:
        """Return raw map odometry; callers must also inspect localization health and age."""
        localization = self._client.system.state().get("localization")
        odometry = localization.get("odometry") if isinstance(localization, dict) else None
        if not isinstance(odometry, dict) or odometry.get("frame_id") != "map":
            raise ProtocolError("map-frame odometry is unavailable")
        if any(
            type(odometry.get(key)) not in (int, float) or not math.isfinite(odometry[key])
            for key in ("x", "y", "z", "yaw")
        ):
            raise ProtocolError("map-frame odometry requires finite x/y/z/yaw")
        return odometry

    def relocalize(
        self,
        map_name: str,
        *,
        initial_pose: dict[str, float] | None = None,
        request_id: str | None = None,
        timeout: float = 60.0,
    ) -> CommandReceipt:
        """Relocalize against the active map; None selects global search, not map loading."""
        body: dict[str, Any] = {
            "map_name": require_text(map_name, "map_name"),
            "mode": "global" if initial_pose is None else "seeded",
        }
        if initial_pose is not None:
            if (
                not isinstance(initial_pose, dict)
                or set(initial_pose) != {"x", "y", "z", "yaw"}
                or any(type(value) not in (int, float) or not math.isfinite(value) for value in initial_pose.values())
            ):
                raise ValueError("initial_pose requires finite x/y/z/yaw")
            body["initial_pose"] = initial_pose
        return self._client._command(
            "/api/v1/localization/relocalizations", body, request_id=request_id, timeout=timeout
        )
