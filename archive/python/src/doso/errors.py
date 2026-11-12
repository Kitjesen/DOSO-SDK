"""Errors preserve the distinction between rejection and an unknown outcome."""

from typing import Any


class DosoError(Exception):
    """Base SDK error."""


class ProtocolError(DosoError):
    """The response cannot safely be interpreted as the expected contract."""

    def __init__(self, message: str, *, request_id: str | None = None, submission_unknown: bool = False):
        super().__init__(message)
        self.request_id = request_id
        self.submission_unknown = submission_unknown


class ApiError(DosoError):
    """An HTTP error; payload retains the service's rejection details."""

    def __init__(
        self, status_code: int, payload: Any, *, method: str = "", path: str = "", request_id: str | None = None
    ):
        super().__init__(f"{method} {path}: HTTP {status_code}")
        self.status_code = status_code
        self.payload = payload
        self.method = method
        self.path = path
        self.request_id = request_id
        self.submission_unknown = method != "GET" and (status_code >= 500 or status_code == 408)


class TransportError(DosoError):
    """A write may have executed even when no response reached the client."""

    def __init__(self, method: str, path: str, request_id: str | None, message: str):
        super().__init__(f"{method} {path}: {message}")
        self.method = method
        self.path = path
        self.request_id = request_id
        self.submission_unknown = method != "GET"


class OperationFailed(DosoError):
    """The requested task or operation reached a non-success terminal state."""

    def __init__(self, message: str, payload: dict[str, Any]):
        super().__init__(message)
        self.payload = payload
