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
  Response<CmsState> getCmsState();
  Response<MotorStatus> getMotorStatus();
  Response<Voltage> getVoltage();
  std::unique_ptr<Subscription> subscribeImu(std::function<void(const ImuSample &)> handler);
  std::unique_ptr<Subscription> subscribeJoints(std::function<void(const JointSample &)> handler);
  std::unique_ptr<Subscription> subscribeCmsState(std::function<void(const CmsState &)> handler);

  [[nodiscard]] ControlState state() const;

 private:
  Result bodyAction(bool stand);

  struct Impl;
  std::unique_ptr<Impl> impl_;
};

}  // namespace brainstem
