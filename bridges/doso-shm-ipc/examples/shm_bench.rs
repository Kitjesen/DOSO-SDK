use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use doso_shm_ipc::{DosoProprioPayload, ProprioRingReader, ProprioRingWriter, DOSO_SHM_JOINTS};

fn percentile(sorted: &[u64], pct: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (((sorted.len() - 1) as f64) * pct).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn summarize(label: &str, samples_ns: &mut [u64]) {
    samples_ns.sort_unstable();
    let p50 = percentile(samples_ns, 0.50);
    let p99 = percentile(samples_ns, 0.99);
    let p999 = percentile(samples_ns, 0.999);
    let max = *samples_ns.last().unwrap_or(&0);
    println!(
        "{label:<22} n={:<7} p50={:>6.2} us  p99={:>6.2} us  p99.9={:>7.2} us  max={:>7.2} us",
        samples_ns.len(),
        p50 as f64 / 1000.0,
        p99 as f64 / 1000.0,
        p999 as f64 / 1000.0,
        max as f64 / 1000.0,
    );
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let hz: u64 = args
        .iter()
        .position(|a| a == "--hz")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let frames: usize = args
        .iter()
        .position(|a| a == "--frames")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000);

    let shm_name = format!(
        "/doso_bench_{}_{}",
        std::process::id(),
        doso_shm_ipc::monotonic_us()
    );
    let mut writer = ProprioRingWriter::open(&shm_name)?;
    let reader = ProprioRingReader::open(&shm_name)?;

    println!(
        "=== DOSO POSIX SHM Lock-Free RingBuffer Benchmark (hz={hz}, frames={frames}, shm={shm_name}) ==="
    );

    // 1. Raw uncontended/contended write & read microbenchmark (10,000 frames).
    let burst_n = 10_000_usize;
    let stop_burst = Arc::new(AtomicBool::new(false));
    let reader_ready = Arc::new(AtomicBool::new(false));
    let stop_r = stop_burst.clone();
    let ready_r = reader_ready.clone();
    let reader_name = shm_name.clone();
    let reader_handle = thread::spawn(move || {
        let r = ProprioRingReader::open(&reader_name).expect("open reader");
        ready_r.store(true, Ordering::Release);
        let mut last = 0_u64;
        let mut read_ns = Vec::with_capacity(burst_n);
        while !stop_r.load(Ordering::Acquire) {
            let t0 = Instant::now();
            if let Some((seq, p)) = r.read_latest(last) {
                let dt = t0.elapsed().as_nanos() as u64;
                assert_eq!(p.sequence, seq);
                let expected = seq as f64 * 0.001;
                assert!(
                    (p.joint_q[0] - expected).abs() < 1e-12,
                    "torn read detected!"
                );
                assert!(
                    (p.joint_tau[DOSO_SHM_JOINTS - 1] - expected).abs() < 1e-12,
                    "torn read detected!"
                );
                read_ns.push(dt);
                last = seq;
            } else {
                std::hint::spin_loop();
            }
        }
        read_ns
    });
    while !reader_ready.load(Ordering::Acquire) {
        std::hint::spin_loop();
    }

    let mut write_ns = Vec::with_capacity(burst_n);
    for seq in 1..=(burst_n as u64) {
        let v = seq as f64 * 0.001;
        let payload = DosoProprioPayload {
            sequence: seq,
            monotonic_us: doso_shm_ipc::monotonic_us(),
            imu_quat_wxyz: [1.0, 0.0, 0.0, 0.0],
            joint_q: [v; DOSO_SHM_JOINTS],
            joint_tau: [v; DOSO_SHM_JOINTS],
            ..Default::default()
        };
        let t0 = Instant::now();
        writer.publish(&payload)?;
        write_ns.push(t0.elapsed().as_nanos() as u64);
    }
    stop_burst.store(true, Ordering::Release);
    let mut read_ns = reader_handle.join().expect("reader thread joined");
    summarize("Burst SHM write", &mut write_ns);
    summarize("Burst SHM read", &mut read_ns);

    // 2. Paced real-time loop (simulates 200Hz / 1000Hz Board-A closed-loop wake jitter + IPC).
    let period = Duration::from_nanos(1_000_000_000 / hz.max(1));
    let mut last_seq = writer.write_seq();
    let mut ipc_rtt_ns = Vec::with_capacity(frames);
    let mut jitter_ns = Vec::with_capacity(frames);
    let mut next_tick = Instant::now() + period;

    for i in 0..frames {
        let now = Instant::now();
        if next_tick > now {
            thread::sleep(next_tick - now);
        }
        let woke = Instant::now();
        let jitter = woke.saturating_duration_since(next_tick).as_nanos() as u64;
        jitter_ns.push(jitter);
        next_tick += period;

        let seq = last_seq + 1;
        let payload = DosoProprioPayload {
            sequence: seq,
            monotonic_us: doso_shm_ipc::monotonic_us(),
            imu_quat_wxyz: [1.0, 0.0, 0.0, 0.0],
            joint_q: [i as f64 * 0.001; DOSO_SHM_JOINTS],
            ..Default::default()
        };
        let t0 = Instant::now();
        writer.publish(&payload)?;
        let (rseq, got) = reader
            .read_latest(last_seq)
            .expect("reader must immediately observe newly published frame");
        let rtt = t0.elapsed().as_nanos() as u64;
        assert_eq!(rseq, seq);
        assert_eq!(got.sequence, seq);
        ipc_rtt_ns.push(rtt);
        last_seq = rseq;
    }

    summarize("Paced write+read IPC", &mut ipc_rtt_ns);
    summarize("OS timer wake jitter", &mut jitter_ns);
    println!("Result: 0 torn reads verified.");
    Ok(())
}
