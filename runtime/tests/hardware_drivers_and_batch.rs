use doso_runtime::hardware::robostride::{
    cantimeout_ticks_to_us, cantimeout_us_to_ticks, decode_compressed_error_names,
    decode_fault_bits_32_names, decode_rs_frame, decode_warning_bits_32_names, epscan_ms_to_raw,
    epscan_raw_to_ms, RawCanFrame, RsBaudRate, RsDataFrame, RsDecodedState, RsEvent, RsKey,
    RsParamType, RsParamValue, RsProtocol, RsRunMode, DEFAULT_HOST_ID,
};
use doso_runtime::hardware::{
    build_all_set_zero_frames, build_can_startup_frames, build_leg_set_zero_frames, parse_imu_baud,
    partition_mit_cmd_by_leg, BenchNullHal, HalBackend, ImuSerialState, SocketCanCore,
    MOTORS_PER_LEG, RX_MMSG_BATCH,
};
use doso_runtime::params::{
    flat_index_to_leg_can, leg_can_to_flat_index, LegId, MotorModel, RobotHardwareParams,
    NUM_JOINTS,
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
    // 16 disable(false) + 16 disable(true) + 16 set_run_mode(Control) + 4 set_cantimeout_us(4, 500_000) + 3*16 set_reporting(true) = 100
    assert_eq!(frames.len(), 100);

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
        for can_id in 1..=4 {
            assert_eq!(frames[cursor].leg, leg);
            assert_eq!(
                frames[cursor].frame,
                RsEvent::set_run_mode(can_id, RsRunMode::Control)
                    .encode()
                    .unwrap()
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
    assert_eq!(cursor, 100);

    // Custom wheel_can_timeout_us from RobotHardwareParams propagates into startup frames
    let mut custom = RobotHardwareParams::thunder_v4().clone();
    custom.safety.wheel_can_timeout_us = 250_000;
    let (_core, custom_startup) = SocketCanCore::with_hardware_params(&custom, true).unwrap();
    assert_eq!(custom_startup.len(), 100);
    assert_eq!(
        custom_startup[48].frame,
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

    // Active mode (`tx_enabled = true`) emits all 100 startup frames
    let (mut core, startup) = SocketCanCore::new(true).unwrap();
    assert_eq!(startup.len(), 100);
    assert!(core.motor_disable_confirmed());

    // Bring all 16 motors online in reset status (0), verify calibrate_set_zero emits 48 persistent frames
    for idx in 0..NUM_JOINTS {
        let (leg, can_id) = flat_index_to_leg_can(idx).unwrap();
        core.ingest_rx_frame(leg, make_motion_report_frame(can_id, 0, 0), 10_000);
    }
    let zero_frames = core.calibrate_set_zero().unwrap();
    assert_eq!(zero_frames.len(), 48);
    assert_eq!(zero_frames[0].frame, RsEvent::set_zero(1).encode().unwrap());
    assert_eq!(
        zero_frames[16].frame,
        RsEvent::set_zero_sta(1, true).encode().unwrap()
    );
    assert_eq!(
        zero_frames[32].frame,
        RsEvent::save_data(1).encode().unwrap()
    );

    // Inject GetVersion reply on FR thigh (can_id = 2, flat_idx = 1) and confirm it does not clobber joint angle q
    let q_before = core.joints()[1].q;
    let ver_rx = RsDataFrame {
        mode: 0x02,
        data2: 2,
        can_id: DEFAULT_HOST_ID,
        data1: [0x00, 0xC4, 0x56, 0, 4, 1, 38, 0],
    }
    .to_can_frame();
    core.ingest_rx_frame(LegId::Fr, ver_rx, 15_000);
    assert_eq!(core.firmware_versions()[1], Some([0, 4, 1, 38]));
    assert_eq!(core.joints()[1].q, q_before);

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
fn all_set_zero_48_frame_sequence_is_exact() {
    let frames = build_all_set_zero_frames().unwrap();
    assert_eq!(frames.len(), 48);

    let mut cursor = 0usize;
    for leg in LegId::ALL {
        for can_id in 1..=4 {
            assert_eq!(frames[cursor].leg, leg);
            assert_eq!(
                frames[cursor].frame,
                RsEvent::set_zero(can_id).encode().unwrap()
            );
            cursor += 1;
        }
    }
    for leg in LegId::ALL {
        for can_id in 1..=4 {
            assert_eq!(frames[cursor].leg, leg);
            assert_eq!(
                frames[cursor].frame,
                RsEvent::set_zero_sta(can_id, true).encode().unwrap()
            );
            cursor += 1;
        }
    }
    for leg in LegId::ALL {
        for can_id in 1..=4 {
            assert_eq!(frames[cursor].leg, leg);
            assert_eq!(
                frames[cursor].frame,
                RsEvent::save_data(can_id).encode().unwrap()
            );
            cursor += 1;
        }
    }
    assert_eq!(cursor, 48);
}

#[test]
fn robostride_2026_keys_complete_roundtrip_and_value_parsing() {
    assert_eq!(RsKey::ALL.len(), 29);
    for key in RsKey::ALL {
        let code = key as u16;
        assert_eq!(RsKey::from_u16(code), Some(key));
        assert_eq!(RsKey::parse_name_or_hex(key.name()), Some(key));
        let hex = format!("0x{code:04x}");
        assert_eq!(RsKey::parse_name_or_hex(&hex), Some(key));

        let sample = match key.value_type() {
            RsParamType::U8 => key.parse_value_str("1").unwrap(),
            RsParamType::U16 => key.parse_value_str("3").unwrap(),
            RsParamType::U32 => key.parse_value_str("20000").unwrap(),
            RsParamType::F32 => key.parse_value_str("12.5").unwrap(),
        };
        let mut bytes4 = [0u8; 4];
        sample.write_le_bytes(&mut bytes4);
        assert_eq!(key.decode_value(bytes4), sample);
        assert!(!sample.format_for_key(key).is_empty());
    }

    assert!(!RsKey::MechPos.is_writable());
    assert!(!RsKey::Iqf.is_writable());
    assert!(!RsKey::MechVel.is_writable());
    assert!(!RsKey::Vbus.is_writable());
    assert!(RsKey::ZeroSta.is_writable());
    assert!(RsKey::RunMode.is_writable());

    assert_eq!(epscan_raw_to_ms(0), 10);
    assert_eq!(epscan_raw_to_ms(1), 10);
    assert_eq!(epscan_raw_to_ms(2), 15);
    assert_eq!(epscan_ms_to_raw(10), 1);
    assert_eq!(epscan_ms_to_raw(25), 4);
    assert_eq!(cantimeout_us_to_ticks(1_000_000), 20_000);
    assert_eq!(cantimeout_ticks_to_us(20_000), 1_000_000);
}

#[test]
fn robostride_2026_events_and_decoded_states() {
    // Wire encodings for Communication Types 0, 5, 7, 17, 18, 22, 23, 25, 26
    let get_id = RsEvent::get_device_id(127).encode().unwrap();
    assert_eq!(get_id.id, (DEFAULT_HOST_ID as u32) << 8 | 127);

    let cal = RsEvent::calibration(2).encode().unwrap();
    assert_eq!(cal.id, (0x05 << 24) | (0x00FD << 8) | 2);

    let set_id = RsEvent::set_id(127, 3).encode().unwrap();
    assert_eq!(
        set_id.id,
        (0x07 << 24) | ((3u32) << 16) | ((DEFAULT_HOST_ID as u32) << 8) | 127
    );

    let save = RsEvent::save_data(4).encode().unwrap();
    assert_eq!(save.id, (0x16 << 24) | ((DEFAULT_HOST_ID as u32) << 8) | 4);
    assert_eq!(save.data, [1, 2, 3, 4, 5, 6, 7, 8]);

    let baud = RsEvent::set_baud_rate(1, RsBaudRate::Rate500K)
        .encode()
        .unwrap();
    assert_eq!(baud.id, (0x17 << 24) | ((DEFAULT_HOST_ID as u32) << 8) | 1);
    assert_eq!(baud.data, [1, 2, 3, 4, 5, 6, 2, 0]);

    let proto = RsEvent::set_protocol(1, RsProtocol::Private)
        .encode()
        .unwrap();
    assert_eq!(proto.id, (0x19 << 24) | ((DEFAULT_HOST_ID as u32) << 8) | 1);
    assert_eq!(proto.data, [1, 2, 3, 4, 5, 6, 0, 0]);

    let ver = RsEvent::get_version(2).encode().unwrap();
    assert_eq!(ver.id, (0x04 << 24) | ((DEFAULT_HOST_ID as u32) << 8) | 2);
    assert_eq!(ver.data, [0x00, 0xC4, 0, 0, 0, 0, 0, 0]);

    // Type-18 validation: read-only & wrong type rejected; valid writes encode little-endian
    assert!(RsEvent::set_param(1, RsKey::Vbus, RsParamValue::F32(48.0))
        .encode()
        .is_err());
    assert!(
        RsEvent::set_param(1, RsKey::ZeroSta, RsParamValue::F32(1.0))
            .encode()
            .is_err()
    );
    let zero_sta = RsEvent::set_zero_sta(3, true).encode().unwrap();
    assert_eq!(zero_sta.data[0..2], [0x29, 0x70]);
    assert_eq!(zero_sta.data[4..8], [1, 0, 0, 0]);

    let epscan = RsEvent::set_epscan_time_ms(1, 20).encode().unwrap();
    assert_eq!(epscan.data[0..2], [0x26, 0x70]);
    assert_eq!(epscan.data[4..6], 3u16.to_le_bytes());

    let damper = RsEvent::set_damper(1, true).encode().unwrap();
    assert_eq!(damper.data[0..2], [0x2A, 0x70]);
    assert_eq!(damper.data[4], 1);

    let add_off = RsEvent::set_add_offset(2, 0.25).encode().unwrap();
    assert_eq!(add_off.data[0..2], [0x2B, 0x70]);
    assert_eq!(add_off.data[4..8], 0.25f32.to_le_bytes());

    // Factory default CAN ID 127 allowed with explicit limits
    let rs04_limits = RobotHardwareParams::thunder_v4().motors.rs04;
    let bench_ctrl = RsEvent::Control {
        can_id: 127,
        torque: 0.0,
        position: 0.0,
        velocity: 0.0,
        kp: 0.0,
        kd: 0.0,
    };
    assert!(bench_ctrl.encode().is_err());
    assert!(bench_ctrl.encode_with_limits(Some(rs04_limits)).is_ok());

    // Decode Type 0 DeviceId
    let dev_raw = RsDataFrame {
        mode: 0x00,
        data2: 2,
        can_id: 0xFE,
        data1: [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
    }
    .to_can_frame();
    let RsDecodedState::DeviceId(dev) = decode_rs_frame(dev_raw).unwrap() else {
        panic!("expected DeviceId");
    };
    assert_eq!(dev.can_id, 2);
    assert_eq!(dev.mcu_hex(), "0102030405060708");
    assert_eq!(dev.mcu_dart_decimal(), "102030405060708");
    assert!(dev.matches_expected("0x0102030405060708"));

    // Decode Type 26 Version response BEFORE Motion decoding (critical disambiguation!)
    let ver_raw = RsDataFrame {
        mode: 0x02,
        data2: ((2u16) << 14) | 127,
        can_id: DEFAULT_HOST_ID,
        data1: [0x00, 0xC4, 0x56, 0, 4, 1, 38, 0],
    }
    .to_can_frame();
    let RsDecodedState::FirmwareVersion(v) = decode_rs_frame(ver_raw).unwrap() else {
        panic!("expected FirmwareVersion, must not misparse as Motion");
    };
    assert_eq!(v.can_id, 127);
    assert_eq!(v.display_string(), "0.4.1.38");
    assert!(v.supports_modern_protocol(MotorModel::Rs04));
    assert!(!RsFirmwareVersionAlias::old_rs04().supports_modern_protocol(MotorModel::Rs04));

    // Decode Type 17 typed getter responses (U8 / U16 / U32 / F32)
    let mut rm_data = [0u8; 8];
    rm_data[0..2].copy_from_slice(&(RsKey::RunMode as u16).to_le_bytes());
    rm_data[4] = 5;
    let rm_raw = RsDataFrame {
        mode: 0x11,
        data2: 1,
        can_id: DEFAULT_HOST_ID,
        data1: rm_data,
    }
    .to_can_frame();
    let RsDecodedState::Param { key, value, .. } = decode_rs_frame(rm_raw).unwrap() else {
        panic!("expected Param");
    };
    assert_eq!(key, RsKey::RunMode);
    assert_eq!(value, RsParamValue::U8(5));

    // Decode Type 21 fault & warning bits
    let mut f_data = [0u8; 8];
    let f_bits = (1u32 << 3) | (1u32 << 14) | (1u32 << 16);
    let w_bits = 1u32;
    f_data[0..4].copy_from_slice(&f_bits.to_le_bytes());
    f_data[4..8].copy_from_slice(&w_bits.to_le_bytes());
    let f_raw = RsDataFrame {
        mode: 0x15,
        data2: 2,
        can_id: DEFAULT_HOST_ID,
        data1: f_data,
    }
    .to_can_frame();
    let RsDecodedState::FaultFrame {
        fault_bits,
        warning_bits,
        ..
    } = decode_rs_frame(f_raw).unwrap()
    else {
        panic!("expected FaultFrame");
    };
    assert_eq!(fault_bits, f_bits);
    assert_eq!(warning_bits, w_bits);
    assert_eq!(
        decode_fault_bits_32_names(fault_bits),
        vec!["overVoltage", "stallOverload", "phaseAOverCurrent"]
    );
    assert_eq!(
        decode_warning_bits_32_names(warning_bits),
        vec!["overTemperatureWarning"]
    );
    assert_eq!(
        decode_compressed_error_names(0b10_0101),
        vec!["underVoltage", "overTemperature", "uncalibrated"]
    );
}

struct RsFirmwareVersionAlias;
impl RsFirmwareVersionAlias {
    const fn old_rs04() -> doso_runtime::hardware::robostride::RsFirmwareVersion {
        doso_runtime::hardware::robostride::RsFirmwareVersion {
            host_id: DEFAULT_HOST_ID,
            can_id: 1,
            status_code: 0,
            error_bits: 0,
            version: [0, 4, 0, 9],
        }
    }
}
