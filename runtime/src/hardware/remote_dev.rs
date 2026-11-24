//! Hardware-only Linux `/dev/yunzhuo` serial and `/dev/input/js*` joystick device reader.
//! All portable mode-gating and stick-mapping logic lives in `crate::arbiter::yunzhuo_gate`.

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod linux_reader {
    use std::ffi::CString;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use crate::arbiter::yunzhuo_gate::{
        JoystickPadMapper, LinuxJsEvent, YunZhuoControlMode, YunZhuoModeStateMachine, YunZhuoState,
        YUNZHUO_WATCHDOG_TIMEOUT_US,
    };
    use crate::hardware::yunzhuo::{
        decode_standard_sbus_frame, SubsStreamDecoder, STANDARD_SBUS_FRAME_SIZE, SUBS_SYNC,
    };
    use crate::types::RemotePadState;

    struct SharedRemoteSlot {
        boot: Instant,
        yunzhuo_machine: YunZhuoModeStateMachine,
        yunzhuo_pad: RemotePadState,
        yunzhuo_last_rx: Option<Instant>,
        pending_l1: bool,
        pending_l2: bool,
        pending_r1: bool,
        pending_r2: bool,
        js_mapper: JoystickPadMapper,
    }

    /// Combined Linux background reader for YunZhuo SBUS/SUBS serial (`DOSO_REMOTE_PORT`, e.g. `/dev/yunzhuo`)
    /// and Linux `/dev/input/js*` joystick with automatic `/dev/input/js0` hotplug discovery.
    pub struct LinuxRemoteInputReader {
        shared: Arc<Mutex<SharedRemoteSlot>>,
        stop_flag: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    impl LinuxRemoteInputReader {
        pub fn spawn(serial_port: Option<String>, preferred_js_dev: String) -> Self {
            let boot = Instant::now();
            let shared = Arc::new(Mutex::new(SharedRemoteSlot {
                boot,
                yunzhuo_machine: YunZhuoModeStateMachine::new(),
                yunzhuo_pad: RemotePadState::default(),
                yunzhuo_last_rx: None,
                pending_l1: false,
                pending_l2: false,
                pending_r1: false,
                pending_r2: false,
                js_mapper: JoystickPadMapper::new(),
            }));
            let stop_flag = Arc::new(AtomicBool::new(false));
            let worker_shared = Arc::clone(&shared);
            let worker_stop = Arc::clone(&stop_flag);

            let handle = thread::Builder::new()
                .name("doso-remote-rx".into())
                .spawn(move || {
                    Self::reader_loop(serial_port, preferred_js_dev, worker_shared, worker_stop);
                })
                .ok();

            Self {
                shared,
                stop_flag,
                handle,
            }
        }

        pub fn sample(&self, now_us: u64) -> RemotePadState {
            let mut guard = self.shared.lock().unwrap();
            let now_inst = Instant::now();
            let rel_us = now_inst.saturating_duration_since(guard.boot).as_micros() as u64;

            if let Some(last_rx) = guard.yunzhuo_last_rx {
                let age_us = now_inst.saturating_duration_since(last_rx).as_micros() as u64;
                if age_us <= YUNZHUO_WATCHDOG_TIMEOUT_US && guard.yunzhuo_pad.active {
                    let scale = guard.yunzhuo_machine.scale_at(rel_us).clamp(0.0, 1.0);
                    let mut out = guard.yunzhuo_pad;
                    if guard.yunzhuo_machine.mode() != YunZhuoControlMode::Dog && scale == 0.0 {
                        out.lx = 0.0;
                        out.ly = 0.0;
                        out.rx = 0.0;
                        out.ry = 0.0;
                    }
                    out.l1 = std::mem::replace(&mut guard.pending_l1, false);
                    out.l2 = std::mem::replace(&mut guard.pending_l2, false);
                    out.r1 = std::mem::replace(&mut guard.pending_r1, false);
                    out.r2 = std::mem::replace(&mut guard.pending_r2, false);
                    out.timestamp_us = now_us.saturating_sub(age_us);
                    return out;
                }
                guard.yunzhuo_machine.reset_for_input_loss();
                guard.yunzhuo_pad = RemotePadState::default();
                guard.yunzhuo_last_rx = None;
                guard.pending_l1 = false;
                guard.pending_l2 = false;
                guard.pending_r1 = false;
                guard.pending_r2 = false;
            }

            guard.js_mapper.sample(now_us)
        }

        fn open_yunzhuo_serial(port_path: &str) -> i32 {
            let Ok(c_path) = CString::new(port_path) else {
                return -1;
            };
            unsafe {
                let fd = libc::open(
                    c_path.as_ptr(),
                    libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK,
                );
                if fd < 0 {
                    return -1;
                }
                let mut tty: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(fd, &mut tty) != 0 {
                    libc::close(fd);
                    return -1;
                }
                libc::cfmakeraw(&mut tty);
                if libc::cfsetispeed(&mut tty, libc::B115200) != 0
                    || libc::cfsetospeed(&mut tty, libc::B115200) != 0
                {
                    libc::close(fd);
                    return -1;
                }
                tty.c_cflag |= libc::CLOCAL | libc::CREAD | libc::CS8;
                tty.c_cflag &= !(libc::CSTOPB | libc::PARENB | libc::CRTSCTS);
                tty.c_cc[libc::VMIN] = 1;
                tty.c_cc[libc::VTIME] = 0;
                libc::tcflush(fd, libc::TCIFLUSH);
                if libc::tcsetattr(fd, libc::TCSANOW, &tty) != 0 {
                    libc::close(fd);
                    return -1;
                }
                // Assert DTR matching Dart SerialPortController(dtr: true)
                let dtr_bits: libc::c_int = libc::TIOCM_DTR;
                let _ = libc::ioctl(fd, libc::TIOCMBIS as _, &dtr_bits);
                fd
            }
        }

        fn try_open_joystick(preferred: &str) -> i32 {
            let mut candidates = vec![preferred.to_string()];
            if preferred != "/dev/input/js0" {
                candidates.push("/dev/input/js0".to_string());
            }
            if let Ok(entries) = std::fs::read_dir("/dev/input") {
                let mut extra: Vec<String> = entries
                    .flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if name.starts_with("js") {
                            Some(e.path().to_string_lossy().into_owned())
                        } else {
                            None
                        }
                    })
                    .collect();
                extra.sort();
                for p in extra {
                    if !candidates.contains(&p) {
                        candidates.push(p);
                    }
                }
            }
            for path in candidates {
                if !Path::new(&path).exists() {
                    continue;
                }
                let Ok(c_path) = CString::new(path) else {
                    continue;
                };
                let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
                if fd >= 0 {
                    return fd;
                }
            }
            -1
        }

        fn reader_loop(
            serial_port: Option<String>,
            preferred_js_dev: String,
            shared: Arc<Mutex<SharedRemoteSlot>>,
            stop: Arc<AtomicBool>,
        ) {
            let mut serial_fd: i32 = -1;
            let mut js_fd: i32 = -1;
            let mut subs_decoder = SubsStreamDecoder::default();
            let mut sbus_fallback_buf: Vec<u8> = Vec::with_capacity(128);
            let mut last_hotplug_scan = Instant::now() - Duration::from_secs(1);

            let mut serial_buf = [0u8; 512];
            let mut js_buf = [0u8; LinuxJsEvent::SIZE * 32];

            while !stop.load(Ordering::Relaxed) {
                if last_hotplug_scan.elapsed() >= Duration::from_millis(500) {
                    last_hotplug_scan = Instant::now();
                    if serial_fd < 0 {
                        if let Some(ref port) = serial_port {
                            if Path::new(port).exists() {
                                serial_fd = Self::open_yunzhuo_serial(port);
                                subs_decoder = SubsStreamDecoder::default();
                                sbus_fallback_buf.clear();
                            }
                        }
                    }
                    if js_fd < 0 {
                        js_fd = Self::try_open_joystick(&preferred_js_dev);
                        if js_fd >= 0 {
                            if let Ok(mut guard) = shared.lock() {
                                let rel_us = guard.boot.elapsed().as_micros() as u64;
                                guard.js_mapper.mark_connected(rel_us);
                            }
                        }
                    }
                }

                if serial_fd < 0 && js_fd < 0 {
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }

                let mut pfds = [
                    libc::pollfd {
                        fd: serial_fd,
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: js_fd,
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                let _ = unsafe { libc::poll(pfds.as_mut_ptr(), 2, 5) };

                if serial_fd >= 0 {
                    if (pfds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL)) != 0 {
                        unsafe { libc::close(serial_fd) };
                        serial_fd = -1;
                    } else if (pfds[0].revents & libc::POLLIN) != 0 {
                        loop {
                            let n = unsafe {
                                libc::read(
                                    serial_fd,
                                    serial_buf.as_mut_ptr().cast(),
                                    serial_buf.len(),
                                )
                            };
                            if n > 0 {
                                let chunk = &serial_buf[..n as usize];
                                let now_inst = Instant::now();
                                let mut decoded_any = false;
                                subs_decoder.push_and_decode(chunk, |channels, flags| {
                                    decoded_any = true;
                                    Self::apply_yunzhuo_channels(
                                        &shared, now_inst, &channels, flags,
                                    );
                                });
                                if !decoded_any {
                                    sbus_fallback_buf.extend_from_slice(chunk);
                                    while sbus_fallback_buf.len() >= STANDARD_SBUS_FRAME_SIZE {
                                        if sbus_fallback_buf[0] != SUBS_SYNC {
                                            sbus_fallback_buf.remove(0);
                                            continue;
                                        }
                                        let mut raw25 = [0u8; STANDARD_SBUS_FRAME_SIZE];
                                        raw25.copy_from_slice(
                                            &sbus_fallback_buf[..STANDARD_SBUS_FRAME_SIZE],
                                        );
                                        if let Some((channels, flags)) =
                                            decode_standard_sbus_frame(&raw25)
                                        {
                                            sbus_fallback_buf.drain(..STANDARD_SBUS_FRAME_SIZE);
                                            Self::apply_yunzhuo_channels(
                                                &shared, now_inst, &channels, flags,
                                            );
                                        } else {
                                            sbus_fallback_buf.remove(0);
                                        }
                                    }
                                    if sbus_fallback_buf.len() > 256 {
                                        sbus_fallback_buf.clear();
                                    }
                                } else {
                                    sbus_fallback_buf.clear();
                                }
                            } else if n == 0 {
                                unsafe { libc::close(serial_fd) };
                                serial_fd = -1;
                                break;
                            } else {
                                let errno = std::io::Error::last_os_error().raw_os_error();
                                if errno == Some(libc::EAGAIN)
                                    || errno == Some(libc::EWOULDBLOCK)
                                    || errno == Some(libc::EINTR)
                                {
                                    break;
                                }
                                unsafe { libc::close(serial_fd) };
                                serial_fd = -1;
                                break;
                            }
                        }
                    }
                }

                if js_fd >= 0 {
                    if (pfds[1].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL)) != 0 {
                        Self::disconnect_js(&mut js_fd, &shared);
                    } else if (pfds[1].revents & libc::POLLIN) != 0 {
                        loop {
                            let n = unsafe {
                                libc::read(js_fd, js_buf.as_mut_ptr().cast(), js_buf.len())
                            };
                            if n >= LinuxJsEvent::SIZE as isize {
                                let count = (n as usize) / LinuxJsEvent::SIZE;
                                if let Ok(mut guard) = shared.lock() {
                                    let rel_us = guard.boot.elapsed().as_micros() as u64;
                                    for i in 0..count {
                                        let off = i * LinuxJsEvent::SIZE;
                                        let mut raw8 = [0u8; 8];
                                        raw8.copy_from_slice(&js_buf[off..off + 8]);
                                        guard.js_mapper.apply_event(
                                            LinuxJsEvent::from_le_bytes(&raw8),
                                            rel_us,
                                        );
                                    }
                                }
                            } else if n < 0 {
                                let errno = std::io::Error::last_os_error().raw_os_error();
                                if errno == Some(libc::EAGAIN)
                                    || errno == Some(libc::EWOULDBLOCK)
                                    || errno == Some(libc::EINTR)
                                {
                                    break;
                                }
                                Self::disconnect_js(&mut js_fd, &shared);
                                break;
                            } else {
                                break;
                            }
                        }
                    }
                }
            }

            if serial_fd >= 0 {
                unsafe { libc::close(serial_fd) };
            }
            if js_fd >= 0 {
                unsafe { libc::close(js_fd) };
            }
        }

        fn apply_yunzhuo_channels(
            shared: &Arc<Mutex<SharedRemoteSlot>>,
            now_inst: Instant,
            channels: &[u16; 16],
            flags: u8,
        ) {
            if let Ok(mut guard) = shared.lock() {
                let rel_us = now_inst.saturating_duration_since(guard.boot).as_micros() as u64;
                let yz = YunZhuoState::from_channels(channels, flags);
                let snap = guard.yunzhuo_machine.update(&yz, rel_us);
                guard.yunzhuo_pad = snap.pad;
                if yz.healthy {
                    guard.yunzhuo_last_rx = Some(now_inst);
                    if snap.standup_triggered {
                        guard.pending_l1 = true;
                    }
                    if snap.sitdown_triggered {
                        guard.pending_l2 = true;
                    }
                    if snap.pad.r1 {
                        guard.pending_r1 = true;
                    }
                    if snap.pad.r2 {
                        guard.pending_r2 = true;
                    }
                } else {
                    guard.yunzhuo_last_rx = None;
                }
            }
        }

        fn disconnect_js(js_fd: &mut i32, shared: &Arc<Mutex<SharedRemoteSlot>>) {
            if *js_fd >= 0 {
                unsafe { libc::close(*js_fd) };
                *js_fd = -1;
            }
            if let Ok(mut guard) = shared.lock() {
                guard.js_mapper.mark_disconnected();
            }
        }
    }

    impl Drop for LinuxRemoteInputReader {
        fn drop(&mut self) {
            self.stop_flag.store(true, Ordering::Relaxed);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux_reader::LinuxRemoteInputReader;
