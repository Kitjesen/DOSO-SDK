#include <brainstem/client.hpp>
#include <chrono>
#include <cstdlib>
#include <iostream>
#include <string>
#include <thread>

namespace {

const char *bodyStateName(brainstem::BodyStateKind state) {
  switch (state) {
    case brainstem::BodyStateKind::Zero:
      return "zero";
    case brainstem::BodyStateKind::Grounded:
      return "grounded";
    case brainstem::BodyStateKind::Standing:
      return "standing";
    case brainstem::BodyStateKind::Walking:
      return "walking";
    case brainstem::BodyStateKind::Transitioning:
      return "transitioning";
    case brainstem::BodyStateKind::Unknown:
      return "unknown";
  }
  return "unknown";
}

bool configureTransport(int argc, char **argv, brainstem::Config &config) {
  if (argc == 4) {
    config.allow_insecure = true;
    return true;
  }
  if (argc != 7 && argc != 8) {
    return false;
  }
  config.tls.ca_file = argv[4];
  config.tls.certificate_file = argv[5];
  config.tls.private_key_file = argv[6];
  if (argc == 8) {
    config.tls.server_name = argv[7];
  }
  return true;
}

}  // namespace

int main(int argc, char **argv) {
  if (argc < 4) {
    std::cerr << "usage: " << argv[0]
              << " <host:port> <client_id> <duration_ms>"
                 " [<ca.pem> <client.crt> <client.key> [<server_name>]]\n";
    return 64;
  }

  brainstem::Config config;
  config.target = argv[1];
  config.client_id = argv[2];
  if (!configureTransport(argc, argv, config)) {
    std::cerr << "invalid TLS arguments\n";
    return 64;
  }

  std::chrono::milliseconds duration;
  try {
    duration = std::chrono::milliseconds(std::stoll(argv[3]));
  } catch (const std::exception &error) {
    std::cerr << "invalid duration_ms: " << error.what() << '\n';
    return 64;
  }
  if (duration <= std::chrono::milliseconds::zero()) {
    std::cerr << "duration_ms must be positive\n";
    return 64;
  }

  brainstem::Client client(config);
  const brainstem::Result connected = client.connect();
  if (!connected.ok) {
    std::cerr << "cannot connect: " << connected.state.reason << " " << connected.error << '\n';
    return 1;
  }
  const brainstem::Result started = client.startTelemetry();
  if (!started.ok) {
    std::cerr << "cannot start telemetry: " << started.state.reason << " " << started.error << '\n';
    return 1;
  }

  std::uint64_t imu_sequence = 0;
  std::uint64_t joint_sequence = 0;
  std::uint64_t body_sequence = 0;
  const auto deadline = std::chrono::steady_clock::now() + duration;
  while (std::chrono::steady_clock::now() < deadline) {
    const brainstem::ImuSnapshot imu = client.latestImu();
    if (imu.available && imu.sequence != imu_sequence) {
      imu_sequence = imu.sequence;
      const auto &gyro = imu.value.angular_velocity_rps;
      std::cout << "imu seq=" << imu.sequence << " fresh=" << imu.fresh
                << " age_ms=" << imu.age.count() << " gyro_rps=[" << gyro.x << ',' << gyro.y << ','
                << gyro.z << ']';
      if (imu.value.linear_acceleration_mps2) {
        const auto &accel = *imu.value.linear_acceleration_mps2;
        std::cout << " accel_mps2=[" << accel.x << ',' << accel.y << ',' << accel.z << ']';
      }
      std::cout << '\n';
    }

    const brainstem::JointSnapshot joints = client.latestJoints();
    if (joints.valid_mask != 0 && joints.sequence != joint_sequence) {
      joint_sequence = joints.sequence;
      const auto &joint0 = joints.joints[0];
      std::cout << "joints seq=" << joints.sequence << " valid_mask=0x" << std::hex
                << joints.valid_mask << " fresh_mask=0x" << joints.fresh_mask << std::dec
                << " joint0=[pos=" << joint0.position_rad << " vel=" << joint0.velocity_rps
                << " torque=" << joint0.torque_nm << " status=" << joint0.status_code << "]\n";
    }

    const brainstem::BodySnapshot body = client.latestBodyState();
    if (body.available && body.sequence != body_sequence) {
      body_sequence = body.sequence;
      std::cout << "body seq=" << body.sequence << " state=" << bodyStateName(body.value.kind)
                << " gesture=" << body.value.gesture_name << '\n';
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
  }

  client.stopTelemetry();
  return 0;
}
