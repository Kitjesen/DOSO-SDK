"""Exercise task/operation identity, terminal evidence, and write-once behavior."""

from __future__ import annotations

import unittest
from unittest.mock import patch

from doso.errors import OperationFailed, ProtocolError
from doso.maps import MapsClient
from doso.models import CommandReceipt
from doso.navigation import NavigationClient


class StubClient:
    timeout = 10.0
    client_id = "test-client"

    def __init__(self, *responses):
        self.responses = list(responses)
        self.calls = []

    def _get(self, path, *, timeout=None):
        self.calls.append(("GET", path, timeout))
        result = self.responses.pop(0)
        if isinstance(result, Exception):
            raise result
        return result

    def _post(self, path, body, *, timeout=None):
        self.calls.append(("POST", path, body))
        result = self.responses.pop(0)
        if isinstance(result, Exception):
            raise result
        return result

    def _command(self, path, body=None, *, request_id=None, timeout=None):
        self.calls.append(("COMMAND", path, body, request_id))
        return self.responses.pop(0)


def navigation_task(state, task_id="task-1"):
    return {
        "found": True,
        "task_id": task_id,
        "status": {"task_id": task_id, "lifecycle_state_name": state},
    }


class NavigationTests(unittest.TestCase):
    def test_goal_uses_physical_map_coordinates(self):
        receipt = CommandReceipt.from_payload({"ok": True, "accepted": True, "task_id": "task-1"}, "req-1")
        client = StubClient(receipt)
        self.assertIs(NavigationClient(client).go_to(1, 2, z=3, yaw=0.5, request_id="req-1"), receipt)
        self.assertEqual(
            client.calls,
            [
                (
                    "COMMAND",
                    "/api/v1/goal",
                    {"x": 1, "y": 2, "z": 3, "yaw": 0.5, "frame_id": "map", "client_id": "test-client"},
                    "req-1",
                )
            ],
        )

    def test_nonfinite_goal_never_sends(self):
        for value in (float("nan"), float("inf"), True):
            client = StubClient()
            with self.assertRaises(ValueError):
                NavigationClient(client).go_to(value, 0)
            self.assertEqual(client.calls, [])

    def test_cancel_targets_exact_encoded_task(self):
        client = StubClient(object())
        NavigationClient(client).cancel("task/one", request_id="cancel-1")
        self.assertEqual(
            client.calls[0],
            (
                "COMMAND",
                "/api/v1/navigation/tasks/task%2Fone/cancel",
                {"task_id": "task/one", "reason": "client_cancel", "client_id": "test-client"},
                "cancel-1",
            ),
        )

    def test_wait_ignores_paused_and_returns_only_own_success(self):
        success = navigation_task("SUCCESS")
        client = StubClient(navigation_task("PAUSED"), success)
        with patch("doso.navigation.time.sleep"):
            self.assertIs(NavigationClient(client).wait("task-1"), success)
        self.assertEqual(len(client.calls), 2)
        self.assertTrue(all(call[0] == "GET" for call in client.calls))

    def test_wait_rejects_wrong_outer_or_inner_task(self):
        wrong_outer = navigation_task("SUCCESS", "another-task")
        wrong_inner = navigation_task("SUCCESS")
        wrong_inner["status"]["task_id"] = "another-task"
        for response in (wrong_outer, wrong_inner):
            with self.assertRaises(ProtocolError):
                NavigationClient(StubClient(response)).wait("task-1")

    def test_failure_and_cancel_are_not_success(self):
        for state in ("FAILED", "CANCELLED"):
            with self.assertRaises(OperationFailed):
                NavigationClient(StubClient(navigation_task(state))).wait("task-1")

    def test_success_rejects_explicit_failure_evidence(self):
        for container in ("payload", "status"):
            for evidence in ({"ok": False}, {"success": False}, {"error": "localization_lost"}):
                with self.subTest(container=container, evidence=evidence):
                    payload = navigation_task("SUCCESS")
                    target = payload if container == "payload" else payload["status"]
                    target.update(evidence)
                    with self.assertRaises(ProtocolError):
                        NavigationClient(StubClient(payload)).wait("task-1")

    def test_unknown_task_times_out_and_bounds_request_timeout(self):
        client = StubClient({"found": False, "task_id": "task-1", "status": None})
        with patch("doso.navigation.time.monotonic", side_effect=[0, 0.2, 1.1, 1.1]):
            with self.assertRaises(TimeoutError):
                NavigationClient(client).wait("task-1", timeout=1)
        self.assertAlmostEqual(client.calls[0][2], 0.8)


class MapsTests(unittest.TestCase):
    def test_save_is_one_write_and_preserves_receipt(self):
        receipt = CommandReceipt.from_payload({"ok": True, "accepted": True, "operation_id": "save-1"}, "req-1")
        client = StubClient(receipt)
        self.assertIs(MapsClient(client).save("garage", request_id="save-1"), receipt)
        self.assertEqual(client.calls, [("COMMAND", "/api/v1/map/save", {"name": "garage"}, "save-1")])

    def test_accepted_writes_require_result_identity(self):
        receipt = CommandReceipt.from_payload({"ok": True, "accepted": True}, "req-1")
        for operation in (
            lambda: MapsClient(StubClient(receipt)).save("garage"),
            lambda: NavigationClient(StubClient(receipt)).go_to(1, 2),
        ):
            with self.assertRaises(ProtocolError) as raised:
                operation()
            self.assertEqual(raised.exception.request_id, "req-1")
            self.assertTrue(raised.exception.submission_unknown)

    def test_wait_save_returns_durable_success(self):
        success = {"ok": True, "operation": {"operation_id": "save-1", "state": "SUCCEEDED"}}
        client = StubClient({"ok": True, "operation": {"operation_id": "save-1", "state": "RUNNING"}}, success)
        with patch("doso.maps.time.sleep"):
            self.assertIs(MapsClient(client).wait("save-1"), success)
        self.assertTrue(all(call[0] == "GET" for call in client.calls))

    def test_wait_save_checks_identities_and_terminal_failures(self):
        for payload in (
            {"operation": {"operation_id": "other", "state": "SUCCEEDED"}},
            {"operation_id": "other", "operation": {"operation_id": "save-1", "state": "SUCCEEDED"}},
        ):
            with self.assertRaises(ProtocolError):
                MapsClient(StubClient(payload)).wait("save-1")

    def test_save_success_rejects_explicit_failure_evidence(self):
        for container in ("payload", "operation"):
            for evidence in ({"ok": False}, {"success": False}, {"error": "persist_failed"}):
                with self.subTest(container=container, evidence=evidence):
                    payload = {"operation": {"operation_id": "save-1", "state": "SUCCEEDED"}}
                    target = payload if container == "payload" else payload["operation"]
                    target.update(evidence)
                    with self.assertRaises(ProtocolError):
                        MapsClient(StubClient(payload)).wait("save-1")
        for state in ("FAILED", "CANCELLED", "CANCELED"):
            payload = {"operation": {"operation_id": "save-1", "state": state}}
            with self.assertRaises(OperationFailed):
                MapsClient(StubClient(payload)).wait("save-1")

    def test_load_uses_productcontrol_and_explicit_session_guard(self):
        response = {"request_id": "load-1", "state": "running", "result": None}
        client = StubClient(response)
        pose = {"x": 1, "y": 2, "z": 3, "yaw": 0.5}
        self.assertIs(
            MapsClient(client).load(
                "garage", expected_product_session_id="session-1", initial_pose=pose, request_id="load-1"
            ),
            response,
        )
        self.assertEqual(
            client.calls,
            [
                (
                    "POST",
                    "/api/v1/product-control/switch",
                    {
                        "product": "nav",
                        "map_name": "garage",
                        "initial_pose": pose,
                        "request_id": "load-1",
                        "expected_product_session_id": "session-1",
                    },
                )
            ],
        )

    def test_invalid_pose_rejected_before_write(self):
        for pose in ({"x": 0, "y": 0, "yaw": 0}, {"x": 0, "y": 0, "z": float("nan"), "yaw": 0}):
            client = StubClient()
            with self.assertRaises(ValueError):
                MapsClient(client).load("garage", expected_product_session_id="", initial_pose=pose)
            self.assertEqual(client.calls, [])

    def test_mapping_does_not_bind_saved_map(self):
        client = StubClient({"request_id": "map-1", "state": "running"})
        MapsClient(client).start_mapping(expected_product_session_id="", request_id="map-1")
        self.assertEqual(
            client.calls[0][2],
            {
                "product": "map",
                "request_id": "map-1",
                "expected_product_session_id": "",
            },
        )

    def test_switch_transport_error_is_not_retried(self):
        client = StubClient(ConnectionError("Gateway restarting"))
        with self.assertRaises(ConnectionError):
            MapsClient(client).load("garage", expected_product_session_id="session-1", request_id="load-1")
        self.assertEqual(len(client.calls), 1)

    def test_product_operation_identity_and_failure(self):
        with self.assertRaises(ProtocolError):
            MapsClient(StubClient({"request_id": "other", "state": "succeeded"})).wait_product_operation("load-1")
        for state in ("failed", "interrupted"):
            with self.assertRaises(OperationFailed):
                MapsClient(StubClient({"request_id": "load-1", "state": state})).wait_product_operation("load-1")
        success = {"request_id": "load-1", "state": "succeeded", "result": {"ok": True, "status": "active"}}
        self.assertIs(MapsClient(StubClient(success)).wait_product_operation("load-1"), success)

    def test_product_success_requires_confirmed_active_result(self):
        for result in (None, {}, {"ok": False, "status": "active"}, {"ok": True, "status": "starting"}):
            with self.subTest(result=result):
                payload = {"request_id": "load-1", "state": "succeeded"}
                if result is not None:
                    payload["result"] = result
                with self.assertRaises(ProtocolError):
                    MapsClient(StubClient(payload)).wait_product_operation("load-1")
        payload = {"request_id": "load-1", "state": "succeeded", "result": {"ok": True, "status": "already_active"}}
        self.assertIs(MapsClient(StubClient(payload)).wait_product_operation("load-1"), payload)

    def test_product_success_rejects_explicit_failure_evidence(self):
        for container in ("payload", "result"):
            for evidence in ({"ok": False}, {"success": False}, {"error": "switch_failed"}):
                with self.subTest(container=container, evidence=evidence):
                    payload = {"request_id": "load-1", "state": "succeeded", "result": {"ok": True, "status": "active"}}
                    target = payload if container == "payload" else payload["result"]
                    target.update(evidence)
                    with self.assertRaises(ProtocolError):
                        MapsClient(StubClient(payload)).wait_product_operation("load-1")

    def test_save_wait_times_out_without_resubmitting(self):
        client = StubClient({"operation": {"operation_id": "save-1", "state": "RUNNING"}})
        with patch("doso.maps.time.monotonic", side_effect=[0, 0.2, 1.1, 1.1]):
            with self.assertRaises(TimeoutError):
                MapsClient(client).wait("save-1", timeout=1)
        self.assertEqual(client.calls[0][0], "GET")
        self.assertAlmostEqual(client.calls[0][2], 0.8)


if __name__ == "__main__":
    unittest.main()
