#include "brainstem/client.hpp"
#include "brainstem_api/cms.grpc.pb.h"
#include <grpcpp/grpcpp.h>
#include <cstdio>
#include <stdexcept>

namespace {
void check(bool value, const char *message) { if (!value) throw std::runtime_error(message); }

class Profiles final : public brainstem::api::v1::RobotControl::Service {
 public:
  grpc::Status GetProfile(grpc::ServerContext *, const google::protobuf::Empty *,
                          brainstem::api::v1::ProfileInfo *reply) override {
    ++reads;
    reply->set_current(current);
    reply->add_available("walk");
    reply->add_descriptions("ordinary controller");
    reply->add_available("stairs");
    reply->add_descriptions("test-only stair controller");
    reply->set_current_description("active controller description");
    return grpc::Status::OK;
  }
  grpc::Status SwitchProfile(grpc::ServerContext *, const brainstem::api::v1::ProfileRequest *request,
                             brainstem::api::v1::ProfileInfo *reply) override {
    ++switches;
    if (!grounded) return {grpc::StatusCode::FAILED_PRECONDITION, "must be Grounded"};
    check(request->name() == "stairs", "profile name must reach server unchanged");
    reply->set_current(request->name());
    return grpc::Status::OK; // Deliberately leave GetProfile reporting the old value.
  }
  std::string current{"walk"};
  bool grounded{false};
  int reads{0}, switches{0};
};

void profileRpcContract() {
  Profiles service;
  grpc::ServerBuilder builder;
  int port = 0;
  builder.AddListeningPort("127.0.0.1:0", grpc::InsecureServerCredentials(), &port);
  builder.RegisterService(&service);
  auto server = builder.BuildAndStart();
  check(server != nullptr && port > 0, "mock service starts");
  brainstem::Config config;
  config.target = "127.0.0.1:" + std::to_string(port);
  config.client_id = "profile-contract-test";
  config.allow_insecure = true;
  brainstem::Client client(config);
  const auto initial = client.getProfile();
  check(initial.result.ok && initial.value.current == "walk", "read actual current profile");
  check(initial.value.available.size() == 2 && initial.value.descriptions.size() == 2 &&
        !initial.value.current_description.empty(), "preserve profile metadata");
  const auto rejected = client.switchProfile("stairs");
  check(!rejected.result.ok && rejected.result.transport_ok && !rejected.result.accepted &&
        rejected.result.error.find("Grounded") != std::string::npos,
        "standing server rejection remains distinct from network failure");
  service.grounded = true;
  const auto accepted = client.switchProfile("stairs");
  check(accepted.result.ok && accepted.result.accepted && !accepted.result.state.motion_ready,
        "accepted profile command must not announce locomotion readiness");
  check(accepted.value.current == "stairs", "preserve RPC response without inventing a later readback");
  const auto separate = client.getProfile();
  check(separate.value.current == "walk" && service.reads == 2,
        "GetProfile issues a separate RPC and does not echo requested profile");
  const int previous = service.switches;
  check(!client.switchProfile("").result.ok && service.switches == previous,
        "empty profile fails before sending an invalid command");
  server->Shutdown();
  server->Wait();
}
}

int main() {
  try { profileRpcContract(); return 0; }
  catch (const std::exception &error) {
    std::fprintf(stderr, "brainstem_profile_test: %s\n", error.what());
    return 1;
  }
}
