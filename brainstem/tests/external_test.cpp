#include "brainstem/external.hpp"
#include "brainstem_api/external.grpc.pb.h"

#include <grpcpp/grpcpp.h>

#include <chrono>
#include <cmath>
#include <condition_variable>
#include <cstdio>
#include <limits>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

namespace {
using namespace std::chrono_literals;
namespace api = brainstem::api::v1;

void check(bool value, const char *message) {
  if (!value) throw std::runtime_error(message);
}

template <typename Predicate>
void waitUntil(Predicate predicate, const char *message) {
  const auto deadline = std::chrono::steady_clock::now() + 2s;
  while (!predicate()) {
    if (std::chrono::steady_clock::now() >= deadline) throw std::runtime_error(message);
    std::this_thread::sleep_for(2ms);
  }
}

class ExternalService final : public api::RobotExternalControl::Service {
 public:
  grpc::Status Enter(grpc::ServerContext *, const api::ExternalEnterRequest *request,
                     api::ExternalStatus *reply) override {
    std::lock_guard<std::mutex> lock(mutex);
    enter_request = *request;
    current.set_selected(true);
    current.set_active(true);
    current.set_session_id(request->request_id());
    current.set_reason("external_ready");
    current.set_joint_count(16);
    *reply = current;
    reply->set_session_token("private-driver-token");
    return grpc::Status::OK;
  }

  grpc::Status Enable(grpc::ServerContext *, const api::ExternalSession *request,
                      api::ExternalStatus *reply) override {
    std::lock_guard<std::mutex> lock(mutex);
    tokens.push_back(request->session_token());
    ++enable_calls;
    if (enable_error != grpc::StatusCode::OK) {
      const auto code = enable_error;
      enable_error = grpc::StatusCode::OK;
      return {code, code == grpc::StatusCode::FAILED_PRECONDITION ? "initial targets required" : "reply unavailable"};
    }
    current.set_motor_output_enabled(true);
    current.set_motor_disable_confirmed(false);
    *reply = current;
    return grpc::Status::OK;
  }

  grpc::Status Disable(grpc::ServerContext *, const api::ExternalSession *request,
                       api::ExternalStatus *reply) override {
    std::lock_guard<std::mutex> lock(mutex);
    tokens.push_back(request->session_token());
    current.set_motor_output_enabled(false);
    current.set_motor_disable_confirmed(true);
    current.set_reason("external_disabled");
    *reply = current;
    return grpc::Status::OK;
  }

  grpc::Status Heartbeat(grpc::ServerContext *, const api::ExternalSession *request,
                         api::ExternalStatus *reply) override {
    std::lock_guard<std::mutex> lock(mutex);
    tokens.push_back(request->session_token());
    *reply = current;
    return grpc::Status::OK;
  }

  grpc::Status Exit(grpc::ServerContext *, const api::ExternalSession *request,
                    api::ExternalStatus *reply) override {
    std::lock_guard<std::mutex> lock(mutex);
    tokens.push_back(request->session_token());
    ++exit_calls;
    current.set_selected(false);
    current.set_active(false);
    current.set_motor_output_enabled(false);
    current.set_motor_disable_confirmed(true);
    current.clear_session_id();
    current.set_input_window(0);
    current.set_reason("builtin");
    *reply = current;
    return grpc::Status::OK;
  }

  grpc::Status GetStatus(grpc::ServerContext *, const google::protobuf::Empty *,
                         api::ExternalStatus *reply) override {
    std::lock_guard<std::mutex> lock(mutex);
    ++status_reads;
    *reply = current;
    return grpc::Status::OK;
  }

  grpc::Status Control(grpc::ServerContext *context,
                       grpc::ServerReaderWriter<api::ExternalStatus, api::ExternalFrame> *stream) override {
    api::ExternalFrame hello;
    if (!stream->Read(&hello)) return grpc::Status::OK;
    api::ExternalStatus update;
    {
      std::lock_guard<std::mutex> lock(mutex);
      handshake = hello;
      current.set_input_window(321);
      update = current;
    }
    stream->Write(update);
    api::ExternalFrame frame;
    while (stream->Read(&frame)) {
      std::unique_lock<std::mutex> lock(mutex);
      frames.push_back(frame);
      changed.notify_all();
      while (!dispatch_allowed && !context->IsCancelled()) changed.wait_for(lock, 5ms);
      if (context->IsCancelled()) break;
      current.set_last_sequence(frame.sequence());
      current.set_input_window(987);
      update = current;
      lock.unlock();
      if (!stream->Write(update)) break;
    }
    {
      std::lock_guard<std::mutex> lock(mutex);
      stream_closed = true;
    }
    changed.notify_all();
    return grpc::Status::OK;
  }

  void waitForFrame() {
    std::unique_lock<std::mutex> lock(mutex);
    check(changed.wait_for(lock, 2s, [&] { return !frames.empty(); }), "server must receive the joint frame");
  }

  void waitForClose() {
    std::unique_lock<std::mutex> lock(mutex);
    check(changed.wait_for(lock, 2s, [&] { return stream_closed; }), "client destruction must cancel the stream");
  }

  std::mutex mutex;
  std::condition_variable changed;
  api::ExternalEnterRequest enter_request;
  api::ExternalFrame handshake;
  api::ExternalStatus current;
  std::vector<api::ExternalFrame> frames;
  std::vector<std::string> tokens;
  grpc::StatusCode enable_error{grpc::StatusCode::OK};
  bool dispatch_allowed{false};
  bool stream_closed{false};
  int status_reads{0}, enable_calls{0}, exit_calls{0};
};

class Server {
 public:
  Server() {
    grpc::ServerBuilder builder;
    int port = 0;
    builder.AddListeningPort("127.0.0.1:0", grpc::InsecureServerCredentials(), &port);
    builder.RegisterService(&service);
    server = builder.BuildAndStart();
    check(server != nullptr && port > 0, "local external server starts");
    config.target = "127.0.0.1:" + std::to_string(port);
    config.client_id = "external-contract-test";
    config.allow_insecure = true;
    config.timeout = 500ms;
  }

  ~Server() {
    server->Shutdown();
    server->Wait();
  }

  ExternalService service;
  brainstem::Config config;
  std::unique_ptr<grpc::Server> server;
};

brainstem::ExternalFrame targets() {
  brainstem::ExternalFrame frame;
  frame.sequence = 7;
  frame.input_window = 321;
  for (std::size_t index = 0; index < frame.joints.size(); ++index) {
    frame.joints[index] = {0.01 * index, -0.02 * index, 30.0 + index, 1.5 + 0.01 * index, 0.03 * index};
  }
  return frame;
}

void streamContract() {
  Server server;
  auto &service = server.service;
  brainstem::ExternalControl client(server.config);
  const auto entered = client.enter("session-1");
  check(entered.result.ok && entered.value.selected && entered.value.active, "enter returns the server session");
  waitUntil([&] { return client.status().input_window == 321; }, "background reader receives the first window");
  {
    std::lock_guard<std::mutex> lock(service.mutex);
    check(service.enter_request.client_id() == "external-contract-test" &&
          service.enter_request.request_id() == "session-1", "enter preserves client and session metadata");
    check(service.handshake.sequence() == 0 && service.handshake.targets_size() == 0 &&
          service.handshake.session_token() == "private-driver-token", "control opens with the private token handshake");
    service.current.set_input_window(654);
  }
  const auto separate = client.getStatus();
  check(separate.result.ok && separate.value.input_window == 654, "unary status reads the current source window");
  const auto frame = targets();
  const auto queued = client.send(frame);
  check(queued.ok && !queued.stop_confirmed, "stream Write succeeds without manufacturing stop evidence");
  service.waitForFrame();
  check(client.status().last_sequence == 0, "successful Write does not invent robot dispatch confirmation");
  {
    std::lock_guard<std::mutex> lock(service.mutex);
    const auto &received = service.frames.front();
    check(received.sequence() == 7 && received.input_window() == 321,
          "the caller's original window survives a later status window");
    check(received.session_token() == "private-driver-token" && received.targets_size() == 16,
          "the stream carries the driver token and all sixteen joints");
    for (int index = 0; index < received.targets_size(); ++index) {
      const auto &actual = received.targets(index);
      const auto &expected = frame.joints[index];
      check(actual.q() == expected.q && actual.dq() == expected.dq && actual.kp() == expected.kp &&
            actual.kd() == expected.kd && actual.tau() == expected.tau, "joint values reach the server unchanged");
    }
    service.dispatch_allowed = true;
    service.changed.notify_all();
  }
  waitUntil([&] { return client.status().last_sequence == 7; }, "background state confirms robot dispatch independently");
  check(client.status().input_window == 987, "background updates preserve robot window changes");
  check(client.enable().result.ok && client.status().motor_output_enabled, "enable uses the current private session");
  check(client.heartbeat().result.ok, "explicit heartbeat uses the current private session");
  const auto disabled = client.disable();
  check(disabled.result.ok && disabled.value.selected && !disabled.value.motor_output_enabled &&
        disabled.value.motor_disable_confirmed, "disable closes output while retaining external selection");
  const auto exited = client.exit();
  check(exited.result.ok && !exited.value.selected && !exited.value.active &&
        !exited.value.motor_output_enabled && exited.value.motor_disable_confirmed, "exit returns the confirmed builtin state");
  check(client.status().fresh && client.status().reason == "builtin" && client.status().input_window == 0,
        "stream shutdown cannot overwrite the confirmed Exit state");
  check(!client.enable().result.accepted, "exit clears local permission for further actuator requests");
  {
    std::lock_guard<std::mutex> lock(service.mutex);
    check(service.status_reads == 1, "background confirmation does not call the unary status API");
    check(service.enable_calls == 1 && service.exit_calls == 1, "closed sessions do not send another Enable");
    for (const auto &token : service.tokens) check(token == "private-driver-token", "session operations preserve the private token");
  }
}

void invalidFramesStayLocal() {
  Server server;
  brainstem::ExternalControl client(server.config);
  check(!client.send(targets()).accepted, "a frame requires an entered stream");
  check(client.enter("validation").result.ok, "validation session enters");
  auto frame = targets();
  frame.joints[3].q = std::numeric_limits<double>::quiet_NaN();
  check(!client.send(frame).accepted, "non-finite targets are rejected before transport");
  frame = targets();
  frame.joints[4].kd = -1;
  check(!client.send(frame).accepted, "negative gains are rejected before transport");
  frame = targets();
  frame.input_window = 0;
  check(!client.send(frame).accepted, "missing windows are rejected before transport");
  frame = targets();
  frame.sequence = 0;
  check(!client.send(frame).accepted, "joint frames require a nonzero sequence");
  std::lock_guard<std::mutex> lock(server.service.mutex);
  check(server.service.frames.empty(), "invalid targets never reach the backend");
}

void rejectionAndUnknownStayDistinct() {
  Server server;
  brainstem::ExternalControl client(server.config);
  check(client.enter("failures").result.ok, "failure test session enters");
  for (const auto code : {grpc::StatusCode::FAILED_PRECONDITION, grpc::StatusCode::UNAVAILABLE,
                          grpc::StatusCode::DEADLINE_EXCEEDED}) {
    {
      std::lock_guard<std::mutex> lock(server.service.mutex);
      server.service.enable_error = code;
    }
    const auto reply = client.enable();
    check(!reply.result.ok && !reply.result.accepted, "failed unary operations do not report acceptance");
    check(reply.result.transport_ok == (code == grpc::StatusCode::FAILED_PRECONDITION),
          "explicit rejection stays distinguishable from a transport-unknown outcome");
    check(reply.value.selected && reply.value.session_id == "failures", "failures retain the last observed state");
    check(!reply.result.error.empty(), "source failure reason is preserved");
  }
}

void dropCancelsWithoutExiting() {
  Server server;
  {
    brainstem::ExternalControl client(server.config);
    check(client.enter("drop").result.ok, "drop test session enters");
    waitUntil([&] { return client.status().input_window == 321; }, "drop test stream receives a window");
  }
  server.service.waitForClose();
  std::lock_guard<std::mutex> lock(server.service.mutex);
  check(server.service.exit_calls == 0, "destruction does not explicitly restore builtin mode");
}

}  // namespace

int main() {
  try {
    streamContract();
    invalidFramesStayLocal();
    rejectionAndUnknownStayDistinct();
    dropCancelsWithoutExiting();
    return 0;
  } catch (const std::exception &error) {
    std::fprintf(stderr, "brainstem_external_test: %s\n", error.what());
    return 1;
  }
}
