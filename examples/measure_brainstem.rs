use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use doso_sdk::internal::motion_board::{rpc::joint::Data, MotionBoardClient};
use tokio::sync::watch;
use tokio::time::{sleep_until, Instant, MissedTickBehavior};

#[path = "support/brainstem.rs"]
mod brainstem;

type BoxResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const DEFAULT_SECONDS: u64 = 900;
const DEFAULT_QUERY_INTERVAL_SECONDS: u64 = 10;
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(60);
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

#[derive(Clone)]
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

struct LiveState {
    imu: SampleStats,
    joints: [SampleStats; JOINTS],
    malformed_imu: u64,
    imu_accel_samples: u64,
    imu_accel_missing: u64,
    imu_accel_norm_min: Option<f64>,
    imu_accel_norm_max: Option<f64>,
    malformed_joints: u64,
    imu_messages: u64,
    joint_messages: u64,
    queries: QueryStats,
}

impl LiveState {
    fn new(start: Instant) -> Self {
        Self {
            imu: SampleStats::new(start),
            joints: std::array::from_fn(|_| SampleStats::new(start)),
            malformed_imu: 0,
            imu_accel_samples: 0,
            imu_accel_missing: 0,
            imu_accel_norm_min: None,
            imu_accel_norm_max: None,
            malformed_joints: 0,
            imu_messages: 0,
            joint_messages: 0,
            queries: QueryStats::new(),
        }
    }

    fn record_imu(&mut self, now: Instant, sample: &doso_sdk::internal::motion_board::rpc::Imu) {
        self.imu_messages += 1;
        self.imu.record(now, sample.timestamp.as_ref());

        let mut malformed = false;
        match sample.gyroscope.as_ref() {
            Some(gyro) if gyro.x.is_finite() && gyro.y.is_finite() && gyro.z.is_finite() => {}
            _ => malformed = true,
        }
        match sample.quaternion.as_ref() {
            Some(quat)
                if quat.w.is_finite()
                    && quat.x.is_finite()
                    && quat.y.is_finite()
                    && quat.z.is_finite() => {}
            _ => malformed = true,
        }
        match sample.linear_acceleration.as_ref() {
            Some(acc) if acc.x.is_finite() && acc.y.is_finite() && acc.z.is_finite() => {
                let norm = (acc.x * acc.x + acc.y * acc.y + acc.z * acc.z).sqrt();
                self.imu_accel_samples += 1;
                self.imu_accel_norm_min = Some(
                    self.imu_accel_norm_min
                        .map_or(norm, |current| current.min(norm)),
                );
                self.imu_accel_norm_max = Some(
                    self.imu_accel_norm_max
                        .map_or(norm, |current| current.max(norm)),
                );
            }
            Some(_) => malformed = true,
            None => self.imu_accel_missing += 1,
        }
        if malformed {
            self.malformed_imu += 1;
        }
    }

    fn record_joint(&mut self, now: Instant, sample: doso_sdk::internal::motion_board::rpc::Joint) {
        self.joint_messages += 1;
        let timestamp = sample.timestamp.as_ref();
        match sample.data {
            Some(Data::SingleJoint(joint)) => {
                if let Some(axis) = self.joints.get_mut(joint.id as usize) {
                    axis.record(now, timestamp);
                } else {
                    self.malformed_joints += 1;
                }
            }
            Some(Data::AllJoints(frame))
                if frame
                    .position
                    .as_ref()
                    .is_some_and(|position| position.values.len() == JOINTS) =>
            {
                for axis in &mut self.joints {
                    axis.record(now, timestamp);
                }
            }
            _ => self.malformed_joints += 1,
        }
    }

    fn checkpoint_snapshot(&self, start: Instant, now: Instant) -> CheckpointMetrics {
        let elapsed_s = now.duration_since(start).as_secs_f64();
        let joint_counts = self.joints.each_ref().map(|axis| axis.count);
        let (imu_hz, joint_axis_hz_min, joint_axis_hz_max) = if elapsed_s > 0.0 {
            let joint_hz = joint_counts.map(|count| count as f64 / elapsed_s);
            let min_hz = joint_hz.iter().copied().fold(f64::INFINITY, f64::min);
            let max_hz = joint_hz.iter().copied().fold(0.0, f64::max);
            (self.imu.count as f64 / elapsed_s, min_hz, max_hz)
        } else {
            (0.0, 0.0, 0.0)
        };
        let joint_max_gap_ms = self
            .joints
            .iter()
            .map(|axis| {
                axis.received
                    .max
                    .max(now.saturating_duration_since(axis.received.last))
            })
            .fold(Duration::ZERO, Duration::max)
            .as_secs_f64()
            * 1000.0;
        let imu_max_gap_ms = self
            .imu
            .received
            .max
            .max(now.saturating_duration_since(self.imu.received.last))
            .as_secs_f64()
            * 1000.0;

        CheckpointMetrics {
            elapsed_s,
            imu_messages: self.imu.count,
            imu_hz,
            imu_max_gap_ms,
            joint_events: self.joint_messages,
            joint_axis_hz_min,
            joint_axis_hz_max,
            joint_max_gap_ms,
            malformed_joints: self.malformed_joints,
            query_cycles: self.queries.cycles,
            query_failures: self.queries.failures,
            motor_online_min: self.queries.motor_online_min,
            voltage_zero_values: self.queries.voltage_zero_values,
            voltage_min_v: self.queries.voltage_min,
            voltage_max_v: self.queries.voltage_max,
        }
    }
}

struct CheckpointMetrics {
    elapsed_s: f64,
    imu_messages: u64,
    imu_hz: f64,
    imu_max_gap_ms: f64,
    joint_events: u64,
    joint_axis_hz_min: f64,
    joint_axis_hz_max: f64,
    joint_max_gap_ms: f64,
    malformed_joints: u64,
    query_cycles: u64,
    query_failures: u64,
    motor_online_min: Option<usize>,
    voltage_zero_values: u64,
    voltage_min_v: Option<f64>,
    voltage_max_v: Option<f64>,
}

struct SummaryReport<'a> {
    passed: bool,
    observed_window_s: f64,
    endpoint: &'a str,
    duration_s: u64,
    query_interval_s: u64,
    expected_query_cycles: u64,
    imu_messages: u64,
    imu_hz: f64,
    imu_max_gap_ms: f64,
    malformed_imu: u64,
    imu_accel_samples: u64,
    imu_accel_missing: u64,
    imu_accel_norm_min_mps2: Option<f64>,
    imu_accel_norm_max_mps2: Option<f64>,
    joint_events: u64,
    joint_axis_hz_min: f64,
    joint_axis_hz_max: f64,
    joint_max_gap_ms: f64,
    malformed_joints: u64,
    queries: &'a QueryStats,
    frequency_ok: bool,
    gap_ok: bool,
    source_ok: bool,
    imu_data_ok: bool,
    query_data_ok: bool,
    motor_online_ok: bool,
    voltage_ok: bool,
    queries_ok: bool,
    imu_stream_complete: bool,
    joint_stream_complete: bool,
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_f64(value: f64, decimals: usize) -> String {
    if value.is_finite() {
        format!("{value:.decimals$}")
    } else {
        "null".to_owned()
    }
}

fn json_opt_f64(value: Option<f64>, decimals: usize) -> String {
    value.map_or_else(|| "null".to_owned(), |v| json_f64(v, decimals))
}

fn json_opt_usize(value: Option<usize>) -> String {
    value.map_or_else(|| "null".to_owned(), |v| v.to_string())
}

fn format_meta_json(
    endpoint: &str,
    duration_s: u64,
    query_interval_s: u64,
    checkpoint_interval_s: u64,
    started_unix_ms: u128,
) -> String {
    format!(
        "{{\n  \"endpoint\": {},\n  \"duration_s\": {},\n  \"query_interval_s\": {},\n  \"checkpoint_interval_s\": {},\n  \"started_unix_ms\": {}\n}}\n",
        json_string(endpoint),
        duration_s,
        query_interval_s,
        checkpoint_interval_s,
        started_unix_ms
    )
}

fn format_checkpoint_json(metrics: &CheckpointMetrics) -> String {
    format!(
        "{{\"elapsed_s\":{},\"imu_messages\":{},\"imu_hz\":{},\"imu_max_gap_ms\":{},\"joint_events\":{},\"joint_axis_hz_min\":{},\"joint_axis_hz_max\":{},\"joint_max_gap_ms\":{},\"malformed_joints\":{},\"query_cycles\":{},\"query_failures\":{},\"motor_online_min\":{},\"voltage_zero_values\":{},\"voltage_min_v\":{},\"voltage_max_v\":{}}}",
        json_f64(metrics.elapsed_s, 3),
        metrics.imu_messages,
        json_f64(metrics.imu_hz, 3),
        json_f64(metrics.imu_max_gap_ms, 3),
        metrics.joint_events,
        json_f64(metrics.joint_axis_hz_min, 3),
        json_f64(metrics.joint_axis_hz_max, 3),
        json_f64(metrics.joint_max_gap_ms, 3),
        metrics.malformed_joints,
        metrics.query_cycles,
        metrics.query_failures,
        json_opt_usize(metrics.motor_online_min),
        metrics.voltage_zero_values,
        json_opt_f64(metrics.voltage_min_v, 4),
        json_opt_f64(metrics.voltage_max_v, 4),
    )
}

fn format_summary_json(report: &SummaryReport<'_>) -> String {
    format!(
        "{{\n\
  \"passed\": {},\n\
  \"observed_window_s\": {},\n\
  \"endpoint\": {},\n\
  \"duration_s\": {},\n\
  \"query_interval_s\": {},\n\
  \"expected_query_cycles\": {},\n\
  \"imu_messages\": {},\n\
  \"imu_hz\": {},\n\
  \"imu_max_gap_ms\": {},\n\
  \"malformed_imu\": {},\n\
  \"imu_accel_samples\": {},\n\
  \"imu_accel_missing\": {},\n\
  \"imu_accel_norm_min_mps2\": {},\n\
  \"imu_accel_norm_max_mps2\": {},\n\
  \"joint_events\": {},\n\
  \"joint_axis_hz_min\": {},\n\
  \"joint_axis_hz_max\": {},\n\
  \"joint_max_gap_ms\": {},\n\
  \"malformed_joints\": {},\n\
  \"query_cycles\": {},\n\
  \"query_calls\": {},\n\
  \"query_failures\": {},\n\
  \"query_malformed\": {},\n\
  \"motor_online_min\": {},\n\
  \"motor_temperature_max_c\": {},\n\
  \"voltage_values\": {},\n\
  \"voltage_zero_values\": {},\n\
  \"voltage_min_v\": {},\n\
  \"voltage_max_v\": {},\n\
  \"thresholds\": {{\n\
    \"frequency_ok\": {},\n\
    \"gap_ok\": {},\n\
    \"source_ok\": {},\n\
    \"imu_data_ok\": {},\n\
    \"query_data_ok\": {},\n\
    \"motor_online_ok\": {},\n\
    \"voltage_ok\": {},\n\
    \"queries_ok\": {},\n\
    \"imu_stream_complete\": {},\n\
    \"joint_stream_complete\": {}\n\
  }}\n\
}}\n",
        report.passed,
        json_f64(report.observed_window_s, 3),
        json_string(report.endpoint),
        report.duration_s,
        report.query_interval_s,
        report.expected_query_cycles,
        report.imu_messages,
        json_f64(report.imu_hz, 3),
        json_f64(report.imu_max_gap_ms, 3),
        report.malformed_imu,
        report.imu_accel_samples,
        report.imu_accel_missing,
        json_opt_f64(report.imu_accel_norm_min_mps2, 4),
        json_opt_f64(report.imu_accel_norm_max_mps2, 4),
        report.joint_events,
        json_f64(report.joint_axis_hz_min, 3),
        json_f64(report.joint_axis_hz_max, 3),
        json_f64(report.joint_max_gap_ms, 3),
        report.malformed_joints,
        report.queries.cycles,
        report.queries.calls,
        report.queries.failures,
        report.queries.malformed,
        json_opt_usize(report.queries.motor_online_min),
        json_opt_f64(report.queries.motor_temperature_max, 2),
        report.queries.voltage_values,
        report.queries.voltage_zero_values,
        json_opt_f64(report.queries.voltage_min, 4),
        json_opt_f64(report.queries.voltage_max, 4),
        report.frequency_ok,
        report.gap_ok,
        report.source_ok,
        report.imu_data_ok,
        report.query_data_ok,
        report.motor_online_ok,
        report.voltage_ok,
        report.queries_ok,
        report.imu_stream_complete,
        report.joint_stream_complete,
    )
}

fn write_meta_file(out_dir: &Path, content: &str) -> std::io::Result<()> {
    fs::create_dir_all(out_dir)?;
    let mut file = File::create(out_dir.join("meta.json"))?;
    file.write_all(content.as_bytes())?;
    file.sync_all()
}

fn append_checkpoint_line(out_dir: &Path, line: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(out_dir.join("checkpoints.jsonl"))?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()?;
    file.sync_data()
}

fn write_summary_atomic(out_dir: &Path, content: &str) -> std::io::Result<()> {
    let tmp = out_dir.join("summary.json.tmp");
    let dst = out_dir.join("summary.json");
    let mut file = File::create(&tmp)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, &dst)
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
    queries: Option<(MotionBoardClient, Duration)>,
    start: Instant,
    deadline: Instant,
    stop: &watch::Sender<bool>,
    state: &Arc<Mutex<LiveState>>,
) -> QueryStats {
    let Some((client, interval)) = queries else {
        return QueryStats::new();
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
                {
                    let mut guard = state.lock().unwrap();
                    guard.queries.cycles += 1;
                    guard.queries.calls += 1;
                }

                let control_res = tokio::time::timeout(QUERY_TIMEOUT, client.control_status()).await;
                {
                    let mut guard = state.lock().unwrap();
                    match control_res {
                        Err(_) => {
                            guard.queries.failures += 1;
                            eprintln!("control_status_error=timeout");
                        }
                        Ok(Err(error)) => {
                            guard.queries.failures += 1;
                            eprintln!("control_status_error={error:?}");
                        }
                        Ok(Ok(status)) if status.fsm.is_some() => guard.queries.control_samples += 1,
                        Ok(Ok(_)) => guard.queries.malformed += 1,
                    }
                    guard.queries.calls += 1;
                }

                let profile_res = tokio::time::timeout(QUERY_TIMEOUT, client.profile()).await;
                {
                    let mut guard = state.lock().unwrap();
                    match profile_res {
                        Err(_) => {
                            guard.queries.failures += 1;
                            eprintln!("profile_error=timeout");
                        }
                        Ok(Err(error)) => {
                            guard.queries.failures += 1;
                            eprintln!("profile_error={error:?}");
                        }
                        Ok(Ok(profile)) if !profile.current.is_empty() => {
                            guard.queries.profile_samples += 1;
                        }
                        Ok(Ok(_)) => guard.queries.malformed += 1,
                    }
                    guard.queries.calls += 1;
                }

                let voltage_res = tokio::time::timeout(QUERY_TIMEOUT, client.voltage()).await;
                {
                    let mut guard = state.lock().unwrap();
                    match voltage_res {
                        Err(_) => {
                            guard.queries.failures += 1;
                            eprintln!("voltage_error=timeout");
                        }
                        Ok(Err(error)) => {
                            guard.queries.failures += 1;
                            eprintln!("voltage_error={error:?}");
                        }
                        Ok(Ok(voltage))
                            if voltage.values.len() == 16
                                && voltage.values.iter().all(|value| value.is_finite()) =>
                        {
                            guard.queries.voltage_values += 16;
                            guard.queries.voltage_zero_values +=
                                voltage.values.iter().filter(|value| **value == 0.0).count() as u64;
                            let min = voltage.values.iter().copied().fold(f64::INFINITY, f64::min);
                            let max = voltage.values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                            guard.queries.voltage_min =
                                Some(guard.queries.voltage_min.map_or(min, |value| value.min(min)));
                            guard.queries.voltage_max =
                                Some(guard.queries.voltage_max.map_or(max, |value| value.max(max)));
                        }
                        Ok(Ok(_)) => guard.queries.malformed += 1,
                    }
                    guard.queries.calls += 1;
                }

                let motor_res = tokio::time::timeout(QUERY_TIMEOUT, client.motor_status()).await;
                {
                    let mut guard = state.lock().unwrap();
                    match motor_res {
                        Err(_) => {
                            guard.queries.failures += 1;
                            eprintln!("motor_status_error=timeout");
                        }
                        Ok(Err(error)) => {
                            guard.queries.failures += 1;
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
                            guard.queries.motor_samples += 1;
                            guard.queries.motor_online_min =
                                Some(guard.queries.motor_online_min.map_or(online, |min| min.min(online)));
                            let max_temperature = status
                                .motors
                                .iter()
                                .map(|motor| motor.temperature)
                                .fold(f64::NEG_INFINITY, f64::max);
                            guard.queries.motor_temperature_max = Some(
                                guard
                                    .queries
                                    .motor_temperature_max
                                    .map_or(max_temperature, |value| value.max(max_temperature)),
                            );
                        }
                        Ok(Ok(_)) => guard.queries.malformed += 1,
                    }
                }
            } => {
                let abort = {
                    let guard = state.lock().unwrap();
                    guard.queries.failures > 0 || guard.queries.malformed > 0
                };
                if abort {
                    stop.send_replace(true);
                    break;
                }
            }
        }
    }
    state.lock().unwrap().queries.clone()
}

async fn poll_checkpoints(
    out_dir: Option<&Path>,
    start: Instant,
    deadline: Instant,
    stop: &watch::Sender<bool>,
    state: &Arc<Mutex<LiveState>>,
) {
    let Some(out_dir) = out_dir else {
        return;
    };
    let mut cancelled = stop.subscribe();
    let mut ticker = tokio::time::interval_at(start + CHECKPOINT_INTERVAL, CHECKPOINT_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            _ = cancelled.wait_for(|stopped| *stopped) => break,
            _ = sleep_until(deadline) => break,
            _ = ticker.tick() => {
                let now = Instant::now().min(deadline);
                let line = {
                    let guard = state.lock().unwrap();
                    format_checkpoint_json(&guard.checkpoint_snapshot(start, now))
                };
                if let Err(error) = append_checkpoint_line(out_dir, &line) {
                    eprintln!("checkpoint_write_error={error}");
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CliArgs {
    endpoint: String,
    seconds: u64,
    query_interval: u64,
    out_dir: Option<PathBuf>,
}

fn parse_u64(value: Option<&String>, name: &str, default: u64, max: u64) -> BoxResult<u64> {
    let value = value.map_or(Ok(default), |value| value.parse())?;
    if value == 0 || value > max {
        return Err(format!("{name} must be between 1 and {max}").into());
    }
    Ok(value)
}

fn parse_args(args: &[String]) -> BoxResult<Option<CliArgs>> {
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        return Ok(None);
    }
    if !(1..=4).contains(&args.len()) {
        return Err(
            "Expected an endpoint, optional duration, query interval, and out_dir; use --help"
                .into(),
        );
    }
    let seconds = parse_u64(args.get(1), "seconds", DEFAULT_SECONDS, 86_400)?;
    let query_interval = args
        .get(2)
        .map_or(Ok(DEFAULT_QUERY_INTERVAL_SECONDS), |value| value.parse())?;
    if query_interval > 300 {
        return Err("query_interval_seconds must be between 0 and 300".into());
    }
    let out_dir = match args.get(3) {
        Some(dir) if dir.trim().is_empty() => return Err("out_dir must not be empty".into()),
        Some(dir) => Some(PathBuf::from(dir)),
        None => None,
    };
    Ok(Some(CliArgs {
        endpoint: args[0].clone(),
        seconds,
        query_interval,
        out_dir,
    }))
}

#[tokio::main]
async fn main() -> BoxResult<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cli) = parse_args(&args)? else {
        println!(
            "measure_brainstem <http(s)://robot:13145> [seconds=900] [query_interval_seconds=10] [out_dir]"
        );
        println!("Read-only Brainstem stability window for IMU, joints, voltage, motors, control status and profile.");
        println!("The default window is 15 minutes. The command never acquires control or sends motion commands.");
        println!("Set query_interval_seconds to 0 for a stream-only window.");
        println!("Set out_dir to write meta.json, 60s checkpoints.jsonl, and atomic summary.json.");
        println!("For HTTPS with mutual TLS, set DOSO_BRAINSTEM_CERT_DIR.");
        return Ok(());
    };
    let expected_query_cycles = if cli.query_interval == 0 {
        0
    } else {
        cli.seconds.div_ceil(cli.query_interval)
    };
    let queries_enabled = expected_query_cycles > 0;

    if let Some(out_dir) = &cli.out_dir {
        let started_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let meta = format_meta_json(
            &cli.endpoint,
            cli.seconds,
            cli.query_interval,
            CHECKPOINT_INTERVAL.as_secs(),
            started_unix_ms,
        );
        write_meta_file(out_dir, &meta)?;
    }

    let stream_client = brainstem::connect(&cli.endpoint).await?;
    let queries = if queries_enabled {
        Some((
            brainstem::connect(&cli.endpoint).await?,
            Duration::from_secs(cli.query_interval),
        ))
    } else {
        None
    };
    let start = Instant::now();
    let deadline = start + Duration::from_secs(cli.seconds);
    let (stop, _) = watch::channel(false);
    let state = Arc::new(Mutex::new(LiveState::new(start)));
    println!(
        "endpoint={} duration_s={} query_interval_s={}",
        cli.endpoint, cli.seconds, cli.query_interval
    );

    let (imu_stream, joint_stream, queries, ()) = tokio::join!(
        watch_stream(
            "imu",
            stream_client.listen_imu(),
            start,
            deadline,
            &stop,
            |now, sample| state.lock().unwrap().record_imu(now, &sample),
        ),
        watch_stream(
            "joint",
            stream_client.listen_joint(),
            start,
            deadline,
            &stop,
            |now, sample| state.lock().unwrap().record_joint(now, sample),
        ),
        poll_queries(queries, start, deadline, &stop, &state),
        poll_checkpoints(cli.out_dir.as_deref(), start, deadline, &stop, &state),
    );
    let end = Instant::now().min(deadline);
    let observed = end.duration_since(start).as_secs_f64();

    let (
        imu_count,
        imu_hz,
        imu_max_gap,
        imu_source_span,
        malformed_imu,
        imu_accel_samples,
        imu_accel_missing,
        imu_accel_norm_min,
        imu_accel_norm_max,
        joint_counts,
        joint_axis_hz,
        min_joint_hz,
        max_joint_hz,
        joint_max_gap,
        malformed_joints,
        gap_ok,
        source_ok,
    ) = {
        let mut guard = state.lock().unwrap();
        guard.imu.finish(end);
        for axis in &mut guard.joints {
            axis.finish(end);
        }
        let joint_counts = guard.joints.each_ref().map(|axis| axis.count);
        let joint_axis_hz = joint_counts.map(|count| count as f64 / observed);
        let min_joint_hz = joint_axis_hz.iter().copied().fold(f64::INFINITY, f64::min);
        let max_joint_hz = joint_axis_hz.iter().copied().fold(0.0, f64::max);
        let imu_hz = guard.imu.count as f64 / observed;
        let gap_ok = guard.imu.received.max <= MAX_GAP
            && guard.joints.iter().all(|axis| axis.received.max <= MAX_GAP);
        let source_ok = guard.imu.source_ok() && guard.joints.iter().all(SampleStats::source_ok);

        guard.imu.report("imu", start);
        for (id, axis) in guard.joints.iter().enumerate() {
            axis.report(&format!("joint_{id}"), start);
        }

        (
            guard.imu.count,
            imu_hz,
            guard.imu.received.max,
            guard.imu.source_span_seconds(),
            guard.malformed_imu,
            guard.imu_accel_samples,
            guard.imu_accel_missing,
            guard.imu_accel_norm_min,
            guard.imu_accel_norm_max,
            joint_counts,
            joint_axis_hz,
            min_joint_hz,
            max_joint_hz,
            joint_stream.received.max,
            guard.malformed_joints,
            gap_ok,
            source_ok,
        )
    };
    let _ = joint_axis_hz;

    let frequency_ok = (MIN_HZ..=MAX_HZ).contains(&imu_hz)
        && (MIN_HZ..=MAX_HZ).contains(&min_joint_hz)
        && max_joint_hz <= MAX_HZ;
    let imu_data_ok = imu_count > 0
        && malformed_imu == 0
        && imu_accel_missing == 0
        && imu_accel_samples == imu_count;
    let motor_online_ok = queries.motor_online_min == Some(16);
    let voltage_ok = queries.voltage_zero_values == 0;
    let query_data_ok = !queries_enabled || (motor_online_ok && voltage_ok);
    let queries_ok = queries.is_complete(expected_query_cycles);
    let passed = imu_stream.complete
        && joint_stream.complete
        && imu_data_ok
        && malformed_joints == 0
        && queries_ok
        && query_data_ok
        && frequency_ok
        && gap_ok
        && source_ok;

    println!("passed={} observed_window_s={observed:.3}", passed);
    println!(
        "imu_messages={} hz={:.3} max_gap_ms={:.3} source_span_s={:?} malformed_imu={} accel_samples={} accel_missing={} accel_norm_min_mps2={:?} accel_norm_max_mps2={:?}",
        imu_count,
        imu_hz,
        imu_max_gap.as_secs_f64() * 1000.0,
        imu_source_span,
        malformed_imu,
        imu_accel_samples,
        imu_accel_missing,
        imu_accel_norm_min,
        imu_accel_norm_max,
    );
    println!(
        "joint_events={} hz={:.3} max_gap_ms={:.3} malformed={malformed_joints}",
        joint_stream.messages,
        joint_stream.messages as f64 / observed,
        joint_max_gap.as_secs_f64() * 1000.0,
    );
    println!("joint_counts={joint_counts:?}");
    println!("joint_axis_hz_min={min_joint_hz:.3} joint_axis_hz_max={max_joint_hz:.3}");
    println!("queries cycles={} calls={} failures={} malformed={} control_samples={} profile_samples={} motor_samples={} motor_online_min={:?} motor_temperature_max_c={:?} voltage_values={} voltage_zero_values={} voltage_min_v={:?} voltage_max_v={:?} expected_cycles={expected_query_cycles}", queries.cycles, queries.calls, queries.failures, queries.malformed, queries.control_samples, queries.profile_samples, queries.motor_samples, queries.motor_online_min, queries.motor_temperature_max, queries.voltage_values, queries.voltage_zero_values, queries.voltage_min, queries.voltage_max);
    println!("thresholds frequency_ok={frequency_ok} gap_ok={gap_ok} source_ok={source_ok} imu_data_ok={imu_data_ok} query_data_ok={query_data_ok} motor_online_ok={motor_online_ok} voltage_ok={voltage_ok} queries_ok={queries_ok}");

    if let Some(out_dir) = &cli.out_dir {
        let summary = format_summary_json(&SummaryReport {
            passed,
            observed_window_s: observed,
            endpoint: &cli.endpoint,
            duration_s: cli.seconds,
            query_interval_s: cli.query_interval,
            expected_query_cycles,
            imu_messages: imu_count,
            imu_hz,
            imu_max_gap_ms: imu_max_gap.as_secs_f64() * 1000.0,
            malformed_imu,
            imu_accel_samples,
            imu_accel_missing,
            imu_accel_norm_min_mps2: imu_accel_norm_min,
            imu_accel_norm_max_mps2: imu_accel_norm_max,
            joint_events: joint_stream.messages,
            joint_axis_hz_min: min_joint_hz,
            joint_axis_hz_max: max_joint_hz,
            joint_max_gap_ms: joint_max_gap.as_secs_f64() * 1000.0,
            malformed_joints,
            queries: &queries,
            frequency_ok,
            gap_ok,
            source_ok,
            imu_data_ok,
            query_data_ok,
            motor_online_ok,
            voltage_ok,
            queries_ok,
            imu_stream_complete: imu_stream.complete,
            joint_stream_complete: joint_stream.complete,
        });
        write_summary_atomic(out_dir, &summary)?;
    }

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

    fn imu_sample(ms: i32, accel: Option<[f64; 3]>) -> doso_sdk::internal::motion_board::rpc::Imu {
        doso_sdk::internal::motion_board::rpc::Imu {
            gyroscope: Some(doso_sdk::internal::motion_board::rpc::Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }),
            quaternion: Some(doso_sdk::internal::motion_board::rpc::Quaternion {
                w: 1.0,
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }),
            linear_acceleration: accel
                .map(|[x, y, z]| doso_sdk::internal::motion_board::rpc::Vector3 { x, y, z }),
            timestamp: Some(timestamp(ms)),
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

    #[test]
    fn parses_cli_arguments_with_optional_out_dir() {
        assert_eq!(parse_args(&["--help".into()]).unwrap(), None);
        assert_eq!(
            parse_args(&["http://127.0.0.1:13145".into()]).unwrap(),
            Some(CliArgs {
                endpoint: "http://127.0.0.1:13145".into(),
                seconds: DEFAULT_SECONDS,
                query_interval: DEFAULT_QUERY_INTERVAL_SECONDS,
                out_dir: None,
            })
        );
        assert_eq!(
            parse_args(&[
                "http://127.0.0.1:13145".into(),
                "3600".into(),
                "10".into(),
                "out/run".into(),
            ])
            .unwrap(),
            Some(CliArgs {
                endpoint: "http://127.0.0.1:13145".into(),
                seconds: 3600,
                query_interval: 10,
                out_dir: Some(PathBuf::from("out/run")),
            })
        );
        assert!(parse_args(&[
            "http://127.0.0.1:13145".into(),
            "60".into(),
            "10".into(),
            "   ".into(),
        ])
        .is_err());
        assert!(
            parse_args(&["a".into(), "1".into(), "2".into(), "3".into(), "4".into(),]).is_err()
        );
    }

    #[test]
    fn writes_meta_checkpoint_and_atomic_summary_json() {
        let dir = std::env::temp_dir().join(format!(
            "doso-measure-test-{}",
            doso_sdk::new_request_id().unwrap()
        ));
        let endpoint = "http://192.168.66.64:13145\\test\"quote";
        let meta = format_meta_json(endpoint, 3600, 10, 60, 12345);
        write_meta_file(&dir, &meta).unwrap();

        let start = Instant::now();
        let mut state = LiveState::new(start);
        state.record_imu(
            start + Duration::from_secs(1),
            &imu_sample(10, Some([0.0, 0.0, 9.81])),
        );
        let snapshot = state.checkpoint_snapshot(start, start + Duration::from_secs(2));
        let cp_line = format_checkpoint_json(&snapshot);
        assert!(cp_line.contains("\"motor_online_min\":null"));
        assert!(cp_line.contains("\"voltage_min_v\":null"));
        append_checkpoint_line(&dir, &cp_line).unwrap();

        let queries = QueryStats::new();
        let summary = format_summary_json(&SummaryReport {
            passed: false,
            observed_window_s: 2.0,
            endpoint,
            duration_s: 3600,
            query_interval_s: 10,
            expected_query_cycles: 360,
            imu_messages: 1,
            imu_hz: 0.5,
            imu_max_gap_ms: 1000.0,
            malformed_imu: 0,
            imu_accel_samples: 1,
            imu_accel_missing: 0,
            imu_accel_norm_min_mps2: Some(9.81),
            imu_accel_norm_max_mps2: Some(9.81),
            joint_events: 0,
            joint_axis_hz_min: 0.0,
            joint_axis_hz_max: 0.0,
            joint_max_gap_ms: 0.0,
            malformed_joints: 0,
            queries: &queries,
            frequency_ok: false,
            gap_ok: true,
            source_ok: true,
            imu_data_ok: true,
            query_data_ok: false,
            motor_online_ok: false,
            voltage_ok: false,
            queries_ok: false,
            imu_stream_complete: true,
            joint_stream_complete: true,
        });
        write_summary_atomic(&dir, &summary).unwrap();

        let meta_read = fs::read_to_string(dir.join("meta.json")).unwrap();
        assert!(meta_read.contains(r#""endpoint": "http://192.168.66.64:13145\\test\"quote""#));
        let cp_read = fs::read_to_string(dir.join("checkpoints.jsonl")).unwrap();
        assert_eq!(cp_read.lines().count(), 1);
        let sum_read = fs::read_to_string(dir.join("summary.json")).unwrap();
        assert!(sum_read.contains("\"passed\": false"));
        assert!(sum_read.contains("\"imu_accel_samples\": 1"));
        assert!(sum_read.contains("\"imu_data_ok\": true"));
        assert!(sum_read.contains("\"motor_temperature_max_c\": null"));
        assert!(!dir.join("summary.json.tmp").exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn validates_imu_linear_acceleration_and_finite_fields() {
        let start = Instant::now();
        let mut state = LiveState::new(start);
        state.record_imu(start, &imu_sample(10, Some([0.0, 0.0, 9.81])));
        state.record_imu(
            start + Duration::from_millis(10),
            &imu_sample(20, Some([3.0, 4.0, 0.0])),
        );
        assert_eq!(state.malformed_imu, 0);
        assert_eq!(state.imu_accel_samples, 2);
        assert_eq!(state.imu_accel_missing, 0);
        assert!((state.imu_accel_norm_min.unwrap() - 5.0).abs() < 1e-9);
        assert!((state.imu_accel_norm_max.unwrap() - 9.81).abs() < 1e-9);

        state.record_imu(start + Duration::from_millis(20), &imu_sample(30, None));
        assert_eq!(state.imu_accel_missing, 1);

        let mut bad_gyro = imu_sample(40, Some([0.0, 0.0, 9.81]));
        bad_gyro.gyroscope.as_mut().unwrap().x = f64::NAN;
        state.record_imu(start + Duration::from_millis(30), &bad_gyro);
        assert_eq!(state.malformed_imu, 1);
    }
}
