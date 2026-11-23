pub mod grpc;
pub mod hot_deploy;
pub mod watchdog;

use std::path::Path;

use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::control::BrainEngine;
use crate::policy::LoadedPolicyPackage;

pub use grpc::{BrainstemGrpcServer, LocalStatusGrpcServer, SharedBrain, TelemetrySnapshot};
pub use hot_deploy::{
    build_hot_deploy_router, build_hot_deploy_router_with_config, generate_boot_id,
    resolve_release_metadata, write_boot_id_marker, HotDeployConfig, PersistedSlotBindings,
    PushPolicyRequest, RollbackRequest, DEFAULT_BOOT_ID_MARKER_PATH, SLOT_BINDINGS_FILE,
};
pub use watchdog::{
    build_systemd_notify_args, default_sd_notify, parse_watchdog_env, parse_watchdog_env_from_os,
    NotifyFn, SystemdWatchdog, WatchdogConfig, DEFAULT_SYSTEMD_NOTIFY_BIN, MIN_WATCHDOG_USEC,
    READY_PAYLOAD, STOPPING_PAYLOAD, WATCHDOG_PAYLOAD,
};

/// Preloads the 4 canonical Thunder baseline policy packages (`flat_w5`, `stairs_cts_v39`,
/// `single_frame_height_v2`, `recovery_v89`), any extra hot-persisted packages found in
/// `packages_dir`, and restores persisted slot bindings (if `.doso_slots.json` is present).
pub fn preload_packages(engine: &mut BrainEngine, packages_dir: &Path) -> Result<()> {
    let slot_defaults = [
        ("flat_w5", Some("flat")),
        ("stairs_cts_v39", Some("stairs")),
        ("single_frame_height_v2", Some("height")),
        ("recovery_v89", None),
    ];
    for (dir_name, slot) in slot_defaults {
        let pkg_path = packages_dir.join(dir_name);
        if pkg_path.join("policy.yaml").is_file() {
            let pkg = LoadedPolicyPackage::load_from_dir(&pkg_path).with_context(|| {
                format!("failed loading baseline package {}", pkg_path.display())
            })?;
            let r = engine.register_policy(pkg, slot)?;
            info!(
                policy = %r.name,
                slot = ?slot,
                warmup_us = r.warmup_last_us,
                golden_steps = r.golden_steps_checked,
                golden_max_err = %format!("{:.2e}", r.golden_max_abs_err),
                "preloaded & Golden-verified policy package"
            );
        }
    }
    if let Ok(entries) = std::fs::read_dir(packages_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            let Some(fname) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            // Skip hidden staging/backup dirs (e.g. `.foo.prev`, `.foo.tmp.*`) and baseline dirs.
            if fname.starts_with('.')
                || slot_defaults.iter().any(|(d, _)| *d == fname)
                || !p.join("policy.yaml").is_file()
            {
                continue;
            }
            match LoadedPolicyPackage::load_from_dir(&p) {
                Ok(pkg) => {
                    let r = engine.register_policy(pkg, None)?;
                    info!(
                        policy = %r.name,
                        warmup_us = r.warmup_last_us,
                        "loaded extra policy package"
                    );
                }
                Err(e) => {
                    warn!(dir = %p.display(), error = %e, "skipped invalid extra policy package")
                }
            }
        }
    }

    // Restore persisted hot-deploy slot bindings & active profile across restarts.
    let slots_file = packages_dir.join(SLOT_BINDINGS_FILE);
    if slots_file.is_file() {
        match std::fs::read_to_string(&slots_file)
            .ok()
            .and_then(|raw| serde_json::from_str::<PersistedSlotBindings>(&raw).ok())
        {
            Some(saved) => {
                for (slot, target) in saved.slots {
                    if let Err(err) = engine.policies.bind_slot(&slot, &target) {
                        warn!(slot = %slot, target = %target, error = %err, "skipped stale persisted slot binding");
                    }
                }
                if let Some(active) = saved.active_profile {
                    if engine.switch_profile(&active).is_ok() {
                        return Ok(());
                    }
                }
            }
            None => warn!(
                path = %slots_file.display(),
                "ignored unreadable persisted slot bindings file"
            ),
        }
    }

    if engine.policies.slots().contains_key("flat") {
        let _ = engine.switch_profile("flat");
    }
    Ok(())
}
