#pragma once

#include "brainstem/types.hpp"
#include <grpcpp/grpcpp.h>
#include <fstream>
#include <sstream>
#include <stdexcept>

namespace brainstem::detail {

inline std::string readPem(const std::string &path, const char *label) {
  std::ifstream input(path, std::ios::binary);
  if (!input) {
    throw std::runtime_error(std::string("cannot read Brainstem ") + label + " PEM: " + path);
  }
  std::ostringstream contents;
  contents << input.rdbuf();
  if (contents.str().empty()) {
    throw std::runtime_error(std::string("Brainstem ") + label + " PEM is empty: " + path);
  }
  return contents.str();
}

inline void validateConfig(const Config &config) {
  if (config.target.empty()) {
    throw std::invalid_argument("Brainstem target must not be empty");
  }
  if (config.client_id.empty()) {
    throw std::invalid_argument("Brainstem client_id must not be empty");
  }
  if (config.timeout <= std::chrono::milliseconds::zero()) {
    throw std::invalid_argument("Brainstem timeout must be positive");
  }
  if (config.telemetry_stale_after <= std::chrono::milliseconds::zero()) {
    throw std::invalid_argument("Brainstem telemetry_stale_after must be positive");
  }
  if (!config.tls.server_name.empty() && !config.tls.enabled()) {
    throw std::invalid_argument("Brainstem TLS server_name requires mTLS certificate files");
  }
  if (config.tls.enabled() && (config.tls.ca_file.empty() || config.tls.certificate_file.empty() ||
                               config.tls.private_key_file.empty())) {
    throw std::invalid_argument(
        "Brainstem mTLS requires ca_file, certificate_file, and private_key_file");
  }
  if (!config.tls.enabled() && !config.allow_insecure) {
    throw std::invalid_argument(
        "Brainstem mTLS is required unless allow_insecure is explicitly enabled");
  }
}

inline std::shared_ptr<grpc::Channel> createChannel(const Config &config) {
  if (!config.tls.enabled()) {
    return grpc::CreateChannel(config.target, grpc::InsecureChannelCredentials());
  }

  grpc::SslCredentialsOptions options;
  options.pem_root_certs = readPem(config.tls.ca_file, "CA");
  options.pem_cert_chain = readPem(config.tls.certificate_file, "client certificate");
  options.pem_private_key = readPem(config.tls.private_key_file, "client private key");
  const auto credentials = grpc::SslCredentials(options);
  if (!credentials) {
    throw std::runtime_error("failed to create Brainstem mTLS credentials");
  }
  if (config.tls.server_name.empty()) {
    return grpc::CreateChannel(config.target, credentials);
  }
  grpc::ChannelArguments arguments;
  arguments.SetSslTargetNameOverride(config.tls.server_name);
  return grpc::CreateCustomChannel(config.target, credentials, arguments);
}

}  // namespace brainstem::detail
