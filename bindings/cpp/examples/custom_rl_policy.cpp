// Example: Replace DOSO's built-in walking policy with a custom C++17 RL controller.
//
// Zero gRPC / Protobuf dependencies: includes only `doso/robot_client.hpp` and loads
// `doso_sdk_ffi` dynamically. Works identically on standalone Board-A (`:5052`) and
// full Board-A + Board-B robots (automatic redirect to `:5053` for L1 low-level).
//
// SAFETY:
//   - Defaults to read-only dry-run (telemetry + capabilities inspection only).
//   - Pass `--enable-motors` ONLY in a safe area with a physical YUNZHUO controller ready.

#include "doso/robot_client.hpp"

#include <array>
#include <chrono>
#include <cmath>
#include <iostream>
#include <string>
#include <thread>

namespace {

std::array<doso::JointCmd, 16> custom_policy_step(const doso::ImuSnapshot& /*imu*/,
                                                  const doso::JointsSnapshot& joints) {
  std::array<doso::JointCmd, 16> cmds{};
  for (size_t i = 0; i < 12; ++i) {
    cmds[i] = doso::JointCmd::LegPd(joints.q[i], 20.0, 2.0);
  }
  for (size_t i = 12; i < 16; ++i) {
    cmds[i] = doso::JointCmd::WheelVel(0.0, 1.5);
  }
  return cmds;
}

}  // namespace

int main(int argc, char** argv) {
  std::string endpoint = "http://192.168.66.64:5052";
  bool enable_motors = false;
  for (int i = 1; i < argc; ++i) {
    const std::string arg = argv[i];
    if (arg == "--enable-motors") {
      enable_motors = true;
    } else if (arg.rfind("--endpoint=", 0) == 0) {
      endpoint = arg.substr(std::string("--endpoint=").size());
    } else if (arg == "--endpoint" && i + 1 < argc) {
      endpoint = argv[++i];
    }
  }

  try {
    auto robot = doso::RobotClient::connect(endpoint, "", 5000);
    const auto caps = robot.capabilities();
    const auto ctrl = robot.sport().control_status();
    std::cout << "Connected: robot_id=" << robot.robot_id()
              << " fw=" << robot.firmware_version()
              << " low_level=" << caps.low_level
              << " board_b_autonomy=" << caps.board_b_autonomy
              << " body_kind=" << ctrl.body_kind << '\n';

    if (!caps.low_level) {
      std::cerr << "Robot does not advertise low_level capability.\n";
      return 2;
    }

    auto obs = robot.observations().start_telemetry(true, true, 200);
    doso::ImuSnapshot imu{};
    doso::JointsSnapshot joints{};
    for (int i = 0; i < 150; ++i) {
      imu = obs.latest_imu();
      joints = obs.latest_joints();
      if (imu.available && joints.complete) break;
      std::this_thread::sleep_for(std::chrono::milliseconds(20));
    }
    if (!imu.available || !joints.complete) {
      std::cerr << "Timed out waiting for IMU + 16-joint telemetry.\n";
      return 3;
    }

    if (!enable_motors) {
      std::cout << "[DRY-RUN] IMU accel=(" << imu.accel_m_s2[0] << ", " << imu.accel_m_s2[1]
                << ", " << imu.accel_m_s2[2] << ") q[0]=" << joints.q[0] << '\n';
      return 0;
    }

    auto ll = robot.low_level();
    ll.publish_joints(custom_policy_step(obs.latest_imu(), obs.latest_joints()));
    ll.enable();
    const auto end_time = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (std::chrono::steady_clock::now() < end_time) {
      const auto t0 = std::chrono::steady_clock::now();
      imu = obs.latest_imu();
      joints = obs.latest_joints();
      if (!imu.fresh || !joints.fresh || !joints.complete) {
        std::cerr << "Telemetry stale — stopping safely.\n";
        break;
      }
      ll.publish_joints(custom_policy_step(imu, joints));
      if (!ll.state().active) {
        std::cerr << "External authority revoked.\n";
        break;
      }
      const auto elapsed = std::chrono::steady_clock::now() - t0;
      const auto period = std::chrono::milliseconds(20);
      if (elapsed < period) {
        std::this_thread::sleep_for(period - elapsed);
      }
    }
    ll.disable();
    ll.close();
    return 0;
  } catch (const doso::Error& err) {
    std::cerr << "DOSO SDK error: " << err.what() << '\n';
    return 1;
  }
}
