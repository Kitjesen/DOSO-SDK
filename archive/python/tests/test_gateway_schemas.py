"""Optional checks against a local LingTu checkout's actual request models."""

import importlib
import io
import json
import os
from pathlib import Path

import pytest

from doso import DosoClient


def test_requests_match_gateway_models(monkeypatch):
    source = os.environ.get("LINGTU_SOURCE")
    if not source:
        pytest.skip("Set LINGTU_SOURCE to validate against a local LingTu checkout")
    monkeypatch.syspath_prepend(str(Path(source) / "src"))
    schemas = importlib.import_module("gateway.schemas")
    robot = DosoClient("http://unused.invalid", client_id="contract-test")
    validated = []

    class SchemaOpener:
        def open(self, request, *, timeout):
            body = json.loads(request.data)
            model_name = {
                "/api/v1/goal": "GoalRequest",
                "/api/v1/map/save": "MapSaveRequest",
                "/api/v1/localization/relocalizations": "LocalizationRelocalizationRequest",
                "/api/v1/stop": "StopRequest",
                "/api/v1/instruction": "InstructionRequest",
                "/api/v1/navigation/tasks/task-1/cancel": "CancelRequest",
                "/api/v1/agent/runs/run-1/cancel": "AgentTaskCancelRequest",
            }[request.selector]
            model = getattr(schemas, model_name)
            model.model_validate(body)
            # Even permissive models must not silently discard misspelled fields.
            assert set(body) <= set(model.model_fields)
            validated.append(model_name)
            return io.BytesIO(
                json.dumps(
                    {
                        "ok": True,
                        "accepted": True,
                        "request_id": body["request_id"],
                        "task_id": "task-1",
                        "operation_id": "save-1",
                        "run_id": "run-1",
                    }
                ).encode()
            )

    robot._opener = SchemaOpener()
    robot.navigation.go_to(1, 2, z=3, yaw=0.5)
    robot.navigation.cancel("task-1")
    robot.maps.save("lab")
    robot.localization.relocalize("lab")
    robot.localization.relocalize("lab", initial_pose={"x": 1, "y": 2, "z": 3, "yaw": 0.5})
    robot.motion.emergency_stop()
    robot.tasks.send_instruction("inspect doorway", run_id="run-1", control_only=True)
    robot.tasks.cancel_run("run-1")
    assert len(validated) == 8
