use doso_core::RemotePadState;

pub const SUBS_SYNC: u8 = 0x0F;
pub const SUBS_FRAME_SIZE: usize = 35;
pub const STANDARD_SBUS_FRAME_SIZE: usize = 25;

pub const SBUS_HIGH: u16 = 1722;
pub const SBUS_CENTER: u16 = 1002;
pub const SBUS_LOW: u16 = 282;

pub fn decode_yunzhuo_channels(channels: &[u16; 16], flags: u8, now_us: u64) -> RemotePadState {
    let healthy = (flags & 0x0C) == 0;
    RemotePadState {
        active: healthy,
        lx: (channels[3] as f64 - SBUS_CENTER as f64) / 720.0,
        ly: (channels[2] as f64 - SBUS_CENTER as f64) / 720.0,
        rx: (channels[0] as f64 - SBUS_CENTER as f64) / 720.0,
        ry: -(channels[1] as f64 - SBUS_CENTER as f64) / 720.0,
        l1: channels[10] == SBUS_HIGH,
        l2: channels[6] == SBUS_HIGH,
        r1: channels[14] == SBUS_HIGH,
        r2: channels[15] == SBUS_HIGH,
        damp_btn: channels[12] == SBUS_HIGH,
        timestamp_us: now_us,
    }
}

/// Streaming decoder for 35-byte XOR-checked YunZhuo SUBS frames (`robo_device_proto/src/subs/state.dart`).
#[derive(Default)]
pub struct SubsStreamDecoder {
    buf: Vec<u8>,
}

impl SubsStreamDecoder {
    pub fn push_and_decode<F: FnMut([u16; 16], u8)>(&mut self, chunk: &[u8], mut on_frame: F) {
        self.buf.extend_from_slice(chunk);
        let mut start = 0usize;
        while start + SUBS_FRAME_SIZE <= self.buf.len() {
            if self.buf[start] != SUBS_SYNC {
                start += 1;
                continue;
            }
            let rest = &self.buf[start + 1..start + SUBS_FRAME_SIZE];
            let xor_expected = rest[33];
            let xor_actual = rest[0..33].iter().fold(0u8, |acc, &b| acc ^ b);
            if xor_actual == xor_expected {
                let mut channels = [0u16; 16];
                for (i, ch) in channels.iter_mut().enumerate() {
                    *ch = u16::from_be_bytes([rest[i * 2], rest[i * 2 + 1]]);
                }
                on_frame(channels, rest[32]);
                start += SUBS_FRAME_SIZE;
            } else {
                start += 1;
            }
        }
        if start > 0 {
            self.buf.drain(0..start);
        }
    }
}

/// Decodes standard 25-byte Futaba SBUS frame (`0x0F` + 22 packed bytes + flags + end).
pub fn decode_standard_sbus_frame(frame: &[u8; STANDARD_SBUS_FRAME_SIZE]) -> Option<([u16; 16], u8)> {
    if frame[0] != SUBS_SYNC {
        return None;
    }
    let end = frame[24];
    if end != 0x00 && (end & 0x0F) != 0x04 {
        return None;
    }
    let data = &frame[1..23];
    let mut channels = [0u16; 16];
    for (ch_idx, out) in channels.iter_mut().enumerate() {
        let bit_offset = ch_idx * 11;
        let byte_offset = bit_offset >> 3;
        let shift = bit_offset & 7;
        let mut packed = (data[byte_offset] as u32) | ((data[byte_offset + 1] as u32) << 8);
        if byte_offset + 2 < data.len() {
            packed |= (data[byte_offset + 2] as u32) << 16;
        }
        *out = ((packed >> shift) & 0x07FF) as u16;
    }
    Some((channels, frame[23]))
}
