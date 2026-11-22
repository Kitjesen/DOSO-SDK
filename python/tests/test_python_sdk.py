import ctypes
import sys
import time
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from doso_sdk import DosoClient, DosoError, JointCmd, RobotClient
from doso_sdk.client import _lib


class PythonSdkTest(unittest.TestCase):
    def test_robot_sport_teleop_and_low_level_round_trip(self) -> None:
        lib = _lib()
        lib.doso_test_mock_server_start.argtypes = [ctypes.c_char_p, ctypes.c_size_t]
        lib.doso_test_mock_server_start.restype = ctypes.c_void_p
        lib.doso_test_mock_server_writes.argtypes = [ctypes.c_void_p]
        lib.doso_test_mock_server_writes.restype = ctypes.c_uint64
        lib.doso_test_mock_server_stop.argtypes = [ctypes.c_void_p]
        lib.doso_test_mock_server_stop.restype = None

        buf = ctypes.create_string_buffer(128)
        mock = lib.doso_test_mock_server_start(buf, len(buf))
        self.assertTrue(mock)
        endpoint = buf.value.decode("utf-8")

        try:
            with RobotClient.connect(endpoint, timeout_ms=3000) as robot:
                self.assertIs(DosoClient, RobotClient)
                robot.ping()
                self.assertEqual(robot.firmware_version, "doso-ffi-test")
                self.assertEqual(robot.robot_id, "8f7e1c62-0a4d-4b89-96e2-6c47b03db1a5")

                caps = robot.capabilities()
                self.assertTrue(caps.motion)
                self.assertTrue(caps.low_level)
                self.assertTrue(caps.observations)
                self.assertTrue(caps.board_b_autonomy)

                status = robot.sport().control_status()
                self.assertEqual(status.body_kind, 1)  # Grounded
                self.assertFalse(status.motor_output_enabled)
                self.assertEqual(status.accepted_count, 11)

                self.assertEqual(robot.sport().current_profile(), "flat")
                self.assertEqual(robot.sport().list_profiles(), ["flat", "stairs"])
                self.assertEqual(robot.sport().list_gestures(), ["bow"])

                with robot.observations().telemetry(imu=True, joints=True) as obs:
                    imu = obs.latest_imu()
                    joints = obs.latest_joints()
                    for _ in range(50):
                        imu = obs.latest_imu()
                        joints = obs.latest_joints()
                        if imu.available and joints.complete:
                            break
                        time.sleep(0.01)
                    self.assertTrue(imu.available and imu.fresh)
                    self.assertAlmostEqual(imu.accel_m_s2[2], 9.81, places=5)
                    self.assertAlmostEqual(imu.quat_wxyz[0], 1.0, places=5)
                    self.assertTrue(joints.available and joints.complete)
                    self.assertEqual(joints.valid_mask, 0xFFFF)
                    self.assertAlmostEqual(joints.q[10], 0.5, places=5)

                motors = robot.observations().motors()
                self.assertEqual(len(motors), 16)
                self.assertTrue(motors[0].online)
                self.assertAlmostEqual(motors[0].temperature_c, 28.5, places=5)

                volts = robot.observations().motor_voltages()
                self.assertEqual(len(volts), 16)
                self.assertAlmostEqual(volts[0], 50.2, places=5)

                robot.sport().stand_up()
                robot.sport().damp()
                robot.sport().play_gesture("bow")
                robot.sport().clear_motor_fault()

                with robot.sport().take_control() as teleop:
                    teleop.send_velocity(0.3, 0.0, 0.1, deadman=True)

                with robot.low_level() as ll:
                    cmds = [JointCmd.leg_pd(0.1, 40.0, 2.0) for _ in range(12)] + [
                        JointCmd.wheel_vel(2.5, 1.5) for _ in range(4)
                    ]
                    ll.publish_joints(cmds)
                    ll.enable()
                    snap = ll.state()
                    self.assertTrue(snap.active)
                    self.assertTrue(snap.motor_output_enabled)
                    self.assertTrue(snap.remote_active)
                    self.assertEqual(snap.remote_buttons, 3)

                with self.assertRaises(DosoError):
                    robot.sport().clear_motor_fault(99)

                task_id = robot.navigation().go_to(1.0, 2.0, 0.0, 0.5)
                self.assertEqual(task_id, "mock-task-1")
                robot.navigation().cancel(task_id, "done")

            self.assertGreaterEqual(lib.doso_test_mock_server_writes(mock), 8)
        finally:
            lib.doso_test_mock_server_stop(mock)


if __name__ == "__main__":
    unittest.main()
