"""One explicit HTTP connection configuration for seven domain clients."""

from __future__ import annotations

import http.client
import json
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

from ._common import new_request_id, positive, require_text
from .errors import ApiError, ProtocolError, TransportError
from .localization import LocalizationClient
from .maps import MapsClient
from .models import CommandReceipt
from .motion import MotionClient
from .navigation import NavigationClient
from .observations import ObservationsClient
from .system import SystemClient
from .tasks import TasksClient


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # A redirect must not replay a command or forward credentials elsewhere.
        return None


class DosoClient:
    """Construct without I/O; every operation is explicit and writes are never retried."""

    def __init__(
        self, base_url: str, *, client_id: str = "doso-sdk", api_key: str | None = None, timeout: float = 10.0
    ) -> None:
        require_text(base_url, "base_url")
        parts = urllib.parse.urlsplit(base_url)
        if (
            parts.scheme not in {"http", "https"}
            or not parts.hostname
            or parts.username is not None
            or parts.password is not None
            or parts.query
            or parts.fragment
            or any(ch.isspace() for ch in base_url)
        ):
            raise ValueError("base_url must be an HTTP(S) URL without credentials, query or fragment")
        _ = parts.port
        self.base_url = base_url.rstrip("/")
        self.client_id = require_text(client_id, "client_id")
        self.timeout = positive(timeout, "timeout")
        if api_key is not None:
            require_text(api_key, "api_key")
            if "\r" in api_key or "\n" in api_key:
                raise ValueError("api_key cannot contain line breaks")
        self._api_key = api_key
        self._opener = urllib.request.build_opener(_NoRedirect())
        self.system = SystemClient(self)
        self.motion = MotionClient(self)
        self.localization = LocalizationClient(self)
        self.maps = MapsClient(self)
        self.navigation = NavigationClient(self)
        self.observations = ObservationsClient(self)
        self.tasks = TasksClient(self)

    def _get(self, path: str, *, timeout: float | None = None) -> dict[str, Any]:
        return self._request("GET", path, None, timeout=timeout)

    def _post(self, path: str, body: dict[str, Any], *, timeout: float | None = None) -> dict[str, Any]:
        return self._request("POST", path, body, timeout=timeout)

    def _command(
        self,
        path: str,
        body: dict[str, Any] | None = None,
        *,
        request_id: str | None = None,
        timeout: float | None = None,
    ) -> CommandReceipt:
        identity = new_request_id() if request_id is None else require_text(request_id, "request_id")
        payload = dict(body or {})
        payload["request_id"] = identity
        return CommandReceipt.from_payload(self._post(path, payload, timeout=timeout), identity)

    def _request(self, method: str, path: str, body: dict[str, Any] | None, *, timeout: float | None) -> dict[str, Any]:
        request_id = body.get("request_id") if body is not None else None
        headers = {"Accept": "application/json", "User-Agent": "doso-sdk/0.1.0a1"}
        if self._api_key is not None:
            headers["X-API-Key"] = self._api_key
        encoded = None
        if body is not None:
            encoded = json.dumps(body, allow_nan=False, ensure_ascii=False).encode("utf-8")
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(self.base_url + path, data=encoded, headers=headers, method=method)
        try:
            with self._opener.open(
                request, timeout=self.timeout if timeout is None else positive(timeout, "timeout")
            ) as r:
                data = r.read()
        except urllib.error.HTTPError as exc:
            try:
                data = exc.read()
                try:
                    payload = json.loads(data)
                except (ValueError, UnicodeError):
                    payload = data.decode("utf-8", errors="replace")
            except (OSError, http.client.HTTPException):
                payload = None
            finally:
                exc.close()
            raise ApiError(exc.code, payload, method=method, path=path, request_id=request_id) from exc
        except (urllib.error.URLError, OSError, http.client.HTTPException) as exc:
            raise TransportError(method, path, request_id, str(exc)) from exc
        try:
            result = json.loads(data)
        except (ValueError, UnicodeError) as exc:
            raise ProtocolError(
                f"{method} {path}: invalid JSON response", request_id=request_id, submission_unknown=method != "GET"
            ) from exc
        if not isinstance(result, dict):
            raise ProtocolError(
                f"{method} {path}: expected a JSON object", request_id=request_id, submission_unknown=method != "GET"
            )
        return result
