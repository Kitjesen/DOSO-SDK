use doso_core::{flat_index_to_leg_can, LegId, NUM_JOINTS};
use doso_hal::robostride::{RawCanFrame, RsDataFrame, RsEvent, RsKey, DEFAULT_HOST_ID};
use doso_hal::yunzhuo::{SBUS_CENTER, SBUS_HIGH, SBUS_LOW};
use doso_hal::{
    build_can_startup_frames, build_leg_set_zero_frames, parse_imu_baud, BenchNullHal, GSState,
    HalBackend, ImuSerialState, JoystickPadMapper, LinuxJsEvent, SocketCanCore,
    YunZhuoControlMode, YunZhuoModeStateMachine, YunZhuoState, JS_EVENT_AXIS, JS_EVENT_BUTTON,
    JS_EVENT_INIT,
};

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

fn make_motion_report_frame(can_id: u8, status_code: u8, error_bits: u8) -> RawCanFrame {
    let data2 = ((status_code as u16 & 0x03) << 14)
        | ((error_bits as u16 & 0x3F) << 8)
        | (can_id as u16);
    RsDataFrame {
        mode: 0x18,
        data2,
        can_id: DEFAULT_HOST_ID,
        data1: [0x80, 0x00, 0x80, 0x00, 0x80, 0x00, 0x01, 0x04],
    }
    .to_can_frame()
}

#[test]
fn set_zero_encoding_matches_dart_robo_device_proto_bit_exact() {
    // Verified against Dart `robo_device_proto/lib/src/robostride/event.dart`:
    // RSEvent.setZero(canId, {int hostId = 0xFF}) =>
    //   RSDataFrame(mode: 0x06, data2: 0xFF, canId: canId, data1: [1, 0, 0, 0, 0, 0, 0, 0])
    for can_id in 1u8..=4u8 {
        let ev = RsEvent::SetZero { can_id };
        assert_eq!(ev, RsEvent::set_zero(can_id));
        let frame = ev.encode().unwrap();
        let expected_id = (0x06u32 << 24) | ((DEFAULT_HOST_ID as u32) << 8) | (can_id as u32);
        assert_eq!(frame.id, expected_id);
        assert_eq!(frame.data, [1, 0, 0, 0, 0, 0, 0, 0]);
    }

    let leg_batch = build_leg_set_zero_frames().unwrap();
    assert_eq!(leg_batch.len(), 12);
    for (i, item) in leg_batch.iter().enumerate() {
        let expected_leg = LegId::from_index(i / 3).unwrap();
        let expected_can = (i % 3 + 1) as u8;
        assert_eq!(item.leg, expected_leg);
        assert_eq!(item.frame, RsEvent::SetZero { can_id: expected_can }.encode().unwrap());
    }
}

#[test]
fn yunzhuo_mode_machine_enforces_neutral_gate_and_rejects_quick_sweep() {
    let mut sm = YunZhuoModeStateMachine::new();

    // 1. Cold boot with G/S already in Up (or Down) stays locked in Neutral
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
        &make_yz_state(GSState::Middle, 0.15, 0.0, 0.0, 0.0, true, false, false, 0.0),
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

    // 4. Quick sweep Dog -> Middle (only 40ms) -> Down (100ms) is rejected to Neutral, never Arm!
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

    // Leaving Arm drops to Neutral IMMEDIATELY in 1ms without waiting for 100ms debounce!
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
    let d0 = m0 + 320_000; // 770_000
    sm.update(
        &make_yz_state(GSState::Up, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
        d0,
    );
    let enter_dog = sm.update(
        &make_yz_state(GSState::Up, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0),
        d0 + 100_000, // 870_000: ramp-up starts at 0.0
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
        exit_req + 100_000, // debounced to Neutral, ramp-down begins from 1.0
    );
    assert_eq!(ramp_down_start.mode, YunZhuoControlMode::Neutral);
    assert!((ramp_down_start.dog_drive_scale - 1.0).abs() < 1e-9);

    let ramp_down_mid1 = sm.update(
        &make_yz_state(GSState::Middle, 0.0, 0.8, 0.0, 0.0, true, false, false, 0.0),
        exit_req + 200_000, // +100ms into 400ms ramp -> 0.75
    );
    assert!((ramp_down_mid1.dog_drive_scale - 0.75).abs() < 1e-9);

    let ramp_down_half = sm.update(
        &make_yz_state(GSState::Middle, 0.0, 0.8, 0.0, 0.0, true, false, false, 0.0),
        exit_req + 300_000, // +200ms into 400ms ramp -> 0.50
    );
    assert!((ramp_down_half.dog_drive_scale - 0.5).abs() < 1e-9);

    sm.update(
        &make_yz_state(GSState::Middle, 0.0, 0.8, 0.0, 0.0, true, false, false, 0.0),
        exit_req + 400_000,
    );
    let ramp_down_end = sm.update(
        &make_yz_state(GSState::Middle, 0.0, 0.8, 0.0, 0.0, true, false, false, 0.0),
        exit_req + 500_000, // +400ms into 400ms ramp -> 0.0
    );
    assert!((ramp_down_end.dog_drive_scale - 0.0).abs() < 1e-9);

    // Deadzone arming (<0.02) and latched switch edge detection (L1/L2/R1) when H toggles on
    let t_btn = exit_req + 550_000;
    let mut yz = make_yz_state(GSState::Middle, 0.0, 0.5, 0.0, 0.0, false, false, false, 0.0);
    yz.l1 = true; // L1 already high while H is off
    let h_off = sm.update(&yz, t_btn);
    assert!(!h_off.standup_triggered);
    assert!(!h_off.direction_armed);

    // H turns on with L1 already high and stick displaced (0.5 >= 0.02):
    // neither L1 nor stick movement fires until baseline/arming occurs!
    yz.h = true;
    let h_just_on = sm.update(&yz, t_btn + 20_000);
    assert!(!h_just_on.standup_triggered);
    assert!(!h_just_on.direction_armed);

    // Toggling L1 (true->false), L2 (false->true), R1 (false->true) while H is enabled triggers one-shot edges
    yz.l1 = false;
    yz.l2 = true;
    yz.r1 = true;
    let edges = sm.update(&yz, t_btn + 40_000);
    assert!(edges.standup_triggered);
    assert!(edges.sitdown_triggered);
    assert!(edges.damp_triggered);

    unreachable_check_edges(&mut sm);
}

fn unreachable_check_edges(sm: &mut YunZhuoModeStateMachine) {
    sm.reset_for_input_loss();
    // Establish H=true baseline with L1=true, L2=false, R1=false
    let mut s = make_yz_state(GSState::Middle, 0.0, 0.0, 0.0, 0.0, true, false, false, 0.0);
    s.l1 = true;
    let first = sm.update(&s, 10_000);
    assert!(!first.standup_triggered);
    assert!(!first.sitdown_triggered);
    assert!(!first.damp_triggered);

    // Flip L1 (true -> false), L2 (false -> true), R1 (false -> true)
    s.l1 = false;
    s.l2 = true;
    s.r1 = true;
    let flipped = sm.update(&s, 30_000);
    assert!(flipped.standup_triggered && flipped.pad.l1);
    assert!(flipped.sitdown_triggered && flipped.pad.l2);
    assert!(flipped.damp_triggered && flipped.pad.r1);

    // Holding same switch positions on next frame does NOT re-trigger
    let held = sm.update(&s, 50_000);
    assert!(!held.standup_triggered && !held.pad.l1);
    assert!(!held.sitdown_triggered && !held.pad.l2);
    assert!(!held.damp_triggered && !held.pad.r1);

    // Enter Dog, then trigger >150ms watchdog frame loss -> resets to Neutral!
    for step in 0..=6 {
        sm.update(&s, 100_000 + step * 50_000);
    }
    s.g_s = GSState::Up;
    sm.update(&s, 420_000);
    let in_dog = sm.update(&s, 520_000);
    assert_eq!(in_dog.mode, YunZhuoControlMode::Dog);

    // 200ms frame gap (> 150ms watchdog)
    assert!(sm.check_watchdog(730_000));
    assert_eq!(sm.mode(), YunZhuoControlMode::Neutral);
    assert!(!sm.neutral_gate_ready());
}

#[test]
fn can_startup_sequence_frame_count_and_order_match_thunder_dart() {
    let frames = build_can_startup_frames().unwrap();
    // 16 disable(false) + 16 disable(true) + 4 set_cantimeout_us(4, 500_000) + 3*16 set_reporting(true) = 84
    assert_eq!(frames.len(), 84);

    let mut cursor = 0usize;
    for leg in LegId::ALL {
        for can_id in 1..=4 {
            assert_eq!(frames[cursor].leg, leg);
            assert_eq!(frames[cursor].frame, RsEvent::disable(can_id, false).encode().unwrap());
            cursor += 1;
        }
    }
    for leg in LegId::ALL {
        for can_id in 1..=4 {
            assert_eq!(frames[cursor].leg, leg);
            assert_eq!(frames[cursor].frame, RsEvent::disable(can_id, true).encode().unwrap());
            cursor += 1;
        }
    }
    for leg in LegId::ALL {
        assert_eq!(frames[cursor].leg, leg);
        assert_eq!(
            frames[cursor].frame,
            RsEvent::set_cantimeout_us(4, 500_000).encode().unwrap()
        );
        cursor += 1;
    }
    for _ in 0..3 {
        for leg in LegId::ALL {
            for can_id in 1..=4 {
                assert_eq!(frames[cursor].leg, leg);
                assert_eq!(
                    frames[cursor].frame,
                    RsEvent::set_reporting(can_id, true).encode().unwrap()
                );
                cursor += 1;
            }
        }
    }
    assert_eq!(cursor, 84);
}

#[test]
fn verified_disable_vbus_polling_fault_decode_and_set_zero_gates() {
    // Shadow-listener mode (`tx_enabled = false`) emits 0 startup frames and confirms disable immediately
    let (mut passive, startup_none) = SocketCanCore::new(false).unwrap();
    assert!(startup_none.is_empty());
    assert!(passive.motor_disable_confirmed());
    assert!(passive.calibrate_set_zero().is_err());

    // Active mode (`tx_enabled = true`) emits all 84 startup frames
    let (mut core, startup) = SocketCanCore::new(true).unwrap();
    assert_eq!(startup.len(), 84);
    assert!(core.motor_disable_confirmed());

    // Bring all 16 motors online in reset status (0), verify calibrate_set_zero succeeds while disabled
    for idx in 0..NUM_JOINTS {
        let (leg, can_id) = flat_index_to_leg_can(idx).unwrap();
        core.ingest_rx_frame(leg, make_motion_report_frame(can_id, 0, 0), 10_000);
    }
    let zero_frames = core.calibrate_set_zero().unwrap();
    assert_eq!(zero_frames.len(), 12);

    // Enable motors -> status_code becomes 2 (`RSStatus.motor`)
    let enable_tx = core.enable_motors().unwrap();
    assert_eq!(enable_tx.len(), 16);
    assert!(!core.motor_disable_confirmed());
    assert!(core.calibrate_set_zero().is_err());

    for idx in 0..NUM_JOINTS {
        let (leg, can_id) = flat_index_to_leg_can(idx).unwrap();
        core.ingest_rx_frame(leg, make_motion_report_frame(can_id, 2, 0), 20_000);
    }

    // Request disable -> disable_pending stays true until ALL online motors report status_code == 0!
    let disable_tx = core.disable_motors(false).unwrap();
    assert_eq!(disable_tx.len(), 16);
    assert!(core.disable_pending());
    assert!(!core.motor_disable_confirmed());
    assert!(
        core.enable_motors().is_err(),
        "re-enable must be refused while disable_pending is unconfirmed"
    );

    // 15 motors report status_code == 0, but RL wheel (idx 15) drops stop and stays status_code == 2
    for idx in 0..15 {
        let (leg, can_id) = flat_index_to_leg_can(idx).unwrap();
        core.ingest_rx_frame(leg, make_motion_report_frame(can_id, 0, 0), 30_000);
    }
    assert!(core.disable_pending());
    assert!(!core.motor_disable_confirmed());

    // sample_proprio_step retries disable for the unconfirmed motor AND polls round-robin get_vbus!
    let mut maint = Vec::new();
    let _ = core.sample_proprio_step(35_000, |f| maint.push(f));
    assert!(maint.iter().any(|f| {
        f.leg == LegId::Rl && f.frame == RsEvent::disable(4, false).encode().unwrap()
    }));

    // Once RL wheel reports status_code == 0, disable_pending clears and motor_disable_confirmed is true!
    core.ingest_rx_frame(LegId::Rl, make_motion_report_frame(4, 0, 0), 40_000);
    assert!(!core.disable_pending());
    assert!(core.motor_disable_confirmed());

    // Verify 16 ticks of sample_proprio_step poll get_vbus across all 16 motors + decode Vbus & Type-21 Fault!
    let mut vbus_polled = Vec::new();
    for step in 0..16u64 {
        core.sample_proprio_step(45_000 + step * 5_000, |f| vbus_polled.push(f));
    }
    assert_eq!(vbus_polled.len(), 16);

    // Inject Vbus getter response (51.2V) on FL thigh (can_id = 2, flat_idx = 4)
    let mut vbus_payload = [0u8; 8];
    vbus_payload[0..2].copy_from_slice(&(RsKey::Vbus as u16).to_le_bytes());
    vbus_payload[4..8].copy_from_slice(&51.2f32.to_le_bytes());
    let vbus_rx = RsDataFrame {
        mode: 0x11,
        data2: 2,
        can_id: DEFAULT_HOST_ID,
        data1: vbus_payload,
    }
    .to_can_frame();
    core.ingest_rx_frame(LegId::Fl, vbus_rx, 130_000);
    assert!((core.joints()[4].vbus_v - 51.2).abs() < 1e-4);

    // Inject Type-21 (0x15) full fault frame with upper-bit fault (`1 << 16` phaseAOverCurrent) on RR calf (idx 8)
    let mut fault_payload = [0u8; 8];
    fault_payload[0..4].copy_from_slice(&(1u32 << 16).to_le_bytes());
    let fault_rx = RsDataFrame {
        mode: 0x15,
        data2: 3,
        can_id: DEFAULT_HOST_ID,
        data1: fault_payload,
    }
    .to_can_frame();
    core.ingest_rx_frame(LegId::Rr, fault_rx, 135_000);
    assert_ne!(core.joints()[8].error_bits, 0);

    // Also verify default HalBackend trait implementations on BenchNullHal
    let mut bench = BenchNullHal::new(false);
    assert!(bench.motor_disable_confirmed());
    assert!(bench.calibrate_set_zero().is_err());
}

#[test]
fn imu_serial_state_staleness_and_linux_js_event_mapper() {
    assert_eq!(parse_imu_baud(None).unwrap(), 921_600);
    assert_eq!(parse_imu_baud(Some("460800")).unwrap(), 460_800);
    assert_eq!(parse_imu_baud(Some("115200")).unwrap(), 115_200);
    assert!(parse_imu_baud(Some("9600")).is_err());

    let mut imu_state = ImuSerialState::new();
    assert!(!imu_state.sample(10_000).online);

    // Valid Hi91 wire frame from dart_golden_parity
    let hex = "5aa54c0002b39101002080e6c54740e20100cdcc4c3d0ad7a3bca4707d3f000020410000a0c10000b04000000000000000000000000000000000000000000000000055cf7d3fa9a8053e0000000000000000";
    let wire: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();

    assert_eq!(imu_state.push_chunk(&wire[..25], 100_000), 0);
    assert_eq!(imu_state.push_chunk(&wire[25..], 100_000), 1);
    assert!(imu_state.sample(250_000).online);
    assert!(!imu_state.sample(300_001).online, "must go offline past 200ms");

    // Verify 8-byte Linux js_event parsing and Xbox pad mapping
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
