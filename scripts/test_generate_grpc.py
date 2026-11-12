"""Check package imports and stale binding cleanup after protocol generation."""

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from textwrap import dedent


class GenerationTest(unittest.TestCase):
    def test_regenerate_and_import(self):
        generator = Path(__file__).with_name("generate_grpc.py")
        with tempfile.TemporaryDirectory() as directory:
            gateway = Path(directory) / "src/gateway"
            gateway.mkdir(parents=True)
            (gateway / "gateway_module.py").touch()
            command = [sys.executable, str(generator), "--lingtu", directory]
            subprocess.run(command, check=True)
            generated = gateway / "rpc/generated"
            before = {path.relative_to(generated): path.read_bytes() for path in generated.rglob("*.py")}
            (generated / "doso_pb2.py").write_text("obsolete", encoding="utf-8")
            (generated / "doso_pb2_grpc.py").write_text("obsolete", encoding="utf-8")
            stale = generated / "obsolete"
            stale.mkdir()
            (stale / "__init__.py").touch()
            (stale / "old_pb2.py").write_text("obsolete", encoding="utf-8")
            subprocess.run(command, check=True)
            self.assertEqual(before, {path.relative_to(generated): path.read_bytes() for path in generated.rglob("*.py")})
            subprocess.run(
                [
                    sys.executable,
                    "-c",
                    dedent('''
                    from pathlib import Path
                    import importlib
                    for path in Path('generated').rglob('*_pb2*.py'):
                        importlib.import_module('.'.join(path.with_suffix('').parts))
                    exports = {
                        'common': ['Snapshot', 'Command', 'Pose', 'Receipt'],
                        'error': ['ErrorDetail'],
                        'system': ['Handshake', 'HandshakeReply', 'SystemInfo', 'ControlState', 'FaultReport'],
                        'motion': ['WatchStateRequest', 'BodyState', 'TeleopReply', 'ProfileInfo', 'SwitchProfileRequest'],
                        'observations': ['ImuSample', 'JointSample', 'MotorStates', 'CameraFrame', 'CloudFrame'],
                        'localization': ['PoseSample', 'Relocalize'],
                        'maps': ['MapList', 'MapContent', 'MapOperation', 'ProductOperation'],
                        'navigation': ['Goal', 'Task'],
                        'tasks': ['Instruction', 'Run'],
                    }
                    for domain, names in exports.items():
                        facade = importlib.import_module(f'generated.{domain}_pb2')
                        messages = importlib.import_module(f'generated.messages.{domain}_pb2')
                        for name in names:
                            assert getattr(facade, name) is getattr(messages, name), (domain, name)
                    from generated import motion_pb2
                    from generated.messages import controller_pb2
                    for name in ['ControllerMode', 'ControllerState', 'JointCommand', 'JointTargets',
                                 'ExternalControlRequest', 'ExternalControlReply']:
                        assert getattr(motion_pb2, name) is getattr(controller_pb2, name), name
                    methods = {
                        ('system', 'System', 'Handshake'): ('Handshake', 'HandshakeReply'),
                        ('system', 'System', 'WatchFaults'): ('Empty', 'FaultReport'),
                        ('motion', 'Motion', 'Resume'): ('Command', 'Receipt'),
                        ('motion', 'Motion', 'EnableMotors'): ('Command', 'Receipt'),
                        ('motion', 'Motion', 'DisableMotors'): ('Command', 'Receipt'),
                        ('motion', 'Motion', 'Profile'): ('Empty', 'ProfileInfo'),
                        ('motion', 'Motion', 'SwitchProfile'): ('SwitchProfileRequest', 'Receipt'),
                        ('motion', 'Motion', 'Controller'): ('Empty', 'ControllerState'),
                        ('motion', 'Motion', 'ExternalControl'): ('ExternalControlRequest', 'ExternalControlReply'),
                        ('motion', 'Motion', 'WatchControlState'): ('Empty', 'ControlState'),
                        ('motion', 'Motion', 'WatchState'): ('Empty', 'Snapshot'),
                        ('motion', 'Motion', 'WatchBodyState'): ('Empty', 'BodyState'),
                        ('observations', 'Observations', 'WatchImu'): ('Empty', 'ImuSample'),
                        ('observations', 'Observations', 'WatchPointCloud'): ('CloudRequest', 'CloudFrame'),
                        ('localization', 'Localization', 'Pose'): ('Empty', 'PoseSample'),
                        ('maps', 'Maps', 'PointCloud'): ('MapContentRequest', 'MapContent'),
                        ('navigation', 'Navigation', 'Watch'): ('TaskQuery', 'Task'),
                        ('tasks', 'Tasks', 'Get'): ('RunQuery', 'Run'),
                    }
                    for (domain, service, method), (request, response) in methods.items():
                        module = importlib.import_module(f'generated.{domain}_pb2')
                        rpc = module.DESCRIPTOR.services_by_name[service].methods_by_name[method]
                        assert rpc.input_type.full_name == f'doso.v1.{request}'
                        assert rpc.output_type.full_name == f'doso.v1.{response}'
                        if method in ('Controller', 'ExternalControl'):
                            assert rpc.client_streaming == (method == 'ExternalControl')
                            assert rpc.server_streaming == (method == 'ExternalControl')
                    '''),
                ],
                cwd=generated.parent,
                check=True,
            )
            (Path(directory) / "src/runtime/__init__.py").touch()
            subprocess.run(
                [
                    sys.executable,
                    "-c",
                    "from runtime.adapters.brainstem.generated.brainstem_api.cms_pb2 import Imu, Joint; "
                    "from runtime.adapters.brainstem.generated.brainstem_api.external_pb2 import ExternalStatus; "
                    "assert Imu.DESCRIPTOR.full_name == 'brainstem.api.v1.Imu'; "
                    "assert ExternalStatus.DESCRIPTOR.full_name == 'brainstem.api.v1.ExternalStatus'; "
                    "assert Joint.DESCRIPTOR.full_name == 'brainstem.api.v1.Joint'",
                ],
                cwd=directory,
                env={**os.environ, "PYTHONPATH": str(Path(directory) / "src")},
                check=True,
            )

    def test_nested_messages_keep_public_exports(self):
        sdk = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as directory:
            checkout = Path(directory) / "sdk"
            scripts = checkout / "scripts"
            scripts.mkdir(parents=True)
            shutil.copyfile(sdk / "scripts/generate_grpc.py", scripts / "generate_grpc.py")
            shutil.copytree(sdk / "brainstem/protocol", checkout / "brainstem/protocol")
            source = checkout / "proto"
            (source / "messages").mkdir(parents=True)
            for name, text in {
                "common.proto": 'import public "messages/common.proto";',
                "messages/common.proto": "message Vector3 { double x = 1; }",
                "messages/sensors.proto": 'import "common.proto"; message ImuSample { Vector3 velocity = 1; }',
                "observations.proto": 'import public "messages/sensors.proto"; service Observations { rpc Read(ImuSample) returns (ImuSample); }',
            }.items():
                (source / name).write_text('syntax = "proto3"; package doso.v1; ' + text, encoding="utf-8")
            lingtu = Path(directory) / "lingtu"
            gateway = lingtu / "src/gateway"
            gateway.mkdir(parents=True)
            (gateway / "gateway_module.py").touch()
            subprocess.run(
                [sys.executable, str(scripts / "generate_grpc.py"), "--lingtu", str(lingtu)], check=True
            )
            subprocess.run(
                [
                    sys.executable,
                    "-c",
                    "from generated import common_pb2, observations_pb2, observations_pb2_grpc; "
                    "from generated.messages import common_pb2 as nested_common, sensors_pb2; "
                    "assert common_pb2.Vector3 is nested_common.Vector3; "
                    "assert observations_pb2.ImuSample is sensors_pb2.ImuSample; "
                    "sample = observations_pb2.ImuSample(velocity=common_pb2.Vector3(x=1.5)); "
                    "assert sensors_pb2.ImuSample.FromString(sample.SerializeToString()).velocity.x == 1.5",
                ],
                cwd=gateway / "rpc",
                check=True,
            )


if __name__ == "__main__":
    unittest.main()
