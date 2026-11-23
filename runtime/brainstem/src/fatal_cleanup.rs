use std::sync::{Arc, TryLockError};
use std::time::Duration;

use anyhow::Result;
use tracing::warn;

use crate::brainstem_grpc::SharedBrain;

fn emergency_disable_motors_with_attempts(shared: &SharedBrain, attempts: usize) -> bool {
    let mut guard_opt = None;
    for _ in 0..attempts {
        match shared.engine.try_lock() {
            Ok(guard) => {
                guard_opt = Some(guard);
                break;
            }
            Err(TryLockError::Poisoned(poisoned)) => {
                guard_opt = Some(poisoned.into_inner());
                break;
            }
            Err(TryLockError::WouldBlock) => {
                std::thread::sleep(Duration::from_micros(250));
            }
        }
    }

    let Some(mut engine) = guard_opt else {
        return false;
    };

    let proprio = *engine.latest_proprio();
    let damp_frame = engine.safety.passive_damping_frame(&proprio);
    let _ = engine.hal.send_mit_verified(&damp_frame);
    let _ = engine.disable_motors(false);
    true
}

/// Acquires `shared.engine` via `try_lock()` (with bounded fallback spin and mutex poison recovery),
/// immediately dispatches a passive damping MIT frame (`kp = 0`, leg `kd = damp_kd_leg`,
/// wheel `dq = 0, kd = damp_kd_wheel`), and invokes `disable_motors(false)` so motors never
/// remain energized on crash or signal termination.
pub fn emergency_disable_motors(shared: &SharedBrain) -> bool {
    let ok = emergency_disable_motors_with_attempts(shared, 200);
    if !ok {
        eprintln!("CRITICAL: emergency_disable_motors timed out acquiring BrainEngine lock");
    }
    ok
}

/// Installs a global panic hook that chains the previous hook and immediately dispatches
/// a passive damping frame + `disable_motors(false)` on any panic in any thread (including
/// post-unwind recovery if the panicking thread itself holds `shared.engine`).
pub fn install_panic_motor_kill_hook(shared: Arc<SharedBrain>) {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let disabled = emergency_disable_motors_with_attempts(&shared, 40);
        if !disabled {
            let deferred = shared.clone();
            let _ = std::thread::Builder::new()
                .name("doso-panic-kill".into())
                .spawn(move || {
                    let _ = emergency_disable_motors_with_attempts(&deferred, 400);
                });
        }
        eprintln!(
            "FATAL PANIC in doso-brainstem (immediate_motor_disable={disabled}): {panic_info}"
        );
        prev_hook(panic_info);
    }));
}

/// Executes every cleanup closure in order even if an earlier cleanup action fails.
pub fn run_best_effort_cleanup<I, F>(actions: I)
where
    I: IntoIterator<Item = F>,
    F: FnOnce() -> Result<()>,
{
    for action in actions {
        if let Err(err) = action() {
            warn!(error = %err, "Best-effort cleanup step returned error");
        }
    }
}

/// Waits for `SIGINT` (`Ctrl+C`) on all platforms and `SIGTERM` on Unix (`systemd stop`).
pub async fn wait_for_shutdown_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => "SIGINT",
                    _ = sigterm.recv() => "SIGTERM",
                }
            }
            Err(err) => {
                warn!(error = %err, "Failed to install SIGTERM listener; waiting on SIGINT only");
                let _ = tokio::signal::ctrl_c().await;
                "SIGINT"
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        "SIGINT"
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Instant;

    use anyhow::bail;
    use doso_brain::BrainEngine;
    use doso_core::{
        ImuState, JointFeedback, MitCmd16, ProprioFrame, RemotePadState, NUM_JOINTS,
        WHEEL_START_INDEX,
    };
    use doso_hal::{HalBackend, HalDiagnostics};
    use doso_safety::SafetyConfig;

    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    enum HalEvent {
        Enabled,
        SentMit(Box<MitCmd16>),
        Disabled { clear_errors: bool },
    }

    struct RecordingHal {
        tx_enabled: bool,
        motors_enabled: bool,
        events: Arc<Mutex<Vec<HalEvent>>>,
    }

    impl RecordingHal {
        fn new(events: Arc<Mutex<Vec<HalEvent>>>) -> Self {
            Self {
                tx_enabled: true,
                motors_enabled: false,
                events,
            }
        }
    }

    impl HalBackend for RecordingHal {
        fn name(&self) -> &'static str {
            "recording-test-hal"
        }

        fn sample_proprio(&mut self, now_us: u64) -> ProprioFrame {
            let joints = std::array::from_fn(|i| JointFeedback {
                q: if i < WHEEL_START_INDEX { 0.25 } else { 0.0 },
                dq: 0.0,
                tau: 0.0,
                temp_c: 30.0,
                status_code: if self.motors_enabled { 2 } else { 0 },
                error_bits: 0,
                vbus_v: 50.0,
                last_seen_us: now_us,
                online: true,
            });
            ProprioFrame {
                sequence: 1,
                monotonic_us: now_us,
                imu: ImuState {
                    timestamp_us: now_us,
                    online: true,
                    ..ImuState::default()
                },
                joints,
            }
        }

        fn sample_remote(&mut self, _now_us: u64) -> RemotePadState {
            RemotePadState::default()
        }

        fn set_tx_enabled(&mut self, enabled: bool) {
            self.tx_enabled = enabled;
        }

        fn tx_enabled(&self) -> bool {
            self.tx_enabled
        }

        fn enable_motors(&mut self) -> Result<()> {
            self.motors_enabled = true;
            self.events.lock().unwrap().push(HalEvent::Enabled);
            Ok(())
        }

        fn disable_motors(&mut self, clear_errors: bool) -> Result<()> {
            self.motors_enabled = false;
            self.events
                .lock()
                .unwrap()
                .push(HalEvent::Disabled { clear_errors });
            Ok(())
        }

        fn send_mit_verified(&mut self, cmd: &MitCmd16) -> Result<()> {
            self.events
                .lock()
                .unwrap()
                .push(HalEvent::SentMit(Box::new(*cmd)));
            Ok(())
        }

        fn diagnostics(&self) -> HalDiagnostics {
            HalDiagnostics {
                backend_name: self.name(),
                tx_enabled: self.tx_enabled,
                tx_frames: 0,
                rx_frames: 0,
                tx_dropped: 0,
            }
        }
    }

    fn make_armed_shared_brain() -> (Arc<SharedBrain>, Arc<Mutex<Vec<HalEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let hal = Box::new(RecordingHal::new(events.clone()));
        let safety_cfg = SafetyConfig {
            allow_motor_enable: true,
            ..SafetyConfig::default()
        };
        let mut engine = BrainEngine::new(hal, safety_cfg);
        engine.tick_200hz(1_000);
        engine.enable_motors().unwrap();
        assert!(engine.safety.motor_output_enabled());
        events.lock().unwrap().clear();

        let shared = Arc::new(SharedBrain {
            engine: Mutex::new(engine),
            boot_instant: Instant::now(),
        });
        (shared, events)
    }

    #[test]
    fn emergency_disable_dispatches_passive_damping_then_disables_motors() {
        let (shared, events) = make_armed_shared_brain();

        assert!(emergency_disable_motors(&shared));

        let guard = shared.engine.lock().unwrap();
        assert!(!guard.safety.motor_output_enabled());

        let recorded = events.lock().unwrap().clone();
        assert_eq!(recorded.len(), 2, "expected passive damping frame + disable_motors(false)");

        match &recorded[0] {
            HalEvent::SentMit(cmd) => {
                for i in 0..NUM_JOINTS {
                    assert_eq!(cmd.0[i].kp, 0.0, "passive damping must have kp == 0 on joint {i}");
                    if i < WHEEL_START_INDEX {
                        assert!((cmd.0[i].q - 0.25).abs() < 1e-9);
                        assert!((cmd.0[i].kd - 8.0).abs() < 1e-9);
                    } else {
                        assert_eq!(cmd.0[i].dq, 0.0);
                        assert!((cmd.0[i].kd - 1.0).abs() < 1e-9);
                    }
                }
            }
            other => panic!("expected SentMit passive damping first, got {other:?}"),
        }
        assert_eq!(recorded[1], HalEvent::Disabled { clear_errors: false });
    }

    #[test]
    fn emergency_disable_recovers_from_poisoned_engine_mutex() {
        let (shared, events) = make_armed_shared_brain();
        let poison_target = shared.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poison_target.engine.lock().unwrap();
            panic!("simulated RT panic while holding engine lock");
        })
        .join();
        assert!(shared.engine.is_poisoned());

        assert!(emergency_disable_motors(&shared));
        let recorded = events.lock().unwrap().clone();
        assert_eq!(recorded.len(), 2);
        assert!(matches!(recorded[0], HalEvent::SentMit(_)));
        assert_eq!(recorded[1], HalEvent::Disabled { clear_errors: false });
    }

    #[test]
    fn panic_hook_de_energizes_motors_on_worker_thread_panic() {
        let (shared, events) = make_armed_shared_brain();
        install_panic_motor_kill_hook(shared.clone());

        let crash_res = std::thread::spawn(|| {
            panic!("simulated background worker crash");
        })
        .join();
        assert!(crash_res.is_err());

        let guard = shared.engine.lock().unwrap();
        assert!(!guard.safety.motor_output_enabled());

        let recorded = events.lock().unwrap().clone();
        assert!(
            recorded.len() >= 2,
            "expected panic hook to dispatch damping + disable, got {recorded:?}"
        );
        assert!(matches!(recorded[0], HalEvent::SentMit(_)));
        assert_eq!(recorded[1], HalEvent::Disabled { clear_errors: false });
    }

    #[test]
    fn best_effort_cleanup_continues_after_earlier_failure() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let t1 = trace.clone();
        let t2 = trace.clone();
        let t3 = trace.clone();

        let steps: Vec<Box<dyn FnOnce() -> Result<()>>> = vec![
            Box::new(move || {
                t1.lock().unwrap().push("disable-motors");
                Ok(())
            }),
            Box::new(move || {
                t2.lock().unwrap().push("watchdog-stop");
                bail!("simulated watchdog error")
            }),
            Box::new(move || {
                t3.lock().unwrap().push("gateway-shutdown");
                Ok(())
            }),
        ];
        run_best_effort_cleanup(steps);

        assert_eq!(
            *trace.lock().unwrap(),
            vec!["disable-motors", "watchdog-stop", "gateway-shutdown"]
        );
    }
}
