use anyhow::Result;
use std::collections::VecDeque;
use std::sync::Mutex;

pub const THUNDER_JOINT_NAMES16: [&str; 16] = [
    "FR_hip", "FR_thigh", "FR_calf", "FL_hip", "FL_thigh", "FL_calf", "RR_hip", "RR_thigh",
    "RR_calf", "RL_hip", "RL_thigh", "RL_calf", "FR_foot", "FL_foot", "RR_foot", "RL_foot",
];

#[derive(Debug, Clone, Default)]
pub struct DdsStamp {
    pub sec: i32,
    pub nanosec: u32,
}

#[derive(Debug, Clone, Default)]
pub struct ImuBodySample {
    pub stamp: DdsStamp,
    pub orientation_xyzw: [f64; 4],
    pub angular_velocity_xyz: [f64; 3],
    pub linear_acceleration_xyz: [f64; 3],
}

#[derive(Debug, Clone, Default)]
pub struct JointStateSample {
    pub stamp: DdsStamp,
    pub robot_model: String,
    pub position: [f64; 16],
    pub velocity: [f64; 16],
    pub effort: [f64; 16],
}

#[derive(Debug, Clone, Default)]
pub struct DriverControlStateMsg {
    pub stamp: DdsStamp,
    pub connected: bool,
    pub ready: bool,
    pub motors_enabled: bool,
    pub critical_fault: bool,
    pub control_assured: bool,
    pub lease_valid: bool,
    pub lease_remaining_ms: u32,
    pub accepted_sequence: u64,
    pub zero_output: bool,
    pub fsm: String,
    pub owner: String,
    pub owner_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct ProfileStateMsg {
    pub stamp: DdsStamp,
    pub task_id: String,
    pub requirement: String,
    pub phase: String,
    pub requested_profile: String,
    pub active_profile: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct PostureRequestMsg {
    pub host_boot_id: String,
    pub producer_boot_id: String,
    pub source_boottime_ns: u64,
    pub client_id: String,
    pub request_id: String,
    pub action: i32,
    pub phase: i32,
    pub profile_name: String,
    pub external_session_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct PostureAckMsg {
    pub stamp: DdsStamp,
    pub producer_boot_id: String,
    pub client_id: String,
    pub request_id: String,
    pub action: i32,
    pub phase: i32,
    pub profile_name: String,
    pub external_session_id: String,
    pub final_result: bool,
    pub accepted: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct ProfileRequestMsg {
    pub task_id: String,
    pub requirement: String,
}

#[derive(Debug, Clone, Default)]
pub struct CmdVelWireMsg {
    pub host_boot_id: String,
    pub producer_boot_id: String,
    pub output_seq: u64,
    pub source_boottime_ns: u64,
    pub source_wall_ns: u64,
    pub vx: f64,
    pub vy: f64,
    pub wz: f64,
}

pub trait DdsBus: Send + Sync {
    fn matched_cmd_vel_writers(&self) -> Result<u32>;
    fn take_cmd_vel(&self) -> Result<Option<(CmdVelWireMsg, u32)>>;
    fn take_posture_request(&self) -> Result<Option<PostureRequestMsg>>;
    fn take_profile_request(&self) -> Result<Option<ProfileRequestMsg>>;

    fn write_imu_body(&self, msg: &ImuBodySample) -> Result<()>;
    fn write_joint_states(&self, msg: &JointStateSample) -> Result<()>;
    fn write_control_state(&self, msg: &DriverControlStateMsg) -> Result<()>;
    fn write_profile_state(&self, msg: &ProfileStateMsg) -> Result<()>;
    fn write_posture_ack(&self, msg: &PostureAckMsg) -> Result<()>;
}

/// Deterministic in-memory DDS bus for unit testing without native CycloneDDS sockets.
#[derive(Default)]
pub struct InMemoryDdsBus {
    pub cmd_vel_writers: Mutex<u32>,
    pub cmd_vel_rx: Mutex<VecDeque<CmdVelWireMsg>>,
    pub posture_req_rx: Mutex<VecDeque<PostureRequestMsg>>,
    pub profile_req_rx: Mutex<VecDeque<ProfileRequestMsg>>,
    pub imu_tx: Mutex<Vec<ImuBodySample>>,
    pub joints_tx: Mutex<Vec<JointStateSample>>,
    pub control_tx: Mutex<Vec<DriverControlStateMsg>>,
    pub profile_tx: Mutex<Vec<ProfileStateMsg>>,
    pub posture_ack_tx: Mutex<Vec<PostureAckMsg>>,
}

impl DdsBus for InMemoryDdsBus {
    fn matched_cmd_vel_writers(&self) -> Result<u32> {
        Ok(*self.cmd_vel_writers.lock().unwrap())
    }

    fn take_cmd_vel(&self) -> Result<Option<(CmdVelWireMsg, u32)>> {
        let mut queue = self.cmd_vel_rx.lock().unwrap();
        let count = queue.len() as u32;
        let latest = queue.drain(..).next_back();
        Ok(latest.map(|item| (item, count)))
    }

    fn take_posture_request(&self) -> Result<Option<PostureRequestMsg>> {
        Ok(self.posture_req_rx.lock().unwrap().drain(..).next_back())
    }

    fn take_profile_request(&self) -> Result<Option<ProfileRequestMsg>> {
        Ok(self.profile_req_rx.lock().unwrap().drain(..).next_back())
    }

    fn write_imu_body(&self, msg: &ImuBodySample) -> Result<()> {
        self.imu_tx.lock().unwrap().push(msg.clone());
        Ok(())
    }

    fn write_joint_states(&self, msg: &JointStateSample) -> Result<()> {
        self.joints_tx.lock().unwrap().push(msg.clone());
        Ok(())
    }

    fn write_control_state(&self, msg: &DriverControlStateMsg) -> Result<()> {
        self.control_tx.lock().unwrap().push(msg.clone());
        Ok(())
    }

    fn write_profile_state(&self, msg: &ProfileStateMsg) -> Result<()> {
        self.profile_tx.lock().unwrap().push(msg.clone());
        Ok(())
    }

    fn write_posture_ack(&self, msg: &PostureAckMsg) -> Result<()> {
        self.posture_ack_tx.lock().unwrap().push(msg.clone());
        Ok(())
    }
}

#[cfg(has_cyclonedds)]
pub mod cyclone {
    use super::*;
    use anyhow::{anyhow, bail};
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::ptr;
    use std::sync::Mutex;

    #[repr(C)]
    struct BridgeImuMsg {
        sec: i32,
        nanosec: u32,
        qx: f64,
        qy: f64,
        qz: f64,
        qw: f64,
        gx: f64,
        gy: f64,
        gz: f64,
        ax: f64,
        ay: f64,
        az: f64,
    }

    #[repr(C)]
    struct BridgeJointStateMsg {
        sec: i32,
        nanosec: u32,
        position: [f64; 16],
        velocity: [f64; 16],
        effort: [f64; 16],
    }

    #[repr(C)]
    struct BridgeControlStateMsg {
        sec: i32,
        nanosec: u32,
        connected: u8,
        ready: u8,
        motors_enabled: u8,
        critical_fault: u8,
        control_assured: u8,
        lease_valid: u8,
        lease_remaining_ms: u32,
        accepted_sequence: u64,
        zero_output: u8,
        fsm: [c_char; 64],
        owner: [c_char; 64],
        owner_id: [c_char; 128],
        reason: [c_char; 128],
    }

    #[repr(C)]
    struct BridgeProfileStateMsg {
        sec: i32,
        nanosec: u32,
        task_id: [c_char; 128],
        requirement: [c_char; 128],
        phase: [c_char; 64],
        requested_profile: [c_char; 128],
        active_profile: [c_char; 128],
        reason: [c_char; 128],
    }

    #[repr(C)]
    struct BridgePostureRequestMsg {
        host_boot_id: [c_char; 128],
        producer_boot_id: [c_char; 128],
        source_boottime_ns: u64,
        client_id: [c_char; 128],
        request_id: [c_char; 128],
        action: i32,
        phase: i32,
        profile_name: [c_char; 128],
        external_session_id: [c_char; 128],
    }

    #[repr(C)]
    struct BridgePostureAckMsg {
        sec: i32,
        nanosec: u32,
        producer_boot_id: [c_char; 128],
        client_id: [c_char; 128],
        request_id: [c_char; 128],
        action: i32,
        phase: i32,
        profile_name: [c_char; 128],
        external_session_id: [c_char; 128],
        final_result: u8,
        accepted: u8,
        reason: [c_char; 128],
    }

    #[repr(C)]
    struct BridgeProfileRequestMsg {
        task_id: [c_char; 128],
        requirement: [c_char; 128],
    }

    #[repr(C)]
    struct BridgeCmdVelSample {
        host_boot_id: [c_char; 128],
        producer_boot_id: [c_char; 128],
        output_seq: u64,
        source_boottime_ns: u64,
        source_wall_ns: u64,
        vx: f64,
        vy: f64,
        wz: f64,
        valid_samples: u32,
    }

    extern "C" {
        fn bridge_dds_create(domain_id: i32) -> *mut c_void;
        fn bridge_dds_destroy(handle: *mut c_void);
        fn bridge_dds_matched_cmd_vel_writers(handle: *const c_void) -> i32;
        fn bridge_dds_take_cmd_vel(
            handle: *const c_void,
            out_latest: *mut BridgeCmdVelSample,
        ) -> i32;
        fn bridge_dds_take_posture_request(
            handle: *const c_void,
            out_req: *mut BridgePostureRequestMsg,
        ) -> i32;
        fn bridge_dds_take_profile_request(
            handle: *const c_void,
            out_req: *mut BridgeProfileRequestMsg,
        ) -> i32;
        fn bridge_dds_write_imu_body(handle: *const c_void, msg: *const BridgeImuMsg) -> i32;
        fn bridge_dds_write_joint_states(
            handle: *const c_void,
            robot_model: *const c_char,
            names16: *const *const c_char,
            msg: *const BridgeJointStateMsg,
        ) -> i32;
        fn bridge_dds_write_control_state(
            handle: *const c_void,
            msg: *const BridgeControlStateMsg,
        ) -> i32;
        fn bridge_dds_write_profile_state(
            handle: *const c_void,
            msg: *const BridgeProfileStateMsg,
        ) -> i32;
        fn bridge_dds_write_posture_ack(
            handle: *const c_void,
            msg: *const BridgePostureAckMsg,
        ) -> i32;

        fn bridge_dds_test_peer_create(domain_id: i32) -> *mut c_void;
        fn bridge_dds_test_peer_destroy(peer: *mut c_void);
        fn bridge_dds_test_peer_matched_bridge_readers(peer: *const c_void) -> i32;
        fn bridge_dds_test_peer_write_cmd_vel(
            peer: *const c_void,
            msg: *const BridgeCmdVelSample,
        ) -> i32;
        fn bridge_dds_test_peer_write_posture_request(
            peer: *const c_void,
            msg: *const BridgePostureRequestMsg,
        ) -> i32;
        fn bridge_dds_test_peer_take_imu_body(
            peer: *const c_void,
            out_msg: *mut BridgeImuMsg,
        ) -> i32;
        fn bridge_dds_test_peer_take_joint_states(
            peer: *const c_void,
            out_msg: *mut BridgeJointStateMsg,
            out_len: *mut u32,
        ) -> i32;
        fn bridge_dds_test_peer_take_control_state(
            peer: *const c_void,
            out_msg: *mut BridgeControlStateMsg,
        ) -> i32;
        fn bridge_dds_test_peer_take_posture_ack(
            peer: *const c_void,
            out_msg: *mut BridgePostureAckMsg,
        ) -> i32;
    }

    fn write_fixed<const N: usize>(src: &str, dst: &mut [c_char; N]) {
        dst.fill(0);
        let bytes = src.as_bytes();
        let copy_len = bytes.len().min(N.saturating_sub(1));
        for (i, &b) in bytes[..copy_len].iter().enumerate() {
            dst[i] = b as c_char;
        }
    }

    fn read_fixed(buf: &[c_char]) -> String {
        unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }

    fn zero_cmd_vel() -> BridgeCmdVelSample {
        BridgeCmdVelSample {
            host_boot_id: [0; 128],
            producer_boot_id: [0; 128],
            output_seq: 0,
            source_boottime_ns: 0,
            source_wall_ns: 0,
            vx: 0.0,
            vy: 0.0,
            wz: 0.0,
            valid_samples: 0,
        }
    }

    fn zero_posture_req() -> BridgePostureRequestMsg {
        BridgePostureRequestMsg {
            host_boot_id: [0; 128],
            producer_boot_id: [0; 128],
            source_boottime_ns: 0,
            client_id: [0; 128],
            request_id: [0; 128],
            action: 0,
            phase: 0,
            profile_name: [0; 128],
            external_session_id: [0; 128],
        }
    }

    pub struct CycloneDdsBus {
        handle: Mutex<*mut c_void>,
        joint_name_cstrs: [CString; 16],
    }

    unsafe impl Send for CycloneDdsBus {}
    unsafe impl Sync for CycloneDdsBus {}

    impl CycloneDdsBus {
        pub fn new(domain_id: i32) -> Result<Self> {
            let handle = unsafe { bridge_dds_create(domain_id) };
            if handle.is_null() {
                return Err(anyhow!(
                    "failed to initialize CycloneDDS bridge participant on domain {domain_id}"
                ));
            }
            let joint_name_cstrs = THUNDER_JOINT_NAMES16.map(|n| CString::new(n).unwrap());
            Ok(Self {
                handle: Mutex::new(handle),
                joint_name_cstrs,
            })
        }
    }

    impl Drop for CycloneDdsBus {
        fn drop(&mut self) {
            let mut guard = self.handle.lock().unwrap();
            if !guard.is_null() {
                unsafe { bridge_dds_destroy(*guard) };
                *guard = ptr::null_mut();
            }
        }
    }

    impl DdsBus for CycloneDdsBus {
        fn matched_cmd_vel_writers(&self) -> Result<u32> {
            let guard = self.handle.lock().unwrap();
            let rc = unsafe { bridge_dds_matched_cmd_vel_writers(*guard) };
            if rc < 0 {
                bail!("dds matched_cmd_vel_writers failed: {rc}");
            }
            Ok(rc as u32)
        }

        fn take_cmd_vel(&self) -> Result<Option<(CmdVelWireMsg, u32)>> {
            let guard = self.handle.lock().unwrap();
            let mut raw = zero_cmd_vel();
            let rc = unsafe { bridge_dds_take_cmd_vel(*guard, &mut raw) };
            if rc < 0 {
                bail!("dds take_cmd_vel failed: {rc}");
            }
            if rc == 0 {
                return Ok(None);
            }
            Ok(Some((
                CmdVelWireMsg {
                    host_boot_id: read_fixed(&raw.host_boot_id),
                    producer_boot_id: read_fixed(&raw.producer_boot_id),
                    output_seq: raw.output_seq,
                    source_boottime_ns: raw.source_boottime_ns,
                    source_wall_ns: raw.source_wall_ns,
                    vx: raw.vx,
                    vy: raw.vy,
                    wz: raw.wz,
                },
                raw.valid_samples,
            )))
        }

        fn take_posture_request(&self) -> Result<Option<PostureRequestMsg>> {
            let guard = self.handle.lock().unwrap();
            let mut raw = zero_posture_req();
            let rc = unsafe { bridge_dds_take_posture_request(*guard, &mut raw) };
            if rc < 0 {
                bail!("dds take_posture_request failed: {rc}");
            }
            if rc == 0 {
                return Ok(None);
            }
            Ok(Some(PostureRequestMsg {
                host_boot_id: read_fixed(&raw.host_boot_id),
                producer_boot_id: read_fixed(&raw.producer_boot_id),
                source_boottime_ns: raw.source_boottime_ns,
                client_id: read_fixed(&raw.client_id),
                request_id: read_fixed(&raw.request_id),
                action: raw.action,
                phase: raw.phase,
                profile_name: read_fixed(&raw.profile_name),
                external_session_id: read_fixed(&raw.external_session_id),
            }))
        }

        fn take_profile_request(&self) -> Result<Option<ProfileRequestMsg>> {
            let guard = self.handle.lock().unwrap();
            let mut raw = BridgeProfileRequestMsg {
                task_id: [0; 128],
                requirement: [0; 128],
            };
            let rc = unsafe { bridge_dds_take_profile_request(*guard, &mut raw) };
            if rc < 0 {
                bail!("dds take_profile_request failed: {rc}");
            }
            if rc == 0 {
                return Ok(None);
            }
            Ok(Some(ProfileRequestMsg {
                task_id: read_fixed(&raw.task_id),
                requirement: read_fixed(&raw.requirement),
            }))
        }

        fn write_imu_body(&self, msg: &ImuBodySample) -> Result<()> {
            let guard = self.handle.lock().unwrap();
            let wire = BridgeImuMsg {
                sec: msg.stamp.sec,
                nanosec: msg.stamp.nanosec,
                qx: msg.orientation_xyzw[0],
                qy: msg.orientation_xyzw[1],
                qz: msg.orientation_xyzw[2],
                qw: msg.orientation_xyzw[3],
                gx: msg.angular_velocity_xyz[0],
                gy: msg.angular_velocity_xyz[1],
                gz: msg.angular_velocity_xyz[2],
                ax: msg.linear_acceleration_xyz[0],
                ay: msg.linear_acceleration_xyz[1],
                az: msg.linear_acceleration_xyz[2],
            };
            let rc = unsafe { bridge_dds_write_imu_body(*guard, &wire) };
            if rc < 0 {
                bail!("dds write_imu_body failed: {rc}");
            }
            Ok(())
        }

        fn write_joint_states(&self, msg: &JointStateSample) -> Result<()> {
            let guard = self.handle.lock().unwrap();
            let model = CString::new(msg.robot_model.as_str())?;
            let ptrs: [*const c_char; 16] =
                std::array::from_fn(|i| self.joint_name_cstrs[i].as_ptr());
            let wire = BridgeJointStateMsg {
                sec: msg.stamp.sec,
                nanosec: msg.stamp.nanosec,
                position: msg.position,
                velocity: msg.velocity,
                effort: msg.effort,
            };
            let rc = unsafe {
                bridge_dds_write_joint_states(*guard, model.as_ptr(), ptrs.as_ptr(), &wire)
            };
            if rc < 0 {
                bail!("dds write_joint_states failed: {rc}");
            }
            Ok(())
        }

        fn write_control_state(&self, msg: &DriverControlStateMsg) -> Result<()> {
            let guard = self.handle.lock().unwrap();
            let mut wire = BridgeControlStateMsg {
                sec: msg.stamp.sec,
                nanosec: msg.stamp.nanosec,
                connected: u8::from(msg.connected),
                ready: u8::from(msg.ready),
                motors_enabled: u8::from(msg.motors_enabled),
                critical_fault: u8::from(msg.critical_fault),
                control_assured: u8::from(msg.control_assured),
                lease_valid: u8::from(msg.lease_valid),
                lease_remaining_ms: msg.lease_remaining_ms,
                accepted_sequence: msg.accepted_sequence,
                zero_output: u8::from(msg.zero_output),
                fsm: [0; 64],
                owner: [0; 64],
                owner_id: [0; 128],
                reason: [0; 128],
            };
            write_fixed(&msg.fsm, &mut wire.fsm);
            write_fixed(&msg.owner, &mut wire.owner);
            write_fixed(&msg.owner_id, &mut wire.owner_id);
            write_fixed(&msg.reason, &mut wire.reason);
            let rc = unsafe { bridge_dds_write_control_state(*guard, &wire) };
            if rc < 0 {
                bail!("dds write_control_state failed: {rc}");
            }
            Ok(())
        }

        fn write_profile_state(&self, msg: &ProfileStateMsg) -> Result<()> {
            let guard = self.handle.lock().unwrap();
            let mut wire = BridgeProfileStateMsg {
                sec: msg.stamp.sec,
                nanosec: msg.stamp.nanosec,
                task_id: [0; 128],
                requirement: [0; 128],
                phase: [0; 64],
                requested_profile: [0; 128],
                active_profile: [0; 128],
                reason: [0; 128],
            };
            write_fixed(&msg.task_id, &mut wire.task_id);
            write_fixed(&msg.requirement, &mut wire.requirement);
            write_fixed(&msg.phase, &mut wire.phase);
            write_fixed(&msg.requested_profile, &mut wire.requested_profile);
            write_fixed(&msg.active_profile, &mut wire.active_profile);
            write_fixed(&msg.reason, &mut wire.reason);
            let rc = unsafe { bridge_dds_write_profile_state(*guard, &wire) };
            if rc < 0 {
                bail!("dds write_profile_state failed: {rc}");
            }
            Ok(())
        }

        fn write_posture_ack(&self, msg: &PostureAckMsg) -> Result<()> {
            let guard = self.handle.lock().unwrap();
            let mut wire = BridgePostureAckMsg {
                sec: msg.stamp.sec,
                nanosec: msg.stamp.nanosec,
                producer_boot_id: [0; 128],
                client_id: [0; 128],
                request_id: [0; 128],
                action: msg.action,
                phase: msg.phase,
                profile_name: [0; 128],
                external_session_id: [0; 128],
                final_result: u8::from(msg.final_result),
                accepted: u8::from(msg.accepted),
                reason: [0; 128],
            };
            write_fixed(&msg.producer_boot_id, &mut wire.producer_boot_id);
            write_fixed(&msg.client_id, &mut wire.client_id);
            write_fixed(&msg.request_id, &mut wire.request_id);
            write_fixed(&msg.profile_name, &mut wire.profile_name);
            write_fixed(&msg.external_session_id, &mut wire.external_session_id);
            write_fixed(&msg.reason, &mut wire.reason);
            let rc = unsafe { bridge_dds_write_posture_ack(*guard, &wire) };
            if rc < 0 {
                bail!("dds write_posture_ack failed: {rc}");
            }
            Ok(())
        }
    }

    pub struct CycloneTestPeer {
        peer: *mut c_void,
    }

    unsafe impl Send for CycloneTestPeer {}
    unsafe impl Sync for CycloneTestPeer {}

    impl CycloneTestPeer {
        pub fn new(domain_id: i32) -> Result<Self> {
            let peer = unsafe { bridge_dds_test_peer_create(domain_id) };
            if peer.is_null() {
                bail!("failed to create CycloneDDS test peer on domain {domain_id}");
            }
            Ok(Self { peer })
        }

        pub fn matched_bridge_readers(&self) -> i32 {
            unsafe { bridge_dds_test_peer_matched_bridge_readers(self.peer) }
        }

        pub fn write_cmd_vel(&self, msg: &CmdVelWireMsg) -> Result<()> {
            let mut wire = zero_cmd_vel();
            write_fixed(&msg.host_boot_id, &mut wire.host_boot_id);
            write_fixed(&msg.producer_boot_id, &mut wire.producer_boot_id);
            wire.output_seq = msg.output_seq;
            wire.source_boottime_ns = msg.source_boottime_ns;
            wire.source_wall_ns = msg.source_wall_ns;
            wire.vx = msg.vx;
            wire.vy = msg.vy;
            wire.wz = msg.wz;
            let rc = unsafe { bridge_dds_test_peer_write_cmd_vel(self.peer, &wire) };
            if rc < 0 {
                bail!("peer write_cmd_vel failed: {rc}");
            }
            Ok(())
        }

        pub fn write_posture_request(&self, msg: &PostureRequestMsg) -> Result<()> {
            let mut wire = zero_posture_req();
            write_fixed(&msg.host_boot_id, &mut wire.host_boot_id);
            write_fixed(&msg.producer_boot_id, &mut wire.producer_boot_id);
            wire.source_boottime_ns = msg.source_boottime_ns;
            write_fixed(&msg.client_id, &mut wire.client_id);
            write_fixed(&msg.request_id, &mut wire.request_id);
            wire.action = msg.action;
            wire.phase = msg.phase;
            write_fixed(&msg.profile_name, &mut wire.profile_name);
            write_fixed(&msg.external_session_id, &mut wire.external_session_id);
            let rc = unsafe { bridge_dds_test_peer_write_posture_request(self.peer, &wire) };
            if rc < 0 {
                bail!("peer write_posture_request failed: {rc}");
            }
            Ok(())
        }

        pub fn take_imu_body(&self) -> Option<ImuBodySample> {
            let mut raw = BridgeImuMsg {
                sec: 0,
                nanosec: 0,
                qx: 0.0,
                qy: 0.0,
                qz: 0.0,
                qw: 1.0,
                gx: 0.0,
                gy: 0.0,
                gz: 0.0,
                ax: 0.0,
                ay: 0.0,
                az: 0.0,
            };
            let rc = unsafe { bridge_dds_test_peer_take_imu_body(self.peer, &mut raw) };
            if rc <= 0 {
                return None;
            }
            Some(ImuBodySample {
                stamp: DdsStamp {
                    sec: raw.sec,
                    nanosec: raw.nanosec,
                },
                orientation_xyzw: [raw.qx, raw.qy, raw.qz, raw.qw],
                angular_velocity_xyz: [raw.gx, raw.gy, raw.gz],
                linear_acceleration_xyz: [raw.ax, raw.ay, raw.az],
            })
        }

        pub fn take_joint_states(&self) -> Option<JointStateSample> {
            let mut raw = BridgeJointStateMsg {
                sec: 0,
                nanosec: 0,
                position: [0.0; 16],
                velocity: [0.0; 16],
                effort: [0.0; 16],
            };
            let mut len = 0_u32;
            let rc =
                unsafe { bridge_dds_test_peer_take_joint_states(self.peer, &mut raw, &mut len) };
            if rc <= 0 || len != 16 {
                return None;
            }
            Some(JointStateSample {
                stamp: DdsStamp {
                    sec: raw.sec,
                    nanosec: raw.nanosec,
                },
                robot_model: "thunder".into(),
                position: raw.position,
                velocity: raw.velocity,
                effort: raw.effort,
            })
        }

        pub fn take_control_state(&self) -> Option<DriverControlStateMsg> {
            let mut raw = BridgeControlStateMsg {
                sec: 0,
                nanosec: 0,
                connected: 0,
                ready: 0,
                motors_enabled: 0,
                critical_fault: 0,
                control_assured: 0,
                lease_valid: 0,
                lease_remaining_ms: 0,
                accepted_sequence: 0,
                zero_output: 0,
                fsm: [0; 64],
                owner: [0; 64],
                owner_id: [0; 128],
                reason: [0; 128],
            };
            let rc = unsafe { bridge_dds_test_peer_take_control_state(self.peer, &mut raw) };
            if rc <= 0 {
                return None;
            }
            Some(DriverControlStateMsg {
                stamp: DdsStamp {
                    sec: raw.sec,
                    nanosec: raw.nanosec,
                },
                connected: raw.connected != 0,
                ready: raw.ready != 0,
                motors_enabled: raw.motors_enabled != 0,
                critical_fault: raw.critical_fault != 0,
                control_assured: raw.control_assured != 0,
                lease_valid: raw.lease_valid != 0,
                lease_remaining_ms: raw.lease_remaining_ms,
                accepted_sequence: raw.accepted_sequence,
                zero_output: raw.zero_output != 0,
                fsm: read_fixed(&raw.fsm),
                owner: read_fixed(&raw.owner),
                owner_id: read_fixed(&raw.owner_id),
                reason: read_fixed(&raw.reason),
            })
        }

        pub fn take_posture_ack(&self) -> Option<PostureAckMsg> {
            let mut raw = BridgePostureAckMsg {
                sec: 0,
                nanosec: 0,
                producer_boot_id: [0; 128],
                client_id: [0; 128],
                request_id: [0; 128],
                action: 0,
                phase: 0,
                profile_name: [0; 128],
                external_session_id: [0; 128],
                final_result: 0,
                accepted: 0,
                reason: [0; 128],
            };
            let rc = unsafe { bridge_dds_test_peer_take_posture_ack(self.peer, &mut raw) };
            if rc <= 0 {
                return None;
            }
            Some(PostureAckMsg {
                stamp: DdsStamp {
                    sec: raw.sec,
                    nanosec: raw.nanosec,
                },
                producer_boot_id: read_fixed(&raw.producer_boot_id),
                client_id: read_fixed(&raw.client_id),
                request_id: read_fixed(&raw.request_id),
                action: raw.action,
                phase: raw.phase,
                profile_name: read_fixed(&raw.profile_name),
                external_session_id: read_fixed(&raw.external_session_id),
                final_result: raw.final_result != 0,
                accepted: raw.accepted != 0,
                reason: read_fixed(&raw.reason),
            })
        }
    }

    impl Drop for CycloneTestPeer {
        fn drop(&mut self) {
            if !self.peer.is_null() {
                unsafe { bridge_dds_test_peer_destroy(self.peer) };
                self.peer = ptr::null_mut();
            }
        }
    }
}
