"""System snapshots retain server readiness and capability evidence."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from .client import DosoClient


class SystemClient:
    def __init__(self, client: DosoClient):
        self._client = client

    def ping(self) -> dict[str, Any]:
        """Check Gateway liveness, not motion readiness or control ownership."""
        return self._client._get("/health")

    def state(self) -> dict[str, Any]:
        return self._client._get("/api/v1/state")

    def health(self) -> dict[str, Any]:
        return self._client._get("/api/v1/health")

    def readiness(self) -> dict[str, Any]:
        return self._client._get("/api/v1/readiness")

    def capabilities(self) -> dict[str, Any]:
        return self._client._get("/api/v1/app/capabilities")

    def session(self) -> dict[str, Any]:
        return self._client._get("/api/v1/session")

    def product_status(self) -> dict[str, Any]:
        return self._client._get("/api/v1/product-control")
