use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};
use doso_runtime::arbiter::{
    ControlArbiter, ControlOwner, GSState, JoystickPadMapper, LinuxJsEvent, YunZhuoControlMode,
    YunZhuoModeStateMachine, YunZhuoState, JS_EVENT_AXIS, JS_EVENT_BUTTON, JS_EVENT_INIT,
};
use doso_runtime::control::BrainEngine;
use doso_runtime::hardware::yunzhuo::{SBUS_CENTER, SBUS_HIGH, SBUS_LOW};
use doso_runtime::hardware::{BenchNullHal, HalBackend, HalDiagnostics};
use doso_runtime::params::{NUM_JOINTS, WHEEL_START_INDEX};
use doso_runtime::safety::{
    deploy_action_max, deploy_action_min, emergency_disable_motors, install_panic_motor_kill_hook,
    run_best_effort_cleanup, SafetyConfig, SafetyGate, SafetyTripReason,
};
use doso_runtime::server::SharedBrain;
use doso_runtime::types::{ImuState, JointFeedback, MitCmd16, ProprioFrame, RemotePadState};

#[allow(clippy::too_many_arguments)]
fn make_yz_state(
    gs: GSState,
    lx: f64,
    ly: f64,
    rx: f64,
    ry: f64,
    h: bool,
    lt: bool,
    rt: bool,
    knob: f64,
) -> YunZhuoState {
    let mut raw_channels = [SBUS_LOW; 16];
    raw_channels[0] = (SBUS_CENTER as f64 + rx * 720.0).round() as u16;
    raw_channels[1] = (SBUS_CENTER as f64 - ry * 720.0).round() as u16;
    raw_channels[2] = (SBUS_CENTER as f64 + ly * 720.0).round() as u16;
    raw_channels[3] = (SBUS_CENTER as f64 + lx * 720.0).round() as u16;
    raw_channels[4] = gs.to_sbus_raw();
    if h {
        raw_channels[5] = SBUS_HIGH;
    }
    if lt {
        raw_channels[7] = SBUS_HIGH;
    }
    if rt {
        raw_channels[11] = SBUS_HIGH;
    }
    raw_channels[13] = (SBUS_CENTER as f64 + knob * 720.0).round() as u16;
    YunZhuoState::from_channels(&raw_channels, 0x00)
}

#[test]
fn yunzhuo_mode_machine_enforces_neutral_gate_and_rejects_quick_sweep() {
    let mut sm = YunZhuoModeStateMachine::new();

    // 1. Cold boot with G/S already in Up stays locked in Neutral
    for step in 0..=10 {
        let snap = sm.update(
            &make_yz_state(GSState::Up, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
            step * 50_000,
        );
        assert_eq!(snap.mode, YunZhuoControlMode::Neutral);
        assert_eq!(snap.dog_drive_scale, 0.0);
    }

    // 2. Centered sticks in Middle for <300ms does NOT open gate; >=300ms opens neutral_gate_ready
    let base = 550_000;
    for offset in [0, 100_000, 200_000, 290_000] {
        let snap = sm.update(
            &make_yz_state(GSState::Middle, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
            base + offset,
        );
        assert!(!snap.neutral_gate_ready);
    }
    let ready = sm.update(
        &make_yz_state(GSState::Middle, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
        base + 300_000,
    );
    assert!(ready.neutral_gate_ready);

    // 3. Displaced stick (>= 0.10) in Middle immediately revokes neutral_gate_ready
    let revoked = sm.update(
        &make_yz_state(
            GSState::Middle,
            0.15,
            0.0,
            0.0,
            0.0,
            true,
            false,
            false,
            0.0,
        ),
        base + 320_000,
    );
    assert!(!revoked.neutral_gate_ready);

    // Re-acquire 300ms centered Middle gate, then debounce 100ms into Dog
    let g0 = 900_000;
    for step in 0..=6 {
        sm.update(
            &make_yz_state(GSState::Middle, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
            g0 + step * 50_000,
        );
    }
    assert!(sm.neutral_gate_ready());

    let u0 = g0 + 320_000;
    let pre_debounce = sm.update(
        &make_yz_state(GSState::Up, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
        u0,
    );
    assert_eq!(pre_debounce.mode, YunZhuoControlMode::Neutral);
    let in_dog = sm.update(
        &make_yz_state(GSState::Up, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
        u0 + 100_000,
    );
    assert_eq!(in_dog.mode, YunZhuoControlMode::Dog);
    assert!(in_dog.mode_changed);

    // 4. Quick sweep Dog -> Middle (only 40ms) -> Down (100ms) is rejected to Neutral, never Arm
    let s0 = u0 + 350_000;
    sm.update(
        &make_yz_state(GSState::Up, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
        s0 - 100_000,
    );
    sm.update(
        &make_yz_state(GSState::Middle, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
        s0,
    );
    sm.update(
        &make_yz_state(GSState::Down, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
        s0 + 40_000,
    );
    let swept = sm.update(
        &make_yz_state(GSState::Down, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
        s0 + 140_000,
    );
    assert_eq!(swept.mode, YunZhuoControlMode::Neutral);
    assert!(!swept.neutral_gate_ready);
}

#[test]
fn yunzhuo_arm_instant_exit_ramps_speed_scale_buttons_and_watchdog() {
    let mut sm = YunZhuoModeStateMachine::new();

    // Enter Arm after 300ms centered Middle + 100ms Down
    for step in 0..=6 {
        sm.update(
            &make_yz_state(GSState::Middle, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
            step * 50_000,
        );
    }
    sm.update(
        &make_yz_state(GSState::Down, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
        320_000,
    );
    let arm = sm.update(
        &make_yz_state(GSState::Down, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
        420_000,
    );
    assert_eq!(arm.mode, YunZhuoControlMode::Arm);

    // Leaving Arm drops to Neutral IMMEDIATELY in 1ms without waiting for 100ms debounce
    let instant_drop = sm.update(
        &make_yz_state(GSState::Middle, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
        421_000,
    );
    assert_eq!(instant_drop.mode, YunZhuoControlMode::Neutral);
    assert!(instant_drop.mode_changed);

    // Re-center 300ms in Middle, then enter Dog and verify 200ms linear ramp-up + speed scales
    let m0 = 450_000;
    for step in 0..=6 {
        sm.update(
            &make_yz_state(GSState::Middle, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
            m0 + step * 50_000,
        );
    }
    let d0 = m0 + 320_000;
    sm.update(
        &make_yz_state(GSState::Up, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
        d0,
    );
    let enter_dog = sm.update(
        &make_yz_state(GSState::Up, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
        d0 + 100_000,
    );
    assert_eq!(enter_dog.mode, YunZhuoControlMode::Dog);
    assert!((enter_dog.dog_drive_scale - 0.0).abs() < 1e-9);
    assert!(enter_dog.direction_armed);

    // Halfway through 200ms ramp-up (+100ms): dog_drive_scale == 0.5
    let half_up = sm.update(
        &make_yz_state(GSState::Up, 0.0, 0.8, 0.0, 0.0, true, false, false, 0.0),
        d0 + 200_000,
    );
    assert!((half_up.dog_drive_scale - 0.5).abs() < 1e-9);
    assert!((half_up.pad.ly - 0.4).abs() < 1e-6);

    // End of 200ms ramp-up (+200ms): dog_drive_scale == 1.0; verify LT=0.5x and RT=1.5x
    let full_lt = sm.update(
        &make_yz_state(GSState::Up, 0.0, 0.8, 0.0, 0.0, true, true, false, 0.0),
        d0 + 300_000,
    );
    assert!((full_lt.dog_drive_scale - 1.0).abs() < 1e-9);
    assert!((full_lt.speed_scale - 0.5).abs() < 1e-9);
    assert!((full_lt.pad.ly - 0.4).abs() < 1e-6);

    let full_rt = sm.update(
        &make_yz_state(GSState::Up, 0.0, 0.8, 0.0, 0.0, true, false, true, 0.0),
        d0 + 350_000,
    );
    assert!((full_rt.speed_scale - 1.5).abs() < 1e-9);
    assert!((full_rt.pad.ly - 1.2).abs() < 1e-6);

    // Verify 400ms linear ramp-down on Dog -> Neutral
    let exit_req = d0 + 400_000;
    sm.update(
        &make_yz_state(GSState::Middle, 0.0, 0.8, 0.0, 0.0, true, false, false, 0.0),
        exit_req,
    );
    let ramp_down_start = sm.update(
        &make_yz_state(GSState::Middle, 0.0, 0.8, 0.0, 0.0, true, false, false, 0.0),
        exit_req + 100_000,
    );
    assert_eq!(ramp_down_start.mode, YunZhuoControlMode::Neutral);
    assert!((ramp_down_start.dog_drive_scale - 1.0).abs() < 1e-9);

    let ramp_down_mid1 = sm.update(
        &make_yz_state(GSState::Middle, 0.0, 0.8, 0.0, 0.0, true, false, false, 0.0),
        exit_req + 200_000,
    );
    assert!((ramp_down_mid1.dog_drive_scale - 0.75).abs() < 1e-9);

    let ramp_down_half = sm.update(
        &make_yz_state(GSState::Middle, 0.0, 0.8, 0.0, 0.0, true, false, false, 0.0),
        exit_req + 300_000,
    );
    assert!((ramp_down_half.dog_drive_scale - 0.5).abs() < 1e-9);

    sm.update(
        &make_yz_state(GSState::Middle, 0.0, 0.8, 0.0, 0.0, true, false, false, 0.0),
        exit_req + 400_000,
    );
    let ramp_down_end = sm.update(
        &make_yz_state(GSState::Middle, 0.0, 0.8, 0.0, 0.0, true, false, false, 0.0),
        exit_req + 500_000,
    );
    assert!((ramp_down_end.dog_drive_scale - 0.0).abs() < 1e-9);

    // Deadzone arming (<0.02) and latched switch edge detection (L1/L2/R1) when H toggles on
    let t_btn = exit_req + 550_000;
    let mut yz = make_yz_state(
        GSState::Middle,
        0.0,
        0.5,
        0.0,
        0.0,
        false,
        false,
        false,
        0.0,
    );
    yz.l1 = true;
    let h_off = sm.update(&yz, t_btn);
    assert!(!h_off.standup_triggered);
    assert!(!h_off.direction_armed);

    yz.h = true;
    let h_just_on = sm.update(&yz, t_btn + 20_000);
    assert!(!h_just_on.standup_triggered);
    assert!(!h_just_on.direction_armed);

    yz.l1 = false;
    yz.l2 = true;
    yz.r1 = true;
    let edges = sm.update(&yz, t_btn + 40_000);
    assert!(edges.standup_triggered);
    assert!(edges.sitdown_triggered);
    assert!(edges.damp_triggered);

    // Verify hold suppresses re-trigger and >150ms watchdog resets Dog -> Neutral
    sm.reset_for_input_loss();
    let mut s = make_yz_state(GSState::Middle, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0);
    s.l1 = true;
    let first = sm.update(&s, 10_000);
    assert!(!first.standup_triggered && !first.sitdown_triggered && !first.damp_triggered);

    s.l1 = false;
    s.l2 = true;
    s.r1 = true;
    let flipped = sm.update(&s, 30_000);
    assert!(flipped.standup_triggered && flipped.pad.l1);
    assert!(flipped.sitdown_triggered && flipped.pad.l2);
    assert!(flipped.damp_triggered && flipped.pad.r1);

    let held = sm.update(&s, 50_000);
    assert!(!held.standup_triggered && !held.pad.l1);
    assert!(!held.sitdown_triggered && !held.pad.l2);
    assert!(!held.damp_triggered && !held.pad.r1);

    for step in 0..=6 {
        sm.update(&s, 100_000 + step * 50_000);
    }
    s.g_s = GSState::Up;
    sm.update(&s, 420_000);
    let in_dog = sm.update(&s, 520_000);
    assert_eq!(in_dog.mode, YunZhuoControlMode::Dog);

    assert!(sm.check_watchdog(730_000));
    assert_eq!(sm.mode(), YunZhuoControlMode::Neutral);
    assert!(!sm.neutral_gate_ready());
}

#[test]
fn linux_js_event_parsing_and_xbox_pad_mapper() {
    let mut mapper = JoystickPadMapper::new();
    mapper.mark_connected(1_000);

    // Init button event must be ignored
    let mut raw_init_btn = [0u8; 8];
    raw_init_btn[4..6].copy_from_slice(&1i16.to_le_bytes());
    raw_init_btn[6] = JS_EVENT_BUTTON | JS_EVENT_INIT;
    raw_init_btn[7] = 0;
    mapper.apply_event(LinuxJsEvent::from_le_bytes(&raw_init_btn), 1_000);
    assert!(!mapper.sample(1_000).l1);

    // Real button A press (btn 0 -> l1) and left stick Y forward (-32767 -> +1.0 ly)
    let mut raw_btn_a = [0u8; 8];
    raw_btn_a[4..6].copy_from_slice(&1i16.to_le_bytes());
    raw_btn_a[6] = JS_EVENT_BUTTON;
    raw_btn_a[7] = 0;
    mapper.apply_event(LinuxJsEvent::from_le_bytes(&raw_btn_a), 2_000);

    let mut raw_axis_ly = [0u8; 8];
    raw_axis_ly[4..6].copy_from_slice(&(-32767i16).to_le_bytes());
    raw_axis_ly[6] = JS_EVENT_AXIS;
    raw_axis_ly[7] = 1;
    mapper.apply_event(LinuxJsEvent::from_le_bytes(&raw_axis_ly), 2_000);

    let pad = mapper.sample(2_000);
    assert!(pad.active && pad.l1);
    assert!((pad.ly - 1.0).abs() < 1e-4);
}

#[test]
fn control_arbiter_yunzhuo_preemption_grpc_lease_and_monotonic_sequence() {
    let mut arb = ControlArbiter::new(350);
    assert_eq!(arb.owner(), ControlOwner::None);

    // gRPC principal A acquires lease
    arb.acquire_grpc("client-a", 10_000).unwrap();
    assert_eq!(arb.owner(), ControlOwner::Grpc);
    assert!(arb.grpc_lease_active(100_000));
    assert_eq!(arb.lease_remaining_ms(110_000), 250);

    // Principal B rejected while A holds lease
    assert_eq!(
        arb.acquire_grpc("client-b", 120_000),
        Err("grpc_lease_held_by_another_principal")
    );

    // Sequence numbers must increment strictly by +1 starting at 1
    assert_eq!(
        arb.renew_grpc_seq("client-a", 0, 130_000),
        Err("sequence_violation")
    );
    assert_eq!(
        arb.renew_grpc_seq("client-a", 2, 130_000),
        Err("sequence_violation")
    );
    arb.renew_grpc_seq("client-a", 1, 130_000).unwrap();
    arb.renew_grpc_seq("client-a", 2, 140_000).unwrap();
    assert_eq!(arb.last_sequence(), 2);

    // Physical remote unconditionally preempts gRPC
    arb.preempt_yunzhuo(150_000);
    assert_eq!(arb.owner(), ControlOwner::Yunzhuo);
    assert!(!arb.grpc_lease_active(150_000));
    assert_eq!(
        arb.acquire_grpc("client-a", 160_000),
        Err("preempted_by_yunzhuo")
    );

    // Lease expiration clears YunZhuo and allows fresh gRPC acquisition
    assert!(arb.expire_if_needed(500_000));
    assert_eq!(arb.owner(), ControlOwner::None);
    arb.acquire_grpc("client-b", 510_000).unwrap();
    assert_eq!(arb.owner_id(), "client-b");
    arb.release_grpc("client-b");
    assert_eq!(arb.owner(), ControlOwner::None);
}

#[test]
fn nan_or_wheel_kp_immediately_trips_and_disables_motors() {
    let mut hal = BenchNullHal::new(true);
    let mut gate = SafetyGate::new(SafetyConfig {
        allow_motor_enable: true,
        ..SafetyConfig::default()
    });
    gate.enable_motors(&mut hal).unwrap();
    let proprio = hal.sample_proprio(1_000);

    let mut bad_wheel = MitCmd16::default();
    bad_wheel.0[12].kp = 10.0;
    let err = gate
        .filter_and_dispatch(&mut hal, &proprio, 1_000, false, None, &bad_wheel)
        .unwrap_err();
    assert_eq!(err, SafetyTripReason::WheelNonZeroStiffness);
    assert!(!gate.motor_output_enabled());

    // Clear fault, re-enable, and verify NaN command also trips immediately
    gate.clear_fault(&mut hal).unwrap();
    gate.enable_motors(&mut hal).unwrap();
    let mut nan_cmd = MitCmd16::default();
    nan_cmd.0[0].q = f64::NAN;
    let err2 = gate
        .filter_and_dispatch(&mut hal, &proprio, 1_000, false, None, &nan_cmd)
        .unwrap_err();
    assert_eq!(err2, SafetyTripReason::NonFiniteCommand);
    assert!(!gate.motor_output_enabled());
}

#[test]
fn safety_gate_enforces_hardware_limits_profile_bounds_tilt_overtemp_and_staleness() {
    let mut hal = BenchNullHal::new(true);
    let mut gate = SafetyGate::new(SafetyConfig {
        allow_motor_enable: true,
        ..SafetyConfig::default()
    });
    gate.enable_motors(&mut hal).unwrap();
    let ok_proprio = hal.sample_proprio(10_000);

    // 1. Hardware limit exceeded (RS02 wheel kd_max = 5.0)
    let mut bad_hw = MitCmd16::default();
    bad_hw.0[12].kd = 6.0;
    assert_eq!(
        gate.validate_frame(&bad_hw, None),
        Err(SafetyTripReason::HardwareLimitExceeded)
    );

    // 2. Profile bound exceeded on active leg joint (kp > 0)
    let mut bad_bound = MitCmd16::default();
    bad_bound.0[0].q = 0.9; // > deploy_action_max()[0] = 0.39
    bad_bound.0[0].kp = 80.0;
    assert_eq!(
        gate.validate_frame(&bad_bound, Some((deploy_action_min(), deploy_action_max()))),
        Err(SafetyTripReason::ProfileBoundExceeded)
    );

    // 3. Tilt exceeded while walking (enforce_tilt = true)
    let mut tilted = ok_proprio;
    tilted.imu.projected_gravity = [0.95, 0.0, -0.20]; // -z = 0.20 < 0.35
    assert_eq!(
        gate.filter_and_dispatch(&mut hal, &tilted, 10_000, true, None, &MitCmd16::default()),
        Err(SafetyTripReason::TiltExceeded)
    );
    assert!(!gate.motor_output_enabled());

    // 4. Overtemp trips MotorFaultOrOvertemp
    gate.clear_fault(&mut hal).unwrap();
    gate.enable_motors(&mut hal).unwrap();
    let mut hot = ok_proprio;
    hot.joints[3].temp_c = 90.0;
    assert_eq!(
        gate.filter_and_dispatch(&mut hal, &hot, 10_000, false, None, &MitCmd16::default()),
        Err(SafetyTripReason::MotorFaultOrOvertemp)
    );

    // 5. Stale proprioception (> 200ms) trips StaleProprioception
    gate.clear_fault(&mut hal).unwrap();
    gate.enable_motors(&mut hal).unwrap();
    assert_eq!(
        gate.filter_and_dispatch(
            &mut hal,
            &ok_proprio,
            250_000,
            false,
            None,
            &MitCmd16::default()
        ),
        Err(SafetyTripReason::StaleProprioception)
    );
}

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

    let shared = Arc::new(SharedBrain::new(engine));
    (shared, events)
}

#[test]
fn emergency_disable_dispatches_passive_damping_then_disables_motors() {
    let (shared, events) = make_armed_shared_brain();

    assert!(emergency_disable_motors(&shared));

    let guard = shared.engine.lock().unwrap();
    assert!(!guard.safety.motor_output_enabled());

    let recorded = events.lock().unwrap().clone();
    assert_eq!(
        recorded.len(),
        2,
        "expected passive damping frame + disable_motors(false)"
    );

    match &recorded[0] {
        HalEvent::SentMit(cmd) => {
            for i in 0..NUM_JOINTS {
                assert_eq!(
                    cmd.0[i].kp, 0.0,
                    "passive damping must have kp == 0 on joint {i}"
                );
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
    assert_eq!(
        recorded[1],
        HalEvent::Disabled {
            clear_errors: false
        }
    );
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
    assert_eq!(
        recorded[1],
        HalEvent::Disabled {
            clear_errors: false
        }
    );
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
    assert_eq!(
        recorded[1],
        HalEvent::Disabled {
            clear_errors: false
        }
    );
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
