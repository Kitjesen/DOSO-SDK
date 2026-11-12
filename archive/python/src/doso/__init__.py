"""DOSO whole-robot SDK preview. Construction never starts robot motion."""

from ._common import new_request_id
from .client import DosoClient
from .errors import ApiError, DosoError, OperationFailed, ProtocolError, TransportError
from .models import CommandReceipt

__version__ = "0.1.0a1"

__all__ = [
    "ApiError",
    "CommandReceipt",
    "DosoClient",
    "DosoError",
    "OperationFailed",
    "ProtocolError",
    "TransportError",
    "new_request_id",
]
