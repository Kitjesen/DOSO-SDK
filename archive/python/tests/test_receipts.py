"""Reject ambiguous success and retain command identities."""

import re

import pytest

from doso import ApiError, CommandReceipt, DosoClient, ProtocolError, new_request_id


def test_write_server_errors_preserve_unknown_outcome():
    for status in (408, 500, 502, 503, 504):
        error = ApiError(status, {"error": "unconfirmed"}, method="POST", request_id="original")
        assert error.submission_unknown is True
        assert error.request_id == "original"
    assert ApiError(409, {}, method="POST").submission_unknown is False
    assert ApiError(503, {}, method="GET").submission_unknown is False


def test_generated_ids_are_canonical_and_distinct():
    ids = {new_request_id() for _ in range(100)}
    assert len(ids) == 100
    assert all(re.fullmatch(r"[0-7][0-9A-HJKMNP-TV-Z]{25}", value) for value in ids)


@pytest.mark.parametrize(
    "payload",
    [
        {"ok": True, "request_id": "other"},
        {"command": {"accepted": True, "request_id": "other"}},
        {"ok": "true"},
        {"ok": True, "task_id": "a", "command": {"task_id": "b"}},
        {"ok": True, "task_id": {}},
    ],
)
def test_invalid_receipt_never_becomes_success(payload):
    with pytest.raises(ProtocolError) as raised:
        CommandReceipt.from_payload(payload, "expected")
    assert raised.value.request_id == "expected"
    assert raised.value.submission_unknown is True


def test_success_without_admission_does_not_invent_acceptance():
    result = CommandReceipt.from_payload({"ok": True, "quality": 0.7}, "relocalization")
    assert result.ok
    assert result.accepted is None


@pytest.mark.parametrize("timeout", [0, -1, float("inf"), float("nan"), True])
def test_invalid_timeouts_fail_before_network(timeout):
    with pytest.raises(ValueError):
        DosoClient("http://unused.invalid", timeout=timeout)


@pytest.mark.parametrize("url", ["robot:5050", "ftp://robot", "http://user:secret@robot", "http://robot?key=secret"])
def test_invalid_or_credential_bearing_urls_are_rejected(url):
    with pytest.raises(ValueError):
        DosoClient(url)
