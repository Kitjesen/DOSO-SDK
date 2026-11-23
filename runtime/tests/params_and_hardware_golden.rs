use std::path::PathBuf;

use doso_runtime::hardware::hipnuc::{decode_hi91_payload, ProtoframeStreamDecoder};
use doso_runtime::hardware::robostride::{decode_rs_frame, RawCanFrame, RsDecodedState, RsEvent};
use doso_runtime::params::{
    flat_index_to_leg_can, leg_can_to_flat_index, MotorModel, MotorRole, RobotHardwareParams,
    EMBEDDED_THUNDER_V4_YAML, NUM_JOINTS, WHEEL_START_INDEX,
};
use doso_runtime::safety::{deploy_action_max, deploy_action_min, SafetyConfig};

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn thunder_v4_yaml_embedded_and_file_match_and_validate() {
    let embedded = RobotHardwareParams::thunder_v4();
    let cfg_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("config")
        .join("thunder_v4.yaml");
    let loaded = RobotHardwareParams::from_file(&cfg_path)
        .expect("config/thunder_v4.yaml must load and validate");
    assert_eq!(embedded, &loaded);
    assert_eq!(loaded.robot_model, "thunder_v4");
    assert_eq!(
        loaded.can_bus_interfaces(),
        ["can0", "can2", "can1", "can3"]
    );
    assert_eq!(loaded.devices.imu_port, "/dev/imu");
    assert_eq!(loaded.devices.imu_baud, 921_600);
    assert_eq!(loaded.devices.remote_port, "/dev/yunzhuo");
    assert_eq!(loaded.devices.joystick_dev, "/dev/input/js0");
    assert_eq!(loaded.safety.wheel_can_timeout_us, 500_000);
    assert_eq!(loaded.safety.max_proprio_age_us, 200_000);
    assert!((loaded.safety.min_upright_cos - 0.35).abs() < 1e-12);
    assert!((loaded.safety.max_temp_c - 85.0).abs() < 1e-6);
    assert!((loaded.safety.damp_kd_leg - 8.0).abs() < 1e-12);
    assert!((loaded.safety.damp_kd_wheel - 1.0).abs() < 1e-12);
    assert_eq!(loaded.motors.rs04.torque_max, 120.0);
    assert_eq!(loaded.motors.rs02.torque_max, 17.0);
    assert_eq!(loaded.joints.len(), NUM_JOINTS);
    assert_eq!(loaded.joints[0].name, "FR_hip");
    assert_eq!(loaded.joints[15].name, "RL_foot");
    assert_eq!(
        loaded.posture.standing_pose,
        [
            -0.12, -0.93, 1.96, 0.12, 0.93, -1.96, 0.12, 0.93, -1.96, -0.12, -0.93, 1.96, 0.0, 0.0,
            0.0, 0.0,
        ]
    );
    assert_eq!(
        loaded.posture.lower_limits,
        [
            -0.39, -3.12, 0.02, -0.39, 0.02, -3.12, -0.39, 0.02, -3.12, -0.39, -3.12, -3.12, -35.0,
            -35.0, -35.0, -35.0,
        ]
    );
    assert_eq!(
        loaded.posture.upper_limits,
        [
            0.39, -0.02, 3.12, 0.39, 3.12, -0.02, 0.39, 3.12, 3.12, 0.39, -0.02, 3.12, 35.0, 35.0,
            35.0, 35.0,
        ]
    );
}

#[test]
fn thunder_v4_validation_rejects_broken_bijection_duplicate_can_wheel_id_and_bad_limits() {
    // 1. Duplicate joint index (broken bijection 0..16)
    let dup_idx = EMBEDDED_THUNDER_V4_YAML.replacen("index: 1,", "index: 0,", 1);
    assert!(
        RobotHardwareParams::from_yaml_str(&dup_idx).is_err(),
        "duplicate joint index must fail validation"
    );

    // 2. Duplicate (leg, can_id) mapping on FR
    let dup_can = EMBEDDED_THUNDER_V4_YAML.replacen(
        "name: FR_thigh, leg: fr, can_id: 2",
        "name: FR_thigh, leg: fr, can_id: 1",
        1,
    );
    assert!(
        RobotHardwareParams::from_yaml_str(&dup_can).is_err(),
        "duplicate (leg, can_id) must fail validation"
    );

    // 3. Wheel with can_id != 4
    let bad_wheel_id = EMBEDDED_THUNDER_V4_YAML.replacen(
        "name: FR_foot,  leg: fr, can_id: 4, motor: rs02, role: wheel",
        "name: FR_foot,  leg: fr, can_id: 3, motor: rs02, role: wheel",
        1,
    );
    assert!(
        RobotHardwareParams::from_yaml_str(&bad_wheel_id).is_err(),
        "wheel with can_id != 4 must fail validation"
    );

    // 4. Non-positive motor torque limit
    let bad_limit = EMBEDDED_THUNDER_V4_YAML.replacen("torque_max: 120.0", "torque_max: 0.0", 1);
    assert!(
        RobotHardwareParams::from_yaml_str(&bad_limit).is_err(),
        "non-positive motor limit must fail validation"
    );

    // 5. Unsupported IMU baud
    let bad_baud = EMBEDDED_THUNDER_V4_YAML.replacen("imu_baud: 921600", "imu_baud: 9600", 1);
    assert!(
        RobotHardwareParams::from_yaml_str(&bad_baud).is_err(),
        "unsupported imu_baud must fail validation"
    );

    // 6. Duplicate CAN bus interface
    let dup_bus = EMBEDDED_THUNDER_V4_YAML.replacen("fl: can2", "fl: can0", 1);
    assert!(
        RobotHardwareParams::from_yaml_str(&dup_bus).is_err(),
        "duplicate CAN bus must fail validation"
    );

    // 7. Posture lower_limits >= upper_limits
    let mut bad_posture_order = RobotHardwareParams::thunder_v4().clone();
    bad_posture_order.posture.lower_limits[0] = 0.39;
    bad_posture_order.posture.upper_limits[0] = 0.39;
    assert!(
        bad_posture_order.validate().is_err(),
        "posture lower_limits >= upper_limits must fail validation"
    );

    // 8. Posture standing_pose outside leg joint limits
    let mut bad_standing = RobotHardwareParams::thunder_v4().clone();
    bad_standing.posture.standing_pose[0] = 0.50;
    assert!(
        bad_standing.validate().is_err(),
        "leg standing_pose outside [lower_limits, upper_limits] must fail validation"
    );

    // 9. Non-finite posture value
    let mut nan_standing = RobotHardwareParams::thunder_v4().clone();
    nan_standing.posture.standing_pose[2] = f64::NAN;
    assert!(
        nan_standing.validate().is_err(),
        "non-finite posture value must fail validation"
    );
}

#[test]
fn flat_index_bijection_matches_thunder_layout() {
    let params = RobotHardwareParams::thunder_v4();
    for idx in 0..NUM_JOINTS {
        let (leg, can_id) = flat_index_to_leg_can(idx).unwrap();
        assert_eq!(leg_can_to_flat_index(leg, can_id), Some(idx));
        let joint = params.joint(idx).unwrap();
        if idx < WHEEL_START_INDEX {
            assert_ne!(can_id, 4);
            assert_eq!(joint.motor, MotorModel::Rs04);
            assert_ne!(joint.role, MotorRole::Wheel);
        } else {
            assert_eq!(can_id, 4);
            assert_eq!(joint.motor, MotorModel::Rs02);
            assert_eq!(joint.role, MotorRole::Wheel);
        }
    }
    assert_eq!(flat_index_to_leg_can(16), None);
}

#[test]
fn safety_config_and_deploy_bounds_match_thunder_v4_yaml() {
    let params = RobotHardwareParams::thunder_v4();
    let cfg = SafetyConfig::from_hardware_params(params, true);
    assert!(cfg.allow_motor_enable);
    assert_eq!(cfg.min_upright_cos, params.safety.min_upright_cos);
    assert_eq!(cfg.max_proprio_age_us, params.safety.max_proprio_age_us);
    assert_eq!(cfg.max_temp_c, params.safety.max_temp_c);
    assert_eq!(cfg.damp_kd_leg, params.safety.damp_kd_leg);
    assert_eq!(cfg.damp_kd_wheel, params.safety.damp_kd_wheel);
    assert_eq!(cfg.deploy_action_min, *deploy_action_min());
    assert_eq!(cfg.deploy_action_max, *deploy_action_max());
    for i in 0..WHEEL_START_INDEX {
        assert_eq!(cfg.motor_limits[i], params.motors.rs04);
    }
    for i in WHEEL_START_INDEX..NUM_JOINTS {
        assert_eq!(cfg.motor_limits[i], params.motors.rs02);
    }
}

#[test]
fn robostride_can_encoding_is_bit_exact_with_dart_brainstem() {
    // Exact byte vectors dumped directly from Dart `robo_device_proto`
    let cases: &[(&str, RsEvent, u32, &str)] = &[
        (
            "enable_hip1",
            RsEvent::enable(1),
            50396929,
            "0000000000000000",
        ),
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
        (
            "get_vbus_1",
            RsEvent::get_vbus(1),
            285277953,
            "1c70000000000000",
        ),
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

    // Verify RS04 / RS02 feedback decode roundtrip
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

    // Feed in fragmented chunks to verify streaming decoder state machine
    let mut decoder = ProtoframeStreamDecoder::default();
    let mut decoded_imu = None;
    decoder.push_and_decode(&wire[..17], |_| panic!("should wait for full frame"));
    decoder.push_and_decode(&wire[17..], |payload| {
        decoded_imu = Some(decode_hi91_payload(payload, 42_000).unwrap());
    });

    let imu = decoded_imu.expect("Hi91 frame should pass CRC16-CCITT and decode");
    let expected_grav = [0.0, -0.2588190734386444, -0.9659258723258972];
    let expected_gyro = [
        0.17453292519943295,
        -0.3490658503988659,
        0.09599310885968812,
    ];
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
