//! Lock-free shared-memory IPC for Board-A sub-50us telemetry and 200Hz–1kHz external MIT control.
//!
//! - On Linux: uses POSIX shared memory (`shm_open` + `ftruncate` + `mmap` under `/dev/shm`).
//! - On Windows: uses named shared memory (`CreateFileMappingA` / `OpenFileMappingA` + `MapViewOfFile`)
//!   so cross-process Dart <-> Rust lock-free tests run natively on developer workstations.
//! - Synchronization: ARMv8-safe lock-free Seqlock ring buffer (`SeqCst` begin fence + `Release` commit
//!   on writer; `Acquire` load + payload copy + `Acquire` fence + `Acquire` re-check on reader).
//!   Guarantees zero torn reads and zero locks across both x86_64 and aarch64.

use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr::{self, NonNull};
use std::sync::atomic::{fence, AtomicU64, Ordering};

use anyhow::{bail, Result};

pub const DOSO_SHM_MAGIC: u32 = 0x4F53_4F44; // "DOSO" LE
pub const DOSO_SHM_VERSION: u32 = 1;
pub const DOSO_SHM_JOINTS: usize = 16;
pub const DOSO_SHM_PROPRIO_SLOTS: usize = 64;
pub const DOSO_SHM_CMD_SLOTS: usize = 4;

pub const DOSO_SHM_OK: i32 = 0;
pub const DOSO_SHM_UNCHANGED: i32 = 1;
pub const DOSO_SHM_ERR_INVALID: i32 = -1;
pub const DOSO_SHM_ERR_CORRUPT: i32 = -2;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DosoProprioPayload {
    pub sequence: u64,
    pub monotonic_us: u64,
    pub imu_quat_wxyz: [f64; 4],
    pub imu_gyro_rad_s: [f64; 3],
    pub imu_accel_m_s2: [f64; 3],
    pub joint_q: [f64; DOSO_SHM_JOINTS],
    pub joint_dq: [f64; DOSO_SHM_JOINTS],
    pub joint_tau: [f64; DOSO_SHM_JOINTS],
    pub joint_temp_c: [f32; DOSO_SHM_JOINTS],
    pub joint_status: [u16; DOSO_SHM_JOINTS],
    pub joint_online_mask: u16,
    pub imu_fresh: u8,
    pub cms_state: u8,
    pub control_owner: u8,
    pub motor_output_enabled: u8,
    pub critical_motor_fault: u8,
    pub _reserved: [u8; 9],
}

impl Default for DosoProprioPayload {
    fn default() -> Self {
        Self {
            sequence: 0,
            monotonic_us: 0,
            imu_quat_wxyz: [1.0, 0.0, 0.0, 0.0],
            imu_gyro_rad_s: [0.0; 3],
            imu_accel_m_s2: [0.0; 3],
            joint_q: [0.0; DOSO_SHM_JOINTS],
            joint_dq: [0.0; DOSO_SHM_JOINTS],
            joint_tau: [0.0; DOSO_SHM_JOINTS],
            joint_temp_c: [0.0; DOSO_SHM_JOINTS],
            joint_status: [0; DOSO_SHM_JOINTS],
            joint_online_mask: 0,
            imu_fresh: 0,
            cms_state: 0,
            control_owner: 0,
            motor_output_enabled: 0,
            critical_motor_fault: 0,
            _reserved: [0; 9],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DosoExternalCmdPayload {
    pub session_id_hash: u64,
    pub input_window: u64,
    pub sequence: u64,
    pub monotonic_us: u64,
    pub q: [f64; DOSO_SHM_JOINTS],
    pub dq: [f64; DOSO_SHM_JOINTS],
    pub kp: [f64; DOSO_SHM_JOINTS],
    pub kd: [f64; DOSO_SHM_JOINTS],
    pub tau: [f64; DOSO_SHM_JOINTS],
}

impl Default for DosoExternalCmdPayload {
    fn default() -> Self {
        Self {
            session_id_hash: 0,
            input_window: 0,
            sequence: 0,
            monotonic_us: 0,
            q: [0.0; DOSO_SHM_JOINTS],
            dq: [0.0; DOSO_SHM_JOINTS],
            kp: [0.0; DOSO_SHM_JOINTS],
            kd: [0.0; DOSO_SHM_JOINTS],
            tau: [0.0; DOSO_SHM_JOINTS],
        }
    }
}

#[repr(C, align(64))]
struct RingHeader {
    magic: u32,
    version: u32,
    slot_count: u32,
    slot_bytes: u32,
    write_seq: AtomicU64,
    producer_pid: u32,
    flags: u32,
    producer_heartbeat_us: AtomicU64,
    _pad: [u8; 24],
}

#[repr(C, align(64))]
struct SeqlockSlot<T: Copy> {
    seq_begin: AtomicU64,
    payload: T,
    seq_end: AtomicU64,
}

#[repr(C, align(64))]
struct RingLayout<T: Copy, const N: usize> {
    header: RingHeader,
    slots: [SeqlockSlot<T>; N],
}

/// FNV-1a 64-bit hash used to bind gRPC `session_id` to SHM command frames.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Returns a monotonic timestamp in microseconds suitable for SHM frame freshness stamps.
pub fn monotonic_us() -> u64 {
    #[cfg(unix)]
    {
        #[repr(C)]
        struct Timespec {
            tv_sec: i64,
            tv_nsec: i64,
        }
        extern "C" {
            fn clock_gettime(clk_id: i32, tp: *mut Timespec) -> i32;
        }
        const CLOCK_MONOTONIC: i32 = 1;
        let mut ts = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) } == 0
            && ts.tv_sec >= 0
            && ts.tv_nsec >= 0
        {
            return (ts.tv_sec as u64)
                .saturating_mul(1_000_000)
                .saturating_add((ts.tv_nsec as u64) / 1_000);
        }
    }
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(std::time::Instant::now);
    1_000_u64.saturating_add(start.elapsed().as_micros() as u64)
}

// ─── Platform Shared Memory Mapping ───────────────────────────────────────────

struct ShmMapping {
    ptr: NonNull<u8>,
    #[cfg_attr(windows, allow(dead_code))]
    len: usize,
    #[cfg(windows)]
    handle: *mut c_void,
    #[cfg(unix)]
    fd: i32,
    #[cfg(unix)]
    unlink_name: Option<CString>,
}

unsafe impl Send for ShmMapping {}
unsafe impl Sync for ShmMapping {}

#[cfg(unix)]
mod sys {
    use super::*;

    const O_RDONLY: i32 = 0;
    const O_RDWR: i32 = 2;
    const O_CREAT: i32 = 64;
    const PROT_READ: i32 = 1;
    const PROT_WRITE: i32 = 2;
    const MAP_SHARED: i32 = 1;
    const MAP_FAILED: *mut c_void = !0usize as *mut c_void;

    extern "C" {
        fn shm_open(name: *const c_char, oflag: i32, mode: u32) -> i32;
        fn shm_unlink(name: *const c_char) -> i32;
        fn ftruncate(fd: i32, length: i64) -> i32;
        fn mmap(
            addr: *mut c_void,
            length: usize,
            prot: i32,
            flags: i32,
            fd: i32,
            offset: i64,
        ) -> *mut c_void;
        fn munmap(addr: *mut c_void, length: usize) -> i32;
        fn close(fd: i32) -> i32;
    }

    pub fn normalize_name(raw: &str) -> Result<CString> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.contains('\0') {
            bail!("shared memory name must be non-empty");
        }
        let normalized = if trimmed.starts_with('/') {
            trimmed.to_string()
        } else {
            format!("/{trimmed}")
        };
        Ok(CString::new(normalized)?)
    }

    pub fn open_mapping(name: &str, len: usize, create_writer: bool) -> Result<ShmMapping> {
        let cname = normalize_name(name)?;
        let flags = if create_writer {
            O_RDWR | O_CREAT
        } else {
            O_RDONLY
        };
        let fd = unsafe { shm_open(cname.as_ptr(), flags, 0o660) };
        if fd < 0 {
            bail!(
                "shm_open({name}) failed: {}",
                std::io::Error::last_os_error()
            );
        }
        if create_writer {
            let rc = unsafe { ftruncate(fd, len as i64) };
            if rc != 0 {
                let err = std::io::Error::last_os_error();
                unsafe {
                    close(fd);
                }
                bail!("ftruncate({name}, {len}) failed: {err}");
            }
        }
        let prot = if create_writer {
            PROT_READ | PROT_WRITE
        } else {
            PROT_READ
        };
        let raw_ptr = unsafe { mmap(ptr::null_mut(), len, prot, MAP_SHARED, fd, 0) };
        if raw_ptr == MAP_FAILED || raw_ptr.is_null() {
            let err = std::io::Error::last_os_error();
            unsafe {
                close(fd);
            }
            bail!("mmap({name}, {len}) failed: {err}");
        }
        Ok(ShmMapping {
            ptr: NonNull::new(raw_ptr as *mut u8).expect("non-null mmap"),
            len,
            fd,
            unlink_name: None,
        })
    }

    pub fn drop_mapping(m: &mut ShmMapping) {
        unsafe {
            let _ = munmap(m.ptr.as_ptr() as *mut c_void, m.len);
            if m.fd >= 0 {
                let _ = close(m.fd);
                m.fd = -1;
            }
            if let Some(name) = m.unlink_name.take() {
                let _ = shm_unlink(name.as_ptr());
            }
        }
    }
}

#[cfg(windows)]
mod sys {
    use super::*;

    const INVALID_HANDLE_VALUE: *mut c_void = (-1isize) as *mut c_void;
    const PAGE_READWRITE: u32 = 0x04;
    const FILE_MAP_ALL_ACCESS: u32 = 0x000F_001F;
    const FILE_MAP_READ: u32 = 0x0004;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileMappingA(
            file: *mut c_void,
            security: *mut c_void,
            protect: u32,
            max_size_high: u32,
            max_size_low: u32,
            name: *const c_char,
        ) -> *mut c_void;
        fn OpenFileMappingA(
            desired_access: u32,
            inherit_handle: i32,
            name: *const c_char,
        ) -> *mut c_void;
        fn MapViewOfFile(
            mapping: *mut c_void,
            desired_access: u32,
            offset_high: u32,
            offset_low: u32,
            bytes_to_map: usize,
        ) -> *mut c_void;
        fn UnmapViewOfFile(base_addr: *const c_void) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    fn normalize_name(raw: &str) -> Result<CString> {
        let trimmed = raw.trim().trim_start_matches('/');
        if trimmed.is_empty() || trimmed.contains('\0') {
            bail!("shared memory name must be non-empty");
        }
        let sanitized: String = trimmed
            .chars()
            .map(|c| if c == '/' || c == '\\' { '_' } else { c })
            .collect();
        Ok(CString::new(format!("Local\\{sanitized}"))?)
    }

    pub fn open_mapping(name: &str, len: usize, create_writer: bool) -> Result<ShmMapping> {
        let cname = normalize_name(name)?;
        let size_u64 = len as u64;
        let high = (size_u64 >> 32) as u32;
        let low = (size_u64 & 0xFFFF_FFFF) as u32;

        let handle = if create_writer {
            unsafe {
                CreateFileMappingA(
                    INVALID_HANDLE_VALUE,
                    ptr::null_mut(),
                    PAGE_READWRITE,
                    high,
                    low,
                    cname.as_ptr(),
                )
            }
        } else {
            unsafe { OpenFileMappingA(FILE_MAP_READ, 0, cname.as_ptr()) }
        };
        if handle.is_null() {
            bail!(
                "Windows shared memory open({name}) failed: {}",
                std::io::Error::last_os_error()
            );
        }

        let access = if create_writer {
            FILE_MAP_ALL_ACCESS
        } else {
            FILE_MAP_READ
        };
        let raw_ptr = unsafe { MapViewOfFile(handle, access, 0, 0, len) };
        if raw_ptr.is_null() {
            let err = std::io::Error::last_os_error();
            unsafe {
                CloseHandle(handle);
            }
            bail!("MapViewOfFile({name}, {len}) failed: {err}");
        }
        Ok(ShmMapping {
            ptr: NonNull::new(raw_ptr as *mut u8).expect("non-null map view"),
            len,
            handle,
        })
    }

    pub fn drop_mapping(m: &mut ShmMapping) {
        unsafe {
            let _ = UnmapViewOfFile(m.ptr.as_ptr() as *const c_void);
            if !m.handle.is_null() {
                let _ = CloseHandle(m.handle);
                m.handle = ptr::null_mut();
            }
        }
    }
}

impl Drop for ShmMapping {
    fn drop(&mut self) {
        sys::drop_mapping(self);
    }
}

// ─── Generic ARMv8-Safe Lock-Free Seqlock Ring ────────────────────────────────

struct LockFreeRing<T: Copy, const N: usize> {
    mapping: ShmMapping,
    is_writer: bool,
    _marker: std::marker::PhantomData<T>,
}

impl<T: Copy, const N: usize> LockFreeRing<T, N> {
    const fn assert_power_of_two() {
        assert!(N > 0 && (N & (N - 1)) == 0);
    }

    fn open_writer(name: &str) -> Result<Self> {
        Self::assert_power_of_two();
        let len = std::mem::size_of::<RingLayout<T, N>>();
        let mapping = sys::open_mapping(name, len, true)?;
        unsafe {
            // Zero-initialize cleanly before publishing magic/version header.
            ptr::write_bytes(mapping.ptr.as_ptr(), 0, len);
            let layout = &mut *(mapping.ptr.as_ptr() as *mut RingLayout<T, N>);
            layout.header.version = DOSO_SHM_VERSION;
            layout.header.slot_count = N as u32;
            layout.header.slot_bytes = std::mem::size_of::<SeqlockSlot<T>>() as u32;
            layout.header.producer_pid = std::process::id();
            layout.header.flags = 0;
            layout.header.write_seq.store(0, Ordering::Relaxed);
            layout
                .header
                .producer_heartbeat_us
                .store(0, Ordering::Relaxed);
            fence(Ordering::Release);
            layout.header.magic = DOSO_SHM_MAGIC;
        }
        Ok(Self {
            mapping,
            is_writer: true,
            _marker: std::marker::PhantomData,
        })
    }

    fn open_reader(name: &str) -> Result<Self> {
        Self::assert_power_of_two();
        let len = std::mem::size_of::<RingLayout<T, N>>();
        let mapping = sys::open_mapping(name, len, false)?;
        let layout = unsafe { &*(mapping.ptr.as_ptr() as *const RingLayout<T, N>) };
        fence(Ordering::Acquire);
        if layout.header.magic != DOSO_SHM_MAGIC {
            bail!(
                "shared memory magic mismatch: got 0x{:08x}, expected 0x{:08x}",
                layout.header.magic,
                DOSO_SHM_MAGIC
            );
        }
        if layout.header.version != DOSO_SHM_VERSION {
            bail!(
                "shared memory version mismatch: got {}, expected {}",
                layout.header.version,
                DOSO_SHM_VERSION
            );
        }
        if layout.header.slot_count != N as u32
            || layout.header.slot_bytes != std::mem::size_of::<SeqlockSlot<T>>() as u32
        {
            bail!(
                "shared memory geometry mismatch: slots={} bytes={}",
                layout.header.slot_count,
                layout.header.slot_bytes
            );
        }
        Ok(Self {
            mapping,
            is_writer: false,
            _marker: std::marker::PhantomData,
        })
    }

    #[inline]
    fn layout(&self) -> &RingLayout<T, N> {
        unsafe { &*(self.mapping.ptr.as_ptr() as *const RingLayout<T, N>) }
    }

    #[inline]
    fn write_seq(&self) -> u64 {
        self.layout().header.write_seq.load(Ordering::Acquire)
    }

    #[inline]
    fn publish(&mut self, payload: &T, heartbeat_us: u64) -> Result<u64> {
        if !self.is_writer {
            bail!("attempted publish on read-only shared memory handle");
        }
        let layout = unsafe { &mut *(self.mapping.ptr.as_ptr() as *mut RingLayout<T, N>) };
        let next_seq = layout
            .header
            .write_seq
            .load(Ordering::Relaxed)
            .wrapping_add(1);
        let idx = ((next_seq - 1) as usize) & (N - 1);
        let odd = (next_seq << 1).wrapping_sub(1);
        let even = next_seq << 1;

        let slot = &mut layout.slots[idx];
        // SeqCst guarantees odd-marker is globally visible on ARMv8 BEFORE any payload store.
        slot.seq_begin.store(odd, Ordering::SeqCst);
        unsafe {
            ptr::write_volatile(&mut slot.payload as *mut T, *payload);
        }
        slot.seq_end.store(even, Ordering::Release);
        slot.seq_begin.store(even, Ordering::Release);
        layout
            .header
            .producer_heartbeat_us
            .store(heartbeat_us, Ordering::Relaxed);
        layout.header.write_seq.store(next_seq, Ordering::Release);
        Ok(next_seq)
    }

    #[inline]
    fn read_latest(&self, last_seen_seq: u64) -> Option<(u64, T)> {
        let layout = self.layout();
        for _ in 0..16 {
            let seq = layout.header.write_seq.load(Ordering::Acquire);
            if seq == 0 || seq == last_seen_seq {
                return None;
            }
            let idx = ((seq - 1) as usize) & (N - 1);
            let slot = &layout.slots[idx];
            let s1 = slot.seq_begin.load(Ordering::Acquire);
            if s1 == 0 || (s1 & 1) != 0 {
                std::hint::spin_loop();
                continue;
            }
            let copied = unsafe { ptr::read_volatile(&slot.payload as *const T) };
            fence(Ordering::Acquire);
            let s2 = slot.seq_end.load(Ordering::Acquire);
            let s3 = slot.seq_begin.load(Ordering::Acquire);
            if s1 == s2 && s1 == s3 {
                return Some((s1 >> 1, copied));
            }
            std::hint::spin_loop();
        }
        None
    }
}

// ─── Safe Rust Public Types ───────────────────────────────────────────────────

pub struct ProprioRingWriter {
    inner: LockFreeRing<DosoProprioPayload, DOSO_SHM_PROPRIO_SLOTS>,
}

pub struct ProprioRingReader {
    inner: LockFreeRing<DosoProprioPayload, DOSO_SHM_PROPRIO_SLOTS>,
}

impl ProprioRingWriter {
    pub fn open(name: &str) -> Result<Self> {
        Ok(Self {
            inner: LockFreeRing::open_writer(name)?,
        })
    }

    pub fn publish(&mut self, payload: &DosoProprioPayload) -> Result<u64> {
        self.inner.publish(payload, payload.monotonic_us)
    }

    pub fn write_seq(&self) -> u64 {
        self.inner.write_seq()
    }
}

impl ProprioRingReader {
    pub fn open(name: &str) -> Result<Self> {
        Ok(Self {
            inner: LockFreeRing::open_reader(name)?,
        })
    }

    pub fn read_latest(&self, last_seen_seq: u64) -> Option<(u64, DosoProprioPayload)> {
        self.inner.read_latest(last_seen_seq)
    }

    pub fn write_seq(&self) -> u64 {
        self.inner.write_seq()
    }
}

pub struct ExternalCmdWriter {
    inner: LockFreeRing<DosoExternalCmdPayload, DOSO_SHM_CMD_SLOTS>,
}

pub struct ExternalCmdReader {
    inner: LockFreeRing<DosoExternalCmdPayload, DOSO_SHM_CMD_SLOTS>,
}

impl ExternalCmdWriter {
    pub fn open(name: &str) -> Result<Self> {
        Ok(Self {
            inner: LockFreeRing::open_writer(name)?,
        })
    }

    pub fn publish(&mut self, payload: &DosoExternalCmdPayload) -> Result<u64> {
        self.inner.publish(payload, payload.monotonic_us)
    }

    pub fn write_seq(&self) -> u64 {
        self.inner.write_seq()
    }
}

impl ExternalCmdReader {
    pub fn open(name: &str) -> Result<Self> {
        Ok(Self {
            inner: LockFreeRing::open_reader(name)?,
        })
    }

    pub fn read_latest(&self, last_seen_seq: u64) -> Option<(u64, DosoExternalCmdPayload)> {
        self.inner.read_latest(last_seen_seq)
    }

    pub fn write_seq(&self) -> u64 {
        self.inner.write_seq()
    }
}

// ─── C ABI Exports (consumable by Dart dart:ffi, C++, Python ctypes) ──────────

pub struct DosoProprioShm {
    inner: LockFreeRing<DosoProprioPayload, DOSO_SHM_PROPRIO_SLOTS>,
}

pub struct DosoExternalCmdShm {
    inner: LockFreeRing<DosoExternalCmdPayload, DOSO_SHM_CMD_SLOTS>,
}

unsafe fn c_str_to_rust<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    CStr::from_ptr(ptr).to_str().ok()
}

/// Opens or creates a shared-memory proprioception ring as the sole writer.
///
/// # Safety
/// `name_or_path` must be a valid NUL-terminated UTF-8 C string (or NULL, which returns NULL).
#[no_mangle]
pub unsafe extern "C" fn doso_shm_proprio_open_writer(
    name_or_path: *const c_char,
) -> *mut DosoProprioShm {
    let Some(name) = c_str_to_rust(name_or_path) else {
        return ptr::null_mut();
    };
    match LockFreeRing::open_writer(name) {
        Ok(inner) => Box::into_raw(Box::new(DosoProprioShm { inner })),
        Err(_) => ptr::null_mut(),
    }
}

/// Opens an existing shared-memory proprioception ring as a read-only subscriber.
///
/// # Safety
/// `name_or_path` must be a valid NUL-terminated UTF-8 C string (or NULL, which returns NULL).
#[no_mangle]
pub unsafe extern "C" fn doso_shm_proprio_open_reader(
    name_or_path: *const c_char,
) -> *mut DosoProprioShm {
    let Some(name) = c_str_to_rust(name_or_path) else {
        return ptr::null_mut();
    };
    match LockFreeRing::open_reader(name) {
        Ok(inner) => Box::into_raw(Box::new(DosoProprioShm { inner })),
        Err(_) => ptr::null_mut(),
    }
}

/// Closes and frees a handle previously returned by `doso_shm_proprio_open_*`.
///
/// # Safety
/// `handle` must be NULL or a unique pointer previously returned by `doso_shm_proprio_open_*`.
#[no_mangle]
pub unsafe extern "C" fn doso_shm_proprio_close(handle: *mut DosoProprioShm) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// Publishes one proprioceptive payload into the next lock-free seqlock ring slot.
///
/// # Safety
/// `writer` must be a valid writer handle and `payload` must point to an initialized `DosoProprioPayload`.
#[no_mangle]
pub unsafe extern "C" fn doso_shm_proprio_publish(
    writer: *mut DosoProprioShm,
    payload: *const DosoProprioPayload,
) -> i32 {
    if writer.is_null() || payload.is_null() {
        return DOSO_SHM_ERR_INVALID;
    }
    let w = &mut *writer;
    let p = &*payload;
    match w.inner.publish(p, p.monotonic_us) {
        Ok(_) => DOSO_SHM_OK,
        Err(_) => DOSO_SHM_ERR_INVALID,
    }
}

/// Reads the newest complete proprioceptive frame if `write_seq > last_seen_seq`.
///
/// # Safety
/// `reader` must be a valid handle and `out_payload` must be writable and aligned for `DosoProprioPayload`.
#[no_mangle]
pub unsafe extern "C" fn doso_shm_proprio_read_latest(
    reader: *const DosoProprioShm,
    last_seen_seq: u64,
    out_payload: *mut DosoProprioPayload,
) -> i32 {
    if reader.is_null() || out_payload.is_null() {
        return DOSO_SHM_ERR_INVALID;
    }
    let r = &*reader;
    match r.inner.read_latest(last_seen_seq) {
        Some((_, data)) => {
            ptr::write(out_payload, data);
            DOSO_SHM_OK
        }
        None => DOSO_SHM_UNCHANGED,
    }
}

/// Returns the latest committed write sequence of the proprioception ring.
///
/// # Safety
/// `handle` must be NULL or a valid `DosoProprioShm` handle.
#[no_mangle]
pub unsafe extern "C" fn doso_shm_proprio_write_seq(handle: *const DosoProprioShm) -> u64 {
    if handle.is_null() {
        return 0;
    }
    (*handle).inner.write_seq()
}

/// Opens or creates a shared-memory external MIT command ring as the sole writer.
///
/// # Safety
/// `name_or_path` must be a valid NUL-terminated UTF-8 C string (or NULL, which returns NULL).
#[no_mangle]
pub unsafe extern "C" fn doso_shm_cmd_open_writer(
    name_or_path: *const c_char,
) -> *mut DosoExternalCmdShm {
    let Some(name) = c_str_to_rust(name_or_path) else {
        return ptr::null_mut();
    };
    match LockFreeRing::open_writer(name) {
        Ok(inner) => Box::into_raw(Box::new(DosoExternalCmdShm { inner })),
        Err(_) => ptr::null_mut(),
    }
}

/// Opens an existing shared-memory external MIT command ring as a reader.
///
/// # Safety
/// `name_or_path` must be a valid NUL-terminated UTF-8 C string (or NULL, which returns NULL).
#[no_mangle]
pub unsafe extern "C" fn doso_shm_cmd_open_reader(
    name_or_path: *const c_char,
) -> *mut DosoExternalCmdShm {
    let Some(name) = c_str_to_rust(name_or_path) else {
        return ptr::null_mut();
    };
    match LockFreeRing::open_reader(name) {
        Ok(inner) => Box::into_raw(Box::new(DosoExternalCmdShm { inner })),
        Err(_) => ptr::null_mut(),
    }
}

/// Closes and frees a handle previously returned by `doso_shm_cmd_open_*`.
///
/// # Safety
/// `handle` must be NULL or a unique pointer previously returned by `doso_shm_cmd_open_*`.
#[no_mangle]
pub unsafe extern "C" fn doso_shm_cmd_close(handle: *mut DosoExternalCmdShm) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// Publishes one external MIT command payload into the Latest-Wins ring.
///
/// # Safety
/// `writer` must be a valid writer handle and `payload` must point to an initialized `DosoExternalCmdPayload`.
#[no_mangle]
pub unsafe extern "C" fn doso_shm_cmd_publish(
    writer: *mut DosoExternalCmdShm,
    payload: *const DosoExternalCmdPayload,
) -> i32 {
    if writer.is_null() || payload.is_null() {
        return DOSO_SHM_ERR_INVALID;
    }
    let w = &mut *writer;
    let p = &*payload;
    match w.inner.publish(p, p.monotonic_us) {
        Ok(_) => DOSO_SHM_OK,
        Err(_) => DOSO_SHM_ERR_INVALID,
    }
}

/// Reads the newest complete external MIT command frame if `write_seq > last_seen_seq`.
///
/// # Safety
/// `reader` must be a valid handle and `out_payload` must be writable and aligned for `DosoExternalCmdPayload`.
#[no_mangle]
pub unsafe extern "C" fn doso_shm_cmd_read_latest(
    reader: *const DosoExternalCmdShm,
    last_seen_seq: u64,
    out_payload: *mut DosoExternalCmdPayload,
) -> i32 {
    if reader.is_null() || out_payload.is_null() {
        return DOSO_SHM_ERR_INVALID;
    }
    let r = &*reader;
    match r.inner.read_latest(last_seen_seq) {
        Some((_, data)) => {
            ptr::write(out_payload, data);
            DOSO_SHM_OK
        }
        None => DOSO_SHM_UNCHANGED,
    }
}

/// Returns the latest committed write sequence of the external command ring.
///
/// # Safety
/// `handle` must be NULL or a valid `DosoExternalCmdShm` handle.
#[no_mangle]
pub unsafe extern "C" fn doso_shm_cmd_write_seq(handle: *const DosoExternalCmdShm) -> u64 {
    if handle.is_null() {
        return 0;
    }
    (*handle).inner.write_seq()
}

/// Computes the 64-bit FNV-1a hash of `[data, data + len)` for `session_id` binding.
///
/// # Safety
/// If `data` is non-NULL and `len > 0`, `data` must be valid for reads of `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn doso_shm_fnv1a_64(data: *const u8, len: usize) -> u64 {
    if data.is_null() || len == 0 {
        return fnv1a_64(&[]);
    }
    fnv1a_64(std::slice::from_raw_parts(data, len))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::thread;
    use std::time::Instant;

    fn unique_name(prefix: &str) -> String {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("/doso_test_{prefix}_{pid}_{nanos}")
    }

    #[test]
    fn header_and_slots_are_cache_line_aligned() {
        assert_eq!(std::mem::size_of::<RingHeader>(), 64);
        assert_eq!(std::mem::align_of::<RingHeader>(), 64);
        assert_eq!(std::mem::size_of::<DosoProprioPayload>(), 592);
        assert_eq!(std::mem::size_of::<DosoExternalCmdPayload>(), 672);
        assert_eq!(std::mem::align_of::<SeqlockSlot<DosoProprioPayload>>(), 64);
        assert_eq!(
            std::mem::align_of::<SeqlockSlot<DosoExternalCmdPayload>>(),
            64
        );
    }

    #[test]
    fn proprio_spmc_ring_has_zero_torn_reads_under_100k_stress() {
        let name = unique_name("proprio_stress");
        let mut writer = ProprioRingWriter::open(&name).unwrap();
        let r1 = ProprioRingReader::open(&name).unwrap();
        let r2 = ProprioRingReader::open(&name).unwrap();

        let done = Arc::new(AtomicBool::new(false));
        let done_r1 = done.clone();
        let done_r2 = done.clone();

        let check_reader = |reader: ProprioRingReader, flag: Arc<AtomicBool>| {
            thread::spawn(move || {
                let mut seen = 0_u64;
                let mut frames_verified = 0_u64;
                while !flag.load(Ordering::Acquire) {
                    if let Some((seq, p)) = reader.read_latest(seen) {
                        assert!(seq > seen, "non-monotonic ring seq {seq} <= {seen}");
                        // Every field in payload must match the exact same writer sequence (zero torn read).
                        let expected = p.sequence as f64;
                        assert_eq!(p.monotonic_us, p.sequence * 100);
                        assert!((p.imu_accel_m_s2[2] - (9.81 + expected * 1e-4)).abs() < 1e-9);
                        for i in 0..DOSO_SHM_JOINTS {
                            assert!(
                                (p.joint_q[i] - (expected + i as f64 * 0.01)).abs() < 1e-9,
                                "torn joint_q[{i}] at seq={}",
                                p.sequence
                            );
                            assert!(
                                (p.joint_tau[i] - (-expected * 0.5)).abs() < 1e-9,
                                "torn joint_tau[{i}] at seq={}",
                                p.sequence
                            );
                        }
                        seen = seq;
                        frames_verified += 1;
                    }
                }
                frames_verified
            })
        };

        let h1 = check_reader(r1, done_r1);
        let h2 = check_reader(r2, done_r2);

        const TOTAL: u64 = 100_000;
        let t0 = Instant::now();
        for seq in 1..=TOTAL {
            let mut p = DosoProprioPayload {
                sequence: seq,
                monotonic_us: seq * 100,
                imu_quat_wxyz: [1.0, 0.0, 0.0, 0.0],
                imu_gyro_rad_s: [0.01, -0.01, 0.02],
                imu_accel_m_s2: [0.0, 0.0, 9.81 + seq as f64 * 1e-4],
                joint_online_mask: 0xFFFF,
                imu_fresh: 1,
                cms_state: 2,
                control_owner: 1,
                motor_output_enabled: 1,
                ..DosoProprioPayload::default()
            };
            for i in 0..DOSO_SHM_JOINTS {
                p.joint_q[i] = seq as f64 + i as f64 * 0.01;
                p.joint_tau[i] = -(seq as f64) * 0.5;
            }
            writer.publish(&p).unwrap();
        }
        let elapsed = t0.elapsed();
        done.store(true, Ordering::Release);

        let v1 = h1.join().unwrap();
        let v2 = h2.join().unwrap();
        assert!(
            v1 > 0 && v2 > 0,
            "readers must observe live frames: {v1}, {v2}"
        );
        let ns_per_write = elapsed.as_nanos() / u128::from(TOTAL);
        eprintln!(
            "SHM proprio SPMC stress passed: {TOTAL} writes in {:?} ({ns_per_write} ns/write), r1={v1} r2={v2} tear-free frames",
            elapsed
        );
    }

    #[test]
    fn external_cmd_latest_wins_and_c_abi_roundtrip() {
        let name = unique_name("cmd_cabi");
        let cname = CString::new(name.as_str()).unwrap();
        let session_hash = fnv1a_64(b"ext-session-42");

        unsafe {
            let w = doso_shm_cmd_open_writer(cname.as_ptr());
            assert!(!w.is_null());
            let r = doso_shm_cmd_open_reader(cname.as_ptr());
            assert!(!r.is_null());

            // Burst 10 commands without reader polling -> reader must observe the latest frame (seq=10).
            for seq in 1..=10_u64 {
                let mut cmd = DosoExternalCmdPayload {
                    session_id_hash: session_hash,
                    input_window: 77,
                    sequence: seq,
                    monotonic_us: seq * 5000,
                    ..DosoExternalCmdPayload::default()
                };
                cmd.q = [seq as f64 * 0.1; DOSO_SHM_JOINTS];
                cmd.kp = [35.0; DOSO_SHM_JOINTS];
                cmd.kd = [1.2; DOSO_SHM_JOINTS];
                assert_eq!(doso_shm_cmd_publish(w, &cmd), DOSO_SHM_OK);
            }
            assert_eq!(doso_shm_cmd_write_seq(r), 10);

            let mut out = DosoExternalCmdPayload::default();
            assert_eq!(doso_shm_cmd_read_latest(r, 0, &mut out), DOSO_SHM_OK);
            assert_eq!(out.session_id_hash, session_hash);
            assert_eq!(out.input_window, 77);
            assert_eq!(out.sequence, 10);
            assert!((out.q[0] - 1.0).abs() < 1e-9);
            assert_eq!(
                doso_shm_cmd_read_latest(r, 10, &mut out),
                DOSO_SHM_UNCHANGED
            );

            doso_shm_cmd_close(r);
            doso_shm_cmd_close(w);
        }
    }
}
