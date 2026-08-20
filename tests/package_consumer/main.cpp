#include <brainstem/client.hpp>

int main() {
  brainstem::ImuSample imu;
  brainstem::JointSample joints;
  brainstem::CmsState cms;
  brainstem::MotorStatus motors;
  brainstem::Voltage voltage;
  brainstem::Config config;
  config.target = "127.0.0.1:13145";
  config.client_id = "consumer@tests";
  config.allow_insecure = true;
  brainstem::Client client(config);
  (void)imu;
  (void)joints;
  (void)cms;
  (void)motors;
  (void)voltage;
  return client.state().connected ? 1 : 0;
}
