use std::time::Duration;

use anyhow::Result;
use clap::{ArgAction, Parser};

use brainstem_dds_bridge::{serve_base_gateway, BaseGatewayConfig};
#[cfg(has_cyclonedds)]
use brainstem_dds_bridge::{BridgeConfig, BridgeRuntime};

#[derive(Debug, Parser)]
#[command(name = "brainstem-dds-bridge")]
#[command(
    about = "Board-A Brainstem sidecar: CycloneDDS IPC bridge + standalone doso.v1 Base Gateway (:5052/:5053)"
)]
struct Args {
    #[arg(
        long,
        env = "BRAINSTEM_ENDPOINT",
        default_value = "http://127.0.0.1:13145"
    )]
    brainstem_endpoint: String,

    #[arg(
        long,
        env = "BRAINSTEM_BASE_GATEWAY_LISTEN",
        default_value = "0.0.0.0:5052"
    )]
    gateway_listen: String,

    #[arg(
        long,
        env = "BRAINSTEM_DDS_ENABLED",
        action = ArgAction::Set,
        default_value_t = cfg!(has_cyclonedds)
    )]
    enable_dds: bool,

    #[arg(long, env = "DOSO_DEVICE_ID")]
    robot_id: Option<String>,

    #[arg(long, env = "DOSO_ROBOT_MODEL", default_value = "thunder")]
    robot_model: String,

    #[arg(long, env = "DOSO_FIRMWARE_VERSION", default_value = "0.1.0")]
    firmware_version: String,

    #[arg(long, env = "DOSO_API_KEY")]
    api_key: Option<String>,

    #[arg(long, env = "LINGTU_DDS_DOMAIN", default_value_t = 0)]
    domain_id: i32,

    #[arg(long, env = "LINGTU_HOST_BOOT_ID", default_value = "*")]
    expected_host_boot_id: String,

    #[arg(
        long,
        env = "BRAINSTEM_DDS_CROSS_BOARD_CLOCK",
        action = ArgAction::Set,
        default_value_t = true
    )]
    cross_board_clock: bool,

    #[arg(
        long,
        env = "BRAINSTEM_DDS_CLIENT_ID",
        default_value = "local-grpc@bsrl1"
    )]
    client_id: String,

    #[arg(long, env = "BRAINSTEM_DDS_POLL_HZ", default_value_t = 100.0)]
    poll_hz: f64,

    #[arg(long, env = "BRAINSTEM_DDS_WATCHDOG_MS", default_value_t = 200)]
    watchdog_ms: u64,

    #[arg(
        long,
        env = "DOSO_SHM_IPC_ENABLED",
        action = ArgAction::Set,
        default_value_t = false
    )]
    shm_enabled: bool,

    #[arg(
        long,
        env = "DOSO_SHM_PROPRIO_NAME",
        default_value = "/doso_proprio_v1"
    )]
    shm_proprio_name: String,

    #[arg(long, env = "DOSO_SHM_CMD_NAME", default_value = "/doso_ext_cmd_v1")]
    shm_cmd_name: String,

    #[arg(long, env = "DOSO_SHM_POLL_HZ", default_value_t = 200.0)]
    shm_poll_hz: f64,
}

fn gateway_enabled(raw: &str) -> bool {
    let trimmed = raw.trim();
    !trimmed.is_empty()
        && !trimmed.eq_ignore_ascii_case("off")
        && !trimmed.eq_ignore_ascii_case("none")
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let args = Args::parse();
    let gw_enabled = gateway_enabled(&args.gateway_listen);
    if !gw_enabled && !args.enable_dds {
        anyhow::bail!("at least one of --gateway-listen or --enable-dds must be active");
    }

    let (gw_stop_tx, gw_stop_rx) = tokio::sync::oneshot::channel();
    let (dds_stop_tx, dds_stop_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = gw_stop_tx.send(());
        let _ = dds_stop_tx.send(());
    });

    let mut handles: Vec<tokio::task::JoinHandle<Result<()>>> = Vec::new();

    let shm_poll_period = Duration::from_secs_f64(1.0 / args.shm_poll_hz.clamp(20.0, 1000.0));

    if gw_enabled {
        let gw_cfg = BaseGatewayConfig {
            brainstem_endpoint: args.brainstem_endpoint.clone(),
            brainstem_client_id: args.client_id.clone(),
            listen_addr: args.gateway_listen.clone(),
            robot_id: args.robot_id.clone(),
            robot_model: args.robot_model.clone(),
            firmware_version: args.firmware_version.clone(),
            api_key: args.api_key.clone(),
            low_level_endpoint_override: None,
            teleop_watchdog: Duration::from_millis(args.watchdog_ms.max(100)),
            shm_enabled: args.shm_enabled,
            shm_proprio_name: args.shm_proprio_name.clone(),
            shm_cmd_name: args.shm_cmd_name.clone(),
            shm_poll_period,
        };
        tracing::info!(
            listen = %gw_cfg.listen_addr,
            brainstem = %gw_cfg.brainstem_endpoint,
            shm_enabled = gw_cfg.shm_enabled,
            "starting Board-A doso.v1 Base Gateway"
        );
        handles.push(tokio::spawn(serve_base_gateway(gw_cfg, gw_stop_rx)));
    }

    if args.enable_dds {
        #[cfg(not(has_cyclonedds))]
        {
            let _ = dds_stop_rx;
            anyhow::bail!(
                "CycloneDDS support was not compiled in; pass --enable-dds false (or BRAINSTEM_DDS_ENABLED=false) to run standalone Board-A mode without Board B"
            );
        }

        #[cfg(has_cyclonedds)]
        {
            let bus = std::sync::Arc::new(
                brainstem_dds_bridge::dds_bus::cyclone::CycloneDdsBus::new(args.domain_id)?,
            );
            let cfg = BridgeConfig {
                brainstem_endpoint: args.brainstem_endpoint,
                client_id: args.client_id,
                expected_host_boot_id: args.expected_host_boot_id,
                cross_board_clock: args.cross_board_clock,
                command_timeout: Duration::from_millis(args.watchdog_ms),
                poll_period: Duration::from_secs_f64(1.0 / args.poll_hz.max(10.0)),
                shm_enabled: args.shm_enabled,
                shm_proprio_name: args.shm_proprio_name,
                shm_poll_period,
                ..BridgeConfig::default()
            };
            tracing::info!(
                endpoint = %cfg.brainstem_endpoint,
                domain_id = args.domain_id,
                shm_enabled = cfg.shm_enabled,
                "starting Board-A CycloneDDS bridge"
            );
            handles.push(tokio::spawn(
                BridgeRuntime::new(cfg, bus).run_until(dds_stop_rx),
            ));
        }
    } else {
        tracing::info!("CycloneDDS disabled (--enable-dds false); running standalone Board-A Base Gateway mode (no Board B required)");
    }

    for h in handles {
        h.await??;
    }
    Ok(())
}
