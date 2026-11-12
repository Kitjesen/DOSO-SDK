#pragma once

#include "brainstem/types.hpp"
#include <memory>

namespace brainstem {

struct ExternalJoint {
  double q{0}, dq{0}, kp{0}, kd{0}, tau{0};
};

struct ExternalFrame {
  std::uint64_t sequence{0};
  std::uint64_t input_window{0};
  std::array<ExternalJoint, 16> joints{};
};

struct ExternalStatus {
  bool selected{false};
  bool active{false};
  bool motor_output_enabled{false};
  bool motor_disable_confirmed{false};
  bool fresh{false};
  std::string session_id;
  std::string reason;
  std::uint64_t last_sequence{0};
  std::uint64_t input_window{0};
  std::uint32_t joint_count{0};
  double received_age_ms{0};
};

class ExternalControl final {
 public:
  explicit ExternalControl(Config config);
  ~ExternalControl();
  ExternalControl(const ExternalControl &) = delete;
  ExternalControl &operator=(const ExternalControl &) = delete;

  Response<ExternalStatus> enter(const std::string &session_id);
  Response<ExternalStatus> enable();
  Response<ExternalStatus> disable();
  Response<ExternalStatus> heartbeat();
  Response<ExternalStatus> exit();
  Response<ExternalStatus> getStatus();
  // Transport write only. status().last_sequence reports robot validation.
  Result send(const ExternalFrame &frame);
  ExternalStatus status() const;

 private:
  struct Impl;
  std::unique_ptr<Impl> impl_;
};

}  // namespace brainstem
