use doso_hal::hipnuc::{decode_hi91_payload, ProtoframeStreamDecoder};
use doso_hal::robostride::{decode_rs_frame, RawCanFrame, RsDecodedState, RsEvent};

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn robostride_can_encoding_is_bit_exact_with_dart_brainstem() {
    // Exact byte vectors dumped directly from Dart `robo_device_proto`!
    let cases: &[(&str, RsEvent, u32, &str)] = &[
        ("enable_hip1", RsEvent::enable(1), 50396929, "0000000000000000"),
        (
            "disable_calf3_clear",
            RsEvent::disable(3, true),
            67174147,
            "0100000000000000",
        ),
        (
            "disable_wheel4_noclear",
            RsEvent::disable(4, false),
            67174148,
            "0000000000000000",
        ),
        (
            "set_reporting_on_2",
            RsEvent::set_reporting(2, true),
            402718466,
            "0102030405060100",
        ),
        ("get_vbus_1", RsEvent::get_vbus(1), 285277953, "1c70000000000000"),
        (
            "set_cantimeout_4_500ms",
            RsEvent::set_cantimeout_us(4, 500_000),
            302055172,
            "2870000010270000",
        ),
        (
            "mit_leg_rs04_can1",
            RsEvent::Control {
                can_id: 1,
                torque: 12.5,
                position: -0.35,
                velocity: 1.2,
                kp: 80.0,
                kd: 6.93,
            },
            26039553,
            "7c6f8a3d041911be",
        ),
        (
            "mit_wheel_rs02_can4",
            RsEvent::Control {
                can_id: 4,
                torque: -2.0,
                position: 0.0,
                velocity: 8.5,
                kp: 0.0,
                kd: 1.0,
            },
            24178948,
            "800098ba00003333",
        ),
    ];

    for (name, ev, expected_id, expected_hex) in cases {
        let raw = ev.encode().unwrap();
        let actual_hex: String = raw.data.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(raw.id, *expected_id, "CAN ID mismatch on {name}");
        assert_eq!(actual_hex, *expected_hex, "CAN payload mismatch on {name}");
    }

    // Also verify RS04 / RS02 feedback decode roundtrip
    let report = RawCanFrame {
        id: (0x18 << 24) | (0b1000_0000_0000_0001_u32 << 8) | 0xFF,
        data: [0x80, 0x00, 0x80, 0x00, 0x80, 0x00, 0x01, 0x2C], // ~0 pos, ~0 vel, ~0 tau, 30.0 C
    };
    let RsDecodedState::Motion(m) = decode_rs_frame(report).unwrap() else {
        panic!("expected motion report");
    };
    assert!(m.is_report);
    assert_eq!(m.can_id, 1);
    assert_eq!(m.status_code, 2);
    assert!((m.temperature_c - 30.0).abs() < 1e-3);
}

#[test]
fn hipnuc_hi91_crc16_and_projected_gravity_are_bit_exact_with_dart() {
    let frame_hex = "5aa54c0002b39101002080e6c54740e20100cdcc4c3d0ad7a3bca4707d3f000020410000a0c10000b04000000000000000000000000000000000000000000000000055cf7d3fa9a8053e0000000000000000";
    let wire = hex_to_bytes(frame_hex);

    // Feed in fragmented chunks to verify streaming decoder state machine!
    let mut decoder = ProtoframeStreamDecoder::default();
    let mut decoded_imu = None;
    decoder.push_and_decode(&wire[..17], |_| panic!("should wait for full frame"));
    decoder.push_and_decode(&wire[17..], |payload| {
        decoded_imu = Some(decode_hi91_payload(payload, 42_000).unwrap());
    });

    let imu = decoded_imu.expect("Hi91 frame should pass CRC16-CCITT and decode");
    let expected_grav = [0.0, -0.2588190734386444, -0.9659258723258972];
    let expected_gyro = [0.17453292519943295, -0.3490658503988659, 0.09599310885968812];
    let expected_acc = [0.4903325073065236, -0.19613299561608583, 9.708583593523501];

    for i in 0..3 {
        assert!(
            (imu.projected_gravity[i] - expected_grav[i]).abs() < 1e-6,
            "projected_gravity[{i}] mismatch: got {}, expected {}",
            imu.projected_gravity[i],
            expected_grav[i]
        );
        assert!((imu.gyro_rad_s[i] - expected_gyro[i]).abs() < 1e-6);
        assert!((imu.accel_m_s2[i] - expected_acc[i]).abs() < 1e-6);
    }
}
