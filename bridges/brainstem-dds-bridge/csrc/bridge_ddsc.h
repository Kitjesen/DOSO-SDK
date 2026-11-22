#ifndef BRIDGE_DDSC_H
#define BRIDGE_DDSC_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct BridgeDdsHandle BridgeDdsHandle;
typedef struct BridgeDdsTestPeer BridgeDdsTestPeer;

typedef struct BridgeImuMsg {
  int32_t sec;
  uint32_t nanosec;
  double qx;
  double qy;
  double qz;
  double qw;
  double gx;
  double gy;
  double gz;
  double ax;
  double ay;
  double az;
} BridgeImuMsg;

typedef struct BridgeJointStateMsg {
  int32_t sec;
  uint32_t nanosec;
  double position[16];
  double velocity[16];
  double effort[16];
} BridgeJointStateMsg;

typedef struct BridgeControlStateMsg {
  int32_t sec;
  uint32_t nanosec;
  uint8_t connected;
  uint8_t ready;
  uint8_t motors_enabled;
  uint8_t critical_fault;
  uint8_t control_assured;
  uint8_t lease_valid;
  uint32_t lease_remaining_ms;
  uint64_t accepted_sequence;
  uint8_t zero_output;
  char fsm[64];
  char owner[64];
  char owner_id[128];
  char reason[128];
} BridgeControlStateMsg;

typedef struct BridgeProfileStateMsg {
  int32_t sec;
  uint32_t nanosec;
  char task_id[128];
  char requirement[128];
  char phase[64];
  char requested_profile[128];
  char active_profile[128];
  char reason[128];
} BridgeProfileStateMsg;

typedef struct BridgePostureRequestMsg {
  char host_boot_id[128];
  char producer_boot_id[128];
  uint64_t source_boottime_ns;
  char client_id[128];
  char request_id[128];
  int32_t action;
  int32_t phase;
  char profile_name[128];
  char external_session_id[128];
} BridgePostureRequestMsg;

typedef struct BridgePostureAckMsg {
  int32_t sec;
  uint32_t nanosec;
  char producer_boot_id[128];
  char client_id[128];
  char request_id[128];
  int32_t action;
  int32_t phase;
  char profile_name[128];
  char external_session_id[128];
  uint8_t final_result;
  uint8_t accepted;
  char reason[128];
} BridgePostureAckMsg;

typedef struct BridgeProfileRequestMsg {
  char task_id[128];
  char requirement[128];
} BridgeProfileRequestMsg;

typedef struct BridgeCmdVelSample {
  char host_boot_id[128];
  char producer_boot_id[128];
  uint64_t output_seq;
  uint64_t source_boottime_ns;
  uint64_t source_wall_ns;
  double vx;
  double vy;
  double wz;
  uint32_t valid_samples;
} BridgeCmdVelSample;

BridgeDdsHandle* bridge_dds_create(int32_t domain_id);
void bridge_dds_destroy(BridgeDdsHandle* handle);

int32_t bridge_dds_matched_cmd_vel_writers(const BridgeDdsHandle* handle);
int32_t bridge_dds_take_cmd_vel(const BridgeDdsHandle* handle, BridgeCmdVelSample* out_latest);
int32_t bridge_dds_take_posture_request(const BridgeDdsHandle* handle,
                                        BridgePostureRequestMsg* out_req);
int32_t bridge_dds_take_profile_request(const BridgeDdsHandle* handle,
                                        BridgeProfileRequestMsg* out_req);

int32_t bridge_dds_write_imu_body(const BridgeDdsHandle* handle, const BridgeImuMsg* msg);
int32_t bridge_dds_write_joint_states(const BridgeDdsHandle* handle, const char* robot_model,
                                      const char* const* names16,
                                      const BridgeJointStateMsg* msg);
int32_t bridge_dds_write_control_state(const BridgeDdsHandle* handle,
                                       const BridgeControlStateMsg* msg);
int32_t bridge_dds_write_profile_state(const BridgeDdsHandle* handle,
                                       const BridgeProfileStateMsg* msg);
int32_t bridge_dds_write_posture_ack(const BridgeDdsHandle* handle,
                                     const BridgePostureAckMsg* msg);

/* Independent second-participant peer helpers for end-to-end CycloneDDS testing. */
BridgeDdsTestPeer* bridge_dds_test_peer_create(int32_t domain_id);
void bridge_dds_test_peer_destroy(BridgeDdsTestPeer* peer);
int32_t bridge_dds_test_peer_matched_bridge_readers(const BridgeDdsTestPeer* peer);
int32_t bridge_dds_test_peer_write_cmd_vel(const BridgeDdsTestPeer* peer,
                                           const BridgeCmdVelSample* msg);
int32_t bridge_dds_test_peer_write_posture_request(const BridgeDdsTestPeer* peer,
                                                   const BridgePostureRequestMsg* msg);
int32_t bridge_dds_test_peer_take_imu_body(const BridgeDdsTestPeer* peer, BridgeImuMsg* out_msg);
int32_t bridge_dds_test_peer_take_joint_states(const BridgeDdsTestPeer* peer,
                                               BridgeJointStateMsg* out_msg, uint32_t* out_len);
int32_t bridge_dds_test_peer_take_control_state(const BridgeDdsTestPeer* peer,
                                                BridgeControlStateMsg* out_msg);
int32_t bridge_dds_test_peer_take_posture_ack(const BridgeDdsTestPeer* peer,
                                              BridgePostureAckMsg* out_msg);

#ifdef __cplusplus
}
#endif

#endif /* BRIDGE_DDSC_H */
