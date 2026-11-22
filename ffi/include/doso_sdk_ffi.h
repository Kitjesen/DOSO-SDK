#ifndef DOSO_SDK_FFI_H
#define DOSO_SDK_FFI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct DosoRobot DosoRobot;
typedef struct DosoTeleop DosoTeleop;
typedef struct DosoLowLevel DosoLowLevel;
typedef struct DosoTelemetry DosoTelemetry;
typedef struct DosoMockServer DosoMockServer;

typedef struct DosoCapabilities {
  uint8_t motion;
  uint8_t low_level;
  uint8_t observations;
  uint8_t navigation;
  uint8_t maps;
  uint8_t localization;
  uint8_t tasks;
  uint8_t board_b_autonomy;
} DosoCapabilities;

typedef struct DosoBodyControlStatus {
  int32_t body_kind;
  int32_t body_transition;
  int32_t source_owner;
  uint8_t motor_output_enabled;
  uint8_t critical_motor_fault;
  uint8_t grpc_lease_active;
  uint8_t source_ready_for_walk;
  uint32_t lease_remaining_ms;
  uint32_t _pad;
  uint64_t last_accepted_sequence;
  uint64_t accepted_count;
  uint64_t rejected_count;
} DosoBodyControlStatus;

typedef struct DosoImuSnapshot {
  uint8_t available;
  uint8_t fresh;
  uint8_t stream_active;
  uint8_t _pad[5];
  double age_ms;
  double gyro_rad_s[3];
  double accel_m_s2[3];
  double quat_wxyz[4];
  int64_t source_elapsed_ns;
} DosoImuSnapshot;

typedef struct DosoJointsSnapshot {
  uint8_t available;
  uint8_t complete;
  uint8_t fresh;
  uint8_t stream_active;
  uint16_t valid_mask;
  uint16_t _pad;
  double age_ms;
  double q[16];
  double dq[16];
  double tau[16];
  uint32_t status_code[16];
  int64_t source_elapsed_ns;
} DosoJointsSnapshot;

typedef struct DosoMotorInfo {
  uint32_t id;
  uint8_t online;
  uint8_t _pad[3];
  uint32_t status_code;
  uint32_t error_count;
  double temperature_c;
} DosoMotorInfo;

typedef struct DosoMotorsSnapshot {
  uint32_t count;
  uint32_t _pad;
  DosoMotorInfo motors[16];
} DosoMotorsSnapshot;

typedef struct DosoJointCmd {
  double q;
  double dq;
  double kp;
  double kd;
  double tau;
} DosoJointCmd;

typedef struct DosoControllerSnapshot {
  int32_t mode;
  uint8_t active;
  uint8_t motor_output_enabled;
  uint8_t fresh;
  uint8_t remote_active;
  uint64_t last_sequence;
  uint32_t joint_count;
  uint32_t remote_buttons;
  double remote_lx;
  double remote_ly;
  double remote_rx;
  double remote_ry;
} DosoControllerSnapshot;

/* Thread-local error message from the last failed call on the current thread. */
size_t doso_last_error(char* buf, size_t cap);

/* Connection & Identity */
DosoRobot* doso_robot_connect(const char* endpoint, const char* api_key, uint32_t timeout_ms);
void doso_robot_free(DosoRobot* robot);
int32_t doso_robot_ping(const DosoRobot* robot);
size_t doso_robot_id(const DosoRobot* robot, char* buf, size_t cap);
size_t doso_firmware_version(const DosoRobot* robot, char* buf, size_t cap);
size_t doso_robot_version(const DosoRobot* robot, char* buf, size_t cap);
size_t doso_low_level_endpoint(const DosoRobot* robot, char* buf, size_t cap);
int32_t doso_robot_capabilities(const DosoRobot* robot, DosoCapabilities* out_caps);

/* Sport / Posture / Fault / Profile */
int32_t doso_sport_enable_motors(const DosoRobot* robot);
int32_t doso_sport_disable_motors(const DosoRobot* robot);
int32_t doso_sport_stand_up(const DosoRobot* robot);
int32_t doso_sport_stand_down(const DosoRobot* robot);
int32_t doso_sport_damp(const DosoRobot* robot);
int32_t doso_sport_stop(const DosoRobot* robot);
int32_t doso_sport_switch_profile(const DosoRobot* robot, const char* name);
int32_t doso_sport_play_gesture(const DosoRobot* robot, const char* name);
int32_t doso_sport_clear_motor_fault(const DosoRobot* robot);
int32_t doso_sport_clear_joint_fault(const DosoRobot* robot, uint32_t joint_id);
int32_t doso_sport_control_status(const DosoRobot* robot, DosoBodyControlStatus* out_status);
int32_t doso_sport_current_profile(const DosoRobot* robot, char* buf, size_t cap);
int32_t doso_sport_list_profiles(const DosoRobot* robot, char* buf, size_t cap);
int32_t doso_sport_list_gestures(const DosoRobot* robot, char* buf, size_t cap);

/* Observations: Managed Telemetry Streams + One-Shot Diagnostics */
DosoTelemetry* doso_observations_start_telemetry(const DosoRobot* robot, uint8_t imu,
                                                 uint8_t joints, uint32_t stale_after_ms);
int32_t doso_observations_latest_imu(const DosoTelemetry* telemetry, DosoImuSnapshot* out_imu);
int32_t doso_observations_latest_joints(const DosoTelemetry* telemetry,
                                        DosoJointsSnapshot* out_joints);
void doso_observations_stop_telemetry(DosoTelemetry* telemetry);
int32_t doso_observations_motors(const DosoRobot* robot, DosoMotorsSnapshot* out_motors);
int32_t doso_observations_motor_voltages(const DosoRobot* robot, double* out_volts16,
                                         uint32_t* out_count);

/* Teleop / Control Lease Session */
DosoTeleop* doso_teleop_take_control(const DosoRobot* robot);
int32_t doso_teleop_send_velocity(DosoTeleop* teleop, double vx_mps, double vy_mps, double yaw_rps,
                                    uint8_t deadman);
int32_t doso_teleop_publish_velocity(DosoTeleop* teleop, double vx_mps, double vy_mps, double yaw_rps,
                                       uint8_t deadman);
int32_t doso_teleop_release(DosoTeleop* teleop);
void doso_teleop_free(DosoTeleop* teleop);

/* Low-Level 16-axis MIT RL Control */
DosoLowLevel* doso_low_level_enter(const DosoRobot* robot);
int32_t doso_low_level_enable(DosoLowLevel* session);
int32_t doso_low_level_disable(DosoLowLevel* session);
int32_t doso_low_level_publish_joints16(DosoLowLevel* session, const DosoJointCmd* joints16);
int32_t doso_low_level_get_state(const DosoLowLevel* session, DosoControllerSnapshot* out_state);
int32_t doso_low_level_close(DosoLowLevel* session);
void doso_low_level_free(DosoLowLevel* session);

/* Navigation */
int32_t doso_nav_go_to(const DosoRobot* robot, double x, double y, double z, double yaw,
                       char* task_id_buf, size_t task_id_cap);
int32_t doso_nav_cancel(const DosoRobot* robot, const char* task_id, const char* reason);

/* Optional test mock server (available when built with `test-mock` feature). */
DosoMockServer* doso_test_mock_server_start(char* endpoint_buf, size_t endpoint_cap);
uint64_t doso_test_mock_server_writes(const DosoMockServer* server);
void doso_test_mock_server_stop(DosoMockServer* server);

#ifdef __cplusplus
}
#endif

#endif /* DOSO_SDK_FFI_H */
