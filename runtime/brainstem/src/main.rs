use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use brainstem_dds_bridge::{
    spawn_base_gateway_on_listener, AskMeVoiceConfig, BaseGatewayConfig, BaseGatewayService,
};
use clap::Parser;
use doso_brain::BrainEngine;
use doso_brainstem::brainstem_grpc::{BrainstemGrpcServer, LocalStatusGrpcServer, SharedBrain};
use doso_brainstem::hot_deploy::{
    build_hot_deploy_router_with_config, generate_boot_id, write_boot_id_marker, HotDeployConfig,
    DEFAULT_BOOT_ID_MARKER_PATH,
};
use doso_brainstem::preload_packages;
use doso_core::{MitCmd16, MitTarget};
use doso_hal::{BenchNullHal, HalBackend};
use doso_safety::SafetyConfig;
use doso_sdk::internal::motion_board::rpc::{
    robot_control_server::RobotControlServer,
    robot_external_control_server::RobotExternalControlServer,
    robot_recovery_server::RobotRecoveryServer,
};
use doso_shm_ipc::{DosoProprioPayload, ExternalCmdReader, ProprioRingWriter};
use tokio::net::TcpListener;
use tracing::{info, warn};

const RT_LOOP_PERIOD_NS: i64 = 5_000_000; // 200Hz
#[cfg(target_os = "linux")]
const RT_SCHED_FIFO_PRIORITY: i32 = 80;

#[derive(Debug, Parser)]
#[command(
    name = "doso-brainstem",
    about = "Unified Single-Process Rust Brainstem Runtime"
)]
struct Args {
    /// Directory containing declarative policy package subdirectories (`policy.yaml` + ONNX + `golden.json`).
    #[arg(long, env = "DOSO_PACKAGES_DIR", default_value = "models/packages")]
    packages_dir: PathBuf,

    /// Hardware backend (`bench` for safe S100P benchtop, `socketcan` for real RDK S100 robot).
    #[arg(long, env = "DOSO_HAL_BACKEND", default_value = "bench")]
    hal_backend: String,

    /// Explicitly allow physical motor enable & CAN TX (`false` = 100% read-only / benchtop safe!).
    #[arg(long, env = "THUNDER_ALLOW_MOTOR_ENABLE", default_value_t = false)]
    allow_motor_enable: bool,

    /// Internal / legacy `brainstem.api.v1` gRPC listen address (`127.0.0.1:13145` or `0.0.0.0:13145`).
    #[arg(long, env = "DOSO_BRAINSTEM_LISTEN", default_value = "127.0.0.1:13145")]
    brainstem_listen: String,

    /// Optional read-only `RobotControl/GetControlStatus` gRPC listener for `doso-cloud-link` (`""` or `"off"` to disable).
    #[arg(
        long,
        env = "DOSO_LOCAL_STATUS_LISTEN",
        default_value = "127.0.0.1:13146"
    )]
    local_status_listen: String,

    /// Unified public `doso.v1` Base Gateway listen address.
    #[arg(
        long,
        env = "BRAINSTEM_BASE_GATEWAY_LISTEN",
        default_value = "0.0.0.0:5052"
    )]
    gateway_listen: String,

    /// Zero-restart `<10s` Policy Hot-Deploy & `/health` HTTP API listen address.
    #[arg(
        long,
        env = "DOSO_POLICY_ADMIN_LISTEN",
        default_value = "0.0.0.0:5054"
    )]
    admin_listen: String,

    /// Optional Bearer token guarding `/v1/policies*` endpoints (`/health` remains readable).
    #[arg(long, env = "DOSO_POLICY_ADMIN_TOKEN")]
    admin_token: Option<String>,

    /// Optional PKCS#8 Ed25519 PEM private key path for signing `GET /health` responses.
    #[arg(long, env = "DOSO_HEALTH_SIGNING_KEY")]
    health_signing_key: Option<PathBuf>,

    /// Optional onboard AskMe HTTP endpoint (e.g. `http://127.0.0.1:8765`).
    #[arg(long, env = "ASKME_ENDPOINT")]
    askme_endpoint: Option<String>,

    #[arg(long, env = "DOSO_DEVICE_ID", default_value = "THUNDER-RUST-001")]
    device_id: String,

    #[arg(long, env = "DOSO_FIRMWARE_VERSION", default_value = "v1.0.0-rust")]
    firmware_version: String,

    #[arg(long, env = "DOSO_RELEASE_ID")]
    release_id: Option<String>,

    #[arg(long, env = "DOSO_SHM_IPC_ENABLED", default_value_t = true)]
    shm_enabled: bool,

    #[arg(
        long,
        env = "DOSO_SHM_PROPRIO_NAME",
        default_value = "/doso_proprio_v1"
    )]
    shm_proprio_name: String,

    #[arg(long, env = "DOSO_SHM_CMD_NAME", default_value = "/doso_ext_cmd_v1")]
    shm_cmd_name: String,
}

fn create_hal(backend: &str, allow_tx: bool) -> Result<Box<dyn HalBackend>> {
    match backend {
        "bench" | "null" | "sim" => Ok(Box::new(BenchNullHal::new(allow_tx))),
        #[cfg(target_os = "linux")]
        "socketcan" | "rdk-s100-pcan" => {
            // Canonical RDK S100 leg-to-SocketCAN mapping: FR=can0, FL=can2, RR=can1, RL=can3
            let hal = doso_hal::SocketCanHal::open(["can0", "can2", "can1", "can3"], allow_tx)?;
            Ok(Box::new(hal))
        }
        other => anyhow::bail!("Unsupported --hal-backend '{other}'"),
    }
}

#[cfg(target_os = "linux")]
fn lock_process_memory_best_effort() {
    let rc = unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) };
    if rc == 0 {
        info!("mlockall(MCL_CURRENT | MCL_FUTURE) active");
    } else {
        warn!(
            error = %std::io::Error::last_os_error(),
            "mlockall unavailable (check LimitMEMLOCK=infinity); continuing without locked RAM"
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn lock_process_memory_best_effort() {}

#[cfg(target_os = "linux")]
fn elevate_rt_thread_best_effort() {
    let param = libc::sched_param {
        sched_priority: RT_SCHED_FIFO_PRIORITY,
    };
    let rc = unsafe { libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param) };
    if rc == 0 {
        info!(
            priority = RT_SCHED_FIFO_PRIORITY,
            "doso-rt-loop elevated to SCHED_FIFO"
        );
    } else {
        warn!(
            errno = rc,
            priority = RT_SCHED_FIFO_PRIORITY,
            "SCHED_FIFO elevation unavailable (check LimitRTPRIO=95); continuing with default scheduler"
        );
    }
}

#[cfg(target_os = "linux")]
#[allow(clippy::unnecessary_cast)]
fn timespec_add_ns(ts: &mut libc::timespec, delta_ns: i64) {
    let total_ns = ts.tv_nsec as i64 + delta_ns;
    ts.tv_sec += total_ns / 1_000_000_000;
    ts.tv_nsec = (total_ns % 1_000_000_000) as _;
}

#[cfg(target_os = "linux")]
fn timespec_before(a: &libc::timespec, b: &libc::timespec) -> bool {
    a.tv_sec < b.tv_sec || (a.tv_sec == b.tv_sec && a.tv_nsec < b.tv_nsec)
}

#[cfg(target_os = "linux")]
fn run_rt_loop_paced<F: FnMut()>(stop_flag: &AtomicBool, mut step: F) {
    elevate_rt_thread_best_effort();
    let mut next_ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut next_ts);
    }
    timespec_add_ns(&mut next_ts, RT_LOOP_PERIOD_NS);

    while !stop_flag.load(Ordering::Relaxed) {
        step();
        loop {
            let rc = unsafe {
                libc::clock_nanosleep(
                    libc::CLOCK_MONOTONIC,
                    libc::TIMER_ABSTIME,
                    &next_ts,
                    std::ptr::null_mut(),
                )
            };
            if rc != libc::EINTR || stop_flag.load(Ordering::Relaxed) {
                break;
            }
        }
        timespec_add_ns(&mut next_ts, RT_LOOP_PERIOD_NS);
        let mut now_ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        unsafe {
            libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now_ts);
        }
        if timespec_before(&next_ts, &now_ts) {
            next_ts = now_ts;
            timespec_add_ns(&mut next_ts, RT_LOOP_PERIOD_NS);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn run_rt_loop_paced<F: FnMut()>(stop_flag: &AtomicBool, mut step: F) {
    let period = Duration::from_nanos(RT_LOOP_PERIOD_NS as u64);
    let mut next_deadline = Instant::now() + period;
    while !stop_flag.load(Ordering::Relaxed) {
        step();
        let now = Instant::now();
        if next_deadline > now {
            std::thread::sleep(next_deadline - now);
        }
        next_deadline += period;
        if next_deadline < Instant::now() {
            next_deadline = Instant::now() + period;
        }
    }
}

fn optional_listen_addr(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("off")
        || trimmed.eq_ignore_ascii_case("none")
    {
        None
    } else {
        Some(trimmed)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let watchdog_cfg = doso_brainstem::watchdog::parse_watchdog_env_from_os()?;
    let boot_id = generate_boot_id();

    // Lock current and future memory pages in RAM on Linux RT boards before launching loops.
    lock_process_memory_best_effort();

    let hal = create_hal(&args.hal_backend, args.allow_motor_enable)?;
    let mut engine = BrainEngine::new(
        hal,
        SafetyConfig {
            allow_motor_enable: args.allow_motor_enable,
            ..SafetyConfig::default()
        },
    );
    preload_packages(&mut engine, &args.packages_dir)?;

    let shared = Arc::new(SharedBrain {
        engine: Mutex::new(engine),
        boot_instant: Instant::now(),
    });
    doso_brainstem::fatal_cleanup::install_panic_motor_kill_hook(shared.clone());

    // 1. Spawn dedicated 200Hz/50Hz deterministic RT control thread + lock-free SHM publisher
    let stop_flag = Arc::new(AtomicBool::new(false));
    let rt_stop = stop_flag.clone();
    let rt_shared = shared.clone();
    let shm_enabled = args.shm_enabled;
    let shm_proprio_name = args.shm_proprio_name.clone();
    let shm_cmd_name = args.shm_cmd_name.clone();

    let rt_handle = std::thread::Builder::new()
        .name("doso-rt-loop".into())
        .spawn(move || {
            let mut shm_writer = if shm_enabled {
                match ProprioRingWriter::open(&shm_proprio_name) {
                    Ok(w) => {
                        info!(shm = %shm_proprio_name, "POSIX/Win32 SHM proprioception publisher active (200Hz)");
                        Some(w)
                    }
                    Err(e) => {
                        warn!(error = %e, "SHM writer unavailable; falling back to gRPC telemetry");
                        None
                    }
                }
            } else {
                None
            };
            let mut shm_cmd_reader: Option<ExternalCmdReader> = None;
            let mut last_shm_cmd_seq = 0u64;

            run_rt_loop_paced(&rt_stop, || {
                let now_us = rt_shared.now_us();
                let mut guard = rt_shared
                    .engine
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if shm_enabled && guard.external.active {
                    if shm_cmd_reader.is_none() {
                        shm_cmd_reader = ExternalCmdReader::open(&shm_cmd_name).ok();
                    }
                    if let Some(reader) = &shm_cmd_reader {
                        if let Some((seq, cmd_payload)) = reader.read_latest(last_shm_cmd_seq) {
                            last_shm_cmd_seq = seq;
                            let targets = std::array::from_fn(|i| MitTarget {
                                q: cmd_payload.q[i],
                                dq: cmd_payload.dq[i],
                                kp: cmd_payload.kp[i],
                                kd: cmd_payload.kd[i],
                                tau: cmd_payload.tau[i],
                            });
                            let _ = guard.external_submit_frame(
                                cmd_payload.sequence,
                                MitCmd16(targets),
                                now_us,
                            );
                        }
                    }
                }
                guard.tick_200hz(now_us);

                if let Some(writer) = &mut shm_writer {
                    let p = guard.latest_proprio();
                    let st = guard.status_snapshot(now_us);
                    let payload = DosoProprioPayload {
                        sequence: p.sequence,
                        monotonic_us: p.monotonic_us,
                        imu_quat_wxyz: p.imu.quat_wxyz,
                        imu_gyro_rad_s: p.imu.gyro_rad_s,
                        imu_accel_m_s2: p.imu.accel_m_s2,
                        joint_q: std::array::from_fn(|i| p.joints[i].q),
                        joint_dq: std::array::from_fn(|i| p.joints[i].dq),
                        joint_tau: std::array::from_fn(|i| p.joints[i].tau),
                        joint_temp_c: std::array::from_fn(|i| p.joints[i].temp_c),
                        joint_status: std::array::from_fn(|i| p.joints[i].status_code as u16),
                        joint_online_mask: p.online_mask(),
                        imu_fresh: u8::from(p.imu.online),
                        cms_state: st.fsm_state.to_public_body_kind() as u8,
                        control_owner: st.source_owner as u8,
                        motor_output_enabled: u8::from(st.motor_output_enabled),
                        critical_motor_fault: u8::from(st.critical_motor_fault),
                        _reserved: [0; 9],
                    };
                    let _ = writer.publish(&payload);
                }
            });
        })?;

    // 2. Start internal / legacy `brainstem.api.v1` gRPC server (:13145)
    let bs_listener = TcpListener::bind(&args.brainstem_listen).await?;
    let bs_local_addr = bs_listener.local_addr()?;
    let bs_svc = Arc::new(BrainstemGrpcServer {
        shared: shared.clone(),
    });
    let (bs_shutdown_tx, bs_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let bs_handle = tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .tcp_nodelay(true)
            .add_service(RobotControlServer::from_arc(bs_svc.clone()))
            .add_service(RobotExternalControlServer::from_arc(bs_svc.clone()))
            .add_service(RobotRecoveryServer::from_arc(bs_svc))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(bs_listener),
                async move {
                    let _ = bs_shutdown_rx.await;
                },
            )
            .await;
    });
    info!(addr = %bs_local_addr, "Rust Brainstem internal/compat gRPC listening");

    // 2b. Optional read-only local status gRPC listener (:13146) for `doso-cloud-link`
    let (local_status_tx, local_status_handle) =
        if let Some(addr_str) = optional_listen_addr(&args.local_status_listen) {
            let ls_listener = TcpListener::bind(addr_str).await?;
            let ls_addr = ls_listener.local_addr()?;
            let ls_svc = Arc::new(LocalStatusGrpcServer {
                shared: shared.clone(),
            });
            let (tx, rx) = tokio::sync::oneshot::channel::<()>();
            let handle = tokio::spawn(async move {
                let _ = tonic::transport::Server::builder()
                    .tcp_nodelay(true)
                    .add_service(RobotControlServer::from_arc(ls_svc))
                    .serve_with_incoming_shutdown(
                        tokio_stream::wrappers::TcpListenerStream::new(ls_listener),
                        async move {
                            let _ = rx.await;
                        },
                    )
                    .await;
            });
            info!(addr = %ls_addr, "Read-only local status gRPC listening (doso-cloud-link compat)");
            (Some(tx), Some(handle))
        } else {
            (None, None)
        };

    // 3. Start unified customer-facing `doso.v1` BaseGateway (:5052) in the SAME single process!
    let gw_listener = TcpListener::bind(&args.gateway_listen).await?;
    let askme_cfg = args.askme_endpoint.as_ref().map(|ep| AskMeVoiceConfig {
        endpoint: ep.clone(),
        operator_id: "supervisor-1".into(),
        api_token: None,
        robot_id: "xiaosuan-1".into(),
        timeout: Duration::from_secs(10),
    });
    let gw_cfg = BaseGatewayConfig {
        brainstem_endpoint: format!("http://127.0.0.1:{}", bs_local_addr.port()),
        listen_addr: args.gateway_listen.clone(),
        robot_id: Some(args.device_id.clone()),
        robot_model: "thunder".into(),
        firmware_version: args.firmware_version.clone(),
        shm_enabled: args.shm_enabled,
        shm_proprio_name: args.shm_proprio_name.clone(),
        shm_cmd_name: args.shm_cmd_name.clone(),
        askme_voice: askme_cfg,
        ..BaseGatewayConfig::default()
    };
    let (gw_shutdown_tx, gw_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let (gw_addr, gw_handle) =
        spawn_base_gateway_on_listener(BaseGatewayService::new(gw_cfg), gw_listener, gw_shutdown_rx)
            .await?;
    info!(addr = %gw_addr, "Unified doso.v1 Base Gateway listening (single-process Rust)");

    // 4. Start <10s Zero-Restart Policy Hot-Deploy & `/health` HTTP API (:5054)
    let admin_listener = TcpListener::bind(&args.admin_listen).await?;
    let admin_addr = admin_listener.local_addr()?;
    let release_id = args
        .release_id
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| args.firmware_version.clone());
    let hot_cfg = HotDeployConfig {
        packages_dir: Some(args.packages_dir.clone()),
        admin_token: args.admin_token.clone(),
        boot_id: boot_id.clone(),
        version: args.firmware_version.clone(),
        release_id,
        health_signing_key_path: args.health_signing_key.clone(),
    };
    let router = build_hot_deploy_router_with_config(shared.clone(), hot_cfg);
    let (admin_shutdown_tx, admin_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let admin_handle = tokio::spawn(async move {
        let _ = axum::serve(admin_listener, router)
            .with_graceful_shutdown(async move {
                let _ = admin_shutdown_rx.await;
            })
            .await;
    });
    info!(
        addr = %admin_addr,
        boot_id = %boot_id,
        "Policy Hot-Deploy & Health HTTP API ready (/health, /v1/policies/{{push,rollback}})"
    );

    // Best-effort boot_id marker for OTA health gate when `/run/doso` is present & writable.
    write_boot_id_marker(Path::new(DEFAULT_BOOT_ID_MARKER_PATH), &boot_id);

    // 5. Notify systemd READY=1 and start native Watchdog keepalive if running under Type=notify
    let mut watchdog = match watchdog_cfg {
        Some(cfg) => {
            let wd = doso_brainstem::watchdog::SystemdWatchdog::start(cfg)?;
            info!(
                interval_ms = wd.interval().map(|d| d.as_millis() as u64),
                "Systemd READY=1 notified and watchdog active"
            );
            Some(wd)
        }
        None => None,
    };

    let signal_name = doso_brainstem::fatal_cleanup::wait_for_shutdown_signal().await;
    info!(signal = signal_name, "Shutting down doso-brainstem...");

    // Deterministic shutdown order:
    // 1) stop RT loop -> 2) passive damping + disable_motors(false) -> 3) watchdog.stop() -> 4) shut down servers
    stop_flag.store(true, Ordering::SeqCst);
    if let Err(err) = rt_handle.join() {
        warn!("RT loop thread joined after panic: {err:?}");
    }

    if doso_brainstem::fatal_cleanup::emergency_disable_motors(&shared) {
        info!("Motors safely damped and disabled (disable_motors(false))");
    } else {
        warn!("Timed out acquiring BrainEngine lock during shutdown motor disable");
    }

    if let Some(mut wd) = watchdog.take() {
        wd.stop().await;
    }

    let _ = gw_shutdown_tx.send(());
    let _ = admin_shutdown_tx.send(());
    if let Some(tx) = local_status_tx {
        let _ = tx.send(());
    }
    let _ = bs_shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(3), gw_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), admin_handle).await;
    if let Some(handle) = local_status_handle {
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), bs_handle).await;
    Ok(())
}
