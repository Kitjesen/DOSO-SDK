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
                self.assertTrue(caps.voice)

                vst = robot.voice().status()
                self.assertEqual(vst.service, "askme")
                self.assertEqual(vst.llm_model, "deepseek-v4-flash")
                vprofs = robot.voice().list_profiles()
                self.assertEqual(vprofs.active_profile, "patrol_default")
                self.assertEqual(vprofs.profiles[0].label, "巡检播报")
                robot.voice().switch_profile("patrol_default")
                pb_id = robot.voice().speak("系统自检通过")
                self.assertEqual(pb_id, "pb-mock-1")
                pb = robot.voice().playback_status(pb_id)
                self.assertEqual(pb.state, "completed")
                robot.voice().cancel_playback(pb_id, "test")
                robot.voice().switch_component("llm", "deepseek", "deepseek-v4-flash")
                robot.voice().update_prompt("你是小算机器狗。")
                chat_reply = robot.voice().chat("你好")
                self.assertEqual(chat_reply.reply_text, "收到: 你好")
                self.assertEqual(chat_reply.thread_id, "conv-mock-1")

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

                bh = robot.sport().body_height_status()
                self.assertTrue(bh.supported)
                self.assertAlmostEqual(bh.current_height_m, 0.375, places=5)
                self.assertAlmostEqual(bh.min_height_m, 0.25, places=5)
                self.assertAlmostEqual(bh.max_height_m, 0.50, places=5)
                bh2 = robot.sport().set_body_height(0.42)
                self.assertTrue(bh2.supported)
                self.assertAlmostEqual(bh2.target_height_m, 0.42, places=5)
                with self.assertRaises(DosoError):
                    robot.sport().set_body_height(0.99)

                rec = robot.sport().recovery_status()
                self.assertEqual(rec.availability, 2)
                rec_enter = robot.sport().recovery_enter("rec-op-1")
                self.assertTrue(rec_enter.selected)
                self.assertEqual(rec_enter.session_token, "rec-mock-tok")
                robot.sport().recovery_heartbeat(rec_enter.session_token)
                rec_exit = robot.sport().recovery_exit(rec_enter.session_token)
                self.assertFalse(rec_exit.selected)
                self.assertTrue(rec_exit.motor_disable_confirmed)

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
