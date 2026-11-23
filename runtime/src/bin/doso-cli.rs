#![deny(unsafe_code)]

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use clap::{Parser, Subcommand};
use doso_runtime::policy::{LoadedPolicyPackage, PolicyManifest};
use doso_runtime::server::{PushPolicyRequest, RollbackRequest};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug, Parser)]
#[command(
    name = "doso-cli",
    about = "Fast <10s zero-restart policy deployment CLI for DOSO Brainstem"
)]
struct Cli {
    /// Policy admin HTTP endpoint (`host:port` or `http://host:port`).
    #[arg(long, env = "DOSO_ADMIN_URL", default_value = "http://127.0.0.1:5054")]
    admin_url: String,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List loaded policies and slot bindings on the robot.
    List,
    /// Inspect `policy.yaml` dimensions, joint mapping, and (if ONNX is present) warmup + Golden parity.
    Inspect { pkg_dir: PathBuf },
    /// Locally validate `policy.yaml` + ONNX + `golden.json` before uploading.
    Validate { pkg_dir: PathBuf },
    /// Push, Golden-verify, Shadow-evaluate, and hot-bind a policy package in <2 seconds (zero restart).
    Push {
        pkg_dir: PathBuf,
        #[arg(long)]
        slot: Option<String>,
        #[arg(long, default_value_t = 50)]
        shadow_frames: usize,
        #[arg(long, default_value_t = false)]
        activate: bool,
    },
    /// Instant 1-command rollback of an operator slot to its previous verified policy.
    Rollback {
        #[arg(long)]
        slot: String,
    },
}

fn joint_order_name(joint_map: &[usize; 16]) -> &'static str {
    match *joint_map {
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15] => "thunder_leg_major",
        [0, 3, 6, 9, 1, 4, 7, 10, 2, 5, 8, 11, 12, 13, 14, 15] => "isaaclab_joint_major",
        [3, 0, 9, 6, 4, 1, 10, 7, 5, 2, 11, 8, 13, 12, 15, 14] => "isaaclab_fl_first",
        [3, 4, 5, 0, 1, 2, 9, 10, 11, 6, 7, 8, 13, 12, 15, 14] => "go2_fl_first_leg_major",
        _ => "custom",
    }
}

fn parse_host_port(url: &str) -> Result<(String, String)> {
    let stripped = url
        .trim()
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let host_port = stripped.split('/').next().unwrap_or(stripped);
    if !host_port.contains(':') {
        Ok((format!("{host_port}:5054"), host_port.to_string()))
    } else {
        Ok((host_port.to_string(), host_port.to_string()))
    }
}

async fn raw_http(
    method: &str,
    admin_url: &str,
    path: &str,
    json_body: Option<&str>,
) -> Result<(u16, String)> {
    let (addr, host_hdr) = parse_host_port(admin_url)?;
    let mut stream = TcpStream::connect(&addr)
        .await
        .with_context(|| format!("failed connecting to {addr}"))?;
    let body_bytes = json_body.unwrap_or("").as_bytes();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host_hdr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body_bytes.len()
    );
    stream.write_all(req.as_bytes()).await?;
    if !body_bytes.is_empty() {
        stream.write_all(body_bytes).await?;
    }
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await?;
    let text = String::from_utf8_lossy(&raw);
    let Some((head, resp_body)) = text.split_once("\r\n\r\n") else {
        bail!("Malformed HTTP response from {addr}");
    };
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(500);
    Ok((status, resp_body.to_string()))
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Command::List => {
            let (code, body) = raw_http("GET", &cli.admin_url, "/v1/policies", None).await?;
            println!("HTTP {code}: {body}");
            if code >= 400 {
                bail!("List failed with HTTP {code}");
            }
        }
        Command::Inspect { pkg_dir } => {
            let yaml_path = pkg_dir.join("policy.yaml");
            let yaml = std::fs::read_to_string(&yaml_path)
                .with_context(|| format!("missing policy.yaml in {}", pkg_dir.display()))?;
            let manifest = PolicyManifest::from_yaml_str(&yaml)?;
            let onnx_path = pkg_dir.join(&manifest.model.file);
            let golden_path = pkg_dir.join("golden.json");

            println!("Package:     {}", pkg_dir.display());
            println!(
                "Policy:      {} (kind={:?}, step_dt_s={:.3}s)",
                manifest.name, manifest.kind, manifest.model.step_dt_s
            );
            println!(
                "Model file:  {} (exists={}, sha256={})",
                manifest.model.file,
                onnx_path.is_file(),
                manifest.model.sha256.as_deref().unwrap_or("none")
            );
            println!(
                "Joint order: {} {:?}",
                joint_order_name(&manifest.posture.joint_map),
                manifest.posture.joint_map
            );
            println!("Inputs ({}):", manifest.inputs.len());
            for (idx, inp) in manifest.inputs.iter().enumerate() {
                println!(
                    "  [{idx}] tensor='{}' frame_dim={} x history={} ({:?}, {:?}, {:?}) -> total_dim={}",
                    inp.tensor,
                    inp.single_frame_dim(),
                    inp.history.length,
                    inp.history.layout,
                    inp.history.order,
                    inp.history.padding,
                    inp.total_dim()
                );
            }
            if manifest.outputs.aux_tensors.is_empty() {
                println!(
                    "Outputs:     action_tensor='{}'",
                    manifest.outputs.action_tensor
                );
            } else {
                println!(
                    "Outputs:     action_tensor='{}' aux_tensors={:?}",
                    manifest.outputs.action_tensor, manifest.outputs.aux_tensors
                );
            }
            println!("Golden file: {}", golden_path.is_file());

            if onnx_path.is_file() {
                let t0 = Instant::now();
                let pkg = LoadedPolicyPackage::load_from_dir(&pkg_dir)?;
                println!(
                    "ORT verify:  OK in {:.1}ms (backend={} sha256={}.. warmup={}us golden_steps={} max_err={:.2e})",
                    t0.elapsed().as_secs_f64() * 1000.0,
                    pkg.report.backend,
                    &pkg.report.sha256[..16],
                    pkg.report.warmup_last_us,
                    pkg.report.golden_steps_checked,
                    pkg.report.golden_max_abs_err
                );
            } else {
                println!(
                    "ORT verify:  skipped (ONNX file not present at {})",
                    onnx_path.display()
                );
            }
        }
        Command::Validate { pkg_dir } => {
            let t0 = Instant::now();
            let pkg = LoadedPolicyPackage::load_from_dir(&pkg_dir)?;
            println!(
                "LOCAL_VALIDATE_OK in {:.1}ms: name={} sha256={}.. warmup={}us golden_steps={} max_err={:.2e}",
                t0.elapsed().as_secs_f64() * 1000.0,
                pkg.report.name,
                &pkg.report.sha256[..16],
                pkg.report.warmup_last_us,
                pkg.report.golden_steps_checked,
                pkg.report.golden_max_abs_err
            );
        }
        Command::Push {
            pkg_dir,
            slot,
            shadow_frames,
            activate,
        } => {
            let t0 = Instant::now();
            let yaml = std::fs::read_to_string(pkg_dir.join("policy.yaml"))
                .with_context(|| format!("missing policy.yaml in {}", pkg_dir.display()))?;
            let manifest = PolicyManifest::from_yaml_str(&yaml)?;
            let onnx_bytes = std::fs::read(pkg_dir.join(&manifest.model.file))?;
            let b64 = B64.encode(&onnx_bytes);
            let golden_opt = std::fs::read_to_string(pkg_dir.join("golden.json")).ok();

            let payload = serde_json::to_string(&PushPolicyRequest {
                manifest_yaml: yaml,
                onnx_base64: b64,
                golden_json: golden_opt,
                slot,
                shadow_frames,
                activate,
            })?;
            let (code, body) =
                raw_http("POST", &cli.admin_url, "/v1/policies/push", Some(&payload)).await?;
            println!(
                "HOT_DEPLOY_RESULT (HTTP {code}, {:.1}ms): {body}",
                t0.elapsed().as_secs_f64() * 1000.0
            );
            if code >= 400 {
                bail!("Hot deploy rejected (HTTP {code})");
            }
        }
        Command::Rollback { slot } => {
            let payload = serde_json::to_string(&RollbackRequest { slot })?;
            let (code, body) = raw_http(
                "POST",
                &cli.admin_url,
                "/v1/policies/rollback",
                Some(&payload),
            )
            .await?;
            println!("ROLLBACK_RESULT (HTTP {code}): {body}");
            if code >= 400 {
                bail!("Rollback failed (HTTP {code})");
            }
        }
    }
    Ok(())
}
