#pragma once

// DOSO Unified C++17 Single-Header Robot SDK (zero gRPC/Protobuf dependency conflicts).
// Inspired by Zsibot SDKClient simplicity + Unitree SportClient / LowLevel separation.

#include "doso_sdk_ffi.h"

#include <array>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <initializer_list>
#include <sstream>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

#if defined(_WIN32)
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#else
#include <dlfcn.h>
#endif

namespace doso {

class Error : public std::runtime_error {
 public:
  explicit Error(const std::string& message) : std::runtime_error(message) {}
};

struct JointCmd {
  double q{0.0};
  double dq{0.0};
  double kp{0.0};
  double kd{0.0};
  double tau{0.0};

  static JointCmd LegPd(double q, double kp, double kd) noexcept {
    return {q, 0.0, kp, kd, 0.0};
  }
  static JointCmd WheelVel(double dq, double kd) noexcept {
    return {0.0, dq, 0.0, kd, 0.0};
  }
  static JointCmd Damping(double kd) noexcept {
    return {0.0, 0.0, 0.0, kd, 0.0};
  }
};

struct ControllerState {
  int32_t mode{0};
  bool active{false};
  bool motor_output_enabled{false};
  bool fresh{false};
  uint64_t last_sequence{0};
  uint32_t joint_count{0};
  bool remote_active{false};
  uint32_t remote_buttons{0};
  double remote_lx{0.0};
  double remote_ly{0.0};
  double remote_rx{0.0};
  double remote_ry{0.0};
};

struct Capabilities {
  bool motion{false};
  bool low_level{false};
  bool observations{false};
  bool navigation{false};
  bool maps{false};
  bool localization{false};
  bool tasks{false};
  bool board_b_autonomy{false};
  bool voice{false};
};

struct BodyControlStatus {
  int32_t body_kind{0};
  int32_t body_transition{0};
  int32_t source_owner{0};
  bool motor_output_enabled{false};
  bool critical_motor_fault{false};
  bool grpc_lease_active{false};
  bool source_ready_for_walk{false};
  uint32_t lease_remaining_ms{0};
  uint64_t last_accepted_sequence{0};
  uint64_t accepted_count{0};
  uint64_t rejected_count{0};
};

struct BodyHeightStatus {
  bool supported{false};
  double current_height_m{0.0};
  double target_height_m{0.0};
  double min_height_m{0.0};
  double max_height_m{0.0};
  double rate_mps{0.0};
};

struct ImuSnapshot {
  bool available{false};
  bool fresh{false};
  bool stream_active{false};
  double age_ms{0.0};
  std::array<double, 3> gyro_rad_s{};
  std::array<double, 3> accel_m_s2{};
  std::array<double, 4> quat_wxyz{1.0, 0.0, 0.0, 0.0};
  int64_t source_elapsed_ns{0};
};

struct JointsSnapshot {
  bool available{false};
  bool complete{false};
  bool fresh{false};
  bool stream_active{false};
  uint16_t valid_mask{0};
  double age_ms{0.0};
  std::array<double, 16> q{};
  std::array<double, 16> dq{};
  std::array<double, 16> tau{};
  std::array<uint32_t, 16> status_code{};
  int64_t source_elapsed_ns{0};
};

struct MotorInfo {
  uint32_t id{0};
  bool online{false};
  uint32_t status_code{0};
  uint32_t error_count{0};
  double temperature_c{0.0};
};

namespace detail {

class FfiApi {
 public:
  static const FfiApi& instance() {
    static FfiApi api;
    return api;
  }

  decltype(&doso_last_error) last_error{};
  decltype(&doso_robot_connect) robot_connect{};
  decltype(&doso_robot_free) robot_free{};
  decltype(&doso_robot_ping) robot_ping{};
  decltype(&doso_robot_id) robot_id{};
  decltype(&doso_firmware_version) firmware_version{};
  decltype(&doso_robot_version) robot_version{};
  decltype(&doso_low_level_endpoint) low_level_endpoint{};
  decltype(&doso_robot_capabilities) robot_capabilities{};

  decltype(&doso_sport_enable_motors) sport_enable_motors{};
  decltype(&doso_sport_disable_motors) sport_disable_motors{};
  decltype(&doso_sport_stand_up) sport_stand_up{};
  decltype(&doso_sport_stand_down) sport_stand_down{};
  decltype(&doso_sport_damp) sport_damp{};
  decltype(&doso_sport_stop) sport_stop{};
  decltype(&doso_sport_switch_profile) sport_switch_profile{};
  decltype(&doso_sport_play_gesture) sport_play_gesture{};
  decltype(&doso_sport_clear_motor_fault) sport_clear_motor_fault{};
  decltype(&doso_sport_clear_joint_fault) sport_clear_joint_fault{};
  decltype(&doso_sport_control_status) sport_control_status{};
  decltype(&doso_sport_current_profile) sport_current_profile{};
  decltype(&doso_sport_list_profiles) sport_list_profiles{};
  decltype(&doso_sport_list_gestures) sport_list_gestures{};
  decltype(&doso_sport_body_height_status) sport_body_height_status{};
  decltype(&doso_sport_set_body_height) sport_set_body_height{};
  decltype(&doso_sport_recovery_status_json) sport_recovery_status_json{};
  decltype(&doso_sport_recovery_enter_json) sport_recovery_enter_json{};
  decltype(&doso_sport_recovery_enable_json) sport_recovery_enable_json{};
  decltype(&doso_sport_recovery_heartbeat_json) sport_recovery_heartbeat_json{};
  decltype(&doso_sport_recovery_start_json) sport_recovery_start_json{};
  decltype(&doso_sport_recovery_cancel_json) sport_recovery_cancel_json{};
  decltype(&doso_sport_recovery_exit_json) sport_recovery_exit_json{};

  decltype(&doso_observations_start_telemetry) observations_start_telemetry{};
  decltype(&doso_observations_latest_imu) observations_latest_imu{};
  decltype(&doso_observations_latest_joints) observations_latest_joints{};
  decltype(&doso_observations_stop_telemetry) observations_stop_telemetry{};
  decltype(&doso_observations_motors) observations_motors{};
  decltype(&doso_observations_motor_voltages) observations_motor_voltages{};

  decltype(&doso_teleop_take_control) teleop_take_control{};
  decltype(&doso_teleop_send_velocity) teleop_send_velocity{};
  decltype(&doso_teleop_publish_velocity) teleop_publish_velocity{};
  decltype(&doso_teleop_release) teleop_release{};
  decltype(&doso_teleop_free) teleop_free{};

  decltype(&doso_low_level_enter) low_level_enter{};
  decltype(&doso_low_level_enable) low_level_enable{};
  decltype(&doso_low_level_disable) low_level_disable{};
  decltype(&doso_low_level_publish_joints16) low_level_publish_joints16{};
  decltype(&doso_low_level_get_state) low_level_get_state{};
  decltype(&doso_low_level_close) low_level_close{};
  decltype(&doso_low_level_free) low_level_free{};

  decltype(&doso_nav_go_to) nav_go_to{};
  decltype(&doso_nav_cancel) nav_cancel{};

  decltype(&doso_voice_status_json) voice_status_json{};
  decltype(&doso_voice_profiles_json) voice_profiles_json{};
  decltype(&doso_voice_switch_profile) voice_switch_profile{};
  decltype(&doso_voice_speak) voice_speak{};
  decltype(&doso_voice_playback_status_json) voice_playback_status_json{};
  decltype(&doso_voice_cancel_playback) voice_cancel_playback{};
  decltype(&doso_voice_switch_component) voice_switch_component{};
  decltype(&doso_voice_update_prompt) voice_update_prompt{};
  decltype(&doso_voice_chat_json) voice_chat_json{};

  [[noreturn]] void throw_last(const char* fallback) const {
    char buf[512]{};
    const size_t n = last_error ? last_error(buf, sizeof(buf)) : 0;
    throw Error(n > 0 ? std::string(buf) : std::string(fallback));
  }

  template <typename Fn>
  Fn optional_symbol(const char* name) const noexcept {
#if defined(_WIN32)
    return reinterpret_cast<Fn>(GetProcAddress(lib_, name));
#else
    return reinterpret_cast<Fn>(dlsym(lib_, name));
#endif
  }

 private:
#if defined(_WIN32)
  HMODULE lib_{nullptr};
#else
  void* lib_{nullptr};
#endif

  template <typename Fn>
  void load(Fn& target, const char* name) {
    target = optional_symbol<Fn>(name);
    if (!target) {
      throw Error(std::string("Missing symbol in doso_sdk_ffi: ") + name);
    }
  }

  static void* open_library() {
    namespace fs = std::filesystem;
    if (const char* env_path = std::getenv("DOSO_SDK_FFI_PATH")) {
      if (*env_path != '\0') {
#if defined(_WIN32)
        if (HMODULE h = LoadLibraryA(env_path)) return h;
#else
        if (void* h = dlopen(env_path, RTLD_NOW | RTLD_LOCAL)) return h;
#endif
      }
    }
#if defined(_WIN32)
    constexpr const char* candidates[] = {
        "doso_sdk_ffi.dll",
        "ffi/target/release/doso_sdk_ffi.dll",
        "ffi/target/debug/doso_sdk_ffi.dll",
        "../ffi/target/release/doso_sdk_ffi.dll",
        "../ffi/target/debug/doso_sdk_ffi.dll",
    };
    for (const char* candidate : candidates) {
      if (fs::exists(candidate) || std::string(candidate) == "doso_sdk_ffi.dll") {
        if (HMODULE h = LoadLibraryA(candidate)) return h;
      }
    }
#elif defined(__APPLE__)
    constexpr const char* candidates[] = {
        "libdoso_sdk_ffi.dylib",
        "ffi/target/release/libdoso_sdk_ffi.dylib",
        "ffi/target/debug/libdoso_sdk_ffi.dylib",
    };
    for (const char* candidate : candidates) {
      if (void* h = dlopen(candidate, RTLD_NOW | RTLD_LOCAL)) return h;
    }
#else
    constexpr const char* candidates[] = {
        "libdoso_sdk_ffi.so",
        "ffi/target/release/libdoso_sdk_ffi.so",
        "ffi/target/debug/libdoso_sdk_ffi.so",
    };
    for (const char* candidate : candidates) {
      if (void* h = dlopen(candidate, RTLD_NOW | RTLD_LOCAL)) return h;
    }
#endif
    return nullptr;
  }

  FfiApi() {
#if defined(_WIN32)
    lib_ = static_cast<HMODULE>(open_library());
#else
    lib_ = open_library();
#endif
    if (!lib_) {
      throw Error("Unable to load doso_sdk_ffi shared library (set DOSO_SDK_FFI_PATH)");
    }
    load(last_error, "doso_last_error");
    load(robot_connect, "doso_robot_connect");
    load(robot_free, "doso_robot_free");
    load(robot_ping, "doso_robot_ping");
    load(robot_id, "doso_robot_id");
    load(firmware_version, "doso_firmware_version");
    load(robot_version, "doso_robot_version");
    load(low_level_endpoint, "doso_low_level_endpoint");
    load(robot_capabilities, "doso_robot_capabilities");

    load(sport_enable_motors, "doso_sport_enable_motors");
    load(sport_disable_motors, "doso_sport_disable_motors");
    load(sport_stand_up, "doso_sport_stand_up");
    load(sport_stand_down, "doso_sport_stand_down");
    load(sport_damp, "doso_sport_damp");
    load(sport_stop, "doso_sport_stop");
    load(sport_switch_profile, "doso_sport_switch_profile");
    load(sport_play_gesture, "doso_sport_play_gesture");
    load(sport_clear_motor_fault, "doso_sport_clear_motor_fault");
    load(sport_clear_joint_fault, "doso_sport_clear_joint_fault");
    load(sport_control_status, "doso_sport_control_status");
    load(sport_current_profile, "doso_sport_current_profile");
    load(sport_list_profiles, "doso_sport_list_profiles");
    load(sport_list_gestures, "doso_sport_list_gestures");
    load(sport_body_height_status, "doso_sport_body_height_status");
    load(sport_set_body_height, "doso_sport_set_body_height");
    load(sport_recovery_status_json, "doso_sport_recovery_status_json");
    load(sport_recovery_enter_json, "doso_sport_recovery_enter_json");
    load(sport_recovery_enable_json, "doso_sport_recovery_enable_json");
    load(sport_recovery_heartbeat_json, "doso_sport_recovery_heartbeat_json");
    load(sport_recovery_start_json, "doso_sport_recovery_start_json");
    load(sport_recovery_cancel_json, "doso_sport_recovery_cancel_json");
    load(sport_recovery_exit_json, "doso_sport_recovery_exit_json");

    load(observations_start_telemetry, "doso_observations_start_telemetry");
    load(observations_latest_imu, "doso_observations_latest_imu");
    load(observations_latest_joints, "doso_observations_latest_joints");
    load(observations_stop_telemetry, "doso_observations_stop_telemetry");
    load(observations_motors, "doso_observations_motors");
    load(observations_motor_voltages, "doso_observations_motor_voltages");

    load(teleop_take_control, "doso_teleop_take_control");
    load(teleop_send_velocity, "doso_teleop_send_velocity");
    load(teleop_publish_velocity, "doso_teleop_publish_velocity");
    load(teleop_release, "doso_teleop_release");
    load(teleop_free, "doso_teleop_free");

    load(low_level_enter, "doso_low_level_enter");
    load(low_level_enable, "doso_low_level_enable");
    load(low_level_disable, "doso_low_level_disable");
    load(low_level_publish_joints16, "doso_low_level_publish_joints16");
    load(low_level_get_state, "doso_low_level_get_state");
    load(low_level_close, "doso_low_level_close");
    load(low_level_free, "doso_low_level_free");

    load(nav_go_to, "doso_nav_go_to");
    load(nav_cancel, "doso_nav_cancel");

    load(voice_status_json, "doso_voice_status_json");
    load(voice_profiles_json, "doso_voice_profiles_json");
    load(voice_switch_profile, "doso_voice_switch_profile");
    load(voice_speak, "doso_voice_speak");
    load(voice_playback_status_json, "doso_voice_playback_status_json");
    load(voice_cancel_playback, "doso_voice_cancel_playback");
    load(voice_switch_component, "doso_voice_switch_component");
    load(voice_update_prompt, "doso_voice_update_prompt");
    load(voice_chat_json, "doso_voice_chat_json");
  }
};

inline std::string read_str(size_t (*fn)(const DosoRobot*, char*, size_t), const DosoRobot* r) {
  char buf[256]{};
  const size_t full = fn(r, buf, sizeof(buf));
  if (full < sizeof(buf)) return std::string(buf);
  std::string out(full + 1, '\0');
  fn(r, out.data(), out.size());
  out.resize(full);
  return out;
}

inline void check(int32_t code, const char* fallback) {
  if (code != 0) FfiApi::instance().throw_last(fallback);
}

inline std::vector<std::string> split_lines(const std::string& raw) {
  std::vector<std::string> items;
  std::istringstream iss(raw);
  std::string line;
  while (std::getline(iss, line)) {
    if (!line.empty()) items.push_back(line);
  }
  return items;
}

}  // namespace detail

class TelemetryStream {
 public:
  ~TelemetryStream() {
    if (handle_) detail::FfiApi::instance().observations_stop_telemetry(handle_);
  }
  TelemetryStream(const TelemetryStream&) = delete;
  TelemetryStream& operator=(const TelemetryStream&) = delete;
  TelemetryStream(TelemetryStream&& other) noexcept
      : handle_(std::exchange(other.handle_, nullptr)) {}
  TelemetryStream& operator=(TelemetryStream&& other) noexcept {
    if (this != &other) {
      if (handle_) detail::FfiApi::instance().observations_stop_telemetry(handle_);
      handle_ = std::exchange(other.handle_, nullptr);
    }
    return *this;
  }

  ImuSnapshot latest_imu() const {
    DosoImuSnapshot raw{};
    detail::check(detail::FfiApi::instance().observations_latest_imu(handle_, &raw),
                  "latest_imu failed");
    ImuSnapshot out;
    out.available = raw.available != 0;
    out.fresh = raw.fresh != 0;
    out.stream_active = raw.stream_active != 0;
    out.age_ms = raw.age_ms;
    for (size_t i = 0; i < 3; ++i) {
      out.gyro_rad_s[i] = raw.gyro_rad_s[i];
      out.accel_m_s2[i] = raw.accel_m_s2[i];
    }
    for (size_t i = 0; i < 4; ++i) out.quat_wxyz[i] = raw.quat_wxyz[i];
    out.source_elapsed_ns = raw.source_elapsed_ns;
    return out;
  }

  JointsSnapshot latest_joints() const {
    DosoJointsSnapshot raw{};
    detail::check(detail::FfiApi::instance().observations_latest_joints(handle_, &raw),
                  "latest_joints failed");
    JointsSnapshot out;
    out.available = raw.available != 0;
    out.complete = raw.complete != 0;
    out.fresh = raw.fresh != 0;
    out.stream_active = raw.stream_active != 0;
    out.valid_mask = raw.valid_mask;
    out.age_ms = raw.age_ms;
    for (size_t i = 0; i < 16; ++i) {
      out.q[i] = raw.q[i];
      out.dq[i] = raw.dq[i];
      out.tau[i] = raw.tau[i];
      out.status_code[i] = raw.status_code[i];
    }
    out.source_elapsed_ns = raw.source_elapsed_ns;
    return out;
  }

  void stop() noexcept {
    if (handle_) {
      detail::FfiApi::instance().observations_stop_telemetry(handle_);
      handle_ = nullptr;
    }
  }

 private:
  friend class ObservationsClient;
  explicit TelemetryStream(DosoTelemetry* handle) : handle_(handle) {}
  DosoTelemetry* handle_{nullptr};
};

class ObservationsClient {
 public:
  TelemetryStream start_telemetry(bool imu = true, bool joints = true,
                                  uint32_t stale_after_ms = 500) const {
    DosoTelemetry* h = detail::FfiApi::instance().observations_start_telemetry(
        robot_, imu ? 1 : 0, joints ? 1 : 0, stale_after_ms);
    if (!h) detail::FfiApi::instance().throw_last("start_telemetry failed");
    return TelemetryStream(h);
  }

  std::vector<MotorInfo> motors() const {
    DosoMotorsSnapshot raw{};
    detail::check(detail::FfiApi::instance().observations_motors(robot_, &raw),
                  "observations motors failed");
    std::vector<MotorInfo> out;
    out.reserve(raw.count);
    for (uint32_t i = 0; i < raw.count && i < 16; ++i) {
      const auto& m = raw.motors[i];
      out.push_back({m.id, m.online != 0, m.status_code, m.error_count, m.temperature_c});
    }
    return out;
  }

  std::vector<double> motor_voltages() const {
    double volts[16]{};
    uint32_t count = 0;
    detail::check(detail::FfiApi::instance().observations_motor_voltages(robot_, volts, &count),
                  "observations motor_voltages failed");
    return std::vector<double>(volts, volts + (count <= 16 ? count : 16));
  }

 private:
  friend class RobotClient;
  explicit ObservationsClient(const DosoRobot* robot) : robot_(robot) {}
  const DosoRobot* robot_;
};

class TeleopSession {
 public:
  ~TeleopSession() {
    if (handle_) detail::FfiApi::instance().teleop_free(handle_);
  }
  TeleopSession(const TeleopSession&) = delete;
  TeleopSession& operator=(const TeleopSession&) = delete;
  TeleopSession(TeleopSession&& other) noexcept
      : handle_(std::exchange(other.handle_, nullptr)) {}
  TeleopSession& operator=(TeleopSession&& other) noexcept {
    if (this != &other) {
      if (handle_) detail::FfiApi::instance().teleop_free(handle_);
      handle_ = std::exchange(other.handle_, nullptr);
    }
    return *this;
  }

  void send_velocity(double vx_mps, double vy_mps, double yaw_rps, bool deadman = true) {
    detail::check(detail::FfiApi::instance().teleop_send_velocity(
                      handle_, vx_mps, vy_mps, yaw_rps, deadman ? 1 : 0),
                  "teleop send_velocity failed");
  }

  void publish_velocity(double vx_mps, double vy_mps, double yaw_rps, bool deadman = true) {
    detail::check(detail::FfiApi::instance().teleop_publish_velocity(
                      handle_, vx_mps, vy_mps, yaw_rps, deadman ? 1 : 0),
                  "teleop publish_velocity failed");
  }

  void release() {
    detail::check(detail::FfiApi::instance().teleop_release(handle_),
                  "teleop release failed");
  }

 private:
  friend class SportClient;
  explicit TeleopSession(DosoTeleop* handle) : handle_(handle) {}
  DosoTeleop* handle_{nullptr};
};

class LowLevelSession {
 public:
  ~LowLevelSession() {
    if (handle_) detail::FfiApi::instance().low_level_free(handle_);
  }
  LowLevelSession(const LowLevelSession&) = delete;
  LowLevelSession& operator=(const LowLevelSession&) = delete;
  LowLevelSession(LowLevelSession&& other) noexcept
      : handle_(std::exchange(other.handle_, nullptr)) {}
  LowLevelSession& operator=(LowLevelSession&& other) noexcept {
    if (this != &other) {
      if (handle_) detail::FfiApi::instance().low_level_free(handle_);
      handle_ = std::exchange(other.handle_, nullptr);
    }
    return *this;
  }

  void enable() {
    detail::check(detail::FfiApi::instance().low_level_enable(handle_),
                  "low_level enable failed");
  }

  void disable() {
    detail::check(detail::FfiApi::instance().low_level_disable(handle_),
                  "low_level disable failed");
  }

  void publish_joints(const std::array<JointCmd, 16>& joints) {
    DosoJointCmd raw[16];
    for (size_t i = 0; i < 16; ++i) {
      raw[i] = {joints[i].q, joints[i].dq, joints[i].kp, joints[i].kd, joints[i].tau};
    }
    detail::check(
        detail::FfiApi::instance().low_level_publish_joints16(handle_, raw),
        "low_level publish_joints failed");
  }

  ControllerState state() const {
    DosoControllerSnapshot s{};
    detail::check(detail::FfiApi::instance().low_level_get_state(handle_, &s),
                  "low_level get_state failed");
    return {
        s.mode,        s.active != 0,   s.motor_output_enabled != 0,
        s.fresh != 0,  s.last_sequence, s.joint_count,
        s.remote_active != 0, s.remote_buttons, s.remote_lx, s.remote_ly,
        s.remote_rx,   s.remote_ry,
    };
  }

  void close() {
    detail::check(detail::FfiApi::instance().low_level_close(handle_),
                  "low_level close failed");
  }

 private:
  friend class RobotClient;
  explicit LowLevelSession(DosoLowLevel* handle) : handle_(handle) {}
  DosoLowLevel* handle_{nullptr};
};

class SportClient {
 public:
  void enable_motors() const {
    detail::check(detail::FfiApi::instance().sport_enable_motors(robot_), "enable_motors failed");
  }
  void disable_motors() const {
    detail::check(detail::FfiApi::instance().sport_disable_motors(robot_), "disable_motors failed");
  }
  void stand_up() const {
    detail::check(detail::FfiApi::instance().sport_stand_up(robot_), "stand_up failed");
  }
  void stand_down() const {
    detail::check(detail::FfiApi::instance().sport_stand_down(robot_), "stand_down failed");
  }
  void damp() const {
    detail::check(detail::FfiApi::instance().sport_damp(robot_), "damp failed");
  }
  void stop() const {
    detail::check(detail::FfiApi::instance().sport_stop(robot_), "stop failed");
  }
  void switch_profile(const std::string& name) const {
    detail::check(detail::FfiApi::instance().sport_switch_profile(robot_, name.c_str()),
                  "switch_profile failed");
  }
  void play_gesture(const std::string& name) const {
    detail::check(detail::FfiApi::instance().sport_play_gesture(robot_, name.c_str()),
                  "play_gesture failed");
  }
  void clear_motor_fault() const {
    detail::check(detail::FfiApi::instance().sport_clear_motor_fault(robot_),
                  "clear_motor_fault failed");
  }
  void clear_motor_fault(uint32_t joint_id) const {
    detail::check(detail::FfiApi::instance().sport_clear_joint_fault(robot_, joint_id),
                  "clear_motor_fault failed");
  }
  BodyControlStatus control_status() const {
    DosoBodyControlStatus raw{};
    detail::check(detail::FfiApi::instance().sport_control_status(robot_, &raw),
                  "control_status failed");
    return {
        raw.body_kind,
        raw.body_transition,
        raw.source_owner,
        raw.motor_output_enabled != 0,
        raw.critical_motor_fault != 0,
        raw.grpc_lease_active != 0,
        raw.source_ready_for_walk != 0,
        raw.lease_remaining_ms,
        raw.last_accepted_sequence,
        raw.accepted_count,
        raw.rejected_count,
    };
  }
  std::string current_profile() const {
    char buf[256]{};
    detail::check(detail::FfiApi::instance().sport_current_profile(robot_, buf, sizeof(buf)),
                  "current_profile failed");
    return std::string(buf);
  }
  std::vector<std::string> list_profiles() const {
    char buf[4096]{};
    detail::check(detail::FfiApi::instance().sport_list_profiles(robot_, buf, sizeof(buf)),
                  "list_profiles failed");
    return detail::split_lines(std::string(buf));
  }
  std::vector<std::string> list_gestures() const {
    char buf[4096]{};
    detail::check(detail::FfiApi::instance().sport_list_gestures(robot_, buf, sizeof(buf)),
                  "list_gestures failed");
    return detail::split_lines(std::string(buf));
  }
  BodyHeightStatus body_height_status() const {
    DosoBodyHeightStatus raw{};
    detail::check(detail::FfiApi::instance().sport_body_height_status(robot_, &raw),
                  "body_height_status failed");
    return {
        raw.supported != 0,
        raw.current_height_m,
        raw.target_height_m,
        raw.min_height_m,
        raw.max_height_m,
        raw.rate_mps,
    };
  }
  BodyHeightStatus set_body_height(double height_m) const {
    DosoBodyHeightStatus raw{};
    detail::check(detail::FfiApi::instance().sport_set_body_height(robot_, height_m, &raw),
                  "set_body_height failed");
    return {
        raw.supported != 0,
        raw.current_height_m,
        raw.target_height_m,
        raw.min_height_m,
        raw.max_height_m,
        raw.rate_mps,
    };
  }
  std::string recovery_status_json() const {
    char buf[2048]{};
    detail::check(detail::FfiApi::instance().sport_recovery_status_json(robot_, buf, sizeof(buf)),
                  "recovery_status failed");
    return std::string(buf);
  }
  std::string recovery_enter_json(const std::string& operation_id) const {
    char buf[2048]{};
    detail::check(detail::FfiApi::instance().sport_recovery_enter_json(
                      robot_, operation_id.c_str(), buf, sizeof(buf)),
                  "recovery_enter failed");
    return std::string(buf);
  }
  std::string recovery_enable_json(const std::string& session_token) const {
    char buf[2048]{};
    detail::check(detail::FfiApi::instance().sport_recovery_enable_json(
                      robot_, session_token.c_str(), buf, sizeof(buf)),
                  "recovery_enable failed");
    return std::string(buf);
  }
  std::string recovery_heartbeat_json(const std::string& session_token) const {
    char buf[2048]{};
    detail::check(detail::FfiApi::instance().sport_recovery_heartbeat_json(
                      robot_, session_token.c_str(), buf, sizeof(buf)),
                  "recovery_heartbeat failed");
    return std::string(buf);
  }
  std::string recovery_start_json(const std::string& operation_id,
                                  const std::string& session_token) const {
    char buf[2048]{};
    detail::check(detail::FfiApi::instance().sport_recovery_start_json(
                      robot_, operation_id.c_str(), session_token.c_str(), buf, sizeof(buf)),
                  "recovery_start failed");
    return std::string(buf);
  }
  std::string recovery_cancel_json(const std::string& operation_id,
                                   const std::string& session_token) const {
    char buf[2048]{};
    detail::check(detail::FfiApi::instance().sport_recovery_cancel_json(
                      robot_, operation_id.c_str(), session_token.c_str(), buf, sizeof(buf)),
                  "recovery_cancel failed");
    return std::string(buf);
  }
  std::string recovery_exit_json(const std::string& session_token) const {
    char buf[2048]{};
    detail::check(detail::FfiApi::instance().sport_recovery_exit_json(
                      robot_, session_token.c_str(), buf, sizeof(buf)),
                  "recovery_exit failed");
    return std::string(buf);
  }
  TeleopSession take_control() const {
    DosoTeleop* t = detail::FfiApi::instance().teleop_take_control(robot_);
    if (!t) detail::FfiApi::instance().throw_last("take_control failed");
    return TeleopSession(t);
  }

 private:
  friend class RobotClient;
  explicit SportClient(const DosoRobot* robot) : robot_(robot) {}
  const DosoRobot* robot_;
};

class NavigationClient {
 public:
  std::string go_to(double x, double y, double z = 0.0, double yaw = 0.0) const {
    char buf[128]{};
    detail::check(
        detail::FfiApi::instance().nav_go_to(robot_, x, y, z, yaw, buf, sizeof(buf)),
        "navigation go_to failed");
    return std::string(buf);
  }
  void cancel(const std::string& task_id, const std::string& reason = "cancelled") const {
    detail::check(
        detail::FfiApi::instance().nav_cancel(robot_, task_id.c_str(), reason.c_str()),
        "navigation cancel failed");
  }

 private:
  friend class RobotClient;
  explicit NavigationClient(const DosoRobot* robot) : robot_(robot) {}
  const DosoRobot* robot_;
};

class VoiceClient {
 public:
  std::string status_json() const {
    char buf[8192]{};
    detail::check(detail::FfiApi::instance().voice_status_json(robot_, buf, sizeof(buf)),
                  "voice status failed");
    return std::string(buf);
  }
  std::string profiles_json() const {
    char buf[16384]{};
    detail::check(detail::FfiApi::instance().voice_profiles_json(robot_, buf, sizeof(buf)),
                  "voice list_profiles failed");
    return std::string(buf);
  }
  void switch_profile(const std::string& profile_id) const {
    detail::check(detail::FfiApi::instance().voice_switch_profile(robot_, profile_id.c_str()),
                  "voice switch_profile failed");
  }
  std::string speak(const std::string& text, const std::string& priority = "normal",
                    const std::string& queue_policy = "enqueue",
                    const std::string& voice_profile_id = "") const {
    char pb_buf[128]{};
    detail::check(detail::FfiApi::instance().voice_speak(
                      robot_, text.c_str(), priority.c_str(), queue_policy.c_str(),
                      voice_profile_id.empty() ? nullptr : voice_profile_id.c_str(), pb_buf,
                      sizeof(pb_buf)),
                  "voice speak failed");
    return std::string(pb_buf);
  }
  std::string playback_status_json(const std::string& playback_id) const {
    char buf[4096]{};
    detail::check(detail::FfiApi::instance().voice_playback_status_json(
                      robot_, playback_id.c_str(), buf, sizeof(buf)),
                  "voice playback_status failed");
    return std::string(buf);
  }
  void cancel_playback(const std::string& playback_id,
                       const std::string& reason = "operator_cancelled") const {
    detail::check(detail::FfiApi::instance().voice_cancel_playback(
                      robot_, playback_id.c_str(), reason.c_str()),
                  "voice cancel_playback failed");
  }
  void switch_component(const std::string& component, const std::string& provider,
                        const std::string& model = "", bool validate = false) const {
    detail::check(detail::FfiApi::instance().voice_switch_component(
                      robot_, component.c_str(), provider.c_str(),
                      model.empty() ? nullptr : model.c_str(), validate ? 1 : 0),
                  "voice switch_component failed");
  }
  void update_prompt(const std::string& system_prompt) const {
    detail::check(detail::FfiApi::instance().voice_update_prompt(robot_, system_prompt.c_str()),
                  "voice update_prompt failed");
  }
  std::string chat_json(const std::string& text, const std::string& thread_id = "",
                        bool speak_reply = false) const {
    char buf[16384]{};
    detail::check(detail::FfiApi::instance().voice_chat_json(
                      robot_, text.c_str(), thread_id.empty() ? nullptr : thread_id.c_str(),
                      speak_reply ? 1 : 0, buf, sizeof(buf)),
                  "voice chat failed");
    return std::string(buf);
  }

 private:
  friend class RobotClient;
  explicit VoiceClient(const DosoRobot* robot) : robot_(robot) {}
  const DosoRobot* robot_;
};

class RobotClient {
 public:
  static RobotClient connect(const std::string& endpoint, const std::string& api_key = "",
                             uint32_t timeout_ms = 10000) {
    const auto& api = detail::FfiApi::instance();
    DosoRobot* raw = api.robot_connect(endpoint.c_str(), api_key.c_str(), timeout_ms);
    if (!raw) api.throw_last("RobotClient::connect failed");
    return RobotClient(raw);
  }

  ~RobotClient() {
    if (handle_) detail::FfiApi::instance().robot_free(handle_);
  }
  RobotClient(const RobotClient&) = delete;
  RobotClient& operator=(const RobotClient&) = delete;
  RobotClient(RobotClient&& other) noexcept
      : handle_(std::exchange(other.handle_, nullptr)) {}
  RobotClient& operator=(RobotClient&& other) noexcept {
    if (this != &other) {
      if (handle_) detail::FfiApi::instance().robot_free(handle_);
      handle_ = std::exchange(other.handle_, nullptr);
    }
    return *this;
  }

  void ping() const {
    detail::check(detail::FfiApi::instance().robot_ping(handle_), "ping failed");
  }
  std::string robot_id() const {
    return detail::read_str(detail::FfiApi::instance().robot_id, handle_);
  }
  std::string firmware_version() const {
    return detail::read_str(detail::FfiApi::instance().firmware_version, handle_);
  }
  std::string robot_version() const {
    return detail::read_str(detail::FfiApi::instance().robot_version, handle_);
  }
  std::string low_level_endpoint() const {
    return detail::read_str(detail::FfiApi::instance().low_level_endpoint, handle_);
  }
  Capabilities capabilities() const {
    DosoCapabilities raw{};
    detail::check(detail::FfiApi::instance().robot_capabilities(handle_, &raw),
                  "capabilities failed");
    return {
        raw.motion != 0,           raw.low_level != 0,
        raw.observations != 0,     raw.navigation != 0,
        raw.maps != 0,             raw.localization != 0,
        raw.tasks != 0,            raw.board_b_autonomy != 0,
        raw.voice != 0,
    };
  }

  SportClient sport() const noexcept { return SportClient(handle_); }
  SportClient motion() const noexcept { return SportClient(handle_); }
  ObservationsClient observations() const noexcept { return ObservationsClient(handle_); }
  NavigationClient navigation() const noexcept { return NavigationClient(handle_); }
  VoiceClient voice() const noexcept { return VoiceClient(handle_); }

  LowLevelSession low_level() const {
    DosoLowLevel* s = detail::FfiApi::instance().low_level_enter(handle_);
    if (!s) detail::FfiApi::instance().throw_last("low_level failed");
    return LowLevelSession(s);
  }

 private:
  explicit RobotClient(DosoRobot* handle) : handle_(handle) {}
  DosoRobot* handle_{nullptr};
};

using DosoClient = RobotClient;

}  // namespace doso
