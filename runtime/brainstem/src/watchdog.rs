use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::warn;

pub const MIN_WATCHDOG_USEC: u64 = 3_000;
pub const DEFAULT_SYSTEMD_NOTIFY_BIN: &str = "/usr/bin/systemd-notify";

pub const READY_PAYLOAD: &str = "READY=1\nSTATUS=Rust Brainstem gRPC and RT loop ready\n";
pub const WATCHDOG_PAYLOAD: &str = "WATCHDOG=1\n";
pub const STOPPING_PAYLOAD: &str = "STOPPING=1\nSTATUS=Rust Brainstem shutting down\n";

pub type NotifyFn = Arc<dyn Fn(&WatchdogConfig, &str) -> Result<()> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogConfig {
    pub notify_socket: String,
    pub ping_interval: Option<Duration>,
    pub notify_bin: String,
}

/// Parses `NOTIFY_SOCKET`, `WATCHDOG_USEC`, and optional `THUNDER_SYSTEMD_NOTIFY_BIN`.
///
/// - Returns `Ok(None)` when `NOTIFY_SOCKET` is unset or empty.
/// - Validates `WATCHDOG_USEC >= 3_000` when set and computes `Duration::from_micros(watchdog_usec / 3)`.
pub fn parse_watchdog_env(
    notify_socket: Option<&str>,
    watchdog_usec: Option<&str>,
    notify_bin: Option<&str>,
) -> Result<Option<WatchdogConfig>> {
    let Some(sock) = notify_socket.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };

    let ping_interval = match watchdog_usec.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => {
            let usec: u64 = raw
                .parse()
                .map_err(|_| anyhow!("invalid WATCHDOG_USEC={raw:?}: expected integer >= {MIN_WATCHDOG_USEC}"))?;
            if usec < MIN_WATCHDOG_USEC {
                bail!("invalid WATCHDOG_USEC={usec}: must be >= {MIN_WATCHDOG_USEC}");
            }
            Some(Duration::from_micros(usec / 3))
        }
        None => None,
    };

    let notify_bin = notify_bin
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_SYSTEMD_NOTIFY_BIN)
        .to_string();

    Ok(Some(WatchdogConfig {
        notify_socket: sock.to_string(),
        ping_interval,
        notify_bin,
    }))
}

/// Reads `NOTIFY_SOCKET`, `WATCHDOG_USEC`, and `THUNDER_SYSTEMD_NOTIFY_BIN` from process env.
pub fn parse_watchdog_env_from_os() -> Result<Option<WatchdogConfig>> {
    let notify_socket = std::env::var("NOTIFY_SOCKET").ok();
    let watchdog_usec = std::env::var("WATCHDOG_USEC").ok();
    let notify_bin = std::env::var("THUNDER_SYSTEMD_NOTIFY_BIN").ok();
    parse_watchdog_env(
        notify_socket.as_deref(),
        watchdog_usec.as_deref(),
        notify_bin.as_deref(),
    )
}

/// Builds CLI arguments for `/usr/bin/systemd-notify` fallback invocation.
pub fn build_systemd_notify_args(pid: u32, state: &str) -> Vec<String> {
    let mut args = Vec::new();
    args.push(format!("--pid={pid}"));
    for line in state.lines().filter(|l| !l.is_empty()) {
        args.push(line.to_string());
    }
    args
}

#[cfg(target_os = "linux")]
fn try_unix_datagram(notify_socket: &str, state: &str) -> std::io::Result<()> {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};

    let sock = UnixDatagram::unbound()?;
    let addr = if let Some(abstract_name) = notify_socket
        .strip_prefix('@')
        .or_else(|| notify_socket.strip_prefix('\0'))
    {
        SocketAddr::from_abstract_name(abstract_name.as_bytes())?
    } else {
        SocketAddr::from_pathname(notify_socket)?
    };
    sock.send_to_addr(state.as_bytes(), &addr)?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn fallback_systemd_notify(cfg: &WatchdogConfig, state: &str) -> Result<()> {
    use anyhow::Context;

    let args = build_systemd_notify_args(std::process::id(), state);
    let output = std::process::Command::new(&cfg.notify_bin)
        .env("NOTIFY_SOCKET", &cfg.notify_socket)
        .args(&args)
        .output()
        .with_context(|| format!("failed to execute fallback {}", cfg.notify_bin))?;
    if !output.status.success() {
        bail!(
            "{} failed ({}): {}",
            cfg.notify_bin,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub fn default_sd_notify(cfg: &WatchdogConfig, state: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        match try_unix_datagram(&cfg.notify_socket, state) {
            Ok(()) => Ok(()),
            Err(dgram_err) => {
                tracing::debug!(
                    error = %dgram_err,
                    socket = %cfg.notify_socket,
                    fallback = %cfg.notify_bin,
                    "UnixDatagram sd_notify failed; falling back to systemd-notify binary"
                );
                fallback_systemd_notify(cfg, state)
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        tracing::debug!(
            socket = %cfg.notify_socket,
            state = %state.trim(),
            "Skipping Linux sd_notify transport on non-Linux host"
        );
        Ok(())
    }
}

/// Native systemd `Type=notify` + `WatchdogSec` notifier and background keepalive task.
pub struct SystemdWatchdog {
    cfg: WatchdogConfig,
    notifier: NotifyFn,
    stopped: Arc<AtomicBool>,
    stop_tx: Option<watch::Sender<bool>>,
    task: Option<JoinHandle<()>>,
}

impl SystemdWatchdog {
    /// Starts systemd readiness & watchdog notifications if `NOTIFY_SOCKET` is configured.
    pub fn start_from_env() -> Result<Option<Self>> {
        match parse_watchdog_env_from_os()? {
            Some(cfg) => Ok(Some(Self::start(cfg)?)),
            None => Ok(None),
        }
    }

    /// Sends `READY=1` immediately and spawns the periodic `WATCHDOG=1` task when `ping_interval` is set.
    pub fn start(cfg: WatchdogConfig) -> Result<Self> {
        Self::start_with_notifier(cfg, Arc::new(default_sd_notify))
    }

    pub fn start_with_notifier(cfg: WatchdogConfig, notifier: NotifyFn) -> Result<Self> {
        (notifier)(&cfg, READY_PAYLOAD)?;

        let stopped = Arc::new(AtomicBool::new(false));
        let (stop_tx, task) = if let Some(interval) = cfg.ping_interval {
            let (tx, mut rx) = watch::channel(false);
            let task_cfg = cfg.clone();
            let task_notifier = notifier.clone();
            let task_stopped = stopped.clone();

            let handle = tokio::spawn(async move {
                let start_at = tokio::time::Instant::now() + interval;
                let mut ticker = tokio::time::interval_at(start_at, interval);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tokio::select! {
                        changed = rx.changed() => {
                            if changed.is_err() || *rx.borrow() {
                                break;
                            }
                        }
                        _ = ticker.tick() => {
                            if task_stopped.load(Ordering::Acquire) {
                                break;
                            }
                            if let Err(err) = (task_notifier)(&task_cfg, WATCHDOG_PAYLOAD) {
                                warn!(error = %err, "Systemd WATCHDOG=1 notification failed");
                            }
                        }
                    }
                }
            });
            (Some(tx), Some(handle))
        } else {
            (None, None)
        };

        Ok(Self {
            cfg,
            notifier,
            stopped,
            stop_tx,
            task,
        })
    }

    pub fn config(&self) -> &WatchdogConfig {
        &self.cfg
    }

    pub fn interval(&self) -> Option<Duration> {
        self.cfg.ping_interval
    }

    /// Cancels the periodic `WATCHDOG=1` task and sends `STOPPING=1`. Idempotent.
    pub async fn stop(&mut self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(handle) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
        }
        if let Err(err) = (self.notifier)(&self.cfg, STOPPING_PAYLOAD) {
            warn!(error = %err, "Systemd STOPPING=1 notification failed");
        }
    }
}

impl Drop for SystemdWatchdog {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(handle) = self.task.take() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn unset_or_empty_notify_socket_returns_none() {
        assert_eq!(parse_watchdog_env(None, None, None).unwrap(), None);
        assert_eq!(parse_watchdog_env(Some(""), Some("3000000"), None).unwrap(), None);
        assert_eq!(parse_watchdog_env(None, Some("invalid"), None).unwrap(), None);
    }

    #[test]
    fn valid_watchdog_usec_computes_one_third_interval() {
        let cfg_3s = parse_watchdog_env(Some("/run/systemd/notify"), Some("3000000"), None)
            .unwrap()
            .expect("watchdog config");
        assert_eq!(cfg_3s.notify_socket, "/run/systemd/notify");
        assert_eq!(cfg_3s.ping_interval, Some(Duration::from_secs(1)));
        assert_eq!(cfg_3s.notify_bin, DEFAULT_SYSTEMD_NOTIFY_BIN);

        let cfg_min = parse_watchdog_env(
            Some("@/org/freedesktop/systemd1/notify"),
            Some("3000"),
            Some("/custom/systemd-notify"),
        )
        .unwrap()
        .expect("abstract socket config");
        assert_eq!(cfg_min.notify_socket, "@/org/freedesktop/systemd1/notify");
        assert_eq!(cfg_min.ping_interval, Some(Duration::from_micros(1_000)));
        assert_eq!(cfg_min.notify_bin, "/custom/systemd-notify");

        let cfg_nul = parse_watchdog_env(Some("\0systemd-abstract"), Some("90000"), None)
            .unwrap()
            .expect("nul-prefixed abstract socket");
        assert_eq!(cfg_nul.notify_socket, "\0systemd-abstract");
        assert_eq!(cfg_nul.ping_interval, Some(Duration::from_millis(30)));

        let cfg_ready_only = parse_watchdog_env(Some("/run/systemd/notify"), None, None)
            .unwrap()
            .expect("notify without watchdog_usec");
        assert_eq!(cfg_ready_only.ping_interval, None);
    }

    #[test]
    fn invalid_or_too_small_watchdog_usec_is_rejected() {
        for bad in ["0", "1", "2999", "-3000", "abc", "3.5"] {
            let err = parse_watchdog_env(Some("/run/systemd/notify"), Some(bad), None)
                .expect_err(&format!("WATCHDOG_USEC={bad} must be rejected"));
            assert!(
                err.to_string().contains("WATCHDOG_USEC"),
                "unexpected error message for {bad}: {err}"
            );
        }
    }

    #[test]
    fn systemd_notify_fallback_args_preserve_lines() {
        let args = build_systemd_notify_args(4242, READY_PAYLOAD);
        assert_eq!(
            args,
            vec![
                "--pid=4242",
                "READY=1",
                "STATUS=Rust Brainstem gRPC and RT loop ready",
            ]
        );
    }

    #[tokio::test]
    async fn watchdog_announces_ready_pings_at_interval_and_stops_cleanly() {
        let cfg = parse_watchdog_env(Some("/run/systemd/notify"), Some("60000"), None)
            .unwrap()
            .unwrap();
        assert_eq!(cfg.ping_interval, Some(Duration::from_millis(20)));

        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        let notifier: NotifyFn = Arc::new(move |_, payload| {
            events_clone.lock().unwrap().push(payload.to_string());
            Ok(())
        });

        let mut wd = SystemdWatchdog::start_with_notifier(cfg, notifier).unwrap();
        assert_eq!(events.lock().unwrap().as_slice(), &[READY_PAYLOAD.to_string()]);

        tokio::time::sleep(Duration::from_millis(75)).await;
        let ping_count = events
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.as_str() == WATCHDOG_PAYLOAD)
            .count();
        assert!(ping_count >= 2, "expected >= 2 watchdog pings, got {ping_count}");

        wd.stop().await;
        let snapshot_after_stop = events.lock().unwrap().clone();
        assert_eq!(
            snapshot_after_stop.last().map(String::as_str),
            Some(STOPPING_PAYLOAD)
        );

        // Verify stop() is idempotent and background task no longer fires
        wd.stop().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(*events.lock().unwrap(), snapshot_after_stop);
    }

    #[tokio::test]
    async fn ready_notification_failure_aborts_startup() {
        let cfg = parse_watchdog_env(Some("/run/systemd/notify"), Some("3000000"), None)
            .unwrap()
            .unwrap();
        let failing_notifier: NotifyFn = Arc::new(|_, _| bail!("simulated notify socket error"));
        let err = match SystemdWatchdog::start_with_notifier(cfg, failing_notifier) {
            Ok(_) => panic!("READY=1 failure must fail startup"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("simulated notify socket error"));
    }
}
