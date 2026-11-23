use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use brainstem_dds_bridge::{
    spawn_base_gateway_on_listener, BaseGatewayConfig, BaseGatewayService,
};
use doso_brain::BrainEngine;
use doso_brainstem::brainstem_grpc::{BrainstemGrpcServer, LocalStatusGrpcServer, SharedBrain};
use doso_brainstem::hot_deploy::{
    build_hot_deploy_router, build_hot_deploy_router_with_config, HotDeployConfig,
};
use doso_brainstem::preload_packages;
use doso_hal::BenchNullHal;
use doso_safety::SafetyConfig;
use doso_sdk::internal::motion_board::{
    rpc::{
        self,
        robot_control_client::RobotControlClient,
        robot_control_server::RobotControlServer,
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
        .parent()
        .unwrap()
        .join("models")
        .join("packages")
}

async fn admin_http(method: &str, addr: std::net::SocketAddr, path: &str, body: Option<&str>) -> (u16, String) {
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

fn esc_json(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "")
        .replace('\n', "\\n")
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

    let shared = Arc::new(SharedBrain {
        engine: Mutex::new(engine),
        boot_instant: Instant::now(),
    });

    // 1. Start 200Hz RT tick thread
    let stop = Arc::new(AtomicBool::new(false));
    let rt_stop = stop.clone();
    let rt_shared = shared.clone();
    let rt_join = std::thread::spawn(move || {
        while !rt_stop.load(Ordering::Relaxed) {
            {
                let mut g = rt_shared.engine.lock().unwrap();
                g.tick_200hz(rt_shared.now_us());
            }
            std::thread::sleep(Duration::from_millis(5));
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

    // 5. Connect real `RobotClient` and verify Gestures + BodyHeight + Recovery + Profiles!
    let client = RobotClient::connect(format!("http://{gw_addr}"))
        .await
        .expect("RobotClient handshake to single-process Rust Brainstem must succeed");

    let gestures = client.motion().list_gestures().await.unwrap();
    let gesture_names: Vec<_> = gestures.gestures.iter().map(|g| g.name.as_str()).collect();
    for expected in ["bow", "nod", "wiggle", "stretch", "dance"] {
        assert!(
            gesture_names.contains(&expected),
            "missing builtin gesture {expected}"
        );
    }

    let prof = client.motion().profile().await.unwrap();
    let baseline_flat_name = prof.current.clone();
    assert!(
        baseline_flat_name.starts_with("flat_w5"),
        "unexpected baseline flat profile: {baseline_flat_name}"
    );

    // Switch to `height` slot and verify set_body_height(0.42)
    client.motion().switch_profile("height", None).await.unwrap();
    let bh = client.motion().set_body_height(0.42, None).await.unwrap();
    assert!(bh.supported);
    assert!((bh.target_height_m - 0.42).abs() < 1e-6);

    // Switch back to `flat` and verify Recovery session lifecycle
    client.motion().switch_profile("flat", None).await.unwrap();
    let rec0 = client.motion().recovery_status().await.unwrap();
    assert_eq!(rec0.availability, 2, "recovery_v89 must be AVAILABLE when Grounded");
    let rec_entered = client
        .motion()
        .recovery_enter("op-e2e-1", None)
        .await
        .unwrap();
    assert!(rec_entered.selected);
    assert!(!rec_entered.session_token.is_empty());
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

    // 6. Test Hot-Deploy Gate: corrupted Golden trajectory MUST be rejected with HTTP 422!
    let flat_dir = pkgs.join("flat_w5");
    let base_yaml = std::fs::read_to_string(flat_dir.join("policy.yaml")).unwrap();
    let parsed_manifest = doso_policy::PolicyManifest::from_yaml_str(&base_yaml).unwrap();
    let onnx_b64 = B64.encode(std::fs::read(flat_dir.join(&parsed_manifest.model.file)).unwrap());
    let good_golden = std::fs::read_to_string(flat_dir.join("golden.json")).unwrap();
    let bad_golden = good_golden.replacen("\"expected_raw_action\": [", "\"expected_raw_action\": [999.0, ", 1);

    let bad_yaml = base_yaml.replacen(
        &format!("name: {baseline_flat_name}"),
        "name: flat_w5_bad_candidate",
        1,
    );
    let bad_payload = format!(
        "{{\"manifest_yaml\":\"{}\",\"onnx_base64\":\"{}\",\"golden_json\":\"{}\",\"slot\":\"flat\",\"shadow_frames\":20,\"activate\":true}}",
        esc_json(&bad_yaml),
        onnx_b64,
        esc_json(&bad_golden)
    );
    let (bad_code, bad_body) =
        admin_http("POST", admin_addr, "/v1/policies/push", Some(&bad_payload)).await;
    assert_eq!(bad_code, 422, "Golden mismatch must be rejected: {bad_body}");
    assert_eq!(
        client.motion().profile().await.unwrap().current,
        baseline_flat_name,
        "rejected candidate must never alter active profile"
    );

    // 7. Test Valid Zero-Restart Hot-Deploy (`flat_w5_hot_v2` -> `flat` slot + 50-frame Shadow + Activate) in < 2s!
    let hot_yaml = base_yaml.replacen(
        &format!("name: {baseline_flat_name}"),
        "name: flat_w5_hot_v2",
        1,
    );
    let hot_payload = format!(
        "{{\"manifest_yaml\":\"{}\",\"onnx_base64\":\"{}\",\"golden_json\":\"{}\",\"slot\":\"flat\",\"shadow_frames\":50,\"activate\":true}}",
        esc_json(&hot_yaml),
        onnx_b64,
        esc_json(&good_golden)
    );
    let t_push = Instant::now();
    let (ok_code, ok_body) =
        admin_http("POST", admin_addr, "/v1/policies/push", Some(&hot_payload)).await;
    let push_ms = t_push.elapsed().as_secs_f64() * 1000.0;
    println!("E2E_HOT_DEPLOY_OK in {push_ms:.2}ms -> {ok_body}");
    assert_eq!(ok_code, 200, "Valid hot deploy failed: {ok_body}");
    assert!(
        push_ms < 2_000.0,
        "Hot deploy must complete in < 2000ms, took {push_ms:.2}ms"
    );
    assert_eq!(
        client.motion().profile().await.unwrap().current,
        "flat_w5_hot_v2"
    );

    // 8. Test Instant 1-Command Rollback (`POST /v1/policies/rollback`)!
    let t_rb = Instant::now();
    let (rb_code, rb_body) =
        admin_http("POST", admin_addr, "/v1/policies/rollback", Some("{\"slot\":\"flat\"}")).await;
    let rb_ms = t_rb.elapsed().as_secs_f64() * 1000.0;
    println!("E2E_ROLLBACK_OK in {rb_ms:.2}ms -> {rb_body}");
    assert_eq!(rb_code, 200, "Rollback failed: {rb_body}");
    assert_eq!(
        client.motion().profile().await.unwrap().current,
        baseline_flat_name
    );

    // 9. Verify MotionBoardClient + RobotControlClient (`SetSpeedMode`/`GetSpeedMode`, `ListenHistory`, `SetZero` gate, and deterministic `Step`+`Tick`)!
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
        rc_client.get_speed_mode(()).await.unwrap().into_inner().mode,
        rpc::SpeedMode::Normal as i32
    );
    rc_client
        .set_speed_mode(rpc::SpeedModeRequest {
            mode: rpc::SpeedMode::High as i32,
        })
        .await
        .unwrap();
    assert_eq!(
        rc_client.get_speed_mode(()).await.unwrap().into_inner().mode,
        rpc::SpeedMode::High as i32
    );
    rc_client
        .set_speed_mode(rpc::SpeedModeRequest {
            mode: rpc::SpeedMode::Normal as i32,
        })
        .await
        .unwrap();
    assert_eq!(
        rc_client.get_speed_mode(()).await.unwrap().into_inner().mode,
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

    // Stop background 200Hz hardware thread to step simulator deterministically via Step + Tick
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

    let expected_grav = doso_hal::hipnuc::rotate_unit_neg_z_by_quat(qw, 0.0, qy, 0.0);
    let grav = sim_hist
        .projected_gravity
        .expect("History.projected_gravity");
    assert!((grav.x - expected_grav[0]).abs() < 1e-12);
    assert!((grav.y - expected_grav[1]).abs() < 1e-12);
    assert!((grav.z - expected_grav[2]).abs() < 1e-12);

    assert_eq!(sim_hist.joint_position.unwrap().values, sim_pos);
    assert_eq!(sim_hist.joint_velocity.unwrap().values, sim_vel);

    let _ = gw_tx.send(());
}

async fn admin_http_with_auth(
    method: &str,
    addr: std::net::SocketAddr,
    path: &str,
    body: Option<&str>,
    bearer_token: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let b = body.unwrap_or("").as_bytes();
    let auth_hdr = bearer_token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{auth_hdr}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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
async fn hot_deploy_persists_across_restart_health_gate_and_bearer_auth() {
    let tmp_dir = tempfile::tempdir().expect("temp packages dir");
    let src_pkgs = packages_root();

    let mut engine = BrainEngine::new(
        Box::new(BenchNullHal::new(false)),
        SafetyConfig {
            allow_motor_enable: false,
            ..SafetyConfig::default()
        },
    );
    preload_packages(&mut engine, &src_pkgs).expect("preload baseline packages");

    let shared = Arc::new(SharedBrain {
        engine: Mutex::new(engine),
        boot_instant: Instant::now(),
    });

    let cfg = HotDeployConfig {
        packages_dir: Some(tmp_dir.path().to_path_buf()),
        admin_token: Some("secret-ota-token".to_string()),
        boot_id: "boot-test-1234".to_string(),
        version: "v1.0.0-rust".to_string(),
        release_id: "v1.0.0-rust-rel".to_string(),
        health_signing_key_path: None,
    };

    let admin_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let admin_addr = admin_listener.local_addr().unwrap();
    let router = build_hot_deploy_router_with_config(shared.clone(), cfg);
    tokio::spawn(async move {
        let _ = axum::serve(admin_listener, router).await;
    });

    // 1. GET /health is ALWAYS readable without Bearer token and reports full OTA health JSON
    let (h_code, h_body) = admin_http_with_auth("GET", admin_addr, "/health", None, None).await;
    assert_eq!(h_code, 200, "GET /health must succeed: {h_body}");
    let health: serde_json::Value = serde_json::from_str(&h_body).unwrap();
    assert_eq!(health["ok"], true);
    assert_eq!(health["boot_id"], "boot-test-1234");
    assert_eq!(health["version"], "v1.0.0-rust");
    assert_eq!(health["release_id"], "v1.0.0-rust-rel");
    assert_eq!(health["brainstem_ready"], true);
    assert_eq!(health["strategy_ready"], true);
    assert_eq!(health["can_read_only"], true);
    assert!(health["active_profile"]
        .as_str()
        .unwrap_or("")
        .starts_with("flat_w5"));
    assert!(health["uptime_s"].as_u64().is_some());

    // 2. Bearer guard rejects unauthenticated /v1/policies* calls with 401
    let (u_code, _) = admin_http_with_auth("GET", admin_addr, "/v1/policies", None, None).await;
    assert_eq!(u_code, 401);
    let (bad_tok_code, _) =
        admin_http_with_auth("GET", admin_addr, "/v1/policies", None, Some("wrong")).await;
    assert_eq!(bad_tok_code, 401);
    let (ok_list_code, _) = admin_http_with_auth(
        "GET",
        admin_addr,
        "/v1/policies",
        None,
        Some("secret-ota-token"),
    )
    .await;
    assert_eq!(ok_list_code, 200);

    // 3. Hot-push a policy with Bearer auth -> persists self-contained package + slot bindings to disk
    let flat_dir = src_pkgs.join("flat_w5");
    let base_yaml = std::fs::read_to_string(flat_dir.join("policy.yaml")).unwrap();
    let parsed_manifest = doso_policy::PolicyManifest::from_yaml_str(&base_yaml).unwrap();
    let onnx_b64 = B64.encode(std::fs::read(flat_dir.join(&parsed_manifest.model.file)).unwrap());
    let good_golden = std::fs::read_to_string(flat_dir.join("golden.json")).unwrap();
    let hot_yaml = base_yaml.replacen(
        &format!("name: {}", parsed_manifest.name),
        "name: flat_persisted_v2",
        1,
    );
    let push_payload = serde_json::json!({
        "manifest_yaml": hot_yaml,
        "onnx_base64": onnx_b64,
        "golden_json": good_golden,
        "slot": "flat",
        "shadow_frames": 20,
        "activate": true,
    })
    .to_string();

    let (p_code, p_body) = admin_http_with_auth(
        "POST",
        admin_addr,
        "/v1/policies/push",
        Some(&push_payload),
        Some("secret-ota-token"),
    )
    .await;
    assert_eq!(p_code, 200, "hot push failed: {p_body}");

    let persisted_pkg_dir = tmp_dir.path().join("flat_persisted_v2");
    assert!(persisted_pkg_dir.join("policy.yaml").is_file());
    assert!(persisted_pkg_dir.join("model.onnx").is_file());
    assert!(persisted_pkg_dir.join("golden.json").is_file());
    assert!(tmp_dir.path().join(".doso_slots.json").is_file());

    // Pushing same name a second time preserves `.flat_persisted_v2.prev` backup
    let (p2_code, p2_body) = admin_http_with_auth(
        "POST",
        admin_addr,
        "/v1/policies/push",
        Some(&push_payload),
        Some("secret-ota-token"),
    )
    .await;
    assert_eq!(p2_code, 200, "second hot push failed: {p2_body}");
    assert!(
        tmp_dir.path().join(".flat_persisted_v2.prev").join("model.onnx").is_file(),
        "second push must preserve .prev backup directory"
    );

    // 4. Simulate clean service restart: fresh BrainEngine preloading ONLY tmp_dir recovers the persisted policy & slot!
    let mut restarted_engine = BrainEngine::new(
        Box::new(BenchNullHal::new(false)),
        SafetyConfig {
            allow_motor_enable: false,
            ..SafetyConfig::default()
        },
    );
    preload_packages(&mut restarted_engine, tmp_dir.path())
        .expect("restarted engine must load self-contained persisted package");
    assert_eq!(restarted_engine.policies.active_name(), "flat_persisted_v2");
    assert_eq!(
        restarted_engine.policies.slots().get("flat").map(String::as_str),
        Some("flat_persisted_v2")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_status_grpc_13146_serves_control_status_and_denies_mutations() {
    let mut engine = BrainEngine::new(
        Box::new(BenchNullHal::new(false)),
        SafetyConfig {
            allow_motor_enable: false,
            ..SafetyConfig::default()
        },
    );
    preload_packages(&mut engine, &packages_root()).expect("preload baseline packages");

    let shared = Arc::new(SharedBrain {
        engine: Mutex::new(engine),
        boot_instant: Instant::now(),
    });

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let svc = Arc::new(LocalStatusGrpcServer { shared });
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let srv_handle = tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .tcp_nodelay(true)
            .add_service(RobotControlServer::from_arc(svc))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async move {
                    let _ = rx.await;
                },
            )
            .await;
    });

    let mut client = RobotControlClient::connect(format!("http://{addr}"))
        .await
        .expect("connect to read-only local status server");

    // Read-only GetControlStatus succeeds (used by doso-cloud-link)
    let status = client.get_control_status(()).await.unwrap().into_inner();
    assert_eq!(status.fsm.unwrap().kind, rpc::CmsStateKind::Grounded as i32);
    assert!(!status.motor_output_enabled);

    // Every mutating RPC must be rejected with PermissionDenied
    assert_eq!(
        client.enable(()).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(
        client.disable(()).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(
        client.stand_up(()).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(
        client.sit_down(()).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(
        client.damp(()).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(
        client
            .walk(rpc::Vector3 {
                x: 0.3,
                y: 0.0,
                z: 0.0
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(
        client
            .switch_profile(rpc::ProfileRequest {
                name: "height".into()
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(
        client
            .set_body_height(rpc::BodyHeightCommand { height_m: 0.4 })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(
        client.set_zero(()).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );

    let _ = tx.send(());
    let _ = srv_handle.await;
}
