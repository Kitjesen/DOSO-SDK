#ifndef DOSO_SHM_IPC_H
#define DOSO_SHM_IPC_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#if defined(_WIN32)
  #ifdef DOSO_SHM_IPC_EXPORTS
    #define DOSO_SHM_API __declspec(dllexport)
  #else
    #define DOSO_SHM_API
  #endif
#else
  #define DOSO_SHM_API __attribute__((visibility("default")))
#endif

#define DOSO_SHM_MAGIC          0x4F534F44u /* "DOSO" LE */
#define DOSO_SHM_VERSION        1u
#define DOSO_SHM_JOINTS         16u
#define DOSO_SHM_PROPRIO_SLOTS  64u
#define DOSO_SHM_CMD_SLOTS      4u

/*
 * ABI-frozen Board-A proprioceptive frame (IMU + 16 canonical joints + control state).
 * Canonical joint order (0..15):
 *   0..2: FR(hip,thigh,calf), 3..5: FL, 6..8: RR, 9..11: RL, 12..15: FR/FL/RR/RL foot wheels.
 */
typedef struct DosoProprioPayload {
  uint64_t sequence;
  uint64_t monotonic_us;
  double imu_quat_wxyz[4];
  double imu_gyro_rad_s[3];
  double imu_accel_m_s2[3];
  double joint_q[DOSO_SHM_JOINTS];
  double joint_dq[DOSO_SHM_JOINTS];
  double joint_tau[DOSO_SHM_JOINTS];
  float joint_temp_c[DOSO_SHM_JOINTS];
  uint16_t joint_status[DOSO_SHM_JOINTS];
  uint16_t joint_online_mask;
  uint8_t imu_fresh;
  uint8_t cms_state;             /* 0=zero,1=grounded,2=standing,3=walking,4=transitioning,5=damping */
  uint8_t control_owner;         /* 0=none,1=grpc,2=yunzhuo */
  uint8_t motor_output_enabled;
  uint8_t critical_motor_fault;
  uint8_t _reserved[9];
} DosoProprioPayload;

/*
 * ABI-frozen 200Hz-1kHz external MIT command frame (data plane only; control plane
 * Enter/Enable/Disable/Exit and input_window authorization stay on gRPC).
 */
typedef struct DosoExternalCmdPayload {
  uint64_t session_id_hash;      /* FNV-1a 64-bit hash of active gRPC session_id */
  uint64_t input_window;         /* Active window granted by Brainstem ExternalRuntime */
  uint64_t sequence;             /* Strictly monotonic frame sequence */
  uint64_t monotonic_us;         /* Producer monotonic timestamp (us) for freshness check */
  double q[DOSO_SHM_JOINTS];
  double dq[DOSO_SHM_JOINTS];
  double kp[DOSO_SHM_JOINTS];
  double kd[DOSO_SHM_JOINTS];
  double tau[DOSO_SHM_JOINTS];
} DosoExternalCmdPayload;

typedef struct DosoProprioShm DosoProprioShm;
typedef struct DosoExternalCmdShm DosoExternalCmdShm;

/*
 * Return codes:
 *   0 = OK / new frame read
 *   1 = No data yet / unchanged sequence (not an error)
 *  <0 = Invalid argument, OS error, or version/magic mismatch
 */

/* --- Proprioception SPMC Lock-Free Seqlock RingBuffer --- */
DOSO_SHM_API DosoProprioShm* doso_shm_proprio_open_writer(const char* name_or_path);
DOSO_SHM_API DosoProprioShm* doso_shm_proprio_open_reader(const char* name_or_path);
DOSO_SHM_API void doso_shm_proprio_close(DosoProprioShm* handle);
DOSO_SHM_API int32_t doso_shm_proprio_publish(
    DosoProprioShm* writer,
    const DosoProprioPayload* payload);
DOSO_SHM_API int32_t doso_shm_proprio_read_latest(
    const DosoProprioShm* reader,
    uint64_t last_seen_seq,
    DosoProprioPayload* out_payload);
DOSO_SHM_API uint64_t doso_shm_proprio_write_seq(const DosoProprioShm* handle);

/* --- External MIT Command SPSC Lock-Free Latest-Wins RingBuffer --- */
DOSO_SHM_API DosoExternalCmdShm* doso_shm_cmd_open_writer(const char* name_or_path);
DOSO_SHM_API DosoExternalCmdShm* doso_shm_cmd_open_reader(const char* name_or_path);
DOSO_SHM_API void doso_shm_cmd_close(DosoExternalCmdShm* handle);
DOSO_SHM_API int32_t doso_shm_cmd_publish(
    DosoExternalCmdShm* writer,
    const DosoExternalCmdPayload* payload);
DOSO_SHM_API int32_t doso_shm_cmd_read_latest(
    const DosoExternalCmdShm* reader,
    uint64_t last_seen_seq,
    DosoExternalCmdPayload* out_payload);
DOSO_SHM_API uint64_t doso_shm_cmd_write_seq(const DosoExternalCmdShm* handle);

/* Helper: FNV-1a 64-bit hash for session_id binding */
DOSO_SHM_API uint64_t doso_shm_fnv1a_64(const uint8_t* data, size_t len);

#ifdef __cplusplus
}
#endif

#endif /* DOSO_SHM_IPC_H */
