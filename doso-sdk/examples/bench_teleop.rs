use std::time::{Duration, Instant};

use doso_sdk::{Code, Config, DosoClient, TeleopVelocity};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        println!("bench_teleop <http(s)://endpoint> <frequency_hz> <duration_s>");
        println!("Publishes explicit teleop samples and reports local queue admission timing.");
        println!("The local fake-owner fixture lives in .local/evidence/teleop-50hz/run.py.");
        return Ok(());
    }
    if args.len() != 3 {
        return Err("Expected an explicit endpoint, frequency and duration; use --help".into());
    }
    let frequency: u32 = args[1].parse()?;
    let seconds: u32 = args[2].parse()?;
    if !(1..=100).contains(&frequency) || !(1..=30).contains(&seconds) {
        return Err("Frequency must be 1..100 Hz and duration 1..30 seconds".into());
    }
    let mut config = Config::new(&args[0]);
    config.client_id = "teleop-rate-bench".into();
    config.api_key = std::env::var("DOSO_API_KEY").ok();
    let client = DosoClient::with_config(config).await?;
    let mut session = client.motion().teleop().await?;
    let start = tokio::time::Instant::now();
    let period = Duration::from_secs_f64(1.0 / f64::from(frequency));
    let attempts = frequency * seconds;
    let mut queued = 0;
    let mut full = 0;
    let mut errors = 0;
    let mut rejected_replies = 0;
    let mut last_reply_id = String::new();
    let mut admission_us = Vec::with_capacity(attempts as usize);
    let mut max_lateness_ms = 0.0_f64;
    for sequence in 0..attempts {
        let deadline = start + period * sequence;
        tokio::time::sleep_until(deadline).await;
        max_lateness_ms = max_lateness_ms.max(
            tokio::time::Instant::now()
                .saturating_duration_since(deadline)
                .as_secs_f64()
                * 1000.0,
        );
        let request_id = format!("bench-{}", sequence + 1);
        let before = Instant::now();
        let result = session.publish_velocity(
            TeleopVelocity {
                vx_m_s: 0.2,
                deadman: true,
                ..Default::default()
            },
            Some(&request_id),
        );
        admission_us.push(before.elapsed().as_secs_f64() * 1_000_000.0);
        match result {
            Ok(_) => queued += 1,
            Err(error) if error.code() == Code::ResourceExhausted => full += 1,
            Err(error) => {
                errors += 1;
                eprintln!("{} {:?}", request_id, error);
            }
        }
        if let Some(reply) = session.latest_reply()? {
            if reply.request_id != last_reply_id {
                if !reply.accepted {
                    rejected_replies += 1;
                }
                last_reply_id = reply.request_id;
            }
        }
    }
    tokio::time::sleep_until(start + Duration::from_secs(u64::from(seconds))).await;
    let send_duration_s = start.elapsed().as_secs_f64();
    let held = session.hold(Some("bench-hold")).await?;
    let closed = session.close(Some("bench-close")).await?;
    admission_us.sort_by(f64::total_cmp);
    let p99 = admission_us[(admission_us.len() - 1) * 99 / 100];
    let max = admission_us.last().copied().unwrap_or(0.0);
    println!(
        "{{\"frequency_hz\":{},\"duration_s\":{},\"attempts\":{},\"queued\":{},\"full\":{},\"errors\":{},\"observed_rejected_replies\":{},\"send_duration_s\":{:.6},\"queued_hz\":{:.3},\"publish_p99_us\":{:.3},\"publish_max_us\":{:.3},\"max_lateness_ms\":{:.3},\"hold_accepted\":{},\"close_accepted\":{}}}",
        frequency, seconds, attempts, queued, full, errors, rejected_replies, send_duration_s,
        f64::from(queued) / send_duration_s, p99, max, max_lateness_ms, held.accepted,
        closed.accepted,
    );
    Ok(())
}
