use std::sync::{Arc, TryLockError};
use std::time::Duration;

use anyhow::Result;
use tracing::warn;

use crate::server::grpc::SharedBrain;

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
    shared.refresh_telemetry(&engine, shared.now_us());
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
