#pragma once

#include <functional>
#include <memory>

#include "brainstem/types.hpp"

namespace brainstem {

class Subscription final {
 public:
  ~Subscription();

  Subscription(const Subscription &) = delete;
  Subscription &operator=(const Subscription &) = delete;
  Subscription(Subscription &&) noexcept;
  Subscription &operator=(Subscription &&) noexcept;

  void cancel() noexcept;
  [[nodiscard]] bool active() const noexcept;
  Result wait();

 private:
  friend class Client;

  struct Impl;
  explicit Subscription(std::unique_ptr<Impl> impl);
  std::unique_ptr<Impl> impl_;
};

class Client final {
 public:
  explicit Client(Config config);
  ~Client();

  Client(const Client &) = delete;
  Client &operator=(const Client &) = delete;
  Client(Client &&) noexcept;
  Client &operator=(Client &&) noexcept;

  Result connect();
  Result refresh();
  Result move(const Velocity &velocity);
  Result stop() noexcept;
  Result standUp();
  Result sitDown();
  Result enableMotors();
  Result disableMotors();
  Result setJointZero();
  Result clearMotorFaults(const std::vector<std::uint32_t> &joint_ids = {});
  Response<std::chrono::system_clock::time_point> getServerStartTime();
  Response<BodyState> getBodyState();
  Response<MotorStatus> getMotorStatus();
  Response<Voltage> getVoltage();
  Result startTelemetry(TelemetryOptions options = {});
  void stopTelemetry() noexcept;
  [[nodiscard]] TelemetrySnapshot telemetry() const;
  [[nodiscard]] ImuSnapshot latestImu() const;
  [[nodiscard]] JointSnapshot latestJoints() const;
  [[nodiscard]] BodyStateSnapshot latestBodyState() const;
  std::unique_ptr<Subscription> subscribeImu(std::function<void(const ImuSample &)> handler);
  std::unique_ptr<Subscription> subscribeJoints(std::function<void(const JointSample &)> handler);
  std::unique_ptr<Subscription> subscribeBodyState(std::function<void(const BodyState &)> handler);

  [[nodiscard]] ControlState controlState() const;

 private:
  Result bodyAction(bool stand);
  Result motorPower(bool enable);

  struct Impl;
  std::unique_ptr<Impl> impl_;
};

}  // namespace brainstem
