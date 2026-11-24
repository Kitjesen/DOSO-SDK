use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::bail;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use brainstem_dds_bridge::{spawn_base_gateway_on_listener, BaseGatewayConfig, BaseGatewayService};
use doso_runtime::control::BrainEngine;
use doso_runtime::hardware::hipnuc::rotate_unit_neg_z_by_quat;
use doso_runtime::hardware::BenchNullHal;
use doso_runtime::policy::PolicyManifest;
use doso_runtime::safety::SafetyConfig;
use doso_runtime::server::{
    build_hot_deploy_router, build_systemd_notify_args, parse_watchdog_env, preload_packages,
    BrainstemGrpcServer, NotifyFn, PushPolicyRequest, RollbackRequest, SharedBrain,
    SystemdWatchdog, DEFAULT_SYSTEMD_NOTIFY_BIN, READY_PAYLOAD, STOPPING_PAYLOAD, WATCHDOG_PAYLOAD,
};
use doso_sdk::internal::motion_board::{
    rpc::{
        self, robot_control_client::RobotControlClient, robot_control_server::RobotControlServer,
        robot_external_control_server::RobotExternalControlServer,
        robot_recovery_server::RobotRecoveryServer,
    },
    MotionBoardClient,
};
use doso_sdk::RobotClient;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn packages_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("models")
        .join("packages")
}

async fn admin_http(
    method: &str,
    addr: std::net::SocketAddr,
    path: &str,
    body: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let b = body.unwrap_or("").as_bytes();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        b.len()
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    if !b.is_empty() {
        stream.write_all(b).await.unwrap();
    }
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw);
    let (head, resp_body) = text.split_once("\r\n\r\n").unwrap();
    let code = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(500);
    (code, resp_body.to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn single_process_rust_brainstem_e2e_and_sub_second_hot_deploy() {
    let mut engine = BrainEngine::new(
        Box::new(BenchNullHal::new(false)),
        SafetyConfig {
            allow_motor_enable: false,
            ..SafetyConfig::default()
        },
    );
    let pkgs = packages_root();
    preload_packages(&mut engine, &pkgs).expect("all 4 Thunder baseline packages must preload");

    let shared = Arc::new(SharedBrain::new(engine));

    // 1. Start 200Hz RT tick thread recording mutex wait, tick duration, and decoupled telemetry
    let stop = Arc::new(AtomicBool::new(false));
    let rt_stop = stop.clone();
    let rt_shared = shared.clone();
    let rt_join = std::thread::spawn(move || {
        let period = Duration::from_millis(5);
        let mut next_deadline = Instant::now() + period;
        let mut jitter_us = 0u64;
        while !rt_stop.load(Ordering::Relaxed) {
            let tick_start = Instant::now();
            let now_us = rt_shared.now_us();
            let mutex_wait_us = {
                let mut g = rt_shared.engine.lock().unwrap();
                let wait_us = tick_start.elapsed().as_micros() as u64;
                g.tick_200hz(now_us);
                rt_shared.refresh_telemetry(&g, now_us);
                wait_us
            };
            let tick_us = tick_start.elapsed().as_micros() as u64;
            rt_shared.record_rt_sample(mutex_wait_us, tick_us, jitter_us);

            let before_sleep = Instant::now();
            if next_deadline > before_sleep {
                std::thread::sleep(next_deadline - before_sleep);
                let wake = Instant::now();
                jitter_us = wake.saturating_duration_since(next_deadline).as_micros() as u64;
                next_deadline += period;
                if next_deadline < wake {
                    next_deadline = wake + period;
                }
            } else {
                jitter_us = before_sleep
                    .saturating_duration_since(next_deadline)
                    .as_micros() as u64;
                next_deadline = before_sleep + period;
            }
        }
    });

    // 2. Start internal Brainstem gRPC server on ephemeral port
    let bs_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bs_addr = bs_listener.local_addr().unwrap();
    let bs_svc = Arc::new(BrainstemGrpcServer {
        shared: shared.clone(),
    });
    tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(RobotControlServer::from_arc(bs_svc.clone()))
            .add_service(RobotExternalControlServer::from_arc(bs_svc.clone()))
            .add_service(RobotRecoveryServer::from_arc(bs_svc))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(bs_listener))
            .await;
    });

    // 3. Start public `doso.v1` BaseGateway on ephemeral port
    let gw_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_cfg = BaseGatewayConfig {
        brainstem_endpoint: format!("http://127.0.0.1:{}", bs_addr.port()),
        listen_addr: "127.0.0.1:0".into(),
        robot_id: Some("THUNDER-RUST-TEST".into()),
        robot_model: "thunder".into(),
        firmware_version: "v1.0.0-rust".into(),
        shm_enabled: false,
        ..BaseGatewayConfig::default()
    };
    let (gw_tx, gw_rx) = tokio::sync::oneshot::channel::<()>();
    let (gw_addr, _gw_handle) =
        spawn_base_gateway_on_listener(BaseGatewayService::new(gw_cfg), gw_listener, gw_rx)
            .await
            .unwrap();

    // 4. Start `:5054`-equivalent Hot-Deploy HTTP API
    let admin_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let admin_addr = admin_listener.local_addr().unwrap();
    let router = build_hot_deploy_router(shared.clone());
    tokio::spawn(async move {
        let _ = axum::serve(admin_listener, router).await;
    });

    // 5. Connect real `RobotClient` and verify Gestures + BodyHeight + Recovery + Profiles
    let client = RobotClient::connect(format!("http://{gw_addr}"))
        .await
        .expect("RobotClient handshake to single-process Rust Brainstem must succeed");

    let gestures = client.motion().list_gestures().await.unwrap();
    let gesture_names: Vec<_> = gestures.gestures.iter().map(|g| g.name.as_str()).collect();
    for expected in ["bow", "nod", "wiggle", "stretch", "dance"] {
        assert!(gesture_names.contains(&expected));
    }

    let prof = client.motion().profile().await.unwrap();
    let baseline_flat_name = prof.current.clone();
    assert!(baseline_flat_name.starts_with("flat_w5"));

    client
        .motion()
        .switch_profile("height", None)
        .await
        .unwrap();
    let bh = client.motion().set_body_height(0.42, None).await.unwrap();
    assert!(bh.supported);
    assert!((bh.target_height_m - 0.42).abs() < 1e-6);

    client.motion().switch_profile("flat", None).await.unwrap();
    let rec0 = client.motion().recovery_status().await.unwrap();
    assert_eq!(rec0.availability, 2);
    let rec_entered = client
        .motion()
        .recovery_enter("op-e2e-1", None)
        .await
        .unwrap();
    assert!(rec_entered.selected && !rec_entered.session_token.is_empty());
    let rec_hb = client
        .motion()
        .recovery_heartbeat(&rec_entered.session_token, None)
        .await
        .unwrap();
    assert!(rec_hb.selected);
    let rec_exited = client
        .motion()
        .recovery_exit(&rec_entered.session_token, None)
        .await
        .unwrap();
    assert!(!rec_exited.selected);

    // 6. Hot-Deploy Gate: corrupted Golden trajectory MUST be rejected with HTTP 422
    let flat_dir = pkgs.join("flat_w5");
    let base_yaml = std::fs::read_to_string(flat_dir.join("policy.yaml")).unwrap();
    let parsed_manifest = PolicyManifest::from_yaml_str(&base_yaml).unwrap();
    let onnx_b64 = B64.encode(std::fs::read(flat_dir.join(&parsed_manifest.model.file)).unwrap());
    let good_golden = std::fs::read_to_string(flat_dir.join("golden.json")).unwrap();
    let bad_golden = good_golden.replacen(
        "\"expected_raw_action\": [",
        "\"expected_raw_action\": [999.0, ",
        1,
    );

    let bad_yaml = base_yaml.replacen(
        &format!("name: {baseline_flat_name}"),
        "name: flat_w5_bad_candidate",
        1,
    );
    let bad_payload = serde_json::to_string(&PushPolicyRequest {
        manifest_yaml: bad_yaml,
        onnx_base64: onnx_b64.clone(),
        golden_json: Some(bad_golden),
        slot: Some("flat".into()),
        shadow_frames: 20,
        activate: true,
    })
    .unwrap();
    let (bad_code, bad_body) =
        admin_http("POST", admin_addr, "/v1/policies/push", Some(&bad_payload)).await;
    assert_eq!(
        bad_code, 422,
        "Golden mismatch must be rejected: {bad_body}"
    );
    let bad_json: serde_json::Value = serde_json::from_str(&bad_body).unwrap();
    assert_eq!(bad_json["ok"], false);
    assert_eq!(bad_json["stage"], "golden_or_schema_gate");
    assert_eq!(
        client.motion().profile().await.unwrap().current,
        baseline_flat_name
    );

    // 7. Zero-Restart Hot-Deploy + off-lock 50-frame Shadow Mode zero-stall RT test
    // Wait until the 200Hz ring buffer actually holds >= 55 frames (avoids Windows 15.6ms sleep quantum underfill).
    let fill_deadline = Instant::now() + Duration::from_secs(5);
    while shared.rt_tick_count.load(Ordering::Relaxed) < 55 {
        assert!(
            Instant::now() < fill_deadline,
            "RT loop failed to accumulate 55 frames within 5s (got {})",
            shared.rt_tick_count.load(Ordering::Relaxed)
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    shared.reset_rt_metrics();
    let tele_before_flat = shared.telemetry.read().unwrap().clone();

    let poll_done = Arc::new(AtomicBool::new(false));
    let poll_done_bg = poll_done.clone();
    let poll_shared = shared.clone();
    let poll_task = tokio::spawn(async move {
        let mut reads = 0u64;
        let mut distinct_refreshes = 0u64;
        let mut prev_rc = poll_shared.telemetry.read().unwrap().refresh_count;
        while !poll_done_bg.load(Ordering::Relaxed) {
            {
                let snap = poll_shared.telemetry.read().unwrap();
                if snap.refresh_count != prev_rc {
                    prev_rc = snap.refresh_count;
                    distinct_refreshes += 1;
                }
                reads += 1;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        (reads, distinct_refreshes)
    });

    let hot_yaml = base_yaml.replacen(
        &format!("name: {baseline_flat_name}"),
        "name: flat_w5_hot_v2",
        1,
    );
    let hot_payload = serde_json::to_string(&PushPolicyRequest {
        manifest_yaml: hot_yaml,
        onnx_base64: onnx_b64,
        golden_json: Some(good_golden),
        slot: Some("flat".into()),
        shadow_frames: 50,
        activate: true,
    })
    .unwrap();
    let t_push = Instant::now();
    let (ok_code, ok_body) =
        admin_http("POST", admin_addr, "/v1/policies/push", Some(&hot_payload)).await;
    let push_ms = t_push.elapsed().as_secs_f64() * 1000.0;
    println!("E2E_HOT_DEPLOY_OK in {push_ms:.2}ms -> {ok_body}");
    assert_eq!(ok_code, 200, "Valid hot deploy failed: {ok_body}");
    let ok_json: serde_json::Value = serde_json::from_str(&ok_body).unwrap();
    assert_eq!(ok_json["ok"], true);
    assert_eq!(ok_json["name"], "flat_w5_hot_v2");
    assert_eq!(ok_json["bound_slot"], "flat");
    assert_eq!(ok_json["active_profile"], "flat_w5_hot_v2");
    assert_eq!(ok_json["shadow"]["steps"], 50);
    assert!(push_ms < 2_000.0);
    assert_eq!(
        client.motion().profile().await.unwrap().current,
        "flat_w5_hot_v2"
    );

    let stairs_dir = pkgs.join("stairs_cts_v39");
    let stairs_base_yaml = std::fs::read_to_string(stairs_dir.join("policy.yaml")).unwrap();
    let stairs_manifest = PolicyManifest::from_yaml_str(&stairs_base_yaml).unwrap();
    let stairs_onnx_b64 =
        B64.encode(std::fs::read(stairs_dir.join(&stairs_manifest.model.file)).unwrap());
    let stairs_golden = std::fs::read_to_string(stairs_dir.join("golden.json")).unwrap();
    let stairs_hot_yaml = stairs_base_yaml.replacen(
        &format!("name: {}", stairs_manifest.name),
        "name: stairs_cts_v39_hot_v2",
        1,
    );
    let stairs_payload = serde_json::to_string(&PushPolicyRequest {
        manifest_yaml: stairs_hot_yaml,
        onnx_base64: stairs_onnx_b64,
        golden_json: Some(stairs_golden),
        slot: Some("stairs".into()),
        shadow_frames: 50,
        activate: false,
    })
    .unwrap();
    let (st_code, st_body) = admin_http(
        "POST",
        admin_addr,
        "/v1/policies/push",
        Some(&stairs_payload),
    )
    .await;
    assert_eq!(
        st_code, 200,
        "Stairs 50-frame shadow hot deploy failed: {st_body}"
    );

    poll_done.store(true, Ordering::Relaxed);
    let (tele_reads, tele_distinct_refreshes) = poll_task.await.unwrap();
    let tele_after = shared.telemetry.read().unwrap().clone();
    let max_mutex_wait_us = shared.rt_mutex_wait_max_us.load(Ordering::Relaxed);
    let max_tick_us = shared.rt_tick_max_us.load(Ordering::Relaxed);
    let max_jitter_us = shared.rt_jitter_max_us.load(Ordering::Relaxed);
    println!(
        "RT_CONTENTION_DURING_50_SHADOW_DEPLOY: max_mutex_wait_us={max_mutex_wait_us} max_tick_us={max_tick_us} max_jitter_us={max_jitter_us} tele_reads={tele_reads} tele_refreshes={tele_distinct_refreshes}"
    );

    assert!(
        tele_after.refresh_count > tele_before_flat.refresh_count && tele_distinct_refreshes >= 1
    );
    assert!(tele_after.updated_us > tele_before_flat.updated_us);
    assert_eq!(tele_after.status.active_profile, "flat_w5_hot_v2");
    let mutex_budget_us = if cfg!(debug_assertions) { 5_000 } else { 2_000 };
    let tick_budget_us = if cfg!(debug_assertions) {
        15_000
    } else {
        2_000
    };
    assert!(
        max_mutex_wait_us < mutex_budget_us,
        "max_mutex_wait_us={max_mutex_wait_us} exceeded budget {mutex_budget_us}"
    );
    assert!(
        max_tick_us < tick_budget_us,
        "max_tick_us={max_tick_us} exceeded budget {tick_budget_us}"
    );

    let (list_code, list_body) = admin_http("GET", admin_addr, "/v1/policies", None).await;
    assert_eq!(list_code, 200);
    assert!(list_body.contains("\"rt_loop\":{\"max_tick_us\":"));
    let list_json: serde_json::Value = serde_json::from_str(&list_body).unwrap();
    assert_eq!(list_json["ok"], true);
    assert_eq!(list_json["active_profile"], "flat_w5_hot_v2");

    // 8. Instant 1-Command Rollback
    let t_rb = Instant::now();
    let rb_req = serde_json::to_string(&RollbackRequest {
        slot: "flat".into(),
    })
    .unwrap();
    let (rb_code, rb_body) =
        admin_http("POST", admin_addr, "/v1/policies/rollback", Some(&rb_req)).await;
    let rb_ms = t_rb.elapsed().as_secs_f64() * 1000.0;
    println!("E2E_ROLLBACK_OK in {rb_ms:.2}ms -> {rb_body}");
    assert_eq!(rb_code, 200);
    let rb_json: serde_json::Value = serde_json::from_str(&rb_body).unwrap();
    assert_eq!(rb_json["ok"], true);
    assert_eq!(rb_json["slot"], "flat");
    assert_eq!(rb_json["restored_policy"], baseline_flat_name);
    assert_eq!(
        client.motion().profile().await.unwrap().current,
        baseline_flat_name
    );

    // 9. SpeedMode, ListenHistory, SetZero gate, and deterministic Step + Tick
    let mb_client = MotionBoardClient::connect(format!("http://{bs_addr}"))
        .await
        .expect("MotionBoardClient must connect to internal Brainstem gRPC");
    assert_eq!(
        mb_client.cms_state().await.unwrap().kind,
        rpc::CmsStateKind::Grounded as i32
    );

    let mut rc_client = RobotControlClient::connect(format!("http://{bs_addr}"))
        .await
        .expect("RobotControlClient must connect to internal Brainstem gRPC");

    assert_eq!(
        rc_client
            .get_speed_mode(())
            .await
            .unwrap()
            .into_inner()
            .mode,
        rpc::SpeedMode::Normal as i32
    );
    rc_client
        .set_speed_mode(rpc::SpeedModeRequest {
            mode: rpc::SpeedMode::High as i32,
        })
        .await
        .unwrap();
    assert_eq!(
        rc_client
            .get_speed_mode(())
            .await
            .unwrap()
            .into_inner()
            .mode,
        rpc::SpeedMode::High as i32
    );
    rc_client
        .set_speed_mode(rpc::SpeedModeRequest {
            mode: rpc::SpeedMode::Normal as i32,
        })
        .await
        .unwrap();
    assert_eq!(
        rc_client
            .get_speed_mode(())
            .await
            .unwrap()
            .into_inner()
            .mode,
        rpc::SpeedMode::Normal as i32
    );

    let mut hist_stream = rc_client.listen_history(()).await.unwrap().into_inner();
    let live_hist = tokio::time::timeout(Duration::from_secs(1), hist_stream.message())
        .await
        .expect("ListenHistory timed out")
        .unwrap()
        .expect("expected live History frame");
    assert_eq!(live_hist.joint_position.as_ref().unwrap().values.len(), 16);

    let sz_err = rc_client.set_zero(()).await.unwrap_err();
    assert_eq!(sz_err.code(), tonic::Code::PermissionDenied);

    stop.store(true, Ordering::Relaxed);
    let _ = rt_join.join();

    let half_pitch = 0.15_f64;
    let (qw, qy) = (half_pitch.cos(), half_pitch.sin());
    let sim_pos: Vec<f64> = (0..16).map(|i| (i as f64) * 0.01 - 0.08).collect();
    let sim_vel: Vec<f64> = (0..16).map(|i| (i as f64) * 0.02 - 0.10).collect();

    rc_client
        .step(rpc::SimState {
            gyroscope: Some(rpc::Vector3 {
                x: 0.11,
                y: -0.22,
                z: 0.33,
            }),
            quaternion: Some(rpc::Quaternion {
                w: qw,
                x: 0.0,
                y: qy,
                z: 0.0,
            }),
            joint_position: Some(rpc::Matrix4 {
                values: sim_pos.clone(),
            }),
            joint_velocity: Some(rpc::Matrix4 {
                values: sim_vel.clone(),
            }),
            timestamp: None,
        })
        .await
        .expect("Step(SimState) must succeed on bench backend");

    let sim_hist = rc_client
        .tick(())
        .await
        .expect("Tick() must succeed on bench backend")
        .into_inner();
    let gyro = sim_hist.gyroscope.expect("History.gyroscope");
    assert!((gyro.x - 0.11).abs() < 1e-12);
    assert!((gyro.y - (-0.22)).abs() < 1e-12);
    assert!((gyro.z - 0.33).abs() < 1e-12);

    let expected_grav = rotate_unit_neg_z_by_quat(qw, 0.0, qy, 0.0);
    let grav = sim_hist
        .projected_gravity
        .expect("History.projected_gravity");
    assert!((grav.x - expected_grav[0]).abs() < 1e-12);
    assert!((grav.y - expected_grav[1]).abs() < 1e-12);
    assert!((grav.z - expected_grav[2]).abs() < 1e-12);

    assert_eq!(sim_hist.joint_position.unwrap().values, sim_pos);
    assert_eq!(sim_hist.joint_velocity.unwrap().values, sim_vel);

    // 10. All 7 decoupled telemetry gRPC RPCs succeed with ZERO `shared.engine` mutex contention
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel::<()>();
    let (unlock_tx, unlock_rx) = std::sync::mpsc::channel::<()>();
    let lock_holder_shared = shared.clone();
    let lock_holder = std::thread::spawn(move || {
        let _guard = lock_holder_shared.engine.lock().unwrap();
        let _ = locked_tx.send(());
        let _ = unlock_rx.recv();
    });
    locked_rx.await.unwrap();

    tokio::time::timeout(Duration::from_millis(500), async {
        let cms = rc_client.get_cms_state(()).await.unwrap().into_inner();
        assert_eq!(cms.kind, rpc::CmsStateKind::Grounded as i32);

        let volts = rc_client.get_voltage(()).await.unwrap().into_inner();
        assert_eq!(volts.values.len(), 16);

        let motors = rc_client.get_motor_status(()).await.unwrap().into_inner();
        assert_eq!(motors.motors.len(), 16);
        assert!((motors.motors[0].position - sim_pos[0]).abs() < 1e-12);

        let mut imu_s = rc_client.listen_imu(()).await.unwrap().into_inner();
        let imu_msg = imu_s.message().await.unwrap().unwrap();
        assert!((imu_msg.gyroscope.unwrap().x - 0.11).abs() < 1e-12);

        let mut joint_s = rc_client.listen_joint(()).await.unwrap().into_inner();
        let joint_msg = joint_s.message().await.unwrap().unwrap();
        if let Some(rpc::joint::Data::AllJoints(all)) = joint_msg.data {
            assert_eq!(all.position.unwrap().values, sim_pos);
        } else {
            panic!("expected AllJoints");
        }

        let mut cms_s = rc_client.listen_cms_state(()).await.unwrap().into_inner();
        let cms_msg = cms_s.message().await.unwrap().unwrap();
        assert_eq!(cms_msg.kind, rpc::CmsStateKind::Grounded as i32);

        let mut hist_s = rc_client.listen_history(()).await.unwrap().into_inner();
        let hist_msg = hist_s.message().await.unwrap().unwrap();
        assert_eq!(hist_msg.joint_position.unwrap().values, sim_pos);
    })
    .await
    .expect("decoupled telemetry gRPC readers must never block on shared.engine mutex");

    let _ = unlock_tx.send(());
    let _ = lock_holder.join();
    let _ = gw_tx.send(());
}

// ─── Systemd Type=notify Watchdog Tests ───────────────────────────────────────

#[test]
fn unset_or_empty_notify_socket_returns_none() {
    assert_eq!(parse_watchdog_env(None, None, None).unwrap(), None);
    assert_eq!(
        parse_watchdog_env(Some(""), Some("3000000"), None).unwrap(),
        None
    );
    assert_eq!(
        parse_watchdog_env(None, Some("invalid"), None).unwrap(),
        None
    );
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
        assert!(err.to_string().contains("WATCHDOG_USEC"));
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
    assert_eq!(
        events.lock().unwrap().as_slice(),
        &[READY_PAYLOAD.to_string()]
    );

    tokio::time::sleep(Duration::from_millis(140)).await;
    let ping_count = events
        .lock()
        .unwrap()
        .iter()
        .filter(|m| m.as_str() == WATCHDOG_PAYLOAD)
        .count();
    assert!(
        ping_count >= 2,
        "expected >= 2 watchdog pings, got {ping_count}"
    );

    wd.stop().await;
    let snapshot_after_stop = events.lock().unwrap().clone();
    assert_eq!(
        snapshot_after_stop.last().map(String::as_str),
        Some(STOPPING_PAYLOAD)
    );

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

#[tokio::test]
async fn ota_health_auth_policy_persistence_and_local_status_13146() {
    use doso_runtime::server::{
        build_hot_deploy_router_with_config, generate_boot_id, write_boot_id_marker,
        HotDeployConfig, LocalStatusGrpcServer, SLOT_BINDINGS_FILE,
    };

    let tmp_root =
        std::env::temp_dir().join(format!("doso-ota-persist-test-{}", generate_boot_id()));
    std::fs::create_dir_all(&tmp_root).unwrap();
    let boot_id = generate_boot_id();
    let boot_marker = tmp_root.join("brainstem.boot_id");
    write_boot_id_marker(&boot_marker, &boot_id);
    assert_eq!(
        std::fs::read_to_string(&boot_marker).unwrap().trim(),
        boot_id
    );

    let mut engine = BrainEngine::new(Box::new(BenchNullHal::new(false)), SafetyConfig::default());
    preload_packages(&mut engine, &packages_root()).unwrap();
    let shared = Arc::new(SharedBrain::new(engine));

    // 1. Start :5054 router with Bearer token + packages_dir persistence + /health
    let admin_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let admin_addr = admin_listener.local_addr().unwrap();
    let router = build_hot_deploy_router_with_config(
        shared.clone(),
        HotDeployConfig {
            packages_dir: Some(tmp_root.clone()),
            admin_token: Some("secret-ota-token".into()),
            boot_id: boot_id.clone(),
            version: "v0.0.1".into(),
            release_id: "rel-v0.0.1".into(),
            health_signing_key_path: None,
        },
    );
    let admin_task = tokio::spawn(async move {
        let _ = axum::serve(admin_listener, router).await;
    });

    // /health is readable without Bearer token and reports boot_id & readiness
    let (h_code, h_body) = admin_http("GET", admin_addr, "/health", None).await;
    assert_eq!(h_code, 200, "GET /health body={h_body}");
    let h_json: serde_json::Value = serde_json::from_str(&h_body).unwrap();
    assert_eq!(h_json["ok"], true);
    assert_eq!(h_json["boot_id"], boot_id);
    assert_eq!(h_json["version"], "v0.0.1");
    assert_eq!(h_json["brainstem_ready"], true);
    assert_eq!(h_json["strategy_ready"], true);
    assert_eq!(h_json["can_read_only"], true);

    // /v1/policies requires Bearer token
    let (unauth_code, _) = admin_http("GET", admin_addr, "/v1/policies", None).await;
    assert_eq!(unauth_code, 401);

    // Helper for authorized HTTP request
    async fn admin_http_auth(
        method: &str,
        addr: std::net::SocketAddr,
        path: &str,
        token: &str,
        body: Option<&str>,
    ) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let b = body.unwrap_or("").as_bytes();
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            b.len()
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        if !b.is_empty() {
            stream.write_all(b).await.unwrap();
        }
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (head, body_part) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let code = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0);
        (code, body_part.to_string())
    }

    let (auth_code, _) =
        admin_http_auth("GET", admin_addr, "/v1/policies", "secret-ota-token", None).await;
    assert_eq!(auth_code, 200);

    // Hot-push flat_w5 under a custom persisted name and verify disk persistence + restart reload
    let flat_dir = packages_root().join("flat_w5");
    let raw_yaml = std::fs::read_to_string(flat_dir.join("policy.yaml")).unwrap();
    let custom_yaml = raw_yaml.replace(
        "name: flat_w5_model89999_kp80_policy_zero_20260929",
        "name: flat_ota_persisted_v2",
    );
    let manifest = PolicyManifest::from_yaml_str(&custom_yaml).unwrap();
    let onnx_bytes = std::fs::read(flat_dir.join(&manifest.model.file)).unwrap();
    let golden_json = std::fs::read_to_string(flat_dir.join("golden.json")).unwrap();
    let push_body = serde_json::json!({
        "manifest_yaml": custom_yaml,
        "onnx_base64": B64.encode(&onnx_bytes),
        "golden_json": golden_json,
        "slot": "flat",
        "shadow_frames": 10,
        "activate": true,
    })
    .to_string();

    let (p_code, p_body) = admin_http_auth(
        "POST",
        admin_addr,
        "/v1/policies/push",
        "secret-ota-token",
        Some(&push_body),
    )
    .await;
    assert_eq!(p_code, 200, "authorized push failed: {p_body}");
    assert!(tmp_root
        .join("flat_ota_persisted_v2")
        .join("policy.yaml")
        .is_file());
    assert!(tmp_root
        .join("flat_ota_persisted_v2")
        .join("model.onnx")
        .is_file());
    assert!(tmp_root.join(SLOT_BINDINGS_FILE).is_file());

    // Simulate daemon restart: preload from tmp_root restores package + slot + active profile
    let mut restarted =
        BrainEngine::new(Box::new(BenchNullHal::new(false)), SafetyConfig::default());
    preload_packages(&mut restarted, &tmp_root).unwrap();
    assert_eq!(restarted.policies.active_name(), "flat_ota_persisted_v2");
    assert_eq!(
        restarted.policies.slots().get("flat").map(String::as_str),
        Some("flat_ota_persisted_v2")
    );

    // 2. Verify read-only :13146 LocalStatusGrpcServer
    let ls_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ls_addr = ls_listener.local_addr().unwrap();
    let ls_svc = Arc::new(LocalStatusGrpcServer {
        shared: shared.clone(),
    });
    let ls_task = tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(RobotControlServer::from_arc(ls_svc))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(ls_listener))
            .await;
    });
    let mut ls_client = RobotControlClient::connect(format!("http://{ls_addr}"))
        .await
        .unwrap();
    let status = ls_client.get_control_status(()).await.unwrap().into_inner();
    assert!(!status.motor_output_enabled);
    let enable_err = ls_client.enable(()).await.unwrap_err();
    assert_eq!(enable_err.code(), tonic::Code::PermissionDenied);

    admin_task.abort();
    ls_task.abort();
    let _ = std::fs::remove_dir_all(&tmp_root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rt_loop_200hz_walking_preserves_low_jitter_under_concurrent_health_and_telemetry_load() {
    use doso_runtime::hardware::{HalBackend, HalDiagnostics};
    use doso_runtime::server::{
        build_hot_deploy_router_with_config, generate_boot_id, HotDeployConfig,
    };
    use doso_runtime::types::{MitCmd16, ProprioFrame, RemotePadState, WalkCommand};

    let mut engine = BrainEngine::new(
        Box::new(BenchNullHal::new(true)),
        SafetyConfig {
            allow_motor_enable: true,
            ..SafetyConfig::default()
        },
    );
    preload_packages(&mut engine, &packages_root()).unwrap();
    engine.enable_motors().unwrap();
    engine.stand_up().unwrap();
    let mut sim_t = 10_000u64;
    for _ in 0..=155 {
        sim_t += 20_000;
        engine.tick_sim(sim_t);
    }
    assert_eq!(
        engine.safety.latched_fault(),
        None,
        "fault during stand_up! fsm={:?}",
        engine.fsm_state()
    );
    engine
        .set_walk_velocity(WalkCommand {
            vx: 0.4,
            vy: 0.0,
            yaw: 0.0,
        })
        .unwrap();

    let shared = Arc::new(SharedBrain::new(engine));
    let admin_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let admin_addr = admin_listener.local_addr().unwrap();
    let router = build_hot_deploy_router_with_config(
        shared.clone(),
        HotDeployConfig {
            boot_id: generate_boot_id(),
            version: "v0.0.2-rt".into(),
            release_id: "rel-rt".into(),
            ..HotDeployConfig::default()
        },
    );
    let admin_task = tokio::spawn(async move {
        let _ = axum::serve(admin_listener, router).await;
    });

    shared.reset_rt_metrics();
    let stop = Arc::new(AtomicBool::new(false));
    let rt_stop = stop.clone();
    let rt_shared = shared.clone();
    let rt_join = std::thread::spawn(move || {
        let period = Duration::from_millis(5);
        let mut next_deadline = Instant::now() + period;
        let mut jitter_us = 0u64;
        while !rt_stop.load(Ordering::Relaxed) {
            let tick_start = Instant::now();
            let now_us = rt_shared.now_us();
            let mutex_wait_us = {
                let mut g = rt_shared.engine.lock().unwrap();
                let wait_us = tick_start.elapsed().as_micros() as u64;
                g.tick_200hz(now_us);
                rt_shared.refresh_telemetry(&g, now_us);
                wait_us
            };
            let tick_us = tick_start.elapsed().as_micros() as u64;
            rt_shared.record_rt_sample(mutex_wait_us, tick_us, jitter_us);

            let before_sleep = Instant::now();
            if next_deadline > before_sleep {
                std::thread::sleep(next_deadline - before_sleep);
                let wake = Instant::now();
                jitter_us = wake.saturating_duration_since(next_deadline).as_micros() as u64;
                next_deadline += period;
                if next_deadline < wake {
                    next_deadline = wake + period;
                }
            } else {
                jitter_us = before_sleep
                    .saturating_duration_since(next_deadline)
                    .as_micros() as u64;
                next_deadline = before_sleep + period;
            }
        }
    });

    // Hammer telemetry.read() + GET /health concurrently while 50Hz ONNX inference runs
    let tele_hammer = {
        let s = shared.clone();
        let flag = stop.clone();
        tokio::spawn(async move {
            let mut reads = 0u64;
            while !flag.load(Ordering::Relaxed) {
                let _snap = s.telemetry.read().unwrap().refresh_count;
                reads += 1;
                if reads.is_multiple_of(16) {
                    tokio::task::yield_now().await;
                }
            }
            reads
        })
    };

    let mut health_ok_count = 0u64;
    for _ in 0..80 {
        let (code, body) = admin_http("GET", admin_addr, "/health", None).await;
        if code != 200 {
            let fault = shared.engine.lock().unwrap().safety.latched_fault();
            panic!("GET /health failed: {body}, fault={fault:?}");
        }
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["imu_online"], true);
        assert_eq!(v["online_joints"], 16);
        health_ok_count += 1;
        tokio::time::sleep(Duration::from_millis(8)).await;
    }

    stop.store(true, Ordering::Relaxed);
    rt_join.join().unwrap();
    let tele_reads = tele_hammer.await.unwrap();
    admin_task.abort();

    let ticks = shared.rt_tick_count.load(Ordering::Relaxed);
    let max_mutex_wait_us = shared.rt_mutex_wait_max_us.load(Ordering::Relaxed);
    let max_tick_us = shared.rt_tick_max_us.load(Ordering::Relaxed);
    let max_jitter_us = shared.rt_jitter_max_us.load(Ordering::Relaxed);
    println!(
        "RT_WALKING_STRESS_METRICS: ticks={ticks} health_requests={health_ok_count} tele_reads={tele_reads} max_mutex_wait_us={max_mutex_wait_us} max_tick_us={max_tick_us} max_jitter_us={max_jitter_us}"
    );
    // Windows default 15.6ms sleep quantum yields ~40-60 ticks in ~800ms; Linux hrtimer yields ~160+.
    assert!(ticks >= 25, "expected >=25 RT ticks, got {ticks}");
    assert!(
        max_mutex_wait_us < 1_000,
        "RT mutex contention too high under /health load: {max_mutex_wait_us} us"
    );
    assert!(
        !shared.engine.lock().unwrap().has_critical_motor_fault(),
        "unexpected safety fault during steady walking"
    );

    // Also verify resolve_release_metadata & socketcan liveness health gating
    let tmp_rel = std::env::temp_dir().join(format!("doso-rel-meta-{}", generate_boot_id()));
    let bin_dir = tmp_rel.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(
        tmp_rel.join("release.json"),
        r#"{"version":"v0.0.2","release_id":"rel-20261124"}"#,
    )
    .unwrap();
    let (ver, rel) = doso_runtime::server::resolve_release_metadata(
        Some(&bin_dir.join("doso-brainstem")),
        "v0.0.1",
        None,
    );
    assert_eq!(ver, "v0.0.2");
    assert_eq!(rel, "rel-20261124");
    let _ = std::fs::remove_dir_all(&tmp_rel);

    // Dead SocketCAN mock HAL (imu offline, 0 rx_frames) must fail /health with 503
    struct DeadSocketCanMock;
    impl HalBackend for DeadSocketCanMock {
        fn name(&self) -> &'static str {
            "socketcan-rdk-s100"
        }
        fn sample_proprio(&mut self, _now_us: u64) -> ProprioFrame {
            ProprioFrame::default()
        }
        fn sample_remote(&mut self, _now_us: u64) -> RemotePadState {
            RemotePadState::default()
        }
        fn set_tx_enabled(&mut self, _enabled: bool) {}
        fn tx_enabled(&self) -> bool {
            false
        }
        fn enable_motors(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
        fn disable_motors(&mut self, _clear_errors: bool) -> anyhow::Result<()> {
            Ok(())
        }
        fn send_mit_verified(&mut self, _cmd: &MitCmd16) -> anyhow::Result<()> {
            Ok(())
        }
        fn diagnostics(&self) -> HalDiagnostics {
            HalDiagnostics {
                backend_name: "socketcan-rdk-s100",
                tx_enabled: false,
                tx_frames: 0,
                rx_frames: 0,
                tx_dropped: 0,
            }
        }
    }
    let mut dead_engine = BrainEngine::new(Box::new(DeadSocketCanMock), SafetyConfig::default());
    preload_packages(&mut dead_engine, &packages_root()).unwrap();
    let dead_shared = Arc::new(SharedBrain::new(dead_engine));
    let dead_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_addr = dead_listener.local_addr().unwrap();
    let dead_router = build_hot_deploy_router_with_config(dead_shared, HotDeployConfig::default());
    let dead_task = tokio::spawn(async move {
        let _ = axum::serve(dead_listener, dead_router).await;
    });
    let (dead_code, dead_body) = admin_http("GET", dead_addr, "/health", None).await;
    assert_eq!(
        dead_code, 503,
        "dead socketcan /health should be 503: {dead_body}"
    );
    dead_task.abort();
}
