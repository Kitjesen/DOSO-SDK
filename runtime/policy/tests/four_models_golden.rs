use std::path::PathBuf;

use doso_core::{ProprioFrame, WalkCommand};
use doso_policy::{AnalyticalMockBackend, LoadedPolicyPackage, ShadowEvaluator};

fn packages_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("models")
        .join("packages")
}

#[test]
fn all_four_real_onnx_models_match_dart_brainstem_goldens_to_1e_5() {
    let root = packages_root();
    for pkg_name in [
        "flat_w5",
        "single_frame_height_v2",
        "stairs_cts_v39",
        "recovery_v89",
    ] {
        let dir = root.join(pkg_name);
        let mut loaded = LoadedPolicyPackage::load_from_dir(&dir)
            .unwrap_or_else(|e| panic!("Failed to load & Golden-verify {pkg_name}: {e:#}"));
        assert!(
            loaded.report.golden_steps_checked >= 1,
            "{pkg_name} must execute >= 1 Golden verification step"
        );
        assert!(
            loaded.report.golden_max_abs_err < 1e-5,
            "{pkg_name} max_abs_err={:.3e} exceeded 1e-5!",
            loaded.report.golden_max_abs_err
        );

        // Verify zero-risk Shadow Mode evaluator runs 20 frames cleanly
        let dummy_frames = vec![ProprioFrame::default(); 20];
        let shadow = ShadowEvaluator::evaluate_frames(
            &mut loaded,
            &dummy_frames,
            WalkCommand {
                vx: 0.3,
                vy: 0.0,
                yaw: 0.0,
            },
            0.38,
        )
        .unwrap();
        assert_eq!(shadow.steps, 20);
        println!(
            "[PASS] {:<24} golden_steps={} max_err={:.6e} warmup={}us shadow_p99={}us",
            loaded.report.name,
            loaded.report.golden_steps_checked,
            loaded.report.golden_max_abs_err,
            loaded.report.warmup_last_us,
            shadow.p99_us
        );
    }
}

#[test]
fn golden_gate_immediately_rejects_corrupted_action_expectations() {
    let dir = packages_root().join("single_frame_height_v2");
    let yaml = std::fs::read_to_string(dir.join("policy.yaml")).unwrap();
    let bad_golden = r#"
max_abs_tolerance: 1e-4
steps:
  - gyro_rad_s: [0.0, 0.0, 0.0]
    projected_gravity: [0.0, 0.0, -1.0]
    velocity_cmd: [0.5, 0.0, 0.0]
    body_height_cmd: 0.38
    q: [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
    dq: [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
    expected_raw_action: [99.0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
"#;
    let res = LoadedPolicyPackage::load_and_verify(
        &yaml.replace("sha256:", "# sha256:"),
        b"dummy",
        Some(bad_golden),
        Some(Box::new(AnalyticalMockBackend::default())),
    );
    assert!(res.is_err());
    let msg = format!("{:#}", res.err().unwrap());
    assert!(msg.contains("Golden gate REJECTED"), "unexpected error: {msg}");
}
