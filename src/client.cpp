#include "brainstem/client.hpp"

#include <google/protobuf/empty.pb.h>
#include <grpcpp/grpcpp.h>

#include <chrono>
#include <cmath>
#include <cstdint>
#include <fstream>
#include <sstream>
#include <stdexcept>
#include <string>
#include <thread>
#include <utility>

#include "brainstem_api/cms.grpc.pb.h"

namespace brainstem {
namespace {

using ApiStatus = api::v1::ControlStatus;
using RejectReason = api::v1::CommandRejectReason;
using namespace std::chrono_literals;

constexpr auto kMinimumCheckedWalkInterval = 20ms;

bool isZero(const Velocity &velocity) noexcept {
  return velocity.vx_mps == 0.0 && velocity.vy_mps == 0.0 && velocity.yaw_rps == 0.0;
}

bool isFinite(const Velocity &velocity) noexcept {
  return std::isfinite(velocity.vx_mps) && std::isfinite(velocity.vy_mps) &&
         std::isfinite(velocity.yaw_rps);
}

std::string readPem(const std::string &path, const char *label) {
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

void validateConfig(const Config &config) {
  if (config.target.empty()) {
    throw std::invalid_argument("Brainstem target must not be empty");
  }
  if (config.client_id.empty()) {
    throw std::invalid_argument("Brainstem client_id must not be empty");
  }
  if (config.timeout <= std::chrono::milliseconds::zero()) {
    throw std::invalid_argument("Brainstem timeout must be positive");
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

std::shared_ptr<grpc::Channel> createChannel(const Config &config) {
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

std::string grpcError(const grpc::Status &status) {
  return std::to_string(static_cast<int>(status.error_code())) + ":" + status.error_message();
}

bool retryableWalkFailure(const grpc::Status &status) noexcept {
  return status.error_code() == grpc::StatusCode::UNAVAILABLE ||
         status.error_code() == grpc::StatusCode::DEADLINE_EXCEEDED;
}

Result rpcFailure(const grpc::Status &status, const char *rpc_name) {
  Result result;
  if (status.error_code() == grpc::StatusCode::UNIMPLEMENTED) {
    result.transport_ok = true;
    result.state.connected = true;
    result.state.reason = "protocol_incompatible";
    result.error = std::string("protocol_incompatible: required Brainstem RPC ") + rpc_name +
                   " is not implemented";
    return result;
  }

  switch (status.error_code()) {
    case grpc::StatusCode::INVALID_ARGUMENT:
    case grpc::StatusCode::FAILED_PRECONDITION:
    case grpc::StatusCode::UNAUTHENTICATED:
    case grpc::StatusCode::PERMISSION_DENIED:
    case grpc::StatusCode::RESOURCE_EXHAUSTED:
    case grpc::StatusCode::ABORTED:
      result.transport_ok = true;
      result.state.connected = true;
      result.state.reason = "request_rejected";
      break;
    default:
      result.state.reason = "transport_error";
      break;
  }
  result.error = std::string(rpc_name) + ": " + grpcError(status);
  return result;
}

std::string fsmName(const ApiStatus &status) {
  if (!status.has_fsm()) {
    return "unknown";
  }
  switch (status.fsm().kind()) {
    case api::v1::CMS_STATE_KIND_GROUNDED:
      return "grounded";
    case api::v1::CMS_STATE_KIND_STANDING:
      return "standing";
    case api::v1::CMS_STATE_KIND_WALKING:
      return "walking";
    case api::v1::CMS_STATE_KIND_TRANSITIONING:
      return "transitioning";
    case api::v1::CMS_STATE_KIND_ZERO:
    default:
      return "zero";
  }
}

std::string ownerName(api::v1::ControlOwner owner) {
  switch (owner) {
    case api::v1::CONTROL_OWNER_GRPC:
      return "grpc";
    case api::v1::CONTROL_OWNER_YUNZHUO:
      return "yunzhuo";
    case api::v1::CONTROL_OWNER_NONE:
    default:
      return "none";
  }
}

std::string reasonName(RejectReason reason) {
  std::string name = api::v1::CommandRejectReason_Name(reason);
  constexpr const char *prefix = "COMMAND_REJECT_REASON_";
  if (name.rfind(prefix, 0) == 0) {
    name.erase(0, std::char_traits<char>::length(prefix));
  }
  for (char &character : name) {
    if (character >= 'A' && character <= 'Z') {
      character = static_cast<char>(character - 'A' + 'a');
    }
  }
  return name.empty() ? "unknown" : name;
}

ControlState decodeStatus(const ApiStatus &status, RejectReason reason, const Config &config,
                          bool connected, bool has_lease_token, bool initial_zero_acknowledged) {
  ControlState state;
  const bool owned_by_client =
      status.owner() == api::v1::CONTROL_OWNER_GRPC && status.owner_id() == config.client_id;
  state.connected = connected;
  state.motors_enabled = status.motor_output_enabled();
  state.critical_fault = status.critical_motor_fault();
  state.lease_valid = has_lease_token && status.grpc_lease_active() && owned_by_client;
  state.initial_zero_acknowledged = initial_zero_acknowledged;
  state.lease_remaining_ms = status.lease_remaining_ms();
  state.accepted_sequence = status.last_accepted_sequence();
  state.fsm = fsmName(status);
  state.reason = reasonName(reason);
  state.control_owner = ownerName(status.owner());
  state.control_owner_id = status.owner_id();
  state.ready = connected && status.ready_for_walk() && state.lease_valid &&
                state.initial_zero_acknowledged && state.motors_enabled && !state.critical_fault &&
                (state.fsm == "standing" || state.fsm == "walking");

  if (!state.ready && state.reason == "none") {
    if (state.critical_fault) {
      state.reason = "motor_fault";
    } else if (!state.motors_enabled) {
      state.reason = "motors_disabled";
    } else if (!state.lease_valid) {
      if (!status.grpc_lease_active()) {
        state.reason = "lease_missing";
      } else if (owned_by_client && !has_lease_token) {
        state.reason = "lease_token_missing";
      } else {
        state.reason = "preempted";
      }
    } else if (!state.initial_zero_acknowledged) {
      state.reason = "initial_zero_required";
    } else if (state.fsm != "standing" && state.fsm != "walking") {
      state.reason = "fsm_not_ready";
    } else {
      state.reason = "not_ready";
    }
  }
  return state;
}

}  // namespace

struct Client::Impl {
  explicit Impl(Config value) : config(std::move(value)) {
    validateConfig(config);
    channel = createChannel(config);
    stub = api::v1::RobotControl::NewStub(channel);
  }

  Config config;
  std::shared_ptr<grpc::Channel> channel;
  std::unique_ptr<api::v1::RobotControl::Stub> stub;
  std::string lease_token;
  std::uint64_t sequence{0};
  bool sequence_uncertain{false};
  bool initial_zero_acknowledged{false};
  bool has_sent_velocity{false};
  std::chrono::steady_clock::time_point last_velocity_at{};
  ControlState last_state;

  void setDeadline(grpc::ClientContext &context) const {
    context.set_deadline(std::chrono::system_clock::now() + config.timeout);
  }

  ControlState remember(const ApiStatus &status, RejectReason reason, bool connected) {
    last_state = decodeStatus(status, reason, config, connected, !lease_token.empty(),
                              initial_zero_acknowledged);
    return last_state;
  }

  void clearLease() noexcept {
    lease_token.clear();
    sequence_uncertain = false;
    initial_zero_acknowledged = false;
    has_sent_velocity = false;
  }

  void requireSafetyZero() noexcept { initial_zero_acknowledged = false; }

  [[nodiscard]] bool statusConfirmsLease(const ApiStatus &status) const noexcept {
    return !lease_token.empty() && status.grpc_lease_active() &&
           status.owner() == api::v1::CONTROL_OWNER_GRPC && status.owner_id() == config.client_id;
  }

  Result rememberFailure(Result failure, bool preserve_lease_token) {
    ControlState state = last_state;
    state.connected = failure.state.connected;
    state.ready = false;
    state.lease_valid = false;
    state.initial_zero_acknowledged = false;
    state.reason = failure.state.reason;
    if (preserve_lease_token) {
      requireSafetyZero();
    } else {
      clearLease();
    }
    failure.state = state;
    last_state = state;
    return failure;
  }
};

Client::Client(Config config) : impl_(std::make_unique<Impl>(std::move(config))) {}

Client::~Client() {
  if (impl_) {
    (void)stop();
  }
}
Client::Client(Client &&) noexcept = default;
Client &Client::operator=(Client &&other) noexcept {
  if (this != &other) {
    if (impl_) {
      (void)stop();
    }
    impl_ = std::move(other.impl_);
  }
  return *this;
}

Result Client::connect() {
  google::protobuf::Empty request;
  ApiStatus response;
  grpc::ClientContext context;
  impl_->setDeadline(context);
  const grpc::Status status = impl_->stub->GetControlStatus(&context, request, &response);
  if (!status.ok()) {
    Result failure = rpcFailure(status, "GetControlStatus");
    const bool preserve_token = !failure.transport_ok && !impl_->lease_token.empty();
    return impl_->rememberFailure(std::move(failure), preserve_token);
  }

  if (!impl_->statusConfirmsLease(response)) {
    impl_->clearLease();
  } else {
    impl_->sequence = response.last_accepted_sequence();
    impl_->sequence_uncertain = false;
  }
  const ControlState state = impl_->remember(response, response.last_decision(), true);
  return {true, true, false, false, state, {}};
}

Result Client::refresh() {
  api::v1::ControlLease lease;
  grpc::Status status;
  const bool acquiring = impl_->lease_token.empty();
  if (acquiring) {
    api::v1::AcquireControlRequest request;
    request.set_client_id(impl_->config.client_id);
    grpc::ClientContext context;
    impl_->setDeadline(context);
    status = impl_->stub->AcquireControl(&context, request, &lease);
  } else {
    api::v1::ControlLeaseRequest request;
    request.set_token(impl_->lease_token);
    grpc::ClientContext context;
    impl_->setDeadline(context);
    status = impl_->stub->RenewControlLease(&context, request, &lease);
  }

  if (!status.ok()) {
    Result failure = rpcFailure(status, acquiring ? "AcquireControl" : "RenewControlLease");
    const bool preserve_token = !acquiring && !failure.transport_ok && !impl_->lease_token.empty();
    return impl_->rememberFailure(std::move(failure), preserve_token);
  }

  if (!lease.accepted()) {
    const bool preserve_token = !acquiring && impl_->statusConfirmsLease(lease.status());
    if (preserve_token) {
      impl_->requireSafetyZero();
    } else {
      impl_->clearLease();
    }
    ControlState state = impl_->remember(lease.status(), lease.reason(), true);
    state.ready = false;
    if (!preserve_token) {
      state.lease_valid = false;
      state.initial_zero_acknowledged = false;
    }
    impl_->last_state = state;
    return {false, true, false, false, state, state.reason};
  }
  if (lease.token().empty()) {
    impl_->clearLease();
    ControlState state = impl_->remember(lease.status(), lease.reason(), true);
    state.ready = false;
    state.lease_valid = false;
    state.initial_zero_acknowledged = false;
    state.reason = "lease_token_missing";
    impl_->last_state = state;
    return {false, true, false, false, state, state.reason};
  }

  if (acquiring) {
    impl_->initial_zero_acknowledged = false;
    impl_->has_sent_velocity = false;
  }
  impl_->lease_token = lease.token();
  impl_->sequence = lease.status().last_accepted_sequence();
  impl_->sequence_uncertain = false;
  ControlState state = impl_->remember(lease.status(), lease.reason(), true);
  if (!impl_->initial_zero_acknowledged) {
    return move(Velocity{});
  }

  state.ready = state.ready && impl_->initial_zero_acknowledged;
  impl_->last_state = state;
  return {state.ready, true, true, false, state, state.ready ? std::string{} : state.reason};
}

Result Client::move(const Velocity &velocity) {
  if (!isFinite(velocity)) {
    ControlState state = impl_->last_state;
    state.ready = false;
    state.reason = "invalid_velocity";
    impl_->last_state = state;
    return {false, false, false, false, state, state.reason};
  }
  if (impl_->lease_token.empty()) {
    ControlState state = impl_->last_state;
    state.ready = false;
    state.lease_valid = false;
    state.initial_zero_acknowledged = false;
    state.reason = "lease_missing";
    impl_->last_state = state;
    return {false, false, false, false, state, state.reason};
  }
  if (!impl_->initial_zero_acknowledged && !isZero(velocity)) {
    ControlState state = impl_->last_state;
    state.connected = true;
    state.ready = false;
    state.lease_valid = true;
    state.initial_zero_acknowledged = false;
    state.reason = "initial_zero_required";
    impl_->last_state = state;
    return {false, false, false, false, state, state.reason};
  }

  if (impl_->has_sent_velocity) {
    const auto earliest = impl_->last_velocity_at + kMinimumCheckedWalkInterval;
    if (std::chrono::steady_clock::now() < earliest) {
      std::this_thread::sleep_until(earliest);
    }
  }
  impl_->last_velocity_at = std::chrono::steady_clock::now();
  impl_->has_sent_velocity = true;

  api::v1::WalkRequest request;
  request.set_token(impl_->lease_token);
  request.set_sequence(++impl_->sequence);
  request.mutable_direction()->set_x(velocity.vx_mps);
  request.mutable_direction()->set_y(velocity.vy_mps);
  request.mutable_direction()->set_z(velocity.yaw_rps);

  api::v1::CommandAck response;
  grpc::Status status;
  for (int attempt = 0; attempt < 2; ++attempt) {
    response.Clear();
    grpc::ClientContext context;
    impl_->setDeadline(context);
    status = impl_->stub->WalkChecked(&context, request, &response);
    if (status.ok() || !retryableWalkFailure(status)) {
      break;
    }
  }
  if (!status.ok()) {
    Result failure = rpcFailure(status, "WalkChecked");
    const bool preserve_token = !failure.transport_ok && !impl_->lease_token.empty();
    if (preserve_token) {
      impl_->sequence_uncertain = true;
    }
    return impl_->rememberFailure(std::move(failure), preserve_token);
  }

  const bool ack_matches =
      response.sequence() == request.sequence() &&
      (!response.accepted() || response.status().last_accepted_sequence() == request.sequence());
  if (!ack_matches) {
    const bool preserve_token = impl_->statusConfirmsLease(response.status());
    if (preserve_token) {
      impl_->sequence = response.status().last_accepted_sequence();
      impl_->sequence_uncertain = false;
      impl_->requireSafetyZero();
    } else {
      impl_->clearLease();
    }
    ControlState state = impl_->remember(response.status(), response.reason(), true);
    state.ready = false;
    if (!preserve_token) {
      state.lease_valid = false;
      state.initial_zero_acknowledged = false;
    }
    state.reason = "ack_sequence_mismatch";
    impl_->last_state = state;
    return {false, true, false, false, state, state.reason};
  }

  if (response.accepted() && !impl_->initial_zero_acknowledged && isZero(velocity)) {
    impl_->initial_zero_acknowledged = true;
  }
  ControlState state = impl_->remember(response.status(), response.reason(), true);
  if (!response.accepted()) {
    const bool preserve_token = impl_->statusConfirmsLease(response.status());
    if (preserve_token) {
      impl_->sequence = response.status().last_accepted_sequence();
      impl_->sequence_uncertain = false;
      impl_->requireSafetyZero();
    } else {
      impl_->clearLease();
    }
    state = impl_->remember(response.status(), response.reason(), true);
    state.ready = false;
    if (!preserve_token) {
      state.lease_valid = false;
      state.initial_zero_acknowledged = false;
    }
    impl_->last_state = state;
    return {false, true, false, false, state, state.reason};
  }

  impl_->last_state = state;
  impl_->sequence_uncertain = false;
  return {state.ready, true, true, false, state, state.ready ? std::string{} : state.reason};
}

Result Client::stop() noexcept {
  ControlState unavailable;
  unavailable.reason = "lease_missing";
  if (!impl_ || impl_->lease_token.empty()) {
    return {false, false, false, false, unavailable, unavailable.reason};
  }

  try {
    if (impl_->sequence_uncertain) {
      Result synchronized = connect();
      if (!synchronized.ok || impl_->lease_token.empty()) {
        synchronized.ok = false;
        synchronized.accepted = false;
        synchronized.stop_confirmed = false;
        synchronized.state.ready = false;
        if (synchronized.state.reason.empty() || synchronized.state.reason == "none") {
          synchronized.state.reason = "stop_sequence_unresolved";
        }
        if (synchronized.error.empty()) {
          synchronized.error = synchronized.state.reason;
        }
        impl_->last_state = synchronized.state;
        return synchronized;
      }
    }

    Result zero = move(Velocity{});
    if (!zero.transport_ok || !zero.accepted) {
      zero.ok = false;
      zero.stop_confirmed = false;
      zero.state.ready = false;
      if (zero.state.reason.empty()) {
        zero.state.reason = "stop_unconfirmed";
      }
      if (zero.error.empty()) {
        zero.error = zero.state.reason;
      }
      impl_->last_state = zero.state;
      return zero;
    }

    api::v1::ControlLeaseRequest request;
    request.set_token(impl_->lease_token);
    ApiStatus response;
    grpc::ClientContext context;
    impl_->setDeadline(context);
    const grpc::Status status = impl_->stub->ReleaseControl(&context, request, &response);
    if (!status.ok()) {
      Result failure = rpcFailure(status, "ReleaseControl");
      const bool preserve_token = !failure.transport_ok;
      if (preserve_token) {
        impl_->requireSafetyZero();
      } else {
        impl_->clearLease();
      }
      ControlState state = zero.state;
      state.connected = failure.state.connected;
      state.ready = false;
      state.lease_valid = false;
      state.initial_zero_acknowledged = false;
      state.reason = "release_transport_error";
      impl_->last_state = state;
      return {false, failure.transport_ok, false, false, state, failure.error};
    }

    const bool released =
        !response.grpc_lease_active() && response.owner_id() != impl_->config.client_id;
    const bool preserve_token = !released && impl_->statusConfirmsLease(response);
    if (preserve_token) {
      impl_->requireSafetyZero();
    } else {
      impl_->clearLease();
    }
    ControlState state = impl_->remember(response, response.last_decision(), true);
    state.ready = false;
    if (!preserve_token) {
      state.lease_valid = false;
      state.initial_zero_acknowledged = false;
    }
    state.reason = released ? "stop_confirmed" : "release_unconfirmed";
    impl_->last_state = state;
    return {released, true, released, released, state, released ? std::string{} : state.reason};
  } catch (const std::exception &error) {
    impl_->requireSafetyZero();
    ControlState state = impl_->last_state;
    state.ready = false;
    state.lease_valid = false;
    state.initial_zero_acknowledged = false;
    state.reason = "stop_exception";
    impl_->last_state = state;
    return {false, false, false, false, state, error.what()};
  } catch (...) {
    impl_->requireSafetyZero();
    ControlState state = impl_->last_state;
    state.ready = false;
    state.lease_valid = false;
    state.initial_zero_acknowledged = false;
    state.reason = "stop_exception";
    impl_->last_state = state;
    return {false, false, false, false, state, state.reason};
  }
}

Result Client::standUp() { return bodyAction(true); }

Result Client::sitDown() { return bodyAction(false); }

Result Client::bodyAction(bool stand) {
  if (!impl_->lease_token.empty()) {
    Result stopped = stop();
    if (!stopped.confirmsStop()) {
      if (stopped.error.empty()) {
        stopped.error = "body_action_stop_unconfirmed";
      }
      return stopped;
    }
  }

  google::protobuf::Empty request;
  google::protobuf::Empty response;
  grpc::ClientContext context;
  impl_->setDeadline(context);
  const char *rpc_name = stand ? "StandUp" : "SitDown";
  const grpc::Status status = stand ? impl_->stub->StandUp(&context, request, &response)
                                    : impl_->stub->SitDown(&context, request, &response);
  if (!status.ok()) {
    Result failure = rpcFailure(status, rpc_name);
    if (failure.transport_ok) {
      failure.state = impl_->last_state;
      failure.state.connected = true;
      failure.state.ready = false;
      failure.state.reason = "body_action_rejected";
    }
    impl_->last_state = failure.state;
    return failure;
  }

  ControlState state = impl_->last_state;
  state.connected = true;
  state.ready = false;
  state.lease_valid = false;
  state.initial_zero_acknowledged = false;
  const bool already_in_target =
      (stand && state.fsm == "standing") || (!stand && state.fsm == "grounded");
  state.fsm = already_in_target ? (stand ? "standing" : "grounded") : "transitioning";
  state.reason = std::string("body_action_accepted:") + (stand ? "stand_up" : "sit_down");
  impl_->last_state = state;
  return {true, true, true, false, state, {}};
}

ControlState Client::state() const { return impl_ ? impl_->last_state : ControlState{}; }

}  // namespace brainstem
