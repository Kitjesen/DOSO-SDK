#pragma once

#include <chrono>
#include <cstdint>
#include <string>

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
};

struct Velocity {
  double vx_mps{0.0};
  double vy_mps{0.0};
  double yaw_rps{0.0};
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

}  // namespace brainstem
