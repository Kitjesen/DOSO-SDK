use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, HeaderName, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use base64::Engine as _;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::Ed25519KeyPair;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::policy::{LoadedPolicyPackage, PolicyManifest, ShadowEvaluator};
use crate::server::grpc::SharedBrain;

pub const DEFAULT_BOOT_ID_MARKER_PATH: &str = "/run/doso/brainstem.boot_id";
pub const SLOT_BINDINGS_FILE: &str = ".doso_slots.json";

#[derive(Debug, Clone)]
pub struct HotDeployConfig {
    /// Directory where hot-pushed policy packages are atomically persisted across restarts.
    pub packages_dir: Option<PathBuf>,
    /// Optional Bearer token guarding `/v1/policies*` endpoints (`None`/empty = open dev/benchtop).
    pub admin_token: Option<String>,
    /// Unique per-boot identifier returned in `GET /health` and written to `/run/doso/brainstem.boot_id`.
    pub boot_id: String,
    /// Firmware/runtime version string (e.g. `v0.0.1`).
    pub version: String,
    /// Release identifier (defaults to `version`).
    pub release_id: String,
    /// Optional PKCS#8 Ed25519 PEM signing key path (`DOSO_HEALTH_SIGNING_KEY`) for signed `/health`.
    pub health_signing_key_path: Option<PathBuf>,
}

impl Default for HotDeployConfig {
    fn default() -> Self {
        Self {
            packages_dir: None,
            admin_token: None,
            boot_id: generate_boot_id(),
            version: "v0.0.1".to_string(),
            release_id: "v0.0.1".to_string(),
            health_signing_key_path: None,
        }
    }
}

#[derive(Clone)]
struct HotDeployState {
    shared: Arc<SharedBrain>,
    cfg: Arc<HotDeployConfig>,
    signing_key: Option<Arc<Ed25519KeyPair>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PushPolicyRequest {
    pub manifest_yaml: String,
    pub onnx_base64: String,
    #[serde(default)]
    pub golden_json: Option<String>,
    #[serde(default)]
    pub slot: Option<String>,
    #[serde(default = "default_shadow_frames")]
    pub shadow_frames: usize,
    #[serde(default)]
    pub activate: bool,
}

fn default_shadow_frames() -> usize {
    50
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RollbackRequest {
    pub slot: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PersistedSlotBindings {
    #[serde(default)]
    pub slots: BTreeMap<String, String>,
    #[serde(default)]
    pub active_profile: Option<String>,
}

#[derive(Debug, Serialize)]
struct HealthCorePayload {
    ok: bool,
    boot_id: String,
    version: String,
    release_id: String,
    brainstem_ready: bool,
    strategy_ready: bool,
    can_read_only: bool,
    active_profile: String,
    imu_online: bool,
    online_joints: u8,
    uptime_s: u64,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    #[serde(flatten)]
    core: HealthCorePayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    algorithm: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
}

#[derive(Serialize)]
struct ErrorResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stage: Option<&'static str>,
    error: String,
}

#[derive(Serialize)]
struct PolicySummaryJson {
    name: String,
    sha256: String,
    warmup_us: u64,
    golden_steps: usize,
    golden_max_err: f64,
}

#[derive(Serialize)]
struct RtLoopMetricsJson {
    max_tick_us: u64,
    max_jitter_us: u64,
    max_mutex_wait_us: u64,
    ticks: u64,
}

#[derive(Serialize)]
struct ListPoliciesResponse {
    ok: bool,
    active_profile: String,
    slots: BTreeMap<String, String>,
    policies: Vec<PolicySummaryJson>,
    rt_loop: RtLoopMetricsJson,
}

#[derive(Serialize)]
struct ShadowEvalJson {
    steps: usize,
    p50_us: u64,
    p99_us: u64,
    max_us: u64,
    clipped_joints: usize,
    max_leg_delta_rad: f64,
}

#[derive(Serialize)]
struct PushPolicyResponse {
    ok: bool,
    elapsed_ms: f64,
    name: String,
    sha256: String,
    warmup_us: u64,
    golden_steps: usize,
    golden_max_err: f64,
    shadow: Option<ShadowEvalJson>,
    bound_slot: Option<String>,
    active_profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    persisted_dir: Option<String>,
}

#[derive(Serialize)]
struct RollbackResponse {
    ok: bool,
    slot: String,
    restored_policy: String,
    active_profile: String,
}

/// Resolves `(version, release_id)` from sibling `release.json` or `metadata.json`
/// in the deployed release root (e.g. `/opt/doso/current/release.json`) when `explicit_release_id`
/// is unset, falling back to `fallback_version` (e.g. `DOSO_FIRMWARE_VERSION`).
pub fn resolve_release_metadata(
    exe_path: Option<&Path>,
    fallback_version: &str,
    explicit_release_id: Option<&str>,
) -> (String, String) {
    let explicit_rel = explicit_release_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    if let Some(rel) = explicit_rel {
        return (fallback_version.to_string(), rel);
    }

    if let Some(exe) = exe_path {
        let candidates = [
            exe.parent()
                .and_then(|p| p.parent())
                .map(|root| root.join("release.json")),
            exe.parent()
                .and_then(|p| p.parent())
                .map(|root| root.join("metadata.json")),
            exe.parent().map(|dir| dir.join("release.json")),
            exe.parent().map(|dir| dir.join("metadata.json")),
        ];
        for cand in candidates.into_iter().flatten() {
            if let Ok(raw) = std::fs::read_to_string(&cand) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                    let ver = val
                        .get("version")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or(fallback_version)
                        .to_string();
                    let rel = val
                        .get("release_id")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&ver)
                        .to_string();
                    return (ver, rel);
                }
            }
        }
    }

    (fallback_version.to_string(), fallback_version.to_string())
}

/// Generates a random UUIDv4-formatted per-boot identifier.
pub fn generate_boot_id() -> String {
    let mut b = [0u8; 16];
    let rng = SystemRandom::new();
    if rng.fill(&mut b).is_err() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id() as u128;
        b = (nanos ^ (pid << 64)).to_le_bytes();
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

/// Best-effort atomic write of `/run/doso/brainstem.boot_id` when the parent directory exists and is writable.
pub fn write_boot_id_marker(path: &Path, boot_id: &str) {
    let Some(parent) = path.parent() else {
        return;
    };
    if !parent.is_dir() {
        return;
    }
    let tmp = parent.join(format!(".brainstem.boot_id.tmp.{}", std::process::id()));
    let payload = format!("{boot_id}\n");
    if let Err(err) =
        std::fs::write(&tmp, payload.as_bytes()).and_then(|()| std::fs::rename(&tmp, path))
    {
        let _ = std::fs::remove_file(&tmp);
        tracing::debug!(path = %path.display(), error = %err, "Skipped boot_id marker write");
    }
}

fn load_ed25519_signing_key(path: &Path) -> Result<Ed25519KeyPair> {
    let pem = std::fs::read_to_string(path)
        .with_context(|| format!("failed reading health signing key {}", path.display()))?;
    let der_b64: String = pem
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("-----"))
        .collect();
    let der = B64
        .decode(der_b64.as_bytes())
        .context("failed decoding PEM base64 in health signing key")?;
    Ed25519KeyPair::from_pkcs8_maybe_unchecked(&der)
        .map_err(|e| anyhow::anyhow!("invalid Ed25519 PKCS#8 health signing key: {e}"))
}

pub fn build_hot_deploy_router(shared: Arc<SharedBrain>) -> Router {
    build_hot_deploy_router_with_config(shared, HotDeployConfig::default())
}

pub fn build_hot_deploy_router_with_config(
    shared: Arc<SharedBrain>,
    mut cfg: HotDeployConfig,
) -> Router {
    cfg.admin_token = cfg
        .admin_token
        .take()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let signing_key = cfg.health_signing_key_path.as_ref().and_then(|path| {
        match load_ed25519_signing_key(path) {
            Ok(kp) => Some(Arc::new(kp)),
            Err(err) => {
                warn!(path = %path.display(), error = %err, "Failed loading DOSO_HEALTH_SIGNING_KEY; serving unsigned /health");
                None
            }
        }
    });

    let state = HotDeployState {
        shared,
        cfg: Arc::new(cfg),
        signing_key,
    };

    Router::new()
        .route("/health", get(handle_health))
        .route("/v1/policies", get(handle_list_policies))
        .route("/v1/policies/push", post(handle_push_policy))
        .route("/v1/policies/rollback", post(handle_rollback_policy))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state)
}

fn json_resp<T: Serialize>(
    code: StatusCode,
    payload: &T,
) -> (StatusCode, [(HeaderName, &'static str); 1], String) {
    let body = serde_json::to_string(payload)
        .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"json encode failed: {e}\"}}"));
    (code, [(header::CONTENT_TYPE, "application/json")], body)
}

fn err_resp(
    code: StatusCode,
    stage: Option<&'static str>,
    error: String,
) -> (StatusCode, [(HeaderName, &'static str); 1], String) {
    json_resp(
        code,
        &ErrorResponse {
            ok: false,
            stage,
            error,
        },
    )
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn check_admin_auth(headers: &HeaderMap, cfg: &HotDeployConfig) -> Option<impl IntoResponse> {
    let expected = cfg.admin_token.as_deref()?;
    let provided = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .map(str::trim);
    match provided {
        Some(tok) if constant_time_eq(tok.as_bytes(), expected.as_bytes()) => None,
        _ => Some(err_resp(
            StatusCode::UNAUTHORIZED,
            None,
            "unauthorized".to_string(),
        )),
    }
}

fn sanitize_policy_dir_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('.')
        || trimmed.contains("..")
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        bail!("invalid policy package name '{name}' for disk persistence");
    }
    Ok(trimmed.to_string())
}

/// Rewrites `model.file` inside a pushed `policy.yaml` to `"model.onnx"` so the persisted package
/// directory is 100% self-contained across service restarts.
fn normalize_persisted_manifest_yaml(raw_yaml: &str) -> Result<String> {
    let mut doc: serde_yaml::Value =
        serde_yaml::from_str(raw_yaml).context("failed parsing manifest YAML for persistence")?;
    if let Some(model_map) = doc
        .as_mapping_mut()
        .and_then(|m| m.get_mut(serde_yaml::Value::String("model".into())))
        .and_then(|v| v.as_mapping_mut())
    {
        model_map.insert(
            serde_yaml::Value::String("file".into()),
            serde_yaml::Value::String("model.onnx".into()),
        );
    }
    let normalized = serde_yaml::to_string(&doc)
        .context("failed serializing normalized policy.yaml for persistence")?;
    let _ = PolicyManifest::from_yaml_str(&normalized)
        .context("normalized policy.yaml failed schema validation")?;
    Ok(normalized)
}

/// Atomically persists `(policy.yaml, model.onnx, golden.json)` into `<packages_dir>/<name>/`
/// via staging `.tmp` + rename, preserving the prior directory as `.<name>.prev`.
fn persist_policy_package_atomic(
    packages_dir: &Path,
    policy_name: &str,
    manifest_yaml: &str,
    onnx_bytes: &[u8],
    golden_json: Option<&str>,
) -> Result<PathBuf> {
    let safe_name = sanitize_policy_dir_name(policy_name)?;
    let normalized_yaml = normalize_persisted_manifest_yaml(manifest_yaml)?;
    std::fs::create_dir_all(packages_dir)
        .with_context(|| format!("failed creating packages_dir {}", packages_dir.display()))?;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_dir = packages_dir.join(format!(".{safe_name}.tmp.{}.{nonce}", std::process::id()));
    let target_dir = packages_dir.join(&safe_name);
    let prev_dir = packages_dir.join(format!(".{safe_name}.prev"));

    let write_res = (|| -> Result<()> {
        std::fs::create_dir_all(&tmp_dir)?;
        std::fs::write(tmp_dir.join("policy.yaml"), normalized_yaml.as_bytes())?;
        std::fs::write(tmp_dir.join("model.onnx"), onnx_bytes)?;
        if let Some(golden) = golden_json {
            std::fs::write(tmp_dir.join("golden.json"), golden.as_bytes())?;
        }
        Ok(())
    })();
    if let Err(err) = write_res {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(err).context("failed staging hot-pushed policy package");
    }

    let mut backed_up = false;
    if target_dir.exists() {
        if prev_dir.exists() {
            let _ = std::fs::remove_dir_all(&prev_dir);
        }
        std::fs::rename(&target_dir, &prev_dir).with_context(|| {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            format!(
                "failed moving existing package {} -> {}",
                target_dir.display(),
                prev_dir.display()
            )
        })?;
        backed_up = true;
    }

    if let Err(err) = std::fs::rename(&tmp_dir, &target_dir) {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        if backed_up && !target_dir.exists() && prev_dir.exists() {
            let _ = std::fs::rename(&prev_dir, &target_dir);
        }
        return Err(err).with_context(|| {
            format!(
                "failed promoting staged policy package into {}",
                target_dir.display()
            )
        });
    }

    Ok(target_dir)
}

fn persist_slot_bindings(
    packages_dir: &Path,
    slots: &BTreeMap<String, String>,
    active_profile: &str,
) -> Result<()> {
    let bindings = PersistedSlotBindings {
        slots: slots.clone(),
        active_profile: if active_profile.is_empty() {
            None
        } else {
            Some(active_profile.to_string())
        },
    };
    let payload = serde_json::to_vec_pretty(&bindings)?;
    let tmp = packages_dir.join(format!(".doso_slots.tmp.{}", std::process::id()));
    let dst = packages_dir.join(SLOT_BINDINGS_FILE);
    std::fs::write(&tmp, payload)?;
    if let Err(err) = std::fs::rename(&tmp, &dst) {
        if err.kind() == std::io::ErrorKind::PermissionDenied && dst.exists() {
            std::fs::remove_file(&dst)?;
            std::fs::rename(&tmp, &dst)?;
        } else {
            let _ = std::fs::remove_file(&tmp);
            return Err(err.into());
        }
    }
    Ok(())
}

async fn handle_health(State(state): State<HotDeployState>) -> impl IntoResponse {
    // Lock-free relative to the 200Hz `engine` mutex: read exclusively from `RwLock<TelemetrySnapshot>`.
    let (
        ok,
        brainstem_ready,
        strategy_ready,
        can_read_only,
        active_profile,
        imu_online,
        online_joints,
        uptime_s,
    ) = {
        let tele = state
            .shared
            .telemetry
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let active = tele.status.active_profile.clone();
        let strat_ready = !active.is_empty();
        let imu_ok = tele.proprio.imu.online;
        let joints_ok = tele.proprio.online_mask().count_ones() as u8;
        let hardware_liveness_ok = if tele.hal_backend == "socketcan" {
            imu_ok && joints_ok == 16 && tele.hal_rx_frames > 0
        } else {
            true
        };
        let bs_ready = strat_ready && !tele.status.critical_motor_fault && hardware_liveness_ok;
        let can_ro = tele.can_read_only;
        let up = state.shared.boot_instant.elapsed().as_secs();
        (
            bs_ready,
            bs_ready,
            strat_ready,
            can_ro,
            active,
            imu_ok,
            joints_ok,
            up,
        )
    };

    let core = HealthCorePayload {
        ok,
        boot_id: state.cfg.boot_id.clone(),
        version: state.cfg.version.clone(),
        release_id: state.cfg.release_id.clone(),
        brainstem_ready,
        strategy_ready,
        can_read_only,
        active_profile,
        imu_online,
        online_joints,
        uptime_s,
    };

    let (algorithm, signature) = if let Some(kp) = &state.signing_key {
        match serde_json::to_vec(&core) {
            Ok(canonical) => {
                let sig = kp.sign(&canonical);
                (Some("ed25519"), Some(B64URL.encode(sig.as_ref())))
            }
            Err(_) => (None, None),
        }
    } else {
        (None, None)
    };

    let resp = HealthResponse {
        core,
        algorithm,
        signature,
    };
    let code = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    json_resp(code, &resp)
}

async fn handle_list_policies(
    State(state): State<HotDeployState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Some(unauth) = check_admin_auth(&headers, &state.cfg) {
        return unauth.into_response();
    }
    let (active_profile, slots, policies) = {
        let guard = state
            .shared
            .engine
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let active = guard.policies.active_name().to_string();
        let slots: BTreeMap<String, String> = guard
            .policies
            .slots()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let policies: Vec<PolicySummaryJson> = guard
            .policies
            .reports()
            .into_iter()
            .map(|r| PolicySummaryJson {
                name: r.name,
                sha256: r.sha256,
                warmup_us: r.warmup_last_us,
                golden_steps: r.golden_steps_checked,
                golden_max_err: r.golden_max_abs_err,
            })
            .collect();
        (active, slots, policies)
    };

    let resp = ListPoliciesResponse {
        ok: true,
        active_profile,
        slots,
        policies,
        rt_loop: RtLoopMetricsJson {
            max_tick_us: state.shared.rt_tick_max_us.load(Ordering::Relaxed),
            max_jitter_us: state.shared.rt_jitter_max_us.load(Ordering::Relaxed),
            max_mutex_wait_us: state.shared.rt_mutex_wait_max_us.load(Ordering::Relaxed),
            ticks: state.shared.rt_tick_count.load(Ordering::Relaxed),
        },
    };
    json_resp(StatusCode::OK, &resp).into_response()
}

async fn handle_push_policy(
    State(state): State<HotDeployState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if let Some(unauth) = check_admin_auth(&headers, &state.cfg) {
        return unauth.into_response();
    }

    let t0 = Instant::now();
    let req: PushPolicyRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return err_resp(
                StatusCode::BAD_REQUEST,
                None,
                format!("invalid request payload: {e}"),
            )
            .into_response();
        }
    };

    let onnx_bytes = match B64.decode(req.onnx_base64.trim()) {
        Ok(b) => b,
        Err(e) => {
            return err_resp(
                StatusCode::BAD_REQUEST,
                None,
                format!("invalid base64 ONNX: {e}"),
            )
            .into_response();
        }
    };

    // 1. Compile, SHA256-check, warmup, and Golden-verify completely OFF the real-time thread.
    let manifest_for_load = req.manifest_yaml.clone();
    let golden_for_load = req.golden_json.clone();
    let onnx_for_load = onnx_bytes.clone();
    let loaded_res = tokio::task::spawn_blocking(move || {
        LoadedPolicyPackage::load_and_verify(
            &manifest_for_load,
            &onnx_for_load,
            golden_for_load.as_deref(),
            None,
        )
    })
    .await;

    let mut pkg = match loaded_res {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            return err_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                Some("golden_or_schema_gate"),
                format!("{e:#}"),
            )
            .into_response();
        }
        Err(e) => {
            return err_resp(
                StatusCode::INTERNAL_SERVER_ERROR,
                None,
                format!("worker join failed: {e}"),
            )
            .into_response();
        }
    };

    // 2. Grab live proprioception snapshot in < 5 us, drop RT mutex immediately, and run Shadow Mode OFF-LOCK.
    let (pkg, shadow) = if req.shadow_frames > 0 {
        let (frames, walk_cmd, bh_m) = {
            let guard = state
                .shared
                .engine
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.snapshot_for_shadow(req.shadow_frames)
        };

        let shadow_res = tokio::task::spawn_blocking(move || {
            let eval = ShadowEvaluator::evaluate_frames(&mut pkg, &frames, walk_cmd, bh_m);
            (pkg, eval)
        })
        .await;

        match shadow_res {
            Ok((pkg_back, Ok(s))) => (
                pkg_back,
                Some(ShadowEvalJson {
                    steps: s.steps,
                    p50_us: s.p50_us,
                    p99_us: s.p99_us,
                    max_us: s.max_us,
                    clipped_joints: s.total_clipped_joints,
                    max_leg_delta_rad: s.max_leg_step_delta_rad,
                }),
            ),
            Ok((_, Err(e))) => {
                return err_resp(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Some("shadow_eval"),
                    format!("{e:#}"),
                )
                .into_response();
            }
            Err(e) => {
                return err_resp(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    None,
                    format!("shadow worker join failed: {e}"),
                )
                .into_response();
            }
        }
    } else {
        (pkg, None)
    };

    // 3. Persist verified package atomically to `<packages_dir>/<name>/` OFF-LOCK before registration.
    let persisted_dir: Option<String> = if let Some(pkg_root) = state.cfg.packages_dir.clone() {
        let policy_name = pkg.report.name.clone();
        let manifest_yaml = req.manifest_yaml.clone();
        let golden_json = req.golden_json.clone();
        let persist_res = tokio::task::spawn_blocking(move || {
            persist_policy_package_atomic(
                &pkg_root,
                &policy_name,
                &manifest_yaml,
                &onnx_bytes,
                golden_json.as_deref(),
            )
        })
        .await;
        match persist_res {
            Ok(Ok(path)) => Some(path.display().to_string()),
            Ok(Err(e)) => {
                return err_resp(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Some("persist"),
                    format!("{e:#}"),
                )
                .into_response();
            }
            Err(e) => {
                return err_resp(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Some("persist"),
                    format!("persist worker join failed: {e}"),
                )
                .into_response();
            }
        }
    } else {
        None
    };

    // 4. Re-lock RT mutex for < 10 us ONLY for atomic slot registration + optional profile switch.
    let (report, active_after, slots_snapshot) = {
        let now = state.shared.now_us();
        let mut guard = state
            .shared
            .engine
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let report = match guard.register_policy(pkg, req.slot.as_deref()) {
            Ok(r) => r,
            Err(e) => {
                return err_resp(StatusCode::BAD_REQUEST, Some("register"), e.to_string())
                    .into_response();
            }
        };

        if req.activate {
            let target = req.slot.as_deref().unwrap_or(&report.name);
            if let Err(e) = guard.switch_profile(target) {
                state.shared.refresh_telemetry(&guard, now);
                return err_resp(StatusCode::CONFLICT, Some("activate"), e.to_string())
                    .into_response();
            }
        }

        let active_after = guard.policies.active_name().to_string();
        let slots_snapshot: BTreeMap<String, String> = guard
            .policies
            .slots()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        state.shared.refresh_telemetry(&guard, now);
        (report, active_after, slots_snapshot)
    };

    if let Some(pkg_root) = &state.cfg.packages_dir {
        if let Err(err) = persist_slot_bindings(pkg_root, &slots_snapshot, &active_after) {
            warn!(error = %err, "Failed persisting slot bindings after hot push");
        }
    }

    let elapsed_ms = (t0.elapsed().as_secs_f64() * 1000.0 * 100.0).round() / 100.0;
    let resp = PushPolicyResponse {
        ok: true,
        elapsed_ms,
        name: report.name,
        sha256: report.sha256,
        warmup_us: report.warmup_last_us,
        golden_steps: report.golden_steps_checked,
        golden_max_err: report.golden_max_abs_err,
        shadow,
        bound_slot: req.slot,
        active_profile: active_after,
        persisted_dir,
    };
    json_resp(StatusCode::OK, &resp).into_response()
}

async fn handle_rollback_policy(
    State(state): State<HotDeployState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if let Some(unauth) = check_admin_auth(&headers, &state.cfg) {
        return unauth.into_response();
    }

    let req: RollbackRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return err_resp(StatusCode::BAD_REQUEST, None, e.to_string()).into_response();
        }
    };
    let (restored, active_after, slots_snapshot) = {
        let now = state.shared.now_us();
        let mut guard = state
            .shared
            .engine
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match guard.rollback_slot(&req.slot) {
            Ok(restored) => {
                let active_after = guard.policies.active_name().to_string();
                let slots_snapshot: BTreeMap<String, String> = guard
                    .policies
                    .slots()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                state.shared.refresh_telemetry(&guard, now);
                (restored, active_after, slots_snapshot)
            }
            Err(e) => {
                return err_resp(StatusCode::CONFLICT, None, e.to_string()).into_response();
            }
        }
    };

    if let Some(pkg_root) = &state.cfg.packages_dir {
        if let Err(err) = persist_slot_bindings(pkg_root, &slots_snapshot, &active_after) {
            warn!(error = %err, "Failed persisting slot bindings after rollback");
        }
    }

    json_resp(
        StatusCode::OK,
        &RollbackResponse {
            ok: true,
            slot: req.slot,
            restored_policy: restored,
            active_profile: active_after,
        },
    )
    .into_response()
}
