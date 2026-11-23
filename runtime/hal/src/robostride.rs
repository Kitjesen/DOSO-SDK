use anyhow::{bail, Result};
use doso_core::kinematics::{thunder_motor_model_for_can_id, MotorProtocolLimits};

pub const DEFAULT_HOST_ID: u8 = 0xFF;
pub const POSITION_MAX: f64 = 12.57;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawCanFrame {
    /// 29-bit extended CAN identifier (`0..=0x1FFF_FFFF`).
    pub id: u32,
    pub data: [u8; 8],
}

impl RawCanFrame {
    pub fn to_hex_compact(&self) -> String {
        format!(
            "{:08x}08{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.id,
            self.data[0],
            self.data[1],
            self.data[2],
            self.data[3],
            self.data[4],
            self.data[5],
            self.data[6],
            self.data[7]
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RsDataFrame {
    pub mode: u8,
    pub data2: u16,
    pub can_id: u8,
    pub data1: [u8; 8],
}

impl RsDataFrame {
    pub const fn to_can_frame(self) -> RawCanFrame {
        let id = ((self.mode as u32) << 24) | ((self.data2 as u32) << 8) | (self.can_id as u32);
        RawCanFrame {
            id,
            data: self.data1,
        }
    }

    pub const fn from_can_frame(frame: RawCanFrame) -> Self {
        Self {
            mode: ((frame.id >> 24) & 0x1F) as u8,
            data2: ((frame.id >> 8) & 0xFFFF) as u16,
            can_id: (frame.id & 0xFF) as u8,
            data1: frame.data,
        }
    }
}

/// Exact match to `floatToUint16` in `robo_device_proto/lib/src/robostride/internal.dart`.
#[inline]
pub fn float_to_u16(value: f64, min: f64, max: f64) -> u16 {
    let clamped = value.clamp(min, max);
    let scaled = ((clamped - min) / (max - min)) * 65535.0;
    (scaled.round() as i64).clamp(0, 65535) as u16
}

/// Exact match to `uint16ToFloat` in `robo_device_proto/lib/src/robostride/internal.dart`.
#[inline]
pub fn u16_to_float(value: u16, min: f64, max: f64) -> f64 {
    min + (value as f64 / 65535.0) * (max - min)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum RsKey {
    RunMode = 0x7005,
    IqRef = 0x7006,
    SpdRef = 0x700A,
    LimitTorque = 0x700B,
    LimitCur = 0x7018,
    MechPos = 0x7019,
    Iqf = 0x701A,
    MechVel = 0x701B,
    Vbus = 0x701C,
    CanTimeout = 0x7028,
}

impl RsKey {
    pub const fn from_u16(raw: u16) -> Option<Self> {
        match raw {
            0x7005 => Some(Self::RunMode),
            0x7006 => Some(Self::IqRef),
            0x700A => Some(Self::SpdRef),
            0x700B => Some(Self::LimitTorque),
            0x7018 => Some(Self::LimitCur),
            0x7019 => Some(Self::MechPos),
            0x701A => Some(Self::Iqf),
            0x701B => Some(Self::MechVel),
            0x701C => Some(Self::Vbus),
            0x7028 => Some(Self::CanTimeout),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RsEvent {
    GetDeviceId {
        can_id: u8,
        host_id: u8,
    },
    Control {
        can_id: u8,
        torque: f64,
        position: f64,
        velocity: f64,
        kp: f64,
        kd: f64,
    },
    Enable {
        can_id: u8,
        host_id: u8,
    },
    Disable {
        can_id: u8,
        host_id: u8,
        clear_errors: bool,
    },
    SetZero {
        can_id: u8,
    },
    SetReporting {
        can_id: u8,
        host_id: u8,
        enable: bool,
    },
    GetParam {
        can_id: u8,
        host_id: u8,
        key: RsKey,
    },
    SetCanTimeoutUs {
        can_id: u8,
        host_id: u8,
        microseconds: u32,
    },
}

impl RsEvent {
    pub const fn enable(can_id: u8) -> Self {
        Self::Enable {
            can_id,
            host_id: DEFAULT_HOST_ID,
        }
    }

    pub const fn disable(can_id: u8, clear_errors: bool) -> Self {
        Self::Disable {
            can_id,
            host_id: DEFAULT_HOST_ID,
            clear_errors,
        }
    }

    pub const fn set_zero(can_id: u8) -> Self {
        Self::SetZero { can_id }
    }

    pub const fn set_reporting(can_id: u8, enable: bool) -> Self {
        Self::SetReporting {
            can_id,
            host_id: DEFAULT_HOST_ID,
            enable,
        }
    }

    pub const fn get_vbus(can_id: u8) -> Self {
        Self::GetParam {
            can_id,
            host_id: DEFAULT_HOST_ID,
            key: RsKey::Vbus,
        }
    }

    pub const fn set_cantimeout_us(can_id: u8, microseconds: u32) -> Self {
        Self::SetCanTimeoutUs {
            can_id,
            host_id: DEFAULT_HOST_ID,
            microseconds,
        }
    }

    pub fn encode(&self) -> Result<RawCanFrame> {
        let df = match *self {
            Self::GetDeviceId { can_id, host_id } => RsDataFrame {
                mode: 0x00,
                data2: host_id as u16,
                can_id,
                data1: [0u8; 8],
            },
            Self::Control {
                can_id,
                torque,
                position,
                velocity,
                kp,
                kd,
            } => {
                let limits = limits_for_can_id(can_id)?;
                let tau_u16 = float_to_u16(torque, -limits.torque_max, limits.torque_max);
                let pos_u16 = float_to_u16(position, -POSITION_MAX, POSITION_MAX);
                let vel_u16 = float_to_u16(velocity, -limits.velocity_max, limits.velocity_max);
                let kp_u16 = float_to_u16(kp, 0.0, limits.kp_max);
                let kd_u16 = float_to_u16(kd, 0.0, limits.kd_max);
                // Note: Dart ByteData.setUint16 defaults to big-endian!
                let mut data1 = [0u8; 8];
                data1[0..2].copy_from_slice(&pos_u16.to_be_bytes());
                data1[2..4].copy_from_slice(&vel_u16.to_be_bytes());
                data1[4..6].copy_from_slice(&kp_u16.to_be_bytes());
                data1[6..8].copy_from_slice(&kd_u16.to_be_bytes());
                RsDataFrame {
                    mode: 0x01,
                    data2: tau_u16,
                    can_id,
                    data1,
                }
            }
            Self::Enable { can_id, host_id } => RsDataFrame {
                mode: 0x03,
                data2: host_id as u16,
                can_id,
                data1: [0u8; 8],
            },
            Self::Disable {
                can_id,
                host_id,
                clear_errors,
            } => RsDataFrame {
                mode: 0x04,
                data2: host_id as u16,
                can_id,
                data1: [u8::from(clear_errors), 0, 0, 0, 0, 0, 0, 0],
            },
            Self::SetZero { can_id } => RsDataFrame {
                mode: 0x06,
                data2: DEFAULT_HOST_ID as u16,
                can_id,
                data1: [1, 0, 0, 0, 0, 0, 0, 0],
            },
            Self::SetReporting {
                can_id,
                host_id,
                enable,
            } => RsDataFrame {
                mode: 0x18,
                data2: host_id as u16,
                can_id,
                data1: [1, 2, 3, 4, 5, 6, u8::from(enable), 0],
            },
            Self::GetParam {
                can_id,
                host_id,
                key,
            } => {
                // Parameter keys in RobStride Type-17/18 are little-endian u16 at offset 0.
                let mut data1 = [0u8; 8];
                data1[0..2].copy_from_slice(&(key as u16).to_le_bytes());
                RsDataFrame {
                    mode: 0x11,
                    data2: host_id as u16,
                    can_id,
                    data1,
                }
            }
            Self::SetCanTimeoutUs {
                can_id,
                host_id,
                microseconds,
            } => {
                let mut data1 = [0u8; 8];
                data1[0..2].copy_from_slice(&(RsKey::CanTimeout as u16).to_le_bytes());
                let ticks = ((microseconds as f64) / 50.0).round() as u32;
                data1[4..8].copy_from_slice(&ticks.to_le_bytes());
                RsDataFrame {
                    mode: 0x12,
                    data2: host_id as u16,
                    can_id,
                    data1,
                }
            }
        };
        Ok(df.to_can_frame())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RsMotionFeedback {
    pub is_report: bool,
    pub host_id: u8,
    pub can_id: u8,
    pub status_code: u8,
    pub error_bits: u8,
    pub position: f64,
    pub velocity: f64,
    pub torque: f64,
    pub temperature_c: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RsDecodedState {
    Motion(RsMotionFeedback),
    ParamFloat {
        host_id: u8,
        can_id: u8,
        key: RsKey,
        value: f32,
    },
    FaultFrame {
        host_id: u8,
        can_id: u8,
        fault_bits: u32,
    },
}

pub fn decode_rs_frame(raw: RawCanFrame) -> Result<RsDecodedState> {
    let df = RsDataFrame::from_can_frame(raw);
    match df.mode {
        0x01 | 0x02 | 0x18 => {
            let can_id = (df.data2 & 0xFF) as u8;
            let limits = limits_for_can_id(can_id)?;
            let pos_u16 = u16::from_be_bytes([df.data1[0], df.data1[1]]);
            let vel_u16 = u16::from_be_bytes([df.data1[2], df.data1[3]]);
            let tau_u16 = u16::from_be_bytes([df.data1[4], df.data1[5]]);
            let temp_u16 = u16::from_be_bytes([df.data1[6], df.data1[7]]);
            Ok(RsDecodedState::Motion(RsMotionFeedback {
                is_report: df.mode == 0x18,
                host_id: df.can_id,
                can_id,
                status_code: ((df.data2 >> 14) & 0x03) as u8,
                error_bits: ((df.data2 >> 8) & 0x3F) as u8,
                position: u16_to_float(pos_u16, -POSITION_MAX, POSITION_MAX),
                velocity: u16_to_float(vel_u16, -limits.velocity_max, limits.velocity_max),
                torque: u16_to_float(tau_u16, -limits.torque_max, limits.torque_max),
                temperature_c: (temp_u16 as f32) / 10.0,
            }))
        }
        0x11 => {
            let can_id = (df.data2 & 0xFF) as u8;
            let status_hi = ((df.data2 >> 8) & 0xFF) as u8;
            if status_hi != 0 {
                bail!("RobStride getter error status=0x{status_hi:02x}");
            }
            let key_raw = u16::from_le_bytes([df.data1[0], df.data1[1]]);
            let Some(key) = RsKey::from_u16(key_raw) else {
                bail!("Unknown RobStride getter key 0x{key_raw:04x}");
            };
            let value = f32::from_le_bytes([df.data1[4], df.data1[5], df.data1[6], df.data1[7]]);
            Ok(RsDecodedState::ParamFloat {
                host_id: df.can_id,
                can_id,
                key,
                value,
            })
        }
        0x15 => {
            let can_id = (df.data2 & 0xFF) as u8;
            let fault_bits =
                u32::from_le_bytes([df.data1[0], df.data1[1], df.data1[2], df.data1[3]]);
            Ok(RsDecodedState::FaultFrame {
                host_id: df.can_id,
                can_id,
                fault_bits,
            })
        }
        other => bail!("Unsupported RobStride frame mode 0x{other:02x}"),
    }
}

fn limits_for_can_id(can_id: u8) -> Result<MotorProtocolLimits> {
    thunder_motor_model_for_can_id(can_id)
        .map(|m| m.limits())
        .ok_or_else(|| anyhow::anyhow!("Invalid Thunder CAN ID {can_id}; expected 1..=4"))
}

/// Folds a 32-bit RobStride Type-21 (`0x15` / `0x3022`) fault word into a non-zero `u8`
/// fault summary while preserving 8-bit fault codes verbatim and never truncating upper-byte
/// faults (such as `stallOverload` bit 14 or `phaseAOverCurrent` bit 16) to zero.
#[inline]
pub const fn fault_bits_to_error_u8(fault_bits: u32) -> u8 {
    if fault_bits == 0 {
        return 0;
    }
    let folded = (fault_bits as u8)
        | ((fault_bits >> 8) as u8)
        | ((fault_bits >> 16) as u8)
        | ((fault_bits >> 24) as u8);
    if folded != 0 {
        folded
    } else {
        0x80
    }
}
