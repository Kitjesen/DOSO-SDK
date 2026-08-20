#include <brainstem/client.hpp>

int main() {
  brainstem::ImuSample imu;
  brainstem::JointSample joints;
  brainstem::BodyState body;
  brainstem::MotorStatus motors;
  brainstem::Voltage voltage;
  brainstem::TelemetryOptions telemetry_options;
  brainstem::TelemetrySnapshot telemetry;
  brainstem::Config config;
  config.target = "127.0.0.1:13145";
  config.client_id = "consumer@tests";
  config.allow_insecure = true;
  brainstem::Client client(config);
  (void)imu;
  (void)joints;
  (void)body;
  (void)motors;
  (void)voltage;
  (void)telemetry_options;
  (void)telemetry;
  return client.state().connected ? 1 : 0;
}
