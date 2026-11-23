#![allow(unsafe_code)]

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Context, Result};
use libloading::Library;

use crate::policy::blocks::PolicyManifest;

pub trait InferenceBackend: Send {
    fn backend_name(&self) -> &'static str;
    fn infer_into(&mut self, inputs: &[(&str, &[f32])], outputs: &mut [Vec<f32>]) -> Result<()>;
}

/// Deterministic analytical mock backend for fast offline/CI tests without a native ORT library.
pub struct AnalyticalMockBackend {
    wheel_scale: f32,
}

impl Default for AnalyticalMockBackend {
    fn default() -> Self {
        Self { wheel_scale: 0.5 }
    }
}

impl InferenceBackend for AnalyticalMockBackend {
    fn backend_name(&self) -> &'static str {
        "analytical-mock"
    }

    fn infer_into(&mut self, inputs: &[(&str, &[f32])], outputs: &mut [Vec<f32>]) -> Result<()> {
        if inputs.is_empty() || outputs.is_empty() {
            bail!("AnalyticalMockBackend requires >=1 input and >=1 output");
        }
        let first = inputs[0].1;
        let vx = if first.len() >= 9 { first[6] } else { 0.0 };
        let out = &mut outputs[0];
        out.resize(16, 0.0);
        out.fill(0.0);
        for item in &mut out[12..16] {
            *item = vx * self.wheel_scale;
        }
        for aux in outputs.iter_mut().skip(1) {
            aux.fill(0.0);
        }
        Ok(())
    }
}

// ─── Dynamic ONNX Runtime C-API Backend (zero-alloc pre-bound I/O hot path) ────

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
struct OrtEnv([u8; 0]);
#[repr(C)]
struct OrtStatus([u8; 0]);
#[repr(C)]
struct OrtSessionOptions([u8; 0]);
#[repr(C)]
struct OrtSession([u8; 0]);
#[repr(C)]
struct OrtRunOptions([u8; 0]);
#[repr(C)]
struct OrtValue([u8; 0]);
#[repr(C)]
struct OrtTypeInfo([u8; 0]);
#[repr(C)]
struct OrtTensorTypeAndShapeInfo([u8; 0]);
#[repr(C)]
struct OrtMemoryInfo([u8; 0]);
#[repr(C)]
struct OrtAllocator([u8; 0]);

// Verified 0-based function pointer slots in OrtApi (onnxruntime_c_api.h API v14..=v22).
const SLOT_GET_ERROR_MESSAGE: usize = 2;
const SLOT_CREATE_ENV: usize = 3;
const SLOT_CREATE_SESSION_FROM_ARRAY: usize = 8;
const SLOT_RUN: usize = 9;
const SLOT_CREATE_SESSION_OPTIONS: usize = 10;
const SLOT_SET_SESSION_EXECUTION_MODE: usize = 13;
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
const SLOT_GET_TENSOR_SHAPE_ELEMENT_COUNT: usize = 64;
const SLOT_GET_TENSOR_TYPE_AND_SHAPE: usize = 65;
const SLOT_CREATE_CPU_MEMORY_INFO: usize = 69;
const SLOT_ALLOCATOR_FREE: usize = 76;
const SLOT_GET_ALLOCATOR_WITH_DEFAULT_OPTIONS: usize = 78;
const SLOT_RELEASE_STATUS: usize = 93;
const SLOT_RELEASE_SESSION: usize = 95;
const SLOT_RELEASE_VALUE: usize = 96;
const SLOT_RELEASE_TYPE_INFO: usize = 98;
const SLOT_RELEASE_TENSOR_TYPE_AND_SHAPE_INFO: usize = 99;
const SLOT_RELEASE_SESSION_OPTIONS: usize = 100;

type GetErrorMessageFn = unsafe extern "system" fn(*const OrtStatus) -> *const c_char;
type ReleaseStatusFn = unsafe extern "system" fn(*mut OrtStatus);
type CreateEnvFn =
    unsafe extern "system" fn(u32, *const c_char, *mut *mut OrtEnv) -> *mut OrtStatus;
type CreateSessionOptionsFn =
    unsafe extern "system" fn(*mut *mut OrtSessionOptions) -> *mut OrtStatus;
type SetSessionU32OptFn = unsafe extern "system" fn(*mut OrtSessionOptions, u32) -> *mut OrtStatus;
type SetThreadsFn = unsafe extern "system" fn(*mut OrtSessionOptions, i32) -> *mut OrtStatus;
type CreateSessionFromArrayFn = unsafe extern "system" fn(
    *const OrtEnv,
    *const c_void,
    usize,
    *const OrtSessionOptions,
    *mut *mut OrtSession,
) -> *mut OrtStatus;
type SessionGetCountFn = unsafe extern "system" fn(*const OrtSession, *mut usize) -> *mut OrtStatus;
type SessionGetTypeInfoFn =
    unsafe extern "system" fn(*const OrtSession, usize, *mut *mut OrtTypeInfo) -> *mut OrtStatus;
type SessionGetNameFn = unsafe extern "system" fn(
    *const OrtSession,
    usize,
    *mut OrtAllocator,
    *mut *mut c_char,
) -> *mut OrtStatus;
type GetAllocatorFn = unsafe extern "system" fn(*mut *mut OrtAllocator) -> *mut OrtStatus;
type AllocatorFreeFn = unsafe extern "system" fn(*mut OrtAllocator, *mut c_void) -> *mut OrtStatus;
type CastTypeInfoFn = unsafe extern "system" fn(
    *const OrtTypeInfo,
    *mut *const OrtTensorTypeAndShapeInfo,
) -> *mut OrtStatus;
type GetDimensionsCountFn =
    unsafe extern "system" fn(*const OrtTensorTypeAndShapeInfo, *mut usize) -> *mut OrtStatus;
type GetDimensionsFn =
    unsafe extern "system" fn(*const OrtTensorTypeAndShapeInfo, *mut i64, usize) -> *mut OrtStatus;
type GetTensorShapeElementCountFn =
    unsafe extern "system" fn(*const OrtTensorTypeAndShapeInfo, *mut usize) -> *mut OrtStatus;
type GetTensorTypeAndShapeFn = unsafe extern "system" fn(
    *const OrtValue,
    *mut *mut OrtTensorTypeAndShapeInfo,
) -> *mut OrtStatus;
type CreateCpuMemoryInfoFn =
    unsafe extern "system" fn(i32, i32, *mut *mut OrtMemoryInfo) -> *mut OrtStatus;
type CreateTensorWithDataFn = unsafe extern "system" fn(
    *const OrtMemoryInfo,
    *mut c_void,
    usize,
    *const i64,
    usize,
    u32,
    *mut *mut OrtValue,
) -> *mut OrtStatus;
type GetTensorMutableDataFn =
    unsafe extern "system" fn(*mut OrtValue, *mut *mut c_void) -> *mut OrtStatus;
type RunFn = unsafe extern "system" fn(
    *mut OrtSession,
    *const OrtRunOptions,
    *const *const c_char,
    *const *const OrtValue,
    usize,
    *const *const c_char,
    usize,
    *mut *mut OrtValue,
) -> *mut OrtStatus;
type ReleasePtrFn = unsafe extern "system" fn(*mut c_void);

/// Process-wide singleton holding the loaded ORT dynamic library, global `OrtEnv`, and CPU `OrtMemoryInfo`.
/// Ensures zero reload overhead and safe concurrent/hot-deploy session creation & destruction.
struct OrtGlobalRuntime {
    _lib: Library,
    api_table: *const usize,
    env: *mut OrtEnv,
    memory_info: *mut OrtMemoryInfo,
}

unsafe impl Send for OrtGlobalRuntime {}
unsafe impl Sync for OrtGlobalRuntime {}

static ORT_GLOBAL: OnceLock<std::result::Result<OrtGlobalRuntime, String>> = OnceLock::new();

impl OrtGlobalRuntime {
    fn get() -> Result<&'static Self> {
        let res = ORT_GLOBAL.get_or_init(|| Self::init().map_err(|e| format!("{e:#}")));
        match res {
            Ok(rt) => Ok(rt),
            Err(msg) => bail!("{msg}"),
        }
    }

    fn init() -> Result<Self> {
        let lib = OrtDynamicMultiBackend::open_ort_library()?;
        unsafe {
            let get_base: libloading::Symbol<unsafe extern "system" fn() -> *const OrtApiBase> =
                lib.get(b"OrtGetApiBase\0")
                    .context("missing OrtGetApiBase symbol in onnxruntime library")?;
            let base = get_base();
            if base.is_null() {
                bail!("OrtGetApiBase returned null");
            }
            let mut api_table: *const usize = ptr::null();
            for ver in (14..=22).rev() {
                let candidate = ((*base).get_api)(ver);
                if !candidate.is_null() {
                    api_table = candidate;
                    break;
                }
            }
            if api_table.is_null() {
                bail!("onnxruntime C-API v14..=v22 not supported by loaded library");
            }

            let mut env: *mut OrtEnv = ptr::null_mut();
            let log_id = CString::new("doso-runtime")?;
            OrtDynamicMultiBackend::check_status(
                api_table,
                OrtDynamicMultiBackend::slot::<CreateEnvFn>(api_table, SLOT_CREATE_ENV)(
                    ORT_LOGGING_LEVEL_WARNING,
                    log_id.as_ptr(),
                    &mut env,
                ),
                "CreateEnv",
            )?;

            let mut mem_info: *mut OrtMemoryInfo = ptr::null_mut();
            OrtDynamicMultiBackend::check_status(
                api_table,
                OrtDynamicMultiBackend::slot::<CreateCpuMemoryInfoFn>(
                    api_table,
                    SLOT_CREATE_CPU_MEMORY_INFO,
                )(ORT_ARENA_ALLOCATOR, ORT_MEM_TYPE_DEFAULT, &mut mem_info),
                "CreateCpuMemoryInfo",
            )?;

            Ok(Self {
                _lib: lib,
                api_table,
                env,
                memory_info: mem_info,
            })
        }
    }
}

pub struct OrtDynamicMultiBackend {
    rt: &'static OrtGlobalRuntime,
    session: *mut OrtSession,
    _input_names: Vec<CString>,
    input_name_ptrs: Vec<*const c_char>,
    input_dims: Vec<usize>,
    _output_names: Vec<CString>,
    output_dims: Vec<usize>,
    output_name_ptrs: Vec<*const c_char>,
    pinned_inputs: Vec<Box<[f32]>>,
    bound_in_values: Vec<*mut OrtValue>,
    pinned_outputs: Vec<Box<[f32]>>,
    bound_out_values: Vec<*mut OrtValue>,
}

unsafe impl Send for OrtDynamicMultiBackend {}

impl OrtDynamicMultiBackend {
    pub fn load_for_manifest(manifest: &PolicyManifest, onnx_bytes: &[u8]) -> Result<Self> {
        let rt = OrtGlobalRuntime::get()?;
        let api_table = rt.api_table;
        unsafe {
            let mut opts: *mut OrtSessionOptions = ptr::null_mut();
            Self::check_status(
                api_table,
                Self::slot::<CreateSessionOptionsFn>(api_table, SLOT_CREATE_SESSION_OPTIONS)(
                    &mut opts,
                ),
                "CreateSessionOptions",
            )?;

            Self::release_if_non_null(
                api_table,
                Self::slot::<SetSessionU32OptFn>(api_table, SLOT_SET_SESSION_EXECUTION_MODE)(
                    opts,
                    ORT_SEQUENTIAL,
                ),
            );
            Self::release_if_non_null(
                api_table,
                Self::slot::<SetSessionU32OptFn>(
                    api_table,
                    SLOT_SET_SESSION_GRAPH_OPTIMIZATION_LEVEL,
                )(opts, ORT_ENABLE_ALL),
            );
            Self::release_if_non_null(
                api_table,
                Self::slot::<SetThreadsFn>(api_table, SLOT_SET_INTRA_OP_NUM_THREADS)(opts, 1),
            );
            Self::release_if_non_null(
                api_table,
                Self::slot::<SetThreadsFn>(api_table, SLOT_SET_INTER_OP_NUM_THREADS)(opts, 1),
            );

            let mut session: *mut OrtSession = ptr::null_mut();
            let create_res = Self::check_status(
                api_table,
                Self::slot::<CreateSessionFromArrayFn>(api_table, SLOT_CREATE_SESSION_FROM_ARRAY)(
                    rt.env,
                    onnx_bytes.as_ptr().cast(),
                    onnx_bytes.len(),
                    opts,
                    &mut session,
                ),
                "CreateSessionFromArray",
            );
            Self::slot::<ReleasePtrFn>(api_table, SLOT_RELEASE_SESSION_OPTIONS)(opts.cast());
            create_res?;

            // Wrap `session` immediately so `Drop` releases `bound_*_values` before `session` on any error.
            let mut backend = Self {
                rt,
                session,
                _input_names: Vec::new(),
                input_name_ptrs: Vec::new(),
                input_dims: Vec::new(),
                _output_names: Vec::new(),
                output_dims: Vec::new(),
                output_name_ptrs: Vec::new(),
                pinned_inputs: Vec::new(),
                bound_in_values: Vec::new(),
                pinned_outputs: Vec::new(),
                bound_out_values: Vec::new(),
            };

            let (model_in_names, model_in_dims, model_in_shapes) =
                Self::query_io_meta(api_table, session, true)?;
            let (model_out_names, model_out_dims, model_out_shapes) =
                Self::query_io_meta(api_table, session, false)?;

            if manifest.inputs.len() != model_in_names.len() {
                bail!(
                    "Policy '{}' declares {} inputs, but ONNX model has {} ({:?})",
                    manifest.name,
                    manifest.inputs.len(),
                    model_in_names.len(),
                    model_in_names
                );
            }

            let mut input_names = Vec::with_capacity(manifest.inputs.len());
            let mut input_dims = Vec::with_capacity(manifest.inputs.len());
            let mut in_positions = Vec::with_capacity(manifest.inputs.len());
            for (idx, inp_spec) in manifest.inputs.iter().enumerate() {
                let pos = if let Some(at_idx_str) = inp_spec.tensor.strip_prefix('@') {
                    let at_idx: usize = at_idx_str.parse().with_context(|| {
                        format!(
                            "Invalid '@<index>' input tensor selector '{}'",
                            inp_spec.tensor
                        )
                    })?;
                    if at_idx >= model_in_names.len() {
                        bail!(
                            "Input tensor selector '{}' out of range for ONNX inputs {:?} (count={})",
                            inp_spec.tensor,
                            model_in_names,
                            model_in_names.len()
                        );
                    }
                    at_idx
                } else {
                    let Some(p) = model_in_names
                        .iter()
                        .position(|n| n.to_str() == Ok(inp_spec.tensor.as_str()))
                    else {
                        bail!(
                            "Input tensor '{}' not found in ONNX inputs {:?} (index {})",
                            inp_spec.tensor,
                            model_in_names,
                            idx
                        );
                    };
                    p
                };
                let expected_dim = inp_spec.total_dim();
                if expected_dim == 0 {
                    bail!(
                        "Input tensor '{}' resolved to 0 elements in policy '{}'",
                        inp_spec.tensor,
                        manifest.name
                    );
                }
                let model_dim = model_in_dims[pos];
                if model_dim > 0 && model_dim != expected_dim {
                    let resolved_name = model_in_names[pos].to_string_lossy();
                    bail!(
                        "Input tensor '{}' (ONNX '{}') dim mismatch: policy.yaml={} ({}) vs ONNX={}",
                        inp_spec.tensor,
                        resolved_name,
                        expected_dim,
                        inp_spec.dim_breakdown(),
                        model_dim
                    );
                }
                input_names.push(model_in_names[pos].clone());
                input_dims.push(expected_dim);
                in_positions.push(pos);
            }

            let mut wanted_outputs = vec![manifest.outputs.action_tensor.clone()];
            wanted_outputs.extend(manifest.outputs.aux_tensors.iter().cloned());
            let mut output_names = Vec::with_capacity(wanted_outputs.len());
            let mut output_dims = Vec::with_capacity(wanted_outputs.len());
            let mut out_positions = Vec::with_capacity(wanted_outputs.len());
            for wanted in &wanted_outputs {
                let pos = if let Some(at_idx_str) = wanted.strip_prefix('@') {
                    let at_idx: usize = at_idx_str.parse().with_context(|| {
                        format!("Invalid '@<index>' output tensor selector '{wanted}'")
                    })?;
                    if at_idx >= model_out_names.len() {
                        bail!(
                            "Output tensor selector '{wanted}' out of range for ONNX outputs {:?} (count={})",
                            model_out_names,
                            model_out_names.len()
                        );
                    }
                    at_idx
                } else {
                    model_out_names
                        .iter()
                        .position(|n| n.to_str() == Ok(wanted.as_str()))
                        .ok_or_else(|| {
                            anyhow!(
                                "Output tensor '{}' not found in ONNX outputs {:?}",
                                wanted,
                                model_out_names
                            )
                        })?
                };
                output_names.push(model_out_names[pos].clone());
                output_dims.push(model_out_dims[pos]);
                out_positions.push(pos);
            }

            backend.input_name_ptrs = input_names.iter().map(|s| s.as_ptr()).collect();
            backend._input_names = input_names;
            backend.input_dims = input_dims;
            backend.output_name_ptrs = output_names.iter().map(|s| s.as_ptr()).collect();
            backend._output_names = output_names;
            backend.output_dims = output_dims;

            // Pre-allocate pinned input buffers and persistent OrtValue wrappers once at load time.
            backend
                .pinned_inputs
                .reserve_exact(backend.input_dims.len());
            backend
                .bound_in_values
                .reserve_exact(backend.input_dims.len());
            for (&pos, &dim) in in_positions.iter().zip(backend.input_dims.iter()) {
                let mut buf: Box<[f32]> = vec![0.0_f32; dim].into_boxed_slice();
                let data_ptr = buf.as_mut_ptr().cast::<c_void>();
                let byte_len = dim * std::mem::size_of::<f32>();
                let shape = Self::concrete_batch1_shape(&model_in_shapes[pos], dim);
                let mut val: *mut OrtValue = ptr::null_mut();
                let st = Self::slot::<CreateTensorWithDataFn>(
                    api_table,
                    SLOT_CREATE_TENSOR_WITH_DATA_AS_ORT_VALUE,
                )(
                    rt.memory_info,
                    data_ptr,
                    byte_len,
                    shape.as_ptr(),
                    shape.len(),
                    ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT,
                    &mut val,
                );
                Self::check_status(api_table, st, "CreateTensorWithData(input)")?;
                backend.pinned_inputs.push(buf);
                backend.bound_in_values.push(val);
            }

            // Pre-allocate pinned output buffers and persistent OrtValue wrappers when all output dims are static.
            if !backend.output_dims.is_empty() && backend.output_dims.iter().all(|&d| d > 0) {
                backend
                    .pinned_outputs
                    .reserve_exact(backend.output_dims.len());
                backend
                    .bound_out_values
                    .reserve_exact(backend.output_dims.len());
                for (&pos, &dim) in out_positions.iter().zip(backend.output_dims.iter()) {
                    let mut buf: Box<[f32]> = vec![0.0_f32; dim].into_boxed_slice();
                    let data_ptr = buf.as_mut_ptr().cast::<c_void>();
                    let byte_len = dim * std::mem::size_of::<f32>();
                    let shape = Self::concrete_batch1_shape(&model_out_shapes[pos], dim);
                    let mut val: *mut OrtValue = ptr::null_mut();
                    let st = Self::slot::<CreateTensorWithDataFn>(
                        api_table,
                        SLOT_CREATE_TENSOR_WITH_DATA_AS_ORT_VALUE,
                    )(
                        rt.memory_info,
                        data_ptr,
                        byte_len,
                        shape.as_ptr(),
                        shape.len(),
                        ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT,
                        &mut val,
                    );
                    Self::check_status(api_table, st, "CreateTensorWithData(output)")?;
                    backend.pinned_outputs.push(buf);
                    backend.bound_out_values.push(val);
                }
            }

            Ok(backend)
        }
    }

    /// Returns true when both inputs and outputs are pre-bound to persistent `OrtValue`s (zero per-tick ORT allocs).
    pub fn has_prebound_io(&self) -> bool {
        !self.bound_in_values.is_empty()
            && self.bound_in_values.len() == self.input_dims.len()
            && !self.bound_out_values.is_empty()
            && self.bound_out_values.len() == self.output_dims.len()
    }

    /// Exposes pinned input/output backing buffer addresses for zero-reallocation verification tests.
    pub fn pinned_buffer_addrs(&self) -> (Vec<usize>, Vec<usize>) {
        let in_addrs = self
            .pinned_inputs
            .iter()
            .map(|b| b.as_ptr() as usize)
            .collect();
        let out_addrs = self
            .pinned_outputs
            .iter()
            .map(|b| b.as_ptr() as usize)
            .collect();
        (in_addrs, out_addrs)
    }

    /// Exposes persistent `OrtValue` handle addresses for zero-recreation verification tests.
    pub fn bound_value_addrs(&self) -> (Vec<usize>, Vec<usize>) {
        let in_vals = self.bound_in_values.iter().map(|&p| p as usize).collect();
        let out_vals = self.bound_out_values.iter().map(|&p| p as usize).collect();
        (in_vals, out_vals)
    }

    fn concrete_batch1_shape(raw_dims: &[i64], flat_dim: usize) -> Vec<i64> {
        match raw_dims.len() {
            1 => vec![flat_dim as i64],
            2 => vec![1, flat_dim as i64],
            n if n > 2 && raw_dims[1..].iter().all(|&d| d > 0) => {
                let mut out = Vec::with_capacity(n);
                out.push(if raw_dims[0] > 0 { raw_dims[0] } else { 1 });
                out.extend_from_slice(&raw_dims[1..]);
                out
            }
            _ => vec![1, flat_dim as i64],
        }
    }

    #[allow(clippy::type_complexity)]
    unsafe fn query_io_meta(
        api_table: *const usize,
        session: *mut OrtSession,
        is_input: bool,
    ) -> Result<(Vec<CString>, Vec<usize>, Vec<Vec<i64>>)> {
        let mut alloc: *mut OrtAllocator = ptr::null_mut();
        Self::check_status(
            api_table,
            Self::slot::<GetAllocatorFn>(api_table, SLOT_GET_ALLOCATOR_WITH_DEFAULT_OPTIONS)(
                &mut alloc,
            ),
            "GetAllocatorWithDefaultOptions",
        )?;
        let count_slot = if is_input {
            SLOT_SESSION_GET_INPUT_COUNT
        } else {
            SLOT_SESSION_GET_OUTPUT_COUNT
        };
        let name_slot = if is_input {
            SLOT_SESSION_GET_INPUT_NAME
        } else {
            SLOT_SESSION_GET_OUTPUT_NAME
        };
        let type_slot = if is_input {
            SLOT_SESSION_GET_INPUT_TYPE_INFO
        } else {
            SLOT_SESSION_GET_OUTPUT_TYPE_INFO
        };

        let mut count = 0usize;
        Self::check_status(
            api_table,
            Self::slot::<SessionGetCountFn>(api_table, count_slot)(session, &mut count),
            "SessionGetIOCount",
        )?;

        let mut names = Vec::with_capacity(count);
        let mut flat_dims = Vec::with_capacity(count);
        let mut shapes = Vec::with_capacity(count);
        for i in 0..count {
            let mut raw_name: *mut c_char = ptr::null_mut();
            Self::check_status(
                api_table,
                Self::slot::<SessionGetNameFn>(api_table, name_slot)(
                    session,
                    i,
                    alloc,
                    &mut raw_name,
                ),
                "SessionGetIOName",
            )?;
            let cstr = CStr::from_ptr(raw_name).to_owned();
            Self::release_if_non_null(
                api_table,
                Self::slot::<AllocatorFreeFn>(api_table, SLOT_ALLOCATOR_FREE)(
                    alloc,
                    raw_name.cast(),
                ),
            );
            names.push(cstr);

            let mut tinfo: *mut OrtTypeInfo = ptr::null_mut();
            Self::check_status(
                api_table,
                Self::slot::<SessionGetTypeInfoFn>(api_table, type_slot)(session, i, &mut tinfo),
                "SessionGetTypeInfo",
            )?;
            let mut sinfo: *const OrtTensorTypeAndShapeInfo = ptr::null();
            Self::check_status(
                api_table,
                Self::slot::<CastTypeInfoFn>(api_table, SLOT_CAST_TYPE_INFO_TO_TENSOR_INFO)(
                    tinfo, &mut sinfo,
                ),
                "CastTypeInfoToTensorInfo",
            )?;
            let mut ndims = 0usize;
            Self::check_status(
                api_table,
                Self::slot::<GetDimensionsCountFn>(api_table, SLOT_GET_DIMENSIONS_COUNT)(
                    sinfo, &mut ndims,
                ),
                "GetDimensionsCount",
            )?;
            let mut dims = vec![0i64; ndims];
            if ndims > 0 {
                Self::check_status(
                    api_table,
                    Self::slot::<GetDimensionsFn>(api_table, SLOT_GET_DIMENSIONS)(
                        sinfo,
                        dims.as_mut_ptr(),
                        ndims,
                    ),
                    "GetDimensions",
                )?;
            }
            Self::slot::<ReleasePtrFn>(api_table, SLOT_RELEASE_TYPE_INFO)(tinfo.cast());

            let elem_dim = match dims.as_slice() {
                [d] if *d > 0 => *d as usize,
                [b, d] if (*b == 1 || *b <= 0) && *d > 0 => *d as usize,
                [b, rest @ ..]
                    if !rest.is_empty() && (*b == 1 || *b <= 0) && rest.iter().all(|&x| x > 0) =>
                {
                    rest.iter()
                        .fold(1usize, |acc, &x| acc.saturating_mul(x as usize))
                }
                all if !all.is_empty() && all.iter().all(|&x| x > 0) => all
                    .iter()
                    .fold(1usize, |acc, &x| acc.saturating_mul(x as usize)),
                _ => 0,
            };
            flat_dims.push(elem_dim);
            shapes.push(dims);
        }
        Ok((names, flat_dims, shapes))
    }

    unsafe fn query_value_elem_count(api_table: *const usize, val: *mut OrtValue) -> Option<usize> {
        let mut sinfo: *mut OrtTensorTypeAndShapeInfo = ptr::null_mut();
        let st = Self::slot::<GetTensorTypeAndShapeFn>(api_table, SLOT_GET_TENSOR_TYPE_AND_SHAPE)(
            val, &mut sinfo,
        );
        if !st.is_null() {
            Self::slot::<ReleaseStatusFn>(api_table, SLOT_RELEASE_STATUS)(st);
            return None;
        }
        let mut count = 0usize;
        let st2 = Self::slot::<GetTensorShapeElementCountFn>(
            api_table,
            SLOT_GET_TENSOR_SHAPE_ELEMENT_COUNT,
        )(sinfo, &mut count);
        Self::slot::<ReleasePtrFn>(api_table, SLOT_RELEASE_TENSOR_TYPE_AND_SHAPE_INFO)(
            sinfo.cast(),
        );
        if !st2.is_null() {
            Self::slot::<ReleaseStatusFn>(api_table, SLOT_RELEASE_STATUS)(st2);
            return None;
        }
        Some(count)
    }

    fn open_ort_library() -> Result<Library> {
        let mut candidates = Vec::new();
        if let Ok(p) = std::env::var("ONNXRUNTIME_DLL_PATH") {
            if !p.trim().is_empty() {
                candidates.push(PathBuf::from(p));
            }
        }
        if let Ok(p) = std::env::var("ORT_DYLIB_PATH") {
            if !p.trim().is_empty() {
                candidates.push(PathBuf::from(p));
            }
        }
        #[cfg(target_os = "linux")]
        {
            candidates.push(PathBuf::from(
                "/usr/lib/aarch64-linux-gnu/libonnxruntime.so",
            ));
            candidates.push(PathBuf::from("/usr/local/lib/libonnxruntime.so"));
            candidates.push(PathBuf::from("libonnxruntime.so"));
        }
        #[cfg(target_os = "windows")]
        {
            candidates.push(PathBuf::from(
                r"C:\Users\99563\miniconda3\Lib\site-packages\onnxruntime\capi\onnxruntime.dll",
            ));
            candidates.push(PathBuf::from("onnxruntime.dll"));
        }
        for c in &candidates {
            if Path::new(c).exists() || c.components().count() == 1 {
                if let Ok(lib) = unsafe { Library::new(c) } {
                    return Ok(lib);
                }
            }
        }
        bail!(
            "Could not open ONNX Runtime shared library (tried {:?}); set ONNXRUNTIME_DLL_PATH",
            candidates
        )
    }

    #[inline]
    unsafe fn slot<F: Copy>(api_table: *const usize, idx: usize) -> F {
        let addr = *api_table.add(idx);
        std::mem::transmute_copy(&addr)
    }

    #[inline]
    unsafe fn release_if_non_null(api_table: *const usize, status: *mut OrtStatus) {
        if !status.is_null() {
            Self::slot::<ReleaseStatusFn>(api_table, SLOT_RELEASE_STATUS)(status);
        }
    }

    unsafe fn check_status(
        api_table: *const usize,
        status: *mut OrtStatus,
        op: &str,
    ) -> Result<()> {
        if status.is_null() {
            return Ok(());
        }
        let msg_ptr = Self::slot::<GetErrorMessageFn>(api_table, SLOT_GET_ERROR_MESSAGE)(status);
        let msg = if msg_ptr.is_null() {
            "unknown ORT error".to_string()
        } else {
            CStr::from_ptr(msg_ptr).to_string_lossy().into_owned()
        };
        Self::slot::<ReleaseStatusFn>(api_table, SLOT_RELEASE_STATUS)(status);
        bail!("ORT {op} failed: {msg}")
    }
}

impl Drop for OrtDynamicMultiBackend {
    fn drop(&mut self) {
        let api_table = self.rt.api_table;
        unsafe {
            // Release all pre-bound input & output OrtValues BEFORE releasing OrtSession!
            for val in self.bound_in_values.drain(..) {
                if !val.is_null() {
                    Self::slot::<ReleasePtrFn>(api_table, SLOT_RELEASE_VALUE)(val.cast());
                }
            }
            for val in self.bound_out_values.drain(..) {
                if !val.is_null() {
                    Self::slot::<ReleasePtrFn>(api_table, SLOT_RELEASE_VALUE)(val.cast());
                }
            }
            if !self.session.is_null() {
                Self::slot::<ReleasePtrFn>(api_table, SLOT_RELEASE_SESSION)(self.session.cast());
                self.session = ptr::null_mut();
            }
        }
    }
}

impl InferenceBackend for OrtDynamicMultiBackend {
    fn backend_name(&self) -> &'static str {
        "ort-capi-dynamic"
    }

    fn infer_into(&mut self, inputs: &[(&str, &[f32])], outputs: &mut [Vec<f32>]) -> Result<()> {
        let api_table = self.rt.api_table;
        if inputs.len() != self.input_dims.len() {
            bail!(
                "Expected {} inputs, got {}",
                self.input_dims.len(),
                inputs.len()
            );
        }
        if outputs.len() < self.output_dims.len() {
            bail!(
                "Expected >= {} output buffers, got {}",
                self.output_dims.len(),
                outputs.len()
            );
        }

        for (i, (_name, data)) in inputs.iter().enumerate() {
            if data.len() != self.input_dims[i] {
                bail!(
                    "Input {} length mismatch: expected {}, got {}",
                    i,
                    self.input_dims[i],
                    data.len()
                );
            }
            self.pinned_inputs[i].copy_from_slice(data);
        }

        unsafe {
            // Zero-alloc hot path: pre-bound input & output OrtValues backed by pinned Box<[f32]>.
            if !self.bound_out_values.is_empty() {
                let run_st = Self::slot::<RunFn>(api_table, SLOT_RUN)(
                    self.session,
                    ptr::null(),
                    self.input_name_ptrs.as_ptr(),
                    self.bound_in_values.as_ptr().cast(),
                    self.bound_in_values.len(),
                    self.output_name_ptrs.as_ptr(),
                    self.output_name_ptrs.len(),
                    self.bound_out_values.as_mut_ptr(),
                );
                Self::check_status(api_table, run_st, "Run")?;

                for (o, pinned_out) in self.pinned_outputs.iter().enumerate() {
                    let dim = pinned_out.len();
                    if outputs[o].len() != dim {
                        outputs[o].resize(dim, 0.0);
                    }
                    outputs[o].copy_from_slice(pinned_out);
                }
                return Ok(());
            }

            // Dynamic fallback path (only when any output_dims[o] == 0).
            let out_count = self.output_name_ptrs.len();
            let mut dyn_out_values: Vec<*mut OrtValue> = vec![ptr::null_mut(); out_count];
            let run_st = Self::slot::<RunFn>(api_table, SLOT_RUN)(
                self.session,
                ptr::null(),
                self.input_name_ptrs.as_ptr(),
                self.bound_in_values.as_ptr().cast(),
                self.bound_in_values.len(),
                self.output_name_ptrs.as_ptr(),
                out_count,
                dyn_out_values.as_mut_ptr(),
            );
            if let Err(e) = Self::check_status(api_table, run_st, "Run") {
                for val in dyn_out_values {
                    if !val.is_null() {
                        Self::slot::<ReleasePtrFn>(api_table, SLOT_RELEASE_VALUE)(val.cast());
                    }
                }
                return Err(e);
            }

            for (o, out_val) in dyn_out_values.into_iter().enumerate() {
                let mut raw_ptr: *mut c_void = ptr::null_mut();
                let st = Self::slot::<GetTensorMutableDataFn>(
                    api_table,
                    SLOT_GET_TENSOR_MUTABLE_DATA,
                )(out_val, &mut raw_ptr);
                if st.is_null() && !raw_ptr.is_null() {
                    let mut dim = self.output_dims[o];
                    if dim == 0 {
                        dim = Self::query_value_elem_count(api_table, out_val).unwrap_or(0);
                    }
                    if dim > 0 {
                        let slice = std::slice::from_raw_parts(raw_ptr.cast::<f32>(), dim);
                        outputs[o].resize(dim, 0.0);
                        outputs[o].copy_from_slice(slice);
                    }
                } else if !st.is_null() {
                    Self::slot::<ReleaseStatusFn>(api_table, SLOT_RELEASE_STATUS)(st);
                }
                Self::slot::<ReleasePtrFn>(api_table, SLOT_RELEASE_VALUE)(out_val.cast());
            }
        }
        Ok(())
    }
}
