use doso_runtime::hardware::robostride::{
    RawCanFrame, RsDataFrame, RsEvent, RsKey, DEFAULT_HOST_ID,
};
use doso_runtime::hardware::{
    build_can_startup_frames, build_leg_set_zero_frames, parse_imu_baud, partition_mit_cmd_by_leg,
    partition_mit_cmd_by_leg_with_lookup, BenchNullHal, HalBackend, ImuSerialState,
    MaintenanceFrameBuf, SocketCanCore, MAX_MAINTENANCE_FRAMES_PER_TICK, MOTORS_PER_LEG,
    RX_MMSG_BATCH,
};
use doso_runtime::params::{
    flat_index_to_leg_can, leg_can_to_flat_index, motor_limits_for_can_id,
    motor_limits_for_flat_index, LegId, RobotHardwareParams, NUM_JOINTS,
};
use doso_runtime::types::{MitCmd16, MitTarget};

fn make_motion_report_frame(can_id: u8, status_code: u8, error_bits: u8) -> RawCanFrame {
    let data2 =
        ((status_code as u16 & 0x03) << 14) | ((error_bits as u16 & 0x3F) << 8) | (can_id as u16);
    RsDataFrame {
        mode: 0x18,
        data2,
        can_id: DEFAULT_HOST_ID,
        data1: [0x80, 0x00, 0x80, 0x00, 0x80, 0x00, 0x01, 0x04],
    }
    .to_can_frame()
}

fn make_motion_report_frame_pos(can_id: u8, status_code: u8, pos_hi: u8) -> RawCanFrame {
    let data2 = ((status_code as u16 & 0x03) << 14) | (can_id as u16);
    RsDataFrame {
        mode: 0x18,
        data2,
        can_id: DEFAULT_HOST_ID,
        data1: [pos_hi, 0x40, 0x80, 0x00, 0x80, 0x00, 0x01, 0x18],
    }
    .to_can_frame()
}

fn build_distinct_mit_cmd16() -> MitCmd16 {
    MitCmd16(std::array::from_fn(|idx| {
        if idx < 12 {
            // RS04 leg joints (can_id 1..=3): pos [-12.57, 12.57], vel [-15, 15], kp [0, 5000], kd [0, 100], tau [-120, 120]
            let sign = if idx % 2 == 0 { 1.0 } else { -1.0 };
            MitTarget {
                q: sign * (0.15 + 0.23 * idx as f64),
                dq: -sign * (0.4 + 0.65 * idx as f64),
                kp: 45.0 + 12.5 * idx as f64,
                kd: 2.2 + 0.75 * idx as f64,
                tau: sign * (1.8 + 3.1 * idx as f64),
            }
        } else {
            // RS02 wheel joints (can_id 4): pos [-12.57, 12.57], vel [-44, 44], kp [0, 500], kd [0, 5], tau [-17, 17]
            let w = (idx - 12) as f64;
            let sign = if idx % 2 == 0 { 1.0 } else { -1.0 };
            MitTarget {
                q: sign * (0.05 + 0.11 * w),
                dq: sign * (5.5 + 6.25 * w),
                kp: 8.0 * w,
                kd: 0.85 + 0.65 * w,
                tau: -sign * (1.25 + 2.15 * w),
            }
        }
    }))
}

#[test]
fn set_zero_encoding_matches_dart_robo_device_proto_bit_exact() {
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
        assert_eq!(
            item.frame,
            RsEvent::SetZero {
                can_id: expected_can
            }
            .encode()
            .unwrap()
        );
    }
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
            assert_eq!(
                frames[cursor].frame,
                RsEvent::disable(can_id, false).encode().unwrap()
            );
            cursor += 1;
        }
    }
    for leg in LegId::ALL {
        for can_id in 1..=4 {
            assert_eq!(frames[cursor].leg, leg);
            assert_eq!(
                frames[cursor].frame,
                RsEvent::disable(can_id, true).encode().unwrap()
            );
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

    // Custom wheel_can_timeout_us from RobotHardwareParams propagates into startup frames
    let mut custom = RobotHardwareParams::thunder_v4().clone();
    custom.safety.wheel_can_timeout_us = 250_000;
    let (_core, custom_startup) = SocketCanCore::with_hardware_params(&custom, true).unwrap();
    assert_eq!(custom_startup.len(), 84);
    assert_eq!(
        custom_startup[32].frame,
        RsEvent::set_cantimeout_us(4, 250_000).encode().unwrap()
    );
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

    // Request disable -> disable_pending stays true until ALL online motors report status_code == 0
    let disable_tx = core.disable_motors(false).unwrap();
    assert_eq!(disable_tx.len(), 16);
    assert!(core.disable_pending());
    assert!(!core.motor_disable_confirmed());
    assert!(
        core.enable_motors().is_err(),
        "re-enable must be refused while disable_pending is unconfirmed"
    );

    // 15 motors report status_code == 0, but RL wheel (idx 15) stays status_code == 2
    for idx in 0..15 {
        let (leg, can_id) = flat_index_to_leg_can(idx).unwrap();
        core.ingest_rx_frame(leg, make_motion_report_frame(can_id, 0, 0), 30_000);
    }
    assert!(core.disable_pending());
    assert!(!core.motor_disable_confirmed());

    // sample_proprio_step retries disable for the unconfirmed motor AND polls round-robin get_vbus
    let mut maint = Vec::new();
    let _ = core.sample_proprio_step(35_000, |f| maint.push(f));
    assert!(maint.iter().any(|f| {
        f.leg == LegId::Rl && f.frame == RsEvent::disable(4, false).encode().unwrap()
    }));

    // Once RL wheel reports status_code == 0, disable_pending clears and motor_disable_confirmed is true
    core.ingest_rx_frame(LegId::Rl, make_motion_report_frame(4, 0, 0), 40_000);
    assert!(!core.disable_pending());
    assert!(core.motor_disable_confirmed());

    // Verify 16 ticks of sample_proprio_step poll get_vbus across all 16 motors + decode Vbus & Type-21 Fault
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

    // Inject Type-21 (0x15) full fault frame with upper-bit fault (`1 << 16`) on RR calf (idx 8)
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

    // Verify default HalBackend trait implementations on BenchNullHal
    let mut bench = BenchNullHal::new(false);
    assert!(bench.motor_disable_confirmed());
    assert!(bench.calibrate_set_zero().is_err());
}

#[test]
fn imu_serial_state_staleness_and_baud_validation() {
    assert_eq!(parse_imu_baud(None).unwrap(), 921_600);
    assert_eq!(parse_imu_baud(Some("460800")).unwrap(), 460_800);
    assert_eq!(parse_imu_baud(Some("115200")).unwrap(), 115_200);
    assert!(parse_imu_baud(Some("9600")).is_err());

    let mut imu_state = ImuSerialState::new();
    assert!(!imu_state.sample(10_000).online);

    // Valid Hi91 wire frame
    let hex = "5aa54c0002b39101002080e6c54740e20100cdcc4c3d0ad7a3bca4707d3f000020410000a0c10000b04000000000000000000000000000000000000000000000000055cf7d3fa9a8053e0000000000000000";
    let wire: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();

    assert_eq!(imu_state.push_chunk(&wire[..25], 100_000), 0);
    assert_eq!(imu_state.push_chunk(&wire[25..], 100_000), 1);
    assert!(imu_state.sample(250_000).online);
    assert!(
        !imu_state.sample(300_001).online,
        "must go offline past 200ms"
    );
}

#[test]
fn mit_cmd_leg_batch_partitioning_order_and_control_payloads_are_exact() {
    assert_eq!(MOTORS_PER_LEG, 4);
    assert_eq!(RX_MMSG_BATCH, 16);

    let cmd = build_distinct_mit_cmd16();
    let batches = partition_mit_cmd_by_leg(&cmd).expect("valid MIT command batch encoding");

    // 1. Verify exact leg order: FR, FL, RR, RL
    let expected_legs = [LegId::Fr, LegId::Fl, LegId::Rr, LegId::Rl];
    assert_eq!(batches.len(), 4);
    for (leg_idx, expected_leg) in expected_legs.into_iter().enumerate() {
        let batch = &batches[leg_idx];
        assert_eq!(
            batch.leg,
            expected_leg,
            "batch[{leg_idx}] must target {:?} ({})",
            expected_leg,
            expected_leg.label()
        );
        assert_eq!(batch.frames.len(), 4);

        // 2. Verify each leg gets can_id = 1..=4 in ascending order with exact RsEvent::Control bytes
        for slot in 0..4usize {
            let expected_can_id = (slot + 1) as u8;
            let frame = batch.frames[slot];
            let df = RsDataFrame::from_can_frame(frame);
            assert_eq!(df.mode, 0x01, "expected Control mode 0x01");
            assert_eq!(
                df.can_id, expected_can_id,
                "{:?} slot {slot} must address can_id {expected_can_id}",
                expected_leg
            );

            let flat_idx = if expected_can_id <= 3 {
                leg_idx * 3 + slot
            } else {
                12 + leg_idx
            };
            assert_eq!(
                leg_can_to_flat_index(expected_leg, expected_can_id),
                Some(flat_idx)
            );

            let target = &cmd.0[flat_idx];
            let expected_raw = RsEvent::Control {
                can_id: expected_can_id,
                torque: target.tau,
                position: target.q,
                velocity: target.dq,
                kp: target.kp,
                kd: target.kd,
            }
            .encode()
            .unwrap();
            assert_eq!(
                frame, expected_raw,
                "bit-exact Control payload mismatch on {} can_id={expected_can_id} (flat_idx={flat_idx})",
                expected_leg.label()
            );
        }
    }

    // 3. Verify all 16 frames across the 4 leg batches are pairwise distinct for distinct joint targets
    let all_frames: Vec<(LegId, RawCanFrame)> = batches
        .iter()
        .flat_map(|b| b.frames.iter().copied().map(move |f| (b.leg, f)))
        .collect();
    assert_eq!(all_frames.len(), NUM_JOINTS);
    for i in 0..all_frames.len() {
        for j in (i + 1)..all_frames.len() {
            assert_ne!(
                all_frames[i], all_frames[j],
                "duplicate frame at {i} vs {j}"
            );
        }
    }
}

#[test]
fn socketcan_core_encode_mit_cmd_by_leg_respects_safety_gates_and_matches_flat_encode() {
    let cmd = build_distinct_mit_cmd16();

    // Gate 1: Passive listener mode (tx_enabled = false) emits None
    let (passive, _) = SocketCanCore::new(false).unwrap();
    assert!(passive.encode_mit_cmd_by_leg(&cmd).unwrap().is_none());

    // Gate 2: Active mode before enable_motors() emits None
    let (mut core, _) = SocketCanCore::new(true).unwrap();
    assert!(core.encode_mit_cmd_by_leg(&cmd).unwrap().is_none());

    // Bring 16 joints online in reset state (status_code = 0) and enable motors
    for idx in 0..NUM_JOINTS {
        let (leg, can_id) = flat_index_to_leg_can(idx).unwrap();
        core.ingest_rx_frame(leg, make_motion_report_frame_pos(can_id, 0, 0x80), 10_000);
    }
    core.enable_motors().unwrap();

    // Active + enabled -> emits exact 4x4 leg batches matching flat encode_mit_cmd(cmd) per (leg, can_id)
    let batched = core
        .encode_mit_cmd_by_leg(&cmd)
        .unwrap()
        .expect("enabled core must emit 4 leg batches");
    let flat = core.encode_mit_cmd(&cmd).unwrap();
    assert_eq!(flat.len(), NUM_JOINTS);

    for item in &flat {
        let can_id = RsDataFrame::from_can_frame(item.frame).can_id;
        let leg_batch = &batched[item.leg as usize];
        assert_eq!(leg_batch.leg, item.leg);
        assert_eq!(leg_batch.frames[(can_id - 1) as usize], item.frame);
    }

    // Gate 3: Unconfirmed disable (disable_pending = true) blocks MIT batch emission
    for idx in 0..NUM_JOINTS {
        let (leg, can_id) = flat_index_to_leg_can(idx).unwrap();
        core.ingest_rx_frame(leg, make_motion_report_frame_pos(can_id, 2, 0x80), 20_000);
    }
    core.disable_motors(false).unwrap();
    assert!(core.disable_pending());
    assert!(core.encode_mit_cmd_by_leg(&cmd).unwrap().is_none());

    // Simulate a 16-frame burst across legs confirming status_code = 0
    for leg in LegId::ALL {
        for round in 0..4u8 {
            for can_id in 1u8..=4u8 {
                let pos_hi = 0x70 + round * 4 + can_id;
                core.ingest_rx_frame(leg, make_motion_report_frame_pos(can_id, 0, pos_hi), 30_000);
            }
        }
    }
    assert!(!core.disable_pending());
    assert!(core.motor_disable_confirmed());
    // 16 initial + 16 active + 64 burst = 96 ingested frames
    assert_eq!(core.rx_frames(), 96);
}

#[test]
fn parse_can_ifaces_override_validates_four_comma_separated_interfaces() {
    use doso_runtime::hardware::parse_can_ifaces_override;

    assert_eq!(
        parse_can_ifaces_override("can0, can1,can2 , can3"),
        Some([
            "can0".to_string(),
            "can1".to_string(),
            "can2".to_string(),
            "can3".to_string(),
        ])
    );
    assert_eq!(parse_can_ifaces_override("can0,can1,can2"), None);
    assert_eq!(parse_can_ifaces_override("can0,can1,can2,can3,can4"), None);
    assert_eq!(parse_can_ifaces_override("can0,,can2,can3"), None);
}

#[test]
fn joint_lookup_table_o1_parity_and_bounds() {
    let params = RobotHardwareParams::thunder_v4();
    let lookup = params.build_lookup_table().unwrap();

    for idx in 0..NUM_JOINTS {
        let (leg, can_id) = flat_index_to_leg_can(idx).unwrap();
        assert_eq!(params.flat_index_to_leg_can(idx), Some((leg, can_id)));
        assert_eq!(lookup.flat_index_to_leg_can(idx), Some((leg, can_id)));
        assert_eq!(leg_can_to_flat_index(leg, can_id), Some(idx));
        assert_eq!(params.leg_can_to_flat_index(leg, can_id), Some(idx));
        assert_eq!(lookup.leg_can_to_flat_index(leg, can_id), Some(idx));
        assert_eq!(
            motor_limits_for_flat_index(idx),
            params.motor_limits_for_flat_index(idx)
        );
        assert_eq!(
            lookup.motor_limits_for_flat_index(idx),
            params.motor_limits_for_flat_index(idx)
        );
    }
    for can_id in 1..=4 {
        assert_eq!(
            motor_limits_for_can_id(can_id),
            params.motor_limits_for_can_id(can_id)
        );
        assert_eq!(
            lookup.motor_limits_for_can_id(can_id),
            params.motor_limits_for_can_id(can_id)
        );
    }
    assert_eq!(lookup.leg_can_to_flat_index(LegId::Fr, 0), None);
    assert_eq!(lookup.leg_can_to_flat_index(LegId::Fr, 5), None);
    assert_eq!(lookup.flat_index_to_leg_can(NUM_JOINTS), None);
    assert_eq!(lookup.motor_limits_for_flat_index(NUM_JOINTS), None);
    assert_eq!(lookup.motor_limits_for_can_id(0), None);
    assert_eq!(lookup.motor_limits_for_can_id(5), None);

    let cmd = build_distinct_mit_cmd16();
    assert_eq!(
        partition_mit_cmd_by_leg(&cmd).unwrap(),
        partition_mit_cmd_by_leg_with_lookup(&cmd, &lookup).unwrap()
    );
}

#[test]
fn maintenance_frame_buf_handles_worst_case_17_frames_on_stack() {
    assert_eq!(MAX_MAINTENANCE_FRAMES_PER_TICK, NUM_JOINTS + 1);
    let mut buf = MaintenanceFrameBuf::new();
    assert!(buf.is_empty());

    let (mut core, _) = SocketCanCore::new(true).unwrap();
    for idx in 0..NUM_JOINTS {
        let (leg, can_id) = flat_index_to_leg_can(idx).unwrap();
        core.ingest_rx_frame(leg, make_motion_report_frame(can_id, 2, 0), 10_000);
    }
    core.disable_motors(true).unwrap();
    assert!(core.disable_pending());

    let _ = core.sample_proprio_step(15_000, |cb| {
        assert!(buf.push(cb), "worst-case 17 frames must fit on stack");
    });
    assert_eq!(buf.len(), MAX_MAINTENANCE_FRAMES_PER_TICK);
    assert_eq!(buf.as_slice().len(), MAX_MAINTENANCE_FRAMES_PER_TICK);
    // 18th frame rejects safely without panicking
    let extra = buf.as_slice()[0];
    assert!(!buf.push(extra));
}
