#include "brainstem/client.hpp"

#include <grpcpp/grpcpp.h>

#include <chrono>
#include <cmath>
#include <condition_variable>
#include <cstdio>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <limits>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

#include "brainstem_api/cms.grpc.pb.h"

namespace {

using namespace std::chrono_literals;

void check(bool value, const char *message) {
  if (!value) {
    throw std::runtime_error(message);
  }
}

void close(double actual, double expected, const char *message) {
  if (std::abs(actual - expected) > 1e-9) {
    throw std::runtime_error(message);
  }
}

std::string readTextFile(const std::filesystem::path &path) {
  std::ifstream input(path, std::ios::binary);
  return std::string(std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>());
}

class ControlService final : public brainstem::api::v1::RobotControl::Service {
 public:
  grpc::Status GetControlStatus(grpc::ServerContext *, const google::protobuf::Empty *,
                                brainstem::api::v1::ControlStatus *response) override {
    std::lock_guard<std::mutex> lock(mutex_);
    ++status_calls_;
    fillStatus(*response);
    return grpc::Status::OK;
  }

  grpc::Status GetCmsState(grpc::ServerContext *, const google::protobuf::Empty *,
                           brainstem::api::v1::CmsState *response) override {
    response->set_kind(brainstem::api::v1::CMS_STATE_KIND_TRANSITIONING);
    response->set_transition(brainstem::api::v1::CMS_TRANSITION_KIND_GESTURE);
    response->set_gesture_name("wave");
    return grpc::Status::OK;
  }

  grpc::Status GetStartTime(grpc::ServerContext *, const google::protobuf::Empty *,
                            google::protobuf::Timestamp *response) override {
    response->set_seconds(1700000000);
    response->set_nanos(123456789);
    return grpc::Status::OK;
  }

  grpc::Status GetMotorStatus(grpc::ServerContext *, const google::protobuf::Empty *,
                              brainstem::api::v1::MotorStatusResponse *response) override {
    auto *motor = response->add_motors();
    motor->set_id(15);
    motor->set_online(true);
    motor->set_status_code(7);
    motor->set_temperature(42.5);
    motor->set_voltage(23.8);
    motor->set_position(1.25);
    motor->set_velocity(-0.75);
    motor->set_torque(3.5);
    motor->add_errors(9);
    motor->add_errors(11);
    return grpc::Status::OK;
  }

  grpc::Status GetVoltage(grpc::ServerContext *, const google::protobuf::Empty *,
                          brainstem::api::v1::Voltage *response) override {
    response->add_values(23.1);
    response->add_values(23.2);
    return grpc::Status::OK;
  }

  grpc::Status Enable(grpc::ServerContext *, const google::protobuf::Empty *,
                      google::protobuf::Empty *) override {
    std::lock_guard<std::mutex> lock(mutex_);
    ++enable_calls_;
    motors_enabled_ = true;
    return grpc::Status::OK;
  }

  grpc::Status Disable(grpc::ServerContext *, const google::protobuf::Empty *,
                       google::protobuf::Empty *) override {
    std::lock_guard<std::mutex> lock(mutex_);
    ++disable_calls_;
    motors_enabled_ = false;
    lease_active_ = false;
    return grpc::Status::OK;
  }

  grpc::Status SetZero(grpc::ServerContext *, const google::protobuf::Empty *,
                       google::protobuf::Empty *) override {
    std::lock_guard<std::mutex> lock(mutex_);
    ++set_zero_calls_;
    return grpc::Status::OK;
  }

  grpc::Status ClearMotorFault(grpc::ServerContext *,
                               const brainstem::api::v1::ClearFaultRequest *request,
                               google::protobuf::Empty *) override {
    std::lock_guard<std::mutex> lock(mutex_);
    ++clear_fault_calls_;
    cleared_joint_ids_.assign(request->joint_ids().begin(), request->joint_ids().end());
    lease_active_ = false;
    if (cleared_joint_ids_.empty()) {
      motors_enabled_ = false;
    }
    return grpc::Status::OK;
  }

  grpc::Status ListenImu(grpc::ServerContext *context, const google::protobuf::Empty *,
                         grpc::ServerWriter<brainstem::api::v1::Imu> *writer) override {
    bool include_acceleration = true;
    {
      std::unique_lock<std::mutex> lock(mutex_);
      include_acceleration = include_imu_acceleration_;
      if (hold_imu_stream_) {
        imu_stream_open_ = true;
        imu_condition_.notify_all();
        lock.unlock();
        while (!context->IsCancelled()) {
          std::this_thread::sleep_for(1ms);
        }
        return grpc::Status::OK;
      }
    }
    brainstem::api::v1::Imu sample;
    sample.mutable_gyroscope()->set_x(1.0);
    sample.mutable_gyroscope()->set_y(-2.0);
    sample.mutable_gyroscope()->set_z(3.5);
    if (include_acceleration) {
      sample.mutable_linear_acceleration()->set_x(0.5);
      sample.mutable_linear_acceleration()->set_y(-1.0);
      sample.mutable_linear_acceleration()->set_z(9.75);
    }
    sample.mutable_quaternion()->set_w(0.5);
    sample.mutable_quaternion()->set_x(0.1);
    sample.mutable_quaternion()->set_y(0.2);
    sample.mutable_quaternion()->set_z(0.3);
    sample.mutable_timestamp()->set_seconds(12);
    sample.mutable_timestamp()->set_nanos(345);
    writer->Write(sample);
    return grpc::Status::OK;
  }

  grpc::Status ListenJoint(grpc::ServerContext *, const google::protobuf::Empty *,
                           grpc::ServerWriter<brainstem::api::v1::Joint> *writer) override {
    brainstem::api::v1::Joint all_sample;
    auto *all = all_sample.mutable_all_joints();
    for (std::uint32_t index = 0; index < 16; ++index) {
      all->mutable_position()->add_values(static_cast<double>(index) + 0.1);
      all->mutable_velocity()->add_values(static_cast<double>(index) + 0.2);
      all->mutable_torque()->add_values(static_cast<double>(index) + 0.3);
      all->mutable_status()->add_values(index + 100);
    }
    all_sample.mutable_timestamp()->set_seconds(5);
    writer->Write(all_sample);

    brainstem::api::v1::Joint single_sample;
    auto *single = single_sample.mutable_single_joint();
    single->set_id(7);
    single->set_position(-1.0);
    single->set_velocity(-2.0);
    single->set_torque(-3.0);
    single->set_status(707);
    single_sample.mutable_timestamp()->set_nanos(99);
    writer->Write(single_sample);
    return grpc::Status::OK;
  }

  grpc::Status ListenCmsState(grpc::ServerContext *, const google::protobuf::Empty *,
                              grpc::ServerWriter<brainstem::api::v1::CmsState> *writer) override {
    brainstem::api::v1::CmsState standing;
    standing.set_kind(brainstem::api::v1::CMS_STATE_KIND_STANDING);
    writer->Write(standing);

    brainstem::api::v1::CmsState transitioning;
    transitioning.set_kind(brainstem::api::v1::CMS_STATE_KIND_TRANSITIONING);
    transitioning.set_transition(brainstem::api::v1::CMS_TRANSITION_KIND_SIT_DOWN);
    writer->Write(transitioning);
    return grpc::Status::OK;
  }

  grpc::Status AcquireControl(grpc::ServerContext *,
                              const brainstem::api::v1::AcquireControlRequest *request,
                              brainstem::api::v1::ControlLease *response) override {
    std::lock_guard<std::mutex> lock(mutex_);
    ++acquire_calls_;
    client_id_ = request->client_id();
    lease_active_ = true;
    preempted_ = false;
    last_accepted_sequence_ = 0;
    response->set_accepted(true);
    response->set_token(kToken);
    response->set_lease_duration_ms(300);
    fillStatus(*response->mutable_status());
    return grpc::Status::OK;
  }

  grpc::Status RenewControlLease(grpc::ServerContext *,
                                 const brainstem::api::v1::ControlLeaseRequest *request,
                                 brainstem::api::v1::ControlLease *response) override {
    std::lock_guard<std::mutex> lock(mutex_);
    ++renew_calls_;
    const bool accepted = lease_active_ && request->token() == kToken;
    response->set_accepted(accepted);
    response->set_token(accepted ? kToken : "");
    response->set_lease_duration_ms(accepted ? 300 : 0);
    response->set_reason(accepted ? brainstem::api::v1::COMMAND_REJECT_REASON_NONE
                                  : brainstem::api::v1::COMMAND_REJECT_REASON_LEASE_EXPIRED);
    fillStatus(*response->mutable_status());
    return grpc::Status::OK;
  }

  grpc::Status WalkChecked(grpc::ServerContext *, const brainstem::api::v1::WalkRequest *request,
                           brainstem::api::v1::CommandAck *response) override {
    std::lock_guard<std::mutex> lock(mutex_);
    walk_request_sequences_.push_back(request->sequence());
    if (retry_sequence_ != 0 && request->sequence() == retry_sequence_) {
      check(request->direction().x() == retry_velocity_.vx_mps &&
                request->direction().y() == retry_velocity_.vy_mps &&
                request->direction().z() == retry_velocity_.yaw_rps,
            "transport retry must reuse the payload");
      if (drop_ack_attempts_ > 0) {
        --drop_ack_attempts_;
        return grpc::Status(grpc::StatusCode::UNAVAILABLE, "simulated lost ACK");
      }
      response->set_accepted(true);
      response->set_sequence(request->sequence());
      response->set_reason(brainstem::api::v1::COMMAND_REJECT_REASON_NONE);
      fillStatus(*response->mutable_status());
      retry_sequence_ = 0;
      return grpc::Status::OK;
    }
    if (retry_sequence_ != 0) {
      check(request->sequence() == retry_sequence_ + 1,
            "command after an ambiguous ACK must continue after the accepted sequence");
      retry_sequence_ = 0;
    }
    const bool preserve_lease_rejection = reject_preserving_lease_next_;
    const bool accepted =
        lease_active_ && request->token() == kToken && !reject_next_ && !preserve_lease_rejection;
    const std::uint64_t response_sequence =
        mismatch_next_ack_ ? request->sequence() + 1 : request->sequence();
    response->set_accepted(accepted);
    response->set_sequence(response_sequence);
    response->set_reason(accepted ? brainstem::api::v1::COMMAND_REJECT_REASON_NONE
                                  : (preserve_lease_rejection
                                         ? brainstem::api::v1::COMMAND_REJECT_REASON_CONTROL_BUSY
                                         : brainstem::api::v1::COMMAND_REJECT_REASON_PREEMPTED));
    if (!accepted && !preserve_lease_rejection) {
      lease_active_ = false;
      preempted_ = true;
    }
    if (accepted) {
      last_accepted_sequence_ = request->sequence();
    }
    fillStatus(*response->mutable_status());
    if (accepted) {
      velocities_.push_back(
          {request->direction().x(), request->direction().y(), request->direction().z()});
      accepted_at_.push_back(std::chrono::steady_clock::now());
      if (drop_ack_attempts_ > 0) {
        --drop_ack_attempts_;
        retry_sequence_ = request->sequence();
        retry_velocity_ = velocities_.back();
        return grpc::Status(grpc::StatusCode::UNAVAILABLE, "simulated lost ACK");
      }
    }
    reject_next_ = false;
    reject_preserving_lease_next_ = false;
    mismatch_next_ack_ = false;
    return grpc::Status::OK;
  }

  grpc::Status ReleaseControl(grpc::ServerContext *,
                              const brainstem::api::v1::ControlLeaseRequest *request,
                              brainstem::api::v1::ControlStatus *response) override {
    std::lock_guard<std::mutex> lock(mutex_);
    ++release_calls_;
    if (fail_next_release_) {
      fail_next_release_ = false;
      return grpc::Status(grpc::StatusCode::UNAVAILABLE, "release failed");
    }
    check(request->token() == kToken, "release must carry the active token");
    lease_active_ = false;
    fillStatus(*response);
    return grpc::Status::OK;
  }

  grpc::Status StandUp(grpc::ServerContext *, const google::protobuf::Empty *,
                       google::protobuf::Empty *) override {
    std::lock_guard<std::mutex> lock(mutex_);
    ++stand_calls_;
    return grpc::Status::OK;
  }

  grpc::Status SitDown(grpc::ServerContext *, const google::protobuf::Empty *,
                       google::protobuf::Empty *) override {
    std::lock_guard<std::mutex> lock(mutex_);
    ++sit_calls_;
    return grpc::Status::OK;
  }

  [[nodiscard]] unsigned statusCalls() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return status_calls_;
  }

  [[nodiscard]] unsigned acquireCalls() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return acquire_calls_;
  }

  [[nodiscard]] unsigned renewCalls() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return renew_calls_;
  }

  [[nodiscard]] unsigned releaseCalls() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return release_calls_;
  }

  [[nodiscard]] unsigned standCalls() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return stand_calls_;
  }

  [[nodiscard]] unsigned sitCalls() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return sit_calls_;
  }

  [[nodiscard]] unsigned enableCalls() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return enable_calls_;
  }

  [[nodiscard]] unsigned disableCalls() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return disable_calls_;
  }

  [[nodiscard]] unsigned setZeroCalls() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return set_zero_calls_;
  }

  [[nodiscard]] unsigned clearFaultCalls() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return clear_fault_calls_;
  }

  [[nodiscard]] std::vector<std::uint32_t> clearedJointIds() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return cleared_joint_ids_;
  }

  [[nodiscard]] std::string clientId() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return client_id_;
  }

  [[nodiscard]] std::vector<brainstem::Velocity> velocities() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return velocities_;
  }

  [[nodiscard]] std::vector<std::uint64_t> walkRequestSequences() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return walk_request_sequences_;
  }

  [[nodiscard]] std::chrono::milliseconds commandSpacing(std::size_t first,
                                                         std::size_t second) const {
    std::lock_guard<std::mutex> lock(mutex_);
    return std::chrono::duration_cast<std::chrono::milliseconds>(accepted_at_.at(second) -
                                                                 accepted_at_.at(first));
  }

  void rejectNextCommand() {
    std::lock_guard<std::mutex> lock(mutex_);
    reject_next_ = true;
  }

  void mismatchNextAck() {
    std::lock_guard<std::mutex> lock(mutex_);
    mismatch_next_ack_ = true;
  }

  void rejectNextCommandPreservingLease() {
    std::lock_guard<std::mutex> lock(mutex_);
    reject_preserving_lease_next_ = true;
  }

  void failNextRelease() {
    std::lock_guard<std::mutex> lock(mutex_);
    fail_next_release_ = true;
  }

  void dropNextAck() {
    std::lock_guard<std::mutex> lock(mutex_);
    drop_ack_attempts_ = 1;
  }

  void dropNextTwoAcks() {
    std::lock_guard<std::mutex> lock(mutex_);
    drop_ack_attempts_ = 2;
  }

  void holdImuStream() {
    std::lock_guard<std::mutex> lock(mutex_);
    hold_imu_stream_ = true;
  }

  void omitImuAcceleration() {
    std::lock_guard<std::mutex> lock(mutex_);
    include_imu_acceleration_ = false;
  }

  bool waitForImuStream() {
    std::unique_lock<std::mutex> lock(mutex_);
    return imu_condition_.wait_for(lock, 2s, [&] { return imu_stream_open_; });
  }

 private:
  void fillStatus(brainstem::api::v1::ControlStatus &status) const {
    status.mutable_fsm()->set_kind(brainstem::api::v1::CMS_STATE_KIND_STANDING);
    status.set_motor_output_enabled(motors_enabled_);
    status.set_owner(lease_active_ ? brainstem::api::v1::CONTROL_OWNER_GRPC
                                   : (preempted_ ? brainstem::api::v1::CONTROL_OWNER_YUNZHUO
                                                 : brainstem::api::v1::CONTROL_OWNER_NONE));
    status.set_critical_motor_fault(false);
    status.set_grpc_lease_active(lease_active_);
    status.set_lease_remaining_ms(lease_active_ ? 300 : 0);
    status.set_ready_for_walk(true);
    status.set_owner_id(lease_active_ ? client_id_ : "");
    status.set_last_accepted_sequence(last_accepted_sequence_);
    status.set_accepted_count(12);
    status.set_rejected_count(3);
  }

  static constexpr const char *kToken = "test-token";

  mutable std::mutex mutex_;
  std::condition_variable imu_condition_;
  std::string client_id_;
  bool lease_active_{false};
  bool preempted_{false};
  bool reject_next_{false};
  bool reject_preserving_lease_next_{false};
  bool mismatch_next_ack_{false};
  bool fail_next_release_{false};
  bool hold_imu_stream_{false};
  bool include_imu_acceleration_{true};
  bool imu_stream_open_{false};
  unsigned drop_ack_attempts_{0};
  std::uint64_t retry_sequence_{0};
  std::uint64_t last_accepted_sequence_{0};
  brainstem::Velocity retry_velocity_;
  unsigned status_calls_{0};
  unsigned acquire_calls_{0};
  unsigned renew_calls_{0};
  unsigned release_calls_{0};
  unsigned stand_calls_{0};
  unsigned sit_calls_{0};
  unsigned enable_calls_{0};
  unsigned disable_calls_{0};
  unsigned set_zero_calls_{0};
  unsigned clear_fault_calls_{0};
  bool motors_enabled_{true};
  std::vector<brainstem::Velocity> velocities_;
  std::vector<std::uint64_t> walk_request_sequences_;
  std::vector<std::uint32_t> cleared_joint_ids_;
  std::vector<std::chrono::steady_clock::time_point> accepted_at_;
};

class LegacyService final : public brainstem::api::v1::RobotControl::Service {};

class TestServer final {
 public:
  TestServer() {
    grpc::ServerBuilder builder;
    builder.AddListeningPort("127.0.0.1:0", grpc::InsecureServerCredentials(), &port_);
    builder.RegisterService(&service_);
    server_ = builder.BuildAndStart();
    check(server_ != nullptr && port_ > 0, "fake Brainstem server must start");
  }

  ~TestServer() {
    server_->Shutdown();
    server_->Wait();
  }

  [[nodiscard]] brainstem::Config config(std::string client_id = "navigation@tests") const {
    brainstem::Config config;
    config.target = "127.0.0.1:" + std::to_string(port_);
    config.timeout = 500ms;
    config.client_id = std::move(client_id);
    config.allow_insecure = true;
    return config;
  }

  ControlService &service() noexcept { return service_; }

 private:
  ControlService service_;
  int port_{0};
  std::unique_ptr<grpc::Server> server_;
};

void testConnectReadsServerState() {
  TestServer server;
  brainstem::Client client(server.config());
  const brainstem::Result result = client.connect();

  check(result.ok, "connect must succeed against RobotControl");
  check(result.transport_ok, "connect must report transport success");
  check(!result.accepted, "a status read must not report command acceptance");
  check(result.state.connected, "connect must expose a connected state");
  check(result.state.motors_enabled, "connect must decode motor output state");
  check(result.state.fsm == "standing", "connect must decode the FSM");
  check(result.state.control_owner == "none", "connect must decode the control owner");
  check(result.state.accepted_count == 12 && result.state.rejected_count == 3,
        "connect must expose command counters");
  check(!result.state.ready, "connect alone must not claim a control lease");
  check(server.service().statusCalls() == 1, "connect must read status exactly once");
}

void testBodyStateSnapshotUsesPublicTypes() {
  TestServer server;
  brainstem::Client client(server.config());

  const brainstem::Response<brainstem::BodyState> response = client.getBodyState();
  check(response.result.ok && response.result.transport_ok,
        "body-state snapshot must report transport success");
  check(!response.result.accepted, "body-state snapshot is not a command acceptance");
  check(response.value.kind == brainstem::BodyStateKind::Transitioning,
        "body-state kind must be mapped");
  check(response.value.transition == brainstem::BodyTransition::Gesture,
        "body-state transition must be mapped");
  check(response.value.gesture_name == "wave", "body-state gesture name must be preserved");
}

void testServerStartTimePreservesUtcPrecision() {
  TestServer server;
  brainstem::Client client(server.config());

  const auto response = client.getServerStartTime();
  check(response.result.ok && response.result.transport_ok,
        "server start time must report transport success");
  const auto elapsed =
      std::chrono::duration_cast<std::chrono::nanoseconds>(response.value.time_since_epoch());
  check(elapsed == std::chrono::seconds(1700000000) + std::chrono::nanoseconds(123456789),
        "server start time must preserve UTC nanosecond precision");
}

void testMotorStatusSnapshotPreservesDiagnostics() {
  TestServer server;
  brainstem::Client client(server.config());

  const brainstem::Response<brainstem::MotorStatus> response = client.getMotorStatus();
  check(response.result.ok && response.value.motors.size() == 1,
        "motor status snapshot must expose returned motors");
  const brainstem::MotorState &motor = response.value.motors.front();
  check(motor.id == 15 && motor.online && motor.status_code == 7,
        "motor identity and status must be preserved");
  close(motor.temperature_c, 42.5, "motor temperature");
  close(motor.voltage_v, 23.8, "motor voltage");
  close(motor.position_rad, 1.25, "motor position");
  close(motor.velocity_rps, -0.75, "motor velocity");
  close(motor.torque_nm, 3.5, "motor torque");
  check(motor.errors == std::vector<std::uint32_t>({9, 11}), "motor errors must be preserved");
}

void testVoltageSnapshotPreservesJointOrder() {
  TestServer server;
  brainstem::Client client(server.config());

  const brainstem::Response<brainstem::Voltage> response = client.getVoltage();
  check(response.result.ok, "voltage snapshot must succeed");
  check(response.value.values_v == std::vector<double>({23.1, 23.2}),
        "voltage values must preserve protocol order");
}

void testImuSubscriptionMapsOneCompleteSample() {
  TestServer server;
  brainstem::Client client(server.config());
  brainstem::ImuSample received;
  bool called = false;

  std::unique_ptr<brainstem::Subscription> subscription =
      client.subscribeImu([&](const brainstem::ImuSample &sample) {
        received = sample;
        called = true;
      });
  const brainstem::Result finished = subscription->wait();

  check(finished.ok && called, "IMU subscription must deliver and finish cleanly");
  close(received.angular_velocity_rps.x, 1.0, "IMU gyro x");
  close(received.angular_velocity_rps.y, -2.0, "IMU gyro y");
  close(received.angular_velocity_rps.z, 3.5, "IMU gyro z");
  check(received.linear_acceleration_mps2.has_value(),
        "IMU sample must preserve acceleration presence");
  close(received.linear_acceleration_mps2->x, 0.5, "IMU acceleration x");
  close(received.linear_acceleration_mps2->y, -1.0, "IMU acceleration y");
  close(received.linear_acceleration_mps2->z, 9.75, "IMU acceleration z");
  close(received.orientation.w, 0.5, "IMU quaternion w");
  close(received.orientation.x, 0.1, "IMU quaternion x");
  close(received.orientation.y, 0.2, "IMU quaternion y");
  close(received.orientation.z, 0.3, "IMU quaternion z");
  check(received.elapsed == std::chrono::seconds(12) + std::chrono::nanoseconds(345),
        "IMU timestamp must preserve duration precision");
}

void testImuSubscriptionAcceptsLegacySampleWithoutAcceleration() {
  TestServer server;
  server.service().omitImuAcceleration();
  brainstem::Client client(server.config());
  brainstem::ImuSample received;

  std::unique_ptr<brainstem::Subscription> subscription =
      client.subscribeImu([&](const brainstem::ImuSample &sample) { received = sample; });
  const brainstem::Result finished = subscription->wait();

  check(finished.ok, "legacy IMU subscription must finish cleanly");
  check(!received.linear_acceleration_mps2.has_value(),
        "a Brainstem 2.1 IMU sample must expose absent acceleration explicitly");
}

void testManagedTelemetryStartsOnlyRequestedStreams() {
  TestServer server;
  brainstem::Client client(server.config());
  brainstem::TelemetryOptions options;
  options.imu = true;
  options.joints = false;
  options.body_state = false;

  const brainstem::Result started = client.startTelemetry(options);
  const brainstem::TelemetrySnapshot telemetry = client.telemetry();
  const brainstem::ImuSnapshot direct_imu = client.latestImu();
  const brainstem::JointSnapshot direct_joints = client.latestJoints();
  const brainstem::BodySnapshot direct_body = client.latestBodyState();

  check(started.ok && started.transport_ok,
        "requested telemetry must start with an initial sample");
  check(telemetry.imu.available && telemetry.imu.fresh,
        "managed telemetry must cache the latest IMU sample");
  check(telemetry.imu.sequence == 1, "first managed IMU sample must have sequence one");
  close(telemetry.imu.value.angular_velocity_rps.x, 1.0, "managed IMU gyro x");
  check(direct_imu.sequence == telemetry.imu.sequence,
        "direct IMU access must read the same managed cache");
  check(direct_joints.valid_mask == telemetry.joints.valid_mask,
        "direct joint access must read the same managed cache");
  check(direct_body.available == telemetry.body.available,
        "direct body-state access must read the same managed cache");
  check(telemetry.joints.valid_mask == 0, "disabled joint telemetry must stay empty");
  check(!telemetry.body.available, "disabled body-state telemetry must stay empty");
}

void testSubscriptionCancellationStopsAStreamingRpc() {
  TestServer server;
  server.service().holdImuStream();
  brainstem::Client client(server.config());
  bool called = false;
  std::unique_ptr<brainstem::Subscription> subscription =
      client.subscribeImu([&](const brainstem::ImuSample &) { called = true; });

  check(server.service().waitForImuStream(), "blocking IMU stream must start");
  check(subscription->active(), "live stream must report active");
  subscription->cancel();
  const brainstem::Result finished = subscription->wait();

  check(finished.ok && finished.transport_ok, "explicit cancellation must finish cleanly");
  check(!subscription->active(), "cancelled stream must report inactive");
  check(!called, "cancelled empty stream must not invent a sample");
}

void testJointSubscriptionMapsSnapshotsAndSingleUpdates() {
  TestServer server;
  brainstem::Client client(server.config());
  std::vector<brainstem::JointSample> received;

  std::unique_ptr<brainstem::Subscription> subscription = client.subscribeJoints(
      [&](const brainstem::JointSample &sample) { received.push_back(sample); });
  const brainstem::Result finished = subscription->wait();

  check(finished.ok && received.size() == 2,
        "joint subscription must deliver both protocol shapes");
  check(received[0].joints.size() == 16, "all-joints sample must preserve all 16 joints");
  check(received[0].joints.front().id == 0 && received[0].joints.back().id == 15,
        "all-joints sample must assign protocol-order ids");
  close(received[0].joints[15].position_rad, 15.1, "all-joints position");
  close(received[0].joints[15].velocity_rps, 15.2, "all-joints velocity");
  close(received[0].joints[15].torque_nm, 15.3, "all-joints torque");
  check(received[0].joints[15].status_code == 115, "all-joints status");
  check(received[0].elapsed == 5s, "all-joints timestamp");

  check(received[1].joints.size() == 1 && received[1].joints[0].id == 7,
        "single-joint update must preserve its id");
  close(received[1].joints[0].position_rad, -1.0, "single-joint position");
  close(received[1].joints[0].velocity_rps, -2.0, "single-joint velocity");
  close(received[1].joints[0].torque_nm, -3.0, "single-joint torque");
  check(received[1].joints[0].status_code == 707, "single-joint status");
  check(received[1].elapsed == 99ns, "single-joint timestamp");
}

void testManagedJointTelemetryMergesSingleJointUpdates() {
  TestServer server;
  brainstem::Client client(server.config());
  brainstem::TelemetryOptions options;
  options.imu = false;
  options.joints = true;
  options.body_state = false;

  const brainstem::Result started = client.startTelemetry(options);
  brainstem::TelemetrySnapshot telemetry;
  for (int attempt = 0; attempt < 100; ++attempt) {
    telemetry = client.telemetry();
    if (telemetry.joints.sequence >= 2) {
      break;
    }
    std::this_thread::sleep_for(1ms);
  }

  check(started.ok, "managed joint telemetry must receive its initial state");
  check(telemetry.joints.complete() && telemetry.joints.fresh(),
        "all 16 managed joints must be valid and fresh");
  check(telemetry.joints.sequence == 2,
        "full snapshot and single-joint update must both advance the sequence");
  close(telemetry.joints.joints[7].position_rad, -1.0, "single-joint update must replace its slot");
  close(telemetry.joints.joints[8].position_rad, 8.1,
        "single-joint update must preserve every other slot");
  check(telemetry.joints.elapsed[7] == 99ns, "single-joint update must replace its slot timestamp");
  check(telemetry.joints.elapsed[8] == 5s,
        "single-joint update must preserve every other slot timestamp");
}

void testBodyStateSubscriptionUsesTheSnapshotMapping() {
  TestServer server;
  brainstem::Client client(server.config());
  std::vector<brainstem::BodyState> received;

  std::unique_ptr<brainstem::Subscription> subscription = client.subscribeBodyState(
      [&](const brainstem::BodyState &state) { received.push_back(state); });
  const brainstem::Result finished = subscription->wait();

  check(finished.ok && received.size() == 2,
        "body-state subscription must deliver all state changes");
  check(received[0].kind == brainstem::BodyStateKind::Standing,
        "body-state subscription must map standing");
  check(received[1].kind == brainstem::BodyStateKind::Transitioning &&
            received[1].transition == brainstem::BodyTransition::SitDown,
        "body-state subscription must map transition intent");
}

void testManagedCmsTelemetryCachesTheLatestState() {
  TestServer server;
  brainstem::Client client(server.config());
  brainstem::TelemetryOptions options;
  options.imu = false;
  options.joints = false;
  options.body_state = true;

  const brainstem::Result started = client.startTelemetry(options);
  brainstem::TelemetrySnapshot telemetry;
  for (int attempt = 0; attempt < 100; ++attempt) {
    telemetry = client.telemetry();
    if (telemetry.body.sequence >= 2) {
      break;
    }
    std::this_thread::sleep_for(1ms);
  }

  check(started.ok, "managed body-state telemetry must receive its initial state");
  check(telemetry.body.available, "managed body-state telemetry must cache a state");
  check(telemetry.body.sequence == 2, "every body-state change must advance the sequence");
  check(telemetry.body.value.kind == brainstem::BodyStateKind::Transitioning &&
            telemetry.body.value.transition == brainstem::BodyTransition::SitDown,
        "managed body-state telemetry must expose the latest transition");
  check(!telemetry.imu.available && telemetry.joints.valid_mask == 0,
        "unrequested managed streams must remain empty");
}

void testManagedTelemetryMarksExpiredSamplesStale() {
  TestServer server;
  brainstem::Config config = server.config();
  config.telemetry_stale_after = 5ms;
  brainstem::Client client(std::move(config));
  brainstem::TelemetryOptions options;
  options.imu = true;
  options.joints = false;
  options.body_state = false;

  check(client.startTelemetry(options).ok, "managed IMU telemetry must receive an initial sample");
  std::this_thread::sleep_for(10ms);
  const brainstem::TelemetrySnapshot telemetry = client.telemetry();

  check(telemetry.imu.available, "an expired sample must remain readable");
  check(!telemetry.imu.fresh, "an expired sample must be marked stale");
  check(telemetry.imu.age >= 5ms, "expired sample age must be exposed");
}

void testStopTelemetryClearsManagedState() {
  TestServer server;
  brainstem::Client client(server.config());
  check(client.startTelemetry().ok, "all managed streams must receive initial data");

  client.stopTelemetry();
  const brainstem::TelemetrySnapshot telemetry = client.telemetry();

  check(!telemetry.imu.active && !telemetry.imu.available,
        "stopped IMU telemetry must be inactive and unavailable");
  check(!telemetry.joints.active && telemetry.joints.valid_mask == 0,
        "stopped joint telemetry must be inactive and invalid");
  check(!telemetry.body.active && !telemetry.body.available,
        "stopped body-state telemetry must be inactive and unavailable");
}

void testManagedTelemetryCanChangeSelection() {
  TestServer server;
  brainstem::Client client(server.config());
  brainstem::TelemetryOptions options;
  options.imu = true;
  options.joints = false;
  options.body_state = false;
  check(client.startTelemetry(options).ok, "initial IMU selection must start");

  options.imu = false;
  options.joints = true;
  check(client.startTelemetry(options).ok, "replacement joint selection must start");
  brainstem::TelemetrySnapshot telemetry;
  for (int attempt = 0; attempt < 100; ++attempt) {
    telemetry = client.telemetry();
    if (telemetry.joints.complete()) {
      break;
    }
    std::this_thread::sleep_for(1ms);
  }

  check(!telemetry.imu.active && !telemetry.imu.available,
        "changing selection must stop and invalidate IMU telemetry");
  check(telemetry.joints.complete(), "changing selection must populate requested joints");
}

void testTlsNameCannotSilentlyUseAnInsecureChannel() {
  brainstem::Config config;
  config.client_id = "tls-client@tests";
  config.tls.server_name = "brainstem.local";
  bool rejected = false;
  try {
    brainstem::Client client(config);
  } catch (const std::invalid_argument &) {
    rejected = true;
  }
  check(rejected, "TLS server_name without certificates must be rejected");
}

void testClientIdentityMustBeExplicit() {
  brainstem::Config config;
  config.allow_insecure = true;
  bool rejected = false;
  try {
    brainstem::Client client(config);
  } catch (const std::invalid_argument &) {
    rejected = true;
  }
  check(rejected, "an empty client identity must be rejected");
}

void testInsecureTransportRequiresExplicitOptIn() {
  brainstem::Config config;
  config.client_id = "local-client@tests";
  bool rejected = false;
  try {
    brainstem::Client client(config);
  } catch (const std::invalid_argument &) {
    rejected = true;
  }
  check(rejected, "plaintext transport must require explicit opt-in");
}

void testRefreshAcquiresLeaseAndAcknowledgesInitialZero() {
  TestServer server;
  brainstem::Client client(server.config("lingtu-driver@robot"));
  const brainstem::Result result = client.refresh();

  check(result.ok, "refresh must acquire a usable control lease");
  check(result.transport_ok && result.accepted, "lease must be explicitly accepted");
  check(result.state.ready, "lease is ready only after checked zero is acknowledged");
  check(result.state.initial_zero_acknowledged, "refresh must acknowledge an initial zero");
  check(result.state.control_owner == "grpc", "active lease must report the gRPC owner");
  check(result.state.control_owner_id == "lingtu-driver@robot",
        "active lease must preserve the control owner id");
  check(server.service().acquireCalls() == 1, "refresh must acquire exactly once");
  check(server.service().clientId() == "lingtu-driver@robot", "client id must be explicit");
  const auto velocities = server.service().velocities();
  check(velocities.size() == 1, "lease acquisition must send exactly one checked zero");
  close(velocities[0].vx_mps, 0.0, "initial zero vx");
  close(velocities[0].vy_mps, 0.0, "initial zero vy");
  close(velocities[0].yaw_rps, 0.0, "initial zero yaw");
}

void testMotorLifecycleClearsControlBeforeReenable() {
  TestServer server;
  brainstem::Client client(server.config());
  check(client.refresh().ok, "test lease must be ready");

  const brainstem::Result disabled = client.disableMotors();
  check(disabled.ok && disabled.transport_ok && disabled.accepted,
        "motor disable must report an accepted hardware action");
  check(!disabled.state.motors_enabled && !disabled.state.lease_valid && !disabled.state.ready,
        "motor disable must clear local control readiness");
  check(server.service().disableCalls() == 1, "motor disable must call the Server once");

  const brainstem::Result enabled = client.enableMotors();
  check(enabled.ok && enabled.accepted && enabled.state.motors_enabled,
        "motor enable must update the cached motor state");
  check(!enabled.state.lease_valid && !enabled.state.ready,
        "motor enable must not invent a locomotion lease");
  check(server.service().enableCalls() == 1, "motor enable must call the Server once");
}

void testJointZeroStopsLocomotionBeforeCalibration() {
  TestServer server;
  brainstem::Client client(server.config());
  check(client.refresh().ok, "test lease must be ready");

  const brainstem::Result result = client.setJointZero();

  check(result.ok && result.transport_ok && result.accepted,
        "joint zero must report an accepted calibration action");
  check(!result.state.lease_valid && !result.state.ready,
        "joint zero must not retain locomotion readiness");
  check(server.service().releaseCalls() == 1,
        "joint zero must confirm stop and release before calibration");
  check(server.service().setZeroCalls() == 1, "joint zero must call the Server once");
}

void testClearMotorFaultsTargetsRequestedJointsAndRevokesControl() {
  TestServer server;
  brainstem::Client client(server.config());
  check(client.refresh().ok, "test lease must be ready");

  const brainstem::Result result = client.clearMotorFaults({2, 15});

  check(result.ok && result.transport_ok && result.accepted,
        "motor fault clear must report an accepted diagnostic action");
  check(!result.state.lease_valid && !result.state.ready,
        "motor fault clear must invalidate locomotion control");
  check(server.service().clearFaultCalls() == 1, "motor fault clear must call the Server once");
  check(server.service().clearedJointIds() == std::vector<std::uint32_t>({2, 15}),
        "motor fault clear must preserve requested joint ids");
}

void testClearMotorFaultsRejectsInvalidJointBeforeIo() {
  TestServer server;
  brainstem::Client client(server.config());

  const brainstem::Result result = client.clearMotorFaults({16});

  check(!result.ok && !result.transport_ok && !result.accepted,
        "invalid fault-clear joint IDs must be rejected locally");
  check(result.state.reason == "invalid_joint_id", "invalid joint reason must be stable");
  check(server.service().clearFaultCalls() == 0,
        "invalid fault-clear joint IDs must not reach the Server");
}

void testMatchingClientIdDoesNotImplyLocalLeaseOwnership() {
  TestServer server;
  brainstem::Client owner(server.config("shared@tests"));
  check(owner.refresh().ok, "first client must own the test lease");

  brainstem::Client observer(server.config("shared@tests"));
  const brainstem::Result observed = observer.connect();
  check(observed.ok, "second client must still be able to read state");
  check(!observed.state.lease_valid,
        "matching client id without the token must not imply local lease ownership");
  check(observed.state.reason == "lease_token_missing",
        "missing local token must remain distinguishable from preemption");
  check(owner.stop().confirmsStop(), "test owner must release control");
}

void testMoveUsesPhysicalUnitsAndCheckedSequence() {
  TestServer server;
  brainstem::Client client(server.config());
  check(client.refresh().ok, "test lease must be ready");

  const brainstem::Result result = client.move({0.25, -0.5, 1.0});
  check(result.ok && result.accepted, "physical velocity must be accepted");
  const auto velocities = server.service().velocities();
  check(velocities.size() == 2, "move must follow the initial zero");
  close(velocities[1].vx_mps, 0.25, "forward velocity must remain m/s");
  close(velocities[1].vy_mps, -0.5, "lateral velocity must remain m/s");
  close(velocities[1].yaw_rps, 1.0, "yaw velocity must remain rad/s");
  check(server.service().commandSpacing(0, 1) >= 18ms,
        "checked commands must respect the server rate limit");
}

void testRefreshRenewsWithoutRepeatingInitialZero() {
  TestServer server;
  brainstem::Client client(server.config());
  check(client.refresh().ok, "initial refresh must acquire control");
  const brainstem::Result renewed = client.refresh();

  check(renewed.ok && renewed.state.ready, "second refresh must renew the live lease");
  check(server.service().acquireCalls() == 1, "renewal must not reacquire control");
  check(server.service().renewCalls() == 1, "second refresh must renew exactly once");
  check(server.service().velocities().size() == 1,
        "renewal must not repeat an already acknowledged initial zero");
}

void testInvalidVelocityIsRejectedBeforeIo() {
  TestServer server;
  brainstem::Client client(server.config());
  check(client.refresh().ok, "test lease must be ready");
  const auto before = server.service().velocities().size();

  const brainstem::Result result = client.move({std::numeric_limits<double>::infinity(), 0.0, 0.0});
  check(!result.ok && !result.transport_ok && !result.accepted,
        "non-finite velocity must be a local rejection");
  check(result.state.reason == "invalid_velocity", "invalid velocity reason must be exact");
  check(server.service().velocities().size() == before,
        "invalid velocity must not issue a Brainstem RPC");
}

void testAckMismatchRecoversWhilePreemptionInvalidatesLease() {
  {
    TestServer server;
    brainstem::Client client(server.config());
    check(client.refresh().ok, "test lease must be ready");
    server.service().mismatchNextAck();
    const brainstem::Result mismatch = client.move({});
    check(!mismatch.ok && mismatch.transport_ok && !mismatch.accepted,
          "mismatched ACK must fail closed without pretending transport loss");
    check(mismatch.state.reason == "ack_sequence_mismatch", "ACK mismatch reason must be exact");
    check(client.stop().confirmsStop(),
          "ACK mismatch must retain only the recovery path needed for a confirmed stop");
  }
  {
    TestServer server;
    brainstem::Client client(server.config());
    check(client.refresh().ok, "test lease must be ready");
    server.service().rejectNextCommand();
    const brainstem::Result rejected = client.move({});
    check(!rejected.ok && rejected.transport_ok && !rejected.accepted,
          "preempted command must fail closed");
    check(rejected.state.reason == "preempted", "server rejection reason must be preserved");
    check(!rejected.state.lease_valid, "preemption must invalidate the local lease");
    check(rejected.state.control_owner == "yunzhuo",
          "preemption must expose the new control owner");
  }
}

void testRejectedCommandRetainsAConfirmedLeaseForStop() {
  TestServer server;
  brainstem::Client client(server.config());
  check(client.refresh().ok, "test lease must be ready");
  check(client.move({0.4, 0.0, 0.0}).ok, "test motion must be accepted");
  server.service().rejectNextCommandPreservingLease();

  const brainstem::Result rejected = client.move({0.2, 0.0, 0.0});
  check(!rejected.ok && rejected.transport_ok && !rejected.accepted,
        "a server rejection must remain visible");
  check(rejected.state.lease_valid,
        "a rejection that confirms ownership must retain the recovery lease");
  check(!rejected.state.initial_zero_acknowledged,
        "a rejected command must require a safety zero before more motion");
  check(client.stop().confirmsStop(),
        "the retained recovery lease must allow checked zero and release");

  const auto sequences = server.service().walkRequestSequences();
  check(sequences.size() == 4, "recovery stop must issue one checked zero");
  check(sequences[2] == sequences[3],
        "recovery stop must continue from the last accepted server sequence");
}

void testLostWalkAckRetriesTheSameCommandOnce() {
  TestServer server;
  brainstem::Client client(server.config());
  check(client.refresh().ok, "test lease must be ready");
  server.service().dropNextAck();

  const brainstem::Result result = client.move({0.3, -0.1, 0.2});
  check(result.ok && result.accepted, "idempotent retry must recover a lost WalkChecked ACK");
  const auto sequences = server.service().walkRequestSequences();
  check(sequences.size() == 3, "lost ACK must issue exactly one retry after initial zero");
  check(sequences[1] == sequences[2], "transport retry must preserve the command sequence");
  check(server.service().velocities().size() == 2,
        "server-side retry cache must prevent duplicate motion dispatch");
}

void testAmbiguousWalkFailureCanResynchronizeAndStop() {
  TestServer server;
  brainstem::Client client(server.config());
  check(client.refresh().ok, "test lease must be ready");
  server.service().dropNextTwoAcks();

  const brainstem::Result ambiguous = client.move({0.3, 0.0, 0.0});
  check(!ambiguous.ok && !ambiguous.transport_ok && !ambiguous.accepted,
        "two lost ACKs must remain a transport failure");
  check(!ambiguous.state.ready && !ambiguous.state.lease_valid,
        "ambiguous transport state must block further nonzero motion");
  check(client.stop().confirmsStop(),
        "stop must query the accepted sequence, send zero, and release after ambiguity");

  const auto sequences = server.service().walkRequestSequences();
  check(sequences.size() == 4, "ambiguous command and stop sequence count must be exact");
  check(sequences[1] == sequences[2], "the ambiguous command retry must reuse its sequence");
  check(sequences[3] == sequences[1] + 1,
        "stop must continue after the server's last accepted sequence");
  check(server.service().velocities().size() == 3,
        "lost ACK retries must not duplicate motor dispatch");
}

void testStopRequiresZeroAndReleaseConfirmation() {
  TestServer server;
  brainstem::Client client(server.config());
  check(client.refresh().ok, "test lease must be ready");
  check(client.move({0.4, 0.0, 0.0}).ok, "test motion must be accepted");

  const brainstem::Result result = client.stop();
  check(result.confirmsStop(), "stop must confirm checked zero and lease release");
  check(result.state.reason == "stop_confirmed", "stop reason must be explicit");
  check(server.service().releaseCalls() == 1, "stop must release control exactly once");
  const auto velocities = server.service().velocities();
  check(velocities.size() == 3, "stop must send a checked zero after motion");
  close(velocities.back().vx_mps, 0.0, "stop vx");
  close(velocities.back().vy_mps, 0.0, "stop vy");
  close(velocities.back().yaw_rps, 0.0, "stop yaw");
}

void testReleaseTransportFailureIsVisible() {
  TestServer server;
  brainstem::Client client(server.config());
  check(client.refresh().ok, "test lease must be ready");
  server.service().failNextRelease();

  const brainstem::Result result = client.stop();
  check(!result.ok && !result.transport_ok && !result.accepted,
        "release transport failure must not confirm stop");
  check(!result.confirmsStop(), "failed release cannot confirm stop");
  check(result.state.reason == "release_transport_error", "release failure reason must be exact");
  check(result.error.find("ReleaseControl") != std::string::npos,
        "release failure must name the RPC");
  check(client.stop().confirmsStop(),
        "a release transport failure must retain the token for a confirmed retry");
}

void testStandAndSitStopLocomotionBeforeTheAction() {
  TestServer server;
  brainstem::Client client(server.config());
  check(client.refresh().ok, "test lease must be ready");

  const brainstem::Result sit = client.sitDown();
  check(sit.ok && sit.transport_ok && sit.accepted, "SitDown must be accepted");
  check(server.service().releaseCalls() == 1, "SitDown must stop and release locomotion first");
  check(server.service().sitCalls() == 1, "SitDown must call the action RPC once");
  check(sit.state.fsm == "transitioning", "accepted SitDown must report transition intent");
  check(sit.state.reason == "body_action_accepted:sit_down",
        "SitDown acceptance reason must be explicit");

  const brainstem::Result stand = client.standUp();
  check(stand.ok && stand.accepted, "StandUp must be accepted without a locomotion lease");
  check(server.service().standCalls() == 1, "StandUp must call the action RPC once");
  check(server.service().releaseCalls() == 1, "StandUp must not release an absent lease");
}

void testBodyActionDoesNotRunAfterUnconfirmedStop() {
  TestServer server;
  brainstem::Client client(server.config());
  check(client.refresh().ok, "test lease must be ready");
  server.service().failNextRelease();

  const brainstem::Result result = client.sitDown();
  check(!result.ok, "body action must fail when stop cannot be confirmed");
  check(server.service().sitCalls() == 0, "unconfirmed stop must suppress SitDown RPC");
}

void testOldServerFailsClosedWithProtocolReason() {
  LegacyService service;
  grpc::ServerBuilder builder;
  int port = 0;
  builder.AddListeningPort("127.0.0.1:0", grpc::InsecureServerCredentials(), &port);
  builder.RegisterService(&service);
  std::unique_ptr<grpc::Server> server = builder.BuildAndStart();
  check(server != nullptr && port > 0, "legacy fake server must start");

  brainstem::Config config;
  config.target = "127.0.0.1:" + std::to_string(port);
  config.timeout = 500ms;
  config.client_id = "legacy@tests";
  config.allow_insecure = true;
  brainstem::Client client(config);
  const brainstem::Result result = client.refresh();
  check(!result.ok && result.transport_ok && !result.accepted,
        "server without lease RPCs must fail closed");
  check(result.state.reason == "protocol_incompatible",
        "missing required RPC must report protocol incompatibility");
  check(result.error.find("AcquireControl") != std::string::npos,
        "protocol failure must name the missing RPC");

  const brainstem::Response<brainstem::BodyState> cms = client.getBodyState();
  check(!cms.result.ok && cms.result.transport_ok,
        "missing telemetry RPC must be a protocol failure, not transport loss");
  check(cms.result.state.reason == "protocol_incompatible",
        "telemetry failure must preserve protocol incompatibility");

  std::unique_ptr<brainstem::Subscription> imu =
      client.subscribeImu([](const brainstem::ImuSample &) {});
  const brainstem::Result stream = imu->wait();
  check(!stream.ok && stream.transport_ok && stream.state.reason == "protocol_incompatible",
        "missing stream RPC must preserve protocol incompatibility");

  server->Shutdown();
  server->Wait();
}

#ifndef _WIN32
void testMutualTlsControlPath() {
  char directory_template[] = "/tmp/brainstem_client_tls_XXXXXX";
  const char *created = mkdtemp(directory_template);
  check(created != nullptr, "TLS fixture directory must be created");
  const std::filesystem::path directory(created);
  const auto openssl_config = directory / "openssl.cnf";
  const auto certificate = directory / "peer.crt";
  const auto private_key = directory / "peer.key";
  {
    std::ofstream output(openssl_config);
    output << "[req]\ndistinguished_name=dn\n[dn]\n";
  }
  const std::string command =
      "openssl req -x509 -newkey rsa:2048 -nodes -days 1 -config " + openssl_config.string() +
      " -subj /CN=localhost -addext subjectAltName=DNS:localhost,IP:127.0.0.1 -keyout " +
      private_key.string() + " -out " + certificate.string() + " >/dev/null 2>&1";
  check(std::system(command.c_str()) == 0, "openssl TLS fixture generation must succeed");

  ControlService service;
  grpc::SslServerCredentialsOptions options(
      GRPC_SSL_REQUEST_AND_REQUIRE_CLIENT_CERTIFICATE_AND_VERIFY);
  options.pem_root_certs = readTextFile(certificate);
  options.pem_key_cert_pairs.push_back({readTextFile(private_key), readTextFile(certificate)});
  grpc::ServerBuilder builder;
  int port = 0;
  builder.AddListeningPort("127.0.0.1:0", grpc::SslServerCredentials(options), &port);
  builder.RegisterService(&service);
  std::unique_ptr<grpc::Server> server = builder.BuildAndStart();
  check(server != nullptr && port > 0, "mTLS Brainstem server must start");

  brainstem::Config config;
  config.target = "127.0.0.1:" + std::to_string(port);
  config.timeout = 1000ms;
  config.client_id = "tls-client@tests";
  config.tls.ca_file = certificate.string();
  config.tls.certificate_file = certificate.string();
  config.tls.private_key_file = private_key.string();
  config.tls.server_name = "localhost";
  brainstem::Client client(config);
  const brainstem::Result ready = client.refresh();
  check(ready.ok && ready.state.initial_zero_acknowledged,
        "mTLS path must acquire and acknowledge initial zero");
  check(client.stop().confirmsStop(), "mTLS path must confirm stop and release");

  server->Shutdown();
  server->Wait();
  std::error_code remove_error;
  std::filesystem::remove_all(directory, remove_error);
}
#endif

}  // namespace

int main() {
  try {
    testConnectReadsServerState();
    testBodyStateSnapshotUsesPublicTypes();
    testServerStartTimePreservesUtcPrecision();
    testMotorStatusSnapshotPreservesDiagnostics();
    testVoltageSnapshotPreservesJointOrder();
    testImuSubscriptionMapsOneCompleteSample();
    testImuSubscriptionAcceptsLegacySampleWithoutAcceleration();
    testManagedTelemetryStartsOnlyRequestedStreams();
    testSubscriptionCancellationStopsAStreamingRpc();
    testJointSubscriptionMapsSnapshotsAndSingleUpdates();
    testManagedJointTelemetryMergesSingleJointUpdates();
    testBodyStateSubscriptionUsesTheSnapshotMapping();
    testManagedCmsTelemetryCachesTheLatestState();
    testManagedTelemetryMarksExpiredSamplesStale();
    testStopTelemetryClearsManagedState();
    testManagedTelemetryCanChangeSelection();
    testTlsNameCannotSilentlyUseAnInsecureChannel();
    testClientIdentityMustBeExplicit();
    testInsecureTransportRequiresExplicitOptIn();
    testRefreshAcquiresLeaseAndAcknowledgesInitialZero();
    testMotorLifecycleClearsControlBeforeReenable();
    testJointZeroStopsLocomotionBeforeCalibration();
    testClearMotorFaultsTargetsRequestedJointsAndRevokesControl();
    testClearMotorFaultsRejectsInvalidJointBeforeIo();
    testMatchingClientIdDoesNotImplyLocalLeaseOwnership();
    testMoveUsesPhysicalUnitsAndCheckedSequence();
    testRefreshRenewsWithoutRepeatingInitialZero();
    testInvalidVelocityIsRejectedBeforeIo();
    testAckMismatchRecoversWhilePreemptionInvalidatesLease();
    testRejectedCommandRetainsAConfirmedLeaseForStop();
    testLostWalkAckRetriesTheSameCommandOnce();
    testAmbiguousWalkFailureCanResynchronizeAndStop();
    testStopRequiresZeroAndReleaseConfirmation();
    testReleaseTransportFailureIsVisible();
    testStandAndSitStopLocomotionBeforeTheAction();
    testBodyActionDoesNotRunAfterUnconfirmedStop();
    testOldServerFailsClosedWithProtocolReason();
#ifndef _WIN32
    testMutualTlsControlPath();
#endif
    return 0;
  } catch (const std::exception &error) {
    std::fprintf(stderr, "brainstem_client_test: FAIL: %s\n", error.what());
    return 1;
  }
}
