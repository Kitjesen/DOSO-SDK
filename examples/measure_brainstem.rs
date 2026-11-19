use std::future::Future;
use std::time::Duration;

use doso_sdk::brainstem::rpc::joint::Data;
use doso_sdk::BrainstemClient;
use tokio::sync::watch;
use tokio::time::{sleep_until, Instant, MissedTickBehavior};

#[path = "support/brainstem.rs"]
mod brainstem;

type BoxResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const DEFAULT_SECONDS: u64 = 900;
const DEFAULT_QUERY_INTERVAL_SECONDS: u64 = 10;
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);
const MIN_HZ: f64 = 98.0;
const MAX_HZ: f64 = 102.0;
const MAX_GAP: Duration = Duration::from_secs(2);
const JOINTS: usize = 16;

struct ReceiveGaps {
    last: Instant,
    max: Duration,
    max_ends_at: Instant,
    long_gaps: u64,
    first_long_gap: Option<Duration>,
}

impl ReceiveGaps {
    fn new(start: Instant) -> Self {
        Self {
            last: start,
            max: Duration::ZERO,
            max_ends_at: start,
            long_gaps: 0,
            first_long_gap: None,
        }
    }

    fn observe(&mut self, now: Instant) -> bool {
        let gap = now - self.last;
        self.last = now;
        if gap > MAX_GAP {
            self.long_gaps += 1;
            self.first_long_gap.get_or_insert(gap);
        }
        if gap <= self.max {
            return false;
        }
        self.max = gap;
        self.max_ends_at = now;
        true
    }
}

struct SampleStats {
    count: u64,
    received: ReceiveGaps,
    source_first: Option<Duration>,
    source_last: Option<Duration>,
    source_max_gap: Option<Duration>,
    source_at_receive_max: Option<Duration>,
    invalid_timestamps: u64,
}

impl SampleStats {
    fn new(start: Instant) -> Self {
        Self {
            count: 0,
            received: ReceiveGaps::new(start),
            source_first: None,
            source_last: None,
            source_max_gap: None,
            source_at_receive_max: None,
            invalid_timestamps: 0,
        }
    }

    fn record(&mut self, now: Instant, timestamp: Option<&prost_types::Duration>) {
        self.count += 1;
        let mut source_gap = None;
        let source = timestamp
            .filter(|source| source.seconds >= 0 && (0..1_000_000_000).contains(&source.nanos))
            .map(|source| Duration::new(source.seconds as u64, source.nanos as u32));
        if let Some(source) =
            source.filter(|source| self.source_last.is_none_or(|last| *source >= last))
        {
            source_gap = self.source_last.map(|last| source - last);
            self.source_first.get_or_insert(source);
            self.source_last = Some(source);
            self.source_max_gap = self.source_max_gap.max(source_gap);
        } else {
            self.invalid_timestamps += 1;
        }
        if self.received.observe(now) {
            self.source_at_receive_max = source_gap;
        }
    }

    fn finish(&mut self, end: Instant) {
        if self.received.observe(end) {
            self.source_at_receive_max = None;
        }
    }

    fn source_ok(&self) -> bool {
        self.invalid_timestamps == 0
            && self
                .source_max_gap
                .is_some_and(|gap| !gap.is_zero() && gap <= MAX_GAP)
    }

    fn source_span_seconds(&self) -> Option<f64> {
        self.source_first
            .zip(self.source_last)
            .map(|(first, last)| (last - first).as_secs_f64())
    }

    fn report(&self, name: &str, start: Instant) {
        let ms = |gap: Duration| gap.as_secs_f64() * 1000.0;
        println!(
            "{name} samples={} receive_max_gap_ms={:.3} receive_long_gaps={} receive_first_long_gap_ms={:?} source_max_gap_ms={:?} source_gap_at_receive_max_ms={:?} invalid_timestamps={} receive_max_gap_end_s={:.3}",
            self.count,
            ms(self.received.max),
            self.received.long_gaps,
            self.received.first_long_gap.map(ms),
            self.source_max_gap.map(ms),
            self.source_at_receive_max.map(ms),
            self.invalid_timestamps,
            self.received.max_ends_at.duration_since(start).as_secs_f64(),
        );
    }
}

struct StreamStats {
    messages: u64,
    received: ReceiveGaps,
    complete: bool,
}

struct QueryStats {
    cycles: u64,
    calls: u64,
    failures: u64,
    malformed: u64,
    voltage_values: u64,
    voltage_zero_values: u64,
    voltage_min: Option<f64>,
    voltage_max: Option<f64>,
    motor_samples: u64,
    motor_online_min: Option<usize>,
    motor_temperature_max: Option<f64>,
    control_samples: u64,
    profile_samples: u64,
}

impl QueryStats {
    fn new() -> Self {
        Self {
            cycles: 0,
            calls: 0,
            failures: 0,
            malformed: 0,
            voltage_values: 0,
            voltage_zero_values: 0,
            voltage_min: None,
            voltage_max: None,
            motor_samples: 0,
            motor_online_min: None,
            motor_temperature_max: None,
            control_samples: 0,
            profile_samples: 0,
        }
    }

    fn is_complete(&self, expected_cycles: u64) -> bool {
        self.failures == 0
            && self.malformed == 0
            && self.cycles == expected_cycles
            && self.calls == expected_cycles * 4
            && self.control_samples == expected_cycles
            && self.profile_samples == expected_cycles
            && self.motor_samples == expected_cycles
            && self.voltage_values == expected_cycles * JOINTS as u64
    }
}

async fn watch_stream<T>(
    name: &str,
    subscribe: impl Future<Output = doso_sdk::Result<tonic::Streaming<T>>>,
    start: Instant,
    deadline: Instant,
    stop: &watch::Sender<bool>,
    mut record: impl FnMut(Instant, T),
) -> StreamStats {
    let mut messages = 0;
    let mut received = ReceiveGaps::new(start);
    let mut cancelled = stop.subscribe();
    let complete = tokio::select! {
        biased;
        _ = cancelled.wait_for(|stopped| *stopped) => false,
        _ = sleep_until(deadline) => true,
        error = async {
            let mut stream = match subscribe.await {
                Ok(stream) => stream,
                Err(error) => return format!("subscribe: {error:?}"),
            };
            loop {
                match stream.message().await {
                    Ok(Some(sample)) => {
                        let now = Instant::now();
                        messages += 1;
                        received.observe(now);
                        record(now, sample);
                        tokio::task::consume_budget().await;
                    }
                    Ok(None) => return "stream ended".to_string(),
                    Err(error) => return format!("stream: {error:?}"),
                }
            }
        } => {
            eprintln!("{name}_error={error}");
            stop.send_replace(true);
            false
        }
    };
    received.observe(Instant::now().min(deadline));
    StreamStats {
        messages,
        received,
        complete,
    }
}

async fn poll_queries(
    queries: Option<(BrainstemClient, Duration)>,
    start: Instant,
    deadline: Instant,
    stop: &watch::Sender<bool>,
) -> QueryStats {
    let mut stats = QueryStats::new();
    let Some((client, interval)) = queries else {
        return stats;
    };
    let mut cancelled = stop.subscribe();
    let mut ticker = tokio::time::interval_at(start, interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            _ = cancelled.wait_for(|stopped| *stopped) => break,
            _ = sleep_until(deadline) => break,
            _ = async {
                ticker.tick().await;
                stats.cycles += 1;

                stats.calls += 1;
                match tokio::time::timeout(QUERY_TIMEOUT, client.control_status()).await {
                    Err(_) => {
                        stats.failures += 1;
                        eprintln!("control_status_error=timeout");
                    }
                    Ok(Err(error)) => {
                        stats.failures += 1;
                        eprintln!("control_status_error={error:?}");
                    }
                    Ok(Ok(status)) if status.fsm.is_some() => stats.control_samples += 1,
                    Ok(Ok(_)) => stats.malformed += 1,
                }

                stats.calls += 1;
                match tokio::time::timeout(QUERY_TIMEOUT, client.profile()).await {
                    Err(_) => {
                        stats.failures += 1;
                        eprintln!("profile_error=timeout");
                    }
                    Ok(Err(error)) => {
                        stats.failures += 1;
                        eprintln!("profile_error={error:?}");
                    }
                    Ok(Ok(profile)) if !profile.current.is_empty() => stats.profile_samples += 1,
                    Ok(Ok(_)) => stats.malformed += 1,
                }

                stats.calls += 1;
                match tokio::time::timeout(QUERY_TIMEOUT, client.voltage()).await {
                    Err(_) => {
                        stats.failures += 1;
                        eprintln!("voltage_error=timeout");
                    }
                    Ok(Err(error)) => {
                        stats.failures += 1;
                        eprintln!("voltage_error={error:?}");
                    }
                    Ok(Ok(voltage))
                        if voltage.values.len() == 16
                            && voltage.values.iter().all(|value| value.is_finite()) =>
                    {
                        stats.voltage_values += 16;
                        stats.voltage_zero_values += voltage.values.iter().filter(|value| **value == 0.0).count() as u64;
                        let min = voltage.values.iter().copied().fold(f64::INFINITY, f64::min);
                        let max = voltage.values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                        stats.voltage_min = Some(stats.voltage_min.map_or(min, |value| value.min(min)));
                        stats.voltage_max = Some(stats.voltage_max.map_or(max, |value| value.max(max)));
                    }
                    Ok(Ok(_)) => stats.malformed += 1,
                }

                stats.calls += 1;
                match tokio::time::timeout(QUERY_TIMEOUT, client.motor_status()).await {
                    Err(_) => {
                        stats.failures += 1;
                        eprintln!("motor_status_error=timeout");
                    }
                    Ok(Err(error)) => {
                        stats.failures += 1;
                        eprintln!("motor_status_error={error:?}");
                    }
                    Ok(Ok(status))
                        if status.motors.len() == 16
                            && status.motors.iter().all(|motor| {
                                motor.temperature.is_finite()
                                    && motor.position.is_finite()
                                    && motor.velocity.is_finite()
                                    && motor.torque.is_finite()
                            }) =>
                    {
                        let online = status.motors.iter().filter(|motor| motor.online).count();
                        stats.motor_samples += 1;
                        stats.motor_online_min = Some(stats.motor_online_min.map_or(online, |min| min.min(online)));
                        let max_temperature = status
                            .motors
                            .iter()
                            .map(|motor| motor.temperature)
                            .fold(f64::NEG_INFINITY, f64::max);
                        stats.motor_temperature_max = Some(
                            stats
                                .motor_temperature_max
                                .map_or(max_temperature, |value| value.max(max_temperature)),
                        );
                    }
                    Ok(Ok(_)) => stats.malformed += 1,
                }
            } => {
                if stats.failures > 0 || stats.malformed > 0 {
                    stop.send_replace(true);
                    break;
                }
            }
        }
    }
    stats
}

fn parse_u64(value: Option<&String>, name: &str, default: u64, max: u64) -> BoxResult<u64> {
    let value = value.map_or(Ok(default), |value| value.parse())?;
    if value == 0 || value > max {
        return Err(format!("{name} must be between 1 and {max}").into());
    }
    Ok(value)
}

#[tokio::main]
async fn main() -> BoxResult<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        println!(
            "measure_brainstem <http(s)://robot:13145> [seconds=900] [query_interval_seconds=10]"
        );
        println!("Read-only Brainstem stability window for IMU, joints, voltage, motors, control status and profile.");
        println!("The default window is 15 minutes. The command never acquires control or sends motion commands.");
        println!("Set query_interval_seconds to 0 for a stream-only window.");
        println!("For HTTPS with mutual TLS, set DOSO_BRAINSTEM_CERT_DIR.");
        return Ok(());
    }
    if !(1..=3).contains(&args.len()) {
        return Err(
            "Expected an endpoint, optional duration and query interval; use --help".into(),
        );
    }
    let seconds = parse_u64(args.get(1), "seconds", DEFAULT_SECONDS, 86_400)?;
    let query_interval = args
        .get(2)
        .map_or(Ok(DEFAULT_QUERY_INTERVAL_SECONDS), |value| value.parse())?;
    if query_interval > 300 {
        return Err("query_interval_seconds must be between 0 and 300".into());
    }
    let expected_query_cycles = if query_interval == 0 {
        0
    } else {
        seconds.div_ceil(query_interval)
    };
    let queries_enabled = expected_query_cycles > 0;

    let stream_client = brainstem::connect(&args[0]).await?;
    let queries = if queries_enabled {
        Some((
            brainstem::connect(&args[0]).await?,
            Duration::from_secs(query_interval),
        ))
    } else {
        None
    };
    let start = Instant::now();
    let deadline = start + Duration::from_secs(seconds);
    let (stop, _) = watch::channel(false);
    let mut imu = SampleStats::new(start);
    let mut joints: [SampleStats; JOINTS] = std::array::from_fn(|_| SampleStats::new(start));
    let mut malformed_joints = 0;
    println!(
        "endpoint={} duration_s={} query_interval_s={}",
        args[0], seconds, query_interval
    );

    let (imu_stream, joint_stream, queries) = tokio::join!(
        watch_stream(
            "imu",
            stream_client.listen_imu(),
            start,
            deadline,
            &stop,
            |now, sample| imu.record(now, sample.timestamp.as_ref()),
        ),
        watch_stream(
            "joint",
            stream_client.listen_joint(),
            start,
            deadline,
            &stop,
            |now, sample| {
                let timestamp = sample.timestamp.as_ref();
                match sample.data {
                    Some(Data::SingleJoint(joint)) => {
                        if let Some(axis) = joints.get_mut(joint.id as usize) {
                            axis.record(now, timestamp);
                        } else {
                            malformed_joints += 1;
                        }
                    }
                    Some(Data::AllJoints(frame))
                        if frame
                            .position
                            .as_ref()
                            .is_some_and(|position| position.values.len() == JOINTS) =>
                    {
                        for axis in &mut joints {
                            axis.record(now, timestamp);
                        }
                    }
                    _ => malformed_joints += 1,
                }
            },
        ),
        poll_queries(queries, start, deadline, &stop),
    );
    let end = Instant::now().min(deadline);
    let observed = end.duration_since(start).as_secs_f64();
    imu.finish(end);
    for axis in &mut joints {
        axis.finish(end);
    }
    let joint_counts = joints.each_ref().map(|axis| axis.count);
    let joint_axis_hz = joint_counts.map(|count| count as f64 / observed);
    let imu_hz = imu.count as f64 / observed;
    let min_joint_hz = joint_axis_hz.iter().copied().fold(f64::INFINITY, f64::min);
    let max_joint_hz = joint_axis_hz.iter().copied().fold(0.0, f64::max);
    let frequency_ok = (MIN_HZ..=MAX_HZ).contains(&imu_hz)
        && (MIN_HZ..=MAX_HZ).contains(&min_joint_hz)
        && max_joint_hz <= MAX_HZ;
    let gap_ok =
        imu.received.max <= MAX_GAP && joints.iter().all(|axis| axis.received.max <= MAX_GAP);
    let source_ok = imu.source_ok() && joints.iter().all(SampleStats::source_ok);
    let motor_online_ok = queries.motor_online_min == Some(16);
    let voltage_ok = queries.voltage_zero_values == 0;
    let query_data_ok = !queries_enabled || (motor_online_ok && voltage_ok);
    let queries_ok = queries.is_complete(expected_query_cycles);
    let passed = imu_stream.complete
        && joint_stream.complete
        && malformed_joints == 0
        && queries_ok
        && query_data_ok
        && frequency_ok
        && gap_ok
        && source_ok;

    println!("passed={} observed_window_s={observed:.3}", passed);
    println!(
        "imu_messages={} hz={:.3} max_gap_ms={:.3} source_span_s={:?}",
        imu.count,
        imu_hz,
        imu.received.max.as_secs_f64() * 1000.0,
        imu.source_span_seconds()
    );
    println!(
        "joint_events={} hz={:.3} max_gap_ms={:.3} malformed={malformed_joints}",
        joint_stream.messages,
        joint_stream.messages as f64 / observed,
        joint_stream.received.max.as_secs_f64() * 1000.0,
    );
    imu.report("imu", start);
    for (id, axis) in joints.iter().enumerate() {
        axis.report(&format!("joint_{id}"), start);
    }
    println!("joint_counts={joint_counts:?}");
    println!("joint_axis_hz_min={min_joint_hz:.3} joint_axis_hz_max={max_joint_hz:.3}");
    println!("queries cycles={} calls={} failures={} malformed={} control_samples={} profile_samples={} motor_samples={} motor_online_min={:?} motor_temperature_max_c={:?} voltage_values={} voltage_zero_values={} voltage_min_v={:?} voltage_max_v={:?} expected_cycles={expected_query_cycles}", queries.cycles, queries.calls, queries.failures, queries.malformed, queries.control_samples, queries.profile_samples, queries.motor_samples, queries.motor_online_min, queries.motor_temperature_max, queries.voltage_values, queries.voltage_zero_values, queries.voltage_min, queries.voltage_max);
    println!("thresholds frequency_ok={frequency_ok} gap_ok={gap_ok} source_ok={source_ok} query_data_ok={query_data_ok} motor_online_ok={motor_online_ok} voltage_ok={voltage_ok} queries_ok={queries_ok}");
    if !passed {
        return Err("Brainstem stability window failed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timestamp(ms: i32) -> prost_types::Duration {
        prost_types::Duration {
            seconds: i64::from(ms / 1000),
            nanos: (ms % 1000) * 1_000_000,
        }
    }

    #[test]
    fn receive_stall_pairs_with_the_source_step() {
        let start = Instant::now();
        let mut stats = SampleStats::new(start);
        stats.record(start, Some(&timestamp(0)));
        stats.record(start + Duration::from_secs(3), Some(&timestamp(10)));

        assert_eq!(stats.received.max, Duration::from_secs(3));
        assert_eq!(stats.received.max_ends_at, start + Duration::from_secs(3));
        assert_eq!(stats.received.long_gaps, 1);
        assert_eq!(stats.source_at_receive_max, Some(Duration::from_millis(10)));
        assert!(stats.source_ok());

        stats.finish(start + Duration::from_secs(7));
        assert_eq!(stats.received.max, Duration::from_secs(4));
        assert_eq!(stats.received.max_ends_at, start + Duration::from_secs(7));
        assert_eq!(stats.source_at_receive_max, None);
    }

    #[test]
    fn acceptance_requires_every_scheduled_query() {
        let mut stats = QueryStats::new();
        assert!(stats.is_complete(0));
        stats.cycles = 89;
        stats.calls = 356;
        stats.control_samples = 89;
        stats.profile_samples = 89;
        stats.motor_samples = 89;
        stats.voltage_values = 89 * JOINTS as u64;
        assert!(stats.is_complete(89));
        assert!(!stats.is_complete(90));

        stats.calls -= 1;
        assert!(!stats.is_complete(89));
    }

    #[test]
    fn one_axis_can_fail_while_traffic_continues() {
        let start = Instant::now();
        let mut stalled_axis = SampleStats::new(start);
        let mut live_axis = SampleStats::new(start);
        for ms in (0..=3000).step_by(10) {
            live_axis.record(
                start + Duration::from_millis(ms as u64),
                Some(&timestamp(ms)),
            );
        }
        stalled_axis.record(start, Some(&timestamp(0)));
        stalled_axis.finish(start + Duration::from_secs(3));

        assert!(live_axis.source_ok());
        assert_eq!(live_axis.received.max, Duration::from_millis(10));
        assert!(!stalled_axis.source_ok());
        assert_eq!(stalled_axis.received.max, Duration::from_secs(3));
    }

    #[test]
    fn rejects_missing_and_backward_timestamps() {
        let start = Instant::now();
        let mut stats = SampleStats::new(start);
        for source in [
            Some(timestamp(20)),
            None,
            Some(timestamp(20)),
            Some(timestamp(10)),
            Some(timestamp(30)),
        ] {
            stats.record(start, source.as_ref());
        }

        assert_eq!(stats.invalid_timestamps, 2);
        assert_eq!(stats.source_max_gap, Some(Duration::from_millis(10)));
        assert!(!stats.source_ok());
    }

    #[test]
    fn time_may_repeat_in_a_batch_but_must_advance() {
        let start = Instant::now();
        let mut stats = SampleStats::new(start);
        stats.record(start, Some(&timestamp(10)));
        stats.record(start, Some(&timestamp(10)));
        assert!(!stats.source_ok());
        stats.record(start + Duration::from_millis(10), Some(&timestamp(20)));
        assert!(stats.source_ok());
    }

    #[tokio::test]
    async fn deadline_and_stop_cancel_pending_subscription() {
        let start = Instant::now();
        let (stop, _) = watch::channel(false);
        let pending = || std::future::pending::<doso_sdk::Result<tonic::Streaming<()>>>();
        let timed = watch_stream("test", pending(), start, start, &stop, |_, ()| {}).await;
        assert!(timed.complete);
        assert_eq!(timed.messages, 0);

        stop.send_replace(true);
        let cancelled =
            watch_stream("test", pending(), start, start + MAX_GAP, &stop, |_, ()| {}).await;
        assert!(!cancelled.complete);
    }
}
