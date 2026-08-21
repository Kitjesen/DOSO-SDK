#include <brainstem/client.hpp>

int main() {
  brainstem::ImuSample imu;
  brainstem::JointSample joints;
  brainstem::BodyState body;
  brainstem::MotorStatus motors;
  brainstem::Voltage voltage;
  brainstem::TelemetryOptions telemetry_options;
  brainstem::TelemetrySnapshot telemetry;
  brainstem::Velocity velocity;
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
  velocity.yaw_rad_s = 0.1;
  imu.angular_velocity_rad_s = {};
  imu.linear_acceleration_m_s2 = {};
  imu.orientation_world_to_body = {};
  joints.server_elapsed = {};
  (void)velocity;
  return client.controlState().connected ? 1 : 0;
}
