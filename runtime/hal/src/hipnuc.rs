use anyhow::{bail, Result};
use doso_core::ImuState;

pub const SYNC1: u8 = 0x5A;
pub const SYNC2: u8 = 0xA5;
pub const HEADER_SIZE: usize = 6;
pub const HI91_PAYLOAD_LEN: usize = 76;
pub const STANDARD_GRAVITY_MPS2: f64 = 9.80665;

/// Exact CRC16-CCITT (`poly = 0x1021`, `init = 0x0000`) matching `protoframe/lib/data.dart`.
pub fn crc16_ccitt(mut crc: u16, data: &[u8]) -> u16 {
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

pub fn protoframe_crc(header4: &[u8; 4], payload: &[u8]) -> u16 {
    crc16_ccitt(crc16_ccitt(0, header4), payload)
}

pub fn encode_protoframe(payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u16;
    let header4 = [SYNC1, SYNC2, (len & 0xFF) as u8, (len >> 8) as u8];
    let crc = protoframe_crc(&header4, payload);
    let mut out = Vec::with_capacity(HEADER_SIZE + payload.len());
    out.extend_from_slice(&header4);
    out.push((crc & 0xFF) as u8);
    out.push((crc >> 8) as u8);
    out.extend_from_slice(payload);
    out
}

/// Streaming Protoframe decoder preserving incomplete trailing bytes across serial reads.
#[derive(Default)]
pub struct ProtoframeStreamDecoder {
    buf: Vec<u8>,
}

impl ProtoframeStreamDecoder {
    pub fn push_and_decode<F: FnMut(&[u8])>(&mut self, chunk: &[u8], mut on_payload: F) {
        self.buf.extend_from_slice(chunk);
        let mut start = 0usize;
        while start + HEADER_SIZE <= self.buf.len() {
            if self.buf[start] != SYNC1 {
                start += 1;
                continue;
            }
            if self.buf[start + 1] != SYNC2 {
                start += 1;
                continue;
            }
            let len = u16::from_le_bytes([self.buf[start + 2], self.buf[start + 3]]) as usize;
            if len > 65516 {
                start += 1;
                continue;
            }
            if start + HEADER_SIZE + len > self.buf.len() {
                break;
            }
            let expected_crc = u16::from_le_bytes([self.buf[start + 4], self.buf[start + 5]]);
            let header4 = [
                self.buf[start],
                self.buf[start + 1],
                self.buf[start + 2],
                self.buf[start + 3],
            ];
            let payload = &self.buf[start + HEADER_SIZE..start + HEADER_SIZE + len];
            if protoframe_crc(&header4, payload) == expected_crc {
                on_payload(payload);
                start += HEADER_SIZE + len;
            } else {
                start += 1;
            }
        }
        if start > 0 {
            self.buf.drain(0..start);
        }
    }
}

/// Rotates world `[0, 0, -1]` into body frame via inverse quaternion `(w, -x, -y, -z)`,
/// matching Dart `imu.quaternion.inverted().rotate(Vector3(0, 0, -1))`.
#[inline]
pub fn rotate_unit_neg_z_by_quat(w: f64, x: f64, y: f64, z: f64) -> [f64; 3] {
    let tx = 2.0 * y;
    let ty = -2.0 * x;
    let rx = w * tx + z * ty;
    let ry = w * ty - z * tx;
    let rz = -1.0 - x * ty + y * tx;
    [rx, ry, rz]
}

/// Decodes a 76-byte Hipnuc Hi91 payload into canonical `ImuState` (`rad/s`, `m/s²`, `quat_wxyz`, `projected_gravity`).
pub fn decode_hi91_payload(bytes: &[u8], monotonic_us: u64) -> Result<ImuState> {
    if bytes.len() < HI91_PAYLOAD_LEN {
        bail!("Hi91 payload too short: {} < {}", bytes.len(), HI91_PAYLOAD_LEN);
    }
    let f32_at = |off: usize| -> f64 {
        f32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]) as f64
    };
    let ax = f32_at(12) * STANDARD_GRAVITY_MPS2;
    let ay = f32_at(16) * STANDARD_GRAVITY_MPS2;
    let az = f32_at(20) * STANDARD_GRAVITY_MPS2;

    let deg2rad = std::f64::consts::PI / 180.0;
    let gx = f32_at(24) * deg2rad;
    let gy = f32_at(28) * deg2rad;
    let gz = f32_at(32) * deg2rad;

    let qw = f32_at(60);
    let qx = f32_at(64);
    let qy = f32_at(68);
    let qz = f32_at(72);

    let projected_gravity = rotate_unit_neg_z_by_quat(qw, qx, qy, qz);
    Ok(ImuState {
        gyro_rad_s: [gx, gy, gz],
        accel_m_s2: [ax, ay, az],
        quat_wxyz: [qw, qx, qy, qz],
        projected_gravity,
        timestamp_us: monotonic_us,
        online: true,
    })
}
