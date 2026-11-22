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
        ..RunnerConfig::default()
    };

    let (stop_tx, stop_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = stop_tx.send(());
    });

    let mut runner = RlRunner::new(runner_cfg, Box::new(backend))?;
    let report = runner.run(&robot, stop_rx).await?;
    tracing::info!(?report, "doso-rl-runner finished cleanly");
    Ok(())
}
