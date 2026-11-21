#include "doso/robot_client.hpp"

#include <array>
#include <cassert>
#include <chrono>
#include <cmath>
#include <cstring>
#include <iostream>
#include <thread>

int main() {
  const auto& api = doso::detail::FfiApi::instance();
  auto start_fn = api.optional_symbol<decltype(&doso_test_mock_server_start)>(
      "doso_test_mock_server_start");
  auto writes_fn = api.optional_symbol<decltype(&doso_test_mock_server_writes)>(
      "doso_test_mock_server_writes");
  auto stop_fn = api.optional_symbol<decltype(&doso_test_mock_server_stop)>(
      "doso_test_mock_server_stop");
  assert(start_fn && writes_fn && stop_fn);

  char endpoint[128]{};
  DosoMockServer* mock = start_fn(endpoint, sizeof(endpoint));
  assert(mock != nullptr);

  auto robot = doso::RobotClient::connect(endpoint, "", 3000);
  robot.ping();
  assert(robot.firmware_version() == "doso-ffi-test");
  assert(robot.robot_id() == "8f7e1c62-0a4d-4b89-96e2-6c47b03db1a5");

  const auto caps = robot.capabilities();
  assert(caps.motion);
  assert(caps.low_level);
  assert(caps.observations);
  assert(caps.board_b_autonomy);

  const auto ctrl_status = robot.sport().control_status();
  assert(ctrl_status.body_kind == 1);  // Grounded
  assert(!ctrl_status.motor_output_enabled);
  assert(ctrl_status.accepted_count == 11);

  assert(robot.sport().current_profile() == "flat");
  const auto profiles = robot.sport().list_profiles();
  assert(profiles.size() == 2 && profiles[0] == "flat" && profiles[1] == "stairs");
  const auto gestures = robot.sport().list_gestures();
  assert(gestures.size() == 1 && gestures[0] == "bow");

  {
    auto obs = robot.observations().start_telemetry(true, true, 500);
    doso::ImuSnapshot imu{};
    doso::JointsSnapshot joints{};
    for (int i = 0; i < 50; ++i) {
      imu = obs.latest_imu();
      joints = obs.latest_joints();
      if (imu.available && joints.complete) break;
      std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
    assert(imu.available && imu.fresh);
    assert(std::abs(imu.accel_m_s2[2] - 9.81) < 1e-6);
    assert(std::abs(imu.quat_wxyz[0] - 1.0) < 1e-6);
    assert(joints.available && joints.complete && joints.valid_mask == 0xFFFF);
    assert(std::abs(joints.q[10] - 0.5) < 1e-6);
  }

  const auto motors = robot.observations().motors();
  assert(motors.size() == 16 && motors[0].online);
  assert(std::abs(motors[0].temperature_c - 28.5) < 1e-6);
  const auto volts = robot.observations().motor_voltages();
  assert(volts.size() == 16 && std::abs(volts[0] - 50.2) < 1e-6);

  robot.sport().stand_up();
  robot.sport().damp();
  robot.sport().play_gesture("bow");
  robot.sport().clear_motor_fault();

  {
    auto teleop = robot.sport().take_control();
    teleop.send_velocity(0.4, 0.0, 0.2, true);
    teleop.release();
  }

  {
    auto ll = robot.low_level();
    std::array<doso::JointCmd, 16> cmds{};
    for (size_t i = 0; i < 12; ++i) {
      cmds[i] = doso::JointCmd::LegPd(0.15, 45.0, 2.5);
    }
    for (size_t i = 12; i < 16; ++i) {
      cmds[i] = doso::JointCmd::WheelVel(2.5, 1.5);
    }
    ll.publish_joints(cmds);
    ll.enable();
    auto state = ll.state();
    assert(state.active);
    assert(state.motor_output_enabled);
    assert(state.remote_active);
    assert(state.remote_buttons == 3);
    ll.close();
  }

  const std::string task_id = robot.navigation().go_to(1.0, 2.0, 0.0, 0.5);
  assert(task_id == "mock-task-1");
  robot.navigation().cancel(task_id, "done");

  assert(writes_fn(mock) >= 8);
  stop_fn(mock);
  std::cout << "C++17 RobotClient smoke test passed: " << endpoint << '\n';
  return 0;
}
