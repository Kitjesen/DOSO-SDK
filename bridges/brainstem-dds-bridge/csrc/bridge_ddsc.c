#include "bridge_ddsc.h"

#include "bridge_messages.h"
#include "dds/dds.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef enum BridgeQosKind {
  BRIDGE_QOS_SENSOR_STREAM = 0,
  BRIDGE_QOS_SYSTEM_STATUS = 1,
  BRIDGE_QOS_CONTROL_COMMAND = 2,
  BRIDGE_QOS_FINAL_VELOCITY = 3,
} BridgeQosKind;

struct BridgeDdsHandle {
  dds_entity_t participant;
  dds_entity_t subscriber;
  dds_entity_t publisher;
  dds_entity_t imu_body_writer;
  dds_entity_t joint_state_writer;
  dds_entity_t control_state_writer;
  dds_entity_t profile_state_writer;
  dds_entity_t posture_ack_writer;
  dds_entity_t cmd_vel_reader;
  dds_entity_t posture_request_reader;
  dds_entity_t profile_request_reader;
};

struct BridgeDdsTestPeer {
  dds_entity_t participant;
  dds_entity_t subscriber;
  dds_entity_t publisher;
  dds_entity_t cmd_vel_writer;
  dds_entity_t posture_request_writer;
  dds_entity_t imu_body_reader;
  dds_entity_t joint_state_reader;
  dds_entity_t control_state_reader;
  dds_entity_t posture_ack_reader;
};

static void copy_cstr(char* dst, size_t cap, const char* src) {
  if (!dst || cap == 0) return;
  if (!src) {
    dst[0] = '\0';
    return;
  }
  snprintf(dst, cap, "%s", src);
}

static dds_qos_t* make_bridge_qos(BridgeQosKind kind) {
  dds_qos_t* qos = dds_create_qos();
  if (!qos) return NULL;
  switch (kind) {
    case BRIDGE_QOS_SENSOR_STREAM:
      dds_qset_reliability(qos, DDS_RELIABILITY_BEST_EFFORT, DDS_SECS(1));
      dds_qset_durability(qos, DDS_DURABILITY_VOLATILE);
      dds_qset_history(qos, DDS_HISTORY_KEEP_LAST, 256);
      break;
    case BRIDGE_QOS_SYSTEM_STATUS:
      dds_qset_reliability(qos, DDS_RELIABILITY_RELIABLE, DDS_SECS(1));
      dds_qset_durability(qos, DDS_DURABILITY_TRANSIENT_LOCAL);
      dds_qset_history(qos, DDS_HISTORY_KEEP_LAST, 1);
      break;
    case BRIDGE_QOS_CONTROL_COMMAND:
      dds_qset_reliability(qos, DDS_RELIABILITY_RELIABLE, DDS_SECS(1));
      dds_qset_durability(qos, DDS_DURABILITY_VOLATILE);
      dds_qset_history(qos, DDS_HISTORY_KEEP_LAST, 1);
      dds_qset_deadline(qos, DDS_MSECS(50));
      break;
    case BRIDGE_QOS_FINAL_VELOCITY:
      dds_qset_reliability(qos, DDS_RELIABILITY_BEST_EFFORT, 0);
      dds_qset_durability(qos, DDS_DURABILITY_VOLATILE);
      dds_qset_history(qos, DDS_HISTORY_KEEP_LAST, 1);
      dds_qset_deadline(qos, DDS_MSECS(50));
      dds_qset_lifespan(qos, DDS_MSECS(200));
      break;
  }
  return qos;
}

static dds_entity_t create_writer(dds_entity_t participant, dds_entity_t publisher,
                                  const dds_topic_descriptor_t* desc, const char* topic_name,
                                  BridgeQosKind kind) {
  dds_entity_t topic = dds_create_topic(participant, desc, topic_name, NULL, NULL);
  if (topic < 0) return topic;
  dds_qos_t* qos = make_bridge_qos(kind);
  if (!qos) return DDS_RETCODE_OUT_OF_RESOURCES;
  dds_entity_t writer = dds_create_writer(publisher, topic, qos, NULL);
  dds_delete_qos(qos);
  return writer;
}

static dds_entity_t create_reader(dds_entity_t participant, dds_entity_t subscriber,
                                  const dds_topic_descriptor_t* desc, const char* topic_name,
                                  BridgeQosKind kind) {
  dds_entity_t topic = dds_create_topic(participant, desc, topic_name, NULL, NULL);
  if (topic < 0) return topic;
  dds_qos_t* qos = make_bridge_qos(kind);
  if (!qos) return DDS_RETCODE_OUT_OF_RESOURCES;
  dds_entity_t reader = dds_create_reader(subscriber, topic, qos, NULL);
  dds_delete_qos(qos);
  return reader;
}

BridgeDdsHandle* bridge_dds_create(int32_t domain_id) {
  BridgeDdsHandle* handle = (BridgeDdsHandle*)calloc(1, sizeof(BridgeDdsHandle));
  if (!handle) return NULL;

  handle->participant = dds_create_participant((dds_domainid_t)domain_id, NULL, NULL);
  if (handle->participant < 0) goto fail;
  handle->subscriber = dds_create_subscriber(handle->participant, NULL, NULL);
  handle->publisher = dds_create_publisher(handle->participant, NULL, NULL);
  if (handle->subscriber < 0 || handle->publisher < 0) goto fail;

  handle->imu_body_writer = create_writer(
      handle->participant, handle->publisher, &lingtu_dds_Imu_desc, "rt/imu/body",
      BRIDGE_QOS_SENSOR_STREAM);
  handle->joint_state_writer = create_writer(
      handle->participant, handle->publisher, &lingtu_dds_JointState_desc, "rt/robot/joint_states",
      BRIDGE_QOS_SENSOR_STREAM);
  handle->control_state_writer = create_writer(
      handle->participant, handle->publisher, &lingtu_dds_DriverControlState_desc,
      "rt/driver/control_state", BRIDGE_QOS_SYSTEM_STATUS);
  handle->profile_state_writer = create_writer(
      handle->participant, handle->publisher, &lingtu_dds_MotionProfileState_desc,
      "rt/driver/profile/state", BRIDGE_QOS_SYSTEM_STATUS);
  handle->posture_ack_writer = create_writer(
      handle->participant, handle->publisher, &lingtu_dds_MotionPostureAck_desc,
      "rt/driver/posture/ack", BRIDGE_QOS_CONTROL_COMMAND);

  handle->cmd_vel_reader = create_reader(
      handle->participant, handle->subscriber, &lingtu_dds_FinalVelocityCommand_desc,
      "rt/nav/cmd_vel", BRIDGE_QOS_FINAL_VELOCITY);
  handle->posture_request_reader = create_reader(
      handle->participant, handle->subscriber, &lingtu_dds_MotionPostureRequest_desc,
      "rt/driver/posture/request", BRIDGE_QOS_CONTROL_COMMAND);
  handle->profile_request_reader = create_reader(
      handle->participant, handle->subscriber, &lingtu_dds_MotionProfileRequest_desc,
      "rt/driver/profile/request", BRIDGE_QOS_CONTROL_COMMAND);

  if (handle->imu_body_writer < 0 || handle->joint_state_writer < 0 ||
      handle->control_state_writer < 0 || handle->profile_state_writer < 0 ||
      handle->posture_ack_writer < 0 || handle->cmd_vel_reader < 0 ||
      handle->posture_request_reader < 0 || handle->profile_request_reader < 0) {
    goto fail;
  }
  return handle;

fail:
  bridge_dds_destroy(handle);
  return NULL;
}

void bridge_dds_destroy(BridgeDdsHandle* handle) {
  if (!handle) return;
  if (handle->participant > 0) {
    dds_delete(handle->participant);
  }
  free(handle);
}

int32_t bridge_dds_matched_cmd_vel_writers(const BridgeDdsHandle* handle) {
  if (!handle) return -1;
  return (int32_t)dds_get_matched_publications(handle->cmd_vel_reader, NULL, 0);
}

int32_t bridge_dds_take_cmd_vel(const BridgeDdsHandle* handle, BridgeCmdVelSample* out_latest) {
  if (!handle || !out_latest) return -1;
  void* samples[8] = {0};
  dds_sample_info_t infos[8];
  memset(infos, 0, sizeof(infos));
  dds_return_t count = dds_take(handle->cmd_vel_reader, samples, infos, 8, 8);
  if (count < 0) return (int32_t)count;

  memset(out_latest, 0, sizeof(*out_latest));
  for (dds_return_t i = 0; i < count; ++i) {
    if (!infos[i].valid_data) continue;
    const lingtu_dds_FinalVelocityCommand* msg =
        (const lingtu_dds_FinalVelocityCommand*)samples[i];
    copy_cstr(out_latest->host_boot_id, sizeof(out_latest->host_boot_id), msg->host_boot_id);
    copy_cstr(out_latest->producer_boot_id, sizeof(out_latest->producer_boot_id),
              msg->producer_boot_id);
    out_latest->output_seq = msg->output_seq;
    out_latest->source_boottime_ns = msg->source_boottime_ns;
    out_latest->source_wall_ns = msg->source_wall_ns;
    out_latest->vx = msg->twist.linear.x;
    out_latest->vy = msg->twist.linear.y;
    out_latest->wz = msg->twist.angular.z;
    out_latest->valid_samples += 1;
  }
  if (count > 0) {
    dds_return_t ret = dds_return_loan(handle->cmd_vel_reader, samples, count);
    if (ret < 0) return (int32_t)ret;
  }
  return out_latest->valid_samples > 0 ? 1 : 0;
}

int32_t bridge_dds_take_posture_request(const BridgeDdsHandle* handle,
                                        BridgePostureRequestMsg* out_req) {
  if (!handle || !out_req) return -1;
  void* samples[8] = {0};
  dds_sample_info_t infos[8];
  memset(infos, 0, sizeof(infos));
  dds_return_t count = dds_take(handle->posture_request_reader, samples, infos, 8, 8);
  if (count < 0) return (int32_t)count;

  int32_t found = 0;
  for (dds_return_t i = 0; i < count; ++i) {
    if (!infos[i].valid_data) continue;
    const lingtu_dds_MotionPostureRequest* msg =
        (const lingtu_dds_MotionPostureRequest*)samples[i];
    memset(out_req, 0, sizeof(*out_req));
    copy_cstr(out_req->host_boot_id, sizeof(out_req->host_boot_id), msg->host_boot_id);
    copy_cstr(out_req->producer_boot_id, sizeof(out_req->producer_boot_id), msg->producer_boot_id);
    out_req->source_boottime_ns = msg->source_boottime_ns;
    copy_cstr(out_req->client_id, sizeof(out_req->client_id), msg->client_id);
    copy_cstr(out_req->request_id, sizeof(out_req->request_id), msg->request_id);
    out_req->action = msg->action;
    out_req->phase = msg->phase;
    copy_cstr(out_req->profile_name, sizeof(out_req->profile_name), msg->profile_name);
    copy_cstr(out_req->external_session_id, sizeof(out_req->external_session_id),
              msg->external_session_id);
    found = 1;
  }
  if (count > 0) {
    dds_return_t ret = dds_return_loan(handle->posture_request_reader, samples, count);
    if (ret < 0) return (int32_t)ret;
  }
  return found;
}

int32_t bridge_dds_take_profile_request(const BridgeDdsHandle* handle,
                                        BridgeProfileRequestMsg* out_req) {
  if (!handle || !out_req) return -1;
  void* samples[8] = {0};
  dds_sample_info_t infos[8];
  memset(infos, 0, sizeof(infos));
  dds_return_t count = dds_take(handle->profile_request_reader, samples, infos, 8, 8);
  if (count < 0) return (int32_t)count;

  int32_t found = 0;
  for (dds_return_t i = 0; i < count; ++i) {
    if (!infos[i].valid_data) continue;
    const lingtu_dds_MotionProfileRequest* msg =
        (const lingtu_dds_MotionProfileRequest*)samples[i];
    memset(out_req, 0, sizeof(*out_req));
    copy_cstr(out_req->task_id, sizeof(out_req->task_id), msg->task_id);
    copy_cstr(out_req->requirement, sizeof(out_req->requirement), msg->requirement);
    found = 1;
  }
  if (count > 0) {
    dds_return_t ret = dds_return_loan(handle->profile_request_reader, samples, count);
    if (ret < 0) return (int32_t)ret;
  }
  return found;
}

int32_t bridge_dds_write_imu_body(const BridgeDdsHandle* handle, const BridgeImuMsg* msg) {
  if (!handle || !msg) return -1;
  lingtu_dds_Imu wire;
  memset(&wire, 0, sizeof(wire));
  wire.header.stamp.sec = msg->sec;
  wire.header.stamp.nanosec = msg->nanosec;
  wire.header.frame_id = (char*)"body";
  wire.orientation.x = msg->qx;
  wire.orientation.y = msg->qy;
  wire.orientation.z = msg->qz;
  wire.orientation.w = msg->qw;
  wire.angular_velocity.x = msg->gx;
  wire.angular_velocity.y = msg->gy;
  wire.angular_velocity.z = msg->gz;
  wire.linear_acceleration.x = msg->ax;
  wire.linear_acceleration.y = msg->ay;
  wire.linear_acceleration.z = msg->az;
  dds_return_t rc = dds_write(handle->imu_body_writer, &wire);
  return rc >= 0 ? 0 : (int32_t)rc;
}

int32_t bridge_dds_write_joint_states(const BridgeDdsHandle* handle, const char* robot_model,
                                      const char* const* names16,
                                      const BridgeJointStateMsg* msg) {
  if (!handle || !robot_model || !names16 || !msg) return -1;
  char* mutable_names[16];
  double pos[16];
  double vel[16];
  double eff[16];
  for (int i = 0; i < 16; ++i) {
    mutable_names[i] = (char*)(names16[i] ? names16[i] : "");
    pos[i] = msg->position[i];
    vel[i] = msg->velocity[i];
    eff[i] = msg->effort[i];
  }
  lingtu_dds_JointState wire;
  memset(&wire, 0, sizeof(wire));
  wire.header.stamp.sec = msg->sec;
  wire.header.stamp.nanosec = msg->nanosec;
  wire.header.frame_id = (char*)"body";
  wire.robot_model = (char*)robot_model;
  wire.names._length = 16;
  wire.names._maximum = 16;
  wire.names._buffer = mutable_names;
  wire.names._release = false;
  wire.position._length = 16;
  wire.position._maximum = 16;
  wire.position._buffer = pos;
  wire.position._release = false;
  wire.velocity._length = 16;
  wire.velocity._maximum = 16;
  wire.velocity._buffer = vel;
  wire.velocity._release = false;
  wire.effort._length = 16;
  wire.effort._maximum = 16;
  wire.effort._buffer = eff;
  wire.effort._release = false;
  dds_return_t rc = dds_write(handle->joint_state_writer, &wire);
  return rc >= 0 ? 0 : (int32_t)rc;
}

int32_t bridge_dds_write_control_state(const BridgeDdsHandle* handle,
                                       const BridgeControlStateMsg* msg) {
  if (!handle || !msg) return -1;
  lingtu_dds_DriverControlState wire;
  memset(&wire, 0, sizeof(wire));
  wire.header.stamp.sec = msg->sec;
  wire.header.stamp.nanosec = msg->nanosec;
  wire.header.frame_id = (char*)"body";
  wire.connected = msg->connected != 0;
  wire.ready = msg->ready != 0;
  wire.motors_enabled = msg->motors_enabled != 0;
  wire.critical_fault = msg->critical_fault != 0;
  wire.control_assured = msg->control_assured != 0;
  wire.lease_valid = msg->lease_valid != 0;
  wire.lease_remaining_ms = msg->lease_remaining_ms;
  wire.accepted_sequence = msg->accepted_sequence;
  wire.zero_output = msg->zero_output != 0;
  wire.fsm = (char*)msg->fsm;
  wire.owner = (char*)msg->owner;
  wire.owner_id = (char*)msg->owner_id;
  wire.reason = (char*)msg->reason;
  dds_return_t rc = dds_write(handle->control_state_writer, &wire);
  return rc >= 0 ? 0 : (int32_t)rc;
}

int32_t bridge_dds_write_profile_state(const BridgeDdsHandle* handle,
                                       const BridgeProfileStateMsg* msg) {
  if (!handle || !msg) return -1;
  lingtu_dds_MotionProfileState wire;
  memset(&wire, 0, sizeof(wire));
  wire.header.stamp.sec = msg->sec;
  wire.header.stamp.nanosec = msg->nanosec;
  wire.header.frame_id = (char*)"body";
  wire.task_id = (char*)msg->task_id;
  wire.requirement = (char*)msg->requirement;
  wire.phase = (char*)msg->phase;
  wire.requested_profile = (char*)msg->requested_profile;
  wire.active_profile = (char*)msg->active_profile;
  wire.reason = (char*)msg->reason;
  dds_return_t rc = dds_write(handle->profile_state_writer, &wire);
  return rc >= 0 ? 0 : (int32_t)rc;
}

int32_t bridge_dds_write_posture_ack(const BridgeDdsHandle* handle,
                                     const BridgePostureAckMsg* msg) {
  if (!handle || !msg) return -1;
  lingtu_dds_MotionPostureAck wire;
  memset(&wire, 0, sizeof(wire));
  wire.header.stamp.sec = msg->sec;
  wire.header.stamp.nanosec = msg->nanosec;
  wire.header.frame_id = (char*)"body";
  wire.producer_boot_id = (char*)msg->producer_boot_id;
  wire.client_id = (char*)msg->client_id;
  wire.request_id = (char*)msg->request_id;
  wire.action = msg->action;
  wire.phase = msg->phase;
  wire.profile_name = (char*)msg->profile_name;
  wire.external_session_id = (char*)msg->external_session_id;
  wire.final_result = msg->final_result != 0;
  wire.accepted = msg->accepted != 0;
  wire.reason = (char*)msg->reason;
  dds_return_t rc = dds_write(handle->posture_ack_writer, &wire);
  return rc >= 0 ? 0 : (int32_t)rc;
}

BridgeDdsTestPeer* bridge_dds_test_peer_create(int32_t domain_id) {
  BridgeDdsTestPeer* peer = (BridgeDdsTestPeer*)calloc(1, sizeof(BridgeDdsTestPeer));
  if (!peer) return NULL;
  peer->participant = dds_create_participant((dds_domainid_t)domain_id, NULL, NULL);
  if (peer->participant < 0) goto fail;
  peer->subscriber = dds_create_subscriber(peer->participant, NULL, NULL);
  peer->publisher = dds_create_publisher(peer->participant, NULL, NULL);
  if (peer->subscriber < 0 || peer->publisher < 0) goto fail;

  peer->cmd_vel_writer = create_writer(
      peer->participant, peer->publisher, &lingtu_dds_FinalVelocityCommand_desc,
      "rt/nav/cmd_vel", BRIDGE_QOS_FINAL_VELOCITY);
  peer->posture_request_writer = create_writer(
      peer->participant, peer->publisher, &lingtu_dds_MotionPostureRequest_desc,
      "rt/driver/posture/request", BRIDGE_QOS_CONTROL_COMMAND);

  peer->imu_body_reader = create_reader(
      peer->participant, peer->subscriber, &lingtu_dds_Imu_desc, "rt/imu/body",
      BRIDGE_QOS_SENSOR_STREAM);
  peer->joint_state_reader = create_reader(
      peer->participant, peer->subscriber, &lingtu_dds_JointState_desc, "rt/robot/joint_states",
      BRIDGE_QOS_SENSOR_STREAM);
  peer->control_state_reader = create_reader(
      peer->participant, peer->subscriber, &lingtu_dds_DriverControlState_desc,
      "rt/driver/control_state", BRIDGE_QOS_SYSTEM_STATUS);
  peer->posture_ack_reader = create_reader(
      peer->participant, peer->subscriber, &lingtu_dds_MotionPostureAck_desc,
      "rt/driver/posture/ack", BRIDGE_QOS_CONTROL_COMMAND);

  if (peer->cmd_vel_writer < 0 || peer->posture_request_writer < 0 ||
      peer->imu_body_reader < 0 || peer->joint_state_reader < 0 ||
      peer->control_state_reader < 0 || peer->posture_ack_reader < 0) {
    goto fail;
  }
  return peer;

fail:
  bridge_dds_test_peer_destroy(peer);
  return NULL;
}

void bridge_dds_test_peer_destroy(BridgeDdsTestPeer* peer) {
  if (!peer) return;
  if (peer->participant > 0) {
    dds_delete(peer->participant);
  }
  free(peer);
}

int32_t bridge_dds_test_peer_matched_bridge_readers(const BridgeDdsTestPeer* peer) {
  if (!peer) return -1;
  return (int32_t)dds_get_matched_subscriptions(peer->cmd_vel_writer, NULL, 0);
}

int32_t bridge_dds_test_peer_write_cmd_vel(const BridgeDdsTestPeer* peer,
                                           const BridgeCmdVelSample* msg) {
  if (!peer || !msg) return -1;
  lingtu_dds_FinalVelocityCommand wire;
  memset(&wire, 0, sizeof(wire));
  wire.host_boot_id = (char*)msg->host_boot_id;
  wire.producer_boot_id = (char*)msg->producer_boot_id;
  wire.output_seq = msg->output_seq;
  wire.source_boottime_ns = msg->source_boottime_ns;
  wire.source_wall_ns = msg->source_wall_ns;
  wire.twist.linear.x = msg->vx;
  wire.twist.linear.y = msg->vy;
  wire.twist.angular.z = msg->wz;
  dds_return_t rc = dds_write(peer->cmd_vel_writer, &wire);
  return rc >= 0 ? 0 : (int32_t)rc;
}

int32_t bridge_dds_test_peer_write_posture_request(const BridgeDdsTestPeer* peer,
                                                   const BridgePostureRequestMsg* msg) {
  if (!peer || !msg) return -1;
  lingtu_dds_MotionPostureRequest wire;
  memset(&wire, 0, sizeof(wire));
  wire.header.frame_id = (char*)"body";
  wire.host_boot_id = (char*)msg->host_boot_id;
  wire.producer_boot_id = (char*)msg->producer_boot_id;
  wire.source_boottime_ns = msg->source_boottime_ns;
  wire.client_id = (char*)msg->client_id;
  wire.request_id = (char*)msg->request_id;
  wire.action = msg->action;
  wire.phase = msg->phase;
  wire.profile_name = (char*)msg->profile_name;
  wire.external_session_id = (char*)msg->external_session_id;
  dds_return_t rc = dds_write(peer->posture_request_writer, &wire);
  return rc >= 0 ? 0 : (int32_t)rc;
}

int32_t bridge_dds_test_peer_take_imu_body(const BridgeDdsTestPeer* peer, BridgeImuMsg* out_msg) {
  if (!peer || !out_msg) return -1;
  void* samples[8] = {0};
  dds_sample_info_t infos[8];
  memset(infos, 0, sizeof(infos));
  dds_return_t count = dds_take(peer->imu_body_reader, samples, infos, 8, 8);
  if (count < 0) return (int32_t)count;
  int32_t found = 0;
  for (dds_return_t i = 0; i < count; ++i) {
    if (!infos[i].valid_data) continue;
    const lingtu_dds_Imu* msg = (const lingtu_dds_Imu*)samples[i];
    out_msg->sec = msg->header.stamp.sec;
    out_msg->nanosec = msg->header.stamp.nanosec;
    out_msg->qx = msg->orientation.x;
    out_msg->qy = msg->orientation.y;
    out_msg->qz = msg->orientation.z;
    out_msg->qw = msg->orientation.w;
    out_msg->gx = msg->angular_velocity.x;
    out_msg->gy = msg->angular_velocity.y;
    out_msg->gz = msg->angular_velocity.z;
    out_msg->ax = msg->linear_acceleration.x;
    out_msg->ay = msg->linear_acceleration.y;
    out_msg->az = msg->linear_acceleration.z;
    found = 1;
  }
  if (count > 0) dds_return_loan(peer->imu_body_reader, samples, count);
  return found;
}

int32_t bridge_dds_test_peer_take_joint_states(const BridgeDdsTestPeer* peer,
                                               BridgeJointStateMsg* out_msg, uint32_t* out_len) {
  if (!peer || !out_msg || !out_len) return -1;
  void* samples[8] = {0};
  dds_sample_info_t infos[8];
  memset(infos, 0, sizeof(infos));
  dds_return_t count = dds_take(peer->joint_state_reader, samples, infos, 8, 8);
  if (count < 0) return (int32_t)count;
  int32_t found = 0;
  for (dds_return_t i = 0; i < count; ++i) {
    if (!infos[i].valid_data) continue;
    const lingtu_dds_JointState* msg = (const lingtu_dds_JointState*)samples[i];
    out_msg->sec = msg->header.stamp.sec;
    out_msg->nanosec = msg->header.stamp.nanosec;
    uint32_t len = msg->position._length < 16 ? msg->position._length : 16;
    *out_len = len;
    for (uint32_t j = 0; j < len; ++j) {
      out_msg->position[j] = msg->position._buffer[j];
      out_msg->velocity[j] = j < msg->velocity._length ? msg->velocity._buffer[j] : 0.0;
      out_msg->effort[j] = j < msg->effort._length ? msg->effort._buffer[j] : 0.0;
    }
    found = 1;
  }
  if (count > 0) dds_return_loan(peer->joint_state_reader, samples, count);
  return found;
}

int32_t bridge_dds_test_peer_take_control_state(const BridgeDdsTestPeer* peer,
                                                BridgeControlStateMsg* out_msg) {
  if (!peer || !out_msg) return -1;
  void* samples[8] = {0};
  dds_sample_info_t infos[8];
  memset(infos, 0, sizeof(infos));
  dds_return_t count = dds_take(peer->control_state_reader, samples, infos, 8, 8);
  if (count < 0) return (int32_t)count;
  int32_t found = 0;
  for (dds_return_t i = 0; i < count; ++i) {
    if (!infos[i].valid_data) continue;
    const lingtu_dds_DriverControlState* msg = (const lingtu_dds_DriverControlState*)samples[i];
    memset(out_msg, 0, sizeof(*out_msg));
    out_msg->sec = msg->header.stamp.sec;
    out_msg->nanosec = msg->header.stamp.nanosec;
    out_msg->connected = msg->connected ? 1 : 0;
    out_msg->ready = msg->ready ? 1 : 0;
    out_msg->motors_enabled = msg->motors_enabled ? 1 : 0;
    out_msg->critical_fault = msg->critical_fault ? 1 : 0;
    out_msg->control_assured = msg->control_assured ? 1 : 0;
    out_msg->lease_valid = msg->lease_valid ? 1 : 0;
    out_msg->lease_remaining_ms = msg->lease_remaining_ms;
    out_msg->accepted_sequence = msg->accepted_sequence;
    out_msg->zero_output = msg->zero_output ? 1 : 0;
    copy_cstr(out_msg->fsm, sizeof(out_msg->fsm), msg->fsm);
    copy_cstr(out_msg->owner, sizeof(out_msg->owner), msg->owner);
    copy_cstr(out_msg->owner_id, sizeof(out_msg->owner_id), msg->owner_id);
    copy_cstr(out_msg->reason, sizeof(out_msg->reason), msg->reason);
    found = 1;
  }
  if (count > 0) dds_return_loan(peer->control_state_reader, samples, count);
  return found;
}

int32_t bridge_dds_test_peer_take_posture_ack(const BridgeDdsTestPeer* peer,
                                              BridgePostureAckMsg* out_msg) {
  if (!peer || !out_msg) return -1;
  void* samples[8] = {0};
  dds_sample_info_t infos[8];
  memset(infos, 0, sizeof(infos));
  dds_return_t count = dds_take(peer->posture_ack_reader, samples, infos, 8, 8);
  if (count < 0) return (int32_t)count;
  int32_t found = 0;
  for (dds_return_t i = 0; i < count; ++i) {
    if (!infos[i].valid_data) continue;
    const lingtu_dds_MotionPostureAck* msg = (const lingtu_dds_MotionPostureAck*)samples[i];
    memset(out_msg, 0, sizeof(*out_msg));
    out_msg->sec = msg->header.stamp.sec;
    out_msg->nanosec = msg->header.stamp.nanosec;
    copy_cstr(out_msg->producer_boot_id, sizeof(out_msg->producer_boot_id), msg->producer_boot_id);
    copy_cstr(out_msg->client_id, sizeof(out_msg->client_id), msg->client_id);
    copy_cstr(out_msg->request_id, sizeof(out_msg->request_id), msg->request_id);
    out_msg->action = msg->action;
    out_msg->phase = msg->phase;
    copy_cstr(out_msg->profile_name, sizeof(out_msg->profile_name), msg->profile_name);
    copy_cstr(out_msg->external_session_id, sizeof(out_msg->external_session_id),
              msg->external_session_id);
    out_msg->final_result = msg->final_result ? 1 : 0;
    out_msg->accepted = msg->accepted ? 1 : 0;
    copy_cstr(out_msg->reason, sizeof(out_msg->reason), msg->reason);
    found = 1;
  }
  if (count > 0) dds_return_loan(peer->posture_ack_reader, samples, count);
  return found;
}
