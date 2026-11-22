use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use doso_sdk::RobotClient;
use tokio::sync::oneshot;

use doso_rl_runner::{DeployConfig, OrtDynamicBackend, RlRunner, RunnerConfig};

#[derive(Debug, Parser)]
#[command(name = "doso-rl-runner")]
#[command(
    about = "Official DOSO Sim2Sim/Sim2Real ONNX RL policy runner over RobotClient::low_level()"
)]
struct Args {
    #[arg(long, env = "DOSO_ENDPOINT", default_value = "http://127.0.0.1:5052")]
    endpoint: String,

    #[arg(long, env = "DOSO_DEPLOY_YAML")]
    deploy_yaml: PathBuf,

    #[arg(long, env = "ONNXRUNTIME_DLL_PATH")]
    onnxruntime_dll: Option<PathBuf>,

    #[arg(long, default_value_t = 0.0)]
    vx: f64,

    #[arg(long, default_value_t = 0.0)]
    vy: f64,

    #[arg(long, default_value_t = 0.0)]
    wz: f64,

    #[arg(long)]
    max_steps: Option<u64>,

    #[arg(long, default_value_t = false)]
    skip_fix_stand: bool,

    #[arg(long, env = "DOSO_SHM_IPC_ENABLED", default_value_t = false)]
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let args = Args::parse();
    let deploy = DeployConfig::from_path(&args.deploy_yaml)?;
    let backend = OrtDynamicBackend::load(&deploy.policy_path, args.onnxruntime_dll.as_deref())?;
    let robot = RobotClient::connect(&args.endpoint).await?;

    let runner_cfg = RunnerConfig {
        deploy,
        target_cmd: [args.vx, args.vy, args.wz],
        max_steps: args.max_steps,
        skip_fix_stand: args.skip_fix_stand,
        shm_enabled: args.shm_enabled,
        shm_proprio_name: args.shm_proprio_name,
        shm_cmd_name: args.shm_cmd_name,
        ..RunnerConfig::default()
    };

    let (stop_tx, stop_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = stop_tx.send(());
    });

    let mut runner = RlRunner::new(runner_cfg, Box::new(backend))?;
    let report = runner.run(&robot, stop_rx).await?;
    tracing::info!(
        mode = ?report.final_mode,
        shm = report.shm_data_plane_active,
        fix_stand_steps = report.fix_stand_steps,
        policy_steps = report.policy_steps,
        step_p50_us = report.step_latency_us.p50_us,
        step_p99_us = report.step_latency_us.p99_us,
        step_p999_us = report.step_latency_us.p999_us,
        infer_p99_us = report.infer_latency_us.p99_us,
        jitter_p99_us = report.tick_jitter_us.p99_us,
        reason = %report.stop_reason,
        "doso-rl-runner finished cleanly"
    );
    Ok(())
}
