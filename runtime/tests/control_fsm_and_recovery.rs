use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use doso_runtime::arbiter::ControlOwner;
use doso_runtime::control::{BrainEngine, FsmState, RecoveryPhase, RecoveryRuntime, SpeedMode};
use doso_runtime::hardware::{BenchNullHal, HalBackend, HalDiagnostics};
use doso_runtime::params::RobotHardwareParams;
use doso_runtime::policy::{LoadedPolicyPackage, ShadowEvaluator};
use doso_runtime::safety::SafetyConfig;
use doso_runtime::types::{
    ImuState, JointFeedback, MitCmd16, ProprioFrame, RemotePadState, WalkCommand,
};

fn packages_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("models")
        .join("packages")
}

fn load_baseline_packages(engine: &mut BrainEngine) {
    let root = packages_root();
    let flat = LoadedPolicyPackage::load_from_dir(root.join("flat_w5"))
        .expect("flat_w5 package must load");
    engine
        .register_policy(flat, Some("flat"))
        .expect("register flat_w5");
    engine.switch_profile("flat").expect("switch to flat");

    let rec = LoadedPolicyPackage::load_from_dir(root.join("recovery_v89"))
        .expect("recovery_v89 package must load");
    engine
        .register_policy(rec, None)
        .expect("register recovery_v89");
}

struct SharedMockState {
    pad: RemotePadState,
    imu_override: Option<ImuState>,
    last_cmd: MitCmd16,
    motors_enabled: bool,
    disable_confirmed: bool,
    set_zero_calls: usize,
}

impl Default for SharedMockState {
    fn default() -> Self {
        Self {
            pad: RemotePadState::default(),
            imu_override: None,
            last_cmd: MitCmd16::default(),
            motors_enabled: false,
            disable_confirmed: true,
            set_zero_calls: 0,
        }
    }
}

struct ControllableHal {
    inner: BenchNullHal,
    state: Arc<Mutex<SharedMockState>>,
}

impl ControllableHal {
    fn new(tx_enabled: bool, state: Arc<Mutex<SharedMockState>>) -> Self {
        Self {
            inner: BenchNullHal::new(tx_enabled),
            state,
        }
    }
}

impl HalBackend for ControllableHal {
    fn name(&self) -> &'static str {
        "controllable-mock"
    }

    fn sample_proprio(&mut self, now_us: u64) -> ProprioFrame {
        let mut p = self.inner.sample_proprio(now_us);
        if let Some(mut imu) = self.state.lock().unwrap().imu_override {
            imu.timestamp_us = now_us;
            imu.online = true;
            p.imu = imu;
        }
        p
    }

    fn sample_remote(&mut self, _now_us: u64) -> RemotePadState {
        self.state.lock().unwrap().pad
    }

    fn set_tx_enabled(&mut self, enabled: bool) {
        self.inner.set_tx_enabled(enabled);
    }

    fn tx_enabled(&self) -> bool {
        self.inner.tx_enabled()
    }

    fn enable_motors(&mut self) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        st.motors_enabled = true;
        st.disable_confirmed = false;
        drop(st);
        self.inner.enable_motors()
    }

    fn disable_motors(&mut self, clear_errors: bool) -> Result<()> {
        self.state.lock().unwrap().motors_enabled = false;
        self.inner.disable_motors(clear_errors)
    }

    fn send_mit_verified(&mut self, cmd: &MitCmd16) -> Result<()> {
        self.state.lock().unwrap().last_cmd = *cmd;
        self.inner.send_mit_verified(cmd)
    }

    fn calibrate_set_zero(&mut self) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        if !self.inner.tx_enabled() || st.motors_enabled || !st.disable_confirmed {
            anyhow::bail!("mock calibrate_set_zero precondition failed");
        }
        st.set_zero_calls += 1;
        Ok(())
    }

    fn motor_disable_confirmed(&self) -> bool {
        self.state.lock().unwrap().disable_confirmed
    }

    fn diagnostics(&self) -> HalDiagnostics {
        self.inner.diagnostics()
    }
}

#[test]
fn recovery_phase_discriminants_match_proto_contract() {
    assert_eq!(RecoveryPhase::Unknown as i32, 0);
    assert_eq!(RecoveryPhase::Disabled as i32, 1);
    assert_eq!(RecoveryPhase::Preparing as i32, 2);
    assert_eq!(RecoveryPhase::Waiting as i32, 3);
    assert_eq!(RecoveryPhase::Recovering as i32, 4);
    assert_eq!(RecoveryPhase::Balanced as i32, 5);
    assert_eq!(RecoveryPhase::Lowering as i32, 6);
    assert_eq!(RecoveryPhase::ProneHold as i32, 7);
    assert_eq!(RecoveryPhase::StandUp as i32, 8);
    assert_eq!(RecoveryPhase::Standing as i32, 9);
    assert_eq!(RecoveryPhase::SitDown as i32, 10);
    assert_eq!(RecoveryPhase::Grounded as i32, 11);
    assert_eq!(RecoveryPhase::Failed as i32, 12);
}

#[test]
fn recovery_op_id_validation_and_session_guards() {
    let mut rec = RecoveryRuntime::new(Some("mujica_recovery_v89".into()), true);
    assert!(rec.enter("", true, 1_000).is_err());
    assert!(rec.enter(&"a".repeat(81), true, 1_000).is_err());
    assert!(rec.enter("bad op!", true, 1_000).is_err());
    assert!(rec.enter("bad/slash", true, 1_000).is_err());

    let max_valid = "A_z-09".repeat(13) + "ab"; // 80 chars
    assert_eq!(max_valid.len(), 80);
    let tok = rec
        .enter(&max_valid, true, 1_000)
        .expect("80-char valid op_id");
    assert_eq!(rec.phase(), RecoveryPhase::Preparing);

    let tok_same = rec.enter(&max_valid, false, 50_000).unwrap();
    assert_eq!(tok_same, tok);

    assert!(rec.enter("other-op-2", true, 60_000).is_err());

    rec.exit(&tok).unwrap();
    assert_eq!(rec.phase(), RecoveryPhase::Disabled);
    assert!(!rec.selected());
}

#[test]
fn full_11_phase_recovery_progression_with_real_onnx() {
    let mock_state = Arc::new(Mutex::new(SharedMockState::default()));
    let mut engine = BrainEngine::new(
        Box::new(ControllableHal::new(true, mock_state.clone())),
        SafetyConfig {
            allow_motor_enable: true,
            ..SafetyConfig::default()
        },
    );
    load_baseline_packages(&mut engine);

    let mut now_us = 10_000u64;
    assert_eq!(
        engine.recovery_status().phase,
        RecoveryPhase::Disabled as i32
    );

    // 1. Enter -> Preparing (2)
    let tok = engine
        .recovery
        .enter("rec_full_pipeline_01", true, now_us)
        .expect("enter recovery");
    assert_eq!(engine.recovery.phase(), RecoveryPhase::Preparing);
    assert!(!engine.recovery_status().motor_disable_confirmed);

    // 2. Enable -> Waiting (3)
    engine.enable_motors().unwrap();
    let initial_pos = engine.latest_proprio().positions();
    engine
        .recovery
        .enable(&tok, initial_pos, now_us)
        .expect("enable recovery");
    assert_eq!(engine.recovery.phase(), RecoveryPhase::Waiting);

    engine.tick_200hz(now_us);
    assert_eq!(engine.status_snapshot(now_us).fsm_state, FsmState::Recovery);
    {
        let last = mock_state.lock().unwrap().last_cmd;
        assert!((last.0[0].kp - 80.0).abs() < 1e-9);
        assert!((last.0[0].kd - 10.0).abs() < 1e-9);
        assert_eq!(last.0[12].kp, 0.0);
        assert_eq!(last.0[12].dq, 0.0);
    }

    // 3. Start -> Recovering (4)
    {
        let rec_name = engine.recovery.policy_name().unwrap().to_owned();
        let pkg = engine.policies.get_mut(&rec_name).unwrap();
        engine
            .recovery
            .start(&tok, "rec_full_pipeline_01", pkg, now_us)
            .expect("start recovery");
    }
    assert_eq!(engine.recovery.phase(), RecoveryPhase::Recovering);

    let step_50hz = |eng: &mut BrainEngine, t: &mut u64, tok: &str| {
        eng.recovery.heartbeat(tok, *t).unwrap();
        for _ in 0..4 {
            *t += 5_000;
            eng.tick_200hz(*t);
        }
    };

    mock_state.lock().unwrap().imu_override = Some(ImuState {
        gyro_rad_s: [0.6, -0.2, 0.1],
        accel_m_s2: [0.0, 0.0, 9.80665],
        quat_wxyz: [0.866, 0.5, 0.0, 0.0],
        projected_gravity: [0.866, 0.0, -0.5],
        timestamp_us: now_us,
        online: true,
    });
    for _ in 0..8 {
        step_50hz(&mut engine, &mut now_us, &tok);
        assert_eq!(engine.recovery.phase(), RecoveryPhase::Recovering);
    }

    mock_state.lock().unwrap().imu_override = Some(ImuState {
        gyro_rad_s: [0.3, 0.0, 0.0],
        accel_m_s2: [0.0, 0.0, 9.80665],
        quat_wxyz: [1.0, 0.0, 0.0, 0.0],
        projected_gravity: [0.0, 0.0, -0.98],
        timestamp_us: now_us,
        online: true,
    });
    for _ in 0..5 {
        step_50hz(&mut engine, &mut now_us, &tok);
        assert_eq!(engine.recovery.phase(), RecoveryPhase::Recovering);
    }

    // 4. Upright & stable for 25 ticks -> Balanced (5)
    mock_state.lock().unwrap().imu_override = Some(ImuState {
        gyro_rad_s: [0.05, -0.05, 0.05],
        accel_m_s2: [0.0, 0.0, 9.80665],
        quat_wxyz: [1.0, 0.0, 0.0, 0.0],
        projected_gravity: [0.0, 0.0, -0.98],
        timestamp_us: now_us,
        online: true,
    });
    for i in 1..=25 {
        step_50hz(&mut engine, &mut now_us, &tok);
        if i < 25 {
            assert_eq!(engine.recovery.phase(), RecoveryPhase::Recovering);
        } else {
            assert_eq!(engine.recovery.phase(), RecoveryPhase::Balanced);
        }
    }

    // 5. Hold Balanced for 15 ticks -> Lowering (6)
    for i in 1..=15 {
        step_50hz(&mut engine, &mut now_us, &tok);
        if i < 15 {
            assert_eq!(engine.recovery.phase(), RecoveryPhase::Balanced);
        } else {
            assert_eq!(engine.recovery.phase(), RecoveryPhase::Lowering);
        }
    }

    // 6. Lowering for 50 ticks -> ProneHold (7)
    for i in 1..=50 {
        step_50hz(&mut engine, &mut now_us, &tok);
        if i < 50 {
            assert_eq!(engine.recovery.phase(), RecoveryPhase::Lowering);
        } else {
            assert_eq!(engine.recovery.phase(), RecoveryPhase::ProneHold);
        }
    }
    {
        let last = mock_state.lock().unwrap().last_cmd;
        for j in 0..12 {
            assert!(last.0[j].q.abs() < 1e-9);
        }
    }

    // 7. ProneHold for 15 ticks -> StandUp (8)
    for i in 1..=15 {
        step_50hz(&mut engine, &mut now_us, &tok);
        if i < 15 {
            assert_eq!(engine.recovery.phase(), RecoveryPhase::ProneHold);
        } else {
            assert_eq!(engine.recovery.phase(), RecoveryPhase::StandUp);
        }
    }

    // 8. StandUp for 75 ticks -> Standing (9)
    for i in 1..=75 {
        step_50hz(&mut engine, &mut now_us, &tok);
        if i < 75 {
            assert_eq!(engine.recovery.phase(), RecoveryPhase::StandUp);
        } else {
            assert_eq!(engine.recovery.phase(), RecoveryPhase::Standing);
        }
    }

    // 9. Standing holds nominal standing_pose with zero wheel velocity
    step_50hz(&mut engine, &mut now_us, &tok);
    assert_eq!(engine.recovery.phase(), RecoveryPhase::Standing);
    {
        let expected_stand = [
            -0.1, -0.8, 1.8, 0.1, 0.8, -1.8, 0.1, 0.8, -1.8, -0.1, -0.8, 1.8,
        ];
        let last = mock_state.lock().unwrap().last_cmd;
        for (i, &exp) in expected_stand.iter().enumerate() {
            assert!((last.0[i].q - exp).abs() < 1e-9);
        }
        for w in 12..16 {
            assert_eq!(last.0[w].dq, 0.0);
            assert_eq!(last.0[w].kp, 0.0);
        }
    }

    // 10 & 11. SitDown (50 ticks) -> Grounded -> Exit -> Disabled
    engine.recovery.sit_down(&tok, now_us).unwrap();
    assert_eq!(engine.recovery.phase(), RecoveryPhase::SitDown);
    for i in 1..=50 {
        step_50hz(&mut engine, &mut now_us, &tok);
        if i < 50 {
            assert_eq!(engine.recovery.phase(), RecoveryPhase::SitDown);
        } else {
            assert_eq!(engine.recovery.phase(), RecoveryPhase::Grounded);
        }
    }

    engine.disable_motors(false).unwrap();
    engine.recovery.exit(&tok).unwrap();
    assert_eq!(engine.recovery.phase(), RecoveryPhase::Disabled);
    assert!(engine.recovery_status().motor_disable_confirmed);
}

#[test]
fn recovery_timeout_exceeding_500_ticks_fails_and_disables_motors() {
    let mock_state = Arc::new(Mutex::new(SharedMockState {
        imu_override: Some(ImuState {
            gyro_rad_s: [0.5, 0.0, 0.0],
            accel_m_s2: [9.80665, 0.0, 0.0],
            quat_wxyz: [0.707, 0.0, 0.707, 0.0],
            projected_gravity: [1.0, 0.0, 0.0],
            timestamp_us: 0,
            online: true,
        }),
        ..SharedMockState::default()
    }));
    let mut engine = BrainEngine::new(
        Box::new(ControllableHal::new(true, mock_state.clone())),
        SafetyConfig {
            allow_motor_enable: true,
            ..SafetyConfig::default()
        },
    );
    load_baseline_packages(&mut engine);

    let mut now_us = 10_000u64;
    let tok = engine
        .recovery
        .enter("op-timeout-test", true, now_us)
        .unwrap();
    engine.enable_motors().unwrap();
    assert!(engine.safety.motor_output_enabled());

    let pos = engine.latest_proprio().positions();
    engine.recovery.enable(&tok, pos, now_us).unwrap();
    {
        let rec_name = engine.recovery.policy_name().unwrap().to_owned();
        let pkg = engine.policies.get_mut(&rec_name).unwrap();
        engine
            .recovery
            .start(&tok, "op-timeout-test", pkg, now_us)
            .unwrap();
    }

    for _ in 0..500 {
        engine.recovery.heartbeat(&tok, now_us).unwrap();
        engine.tick_200hz(now_us);
        for _ in 0..3 {
            now_us += 5_000;
            engine.tick_200hz(now_us);
        }
        now_us += 5_000;
    }
    assert_eq!(engine.recovery.phase(), RecoveryPhase::Recovering);

    engine.recovery.heartbeat(&tok, now_us).unwrap();
    engine.tick_200hz(now_us);
    assert_eq!(engine.recovery.phase(), RecoveryPhase::Failed);
    assert!(engine.recovery_status().motor_disable_confirmed);
    assert!(!engine.safety.motor_output_enabled());
    assert!(!mock_state.lock().unwrap().motors_enabled);
}

#[test]
fn speed_mode_clamps_walk_velocity_and_reclamps_on_downgrade() {
    let mut engine = BrainEngine::new(
        Box::new(BenchNullHal::new(true)),
        SafetyConfig {
            allow_motor_enable: true,
            ..SafetyConfig::default()
        },
    );
    load_baseline_packages(&mut engine);
    assert_eq!(engine.speed_mode(), SpeedMode::Normal);

    engine.enable_motors().unwrap();
    engine.stand_up().unwrap();
    for step in 0..=155 {
        engine.tick_sim(10_000 + step * 20_000);
    }
    assert_eq!(
        engine.status_snapshot(4_000_000).fsm_state,
        FsmState::Standing
    );

    engine
        .set_walk_velocity(WalkCommand {
            vx: 2.2,
            vy: -1.9,
            yaw: 2.3,
        })
        .unwrap();
    assert_eq!(
        engine.status_snapshot(4_000_000).fsm_state,
        FsmState::Walking
    );
    assert_eq!(
        engine.walk_command(),
        WalkCommand {
            vx: 1.0,
            vy: -1.0,
            yaw: 1.5,
        }
    );

    engine.set_speed_mode(SpeedMode::High);
    assert_eq!(engine.speed_mode(), SpeedMode::High);
    engine
        .set_walk_velocity(WalkCommand {
            vx: 2.2,
            vy: -1.9,
            yaw: -2.3,
        })
        .unwrap();
    assert_eq!(
        engine.walk_command(),
        WalkCommand {
            vx: 2.2,
            vy: -1.9,
            yaw: -2.3,
        }
    );

    engine
        .set_walk_velocity(WalkCommand {
            vx: 4.0,
            vy: -3.2,
            yaw: 3.8,
        })
        .unwrap();
    assert_eq!(
        engine.walk_command(),
        WalkCommand {
            vx: 2.5,
            vy: -2.5,
            yaw: 2.5,
        }
    );

    engine.set_speed_mode(SpeedMode::Normal);
    assert_eq!(
        engine.walk_command(),
        WalkCommand {
            vx: 1.0,
            vy: -1.0,
            yaw: 1.5,
        }
    );
}

#[test]
fn remote_pad_rising_edge_transitions_and_stick_mapping() {
    let mock_state = Arc::new(Mutex::new(SharedMockState::default()));
    let mut engine = BrainEngine::new(
        Box::new(ControllableHal::new(true, mock_state.clone())),
        SafetyConfig {
            allow_motor_enable: true,
            ..SafetyConfig::default()
        },
    );
    load_baseline_packages(&mut engine);
    engine.enable_motors().unwrap();

    let mut now_us = 10_000u64;
    let step_200hz = |eng: &mut BrainEngine, t: &mut u64, n: usize| {
        for _ in 0..n {
            *t += 5_000;
            eng.tick_200hz(*t);
        }
    };

    mock_state.lock().unwrap().pad = RemotePadState {
        active: true,
        ..RemotePadState::default()
    };
    step_200hz(&mut engine, &mut now_us, 1);
    assert_eq!(
        engine.status_snapshot(now_us).source_owner,
        ControlOwner::Yunzhuo
    );
    assert_eq!(engine.status_snapshot(now_us).fsm_state, FsmState::Grounded);

    // 1. Rising L1 when Grounded -> StandUp -> Standing
    mock_state.lock().unwrap().pad.l1 = true;
    step_200hz(&mut engine, &mut now_us, 1);
    assert_eq!(
        engine.status_snapshot(now_us).fsm_state,
        FsmState::Transitioning
    );
    step_200hz(&mut engine, &mut now_us, 155 * 4);
    assert_eq!(engine.status_snapshot(now_us).fsm_state, FsmState::Standing);
    mock_state.lock().unwrap().pad.l1 = false;

    // 2. Remote sticks map (vx: pad.ly, vy: -pad.lx, yaw: -pad.rx) into Walking
    mock_state.lock().unwrap().pad.ly = 0.75;
    mock_state.lock().unwrap().pad.lx = 0.35;
    mock_state.lock().unwrap().pad.rx = -0.5;
    step_200hz(&mut engine, &mut now_us, 4);
    assert_eq!(engine.status_snapshot(now_us).fsm_state, FsmState::Walking);
    assert_eq!(
        engine.walk_command(),
        WalkCommand {
            vx: 0.75,
            vy: -0.35,
            yaw: 0.5,
        }
    );

    mock_state.lock().unwrap().pad.ly = 0.0;
    mock_state.lock().unwrap().pad.lx = 0.0;
    mock_state.lock().unwrap().pad.rx = 0.0;
    step_200hz(&mut engine, &mut now_us, 1);
    assert_eq!(engine.status_snapshot(now_us).fsm_state, FsmState::Standing);

    // 3. Rising L2 when Standing -> SitDown -> Grounded
    mock_state.lock().unwrap().pad.l2 = true;
    step_200hz(&mut engine, &mut now_us, 1);
    assert_eq!(
        engine.status_snapshot(now_us).fsm_state,
        FsmState::Transitioning
    );
    step_200hz(&mut engine, &mut now_us, 155 * 4);
    assert_eq!(engine.status_snapshot(now_us).fsm_state, FsmState::Grounded);
    mock_state.lock().unwrap().pad.l2 = false;
    step_200hz(&mut engine, &mut now_us, 1);

    // 4. Stand up again, then rising R1 -> Damping even while Yunzhuo owns ControlArbiter
    mock_state.lock().unwrap().pad.l1 = true;
    step_200hz(&mut engine, &mut now_us, 1);
    mock_state.lock().unwrap().pad.l1 = false;
    step_200hz(&mut engine, &mut now_us, 155 * 4);
    assert_eq!(engine.status_snapshot(now_us).fsm_state, FsmState::Standing);

    mock_state.lock().unwrap().pad.r1 = true;
    step_200hz(&mut engine, &mut now_us, 1);
    assert_eq!(engine.status_snapshot(now_us).fsm_state, FsmState::Damping);

    step_200hz(&mut engine, &mut now_us, 35 * 4);
    assert_eq!(engine.status_snapshot(now_us).fsm_state, FsmState::Grounded);

    // 5. Dedicated damp_btn also triggers Damping
    mock_state.lock().unwrap().pad.r1 = false;
    mock_state.lock().unwrap().pad.damp_btn = true;
    step_200hz(&mut engine, &mut now_us, 1);
    assert_eq!(engine.status_snapshot(now_us).fsm_state, FsmState::Damping);
}

#[test]
fn sim_inject_tick_history_and_calibrate_set_zero() {
    let mut bench_engine = BrainEngine::new(
        Box::new(BenchNullHal::new(true)),
        SafetyConfig {
            allow_motor_enable: true,
            ..SafetyConfig::default()
        },
    );
    assert!(bench_engine
        .calibrate_set_zero()
        .unwrap_err()
        .to_string()
        .contains("unsupported"));

    let mock_state = Arc::new(Mutex::new(SharedMockState::default()));
    let mut engine = BrainEngine::new(
        Box::new(ControllableHal::new(true, mock_state.clone())),
        SafetyConfig {
            allow_motor_enable: true,
            ..SafetyConfig::default()
        },
    );
    load_baseline_packages(&mut engine);

    mock_state.lock().unwrap().disable_confirmed = false;
    assert!(engine.calibrate_set_zero().is_err());
    assert_eq!(mock_state.lock().unwrap().set_zero_calls, 0);

    mock_state.lock().unwrap().disable_confirmed = true;
    engine
        .calibrate_set_zero()
        .expect("calibrate_set_zero must succeed when Grounded, disabled, and confirmed");
    assert_eq!(mock_state.lock().unwrap().set_zero_calls, 1);
    assert!(engine.latest_history().is_none());
    assert_eq!(engine.history_seq(), 0);

    engine.enable_motors().unwrap();
    assert!(engine.calibrate_set_zero().is_err());
    assert_eq!(mock_state.lock().unwrap().set_zero_calls, 1);

    let stand_pose = RobotHardwareParams::thunder_v4().posture.standing_pose;
    let sim_proprio = ProprioFrame {
        sequence: 42,
        monotonic_us: 100_000,
        imu: ImuState {
            gyro_rad_s: [0.01, -0.02, 0.03],
            accel_m_s2: [0.0, 0.0, 9.80665],
            quat_wxyz: [1.0, 0.0, 0.0, 0.0],
            projected_gravity: [0.0, 0.0, -1.0],
            timestamp_us: 100_000,
            online: false,
        },
        joints: std::array::from_fn(|i| JointFeedback {
            q: stand_pose[i],
            dq: 0.05 * (i as f64),
            ..JointFeedback::default()
        }),
    };
    engine.inject_sim_state(sim_proprio);

    engine.stand_up().unwrap();
    let mut now_us = 100_000u64;
    for _ in 0..=150 {
        now_us += 20_000;
        let h = engine.tick_sim(now_us);
        assert_eq!(h.monotonic_us, now_us);
        assert_eq!(h.gyro_rad_s, [0.01, -0.02, 0.03]);
        assert!(h.observation.is_empty());
    }
    assert_eq!(engine.status_snapshot(now_us).fsm_state, FsmState::Standing);
    assert_eq!(engine.history_seq(), 151);

    assert!(engine.calibrate_set_zero().is_err());

    engine
        .set_walk_velocity(WalkCommand {
            vx: 0.6,
            vy: -0.1,
            yaw: 0.2,
        })
        .unwrap();
    now_us += 20_000;
    let h_walk1 = engine.tick_sim(now_us);
    assert_eq!(engine.history_seq(), 152);
    assert_eq!(h_walk1.walk_cmd.vx, 0.6);
    assert_eq!(h_walk1.joint_pos.0, stand_pose);
    assert_eq!(h_walk1.observation.len(), 53);
    assert!((h_walk1.kp.0[0] - 73.0).abs() < 1e-9);
    assert!((h_walk1.kd.0[0] - 6.93).abs() < 1e-9);

    now_us += 20_000;
    let h_walk2 = engine.tick_sim(now_us);
    assert_eq!(engine.history_seq(), 153);
    assert_eq!(h_walk2.action, h_walk1.next_action);
    assert_eq!(
        engine.latest_history().unwrap().monotonic_us,
        h_walk2.monotonic_us
    );
}

#[test]
fn snapshot_for_shadow_preserves_chronological_frames_and_matches_off_lock_eval() {
    let mut engine = BrainEngine::new(
        Box::new(BenchNullHal::new(false)),
        SafetyConfig {
            allow_motor_enable: false,
            ..SafetyConfig::default()
        },
    );
    load_baseline_packages(&mut engine);

    let (empty_frames, empty_cmd, empty_bh) = engine.snapshot_for_shadow(4);
    assert_eq!(empty_frames.len(), 10);
    assert!(empty_cmd.is_zero());
    assert!((empty_bh - 0.375).abs() < 1e-9);

    for step in 1..=60u64 {
        engine.tick_200hz(step * 5_000);
    }
    let (frames, walk_cmd, bh_m) = engine.snapshot_for_shadow(50);
    assert_eq!(frames.len(), 50);
    assert_eq!(frames.first().unwrap().monotonic_us, 11 * 5_000);
    assert_eq!(frames.last().unwrap().monotonic_us, 60 * 5_000);

    let mut candidate_a =
        LoadedPolicyPackage::load_from_dir(packages_root().join("flat_w5")).unwrap();
    let mut candidate_b =
        LoadedPolicyPackage::load_from_dir(packages_root().join("flat_w5")).unwrap();

    let off_lock =
        ShadowEvaluator::evaluate_frames(&mut candidate_a, &frames, walk_cmd, bh_m).unwrap();
    let in_lock = engine
        .shadow_evaluate_candidate(&mut candidate_b, 50)
        .unwrap();
    assert_eq!(off_lock.steps, 50);
    assert_eq!(off_lock.steps, in_lock.steps);
    assert_eq!(off_lock.total_clipped_joints, in_lock.total_clipped_joints);
    assert!((off_lock.max_leg_step_delta_rad - in_lock.max_leg_step_delta_rad).abs() < 1e-12);
}

#[test]
fn standing_gesture_and_external_enforce_joint_bounds_and_external_timeout_soft_damps() {
    let mock_state = Arc::new(Mutex::new(SharedMockState::default()));
    let mut engine = BrainEngine::new(
        Box::new(ControllableHal::new(true, mock_state.clone())),
        SafetyConfig {
            allow_motor_enable: true,
            ..SafetyConfig::default()
        },
    );
    load_baseline_packages(&mut engine);

    // 1. Out-of-envelope ExternalControl frame must be rejected cleanly
    let mut now_us = 10_000u64;
    engine.external_enter("ext-safe-1", now_us).unwrap();
    engine.external_enable_motors().unwrap();
    let mut oob_cmd = MitCmd16::default();
    oob_cmd.0[0].q = 2.5; // > hip limit 0.39 rad (would pass raw 12.57 motor limit)
    oob_cmd.0[0].kp = 80.0;
    oob_cmd.0[0].kd = 6.0;
    assert!(engine.external_submit_frame(1, oob_cmd, now_us).is_err());

    // 2. Valid standing pose passes ExternalControl, and 500ms timeout enters soft Damping
    //    before disabling motors (never drops straight to Grounded while motors enabled)
    let stand = RobotHardwareParams::thunder_v4().posture.standing_pose;
    let valid_cmd = MitCmd16(std::array::from_fn(|i| {
        if i < 12 {
            doso_runtime::types::MitTarget::leg_pd(stand[i], 60.0, 6.0)
        } else {
            doso_runtime::types::MitTarget::wheel_vel(0.0, 1.0)
        }
    }));
    engine
        .external_submit_frame(1, valid_cmd, now_us)
        .expect("valid standing-envelope frame must pass");
    engine.tick_200hz(now_us);
    assert_eq!(engine.status_snapshot(now_us).fsm_state, FsmState::External);

    // Timeout (> 500_000 us) enters Damping with motors still enabled during soft ramp-down
    now_us += 600_000;
    engine.tick_200hz(now_us);
    let snap_damp = engine.status_snapshot(now_us);
    assert_eq!(snap_damp.fsm_state, FsmState::Damping);
    assert!(snap_damp.motor_output_enabled);

    // Finish damping settle (29+ 50Hz ticks): settles into Grounded and disables motors
    for _ in 0..35 {
        for _ in 0..4 {
            now_us += 5_000;
            engine.tick_200hz(now_us);
        }
    }
    let snap_done = engine.status_snapshot(now_us);
    assert_eq!(snap_done.fsm_state, FsmState::Grounded);
    assert!(!snap_done.motor_output_enabled);

    // 3. Standing / Gesture also reject injected NaN or out-of-envelope commands via SafetyGate
    engine.enable_motors().unwrap();
    engine.stand_up().unwrap();
    for _ in 0..160 {
        for _ in 0..4 {
            now_us += 5_000;
            engine.tick_200hz(now_us);
        }
    }
    assert_eq!(engine.status_snapshot(now_us).fsm_state, FsmState::Standing);
    assert!(engine.safety.motor_output_enabled());
}

#[test]
fn joystick_idle_does_not_preempt_and_release_immediately_stops_walking() {
    let mut mapper = doso_runtime::arbiter::JoystickPadMapper::new();
    mapper.mark_connected(1_000);
    assert!(
        !mapper.sample(2_000).active,
        "idle connected gamepad must not claim active ownership"
    );

    let mock_state = Arc::new(Mutex::new(SharedMockState::default()));
    let mut engine = BrainEngine::new(
        Box::new(ControllableHal::new(true, mock_state.clone())),
        SafetyConfig {
            allow_motor_enable: true,
            ..SafetyConfig::default()
        },
    );
    load_baseline_packages(&mut engine);
    engine.enable_motors().unwrap();

    // Even with a connected idle pad (active=false), SDK damp() succeeds
    let mut now_us = 10_000u64;
    engine.tick_200hz(now_us);
    assert_eq!(
        engine.status_snapshot(now_us).source_owner,
        ControlOwner::None
    );

    // Drive forward via pad, then drop pad.active=false -> immediate Walking -> Standing stop
    mock_state.lock().unwrap().pad = RemotePadState {
        active: true,
        l1: true,
        ..RemotePadState::default()
    };
    now_us += 5_000;
    engine.tick_200hz(now_us);
    mock_state.lock().unwrap().pad.l1 = false;
    for _ in 0..(155 * 4) {
        now_us += 5_000;
        engine.tick_200hz(now_us);
    }
    mock_state.lock().unwrap().pad.ly = 0.6;
    for _ in 0..4 {
        now_us += 5_000;
        engine.tick_200hz(now_us);
    }
    assert_eq!(engine.status_snapshot(now_us).fsm_state, FsmState::Walking);

    mock_state.lock().unwrap().pad = RemotePadState::default();
    now_us += 5_000;
    engine.tick_200hz(now_us);
    assert_eq!(
        engine.status_snapshot(now_us).fsm_state,
        FsmState::Standing,
        "releasing remote active flag must zero walk velocity immediately"
    );
    assert!(engine.walk_command().is_zero());

    // Simulate lease expiry then SDK damp() must succeed without Yunzhuo lockup
    now_us += 500_000;
    engine.tick_200hz(now_us);
    engine
        .damp()
        .expect("SDK damp must succeed once remote is idle");
    assert_eq!(engine.status_snapshot(now_us).fsm_state, FsmState::Damping);
}

#[test]
fn recovery_enable_enables_motors_and_rejects_cleanly_when_disallowed() {
    let mut denied_engine = BrainEngine::new(
        Box::new(BenchNullHal::new(false)),
        SafetyConfig {
            allow_motor_enable: false,
            ..SafetyConfig::default()
        },
    );
    load_baseline_packages(&mut denied_engine);
    let tok_denied = denied_engine
        .recovery
        .enter("op-deny", true, 1_000)
        .unwrap();
    assert!(denied_engine.recovery_enable(&tok_denied, 2_000).is_err());
    assert_eq!(denied_engine.recovery.phase(), RecoveryPhase::Preparing);
    assert!(!denied_engine.safety.motor_output_enabled());

    let mut ok_engine = BrainEngine::new(
        Box::new(BenchNullHal::new(true)),
        SafetyConfig {
            allow_motor_enable: true,
            ..SafetyConfig::default()
        },
    );
    load_baseline_packages(&mut ok_engine);
    let tok_ok = ok_engine.recovery.enter("op-ok", true, 1_000).unwrap();
    ok_engine.recovery_enable(&tok_ok, 2_000).unwrap();
    assert_eq!(ok_engine.recovery.phase(), RecoveryPhase::Waiting);
    assert!(ok_engine.safety.motor_output_enabled());
}

#[test]
fn fsm_guards_require_enabled_online_proprio_and_clear_fault_resets_grounded() {
    struct OfflineSocketCanHal(BenchNullHal);
    impl HalBackend for OfflineSocketCanHal {
        fn name(&self) -> &'static str {
            "socketcan-rdk-s100"
        }
        fn sample_proprio(&mut self, now_us: u64) -> ProprioFrame {
            let mut p = self.0.sample_proprio(now_us);
            p.imu.online = false;
            p
        }
        fn sample_remote(&mut self, now_us: u64) -> RemotePadState {
            self.0.sample_remote(now_us)
        }
        fn set_tx_enabled(&mut self, enabled: bool) {
            self.0.set_tx_enabled(enabled);
        }
        fn tx_enabled(&self) -> bool {
            self.0.tx_enabled()
        }
        fn enable_motors(&mut self) -> Result<()> {
            self.0.enable_motors()
        }
        fn disable_motors(&mut self, clear_errors: bool) -> Result<()> {
            self.0.disable_motors(clear_errors)
        }
        fn send_mit_verified(&mut self, cmd: &MitCmd16) -> Result<()> {
            self.0.send_mit_verified(cmd)
        }
        fn diagnostics(&self) -> HalDiagnostics {
            self.0.diagnostics()
        }
    }

    let mut engine = BrainEngine::new(
        Box::new(OfflineSocketCanHal(BenchNullHal::new(true))),
        SafetyConfig {
            allow_motor_enable: true,
            ..SafetyConfig::default()
        },
    );
    load_baseline_packages(&mut engine);

    // 1. stand_up rejected when motors disabled
    assert!(engine.stand_up().is_err());

    // 2. stand_up rejected when motors enabled but IMU/proprio is offline on socketcan
    engine.enable_motors().unwrap();
    engine.tick_200hz(10_000);
    assert!(engine.stand_up().is_err());

    // 3. external_enter moves to External where Grounded enable_motors fails, clear_motor_fault resets Grounded
    engine.disable_motors(false).unwrap();
    engine.external_enter("ext-guard-1", 20_000).unwrap();
    assert!(engine.enable_motors().is_err());
    engine.external_enable_motors().unwrap();
    assert!(engine.safety.motor_output_enabled());
    engine.clear_motor_fault().unwrap();
    assert_eq!(engine.fsm_state(), FsmState::Grounded);
    assert!(!engine.safety.motor_output_enabled());
}
