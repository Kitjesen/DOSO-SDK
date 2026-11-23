use anyhow::{bail, Result};

use crate::hardware::robostride::{
    decode_rs_frame_with_limits, fault_bits_to_error_u8, RawCanFrame, RsDecodedState, RsEvent,
    RsKey,
};
use crate::hardware::CanBusFrame;
use crate::params::{LegId, RobotHardwareParams, NUM_JOINTS};
use crate::types::{ImuState, JointFeedback, MitCmd16, ProprioFrame};

pub const MOTORS_PER_LEG: usize = 4;
pub const RX_MMSG_BATCH: usize = 16;

/// Stack-allocated 4-frame CAN batch for a single leg (`can_id = 1..=4` in order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegCanBatch {
    pub leg: LegId,
    pub frames: [RawCanFrame; MOTORS_PER_LEG],
}

/// Encodes and partitions a 16-joint [`MitCmd16`] into 4 leg-grouped batches ordered
/// `[FR, FL, RR, RL]`, where each leg receives `can_id = 1..=4` (`[Hip, Thigh, Calf, Wheel]`)
/// with exact [`RsEvent::Control`] payloads and zero heap allocation.
pub fn partition_mit_cmd_by_leg_with_params(
    cmd: &MitCmd16,
    params: &RobotHardwareParams,
) -> Result<[LegCanBatch; 4]> {
    let zero_frame = RawCanFrame {
        id: 0,
        data: [0; 8],
    };
    let mut batches = [LegCanBatch {
        leg: LegId::Fr,
        frames: [zero_frame; MOTORS_PER_LEG],
    }; 4];

    for (leg_idx, leg) in LegId::ALL.into_iter().enumerate() {
        let mut leg_frames = [zero_frame; MOTORS_PER_LEG];
        for can_id in 1u8..=4u8 {
            let flat_idx = params
                .leg_can_to_flat_index(leg, can_id)
                .ok_or_else(|| anyhow::anyhow!("Missing joint for ({leg:?}, can_id={can_id})"))?;
            let limits = params
                .motor_limits_for_flat_index(flat_idx)
                .ok_or_else(|| anyhow::anyhow!("Missing motor limits for joint {flat_idx}"))?;
            let t = &cmd.0[flat_idx];
            leg_frames[(can_id - 1) as usize] =
                RsEvent::encode_control_with_limits(can_id, t.tau, t.q, t.dq, t.kp, t.kd, limits);
        }
        batches[leg_idx] = LegCanBatch {
            leg,
            frames: leg_frames,
        };
    }
    Ok(batches)
}

/// Encodes and partitions [`MitCmd16`] into 4 leg-grouped batches using `config/thunder_v4.yaml`.
pub fn partition_mit_cmd_by_leg(cmd: &MitCmd16) -> Result<[LegCanBatch; 4]> {
    partition_mit_cmd_by_leg_with_params(cmd, RobotHardwareParams::thunder_v4())
}

/// Builds the canonical 84-frame Thunder CAN bringup sequence with an explicit wheel CAN timeout:
/// 1. `disable_all(false)` across `FR/FL/RR/RL` `can_id = 1..=4` (16 frames)
/// 2. `disable_all(true)` (clear faults) across `FR/FL/RR/RL` `can_id = 1..=4` (16 frames)
/// 3. `set_cantimeout_us(4, wheel_can_timeout_us)` on all 4 legs' wheel motors `can_id = 4` (4 frames)
/// 4. `3x set_reporting(can_id, true)` for `can_id = 1..=4` across all 4 legs (48 frames)
pub fn build_can_startup_frames_with_timeout(
    wheel_can_timeout_us: u32,
) -> Result<Vec<CanBusFrame>> {
    let mut out = Vec::with_capacity(84);
    for leg in LegId::ALL {
        for can_id in 1..=4 {
            out.push(CanBusFrame {
                leg,
                frame: RsEvent::disable(can_id, false).encode()?,
            });
        }
    }
    for leg in LegId::ALL {
        for can_id in 1..=4 {
            out.push(CanBusFrame {
                leg,
                frame: RsEvent::disable(can_id, true).encode()?,
            });
        }
    }
    for leg in LegId::ALL {
        out.push(CanBusFrame {
            leg,
            frame: RsEvent::set_cantimeout_us(4, wheel_can_timeout_us).encode()?,
        });
    }
    for _retry in 0..3 {
        for leg in LegId::ALL {
            for can_id in 1..=4 {
                out.push(CanBusFrame {
                    leg,
                    frame: RsEvent::set_reporting(can_id, true).encode()?,
                });
            }
        }
    }
    Ok(out)
}

/// Builds the canonical 84-frame Thunder CAN bringup sequence using `config/thunder_v4.yaml`.
pub fn build_can_startup_frames() -> Result<Vec<CanBusFrame>> {
    build_can_startup_frames_with_timeout(
        RobotHardwareParams::thunder_v4()
            .safety
            .wheel_can_timeout_us,
    )
}

/// Builds the 12-frame `RsEvent::SetZero` batch for all leg motors (`can_id = 1..=3` across 4 legs).
pub fn build_leg_set_zero_frames() -> Result<Vec<CanBusFrame>> {
    let mut out = Vec::with_capacity(12);
    for leg in LegId::ALL {
        for can_id in 1..=3 {
            out.push(CanBusFrame {
                leg,
                frame: RsEvent::SetZero { can_id }.encode()?,
            });
        }
    }
    Ok(out)
}

/// Cross-platform deterministic CAN supervisor state machine backing `SocketCanHal` (and testable on Windows).
pub struct SocketCanCore {
    params: RobotHardwareParams,
    tx_enabled: bool,
    startup_sent: bool,
    motors_enabled: bool,
    disable_pending: bool,
    disable_clear_errors: bool,
    pending_disable_mask: u16,
    vbus_cursor: usize,
    seq: u64,
    imu: ImuState,
    joints: [JointFeedback; NUM_JOINTS],
    rx_frames: u64,
}

impl SocketCanCore {
    pub fn new(tx_enabled: bool) -> Result<(Self, Vec<CanBusFrame>)> {
        Self::with_hardware_params(RobotHardwareParams::thunder_v4(), tx_enabled)
    }

    pub fn with_hardware_params(
        params: &RobotHardwareParams,
        tx_enabled: bool,
    ) -> Result<(Self, Vec<CanBusFrame>)> {
        let mut core = Self {
            params: params.clone(),
            tx_enabled,
            startup_sent: false,
            motors_enabled: false,
            disable_pending: false,
            disable_clear_errors: false,
            pending_disable_mask: 0,
            vbus_cursor: 0,
            seq: 0,
            imu: ImuState::default(),
            joints: [JointFeedback::default(); NUM_JOINTS],
            rx_frames: 0,
        };
        let startup = core.maybe_take_startup_frames()?;
        Ok((core, startup))
    }

    pub fn hardware_params(&self) -> &RobotHardwareParams {
        &self.params
    }

    pub fn tx_enabled(&self) -> bool {
        self.tx_enabled
    }

    pub fn motors_enabled(&self) -> bool {
        self.motors_enabled
    }

    pub fn disable_pending(&self) -> bool {
        self.disable_pending
    }

    pub fn rx_frames(&self) -> u64 {
        self.rx_frames
    }

    pub fn joints(&self) -> &[JointFeedback; NUM_JOINTS] {
        &self.joints
    }

    pub fn set_imu(&mut self, imu: ImuState) {
        self.imu = imu;
    }

    pub fn set_tx_enabled(&mut self, enabled: bool) -> Result<Vec<CanBusFrame>> {
        self.tx_enabled = enabled;
        if !enabled {
            self.motors_enabled = false;
            self.disable_pending = false;
            self.pending_disable_mask = 0;
            return Ok(Vec::new());
        }
        self.maybe_take_startup_frames()
    }

    fn maybe_take_startup_frames(&mut self) -> Result<Vec<CanBusFrame>> {
        if self.tx_enabled && !self.startup_sent {
            self.startup_sent = true;
            build_can_startup_frames_with_timeout(self.params.safety.wheel_can_timeout_us)
        } else {
            Ok(Vec::new())
        }
    }

    pub fn motor_disable_confirmed(&self) -> bool {
        if !self.tx_enabled {
            return true;
        }
        !self.motors_enabled
            && !self.disable_pending
            && self
                .joints
                .iter()
                .filter(|j| j.online)
                .all(|j| j.status_code == 0)
    }

    pub fn ingest_rx_frame(&mut self, leg: LegId, raw: RawCanFrame, now_us: u64) {
        self.rx_frames += 1;
        let Ok(decoded) = decode_rs_frame_with_limits(raw, |can_id| {
            self.params
                .leg_can_to_flat_index(leg, can_id)
                .and_then(|idx| self.params.motor_limits_for_flat_index(idx))
                .ok_or_else(|| anyhow::anyhow!("Invalid ({leg:?}, can_id={can_id})"))
        }) else {
            return;
        };
        match decoded {
            RsDecodedState::Motion(m) => {
                if let Some(flat_idx) = self.params.leg_can_to_flat_index(leg, m.can_id) {
                    let slot = &mut self.joints[flat_idx];
                    slot.q = m.position;
                    slot.dq = m.velocity;
                    slot.tau = m.torque;
                    slot.temp_c = m.temperature_c;
                    slot.status_code = m.status_code;
                    slot.error_bits = m.error_bits;
                    slot.last_seen_us = now_us;
                    slot.online = true;
                    if m.status_code == 0 {
                        self.pending_disable_mask &= !(1u16 << flat_idx);
                        self.disable_pending = self.pending_disable_mask != 0;
                    }
                }
            }
            RsDecodedState::ParamFloat {
                can_id,
                key: RsKey::Vbus,
                value,
                ..
            } => {
                if let Some(flat_idx) = self.params.leg_can_to_flat_index(leg, can_id) {
                    self.joints[flat_idx].vbus_v = value;
                }
            }
            RsDecodedState::FaultFrame {
                can_id, fault_bits, ..
            } => {
                if let Some(flat_idx) = self.params.leg_can_to_flat_index(leg, can_id) {
                    let slot = &mut self.joints[flat_idx];
                    slot.error_bits = fault_bits_to_error_u8(fault_bits);
                    slot.last_seen_us = now_us;
                    slot.online = true;
                }
            }
            _ => {}
        }
    }

    /// Updates joint staleness, schedules round-robin `get_vbus` + any unconfirmed disable retries,
    /// and returns `ProprioFrame`.
    pub fn sample_proprio_step(
        &mut self,
        now_us: u64,
        mut on_tx: impl FnMut(CanBusFrame),
    ) -> ProprioFrame {
        self.seq = self.seq.wrapping_add(1);
        let stale_timeout_us = self.params.safety.max_proprio_age_us;
        for j in &mut self.joints {
            if j.last_seen_us == 0
                || now_us < j.last_seen_us
                || now_us.saturating_sub(j.last_seen_us) > stale_timeout_us
            {
                j.online = false;
            }
        }

        if self.tx_enabled {
            // Re-send disable to any joint whose disable confirmation is still pending
            if self.disable_pending {
                for idx in 0..NUM_JOINTS {
                    if (self.pending_disable_mask & (1u16 << idx)) != 0 {
                        let (leg, can_id) = self.params.flat_index_to_leg_can(idx).unwrap();
                        if let Ok(frame) =
                            RsEvent::disable(can_id, self.disable_clear_errors).encode()
                        {
                            on_tx(CanBusFrame { leg, frame });
                        }
                    }
                }
            }

            // Periodic round-robin get_vbus query across all 16 motors
            let idx = self.vbus_cursor % NUM_JOINTS;
            self.vbus_cursor = (idx + 1) % NUM_JOINTS;
            let (leg, can_id) = self.params.flat_index_to_leg_can(idx).unwrap();
            if let Ok(frame) = RsEvent::get_vbus(can_id).encode() {
                on_tx(CanBusFrame { leg, frame });
            }
        }

        ProprioFrame {
            sequence: self.seq,
            monotonic_us: now_us,
            imu: self.imu,
            joints: self.joints,
        }
    }

    pub fn enable_motors(&mut self) -> Result<Vec<CanBusFrame>> {
        if self.tx_enabled && !self.motor_disable_confirmed() {
            bail!("Motor enable blocked: previous disable not yet confirmed across online joints");
        }
        if !self.tx_enabled {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(16);
        for leg in LegId::ALL {
            for can_id in 1..=4 {
                out.push(CanBusFrame {
                    leg,
                    frame: RsEvent::enable(can_id).encode()?,
                });
            }
        }
        self.motors_enabled = true;
        self.disable_pending = false;
        self.pending_disable_mask = 0;
        Ok(out)
    }

    pub fn disable_motors(&mut self, clear_errors: bool) -> Result<Vec<CanBusFrame>> {
        self.motors_enabled = false;
        self.disable_clear_errors = clear_errors;
        if !self.tx_enabled {
            self.disable_pending = false;
            self.pending_disable_mask = 0;
            return Ok(Vec::new());
        }

        let mut mask = self.pending_disable_mask;
        for (idx, j) in self.joints.iter().enumerate() {
            if j.online && j.status_code != 0 {
                mask |= 1u16 << idx;
            }
        }
        self.pending_disable_mask = mask;
        self.disable_pending = mask != 0;

        let mut out = Vec::with_capacity(16);
        for leg in LegId::ALL {
            for can_id in 1..=4 {
                out.push(CanBusFrame {
                    leg,
                    frame: RsEvent::disable(can_id, clear_errors).encode()?,
                });
            }
        }
        Ok(out)
    }

    pub fn encode_mit_cmd(&self, cmd: &MitCmd16) -> Result<Vec<CanBusFrame>> {
        if !self.tx_enabled || self.disable_pending || !self.motors_enabled {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(NUM_JOINTS);
        for (idx, t) in cmd.0.iter().enumerate() {
            let (leg, can_id) = self.params.flat_index_to_leg_can(idx).unwrap();
            let limits = self.params.motor_limits_for_flat_index(idx).unwrap();
            let frame =
                RsEvent::encode_control_with_limits(can_id, t.tau, t.q, t.dq, t.kp, t.kd, limits);
            out.push(CanBusFrame { leg, frame });
        }
        Ok(out)
    }

    /// Encodes [`MitCmd16`] into 4 stack-allocated [`LegCanBatch`]es (`FR`, `FL`, `RR`, `RL`,
    /// each `can_id = 1..=4`) when transmission is active, or `Ok(None)` when gated off.
    pub fn encode_mit_cmd_by_leg(&self, cmd: &MitCmd16) -> Result<Option<[LegCanBatch; 4]>> {
        if !self.tx_enabled || self.disable_pending || !self.motors_enabled {
            return Ok(None);
        }
        partition_mit_cmd_by_leg_with_params(cmd, &self.params).map(Some)
    }

    pub fn calibrate_set_zero(&mut self) -> Result<Vec<CanBusFrame>> {
        if !self.tx_enabled {
            bail!("calibrate_set_zero refused: tx_enabled is false");
        }
        if self.motors_enabled {
            bail!("calibrate_set_zero refused: motors must be disabled before setting zero");
        }
        if !self.motor_disable_confirmed() {
            bail!("calibrate_set_zero refused: motor disable is not confirmed");
        }
        build_leg_set_zero_frames()
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod linux_hal {
    use std::path::Path;
    use std::thread;
    use std::time::Duration;

    use anyhow::{bail, Result};

    use crate::hardware::remote_dev::LinuxRemoteInputReader;
    use crate::hardware::robostride::RawCanFrame;
    use crate::hardware::serial_imu::{resolve_imu_baud_from_env_or, SerialImuReader};
    use crate::hardware::{CanBusFrame, HalBackend, HalDiagnostics};
    use crate::params::{DeviceEndpoints, LegId, RobotHardwareParams};
    use crate::types::{MitCmd16, ProprioFrame, RemotePadState};

    use super::{SocketCanCore, MOTORS_PER_LEG, RX_MMSG_BATCH};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LinuxCanFrame {
        can_id: u32,
        len: u8,
        pad: u8,
        res0: u8,
        len8_dlc: u8,
        data: [u8; 8],
    }

    impl LinuxCanFrame {
        const ZERO: Self = Self {
            can_id: 0,
            len: 0,
            pad: 0,
            res0: 0,
            len8_dlc: 0,
            data: [0; 8],
        };
    }

    const _: () = assert!(std::mem::size_of::<LinuxCanFrame>() == 16);

    /// Native Linux SocketCAN (`can0..can3`) backend with Hipnuc Hi91 serial IMU,
    /// YunZhuo / Linux `/dev/input/js*` remote input, round-robin Vbus polling, and verified disable.
    pub struct SocketCanHal {
        fds: [i32; 4],
        core: SocketCanCore,
        imu_reader: Option<SerialImuReader>,
        remote_reader: Option<LinuxRemoteInputReader>,
        tx_frames: u64,
        tx_dropped: u64,
    }

    impl SocketCanHal {
        const AF_CAN: i32 = 29;
        const SOCK_RAW: i32 = 3;
        const CAN_RAW: i32 = 1;
        const CAN_EFF_FLAG: u32 = 0x8000_0000;
        const CAN_EFF_MASK: u32 = 0x1FFF_FFFF;

        /// Opens `FR/FL/RR/RL` SocketCAN interfaces configured in `params` (with env overrides),
        /// executes the 84-frame CAN startup sequence if `tx_enabled`, and wires optional IMU & remote readers.
        pub fn open_with_params(params: &RobotHardwareParams, tx_enabled: bool) -> Result<Self> {
            let env_or = |key: &str, fallback: &str| -> String {
                std::env::var(key)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| fallback.to_string())
            };
            let ifaces = [
                env_or("DOSO_CAN_FR", &params.can_buses.fr),
                env_or("DOSO_CAN_FL", &params.can_buses.fl),
                env_or("DOSO_CAN_RR", &params.can_buses.rr),
                env_or("DOSO_CAN_RL", &params.can_buses.rl),
            ];

            let mut fds = [-1i32; 4];
            for (i, ifname) in ifaces.iter().enumerate() {
                if let Err(err) = Self::open_one(ifname).map(|fd| fds[i] = fd) {
                    for &opened in &fds[..i] {
                        if opened >= 0 {
                            unsafe { libc::close(opened) };
                        }
                    }
                    return Err(err);
                }
            }

            let imu_reader = match Self::init_optional_imu(&params.devices) {
                Ok(reader) => reader,
                Err(err) => {
                    for &fd in &fds {
                        if fd >= 0 {
                            unsafe { libc::close(fd) };
                        }
                    }
                    return Err(err);
                }
            };
            let remote_reader = Self::init_optional_remote(&params.devices);

            let (core, startup_frames) = SocketCanCore::with_hardware_params(params, tx_enabled)?;
            let mut hal = Self {
                fds,
                core,
                imu_reader,
                remote_reader,
                tx_frames: 0,
                tx_dropped: 0,
            };
            hal.send_startup_batch(&startup_frames);
            Ok(hal)
        }

        /// Convenience constructor opening explicit `FR/FL/RR/RL` SocketCAN interfaces with Thunder V4 defaults.
        pub fn open(ifaces_fr_fl_rr_rl: [&str; 4], tx_enabled: bool) -> Result<Self> {
            let mut params = RobotHardwareParams::thunder_v4().clone();
            params.can_buses.fr = ifaces_fr_fl_rr_rl[0].to_string();
            params.can_buses.fl = ifaces_fr_fl_rr_rl[1].to_string();
            params.can_buses.rr = ifaces_fr_fl_rr_rl[2].to_string();
            params.can_buses.rl = ifaces_fr_fl_rr_rl[3].to_string();
            Self::open_with_params(&params, tx_enabled)
        }

        fn init_optional_imu(devices: &DeviceEndpoints) -> Result<Option<SerialImuReader>> {
            let imu_required = std::env::var("DOSO_IMU_REQUIRED")
                .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
                .unwrap_or(false);
            let port_opt = std::env::var("DOSO_IMU_PORT")
                .ok()
                .or_else(|| std::env::var("THUNDER_IMU_PORT").ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    if Path::new(&devices.imu_port).exists() {
                        Some(devices.imu_port.clone())
                    } else {
                        None
                    }
                });

            let Some(port) = port_opt else {
                if imu_required {
                    bail!(
                        "DOSO_IMU_REQUIRED=true but DOSO_IMU_PORT is not set and {} is absent",
                        devices.imu_port
                    );
                }
                return Ok(None);
            };

            let baud = match resolve_imu_baud_from_env_or(devices.imu_baud) {
                Ok(b) => b,
                Err(e) => {
                    if imu_required {
                        return Err(e);
                    }
                    eprintln!(
                        "doso-runtime: invalid DOSO_IMU_BAUD ({e}); falling back to {}",
                        devices.imu_baud
                    );
                    devices.imu_baud
                }
            };

            match SerialImuReader::open(&port, baud) {
                Ok(r) => Ok(Some(r)),
                Err(e) => {
                    if imu_required {
                        Err(e)
                    } else {
                        eprintln!("doso-runtime: IMU serial unavailable on {port}@{baud}: {e}");
                        Ok(None)
                    }
                }
            }
        }

        fn init_optional_remote(devices: &DeviceEndpoints) -> Option<LinuxRemoteInputReader> {
            let disabled = std::env::var("DOSO_REMOTE_DISABLED")
                .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
                .unwrap_or(false);
            if disabled {
                return None;
            }
            let serial_port = std::env::var("DOSO_REMOTE_PORT")
                .ok()
                .or_else(|| std::env::var("THUNDER_YUNZHUO_PORT").ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| Some(devices.remote_port.clone()));
            let js_dev = std::env::var("DOSO_JOYSTICK_DEV")
                .ok()
                .or_else(|| std::env::var("THUNDER_XBOX_DEVICE").ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| devices.joystick_dev.clone());
            Some(LinuxRemoteInputReader::spawn(serial_port, js_dev))
        }

        fn send_startup_batch(&mut self, frames: &[CanBusFrame]) {
            if frames.is_empty() {
                return;
            }
            // Pace batches slightly across the 84-frame sequence so hardware CAN TX queues never drop
            for (idx, item) in frames.iter().enumerate() {
                self.send_raw(item.leg, item.frame);
                if idx == 15 || idx == 31 || idx == 35 || idx == 51 || idx == 67 {
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }

        fn open_one(ifname: &str) -> Result<i32> {
            unsafe {
                let fd = libc::socket(Self::AF_CAN, Self::SOCK_RAW, Self::CAN_RAW);
                if fd < 0 {
                    bail!(
                        "socket(AF_CAN) failed for {ifname}: {}",
                        std::io::Error::last_os_error()
                    );
                }
                #[repr(C)]
                struct Ifreq {
                    ifr_name: [u8; 16],
                    ifr_ifindex: i32,
                    _pad: [u8; 20],
                }
                let mut ifr = Ifreq {
                    ifr_name: [0; 16],
                    ifr_ifindex: 0,
                    _pad: [0; 20],
                };
                let bytes = ifname.as_bytes();
                let n = bytes.len().min(15);
                ifr.ifr_name[..n].copy_from_slice(&bytes[..n]);
                if libc::ioctl(fd, 0x8933, &mut ifr) < 0 {
                    let err = std::io::Error::last_os_error();
                    libc::close(fd);
                    bail!("ioctl(SIOCGIFINDEX) failed for {ifname}: {err}");
                }
                #[repr(C)]
                struct SockaddrCan {
                    can_family: u16,
                    can_ifindex: i32,
                    rx_id: u32,
                    tx_id: u32,
                }
                let addr = SockaddrCan {
                    can_family: Self::AF_CAN as u16,
                    can_ifindex: ifr.ifr_ifindex,
                    rx_id: 0,
                    tx_id: 0,
                };
                if libc::bind(
                    fd,
                    (&addr as *const SockaddrCan).cast::<libc::sockaddr>(),
                    std::mem::size_of::<SockaddrCan>() as libc::socklen_t,
                ) < 0
                {
                    let err = std::io::Error::last_os_error();
                    libc::close(fd);
                    bail!("bind(AF_CAN) failed for {ifname}: {err}");
                }
                let buf_sz: i32 = 262_144;
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_RCVBUF,
                    (&buf_sz as *const i32).cast(),
                    std::mem::size_of::<i32>() as libc::socklen_t,
                );
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&buf_sz as *const i32).cast(),
                    std::mem::size_of::<i32>() as libc::socklen_t,
                );
                Ok(fd)
            }
        }

        fn poll_rx(&mut self, now_us: u64) {
            let frame_sz = std::mem::size_of::<LinuxCanFrame>();
            for (leg_idx, &fd) in self.fds.iter().enumerate() {
                if fd < 0 {
                    continue;
                }
                let leg = LegId::from_index(leg_idx).unwrap();
                loop {
                    let mut cfs = [LinuxCanFrame::ZERO; RX_MMSG_BATCH];
                    let mut iovecs: [libc::iovec; RX_MMSG_BATCH] = unsafe { std::mem::zeroed() };
                    let mut hdrs: [libc::mmsghdr; RX_MMSG_BATCH] = unsafe { std::mem::zeroed() };

                    for ((cf_slot, iov_slot), hdr_slot) in
                        cfs.iter_mut().zip(iovecs.iter_mut()).zip(hdrs.iter_mut())
                    {
                        iov_slot.iov_base = (cf_slot as *mut LinuxCanFrame).cast::<libc::c_void>();
                        iov_slot.iov_len = frame_sz;
                        hdr_slot.msg_hdr.msg_iov = iov_slot as *mut libc::iovec;
                        hdr_slot.msg_hdr.msg_iovlen = 1;
                    }

                    let count = unsafe {
                        libc::recvmmsg(
                            fd,
                            hdrs.as_mut_ptr(),
                            RX_MMSG_BATCH as libc::c_uint,
                            libc::MSG_DONTWAIT,
                            std::ptr::null_mut(),
                        )
                    };
                    if count <= 0 {
                        break;
                    }
                    let received = (count as usize).min(RX_MMSG_BATCH);
                    for (cf, hdr) in cfs[..received].iter().zip(hdrs[..received].iter()) {
                        if (hdr.msg_len as usize) < frame_sz {
                            continue;
                        }
                        if (cf.can_id & Self::CAN_EFF_FLAG) == 0 || cf.len != 8 {
                            continue;
                        }
                        let raw = RawCanFrame {
                            id: cf.can_id & Self::CAN_EFF_MASK,
                            data: cf.data,
                        };
                        self.core.ingest_rx_frame(leg, raw, now_us);
                    }
                    if received < RX_MMSG_BATCH {
                        break;
                    }
                }
            }
        }

        fn send_batch_leg(&mut self, leg: LegId, frames: &[RawCanFrame]) {
            if frames.is_empty() || !self.core.tx_enabled() {
                return;
            }
            let fd = self.fds[leg as usize];
            if fd < 0 {
                self.tx_dropped += frames.len() as u64;
                return;
            }

            let frame_sz = std::mem::size_of::<LinuxCanFrame>();
            for chunk in frames.chunks(MOTORS_PER_LEG) {
                let batch_len = chunk.len();
                let mut cfs = [LinuxCanFrame::ZERO; MOTORS_PER_LEG];
                let mut iovecs: [libc::iovec; MOTORS_PER_LEG] = unsafe { std::mem::zeroed() };
                let mut hdrs: [libc::mmsghdr; MOTORS_PER_LEG] = unsafe { std::mem::zeroed() };

                for ((cf_slot, iov_slot), (hdr_slot, raw)) in cfs[..batch_len]
                    .iter_mut()
                    .zip(iovecs[..batch_len].iter_mut())
                    .zip(hdrs[..batch_len].iter_mut().zip(chunk.iter()))
                {
                    *cf_slot = LinuxCanFrame {
                        can_id: raw.id | Self::CAN_EFF_FLAG,
                        len: 8,
                        pad: 0,
                        res0: 0,
                        len8_dlc: 0,
                        data: raw.data,
                    };
                    iov_slot.iov_base = (cf_slot as *mut LinuxCanFrame).cast::<libc::c_void>();
                    iov_slot.iov_len = frame_sz;
                    hdr_slot.msg_hdr.msg_iov = iov_slot as *mut libc::iovec;
                    hdr_slot.msg_hdr.msg_iovlen = 1;
                }

                let rc = unsafe {
                    libc::sendmmsg(
                        fd,
                        hdrs.as_mut_ptr(),
                        batch_len as libc::c_uint,
                        libc::MSG_DONTWAIT,
                    )
                };
                let sent = if rc > 0 {
                    (rc as usize).min(batch_len)
                } else {
                    0
                };
                self.tx_frames += sent as u64;
                self.tx_dropped += (batch_len - sent) as u64;
            }
        }

        fn send_raw_checked(&mut self, leg: LegId, raw: RawCanFrame) -> bool {
            if !self.core.tx_enabled() {
                return false;
            }
            let fd = self.fds[leg as usize];
            if fd < 0 {
                self.tx_dropped += 1;
                return false;
            }
            let cf = LinuxCanFrame {
                can_id: raw.id | Self::CAN_EFF_FLAG,
                len: 8,
                pad: 0,
                res0: 0,
                len8_dlc: 0,
                data: raw.data,
            };
            let written = unsafe {
                libc::send(
                    fd,
                    (&cf as *const LinuxCanFrame).cast(),
                    std::mem::size_of::<LinuxCanFrame>(),
                    libc::MSG_DONTWAIT,
                )
            };
            if written == std::mem::size_of::<LinuxCanFrame>() as isize {
                self.tx_frames += 1;
                true
            } else {
                self.tx_dropped += 1;
                false
            }
        }

        fn send_raw(&mut self, leg: LegId, raw: RawCanFrame) {
            let _ = self.send_raw_checked(leg, raw);
        }
    }

    impl Drop for SocketCanHal {
        fn drop(&mut self) {
            for &fd in &self.fds {
                if fd >= 0 {
                    unsafe {
                        libc::close(fd);
                    }
                }
            }
        }
    }

    impl HalBackend for SocketCanHal {
        fn name(&self) -> &'static str {
            "socketcan-rdk-s100"
        }

        fn sample_proprio(&mut self, now_us: u64) -> ProprioFrame {
            self.poll_rx(now_us);
            if let Some(ref imu_reader) = self.imu_reader {
                self.core.set_imu(imu_reader.sample(now_us));
            }
            let mut maintenance = Vec::with_capacity(4);
            let frame = self.core.sample_proprio_step(now_us, |cb| {
                maintenance.push(cb);
            });
            for item in maintenance {
                self.send_raw(item.leg, item.frame);
            }
            frame
        }

        fn sample_remote(&mut self, now_us: u64) -> RemotePadState {
            if let Some(ref reader) = self.remote_reader {
                reader.sample(now_us)
            } else {
                RemotePadState::default()
            }
        }

        fn set_tx_enabled(&mut self, enabled: bool) {
            if let Ok(startup) = self.core.set_tx_enabled(enabled) {
                self.send_startup_batch(&startup);
            }
        }

        fn tx_enabled(&self) -> bool {
            self.core.tx_enabled()
        }

        fn enable_motors(&mut self) -> Result<()> {
            let frames = self.core.enable_motors()?;
            for item in frames {
                self.send_raw(item.leg, item.frame);
            }
            Ok(())
        }

        fn disable_motors(&mut self, clear_errors: bool) -> Result<()> {
            let frames = self.core.disable_motors(clear_errors)?;
            for item in frames {
                self.send_raw(item.leg, item.frame);
            }
            Ok(())
        }

        fn send_mit_verified(&mut self, cmd: &MitCmd16) -> Result<()> {
            if let Some(batches) = self.core.encode_mit_cmd_by_leg(cmd)? {
                for batch in &batches {
                    self.send_batch_leg(batch.leg, &batch.frames);
                }
            }
            Ok(())
        }

        fn calibrate_set_zero(&mut self) -> Result<()> {
            let frames = self.core.calibrate_set_zero()?;
            for item in frames {
                if !self.send_raw_checked(item.leg, item.frame) {
                    bail!(
                        "calibrate_set_zero aborted: SocketCAN immediate write failed on {:?}",
                        item.leg
                    );
                }
            }
            Ok(())
        }

        fn motor_disable_confirmed(&self) -> bool {
            self.core.motor_disable_confirmed()
        }

        fn diagnostics(&self) -> HalDiagnostics {
            HalDiagnostics {
                backend_name: self.name(),
                tx_enabled: self.core.tx_enabled(),
                tx_frames: self.tx_frames,
                rx_frames: self.core.rx_frames(),
                tx_dropped: self.tx_dropped,
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux_hal::SocketCanHal;
