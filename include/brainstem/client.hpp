#pragma once

#include <memory>

#include "brainstem/types.hpp"

namespace brainstem {

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

  [[nodiscard]] ControlState state() const;

 private:
  Result bodyAction(bool stand);

  struct Impl;
  std::unique_ptr<Impl> impl_;
};

}  // namespace brainstem
