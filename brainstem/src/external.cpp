#include "brainstem/external.hpp"

#include "brainstem_api/external.grpc.pb.h"
#include "connection.hpp"

#include <cmath>
#include <mutex>
#include <thread>
#include <utility>

namespace brainstem {
namespace {

using Stub = api::v1::RobotExternalControl::Stub;
using Clock = std::chrono::steady_clock;

ExternalStatus decode(const api::v1::ExternalStatus &source) {
  ExternalStatus state;
  state.selected = source.selected();
  state.active = source.active();
  state.motor_output_enabled = source.motor_output_enabled();
  state.motor_disable_confirmed = source.motor_disable_confirmed();
  state.session_id = source.session_id();
  state.reason = source.reason();
  state.last_sequence = source.last_sequence();
  state.input_window = source.input_window();
  state.joint_count = source.joint_count();
  state.fresh = true;
  return state;
}

Result result(const grpc::Status &status, const ExternalStatus &state) {
  Result value;
  value.ok = status.ok();
  value.accepted = status.ok();
  value.transport_ok = status.ok() || status.error_code() == grpc::StatusCode::FAILED_PRECONDITION ||
      status.error_code() == grpc::StatusCode::INVALID_ARGUMENT ||
      status.error_code() == grpc::StatusCode::PERMISSION_DENIED ||
      status.error_code() == grpc::StatusCode::UNIMPLEMENTED;
  value.state.connected = value.transport_ok;
  value.state.motor_output_enabled = state.motor_output_enabled;
  value.state.reason = status.ok() ? state.reason : status.error_message();
  if (!status.ok()) value.error = status.error_message();
  return value;
}

}  // namespace

struct ExternalControl::Impl {
  explicit Impl(Config value) : config(std::move(value)) {
    detail::validateConfig(config);
    stub = api::v1::RobotExternalControl::NewStub(detail::createChannel(config));
  }

  ~Impl() { closeStream(); }

  void remember(const api::v1::ExternalStatus &response) {
    std::lock_guard<std::mutex> lock(state_mutex);
    latest = decode(response);
    received_at = Clock::now();
  }

  ExternalStatus snapshot() const {
    std::lock_guard<std::mutex> lock(state_mutex);
    auto copy = latest;
    if (received_at == Clock::time_point{}) return copy;
    copy.received_age_ms = std::chrono::duration<double, std::milli>(Clock::now() - received_at).count();
    copy.fresh = copy.fresh && copy.received_age_ms <= 250.0;
    return copy;
  }

  void closeStream() {
    if (context) context->TryCancel();
    if (reader.joinable()) reader.join();
    if (stream) (void)stream->Finish();
    stream.reset();
    context.reset();
  }

  bool openStream() {
    context = std::make_unique<grpc::ClientContext>();
    stream = stub->Control(context.get());
    api::v1::ExternalFrame hello;
    hello.set_session_token(token);
    if (!stream || !stream->Write(hello)) {
      closeStream();
      return false;
    }
    reader = std::thread([this] {
      api::v1::ExternalStatus response;
      while (stream->Read(&response)) remember(response);
      std::lock_guard<std::mutex> lock(state_mutex);
      latest.fresh = false;
      latest.input_window = 0;
      latest.reason = "external command stream ended";
    });
    return true;
  }

  using Operation = grpc::Status (Stub::*)(grpc::ClientContext *, const api::v1::ExternalSession &,
                                         api::v1::ExternalStatus *);

  Response<ExternalStatus> call(Operation operation) {
    if (token.empty()) {
      const auto state = snapshot();
      return {result({grpc::StatusCode::FAILED_PRECONDITION, "external session is missing"}, state), state};
    }
    api::v1::ExternalSession request;
    request.set_session_token(token);
    api::v1::ExternalStatus response;
    grpc::ClientContext call_context;
    call_context.set_deadline(std::chrono::system_clock::now() + config.timeout);
    const auto status = (stub.get()->*operation)(&call_context, request, &response);
    if (status.ok()) remember(response);
    const auto state = status.ok() ? decode(response) : snapshot();
    return {result(status, state), state};
  }

  Config config;
  std::unique_ptr<Stub> stub;
  std::string token;
  mutable std::mutex state_mutex;
  ExternalStatus latest;
  Clock::time_point received_at{};
  std::unique_ptr<grpc::ClientContext> context;
  std::unique_ptr<grpc::ClientReaderWriter<api::v1::ExternalFrame, api::v1::ExternalStatus>> stream;
  std::thread reader;
};

ExternalControl::ExternalControl(Config config) : impl_(std::make_unique<Impl>(std::move(config))) {}
ExternalControl::~ExternalControl() = default;

Response<ExternalStatus> ExternalControl::enter(const std::string &session_id) {
  impl_->closeStream();
  api::v1::ExternalEnterRequest request;
  request.set_client_id(impl_->config.client_id);
  request.set_request_id(session_id);
  api::v1::ExternalStatus response;
  grpc::ClientContext context;
  context.set_deadline(std::chrono::system_clock::now() + impl_->config.timeout);
  const auto rpc = impl_->stub->Enter(&context, request, &response);
  if (!rpc.ok()) return {result(rpc, impl_->snapshot()), impl_->snapshot()};
  impl_->remember(response);
  impl_->token = response.session_token();
  const auto state = decode(response);
  if (session_id.empty() || response.session_id() != session_id || impl_->token.empty() ||
      !response.selected() || !response.active()) {
    return {result({grpc::StatusCode::DATA_LOSS, "invalid external Enter acknowledgement"}, state), state};
  }
  if (!impl_->openStream()) {
    return {result({grpc::StatusCode::UNAVAILABLE, "external command stream did not open"}, state), state};
  }
  return {result(grpc::Status::OK, state), state};
}

Response<ExternalStatus> ExternalControl::enable() { return impl_->call(&Stub::Enable); }
Response<ExternalStatus> ExternalControl::disable() { return impl_->call(&Stub::Disable); }
Response<ExternalStatus> ExternalControl::heartbeat() { return impl_->call(&Stub::Heartbeat); }

Response<ExternalStatus> ExternalControl::exit() {
  auto response = impl_->call(&Stub::Exit);
  if (response.result.ok) {
    impl_->closeStream();
    impl_->token.clear();
    // The stream cancellation completes before the confirmed unary result is retained.
    std::lock_guard<std::mutex> lock(impl_->state_mutex);
    impl_->latest = response.value;
    impl_->received_at = Clock::now();
  }
  return response;
}

Response<ExternalStatus> ExternalControl::getStatus() {
  google::protobuf::Empty request;
  api::v1::ExternalStatus response;
  grpc::ClientContext context;
  context.set_deadline(std::chrono::system_clock::now() + impl_->config.timeout);
  const auto rpc = impl_->stub->GetStatus(&context, request, &response);
  if (rpc.ok()) impl_->remember(response);
  const auto state = rpc.ok() ? decode(response) : impl_->snapshot();
  return {result(rpc, state), state};
}

Result ExternalControl::send(const ExternalFrame &frame) {
  if (!impl_->stream || frame.sequence == 0 || frame.input_window == 0) {
    return result({grpc::StatusCode::FAILED_PRECONDITION, "external input window is missing"}, status());
  }
  api::v1::ExternalFrame request;
  request.set_session_token(impl_->token);
  request.set_sequence(frame.sequence);
  request.set_input_window(frame.input_window);
  for (const auto &joint : frame.joints) {
    if (!std::isfinite(joint.q) || !std::isfinite(joint.dq) || !std::isfinite(joint.kp) ||
        !std::isfinite(joint.kd) || !std::isfinite(joint.tau) || joint.kp < 0 || joint.kd < 0) {
      return result({grpc::StatusCode::INVALID_ARGUMENT, "invalid external joint target"}, status());
    }
    auto *target = request.add_targets();
    target->set_q(joint.q);
    target->set_dq(joint.dq);
    target->set_kp(joint.kp);
    target->set_kd(joint.kd);
    target->set_tau(joint.tau);
  }
  const bool written = impl_->stream->Write(request);
  return result(written ? grpc::Status::OK : grpc::Status(grpc::StatusCode::UNAVAILABLE, "external stream write failed"), status());
}

ExternalStatus ExternalControl::status() const { return impl_->snapshot(); }

}  // namespace brainstem
