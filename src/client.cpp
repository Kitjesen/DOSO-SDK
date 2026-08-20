#include "brainstem/client.hpp"

#include <google/protobuf/empty.pb.h>
#include <grpcpp/grpcpp.h>

#include <atomic>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <cstdint>
#include <fstream>
#include <functional>
#include <mutex>
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

Result withCachedState(Result failure, const ControlState &cached_state) {
  const bool connected = failure.state.connected;
  std::string reason = std::move(failure.state.reason);
  failure.state = cached_state;
  failure.state.connected = connected;
  failure.state.ready = false;
  failure.state.reason = std::move(reason);
  return failure;
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

CmsState decodeCmsState(const api::v1::CmsState &source) {
  CmsState state;
  switch (source.kind()) {
    case api::v1::CMS_STATE_KIND_ZERO:
      state.kind = CmsStateKind::Zero;
      break;
    case api::v1::CMS_STATE_KIND_GROUNDED:
      state.kind = CmsStateKind::Grounded;
      break;
    case api::v1::CMS_STATE_KIND_STANDING:
      state.kind = CmsStateKind::Standing;
      break;
    case api::v1::CMS_STATE_KIND_WALKING:
      state.kind = CmsStateKind::Walking;
      break;
    case api::v1::CMS_STATE_KIND_TRANSITIONING:
      state.kind = CmsStateKind::Transitioning;
      break;
    default:
      state.kind = CmsStateKind::Unknown;
      break;
  }
  switch (source.transition()) {
    case api::v1::CMS_TRANSITION_KIND_STAND_UP:
      state.transition = CmsTransition::StandUp;
      break;
    case api::v1::CMS_TRANSITION_KIND_SIT_DOWN:
      state.transition = CmsTransition::SitDown;
      break;
    case api::v1::CMS_TRANSITION_KIND_GESTURE:
      state.transition = CmsTransition::Gesture;
      break;
    case api::v1::CMS_TRANSITION_KIND_NONE:
    default:
      state.transition = CmsTransition::None;
      break;
  }
  state.gesture_name = source.gesture_name();
  return state;
}

MotorStatus decodeMotorStatus(const api::v1::MotorStatusResponse &source) {
  MotorStatus status;
  status.motors.reserve(static_cast<std::size_t>(source.motors_size()));
  for (const auto &source_motor : source.motors()) {
    MotorState motor;
    motor.id = source_motor.id();
    motor.online = source_motor.online();
    motor.status_code = source_motor.status_code();
    motor.temperature_c = source_motor.temperature();
    motor.voltage_v = source_motor.voltage();
    motor.position_rad = source_motor.position();
    motor.velocity_rps = source_motor.velocity();
    motor.torque_nm = source_motor.torque();
    motor.errors.assign(source_motor.errors().begin(), source_motor.errors().end());
    status.motors.push_back(std::move(motor));
  }
  return status;
}

std::chrono::nanoseconds decodeDuration(const google::protobuf::Duration &source) {
  return std::chrono::seconds(source.seconds()) + std::chrono::nanoseconds(source.nanos());
}

ImuSample decodeImu(const api::v1::Imu &source) {
  ImuSample sample;
  sample.angular_velocity_rps = {source.gyroscope().x(), source.gyroscope().y(),
                                 source.gyroscope().z()};
  sample.orientation = {source.quaternion().w(), source.quaternion().x(), source.quaternion().y(),
                        source.quaternion().z()};
  sample.elapsed = decodeDuration(source.timestamp());
  return sample;
}

JointSample decodeJoint(const api::v1::Joint &source) {
  JointSample sample;
  sample.elapsed = decodeDuration(source.timestamp());
  if (source.has_single_joint()) {
    const api::v1::SingleJoint &source_joint = source.single_joint();
    sample.joints.push_back({source_joint.id(), source_joint.position(), source_joint.velocity(),
                             source_joint.torque(), source_joint.status()});
    return sample;
  }
  if (!source.has_all_joints()) {
    throw std::runtime_error("ListenJoint: sample contains no joint data");
  }

  const api::v1::AllJoints &source_joints = source.all_joints();
  const int count = source_joints.position().values_size();
  if (count != 16 || source_joints.velocity().values_size() != count ||
      source_joints.torque().values_size() != count ||
      source_joints.status().values_size() != count) {
    throw std::runtime_error("ListenJoint: all-joints sample must contain 16 aligned values");
  }
  sample.joints.reserve(static_cast<std::size_t>(count));
  for (int index = 0; index < count; ++index) {
    sample.joints.push_back(
        {static_cast<std::uint32_t>(index), source_joints.position().values(index),
         source_joints.velocity().values(index), source_joints.torque().values(index),
         source_joints.status().values(index)});
  }
  return sample;
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
  state.accepted_count = status.accepted_count();
  state.rejected_count = status.rejected_count();
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
  mutable std::mutex telemetry_lifecycle_mutex;
  mutable std::mutex telemetry_mutex;
  std::condition_variable telemetry_changed;
  std::unique_ptr<Subscription> imu_subscription;
  ImuSample latest_imu;
  std::chrono::steady_clock::time_point imu_received_at{};
  std::uint64_t imu_sequence{0};
  bool imu_available{false};
  std::unique_ptr<Subscription> joint_subscription;
  std::array<JointState, kJointCount> latest_joints{};
  std::array<std::chrono::nanoseconds, kJointCount> joint_elapsed{};
  std::array<std::chrono::steady_clock::time_point, kJointCount> joint_received_at{};
  std::uint16_t joint_valid_mask{0};
  std::uint64_t joint_sequence{0};
  std::unique_ptr<Subscription> cms_subscription;
  CmsState latest_cms;
  std::uint64_t cms_sequence{0};
  bool cms_available{false};

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

struct Subscription::Impl {
  explicit Impl(std::function<Result(Impl &)> operation) {
    worker = std::thread([this, operation = std::move(operation)]() mutable {
      try {
        result = operation(*this);
      } catch (const std::exception &error) {
        result.state.reason = "subscription_error";
        result.error = error.what();
      } catch (...) {
        result.state.reason = "subscription_error";
        result.error = result.state.reason;
      }
      clearContext();
      is_active.store(false);
    });
  }

  ~Impl() {
    cancel();
    if (worker.joinable()) {
      worker.join();
    }
  }

  grpc::ClientContext *beginContext() {
    std::lock_guard<std::mutex> lock(context_mutex);
    context = std::make_unique<grpc::ClientContext>();
    if (cancel_requested.load()) {
      context->TryCancel();
    }
    return context.get();
  }

  void clearContext() noexcept {
    std::lock_guard<std::mutex> lock(context_mutex);
    context.reset();
  }

  void cancel() noexcept {
    cancel_requested.store(true);
    std::lock_guard<std::mutex> lock(context_mutex);
    if (context) {
      context->TryCancel();
    }
  }

  Result wait() {
    if (worker.joinable()) {
      worker.join();
    }
    return result;
  }

  template <typename WireSample, typename Handler, typename Start, typename Decode>
  Result runStream(const std::shared_ptr<grpc::Channel> &channel, const ControlState &state,
                   Handler &handler, Start start, Decode decode, const char *rpc_name) {
    const auto stub = api::v1::RobotControl::NewStub(channel);
    google::protobuf::Empty request;
    grpc::ClientContext *stream_context = beginContext();
    std::unique_ptr<grpc::ClientReader<WireSample>> reader = start(*stub, stream_context, request);
    WireSample wire_sample;
    while (reader->Read(&wire_sample)) {
      const auto sample = decode(wire_sample);
      try {
        handler(sample);
      } catch (...) {
        cancel();
        throw;
      }
    }
    const grpc::Status status = reader->Finish();
    if (status.ok() || cancel_requested.load()) {
      return {true, true, false, false, state, {}};
    }
    return withCachedState(rpcFailure(status, rpc_name), state);
  }

  std::atomic_bool is_active{true};
  std::atomic_bool cancel_requested{false};
  std::mutex context_mutex;
  std::unique_ptr<grpc::ClientContext> context;
  Result result;
  std::thread worker;
};

Subscription::Subscription(std::unique_ptr<Impl> impl) : impl_(std::move(impl)) {}

Subscription::~Subscription() = default;
Subscription::Subscription(Subscription &&) noexcept = default;
Subscription &Subscription::operator=(Subscription &&) noexcept = default;

void Subscription::cancel() noexcept {
  if (impl_) {
    impl_->cancel();
  }
}

bool Subscription::active() const noexcept { return impl_ && impl_->is_active.load(); }

Result Subscription::wait() {
  if (!impl_) {
    Result result;
    result.state.reason = "subscription_missing";
    result.error = result.state.reason;
    return result;
  }
  return impl_->wait();
}

Client::Client(Config config) : impl_(std::make_unique<Impl>(std::move(config))) {}

Client::~Client() {
  if (impl_) {
    stopTelemetry();
    (void)stop();
  }
}
Client::Client(Client &&) noexcept = default;
Client &Client::operator=(Client &&other) noexcept {
  if (this != &other) {
    if (impl_) {
      stopTelemetry();
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

Response<CmsState> Client::getCmsState() {
  google::protobuf::Empty request;
  api::v1::CmsState response;
  grpc::ClientContext context;
  impl_->setDeadline(context);
  const grpc::Status status = impl_->stub->GetCmsState(&context, request, &response);
  if (!status.ok()) {
    return {withCachedState(rpcFailure(status, "GetCmsState"), impl_->last_state), {}};
  }
  return {{true, true, false, false, impl_->last_state, {}}, decodeCmsState(response)};
}

Response<MotorStatus> Client::getMotorStatus() {
  google::protobuf::Empty request;
  api::v1::MotorStatusResponse response;
  grpc::ClientContext context;
  impl_->setDeadline(context);
  const grpc::Status status = impl_->stub->GetMotorStatus(&context, request, &response);
  if (!status.ok()) {
    return {withCachedState(rpcFailure(status, "GetMotorStatus"), impl_->last_state), {}};
  }
  return {{true, true, false, false, impl_->last_state, {}}, decodeMotorStatus(response)};
}

Response<Voltage> Client::getVoltage() {
  google::protobuf::Empty request;
  api::v1::Voltage response;
  grpc::ClientContext context;
  impl_->setDeadline(context);
  const grpc::Status status = impl_->stub->GetVoltage(&context, request, &response);
  if (!status.ok()) {
    return {withCachedState(rpcFailure(status, "GetVoltage"), impl_->last_state), {}};
  }
  Voltage voltage;
  voltage.values_v.assign(response.values().begin(), response.values().end());
  return {{true, true, false, false, impl_->last_state, {}}, std::move(voltage)};
}

Result Client::startTelemetry(TelemetryOptions options) {
  if (!impl_) {
    Result failure;
    failure.state.reason = "client_moved";
    failure.error = failure.state.reason;
    return failure;
  }

  std::lock_guard<std::mutex> lifecycle_lock(impl_->telemetry_lifecycle_mutex);
  if (!options.imu) {
    impl_->imu_subscription.reset();
    std::lock_guard<std::mutex> data_lock(impl_->telemetry_mutex);
    impl_->imu_available = false;
    impl_->imu_sequence = 0;
  } else if (!impl_->imu_subscription || !impl_->imu_subscription->active()) {
    impl_->imu_subscription.reset();
    {
      std::lock_guard<std::mutex> data_lock(impl_->telemetry_mutex);
      impl_->imu_available = false;
      impl_->imu_sequence = 0;
    }
    Impl *const state = impl_.get();
    impl_->imu_subscription = subscribeImu([state](const ImuSample &sample) {
      {
        std::lock_guard<std::mutex> lock(state->telemetry_mutex);
        state->latest_imu = sample;
        state->imu_received_at = std::chrono::steady_clock::now();
        state->imu_available = true;
        ++state->imu_sequence;
      }
      state->telemetry_changed.notify_all();
    });
  }

  if (!options.joints) {
    impl_->joint_subscription.reset();
    std::lock_guard<std::mutex> data_lock(impl_->telemetry_mutex);
    impl_->joint_valid_mask = 0;
    impl_->joint_sequence = 0;
  } else if (!impl_->joint_subscription || !impl_->joint_subscription->active()) {
    impl_->joint_subscription.reset();
    {
      std::lock_guard<std::mutex> data_lock(impl_->telemetry_mutex);
      impl_->joint_valid_mask = 0;
      impl_->joint_sequence = 0;
    }
    Impl *const state = impl_.get();
    impl_->joint_subscription = subscribeJoints([state](const JointSample &sample) {
      const auto received_at = std::chrono::steady_clock::now();
      {
        std::lock_guard<std::mutex> lock(state->telemetry_mutex);
        for (const JointState &joint : sample.joints) {
          if (joint.id >= kJointCount) {
            throw std::runtime_error("joint telemetry id is out of range");
          }
          state->latest_joints[joint.id] = joint;
          state->joint_elapsed[joint.id] = sample.elapsed;
          state->joint_received_at[joint.id] = received_at;
          state->joint_valid_mask |= static_cast<std::uint16_t>(1U << joint.id);
        }
        ++state->joint_sequence;
      }
      state->telemetry_changed.notify_all();
    });
  }

  if (!options.cms) {
    impl_->cms_subscription.reset();
    std::lock_guard<std::mutex> data_lock(impl_->telemetry_mutex);
    impl_->cms_available = false;
    impl_->cms_sequence = 0;
  } else if (!impl_->cms_subscription || !impl_->cms_subscription->active()) {
    impl_->cms_subscription.reset();
    {
      std::lock_guard<std::mutex> data_lock(impl_->telemetry_mutex);
      impl_->cms_available = false;
      impl_->cms_sequence = 0;
    }
    Impl *const state = impl_.get();
    impl_->cms_subscription = subscribeCmsState([state](const CmsState &cms) {
      {
        std::lock_guard<std::mutex> lock(state->telemetry_mutex);
        state->latest_cms = cms;
        state->cms_available = true;
        ++state->cms_sequence;
      }
      state->telemetry_changed.notify_all();
    });
  }

  std::unique_lock<std::mutex> data_lock(impl_->telemetry_mutex);
  const bool ready = impl_->telemetry_changed.wait_for(data_lock, impl_->config.timeout, [&] {
    return (!options.imu || impl_->imu_available) &&
           (!options.joints || impl_->joint_valid_mask != 0) &&
           (!options.cms || impl_->cms_available);
  });
  ControlState state = impl_->last_state;
  if (!ready) {
    state.ready = false;
    state.reason = "telemetry_timeout";
    return {false, false, false, false, state, state.reason};
  }
  return {true, true, false, false, state, {}};
}

void Client::stopTelemetry() noexcept {
  if (!impl_) {
    return;
  }
  std::lock_guard<std::mutex> lifecycle_lock(impl_->telemetry_lifecycle_mutex);
  impl_->imu_subscription.reset();
  impl_->joint_subscription.reset();
  impl_->cms_subscription.reset();
  std::lock_guard<std::mutex> data_lock(impl_->telemetry_mutex);
  impl_->imu_available = false;
  impl_->imu_sequence = 0;
  impl_->joint_valid_mask = 0;
  impl_->joint_sequence = 0;
  impl_->cms_available = false;
  impl_->cms_sequence = 0;
}

TelemetrySnapshot Client::telemetry() const {
  TelemetrySnapshot snapshot;
  if (!impl_) {
    return snapshot;
  }
  std::lock_guard<std::mutex> lifecycle_lock(impl_->telemetry_lifecycle_mutex);
  std::lock_guard<std::mutex> data_lock(impl_->telemetry_mutex);
  snapshot.imu.active = impl_->imu_subscription && impl_->imu_subscription->active();
  snapshot.imu.available = impl_->imu_available;
  snapshot.imu.sequence = impl_->imu_sequence;
  snapshot.imu.value = impl_->latest_imu;
  if (impl_->imu_available) {
    const auto age = std::chrono::steady_clock::now() - impl_->imu_received_at;
    snapshot.imu.age = std::chrono::duration_cast<std::chrono::milliseconds>(age);
    snapshot.imu.fresh = age <= impl_->config.telemetry_stale_after;
  }
  snapshot.joints.active = impl_->joint_subscription && impl_->joint_subscription->active();
  snapshot.joints.valid_mask = impl_->joint_valid_mask;
  snapshot.joints.sequence = impl_->joint_sequence;
  snapshot.joints.joints = impl_->latest_joints;
  snapshot.joints.elapsed = impl_->joint_elapsed;
  const auto now = std::chrono::steady_clock::now();
  for (std::size_t id = 0; id < kJointCount; ++id) {
    const std::uint16_t bit = static_cast<std::uint16_t>(1U << id);
    if ((impl_->joint_valid_mask & bit) != 0 &&
        now - impl_->joint_received_at[id] <= impl_->config.telemetry_stale_after) {
      snapshot.joints.fresh_mask |= bit;
    }
  }
  snapshot.cms.active = impl_->cms_subscription && impl_->cms_subscription->active();
  snapshot.cms.available = impl_->cms_available;
  snapshot.cms.sequence = impl_->cms_sequence;
  snapshot.cms.value = impl_->latest_cms;
  return snapshot;
}

std::unique_ptr<Subscription> Client::subscribeImu(std::function<void(const ImuSample &)> handler) {
  if (!handler) {
    throw std::invalid_argument("IMU subscription handler must not be empty");
  }

  const std::shared_ptr<grpc::Channel> channel = impl_->channel;
  const ControlState state = impl_->last_state;
  auto operation = [channel, state,
                    handler = std::move(handler)](Subscription::Impl &subscription) mutable {
    return subscription.runStream<api::v1::Imu>(
        channel, state, handler,
        [](api::v1::RobotControl::Stub &stub, grpc::ClientContext *context,
           const google::protobuf::Empty &request) { return stub.ListenImu(context, request); },
        decodeImu, "ListenImu");
  };

  return std::unique_ptr<Subscription>(
      new Subscription(std::make_unique<Subscription::Impl>(std::move(operation))));
}

std::unique_ptr<Subscription> Client::subscribeJoints(
    std::function<void(const JointSample &)> handler) {
  if (!handler) {
    throw std::invalid_argument("joint subscription handler must not be empty");
  }

  const std::shared_ptr<grpc::Channel> channel = impl_->channel;
  const ControlState state = impl_->last_state;
  auto operation = [channel, state,
                    handler = std::move(handler)](Subscription::Impl &subscription) mutable {
    return subscription.runStream<api::v1::Joint>(
        channel, state, handler,
        [](api::v1::RobotControl::Stub &stub, grpc::ClientContext *context,
           const google::protobuf::Empty &request) { return stub.ListenJoint(context, request); },
        decodeJoint, "ListenJoint");
  };

  return std::unique_ptr<Subscription>(
      new Subscription(std::make_unique<Subscription::Impl>(std::move(operation))));
}

std::unique_ptr<Subscription> Client::subscribeCmsState(
    std::function<void(const CmsState &)> handler) {
  if (!handler) {
    throw std::invalid_argument("CMS state subscription handler must not be empty");
  }

  const std::shared_ptr<grpc::Channel> channel = impl_->channel;
  const ControlState state = impl_->last_state;
  auto operation = [channel, state,
                    handler = std::move(handler)](Subscription::Impl &subscription) mutable {
    return subscription.runStream<api::v1::CmsState>(
        channel, state, handler,
        [](api::v1::RobotControl::Stub &stub, grpc::ClientContext *context,
           const google::protobuf::Empty &request) {
          return stub.ListenCmsState(context, request);
        },
        decodeCmsState, "ListenCmsState");
  };

  return std::unique_ptr<Subscription>(
      new Subscription(std::make_unique<Subscription::Impl>(std::move(operation))));
}

ControlState Client::state() const { return impl_ ? impl_->last_state : ControlState{}; }

}  // namespace brainstem
