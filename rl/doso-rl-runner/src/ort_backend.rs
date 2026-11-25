use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;

use anyhow::{anyhow, bail, Context, Result};
use libloading::Library;

use crate::config::DeployConfig;

pub trait PolicyBackend: Send + Sync {
    fn input_dim(&self) -> usize;
    fn output_dim(&self) -> usize;
    fn infer(&mut self, obs: &[f32]) -> Result<Vec<f32>>;

    /// Zero-allocation inference writing outputs directly into `out` (length `output_dim()`).
    fn infer_into(&mut self, obs: &[f32], out: &mut [f32]) -> Result<()> {
        if out.len() != self.output_dim() {
            bail!(
                "policy output buffer length {} != expected {}",
                out.len(),
                self.output_dim()
            );
        }
        let result = self.infer(obs)?;
        if result.len() != out.len() {
            bail!(
                "policy returned {} floats, expected {}",
                result.len(),
                out.len()
            );
        }
        out.copy_from_slice(&result);
        Ok(())
    }
}

/// Deterministic analytical policy for unit and integration tests without an external ONNX runtime.
pub struct GainScheduleTestBackend {
    input_dim: usize,
    output_dim: usize,
    wheel_vel_scale: f32,
}

impl GainScheduleTestBackend {
    pub fn new(config: &DeployConfig) -> Self {
        Self {
            input_dim: config.obs_dim(),
            output_dim: 16,
            wheel_vel_scale: 1.0,
        }
    }
}

impl PolicyBackend for GainScheduleTestBackend {
    fn input_dim(&self) -> usize {
        self.input_dim
    }

    fn output_dim(&self) -> usize {
        self.output_dim
    }

    fn infer(&mut self, obs: &[f32]) -> Result<Vec<f32>> {
        let mut out = vec![0.0_f32; self.output_dim];
        self.infer_into(obs, &mut out)?;
        Ok(out)
    }

    fn infer_into(&mut self, obs: &[f32], out: &mut [f32]) -> Result<()> {
        if obs.len() != self.input_dim {
            bail!(
                "GainScheduleTestBackend expected {} floats, got {}",
                self.input_dim,
                obs.len()
            );
        }
        if out.len() != self.output_dim {
            bail!(
                "GainScheduleTestBackend expected output buffer {}, got {}",
                self.output_dim,
                out.len()
            );
        }
        out.fill(0.0);
        let base = obs.len().saturating_sub(57);
        let cmd_vx = obs[base + 6];
        for item in &mut out[12..16] {
            *item = cmd_vx * self.wheel_vel_scale;
        }
        Ok(())
    }
}

const ORT_LOGGING_LEVEL_WARNING: u32 = 2;
const ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT: u32 = 1;
const ORT_ARENA_ALLOCATOR: i32 = 0;
const ORT_MEM_TYPE_DEFAULT: i32 = 0;
const ORT_SEQUENTIAL: u32 = 0;
const ORT_ENABLE_ALL: u32 = 99;

#[repr(C)]
struct OrtApiBase {
    get_api: unsafe extern "system" fn(u32) -> *const usize,
    _get_version_string: unsafe extern "system" fn() -> *const c_char,
}

#[repr(C)]
struct OrtEnv {
    _private: [u8; 0],
}
#[repr(C)]
struct OrtStatus {
    _private: [u8; 0],
}
#[repr(C)]
struct OrtSessionOptions {
    _private: [u8; 0],
}
#[repr(C)]
struct OrtSession {
    _private: [u8; 0],
}
#[repr(C)]
struct OrtRunOptions {
    _private: [u8; 0],
}
#[repr(C)]
struct OrtValue {
    _private: [u8; 0],
}
#[repr(C)]
struct OrtTypeInfo {
    _private: [u8; 0],
}
#[repr(C)]
struct OrtTensorTypeAndShapeInfo {
    _private: [u8; 0],
}
#[repr(C)]
struct OrtMemoryInfo {
    _private: [u8; 0],
}
#[repr(C)]
struct OrtAllocator {
    _private: [u8; 0],
}

// Verified 0-based function pointer slots in OrtApi (onnxruntime_c_api.h API v14..=v22).
const SLOT_GET_ERROR_MESSAGE: usize = 2;
const SLOT_CREATE_ENV: usize = 3;
const SLOT_CREATE_SESSION_FROM_ARRAY: usize = 8;
const SLOT_RUN: usize = 9;
const SLOT_CREATE_SESSION_OPTIONS: usize = 10;
const SLOT_SET_SESSION_EXECUTION_MODE: usize = 13;
const SLOT_ENABLE_MEM_PATTERN: usize = 16;
const SLOT_ENABLE_CPU_MEM_ARENA: usize = 18;
const SLOT_SET_SESSION_GRAPH_OPTIMIZATION_LEVEL: usize = 23;
const SLOT_SET_INTRA_OP_NUM_THREADS: usize = 24;
const SLOT_SET_INTER_OP_NUM_THREADS: usize = 25;
const SLOT_SESSION_GET_INPUT_COUNT: usize = 30;
const SLOT_SESSION_GET_OUTPUT_COUNT: usize = 31;
const SLOT_SESSION_GET_INPUT_TYPE_INFO: usize = 33;
const SLOT_SESSION_GET_OUTPUT_TYPE_INFO: usize = 34;
const SLOT_SESSION_GET_INPUT_NAME: usize = 36;
const SLOT_SESSION_GET_OUTPUT_NAME: usize = 37;
const SLOT_CREATE_TENSOR_WITH_DATA_AS_ORT_VALUE: usize = 49;
const SLOT_GET_TENSOR_MUTABLE_DATA: usize = 51;
const SLOT_CAST_TYPE_INFO_TO_TENSOR_INFO: usize = 55;
const SLOT_GET_DIMENSIONS_COUNT: usize = 61;
const SLOT_GET_DIMENSIONS: usize = 62;
const SLOT_CREATE_CPU_MEMORY_INFO: usize = 69;
const SLOT_ALLOCATOR_FREE: usize = 76;
const SLOT_GET_ALLOCATOR_WITH_DEFAULT_OPTIONS: usize = 78;
const SLOT_RELEASE_ENV: usize = 92;
const SLOT_RELEASE_STATUS: usize = 93;
const SLOT_RELEASE_MEMORY_INFO: usize = 94;
const SLOT_RELEASE_SESSION: usize = 95;
const SLOT_RELEASE_VALUE: usize = 96;
const SLOT_RELEASE_TYPE_INFO: usize = 98;
const SLOT_RELEASE_SESSION_OPTIONS: usize = 100;

struct OrtVtable {
    _lib: Library,
    table: *const usize,
}

unsafe impl Send for OrtVtable {}
unsafe impl Sync for OrtVtable {}

impl OrtVtable {
    unsafe fn slot<T: Copy>(&self, idx: usize) -> T {
        let ptr = *self.table.add(idx) as *const c_void;
        std::mem::transmute_copy(&ptr)
    }

    unsafe fn check(&self, status: *mut OrtStatus) -> Result<()> {
        if status.is_null() {
            return Ok(());
        }
        let get_msg: unsafe extern "system" fn(*const OrtStatus) -> *const c_char =
            self.slot(SLOT_GET_ERROR_MESSAGE);
        let release_status: unsafe extern "system" fn(*mut OrtStatus) =
            self.slot(SLOT_RELEASE_STATUS);
        let msg_ptr = get_msg(status);
        let msg = if msg_ptr.is_null() {
            "unknown ONNX Runtime error".to_string()
        } else {
            CStr::from_ptr(msg_ptr).to_string_lossy().into_owned()
        };
        release_status(status);
        Err(anyhow!("ONNX Runtime error: {msg}"))
    }
}

pub struct OrtDynamicBackend {
    api: OrtVtable,
    env: *mut OrtEnv,
    session: *mut OrtSession,
    memory_info: *mut OrtMemoryInfo,
    input_name: CString,
    output_name: CString,
    input_shape: Vec<i64>,
    output_shape: Vec<i64>,
    input_dim: usize,
    output_dim: usize,
    // Pre-allocated, fixed-address backing buffers bound once to OrtValue tensors
    // so 200Hz-1kHz inference runs with zero heap allocations and zero OrtValue churn.
    input_buf: Box<[f32]>,
    output_buf: Box<[f32]>,
    input_val: *mut OrtValue,
    output_val: *mut OrtValue,
}

unsafe impl Send for OrtDynamicBackend {}
unsafe impl Sync for OrtDynamicBackend {}

impl Drop for OrtDynamicBackend {
    fn drop(&mut self) {
        unsafe {
            let release_val: unsafe extern "system" fn(*mut OrtValue) =
                self.api.slot(SLOT_RELEASE_VALUE);
            if !self.input_val.is_null() {
                release_val(self.input_val);
                self.input_val = ptr::null_mut();
            }
            if !self.output_val.is_null() {
                release_val(self.output_val);
                self.output_val = ptr::null_mut();
            }
            if !self.memory_info.is_null() {
                let release: unsafe extern "system" fn(*mut OrtMemoryInfo) =
                    self.api.slot(SLOT_RELEASE_MEMORY_INFO);
                release(self.memory_info);
                self.memory_info = ptr::null_mut();
            }
            if !self.session.is_null() {
                let release: unsafe extern "system" fn(*mut OrtSession) =
                    self.api.slot(SLOT_RELEASE_SESSION);
                release(self.session);
                self.session = ptr::null_mut();
            }
            if !self.env.is_null() {
                let release: unsafe extern "system" fn(*mut OrtEnv) =
                    self.api.slot(SLOT_RELEASE_ENV);
                release(self.env);
                self.env = ptr::null_mut();
            }
        }
    }
}

pub fn discover_onnxruntime_library(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    if let Ok(env_path) = std::env::var("ONNXRUNTIME_DLL_PATH") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return Ok(p);
        }
    }
    #[cfg(target_os = "windows")]
    let candidates = [
        "onnxruntime.dll",
        r"C:\Users\99563\miniconda3\Lib\site-packages\onnxruntime\capi\onnxruntime.dll",
    ];
    #[cfg(not(target_os = "windows"))]
    let candidates = [
        "libonnxruntime.so",
        "/usr/local/lib/libonnxruntime.so",
        "/usr/lib/libonnxruntime.so",
    ];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return Ok(p);
        }
    }
    Ok(PathBuf::from(candidates[0]))
}

impl OrtDynamicBackend {
    pub fn load(model_path: impl AsRef<Path>, dll_path: Option<&Path>) -> Result<Self> {
        let model_path = model_path.as_ref();
        let model_bytes = std::fs::read(model_path)
            .with_context(|| format!("failed to read ONNX model {}", model_path.display()))?;
        let lib_path = discover_onnxruntime_library(dll_path)?;

        let lib = unsafe { Library::new(&lib_path) }.with_context(|| {
            format!("failed to load ONNX Runtime library {}", lib_path.display())
        })?;

        let api_table = unsafe {
            let get_base: libloading::Symbol<unsafe extern "system" fn() -> *const OrtApiBase> =
                lib.get(b"OrtGetApiBase")?;
            let base = get_base();
            if base.is_null() {
                bail!("OrtGetApiBase returned null");
            }
            let mut found = ptr::null();
            for ver in (14_u32..=22).rev() {
                let candidate = ((*base).get_api)(ver);
                if !candidate.is_null() {
                    found = candidate;
                    break;
                }
            }
            if found.is_null() {
                bail!("OrtApiBase::GetApi returned null for versions 14..=22");
            }
            found
        };

        let api = OrtVtable {
            _lib: lib,
            table: api_table,
        };

        unsafe {
            let mut env: *mut OrtEnv = ptr::null_mut();
            let log_id = CString::new("doso-rl-runner")?;
            let create_env: unsafe extern "system" fn(
                u32,
                *const c_char,
                *mut *mut OrtEnv,
            ) -> *mut OrtStatus = api.slot(SLOT_CREATE_ENV);
            api.check(create_env(
                ORT_LOGGING_LEVEL_WARNING,
                log_id.as_ptr(),
                &mut env,
            ))?;

            let mut opts: *mut OrtSessionOptions = ptr::null_mut();
            let create_opts: unsafe extern "system" fn(
                *mut *mut OrtSessionOptions,
            ) -> *mut OrtStatus = api.slot(SLOT_CREATE_SESSION_OPTIONS);
            if let Err(err) = api.check(create_opts(&mut opts)) {
                let release_env: unsafe extern "system" fn(*mut OrtEnv) =
                    api.slot(SLOT_RELEASE_ENV);
                release_env(env);
                return Err(err);
            }

            // Deterministic real-time single-thread execution with full graph fusion & arena reuse.
            let set_exec_mode: unsafe extern "system" fn(
                *mut OrtSessionOptions,
                u32,
            ) -> *mut OrtStatus = api.slot(SLOT_SET_SESSION_EXECUTION_MODE);
            let _ = api.check(set_exec_mode(opts, ORT_SEQUENTIAL));

            let enable_mem_pattern: unsafe extern "system" fn(
                *mut OrtSessionOptions,
            ) -> *mut OrtStatus = api.slot(SLOT_ENABLE_MEM_PATTERN);
            let _ = api.check(enable_mem_pattern(opts));

            let enable_arena: unsafe extern "system" fn(*mut OrtSessionOptions) -> *mut OrtStatus =
                api.slot(SLOT_ENABLE_CPU_MEM_ARENA);
            let _ = api.check(enable_arena(opts));

            let set_opt_level: unsafe extern "system" fn(
                *mut OrtSessionOptions,
                u32,
            ) -> *mut OrtStatus = api.slot(SLOT_SET_SESSION_GRAPH_OPTIMIZATION_LEVEL);
            let _ = api.check(set_opt_level(opts, ORT_ENABLE_ALL));

            let set_intra: unsafe extern "system" fn(
                *mut OrtSessionOptions,
                i32,
            ) -> *mut OrtStatus = api.slot(SLOT_SET_INTRA_OP_NUM_THREADS);
            let _ = api.check(set_intra(opts, 1));

            let set_inter: unsafe extern "system" fn(
                *mut OrtSessionOptions,
                i32,
            ) -> *mut OrtStatus = api.slot(SLOT_SET_INTER_OP_NUM_THREADS);
            let _ = api.check(set_inter(opts, 1));

            let mut session: *mut OrtSession = ptr::null_mut();
            let create_sess: unsafe extern "system" fn(
                *const OrtEnv,
                *const c_void,
                usize,
                *const OrtSessionOptions,
                *mut *mut OrtSession,
            ) -> *mut OrtStatus = api.slot(SLOT_CREATE_SESSION_FROM_ARRAY);
            let sess_res = api.check(create_sess(
                env,
                model_bytes.as_ptr() as *const c_void,
                model_bytes.len(),
                opts,
                &mut session,
            ));
            let release_opts: unsafe extern "system" fn(*mut OrtSessionOptions) =
                api.slot(SLOT_RELEASE_SESSION_OPTIONS);
            release_opts(opts);
            if let Err(err) = sess_res {
                let release_env: unsafe extern "system" fn(*mut OrtEnv) =
                    api.slot(SLOT_RELEASE_ENV);
                release_env(env);
                return Err(err);
            }

            let mut mem_info: *mut OrtMemoryInfo = ptr::null_mut();
            let create_mem: unsafe extern "system" fn(
                i32,
                i32,
                *mut *mut OrtMemoryInfo,
            ) -> *mut OrtStatus = api.slot(SLOT_CREATE_CPU_MEMORY_INFO);
            api.check(create_mem(
                ORT_ARENA_ALLOCATOR,
                ORT_MEM_TYPE_DEFAULT,
                &mut mem_info,
            ))?;

            let mut backend = Self {
                api,
                env,
                session,
                memory_info: mem_info,
                input_name: CString::new("obs")?,
                output_name: CString::new("actions")?,
                input_shape: Vec::new(),
                output_shape: Vec::new(),
                input_dim: 0,
                output_dim: 0,
                input_buf: Box::new([]),
                output_buf: Box::new([]),
                input_val: ptr::null_mut(),
                output_val: ptr::null_mut(),
            };
            backend.inspect_io()?;
            backend.init_prebound_tensors()?;
            Ok(backend)
        }
    }

    unsafe fn init_prebound_tensors(&mut self) -> Result<()> {
        self.input_buf = vec![0.0_f32; self.input_dim].into_boxed_slice();
        self.output_buf = vec![0.0_f32; self.output_dim].into_boxed_slice();

        let create_tensor: unsafe extern "system" fn(
            *const OrtMemoryInfo,
            *mut c_void,
            usize,
            *const i64,
            usize,
            u32,
            *mut *mut OrtValue,
        ) -> *mut OrtStatus = self.api.slot(SLOT_CREATE_TENSOR_WITH_DATA_AS_ORT_VALUE);

        self.api.check(create_tensor(
            self.memory_info,
            self.input_buf.as_mut_ptr() as *mut c_void,
            std::mem::size_of_val(&*self.input_buf),
            self.input_shape.as_ptr(),
            self.input_shape.len(),
            ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT,
            &mut self.input_val,
        ))?;

        self.api.check(create_tensor(
            self.memory_info,
            self.output_buf.as_mut_ptr() as *mut c_void,
            std::mem::size_of_val(&*self.output_buf),
            self.output_shape.as_ptr(),
            self.output_shape.len(),
            ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT,
            &mut self.output_val,
        ))?;
        Ok(())
    }

    unsafe fn inspect_io(&mut self) -> Result<()> {
        let mut alloc: *mut OrtAllocator = ptr::null_mut();
        let get_alloc: unsafe extern "system" fn(*mut *mut OrtAllocator) -> *mut OrtStatus =
            self.api.slot(SLOT_GET_ALLOCATOR_WITH_DEFAULT_OPTIONS);
        self.api.check(get_alloc(&mut alloc))?;

        let mut in_count = 0usize;
        let get_in_cnt: unsafe extern "system" fn(*const OrtSession, *mut usize) -> *mut OrtStatus =
            self.api.slot(SLOT_SESSION_GET_INPUT_COUNT);
        self.api.check(get_in_cnt(self.session, &mut in_count))?;
        if in_count == 0 {
            bail!("ONNX model has 0 inputs");
        }

        let mut out_count = 0usize;
        let get_out_cnt: unsafe extern "system" fn(
            *const OrtSession,
            *mut usize,
        ) -> *mut OrtStatus = self.api.slot(SLOT_SESSION_GET_OUTPUT_COUNT);
        self.api.check(get_out_cnt(self.session, &mut out_count))?;
        if out_count == 0 {
            bail!("ONNX model has 0 outputs");
        }

        let mut in_name_ptr: *mut c_char = ptr::null_mut();
        let get_in_name: unsafe extern "system" fn(
            *const OrtSession,
            usize,
            *mut OrtAllocator,
            *mut *mut c_char,
        ) -> *mut OrtStatus = self.api.slot(SLOT_SESSION_GET_INPUT_NAME);
        self.api
            .check(get_in_name(self.session, 0, alloc, &mut in_name_ptr))?;
        let in_name_str = CStr::from_ptr(in_name_ptr).to_string_lossy().into_owned();
        let alloc_free: unsafe extern "system" fn(
            *mut OrtAllocator,
            *mut c_void,
        ) -> *mut OrtStatus = self.api.slot(SLOT_ALLOCATOR_FREE);
        let _ = self
            .api
            .check(alloc_free(alloc, in_name_ptr as *mut c_void));
        self.input_name = CString::new(in_name_str)?;

        let mut out_name_ptr: *mut c_char = ptr::null_mut();
        let get_out_name: unsafe extern "system" fn(
            *const OrtSession,
            usize,
            *mut OrtAllocator,
            *mut *mut c_char,
        ) -> *mut OrtStatus = self.api.slot(SLOT_SESSION_GET_OUTPUT_NAME);
        self.api
            .check(get_out_name(self.session, 0, alloc, &mut out_name_ptr))?;
        let out_name_str = CStr::from_ptr(out_name_ptr).to_string_lossy().into_owned();
        let _ = self
            .api
            .check(alloc_free(alloc, out_name_ptr as *mut c_void));
        self.output_name = CString::new(out_name_str)?;

        let (in_shape, in_elems) = self.read_tensor_dims(true)?;
        let (out_shape, out_elems) = self.read_tensor_dims(false)?;
        self.input_shape = in_shape;
        self.output_shape = out_shape;
        self.input_dim = in_elems;
        self.output_dim = out_elems;
        Ok(())
    }

    unsafe fn read_tensor_dims(&self, is_input: bool) -> Result<(Vec<i64>, usize)> {
        let mut type_info: *mut OrtTypeInfo = ptr::null_mut();
        let slot_idx = if is_input {
            SLOT_SESSION_GET_INPUT_TYPE_INFO
        } else {
            SLOT_SESSION_GET_OUTPUT_TYPE_INFO
        };
        let get_ti: unsafe extern "system" fn(
            *const OrtSession,
            usize,
            *mut *mut OrtTypeInfo,
        ) -> *mut OrtStatus = self.api.slot(slot_idx);
        self.api.check(get_ti(self.session, 0, &mut type_info))?;

        let mut tensor_info: *const OrtTensorTypeAndShapeInfo = ptr::null();
        let cast_ti: unsafe extern "system" fn(
            *const OrtTypeInfo,
            *mut *const OrtTensorTypeAndShapeInfo,
        ) -> *mut OrtStatus = self.api.slot(SLOT_CAST_TYPE_INFO_TO_TENSOR_INFO);
        let cast_res = self.api.check(cast_ti(type_info, &mut tensor_info));
        let release_ti: unsafe extern "system" fn(*mut OrtTypeInfo) =
            self.api.slot(SLOT_RELEASE_TYPE_INFO);
        if let Err(err) = cast_res {
            release_ti(type_info);
            return Err(err);
        }

        let mut rank = 0usize;
        let get_cnt: unsafe extern "system" fn(
            *const OrtTensorTypeAndShapeInfo,
            *mut usize,
        ) -> *mut OrtStatus = self.api.slot(SLOT_GET_DIMENSIONS_COUNT);
        if let Err(err) = self.api.check(get_cnt(tensor_info, &mut rank)) {
            release_ti(type_info);
            return Err(err);
        }

        let mut dims = vec![0_i64; rank];
        if rank > 0 {
            let get_dims: unsafe extern "system" fn(
                *const OrtTensorTypeAndShapeInfo,
                *mut i64,
                usize,
            ) -> *mut OrtStatus = self.api.slot(SLOT_GET_DIMENSIONS);
            if let Err(err) = self
                .api
                .check(get_dims(tensor_info, dims.as_mut_ptr(), rank))
            {
                release_ti(type_info);
                return Err(err);
            }
        }
        release_ti(type_info);

        if dims.is_empty() {
            return Ok((vec![1], 1));
        }
        let mut resolved = Vec::with_capacity(dims.len());
        let mut total = 1usize;
        for d in dims {
            let positive = if d <= 0 { 1 } else { d };
            resolved.push(positive);
            total *= positive as usize;
        }
        Ok((resolved, total))
    }
}

impl PolicyBackend for OrtDynamicBackend {
    fn input_dim(&self) -> usize {
        self.input_dim
    }

    fn output_dim(&self) -> usize {
        self.output_dim
    }

    fn infer(&mut self, obs: &[f32]) -> Result<Vec<f32>> {
        let mut out = vec![0.0_f32; self.output_dim];
        self.infer_into(obs, &mut out)?;
        Ok(out)
    }

    fn infer_into(&mut self, obs: &[f32], out: &mut [f32]) -> Result<()> {
        if obs.len() != self.input_dim {
            bail!(
                "ONNX policy expects {} floats for shape {:?}, got {}",
                self.input_dim,
                self.input_shape,
                obs.len()
            );
        }
        if out.len() != self.output_dim {
            bail!(
                "ONNX policy output buffer length {} != expected {}",
                out.len(),
                self.output_dim
            );
        }

        self.input_buf.copy_from_slice(obs);

        unsafe {
            let in_names = [self.input_name.as_ptr()];
            let out_names = [self.output_name.as_ptr()];
            let inputs = [self.input_val as *const OrtValue];
            let mut outputs = [self.output_val];

            let run_fn: unsafe extern "system" fn(
                *mut OrtSession,
                *const OrtRunOptions,
                *const *const c_char,
                *const *const OrtValue,
                usize,
                *const *const c_char,
                usize,
                *mut *mut OrtValue,
            ) -> *mut OrtStatus = self.api.slot(SLOT_RUN);

            self.api.check(run_fn(
                self.session,
                ptr::null(),
                in_names.as_ptr(),
                inputs.as_ptr(),
                1,
                out_names.as_ptr(),
                1,
                outputs.as_mut_ptr(),
            ))?;

            // Fast path: ORT wrote directly into our pre-bound output_buf (zero alloc).
            // Defensive fallback: if an ORT version replaced outputs[0], read & release it.
            if outputs[0] == self.output_val {
                out.copy_from_slice(&self.output_buf);
                Ok(())
            } else {
                let mut data_ptr: *mut c_void = ptr::null_mut();
                let get_data: unsafe extern "system" fn(
                    *mut OrtValue,
                    *mut *mut c_void,
                ) -> *mut OrtStatus = self.api.slot(SLOT_GET_TENSOR_MUTABLE_DATA);
                let data_res = self.api.check(get_data(outputs[0], &mut data_ptr));
                let result = match data_res {
                    Ok(()) if !data_ptr.is_null() => {
                        let slice =
                            std::slice::from_raw_parts(data_ptr as *const f32, self.output_dim);
                        out.copy_from_slice(slice);
                        Ok(())
                    }
                    Ok(()) => Err(anyhow!("ONNX Runtime returned null output data pointer")),
                    Err(err) => Err(err),
                };
                if !outputs[0].is_null() {
                    let release_val: unsafe extern "system" fn(*mut OrtValue) =
                        self.api.slot(SLOT_RELEASE_VALUE);
                    release_val(outputs[0]);
                }
                result
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obs::ObservationBuilder;

    fn require_ort_dll_for_test() -> Option<PathBuf> {
        let p = discover_onnxruntime_library(None).ok()?;
        if p.exists() {
            Some(p)
        } else if std::env::var_os("ONNXRUNTIME_DLL_PATH").is_some() {
            panic!(
                "ONNXRUNTIME_DLL_PATH was set but {} does not exist",
                p.display()
            );
        } else {
            None
        }
    }

    #[test]
    fn ort_dynamic_backend_runs_add_one_and_policy_onnx() {
        let Some(dll) = require_ort_dll_for_test() else {
            return;
        };

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let add_one = root.join("../../../brainstem/onnx_runtime/test/add_one.onnx");
        if !add_one.exists() {
            eprintln!(
                "skipping sibling brainstem ONNX fixture test ({} not present in standalone checkout)",
                add_one.display()
            );
            return;
        }
        let mut backend_add = OrtDynamicBackend::load(&add_one, Some(&dll)).unwrap();
        let in_dim = backend_add.input_dim();
        let out_add = backend_add.infer(&vec![2.5_f32; in_dim]).unwrap();
        assert_eq!(out_add.len(), backend_add.output_dim());
        assert!(
            (out_add[0] - 3.5).abs() < 1e-4,
            "expected 3.5, got {:?}",
            out_add
        );

        let deploy_yaml = root.join("examples/thunder_16dof_deploy.yaml");
        let cfg = DeployConfig::from_path(&deploy_yaml).unwrap();
        let mut backend_policy = OrtDynamicBackend::load(&cfg.policy_path, Some(&dll)).unwrap();
        assert_eq!(backend_policy.input_dim(), cfg.obs_dim());
        assert_eq!(backend_policy.output_dim(), 16);

        let mut builder = ObservationBuilder::new(cfg.clone());
        let obs = builder
            .push_and_build(
                [0.0, 0.0, 0.0],
                [0.0, 0.0, -1.0],
                [0.0, 0.0, 0.0],
                &cfg.default_joint_pos,
                &[0.0; 16],
                &[0.0; 16],
            )
            .unwrap();
        let actions = backend_policy.infer(&obs).unwrap();
        assert_eq!(actions.len(), 16);
        assert!(actions.iter().all(|v| v.is_finite()));
    }
}
