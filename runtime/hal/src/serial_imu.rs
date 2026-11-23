use anyhow::{bail, Result};
use doso_core::ImuState;

use crate::hipnuc::{decode_hi91_payload, ProtoframeStreamDecoder};

pub const DEFAULT_IMU_BAUD: u32 = 921_600;
pub const IMU_STALE_TIMEOUT_US: u64 = 200_000;

/// Parses and validates Hipnuc Hi91 UART baud rate (`115200`, `460800`, `921600`).
pub fn parse_imu_baud(raw: Option<&str>) -> Result<u32> {
    let Some(trimmed) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(DEFAULT_IMU_BAUD);
    };
    let baud: u32 = trimmed
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid DOSO_IMU_BAUD '{trimmed}'"))?;
    match baud {
        115_200 | 460_800 | 921_600 => Ok(baud),
        other => bail!(
            "Unsupported IMU baud rate {other}; expected 115200, 460800, or 921600"
        ),
    }
}

/// Resolves IMU baud rate from `DOSO_IMU_BAUD`, defaulting to `921600`.
pub fn resolve_imu_baud_from_env() -> Result<u32> {
    let val = std::env::var("DOSO_IMU_BAUD").ok();
    parse_imu_baud(val.as_deref())
}

/// Cross-platform streaming Hipnuc Hi91 serial state buffer with 200ms staleness & disconnect gating.
#[derive(Default)]
pub struct ImuSerialState {
    decoder: ProtoframeStreamDecoder,
    latest: ImuState,
    connected: bool,
    frames_decoded: u64,
}

impl ImuSerialState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn frames_decoded(&self) -> u64 {
        self.frames_decoded
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Marks serial link as disconnected and immediately forces `online = false`.
    pub fn mark_disconnected(&mut self) {
        self.connected = false;
        self.latest.online = false;
    }

    pub fn mark_connected(&mut self) {
        self.connected = true;
    }

    /// Streams an arbitrary serial byte chunk through `ProtoframeStreamDecoder` + `decode_hi91_payload`.
    /// Returns the number of valid Hi91 frames decoded in this chunk.
    pub fn push_chunk(&mut self, chunk: &[u8], now_us: u64) -> usize {
        let mut count = 0usize;
        let latest = &mut self.latest;
        let connected = &mut self.connected;
        let total = &mut self.frames_decoded;
        self.decoder.push_and_decode(chunk, |payload| {
            if let Ok(imu) = decode_hi91_payload(payload, now_us) {
                *latest = imu;
                *connected = true;
                *total += 1;
                count += 1;
            }
        });
        count
    }

    /// Samples the latest `ImuState` at `now_us`, marking `online = false` if disconnected
    /// or older than `IMU_STALE_TIMEOUT_US` (`200_000 us`).
    pub fn sample(&mut self, now_us: u64) -> ImuState {
        if !self.connected
            || !self.latest.online
            || (now_us < self.latest.timestamp_us)
            || now_us.saturating_sub(self.latest.timestamp_us) > IMU_STALE_TIMEOUT_US
        {
            self.latest.online = false;
        }
        self.latest
    }
}

#[cfg(target_os = "linux")]
mod linux_reader {
    use std::ffi::CString;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use anyhow::{bail, Result};
    use doso_core::ImuState;

    use super::{parse_imu_baud, ImuSerialState, IMU_STALE_TIMEOUT_US};

    struct SharedImuSlot {
        state: ImuSerialState,
        last_rx_instant: Option<Instant>,
    }

    /// Non-blocking Linux `termios` background reader for Hipnuc Hi91 IMU (`/dev/imu` / `/dev/ttyUSB*`).
    pub struct SerialImuReader {
        shared: Arc<Mutex<SharedImuSlot>>,
        stop_flag: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    impl SerialImuReader {
        pub fn open(port_path: &str, baud: u32) -> Result<Self> {
            let _ = parse_imu_baud(Some(&baud.to_string()))?;
            let initial_fd = Self::open_and_configure_tty(port_path, baud)?;
            let boot = Instant::now();
            let shared = Arc::new(Mutex::new(SharedImuSlot {
                state: {
                    let mut st = ImuSerialState::new();
                    st.mark_connected();
                    st
                },
                last_rx_instant: None,
            }));
            let stop_flag = Arc::new(AtomicBool::new(false));
            let worker_shared = Arc::clone(&shared);
            let worker_stop = Arc::clone(&stop_flag);
            let port_owned = port_path.to_string();

            let handle = thread::Builder::new()
                .name("doso-imu-rx".into())
                .spawn(move || {
                    Self::reader_loop(initial_fd, &port_owned, baud, boot, worker_shared, worker_stop);
                })?;

            Ok(Self {
                shared,
                stop_flag,
                handle: Some(handle),
            })
        }

        /// Reads latest `ImuState` translated into the caller's `now_us` clock domain so there is
        /// zero clock skew regardless of when the caller's monotonic epoch started.
        pub fn sample(&self, now_us: u64) -> ImuState {
            let guard = self.shared.lock().unwrap();
            let Some(rx_at) = guard.last_rx_instant else {
                return ImuState {
                    online: false,
                    ..guard.state.latest
                };
            };
            let age_us = Instant::now().saturating_duration_since(rx_at).as_micros() as u64;
            let mut out = guard.state.latest;
            out.timestamp_us = now_us.saturating_sub(age_us);
            if !guard.state.is_connected() || age_us > IMU_STALE_TIMEOUT_US {
                out.online = false;
            }
            out
        }

        fn open_and_configure_tty(port_path: &str, baud: u32) -> Result<i32> {
            let speed = match baud {
                115_200 => libc::B115200,
                460_800 => libc::B460800,
                921_600 => libc::B921600,
                other => bail!("Unsupported IMU baud rate {other}"),
            };
            let c_path = CString::new(port_path)
                .map_err(|_| anyhow::anyhow!("Invalid serial port path '{port_path}'"))?;
            unsafe {
                let fd = libc::open(
                    c_path.as_ptr(),
                    libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK,
                );
                if fd < 0 {
                    bail!(
                        "open({port_path}) failed: {}",
                        std::io::Error::last_os_error()
                    );
                }
                let mut tty: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(fd, &mut tty) != 0 {
                    let err = std::io::Error::last_os_error();
                    libc::close(fd);
                    bail!("tcgetattr({port_path}) failed: {err}");
                }
                libc::cfmakeraw(&mut tty);
                if libc::cfsetispeed(&mut tty, speed) != 0 || libc::cfsetospeed(&mut tty, speed) != 0
                {
                    let err = std::io::Error::last_os_error();
                    libc::close(fd);
                    bail!("cfsetspeed({port_path}, {baud}) failed: {err}");
                }
                tty.c_cflag |= libc::CLOCAL | libc::CREAD | libc::CS8;
                tty.c_cflag &= !(libc::CSTOPB | libc::CRTSCTS);
                tty.c_cc[libc::VMIN] = 0;
                tty.c_cc[libc::VTIME] = 0;
                libc::tcflush(fd, libc::TCIFLUSH);
                if libc::tcsetattr(fd, libc::TCSANOW, &tty) != 0 {
                    let err = std::io::Error::last_os_error();
                    libc::close(fd);
                    bail!("tcsetattr({port_path}) failed: {err}");
                }
                Ok(fd)
            }
        }

        fn reader_loop(
            mut fd: i32,
            port_path: &str,
            baud: u32,
            boot: Instant,
            shared: Arc<Mutex<SharedImuSlot>>,
            stop: Arc<AtomicBool>,
        ) {
            let mut buf = [0u8; 1024];
            while !stop.load(Ordering::Relaxed) {
                if fd < 0 {
                    thread::sleep(Duration::from_millis(200));
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Ok(new_fd) = Self::open_and_configure_tty(port_path, baud) {
                        fd = new_fd;
                        if let Ok(mut guard) = shared.lock() {
                            guard.state.mark_connected();
                        }
                    }
                    continue;
                }

                let mut pfd = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let pr = unsafe { libc::poll(&mut pfd, 1, 5) };
                if pr < 0 {
                    let err = std::io::Error::last_os_error();
                    if err.raw_os_error() == Some(libc::EINTR) {
                        continue;
                    }
                    Self::disconnect_fd(&mut fd, &shared);
                    continue;
                }
                if pr == 0 {
                    continue;
                }
                if (pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL)) != 0 {
                    Self::disconnect_fd(&mut fd, &shared);
                    continue;
                }

                loop {
                    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
                    if n > 0 {
                        let now_inst = Instant::now();
                        let rel_us = now_inst.saturating_duration_since(boot).as_micros() as u64;
                        if let Ok(mut guard) = shared.lock() {
                            let decoded = guard.state.push_chunk(&buf[..n as usize], rel_us);
                            if decoded > 0 {
                                guard.last_rx_instant = Some(now_inst);
                            }
                        }
                    } else if n == 0 {
                        Self::disconnect_fd(&mut fd, &shared);
                        break;
                    } else {
                        let errno = std::io::Error::last_os_error().raw_os_error();
                        if errno == Some(libc::EAGAIN)
                            || errno == Some(libc::EWOULDBLOCK)
                            || errno == Some(libc::EINTR)
                        {
                            break;
                        }
                        Self::disconnect_fd(&mut fd, &shared);
                        break;
                    }
                }
            }
            if fd >= 0 {
                unsafe {
                    libc::close(fd);
                }
            }
        }

        fn disconnect_fd(fd: &mut i32, shared: &Arc<Mutex<SharedImuSlot>>) {
            if *fd >= 0 {
                unsafe {
                    libc::close(*fd);
                }
                *fd = -1;
            }
            if let Ok(mut guard) = shared.lock() {
                guard.state.mark_disconnected();
            }
        }
    }

    impl Drop for SerialImuReader {
        fn drop(&mut self) {
            self.stop_flag.store(true, Ordering::Relaxed);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux_reader::SerialImuReader;
