#include <brainstem/client.hpp>
#include <cstdint>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

namespace {

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

std::vector<std::uint32_t> parseJointIds(const std::string &command) {
  constexpr const char *prefix = "clear-faults:";
  if (command == "clear-faults") {
    return {};
  }
  if (command.rfind(prefix, 0) != 0) {
    throw std::invalid_argument("not a clear-faults command");
  }

  std::vector<std::uint32_t> joint_ids;
  std::stringstream values(command.substr(std::char_traits<char>::length(prefix)));
  std::string value;
  while (std::getline(values, value, ',')) {
    std::size_t consumed = 0;
    const unsigned long joint_id = std::stoul(value, &consumed);
    if (value.empty() || consumed != value.size()) {
      throw std::invalid_argument("joint IDs must be comma-separated integers");
    }
    joint_ids.push_back(static_cast<std::uint32_t>(joint_id));
  }
  if (joint_ids.empty()) {
    throw std::invalid_argument("clear-faults: requires at least one joint ID");
  }
  return joint_ids;
}

}  // namespace

int main(int argc, char **argv) {
  if (argc < 4) {
    std::cerr << "usage: " << argv[0]
              << " <host:port> <client_id>"
                 " <enable|disable|set-zero|clear-faults|clear-faults:2,15|stand-up|sit-down>"
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

  brainstem::Client client(config);
  const brainstem::Result connected = client.connect();
  if (!connected.ok) {
    std::cerr << "cannot connect: " << connected.state.reason << " " << connected.error << '\n';
    return 1;
  }

  const std::string command = argv[3];
  brainstem::Result result;
  try {
    if (command == "enable") {
      result = client.enableMotors();
    } else if (command == "disable") {
      result = client.disableMotors();
    } else if (command == "set-zero") {
      result = client.setJointZero();
    } else if (command == "stand-up") {
      result = client.standUp();
    } else if (command == "sit-down") {
      result = client.sitDown();
    } else if (command == "clear-faults" || command.rfind("clear-faults:", 0) == 0) {
      result = client.clearMotorFaults(parseJointIds(command));
    } else {
      std::cerr << "unknown command: " << command << '\n';
      return 64;
    }
  } catch (const std::exception &error) {
    std::cerr << "invalid command: " << error.what() << '\n';
    return 64;
  }

  if (!result.ok) {
    std::cerr << "command failed: " << result.state.reason << " " << result.error << '\n';
    return 1;
  }
  std::cout << "accepted=" << result.accepted << " motors_enabled=" << result.state.motors_enabled
            << " lease_valid=" << result.state.lease_valid << " state=" << result.state.fsm
            << " reason=" << result.state.reason << '\n';
  return result.accepted ? 0 : 1;
}
