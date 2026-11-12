"""Exercise the public SDK against a local HTTP server; never contact hardware."""

from __future__ import annotations

import json
import os
import queue
import socket
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import patch

from doso import ApiError, DosoClient, ProtocolError, TransportError


class HttpClientTest(unittest.TestCase):
    def setUp(self):
        proxy_environment = patch.dict(os.environ, {"NO_PROXY": "127.0.0.1", "no_proxy": "127.0.0.1"})
        proxy_environment.start()
        self.addCleanup(proxy_environment.stop)
        self.requests = []
        self.responses = queue.Queue()
        test = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                self.respond()

            def do_POST(self):
                self.respond()

            def respond(self):
                raw_body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                test.requests.append(
                    {
                        "method": self.command,
                        "path": self.path,
                        "headers": dict(self.headers),
                        "body": json.loads(raw_body) if raw_body else None,
                    }
                )
                response = test.responses.get_nowait() if not test.responses.empty() else {}
                if response.get("disconnect"):
                    self.connection.shutdown(socket.SHUT_RDWR)
                    self.connection.close()
                    return
                payload = response.get("raw")
                if payload is None:
                    payload = json.dumps(response.get("payload", {})).encode("utf-8")
                self.send_response(response.get("status", 200))
                for name, value in response.get("headers", {}).items():
                    self.send_header(name, value)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(
            target=self.server.serve_forever,
            kwargs={"poll_interval": 0.01},
            daemon=True,
        )
        self.thread.start()
        self.addCleanup(self.close_server)
        self.base_url = f"http://127.0.0.1:{self.server.server_port}"
        self.client = DosoClient(self.base_url, client_id="sdk-test", timeout=1)

    def close_server(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)

    def reply(self, payload, *, status=200, headers=None):
        self.responses.put({"payload": payload, "status": status, "headers": headers or {}})

    def test_construction_exposes_seven_domains_without_network(self):
        for name in ("system", "motion", "localization", "maps", "navigation", "observations", "tasks"):
            with self.subTest(domain=name):
                self.assertIsNotNone(getattr(self.client, name))
        self.assertEqual(self.requests, [])

    def test_read_domains_use_existing_gateway_routes(self):
        calls = [
            (self.client.system.ping, "/health"),
            (self.client.system.state, "/api/v1/state"),
            (self.client.system.health, "/api/v1/health"),
            (self.client.system.readiness, "/api/v1/readiness"),
            (self.client.system.capabilities, "/api/v1/app/capabilities"),
            (self.client.system.session, "/api/v1/session"),
            (self.client.system.product_status, "/api/v1/product-control"),
            (self.client.motion.status, "/api/v1/navigation/status"),
            (self.client.localization.status, "/api/v1/localization/status"),
            (self.client.observations.scene, "/api/v1/scene_graph"),
            (self.client.observations.camera_status, "/api/v1/camera/status"),
        ]
        for call, path in calls:
            with self.subTest(path=path):
                payload = {"source": path, "ok": True}
                self.reply(payload)
                self.assertEqual(call(), payload)
                self.assertEqual(self.requests[-1]["path"], path)
                self.assertEqual(self.requests[-1]["method"], "GET")

    def test_api_key_is_sent_as_header(self):
        self.reply({"ok": True})
        client = DosoClient(self.base_url, api_key="test-only-key")
        client.system.ping()
        headers = {name.lower(): value for name, value in self.requests[-1]["headers"].items()}
        self.assertEqual(headers["x-api-key"], "test-only-key")
        self.assertNotIn("test-only-key", self.requests[-1]["path"])

    def test_emergency_stop_preserves_explicit_receipt_and_identity(self):
        self.reply(
            {
                "ok": True,
                "accepted": True,
                "command": {"request_id": "stop-request", "accepted": True},
                "stage": "native_acknowledged",
            }
        )
        receipt = self.client.motion.emergency_stop(request_id="stop-request")
        request = self.requests[-1]
        self.assertEqual(request["method"], "POST")
        self.assertEqual(request["path"], "/api/v1/stop")
        self.assertEqual(request["body"]["client_id"], "sdk-test")
        self.assertEqual(request["body"]["request_id"], "stop-request")
        self.assertTrue(receipt.ok)
        self.assertIs(receipt.accepted, True)
        self.assertEqual(receipt.request_id, "stop-request")
        self.assertEqual(receipt.raw["stage"], "native_acknowledged")

    def test_commands_generate_a_request_identity(self):
        self.reply({"ok": True, "accepted": True})
        receipt = self.client.motion.emergency_stop()
        request_id = self.requests[-1]["body"]["request_id"]
        self.assertIsInstance(request_id, str)
        self.assertTrue(request_id)
        self.assertEqual(receipt.request_id, request_id)

    def test_http_success_does_not_imply_command_acceptance(self):
        self.reply({"status": "stopped"})
        receipt = self.client.motion.emergency_stop()
        self.assertFalse(receipt.ok)
        self.assertIsNone(receipt.accepted)

    def test_explicit_rejection_overrides_positive_ok(self):
        self.reply({"ok": True, "accepted": False, "reason": "control_busy"})
        receipt = self.client.motion.emergency_stop()
        self.assertFalse(receipt.ok)
        self.assertIs(receipt.accepted, False)

    def test_nested_gateway_admission_is_read(self):
        self.reply({"ok": True, "command": {"accepted": True, "request_id": "nested"}})
        receipt = self.client.motion.emergency_stop(request_id="nested")
        self.assertTrue(receipt.ok)
        self.assertIs(receipt.accepted, True)
        self.assertEqual(receipt.request_id, "nested")

    def test_instruction_and_agent_run_identity(self):
        self.reply({"ok": True, "accepted": True, "run_id": "run-17"})
        receipt = self.client.tasks.send_instruction(
            "inspect the doorway",
            run_id="run-17",
            control_only=True,
            request_id="instruction-17",
        )
        self.assertEqual(receipt.run_id, "run-17")
        self.assertEqual(self.requests[-1]["path"], "/api/v1/instruction")
        self.assertEqual(
            self.requests[-1]["body"],
            {
                "text": "inspect the doorway",
                "run_id": "run-17",
                "control_only": True,
                "request_id": "instruction-17",
                "client_id": "sdk-test",
            },
        )
        self.reply({"found": True, "run_id": "run/a b"})
        self.assertTrue(self.client.tasks.get_run("run/a b")["found"])
        self.assertEqual(self.requests[-1]["path"], "/api/v1/agent/runs/run%2Fa%20b")
        self.reply({"ok": True, "accepted": True, "run_id": "run/a b"})
        self.client.tasks.cancel_run("run/a b")
        self.assertEqual(self.requests[-1]["path"], "/api/v1/agent/runs/run%2Fa%20b/cancel")
        self.assertEqual(self.requests[-1]["body"]["client_id"], "sdk-test")

    def test_map_pose_preserves_original_data(self):
        pose = {"frame_id": "map", "x": 1.2, "y": -0.5, "z": 0.3, "yaw": 0.7, "timestamp": 42}
        self.reply({"localization": {"odometry": pose}})
        self.assertEqual(self.client.localization.pose(), pose)

    def test_missing_or_wrong_pose_frame_is_not_defaulted(self):
        for state in (
            {},
            {"localization": {}},
            {"localization": {"odometry": {"x": 1}}},
            {"localization": {"odometry": {"frame_id": "odom", "x": 1}}},
        ):
            with self.subTest(state=state):
                self.reply(state)
                with self.assertRaises(ProtocolError):
                    self.client.localization.pose()

    def test_http_rejection_retains_structured_error_and_request_identity(self):
        payload = {"ok": False, "error": "native_command_rejected", "detail": {"reason": "safety_stop"}}
        self.reply(payload, status=409)
        with self.assertRaises(ApiError) as raised:
            self.client.motion.emergency_stop(request_id="rejected-stop")
        error = raised.exception
        self.assertEqual(error.status_code, 409)
        self.assertEqual(error.payload, payload)
        self.assertEqual(error.method, "POST")
        self.assertEqual(error.path, "/api/v1/stop")
        self.assertEqual(error.request_id, "rejected-stop")

    def test_write_disconnect_is_not_retried_and_reports_unknown_submission(self):
        self.responses.put({"disconnect": True})
        with self.assertRaises(TransportError) as raised:
            self.client.motion.emergency_stop(request_id="lost-response")
        error = raised.exception
        self.assertEqual(len(self.requests), 1)
        self.assertEqual(error.method, "POST")
        self.assertEqual(error.path, "/api/v1/stop")
        self.assertEqual(error.request_id, "lost-response")
        self.assertIs(error.submission_unknown, True)

    def test_write_service_unconfirmed_retains_unknown_submission(self):
        self.reply({"ok": False, "error": "native_command_unconfirmed"}, status=503)
        with self.assertRaises(ApiError) as raised:
            self.client.motion.emergency_stop(request_id="unconfirmed-stop")
        self.assertTrue(raised.exception.submission_unknown)
        self.assertEqual(raised.exception.request_id, "unconfirmed-stop")
        self.assertEqual(len(self.requests), 1)

    def test_redirect_is_not_followed(self):
        self.reply({}, status=302, headers={"Location": self.base_url + "/redirected"})
        with self.assertRaises(ApiError) as raised:
            self.client.motion.emergency_stop(request_id="redirect-stop")
        self.assertEqual(raised.exception.status_code, 302)
        self.assertEqual(len(self.requests), 1)
        self.assertEqual(self.requests[0]["path"], "/api/v1/stop")

    def test_invalid_json_and_non_object_responses_are_protocol_errors(self):
        for raw in (b"not-json", b"[]", b"null", b"true"):
            with self.subTest(raw=raw):
                self.responses.put({"raw": raw})
                with self.assertRaises(ProtocolError):
                    self.client.system.state()

    def test_non_finite_navigation_coordinates_are_rejected_before_http(self):
        for coordinate in (float("nan"), float("inf"), float("-inf")):
            with self.subTest(coordinate=coordinate):
                with self.assertRaises((ValueError, TypeError)):
                    self.client.navigation.go_to(coordinate, 0.0)
        self.assertEqual(self.requests, [])


if __name__ == "__main__":
    unittest.main()
