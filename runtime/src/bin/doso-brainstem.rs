#![deny(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use brainstem_dds_bridge::{
    spawn_base_gateway_on_listener, AskMeVoiceConfig, BaseGatewayConfig, BaseGatewayService,
};
use clap::Parser;
use doso_runtime::control::BrainEngine;
use doso_runtime::hardware::{BenchNullHal, HalBackend};
use doso_runtime::params::RobotHardwareParams;
use doso_runtime::safety::{
    emergency_disable_motors, install_panic_motor_kill_hook, wait_for_shutdown_signal, SafetyConfig,
};
use doso_runtime::server::{
    build_hot_deploy_router_with_config, generate_boot_id, parse_watchdog_env_from_os,
    preload_packages, write_boot_id_marker, BrainstemGrpcServer, HotDeployConfig,
    LocalStatusGrpcServer, SharedBrain, SystemdWatchdog, DEFAULT_BOOT_ID_MARKER_PATH,
};
use doso_runtime::types::{MitCmd16, MitTarget};
use doso_sdk::internal::motion_board::rpc::{
    robot_control_server::RobotControlServer,
    robot_external_control_server::RobotExternalControlServer,
    robot_recovery_server::RobotRecoveryServer,
};
use doso_shm_ipc::{DosoProprioPayload, ExternalCmdReader, ProprioRingWriter};
use tokio::net::TcpListener;
use tracing::{info, warn};

#[derive(Debug, Parser)]
#[command(
    name = "doso-brainstem",
    about = "Unified Single-Process Rust Brainstem Runtime"
)]
struct Args {
    /// Directory containing declarative policy package subdirectories (`policy.yaml` + ONNX + `golden.json`).
    #[arg(long, env = "DOSO_PACKAGES_DIR", default_value = "models/packages")]
    packages_dir: PathBuf,

    /// Optional hardware parameter YAML path (defaults to embedded `config/thunder_v4.yaml`).
    #[arg(long, env = "DOSO_HARDWARE_CONFIG")]
    hardware_config: Option<PathBuf>,

    /// Hardware backend (`bench` for safe S100P benchtop, `socketcan` for real RDK S100 robot).
    #[arg(long, env = "DOSO_HAL_BACKEND", default_value = "bench")]
    hal_backend: String,

    /// Explicitly allow physical motor enable & CAN TX (`false` = 100% read-only / benchtop safe).
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
    #[arg(long, env = "DOSO_POLICY_ADMIN_LISTEN", default_value = "0.0.0.0:5054")]
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

#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
fn create_hal(
    backend: &str,
    hw_params: &RobotHardwareParams,
    allow_tx: bool,
) -> Result<Box<dyn HalBackend>> {
    match backend {
        "bench" | "null" | "sim" => Ok(Box::new(BenchNullHal::new(allow_tx))),
        #[cfg(target_os = "linux")]
        "socketcan" | "rdk-s100-pcan" => {
            let hal = doso_runtime::hardware::SocketCanHal::open_with_params(hw_params, allow_tx)?;
            Ok(Box::new(hal))
        }
        other => anyhow::bail!("Unsupported --hal-backend '{other}'"),
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

#[cfg(target_os = "linux")]
const NSEC_PER_SEC: i64 = 1_000_000_000;
#[cfg(target_os = "linux")]
const RT_PERIOD_NS: i64 = 5_000_000; // 5ms = 200Hz
#[cfg(target_os = "linux")]
const RT_SCHED_FIFO_PRIORITY: i32 = 80;

#[cfg(target_os = "linux")]
#[inline]
#[allow(unsafe_code, clippy::unnecessary_cast)]
fn monotonic_now_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid mutable pointer to stack timespec; CLOCK_MONOTONIC is always valid on Linux.
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
    }
    (ts.tv_sec as i64) * NSEC_PER_SEC + (ts.tv_nsec as i64)
}

#[cfg(target_os = "linux")]
#[inline]
#[allow(clippy::unnecessary_cast)]
fn ns_to_timespec(total_ns: i64) -> libc::timespec {
    libc::timespec {
        tv_sec: (total_ns / NSEC_PER_SEC) as libc::time_t,
        tv_nsec: (total_ns % NSEC_PER_SEC) as libc::c_long,
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn sleep_until_monotonic_ns(deadline_ns: i64, rt_stop: &AtomicBool) {
    let ts = ns_to_timespec(deadline_ns);
    loop {
        // SAFETY: absolute CLOCK_MONOTONIC sleep with valid timespec and null remain pointer.
        let rc = unsafe {
            libc::clock_nanosleep(
                libc::CLOCK_MONOTONIC,
                libc::TIMER_ABSTIME,
                &ts,
                std::ptr::null_mut(),
            )
        };
        if rc != libc::EINTR || rt_stop.load(Ordering::Relaxed) {
            break;
        }
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn configure_linux_rt_thread() {
    if let Ok(raw_mlock) = std::env::var("DOSO_RT_MLOCK") {
        let trimmed = raw_mlock.trim();
        if trimmed.eq_ignore_ascii_case("true") || trimmed == "1" {
            // SAFETY: best-effort mlockall to prevent RT page-fault stalls; failure is logged and non-fatal.
            let rc = unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) };
            if rc == 0 {
                info!("RT memory page lock active (mlockall MCL_CURRENT | MCL_FUTURE)");
            } else {
                let err = std::io::Error::last_os_error();
                warn!(error = %err, "Best-effort mlockall(MCL_CURRENT | MCL_FUTURE) failed; continuing");
            }
        }
    }

    let param = libc::sched_param {
        sched_priority: RT_SCHED_FIFO_PRIORITY,
    };
    // SAFETY: best-effort SCHED_FIFO elevation for current RT thread; non-fatal if unprivileged.
    let sched_rc =
        unsafe { libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param) };
    if sched_rc == 0 {
        info!(
            priority = RT_SCHED_FIFO_PRIORITY,
            "doso-rt-loop elevated to SCHED_FIFO"
        );
    } else {
        warn!(
            errno = sched_rc,
            priority = RT_SCHED_FIFO_PRIORITY,
            "SCHED_FIFO elevation unavailable (check LimitRTPRIO=95); continuing with default scheduler"
        );
    }

    if let Ok(raw_core) = std::env::var("DOSO_RT_CPU_CORE") {
        let trimmed = raw_core.trim();
        if !trimmed.is_empty() {
            match trimmed.parse::<usize>() {
                Ok(core_id) if core_id < libc::CPU_SETSIZE as usize => {
                    // SAFETY: zero-initialized cpu_set_t with valid core index < CPU_SETSIZE; pid=0 targets calling thread.
                    unsafe {
                        let mut cpuset: libc::cpu_set_t = std::mem::zeroed();
                        libc::CPU_ZERO(&mut cpuset);
                        libc::CPU_SET(core_id, &mut cpuset);
                        let rc = libc::sched_setaffinity(
                            0,
                            std::mem::size_of::<libc::cpu_set_t>(),
                            &cpuset,
                        );
                        if rc == 0 {
                            info!(
                                cpu_core = core_id,
                                "Pinned doso-rt-loop thread CPU affinity"
                            );
                        } else {
                            let err = std::io::Error::last_os_error();
                            warn!(cpu_core = core_id, error = %err, "Best-effort sched_setaffinity failed");
                        }
                    }
                }
                _ => {
                    warn!(DOSO_RT_CPU_CORE = %trimmed, "Invalid DOSO_RT_CPU_CORE; expected valid CPU core index");
                }
            }
        }
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
    let watchdog_cfg = parse_watchdog_env_from_os()?;
    let boot_id = generate_boot_id();
    let hw_params = RobotHardwareParams::load_or_default(args.hardware_config.as_deref())?;
    info!(
        robot_model = %hw_params.robot_model,
        can_buses = ?hw_params.can_bus_interfaces(),
        hardware_config = ?args.hardware_config,
        boot_id = %boot_id,
        "Loaded validated robot hardware parameters"
    );

    let hal = create_hal(&args.hal_backend, &hw_params, args.allow_motor_enable)?;
    let safety_cfg = SafetyConfig::from_hardware_params(&hw_params, args.allow_motor_enable);
    let mut engine = BrainEngine::new(hal, safety_cfg);
    preload_packages(&mut engine, &args.packages_dir)?;

    let shared = Arc::new(SharedBrain::new(engine));
    install_panic_motor_kill_hook(shared.clone());

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
            #[cfg(target_os = "linux")]
            configure_linux_rt_thread();

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

            #[cfg(target_os = "linux")]
            let mut deadline_ns = monotonic_now_ns();
            #[cfg(not(target_os = "linux"))]
            let period = Duration::from_millis(5); // 200Hz
            #[cfg(not(target_os = "linux"))]
            let mut next_deadline = Instant::now() + period;

            let mut jitter_us = 0u64;

            while !rt_stop.load(Ordering::Relaxed) {
                let tick_start = Instant::now();
                let now_us = rt_shared.now_us();
                let (mutex_wait_us, shm_payload) = {
                    let mut guard = rt_shared
                        .engine
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let wait_us = tick_start.elapsed().as_micros() as u64;

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
                    rt_shared.refresh_telemetry(&guard, now_us);

                    let payload = if shm_writer.is_some() {
                        let p = guard.latest_proprio();
                        let st = guard.status_snapshot(now_us);
                        Some(DosoProprioPayload {
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
                        })
                    } else {
                        None
                    };
                    (wait_us, payload)
                };

                if let (Some(writer), Some(payload)) = (&mut shm_writer, &shm_payload) {
                    let _ = writer.publish(payload);
                }

                let tick_us = tick_start.elapsed().as_micros() as u64;
                rt_shared.record_rt_sample(mutex_wait_us, tick_us, jitter_us);

                #[cfg(target_os = "linux")]
                {
                    deadline_ns += RT_PERIOD_NS;
                    let before_sleep_ns = monotonic_now_ns();
                    if deadline_ns > before_sleep_ns {
                        sleep_until_monotonic_ns(deadline_ns, &rt_stop);
                        let wake_ns = monotonic_now_ns();
                        jitter_us = ((wake_ns - deadline_ns).max(0) / 1_000) as u64;
                    } else {
                        jitter_us = ((before_sleep_ns - deadline_ns) / 1_000) as u64;
                        deadline_ns = before_sleep_ns;
                    }
                }

                #[cfg(not(target_os = "linux"))]
                {
                    let before_sleep = Instant::now();
                    if next_deadline > before_sleep {
                        std::thread::sleep(next_deadline - before_sleep);
                        let wake = Instant::now();
                        jitter_us = wake.saturating_duration_since(next_deadline).as_micros() as u64;
                        next_deadline += period;
                        if next_deadline < wake {
                            next_deadline = wake + period;
                        }
                    } else {
                        jitter_us = before_sleep
                            .saturating_duration_since(next_deadline)
                            .as_micros() as u64;
                        next_deadline = before_sleep + period;
                    }
                }
            }
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
    let (local_status_tx, local_status_handle) = if let Some(addr_str) =
        optional_listen_addr(&args.local_status_listen)
    {
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

    // 3. Start unified customer-facing `doso.v1` BaseGateway (:5052) in the SAME single process
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
        robot_model: hw_params.robot_model.clone(),
        firmware_version: args.firmware_version.clone(),
        shm_enabled: args.shm_enabled,
        shm_proprio_name: args.shm_proprio_name.clone(),
        shm_cmd_name: args.shm_cmd_name.clone(),
        askme_voice: askme_cfg,
        ..BaseGatewayConfig::default()
    };
    let (gw_shutdown_tx, gw_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let (gw_addr, gw_handle) = spawn_base_gateway_on_listener(
        BaseGatewayService::new(gw_cfg),
        gw_listener,
        gw_shutdown_rx,
    )
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
            let wd = SystemdWatchdog::start(cfg)?;
            info!(
                interval_ms = wd.interval().map(|d| d.as_millis() as u64),
                "Systemd READY=1 notified and watchdog active"
            );
            Some(wd)
        }
        None => None,
    };

    let signal_name = wait_for_shutdown_signal().await;
    info!(signal = signal_name, "Shutting down doso-brainstem...");

    // Deterministic shutdown order:
    // 1) stop RT loop -> 2) passive damping + disable_motors(false) -> 3) watchdog.stop() -> 4) shut down servers
    stop_flag.store(true, Ordering::SeqCst);
    if let Err(err) = rt_handle.join() {
        warn!("RT loop thread joined after panic: {err:?}");
    }

    if emergency_disable_motors(&shared) {
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
