use anyhow::{bail, Result};
use doso_core::{
    flat_index_to_leg_can, leg_can_to_flat_index, ImuState, JointFeedback, LegId, MitCmd16,
    ProprioFrame, NUM_JOINTS,
};

use crate::robostride::{
    decode_rs_frame, fault_bits_to_error_u8, RawCanFrame, RsDecodedState, RsEvent, RsKey,
};
use crate::traits::CanBusFrame;

pub const WHEEL_CAN_TIMEOUT_US: u32 = 500_000;
pub const JOINT_STALE_TIMEOUT_US: u64 = 200_000;

/// Builds the canonical 84-frame Thunder CAN bringup sequence matching `thunder.dart`:
/// 1. `disable_all(false)` across `FR/FL/RR/RL` `can_id = 1..=4` (16 frames)
/// 2. `disable_all(true)` (clear faults) across `FR/FL/RR/RL` `can_id = 1..=4` (16 frames)
/// 3. `set_cantimeout_us(4, 500_000)` on all 4 legs' wheel motors `can_id = 4` (4 frames)
/// 4. `3x set_reporting(can_id, true)` for `can_id = 1..=4` across all 4 legs (48 frames)
pub fn build_can_startup_frames() -> Result<Vec<CanBusFrame>> {
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
            frame: RsEvent::set_cantimeout_us(4, WHEEL_CAN_TIMEOUT_US).encode()?,
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
        let mut core = Self {
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
            build_can_startup_frames()
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
        let Ok(decoded) = decode_rs_frame(raw) else {
            return;
        };
        match decoded {
            RsDecodedState::Motion(m) => {
                if let Some(flat_idx) = leg_can_to_flat_index(leg, m.can_id) {
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
                if let Some(flat_idx) = leg_can_to_flat_index(leg, can_id) {
                    self.joints[flat_idx].vbus_v = value;
                }
            }
            RsDecodedState::FaultFrame {
                can_id, fault_bits, ..
            } => {
                if let Some(flat_idx) = leg_can_to_flat_index(leg, can_id) {
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
    /// and returns `(ProprioFrame, outgoing_maintenance_frames)`.
    pub fn sample_proprio_step(
        &mut self,
        now_us: u64,
        mut on_tx: impl FnMut(CanBusFrame),
    ) -> ProprioFrame {
        self.seq = self.seq.wrapping_add(1);
        for j in &mut self.joints {
            if j.last_seen_us == 0
                || now_us < j.last_seen_us
                || now_us.saturating_sub(j.last_seen_us) > JOINT_STALE_TIMEOUT_US
            {
                j.online = false;
            }
        }

        if self.tx_enabled {
            // Re-send disable to any joint whose disable confirmation is still pending
            if self.disable_pending {
                for idx in 0..NUM_JOINTS {
                    if (self.pending_disable_mask & (1u16 << idx)) != 0 {
                        let (leg, can_id) = flat_index_to_leg_can(idx).unwrap();
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
            let (leg, can_id) = flat_index_to_leg_can(idx).unwrap();
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
            bail!(
                "Motor enable blocked: previous disable not yet confirmed across online joints"
            );
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
            let (leg, can_id) = flat_index_to_leg_can(idx).unwrap();
            let frame = RsEvent::Control {
                can_id,
                torque: t.tau,
                position: t.q,
                velocity: t.dq,
                kp: t.kp,
                kd: t.kd,
            }
            .encode()?;
            out.push(CanBusFrame { leg, frame });
        }
        Ok(out)
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
mod linux_hal {
    use std::path::Path;
    use std::thread;
    use std::time::Duration;

    use anyhow::{bail, Result};
    use doso_core::{LegId, MitCmd16, ProprioFrame, RemotePadState};

    use crate::remote_input::LinuxRemoteInputReader;
    use crate::robostride::RawCanFrame;
    use crate::serial_imu::{resolve_imu_baud_from_env, SerialImuReader, DEFAULT_IMU_BAUD};
    use crate::traits::{CanBusFrame, HalBackend, HalDiagnostics};

    use super::SocketCanCore;

    #[repr(C)]
    struct LinuxCanFrame {
        can_id: u32,
        len: u8,
        pad: u8,
        res0: u8,
        len8_dlc: u8,
        data: [u8; 8],
    }

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

        /// Opens `FR/FL/RR/RL` SocketCAN interfaces (`["can0", "can2", "can1", "can3"]` on RDK S100),
        /// executes CAN startup sequence if `tx_enabled`, and wires optional IMU & remote input readers.
        pub fn open(ifaces_fr_fl_rr_rl: [&str; 4], tx_enabled: bool) -> Result<Self> {
            let mut fds = [-1i32; 4];
            for (i, ifname) in ifaces_fr_fl_rr_rl.iter().enumerate() {
                if let Err(err) = Self::open_one(ifname).map(|fd| fds[i] = fd) {
                    for &opened in &fds[..i] {
                        if opened >= 0 {
                            unsafe { libc::close(opened) };
                        }
                    }
                    return Err(err);
                }
            }

            let imu_reader = match Self::init_optional_imu() {
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
            let remote_reader = Self::init_optional_remote();

            let (core, startup_frames) = SocketCanCore::new(tx_enabled)?;
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

        fn init_optional_imu() -> Result<Option<SerialImuReader>> {
            let imu_required = std::env::var("DOSO_IMU_REQUIRED")
                .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
                .unwrap_or(false);
            let port_opt = std::env::var("DOSO_IMU_PORT")
                .ok()
                .or_else(|| std::env::var("THUNDER_IMU_PORT").ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    if Path::new("/dev/imu").exists() {
                        Some("/dev/imu".to_string())
                    } else {
                        None
                    }
                });

            let Some(port) = port_opt else {
                if imu_required {
                    bail!("DOSO_IMU_REQUIRED=true but DOSO_IMU_PORT is not set and /dev/imu is absent");
                }
                return Ok(None);
            };

            let baud = match resolve_imu_baud_from_env() {
                Ok(b) => b,
                Err(e) => {
                    if imu_required {
                        return Err(e);
                    }
                    eprintln!("doso-hal: invalid DOSO_IMU_BAUD ({e}); falling back to 921600");
                    DEFAULT_IMU_BAUD
                }
            };

            match SerialImuReader::open(&port, baud) {
                Ok(r) => Ok(Some(r)),
                Err(e) => {
                    if imu_required {
                        Err(e)
                    } else {
                        eprintln!("doso-hal: IMU serial unavailable on {port}@{baud}: {e}");
                        Ok(None)
                    }
                }
            }
        }

        fn init_optional_remote() -> Option<LinuxRemoteInputReader> {
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
                .or_else(|| Some("/dev/yunzhuo".to_string()));
            let js_dev = std::env::var("DOSO_JOYSTICK_DEV")
                .ok()
                .or_else(|| std::env::var("THUNDER_XBOX_DEVICE").ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "/dev/input/js0".to_string());
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
            for (leg_idx, &fd) in self.fds.iter().enumerate() {
                if fd < 0 {
                    continue;
                }
                let leg = LegId::from_index(leg_idx).unwrap();
                loop {
                    let mut cf = LinuxCanFrame {
                        can_id: 0,
                        len: 0,
                        pad: 0,
                        res0: 0,
                        len8_dlc: 0,
                        data: [0; 8],
                    };
                    let n = unsafe {
                        libc::recv(
                            fd,
                            (&mut cf as *mut LinuxCanFrame).cast(),
                            std::mem::size_of::<LinuxCanFrame>(),
                            libc::MSG_DONTWAIT,
                        )
                    };
                    if n < std::mem::size_of::<LinuxCanFrame>() as isize {
                        break;
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
            let frames = self.core.encode_mit_cmd(cmd)?;
            for item in frames {
                self.send_raw(item.leg, item.frame);
            }
            Ok(())
        }

        fn calibrate_set_zero(&mut self) -> Result<()> {
            let frames = self.core.calibrate_set_zero()?;
            for item in frames {
                if !self.send_raw_checked(item.leg, item.frame) {
                    bail!("calibrate_set_zero aborted: SocketCAN immediate write failed on {:?}", item.leg);
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
