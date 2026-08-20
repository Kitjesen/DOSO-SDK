#pragma once

#include <array>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace brainstem {

struct TlsConfig {
  std::string ca_file;
  std::string certificate_file;
  std::string private_key_file;
  std::string server_name;

  [[nodiscard]] bool enabled() const noexcept {
    return !ca_file.empty() || !certificate_file.empty() || !private_key_file.empty();
  }
};

struct Config {
  std::string target{"127.0.0.1:13145"};
  std::chrono::milliseconds timeout{1000};
  std::string client_id;
  bool allow_insecure{false};
  TlsConfig tls;
  std::chrono::milliseconds telemetry_stale_after{500};
};

struct Velocity {
  double vx_mps{0.0};
  double vy_mps{0.0};
  double yaw_rps{0.0};
};

struct Vector3 {
  double x{0.0};
  double y{0.0};
  double z{0.0};
};

struct Quaternion {
  double w{1.0};
  double x{0.0};
  double y{0.0};
  double z{0.0};
};

struct ImuSample {
  Vector3 angular_velocity_rps;
  Quaternion orientation;
  std::chrono::nanoseconds elapsed{0};
};

struct JointState {
  std::uint32_t id{0};
  double position_rad{0.0};
  double velocity_rps{0.0};
  double torque_nm{0.0};
  std::uint32_t status_code{0};
};

struct JointSample {
  std::vector<JointState> joints;
  std::chrono::nanoseconds elapsed{0};
};

enum class CmsStateKind {
  Unknown,
  Zero,
  Grounded,
  Standing,
  Walking,
  Transitioning,
};

enum class CmsTransition {
  None,
  StandUp,
  SitDown,
  Gesture,
};

struct CmsState {
  CmsStateKind kind{CmsStateKind::Unknown};
  CmsTransition transition{CmsTransition::None};
  std::string gesture_name;
};

inline constexpr std::size_t kJointCount = 16;

struct TelemetryOptions {
  bool imu{true};
  bool joints{true};
  bool cms{true};
};

struct ImuSnapshot {
  bool active{false};
  bool available{false};
  bool fresh{false};
  std::chrono::milliseconds age{0};
  std::uint64_t sequence{0};
  ImuSample value;
};

struct JointSnapshot {
  bool active{false};
  std::uint16_t valid_mask{0};
  std::uint16_t fresh_mask{0};
  std::uint64_t sequence{0};
  std::array<JointState, kJointCount> joints{};
  std::array<std::chrono::nanoseconds, kJointCount> elapsed{};

  [[nodiscard]] bool complete() const noexcept { return valid_mask == 0xFFFFU; }
  [[nodiscard]] bool fresh() const noexcept { return fresh_mask == 0xFFFFU; }
};

struct CmsSnapshot {
  bool active{false};
  bool available{false};
  std::uint64_t sequence{0};
  CmsState value;
};

struct TelemetrySnapshot {
  ImuSnapshot imu;
  JointSnapshot joints;
  CmsSnapshot cms;
};

struct MotorState {
  std::uint32_t id{0};
  bool online{false};
  std::uint32_t status_code{0};
  double temperature_c{0.0};
  double voltage_v{0.0};
  double position_rad{0.0};
  double velocity_rps{0.0};
  double torque_nm{0.0};
  std::vector<std::uint32_t> errors;
};

struct MotorStatus {
  std::vector<MotorState> motors;
};

struct Voltage {
  std::vector<double> values_v;
};

struct ControlState {
  bool connected{false};
  bool ready{false};
  bool motors_enabled{false};
  bool critical_fault{false};
  bool lease_valid{false};
  bool initial_zero_acknowledged{false};
  std::uint32_t lease_remaining_ms{0};
  std::uint64_t accepted_sequence{0};
  std::uint64_t accepted_count{0};
  std::uint64_t rejected_count{0};
  std::string fsm{"unknown"};
  std::string reason{"not_connected"};
  std::string control_owner{"none"};
  std::string control_owner_id;
};

struct Result {
  bool ok{false};
  bool transport_ok{false};
  bool accepted{false};
  bool stop_confirmed{false};
  ControlState state;
  std::string error;

  [[nodiscard]] bool confirmsStop() const noexcept {
    return ok && transport_ok && accepted && stop_confirmed && !state.ready && !state.lease_valid;
  }
};

template <typename T>
struct Response {
  Result result;
  T value;
};

}  // namespace brainstem
