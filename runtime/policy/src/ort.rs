use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Context, Result};
use libloading::Library;

use crate::blocks::PolicyManifest;

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

// ─── Dynamic ONNX Runtime C-API Backend (multi-IO, prebound zero-alloc hot path) ────

const MAX_IO_TENSORS: usize = 4;

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
const SLOT_RELEASE_STATUS: usize = 93;
const SLOT_RELEASE_SESSION: usize = 95;
const SLOT_RELEASE_VALUE: usize = 96;
const SLOT_RELEASE_TYPE_INFO: usize = 98;
const SLOT_RELEASE_SESSION_OPTIONS: usize = 100;

type GetErrorMessageFn = unsafe extern "system" fn(*const OrtStatus) -> *const c_char;
type ReleaseStatusFn = unsafe extern "system" fn(*mut OrtStatus);
type CreateEnvFn =
    unsafe extern "system" fn(u32, *const c_char, *mut *mut OrtEnv) -> *mut OrtStatus;
type CreateSessionOptionsFn =
    unsafe extern "system" fn(*mut *mut OrtSessionOptions) -> *mut OrtStatus;
type SetOptionU32Fn = unsafe extern "system" fn(*mut OrtSessionOptions, u32) -> *mut OrtStatus;
type EnableOptionFn = unsafe extern "system" fn(*mut OrtSessionOptions) -> *mut OrtStatus;
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
type GetDimensionsFn = unsafe extern "system" fn(
    *const OrtTensorTypeAndShapeInfo,
    *mut i64,
    usize,
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
type IoMeta = (Vec<CString>, Vec<usize>, Vec<Vec<i64>>);

/// Process-wide singleton holding the loaded ORT dynamic library, global `OrtEnv`, and CPU `OrtMemoryInfo`.
/// Ensures zero reload overhead and safe concurrent/hot-deploy session creation & destruction!
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
            let log_id = CString::new("doso-policy")?;
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
    // Pre-allocated, fixed-address backing buffers bound once to OrtValue handles so
    // 50Hz–200Hz real-time inference runs with zero heap allocations and zero OrtValue churn.
    input_bufs: Vec<Box<[f32]>>,
    input_vals: Vec<*mut OrtValue>,
    _output_names: Vec<CString>,
    output_name_ptrs: Vec<*const c_char>,
    output_dims: Vec<usize>,
    output_bufs: Vec<Box<[f32]>>,
    output_vals: Vec<*mut OrtValue>,
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

            // Deterministic real-time single-thread execution with full graph fusion & arena reuse.
            Self::ignore_status(
                api_table,
                Self::slot::<SetOptionU32Fn>(api_table, SLOT_SET_SESSION_EXECUTION_MODE)(
                    opts,
                    ORT_SEQUENTIAL,
                ),
            );
            Self::ignore_status(
                api_table,
                Self::slot::<EnableOptionFn>(api_table, SLOT_ENABLE_MEM_PATTERN)(opts),
            );
            Self::ignore_status(
                api_table,
                Self::slot::<EnableOptionFn>(api_table, SLOT_ENABLE_CPU_MEM_ARENA)(opts),
            );
            Self::ignore_status(
                api_table,
                Self::slot::<SetOptionU32Fn>(api_table, SLOT_SET_SESSION_GRAPH_OPTIMIZATION_LEVEL)(
                    opts,
                    ORT_ENABLE_ALL,
                ),
            );
            Self::ignore_status(
                api_table,
                Self::slot::<SetThreadsFn>(api_table, SLOT_SET_INTRA_OP_NUM_THREADS)(opts, 1),
            );
            Self::ignore_status(
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

            let io_res = (|| -> Result<Self> {
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
                if manifest.inputs.len() > MAX_IO_TENSORS {
                    bail!(
                        "Policy '{}' declares {} inputs (max supported: {})",
                        manifest.name,
                        manifest.inputs.len(),
                        MAX_IO_TENSORS
                    );
                }

                let mut input_names = Vec::with_capacity(manifest.inputs.len());
                let mut input_dims = Vec::with_capacity(manifest.inputs.len());
                let mut input_shapes = Vec::with_capacity(manifest.inputs.len());
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
                    let shape = if model_in_shapes[pos].len() == 1 {
                        vec![expected_dim as i64]
                    } else {
                        vec![1i64, expected_dim as i64]
                    };
                    input_names.push(model_in_names[pos].clone());
                    input_dims.push(expected_dim);
                    input_shapes.push(shape);
                }

                let mut wanted_outputs = vec![manifest.outputs.action_tensor.clone()];
                wanted_outputs.extend(manifest.outputs.aux_tensors.iter().cloned());
                if wanted_outputs.len() > MAX_IO_TENSORS {
                    bail!(
                        "Policy '{}' declares {} outputs (max supported: {})",
                        manifest.name,
                        wanted_outputs.len(),
                        MAX_IO_TENSORS
                    );
                }
                let mut output_names = Vec::with_capacity(wanted_outputs.len());
                let mut output_dims = Vec::with_capacity(wanted_outputs.len());
                let mut output_shapes = Vec::with_capacity(wanted_outputs.len());
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
                    output_shapes.push(model_out_shapes[pos].clone());
                }

                let input_name_ptrs = input_names.iter().map(|s| s.as_ptr()).collect();
                let output_name_ptrs = output_names.iter().map(|s| s.as_ptr()).collect();

                let mut backend = Self {
                    rt,
                    session,
                    _input_names: input_names,
                    input_name_ptrs,
                    input_dims,
                    input_bufs: Vec::new(),
                    input_vals: Vec::new(),
                    _output_names: output_names,
                    output_name_ptrs,
                    output_dims,
                    output_bufs: Vec::new(),
                    output_vals: Vec::new(),
                };
                backend.init_prebound_tensors(&input_shapes, &output_shapes)?;
                Ok(backend)
            })();

            if io_res.is_err() && !session.is_null() {
                Self::slot::<ReleasePtrFn>(api_table, SLOT_RELEASE_SESSION)(session.cast());
            }
            io_res
        }
    }

    unsafe fn init_prebound_tensors(
        &mut self,
        input_shapes: &[Vec<i64>],
        output_shapes: &[Vec<i64>],
    ) -> Result<()> {
        let api_table = self.rt.api_table;
        let create_tensor = Self::slot::<CreateTensorWithDataFn>(
            api_table,
            SLOT_CREATE_TENSOR_WITH_DATA_AS_ORT_VALUE,
        );

        let mut input_bufs = Vec::with_capacity(self.input_dims.len());
        let mut input_vals: Vec<*mut OrtValue> = Vec::with_capacity(self.input_dims.len());
        for (&dim, shape) in self.input_dims.iter().zip(input_shapes.iter()) {
            let mut buf = vec![0.0_f32; dim].into_boxed_slice();
            let mut val: *mut OrtValue = ptr::null_mut();
            let st = create_tensor(
                self.rt.memory_info,
                buf.as_mut_ptr().cast::<c_void>(),
                std::mem::size_of_val(&*buf),
                shape.as_ptr(),
                shape.len(),
                ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT,
                &mut val,
            );
            if let Err(e) = Self::check_status(api_table, st, "CreatePreboundInputTensor") {
                for prev in input_vals {
                    if !prev.is_null() {
                        Self::slot::<ReleasePtrFn>(api_table, SLOT_RELEASE_VALUE)(prev.cast());
                    }
                }
                return Err(e);
            }
            input_bufs.push(buf);
            input_vals.push(val);
        }

        let mut output_bufs = Vec::with_capacity(self.output_dims.len());
        let mut output_vals: Vec<*mut OrtValue> = Vec::with_capacity(self.output_dims.len());
        for (&dim, shape) in self.output_dims.iter().zip(output_shapes.iter()) {
            let shape_elems: usize = shape
                .iter()
                .try_fold(1usize, |acc, &d| usize::try_from(d).ok()?.checked_mul(acc))
                .unwrap_or(0);
            if dim > 0 && !shape.is_empty() && shape_elems == dim {
                let mut buf = vec![0.0_f32; dim].into_boxed_slice();
                let mut val: *mut OrtValue = ptr::null_mut();
                let st = create_tensor(
                    self.rt.memory_info,
                    buf.as_mut_ptr().cast::<c_void>(),
                    std::mem::size_of_val(&*buf),
                    shape.as_ptr(),
                    shape.len(),
                    ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT,
                    &mut val,
                );
                if let Err(e) = Self::check_status(api_table, st, "CreatePreboundOutputTensor") {
                    for prev in input_vals.into_iter().chain(output_vals.into_iter()) {
                        if !prev.is_null() {
                            Self::slot::<ReleasePtrFn>(api_table, SLOT_RELEASE_VALUE)(prev.cast());
                        }
                    }
                    return Err(e);
                }
                output_bufs.push(buf);
                output_vals.push(val);
            } else {
                // Unknown dynamic output shape: let ORT allocate output on Run and read via fallback.
                output_bufs.push(Box::new([]));
                output_vals.push(ptr::null_mut());
            }
        }

        self.input_bufs = input_bufs;
        self.input_vals = input_vals;
        self.output_bufs = output_bufs;
        self.output_vals = output_vals;
        Ok(())
    }

    unsafe fn query_io_meta(
        api_table: *const usize,
        session: *mut OrtSession,
        is_input: bool,
    ) -> Result<IoMeta> {
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
            let free_st = Self::slot::<AllocatorFreeFn>(api_table, SLOT_ALLOCATOR_FREE)(
                alloc,
                raw_name.cast(),
            );
            Self::ignore_status(api_table, free_st);
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

            let feature_dim = dims.last().copied().unwrap_or(0);
            let resolved_dim = if feature_dim > 0 {
                feature_dim as usize
            } else {
                0
            };
            // Normalize dynamic leading batch dim (<= 0) to 1 for prebound single-batch inference.
            let resolved_shape: Vec<i64> = dims
                .iter()
                .enumerate()
                .map(|(axis, &d)| {
                    if d > 0 {
                        d
                    } else if axis == 0 && dims.len() >= 2 {
                        1
                    } else {
                        0
                    }
                })
                .collect();
            flat_dims.push(resolved_dim);
            shapes.push(resolved_shape);
        }
        Ok((names, flat_dims, shapes))
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
            candidates.push(PathBuf::from("/usr/lib/aarch64-linux-gnu/libonnxruntime.so"));
            candidates.push(PathBuf::from("/usr/local/lib/libonnxruntime.so"));
            candidates.push(PathBuf::from("/opt/doso/current/lib/libonnxruntime.so"));
            candidates.push(PathBuf::from("libonnxruntime.so"));
        }
        #[cfg(target_os = "windows")]
        {
            candidates.push(PathBuf::from("onnxruntime.dll"));
        }
        #[cfg(target_os = "macos")]
        {
            candidates.push(PathBuf::from("/usr/local/lib/libonnxruntime.dylib"));
            candidates.push(PathBuf::from("libonnxruntime.dylib"));
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
    unsafe fn ignore_status(api_table: *const usize, status: *mut OrtStatus) {
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
        unsafe {
            let release_val = Self::slot::<ReleasePtrFn>(self.rt.api_table, SLOT_RELEASE_VALUE);
            for val in self.input_vals.drain(..) {
                if !val.is_null() {
                    release_val(val.cast());
                }
            }
            for val in self.output_vals.drain(..) {
                if !val.is_null() {
                    release_val(val.cast());
                }
            }
            if !self.session.is_null() {
                Self::slot::<ReleasePtrFn>(self.rt.api_table, SLOT_RELEASE_SESSION)(
                    self.session.cast(),
                );
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
        let n_in = self.input_vals.len();
        let n_out = self.output_vals.len();
        if inputs.len() != n_in {
            bail!("Expected {n_in} inputs, got {}", inputs.len());
        }
        if outputs.len() < n_out {
            bail!("Expected >= {n_out} output buffers, got {}", outputs.len());
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
            self.input_bufs[i].copy_from_slice(data);
        }

        let mut in_ptrs: [*const OrtValue; MAX_IO_TENSORS] = [ptr::null(); MAX_IO_TENSORS];
        let mut out_ptrs: [*mut OrtValue; MAX_IO_TENSORS] = [ptr::null_mut(); MAX_IO_TENSORS];
        for (slot, &val) in in_ptrs.iter_mut().zip(self.input_vals.iter()) {
            *slot = val.cast_const();
        }
        out_ptrs[..n_out].copy_from_slice(&self.output_vals[..n_out]);

        unsafe {
            let run_st = Self::slot::<RunFn>(api_table, SLOT_RUN)(
                self.session,
                ptr::null(),
                self.input_name_ptrs.as_ptr(),
                in_ptrs.as_ptr(),
                n_in,
                self.output_name_ptrs.as_ptr(),
                n_out,
                out_ptrs.as_mut_ptr(),
            );

            if let Err(e) = Self::check_status(api_table, run_st, "Run") {
                for (o, &val) in out_ptrs.iter().take(n_out).enumerate() {
                    if !val.is_null() && val != self.output_vals[o] {
                        Self::slot::<ReleasePtrFn>(api_table, SLOT_RELEASE_VALUE)(val.cast());
                    }
                }
                return Err(e);
            }

            for (o, &out_val) in out_ptrs.iter().take(n_out).enumerate() {
                let dim = self.output_dims[o];
                if !self.output_vals[o].is_null() && out_val == self.output_vals[o] {
                    // Fast path: ORT wrote directly into our prebound output_buf (zero alloc).
                    if dim > 0 {
                        outputs[o].resize(dim, 0.0);
                        outputs[o].copy_from_slice(&self.output_bufs[o]);
                    }
                } else {
                    // Fallback: ORT allocated/replaced the output tensor; copy and release it.
                    let mut raw_ptr: *mut c_void = ptr::null_mut();
                    let st = Self::slot::<GetTensorMutableDataFn>(
                        api_table,
                        SLOT_GET_TENSOR_MUTABLE_DATA,
                    )(out_val, &mut raw_ptr);
                    if st.is_null() && !raw_ptr.is_null() {
                        if dim > 0 {
                            let slice = std::slice::from_raw_parts(raw_ptr.cast::<f32>(), dim);
                            outputs[o].resize(dim, 0.0);
                            outputs[o].copy_from_slice(slice);
                        }
                    } else {
                        Self::ignore_status(api_table, st);
                    }
                    if !out_val.is_null() {
                        Self::slot::<ReleasePtrFn>(api_table, SLOT_RELEASE_VALUE)(out_val.cast());
                    }
                }
            }
        }
        Ok(())
    }
}
