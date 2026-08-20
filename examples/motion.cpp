#include <algorithm>
#include <brainstem/client.hpp>
#include <chrono>
#include <cstdlib>
#include <iostream>
#include <string>
#include <thread>

int main(int argc, char **argv) {
  if (argc != 7 && argc != 10 && argc != 11) {
    std::cerr << "usage: " << argv[0]
              << " <host:port> <client_id> <vx_mps> <vy_mps> <yaw_rps> <duration_ms>"
                 " [<ca.pem> <client.crt> <client.key> [<server_name>]]\n";
    return 64;
  }

  brainstem::Config config;
  config.target = argv[1];
  config.client_id = argv[2];
  if (argc >= 10) {
    config.tls.ca_file = argv[7];
    config.tls.certificate_file = argv[8];
    config.tls.private_key_file = argv[9];
  } else {
    config.allow_insecure = true;
  }
  if (argc == 11) {
    config.tls.server_name = argv[10];
  }
  brainstem::Client client(config);

  const brainstem::Result ready = client.refresh();
  if (!ready.ok) {
    std::cerr << "cannot acquire control: " << ready.state.reason << " " << ready.error << '\n';
    return 1;
  }

  brainstem::Velocity velocity;
  std::chrono::milliseconds duration;
  try {
    velocity = {std::stod(argv[3]), std::stod(argv[4]), std::stod(argv[5])};
    duration = std::chrono::milliseconds(std::stoll(argv[6]));
  } catch (const std::exception &error) {
    std::cerr << "invalid motion argument: " << error.what() << '\n';
    return 64;
  }
  if (duration <= std::chrono::milliseconds::zero()) {
    std::cerr << "duration_ms must be positive\n";
    return 64;
  }

  const brainstem::Result moving = client.move(velocity);
  if (!moving.ok) {
    std::cerr << "motion rejected: " << moving.state.reason << " " << moving.error << '\n';
    return 1;
  }

  const auto deadline = std::chrono::steady_clock::now() + duration;
  brainstem::Result lease = moving;
  while (std::chrono::steady_clock::now() < deadline) {
    const auto remaining = std::chrono::duration_cast<std::chrono::milliseconds>(
        deadline - std::chrono::steady_clock::now());
    const auto lease_half = std::chrono::milliseconds(lease.state.lease_remaining_ms / 2);
    const auto wait = std::min(remaining, std::max(std::chrono::milliseconds(20), lease_half));
    std::this_thread::sleep_for(wait);
    if (std::chrono::steady_clock::now() >= deadline) {
      break;
    }
    lease = client.refresh();
    if (!lease.ok) {
      std::cerr << "control lease lost: " << lease.state.reason << " " << lease.error << '\n';
      const brainstem::Result stopped = client.stop();
      if (!stopped.confirmsStop()) {
        std::cerr << "stop after lease failure was not confirmed: " << stopped.state.reason << " "
                  << stopped.error << '\n';
      }
      return 2;
    }
  }

  const brainstem::Result stopped = client.stop();
  if (!stopped.confirmsStop()) {
    std::cerr << "stop was not confirmed: " << stopped.state.reason << " " << stopped.error << '\n';
    return 2;
  }
  return 0;
}
