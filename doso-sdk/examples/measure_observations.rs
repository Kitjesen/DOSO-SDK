use std::future::{poll_fn, Future};
use std::task::Poll;
use std::time::Duration;

use doso_sdk::{Config, DosoClient};
use tokio::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const IDLE_LIMIT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
struct Mode {
    imu: bool,
    joints: bool,
}

impl Mode {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "both" => Ok(Self {
                imu: true,
                joints: true,
            }),
            "imu" => Ok(Self {
                imu: true,
                joints: false,
            }),
            "joints" => Ok(Self {
                imu: false,
                joints: true,
            }),
            _ => Err("Mode must be both, imu or joints".into()),
        }
    }

    fn stats<'a>(self, imu: &'a Stats, joints: &'a [Stats; 16]) -> impl Iterator<Item = &'a Stats> {
        joints
            .iter()
            .filter(move |_| self.joints)
            .chain(std::iter::once(imu).filter(move |_| self.imu))
    }
}

struct Stats {
    count: u64,
    last: Instant,
    max_gap: Duration,
    source_first_ns: Option<i128>,
    source_last_ns: Option<i128>,
    source_count: u64,
}

impl Stats {
    fn new(start: Instant) -> Self {
        Self {
            count: 0,
            last: start,
            max_gap: Duration::ZERO,
            source_first_ns: None,
            source_last_ns: None,
            source_count: 0,
        }
    }

    fn record(&mut self, now: Instant, source: Option<&prost_types::Duration>) {
        self.max_gap = self.max_gap.max(now - self.last);
        self.last = now;
        self.count += 1;
        if let Some(source) = source {
            let ns = i128::from(source.seconds) * 1_000_000_000 + i128::from(source.nanos);
            self.source_first_ns.get_or_insert(ns);
            self.source_last_ns = Some(ns);
            self.source_count += 1;
        }
    }

    fn print(&self, name: &str, start: Instant, end: Instant) {
        let source_span = match (self.source_first_ns, self.source_last_ns) {
            (Some(first), Some(last)) if self.source_count >= 2 => {
                format!("{:.6}", (last - first) as f64 / 1e9)
            }
            _ => "unavailable".into(),
        };
        println!(
            "{name}: messages={} hz={:.3} max_receive_gap_ms={:.3} source_samples={} source_span_s={source_span}",
            self.count,
            self.count as f64 / (end - start).as_secs_f64(),
            self.max_gap.max(end - self.last).as_secs_f64() * 1000.0,
            self.source_count,
        );
        println!(
            "{name}: source_first_ns={:?} source_last_ns={:?}",
            self.source_first_ns, self.source_last_ns
        );
    }

    fn print_consumer_idle(&self, name: &str, now: Instant) {
        let idle = now - self.last;
        if idle >= IDLE_LIMIT {
            eprintln!(
                "consumer_timeout trigger={name} consumer_idle_ms={:.3} last_reported_source_ns={:?}",
                idle.as_secs_f64() * 1000.0,
                self.source_last_ns,
            );
        }
    }
}

async fn probe_once<F>(future: Option<F>) -> String
where
    F: Future<Output = std::result::Result<Option<Option<prost_types::Duration>>, tonic::Status>>,
{
    let Some(future) = future else {
        return "not_selected".into();
    };
    tokio::pin!(future);
    poll_fn(|context| {
        Poll::Ready(match future.as_mut().poll(context) {
            Poll::Ready(Ok(Some(source))) => {
                let next_source = source.map_or_else(
                    || "unknown".into(),
                    |source| {
                        (i128::from(source.seconds) * 1_000_000_000 + i128::from(source.nanos))
                            .to_string()
                    },
                );
                format!("sample_ready(next_source_ns={next_source})")
            }
            Poll::Ready(Ok(None)) => "eof".into(),
            Poll::Ready(Err(error)) => format!("rpc_error({:?})", error.code()),
            Poll::Pending => "no_sample_ready".into(),
        })
    })
    .await
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        println!("measure_observations <http(s)://gateway:5052> [seconds=10] [both|imu|joints]");
        println!("Read-only selected streams (default: both); duration: 1..3600 whole seconds.");
        println!("Fails on EOF/errors, missing data, or 2 seconds without consuming a selected IMU/joint.");
        println!("Consumer timeout reports triggers and polls each selected read once without waiting; still fails.");
        println!("DOSO_API_KEY: optional API key; DOSO_TLS_CA_FILE: optional HTTPS PEM CA.");
        return Ok(());
    }
    if !(1..=3).contains(&args.len()) {
        return Err("Expected an endpoint, optional duration and mode; use --help".into());
    }
    let seconds: u32 = args.get(1).map_or(Ok(10), |value| value.parse())?;
    if !(1..=3600).contains(&seconds) {
        return Err("Duration must be 1..3600 whole seconds".into());
    }
    let mode_name = args.get(2).map_or("both", String::as_str);
    let mode = Mode::parse(mode_name)?;
    let mut config = Config::new(&args[0]);
    config.api_key = std::env::var("DOSO_API_KEY").ok();
    if let Some(path) = std::env::var_os("DOSO_TLS_CA_FILE") {
        config.tls_ca_certificate = Some(std::fs::read(path)?);
    }
    let robot = DosoClient::with_config(config).await?;
    let info = robot.system().info();
    println!(
        "system_info robot_id={:?} robot_version={:?} firmware_version={:?}",
        info.robot_id, info.robot_version, info.firmware_version
    );
    let observations = robot.observations();
    let (mut imu, mut joints) = tokio::time::timeout(IDLE_LIMIT, async {
        tokio::try_join!(
            async {
                if mode.imu {
                    observations.watch_imu().await.map(Some)
                } else {
                    Ok(None)
                }
            },
            async {
                if mode.joints {
                    observations.watch_joints().await.map(Some)
                } else {
                    Ok(None)
                }
            },
        )
    })
    .await
    .map_err(|_| "Observation subscriptions did not start within 2 seconds")??;

    let start = Instant::now();
    let deadline = start + Duration::from_secs(u64::from(seconds));
    let mut imu_stats = Stats::new(start);
    let mut joint_messages = Stats::new(start);
    let mut joint_stats: [Stats; 16] = std::array::from_fn(|_| Stats::new(start));
    let mut acceleration_count = 0_u64;
    let mut consumer_timeout = false;
    let result: Result<()> = async {
        loop {
            let idle_deadline = mode.stats(&imu_stats, &joint_stats)
                .map(|stats| stats.last)
                .min()
                .unwrap()
                + IDLE_LIMIT;
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(deadline) => break,
                _ = tokio::time::sleep_until(idle_deadline) => {
                    consumer_timeout = true;
                    return Err("Consumer did not process a selected IMU/joint for 2 seconds".into());
                }
                sample = async { imu.as_mut().unwrap().message().await }, if mode.imu => {
                    let sample = sample?.ok_or("IMU source stream ended before the window completed")?;
                    imu_stats.record(Instant::now(), sample.source_elapsed.as_ref());
                    acceleration_count += u64::from(sample.linear_acceleration_m_s2.is_some());
                }
                sample = async { joints.as_mut().unwrap().message().await }, if mode.joints => {
                    let sample = sample?.ok_or("Joint source stream ended before the window completed")?;
                    if sample.joints.is_empty() {
                        return Err("Joint source returned an empty event".into());
                    }
                    let now = Instant::now();
                    joint_messages.record(now, sample.source_elapsed.as_ref());
                    for joint in sample.joints {
                        let stats = joint_stats.get_mut(joint.id as usize).ok_or("Joint ID is outside 0..15")?;
                        stats.record(now, sample.source_elapsed.as_ref());
                    }
                }
            }
        }
        let now = Instant::now();
        if mode.stats(&imu_stats, &joint_stats).any(|stats| now - stats.last >= IDLE_LIMIT) {
            consumer_timeout = true;
            return Err("Consumer did not process a selected IMU/joint for 2 seconds".into());
        }
        if mode.stats(&imu_stats, &joint_stats).any(|stats| stats.count == 0) {
            return Err("Window completed without data for a selected IMU/joint".into());
        }
        Ok(())
    }
    .await;
    let end = Instant::now();
    if consumer_timeout {
        if mode.imu {
            imu_stats.print_consumer_idle("imu", end);
        }
        if mode.joints {
            for (id, stats) in joint_stats.iter().enumerate() {
                stats.print_consumer_idle(&format!("joint[{id}]"), end);
            }
        }
        let (imu_probe, joints_probe) = tokio::join!(
            probe_once(imu.as_mut().map(|stream| async move {
                stream
                    .message()
                    .await
                    .map(|sample| sample.map(|sample| sample.source_elapsed))
            })),
            probe_once(joints.as_mut().map(|stream| async move {
                stream
                    .message()
                    .await
                    .map(|sample| sample.map(|sample| sample.source_elapsed))
            })),
        );
        eprintln!("read_after_consumer_timeout imu={imu_probe} joints={joints_probe}");
    }
    drop(imu);
    drop(joints);
    println!(
        "requested_window_s={seconds} mode={mode_name} observed_window_s={:.6} completed={}",
        (end - start).as_secs_f64(),
        result.is_ok(),
    );
    if mode.imu {
        imu_stats.print("IMU", start, end);
        println!(
            "IMU linear_acceleration_present={acceleration_count}/{}",
            imu_stats.count
        );
    }
    if mode.joints {
        joint_messages.print("joint_events", start, end);
        for (id, stats) in joint_stats.iter().enumerate() {
            stats.print(&format!("joint[{id}]"), start, end);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_probe_reports_ready_eof_error_pending_and_unselected_without_waiting() {
        use std::future::{pending, ready};

        assert_eq!(
            probe_once(Some(ready(Ok(Some(Some(prost_types::Duration {
                seconds: 4,
                nanos: 5,
            }))))))
            .await,
            "sample_ready(next_source_ns=4000000005)",
        );
        assert_eq!(
            probe_once(Some(ready(Ok(Some(None))))).await,
            "sample_ready(next_source_ns=unknown)"
        );
        assert_eq!(probe_once(Some(ready(Ok(None)))).await, "eof");
        assert_eq!(
            probe_once(Some(ready(Err(tonic::Status::unavailable("test"))))).await,
            "rpc_error(Unavailable)"
        );
        assert_eq!(
            probe_once(Some(pending::<
                std::result::Result<Option<Option<prost_types::Duration>>, tonic::Status>,
            >()))
            .await,
            "no_sample_ready",
        );
        assert_eq!(
            probe_once(None::<std::future::Ready<_>>).await,
            "not_selected"
        );
    }

    #[tokio::test]
    async fn expired_consumer_timer_can_win_while_a_buffered_sample_is_ready() {
        let expired = tokio::time::sleep_until(Instant::now() - IDLE_LIMIT);
        let mut buffered = std::future::ready(Ok(Some(None)));
        let consumer_timeout = tokio::select! {
            biased;
            _ = expired => true,
            _ = &mut buffered => false,
        };
        assert!(consumer_timeout);
        assert_eq!(
            probe_once(Some(buffered)).await,
            "sample_ready(next_source_ns=unknown)"
        );
    }

    #[test]
    fn modes_select_only_the_requested_streams_and_stats() {
        let start = Instant::now();
        let mut imu = Stats::new(start);
        imu.record(start + Duration::from_secs(1), None);
        let mut joints: [Stats; 16] = std::array::from_fn(|_| Stats::new(start));
        let imu_only = Mode::parse("imu").unwrap();
        assert!(imu_only.imu && !imu_only.joints);
        assert_eq!(imu_only.stats(&imu, &joints).count(), 1);
        assert!(imu_only.stats(&imu, &joints).all(|stats| stats.count > 0));
        assert_eq!(
            imu_only.stats(&imu, &joints).map(|stats| stats.last).min(),
            Some(imu.last)
        );

        for stats in &mut joints {
            stats.record(start + Duration::from_secs(2), None);
        }
        let joints_only = Mode::parse("joints").unwrap();
        assert!(!joints_only.imu && joints_only.joints);
        assert_eq!(joints_only.stats(&imu, &joints).count(), 16);
        assert!(joints_only
            .stats(&imu, &joints)
            .all(|stats| stats.last > imu.last));
        let both = Mode::parse("both").unwrap();
        assert!(both.imu && both.joints);
        assert_eq!(both.stats(&imu, &joints).count(), 17);
        assert!(Mode::parse("unknown").is_err());
    }

    #[test]
    fn counts_receive_gaps_and_only_reported_source_times() {
        let start = Instant::now();
        let mut stats = Stats::new(start);
        stats.record(start + Duration::from_millis(10), None);
        stats.record(
            start + Duration::from_millis(30),
            Some(&prost_types::Duration {
                seconds: 7,
                nanos: 900_000_000,
            }),
        );
        stats.record(
            start + Duration::from_millis(40),
            Some(&prost_types::Duration {
                seconds: 8,
                nanos: 100_000_000,
            }),
        );
        assert_eq!(stats.count, 3);
        assert_eq!(stats.source_count, 2);
        assert_eq!(stats.max_gap, Duration::from_millis(20));
        assert_eq!(
            stats.source_last_ns.unwrap() - stats.source_first_ns.unwrap(),
            200_000_000
        );
    }
}
