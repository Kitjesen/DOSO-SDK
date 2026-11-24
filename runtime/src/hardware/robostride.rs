use anyhow::{bail, Result};

use crate::params::{motor_limits_for_can_id, MotorLimits, MotorModel};

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
#[repr(u8)]
pub enum RsRunMode {
    Control = 0,
    Pp = 1,
    Velocity = 2,
    Torque = 3,
    Csp = 5,
}

impl RsRunMode {
    pub const fn from_u8(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Control),
            1 => Some(Self::Pp),
            2 => Some(Self::Velocity),
            3 => Some(Self::Torque),
            5 => Some(Self::Csp),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Control => "Control(0)",
            Self::Pp => "PP(1)",
            Self::Velocity => "Velocity(2)",
            Self::Torque => "Torque(3)",
            Self::Csp => "CSP(5)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RsBaudRate {
    Rate1M = 1,
    Rate500K = 2,
    Rate250K = 3,
    Rate125K = 4,
}

impl RsBaudRate {
    pub const fn from_u8(raw: u8) -> Option<Self> {
        match raw {
            1 => Some(Self::Rate1M),
            2 => Some(Self::Rate500K),
            3 => Some(Self::Rate250K),
            4 => Some(Self::Rate125K),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RsProtocol {
    Private = 0,
    Canopen = 1,
    Mit = 2,
}

impl RsProtocol {
    pub const fn from_u8(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Private),
            1 => Some(Self::Canopen),
            2 => Some(Self::Mit),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RsParamType {
    U8,
    U16,
    U32,
    F32,
}

/// Typed value carried in RobStride Type-17 (read) and Type-18 (write) parameter frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RsParamValue {
    U8(u8),
    U16(u16),
    U32(u32),
    F32(f32),
}

impl RsParamValue {
    pub const fn as_f32(self) -> Option<f32> {
        match self {
            Self::F32(v) => Some(v),
            _ => None,
        }
    }

    pub const fn as_u8(self) -> Option<u8> {
        match self {
            Self::U8(v) => Some(v),
            _ => None,
        }
    }

    pub const fn as_u16(self) -> Option<u16> {
        match self {
            Self::U16(v) => Some(v),
            _ => None,
        }
    }

    pub const fn as_u32(self) -> Option<u32> {
        match self {
            Self::U32(v) => Some(v),
            _ => None,
        }
    }

    pub fn write_le_bytes(self, dst: &mut [u8; 4]) {
        *dst = [0u8; 4];
        match self {
            Self::U8(v) => dst[0] = v,
            Self::U16(v) => dst[0..2].copy_from_slice(&v.to_le_bytes()),
            Self::U32(v) => dst.copy_from_slice(&v.to_le_bytes()),
            Self::F32(v) => dst.copy_from_slice(&v.to_le_bytes()),
        }
    }

    pub fn format_for_key(self, key: RsKey) -> String {
        match (key, self) {
            (RsKey::RunMode, Self::U8(raw)) => RsRunMode::from_u8(raw)
                .map(|m| m.label().to_string())
                .unwrap_or_else(|| format!("Unknown({raw})")),
            (RsKey::EpscanTime, Self::U16(raw)) => {
                let ms = epscan_raw_to_ms(raw);
                format!("{ms}ms (raw={raw})")
            }
            (RsKey::CanTimeout, Self::U32(raw)) => {
                let us = cantimeout_ticks_to_us(raw);
                format!("{us}us (raw={raw})")
            }
            (RsKey::ZeroSta, Self::U8(v)) => {
                if v != 0 {
                    "1 (-pi..pi)".to_string()
                } else {
                    "0 (0..2pi)".to_string()
                }
            }
            (RsKey::Damper, Self::U8(v)) => {
                if v != 0 {
                    "1 (off-damping disabled)".to_string()
                } else {
                    "0 (off-damping active)".to_string()
                }
            }
            (_, Self::U8(v)) => format!("{v}"),
            (_, Self::U16(v)) => format!("{v}"),
            (_, Self::U32(v)) => format!("{v}"),
            (_, Self::F32(v)) => format!("{v:.4}"),
        }
    }
}

/// Converts raw `EPScan_time` (`0x7026`) register value (`1 -> 10ms`, `+1 -> +5ms`) to milliseconds.
#[inline]
pub const fn epscan_raw_to_ms(raw: u16) -> u32 {
    if raw <= 1 {
        10
    } else {
        10 + 5 * (raw as u32 - 1)
    }
}

/// Converts desired active-report period in milliseconds to raw `EPScan_time` (`0x7026`) register value.
#[inline]
pub fn epscan_ms_to_raw(ms: u32) -> u16 {
    let clamped = ms.max(10);
    let steps = ((clamped - 10) as f64 / 5.0).round() as u32;
    (steps + 1).min(u16::MAX as u32) as u16
}

/// Converts raw `canTimeout` (`0x7028`) ticks (`20000 = 1s`, `1 tick = 50us`) to microseconds.
#[inline]
pub const fn cantimeout_ticks_to_us(ticks: u32) -> u64 {
    (ticks as u64) * 50
}

/// Converts timeout in microseconds to `canTimeout` (`0x7028`) ticks (`50us` per tick).
#[inline]
pub fn cantimeout_us_to_ticks(microseconds: u32) -> u32 {
    ((microseconds as f64) / 50.0).round() as u32
}

/// Complete RS04 / RS02 (2026-10-23 manual) `0x70xx` parameter table (all 29 indices).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum RsKey {
    RunMode = 0x7005,
    IqRef = 0x7006,
    SpdRef = 0x700A,
    LimitTorque = 0x700B,
    CurKp = 0x7010,
    CurKi = 0x7011,
    CurFiltGain = 0x7014,
    LocRef = 0x7016,
    LimitSpd = 0x7017,
    LimitCur = 0x7018,
    MechPos = 0x7019,
    Iqf = 0x701A,
    MechVel = 0x701B,
    Vbus = 0x701C,
    LocKp = 0x701E,
    SpdKp = 0x701F,
    SpdKi = 0x7020,
    SpdFiltGain = 0x7021,
    AccRad = 0x7022,
    VelMax = 0x7024,
    AccSet = 0x7025,
    EpscanTime = 0x7026,
    CanTimeout = 0x7028,
    ZeroSta = 0x7029,
    Damper = 0x702A,
    AddOffset = 0x702B,
    AlveolousOpen = 0x702C,
    IqTest = 0x702D,
    DccSet = 0x702E,
}

impl RsKey {
    pub const ALL: [Self; 29] = [
        Self::RunMode,
        Self::IqRef,
        Self::SpdRef,
        Self::LimitTorque,
        Self::CurKp,
        Self::CurKi,
        Self::CurFiltGain,
        Self::LocRef,
        Self::LimitSpd,
        Self::LimitCur,
        Self::MechPos,
        Self::Iqf,
        Self::MechVel,
        Self::Vbus,
        Self::LocKp,
        Self::SpdKp,
        Self::SpdKi,
        Self::SpdFiltGain,
        Self::AccRad,
        Self::VelMax,
        Self::AccSet,
        Self::EpscanTime,
        Self::CanTimeout,
        Self::ZeroSta,
        Self::Damper,
        Self::AddOffset,
        Self::AlveolousOpen,
        Self::IqTest,
        Self::DccSet,
    ];

    pub const fn from_u16(raw: u16) -> Option<Self> {
        match raw {
            0x7005 => Some(Self::RunMode),
            0x7006 => Some(Self::IqRef),
            0x700A => Some(Self::SpdRef),
            0x700B => Some(Self::LimitTorque),
            0x7010 => Some(Self::CurKp),
            0x7011 => Some(Self::CurKi),
            0x7014 => Some(Self::CurFiltGain),
            0x7016 => Some(Self::LocRef),
            0x7017 => Some(Self::LimitSpd),
            0x7018 => Some(Self::LimitCur),
            0x7019 => Some(Self::MechPos),
            0x701A => Some(Self::Iqf),
            0x701B => Some(Self::MechVel),
            0x701C => Some(Self::Vbus),
            0x701E => Some(Self::LocKp),
            0x701F => Some(Self::SpdKp),
            0x7020 => Some(Self::SpdKi),
            0x7021 => Some(Self::SpdFiltGain),
            0x7022 => Some(Self::AccRad),
            0x7024 => Some(Self::VelMax),
            0x7025 => Some(Self::AccSet),
            0x7026 => Some(Self::EpscanTime),
            0x7028 => Some(Self::CanTimeout),
            0x7029 => Some(Self::ZeroSta),
            0x702A => Some(Self::Damper),
            0x702B => Some(Self::AddOffset),
            0x702C => Some(Self::AlveolousOpen),
            0x702D => Some(Self::IqTest),
            0x702E => Some(Self::DccSet),
            _ => None,
        }
    }

    pub const fn value_type(self) -> RsParamType {
        match self {
            Self::RunMode | Self::ZeroSta | Self::Damper | Self::AlveolousOpen | Self::IqTest => {
                RsParamType::U8
            }
            Self::EpscanTime => RsParamType::U16,
            Self::CanTimeout => RsParamType::U32,
            Self::IqRef
            | Self::SpdRef
            | Self::LimitTorque
            | Self::CurKp
            | Self::CurKi
            | Self::CurFiltGain
            | Self::LocRef
            | Self::LimitSpd
            | Self::LimitCur
            | Self::MechPos
            | Self::Iqf
            | Self::MechVel
            | Self::Vbus
            | Self::LocKp
            | Self::SpdKp
            | Self::SpdKi
            | Self::SpdFiltGain
            | Self::AccRad
            | Self::VelMax
            | Self::AccSet
            | Self::AddOffset
            | Self::DccSet => RsParamType::F32,
        }
    }

    pub const fn is_writable(self) -> bool {
        !matches!(self, Self::MechPos | Self::Iqf | Self::MechVel | Self::Vbus)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::RunMode => "run_mode",
            Self::IqRef => "iq_ref",
            Self::SpdRef => "spd_ref",
            Self::LimitTorque => "limit_torque",
            Self::CurKp => "cur_kp",
            Self::CurKi => "cur_ki",
            Self::CurFiltGain => "cur_filt_gain",
            Self::LocRef => "loc_ref",
            Self::LimitSpd => "limit_spd",
            Self::LimitCur => "limit_cur",
            Self::MechPos => "mech_pos",
            Self::Iqf => "iqf",
            Self::MechVel => "mech_vel",
            Self::Vbus => "vbus",
            Self::LocKp => "loc_kp",
            Self::SpdKp => "spd_kp",
            Self::SpdKi => "spd_ki",
            Self::SpdFiltGain => "spd_filt_gain",
            Self::AccRad => "acc_rad",
            Self::VelMax => "vel_max",
            Self::AccSet => "acc_set",
            Self::EpscanTime => "epscan_time",
            Self::CanTimeout => "cantimeout",
            Self::ZeroSta => "zero_sta",
            Self::Damper => "damper",
            Self::AddOffset => "add_offset",
            Self::AlveolousOpen => "alveolous_open",
            Self::IqTest => "iq_test",
            Self::DccSet => "dcc_set",
        }
    }

    pub fn parse_name_or_hex(raw: &str) -> Option<Self> {
        let norm = raw.trim().to_ascii_lowercase().replace('-', "_");
        if let Some(hex) = norm.strip_prefix("0x") {
            let code = u16::from_str_radix(hex, 16).ok()?;
            return Self::from_u16(code);
        }
        match norm.as_str() {
            "run_mode" | "runmode" => Some(Self::RunMode),
            "iq_ref" | "iqref" => Some(Self::IqRef),
            "spd_ref" | "spdref" => Some(Self::SpdRef),
            "limit_torque" | "limittorque" => Some(Self::LimitTorque),
            "cur_kp" | "curkp" => Some(Self::CurKp),
            "cur_ki" | "curki" => Some(Self::CurKi),
            "cur_filt_gain" | "curfiltgain" => Some(Self::CurFiltGain),
            "loc_ref" | "locref" => Some(Self::LocRef),
            "limit_spd" | "limitspd" => Some(Self::LimitSpd),
            "limit_cur" | "limitcur" => Some(Self::LimitCur),
            "mech_pos" | "mechpos" => Some(Self::MechPos),
            "iqf" => Some(Self::Iqf),
            "mech_vel" | "mechvel" => Some(Self::MechVel),
            "vbus" => Some(Self::Vbus),
            "loc_kp" | "lockp" => Some(Self::LocKp),
            "spd_kp" | "spdkp" => Some(Self::SpdKp),
            "spd_ki" | "spdki" => Some(Self::SpdKi),
            "spd_filt_gain" | "spdfiltgain" => Some(Self::SpdFiltGain),
            "acc_rad" | "accrad" => Some(Self::AccRad),
            "vel_max" | "velmax" => Some(Self::VelMax),
            "acc_set" | "accset" => Some(Self::AccSet),
            "epscan_time" | "epscantime" => Some(Self::EpscanTime),
            "cantimeout" | "can_timeout" => Some(Self::CanTimeout),
            "zero_sta" | "zerosta" => Some(Self::ZeroSta),
            "damper" => Some(Self::Damper),
            "add_offset" | "addoffset" => Some(Self::AddOffset),
            "alveolous_open" | "alveolousopen" => Some(Self::AlveolousOpen),
            "iq_test" | "iqtest" => Some(Self::IqTest),
            "dcc_set" | "dccset" => Some(Self::DccSet),
            _ => None,
        }
    }

    pub fn decode_value(self, bytes4: [u8; 4]) -> RsParamValue {
        match self.value_type() {
            RsParamType::U8 => RsParamValue::U8(bytes4[0]),
            RsParamType::U16 => RsParamValue::U16(u16::from_le_bytes([bytes4[0], bytes4[1]])),
            RsParamType::U32 => RsParamValue::U32(u32::from_le_bytes(bytes4)),
            RsParamType::F32 => RsParamValue::F32(f32::from_le_bytes(bytes4)),
        }
    }

    pub fn parse_value_str(self, raw: &str) -> Result<RsParamValue> {
        let s = raw.trim();
        match self.value_type() {
            RsParamType::U8 => {
                let v: u8 = match s.to_ascii_lowercase().as_str() {
                    "true" | "on" => 1,
                    "false" | "off" => 0,
                    _ => s.parse().map_err(|_| {
                        anyhow::anyhow!("Invalid u8 value '{s}' for {}", self.name())
                    })?,
                };
                Ok(RsParamValue::U8(v))
            }
            RsParamType::U16 => {
                let v: u16 = s
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid u16 value '{s}' for {}", self.name()))?;
                Ok(RsParamValue::U16(v))
            }
            RsParamType::U32 => {
                let v: u32 = s
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid u32 value '{s}' for {}", self.name()))?;
                Ok(RsParamValue::U32(v))
            }
            RsParamType::F32 => {
                let v: f32 = s
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid f32 value '{s}' for {}", self.name()))?;
                if !v.is_finite() {
                    bail!("Non-finite value '{s}' for {}", self.name());
                }
                Ok(RsParamValue::F32(v))
            }
        }
    }
}

/// Complete RobStride private CAN protocol command set (Communication Types 0..26).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RsEvent {
    /// Type 0 (`0x00`): Query device CAN ID + 64-bit MCU unique ID.
    GetDeviceId { can_id: u8, host_id: u8 },
    /// Type 1 (`0x01`): MIT motion-control 5-parameter command (Thunder layout IDs `1..=4`).
    Control {
        can_id: u8,
        torque: f64,
        position: f64,
        velocity: f64,
        kp: f64,
        kd: f64,
    },
    /// Type 3 (`0x03`): Enable motor output (`Motor` mode).
    Enable { can_id: u8, host_id: u8 },
    /// Type 4 (`0x04`): Disable motor output (`Reset` mode), optionally clearing faults (`Byte[0]=1`).
    Disable {
        can_id: u8,
        host_id: u8,
        clear_errors: bool,
    },
    /// Type 5 (`0x05`): Trigger encoder magnetic calibration (`data2=0x00FD`).
    Calibration { can_id: u8 },
    /// Type 6 (`0x06`): Set current mechanical position as zero (`Byte[0]=1`).
    SetZero { can_id: u8 },
    /// Type 7 (`0x07`): Change motor CAN ID (`Bit16..23 = new_id`, `Bit8..15 = host_id`).
    SetId { can_id: u8, new_id: u8, host_id: u8 },
    /// Type 17 (`0x11`): Read single `0x70xx` parameter.
    GetParam { can_id: u8, host_id: u8, key: RsKey },
    /// Type 18 (`0x12`): Write single `0x70xx` parameter (volatile until `SaveData`).
    SetParam {
        can_id: u8,
        host_id: u8,
        key: RsKey,
        value: RsParamValue,
    },
    /// Type 18 (`0x12`) convenience variant for `canTimeout` (`0x7028`) in microseconds.
    SetCanTimeoutUs {
        can_id: u8,
        host_id: u8,
        microseconds: u32,
    },
    /// Type 22 (`0x16`): Save all storable parameters (`0x20xx` / `zero_sta` / etc.) to motor Flash.
    SaveData { can_id: u8, host_id: u8 },
    /// Type 23 (`0x17`): Change CAN baud rate (takes effect after power cycle).
    SetBaudRate {
        can_id: u8,
        host_id: u8,
        baud_rate: RsBaudRate,
    },
    /// Type 24 (`0x18`): Enable/disable periodic active motor feedback reporting.
    SetReporting {
        can_id: u8,
        host_id: u8,
        enable: bool,
    },
    /// Type 25 (`0x19`): Switch motor CAN protocol (`Private` / `Canopen` / `Mit`, effective after reboot).
    SetProtocol {
        can_id: u8,
        host_id: u8,
        protocol: RsProtocol,
    },
    /// Type 26 (`mode=0x04`, `Byte[0..1]=[0x00, 0xC4]`): Read motor firmware version.
    GetVersion { can_id: u8, host_id: u8 },
}

impl RsEvent {
    pub const fn get_device_id(can_id: u8) -> Self {
        Self::GetDeviceId {
            can_id,
            host_id: DEFAULT_HOST_ID,
        }
    }

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

    pub const fn calibration(can_id: u8) -> Self {
        Self::Calibration { can_id }
    }

    pub const fn set_zero(can_id: u8) -> Self {
        Self::SetZero { can_id }
    }

    pub const fn set_id(can_id: u8, new_id: u8) -> Self {
        Self::SetId {
            can_id,
            new_id,
            host_id: DEFAULT_HOST_ID,
        }
    }

    pub const fn get_param(can_id: u8, key: RsKey) -> Self {
        Self::GetParam {
            can_id,
            host_id: DEFAULT_HOST_ID,
            key,
        }
    }

    pub const fn set_param(can_id: u8, key: RsKey, value: RsParamValue) -> Self {
        Self::SetParam {
            can_id,
            host_id: DEFAULT_HOST_ID,
            key,
            value,
        }
    }

    pub const fn set_zero_sta(can_id: u8, signed_minus_pi_to_pi: bool) -> Self {
        Self::SetParam {
            can_id,
            host_id: DEFAULT_HOST_ID,
            key: RsKey::ZeroSta,
            value: RsParamValue::U8(if signed_minus_pi_to_pi { 1 } else { 0 }),
        }
    }

    pub const fn set_run_mode(can_id: u8, mode: RsRunMode) -> Self {
        Self::SetParam {
            can_id,
            host_id: DEFAULT_HOST_ID,
            key: RsKey::RunMode,
            value: RsParamValue::U8(mode as u8),
        }
    }

    pub fn set_epscan_time_ms(can_id: u8, ms: u32) -> Self {
        Self::SetParam {
            can_id,
            host_id: DEFAULT_HOST_ID,
            key: RsKey::EpscanTime,
            value: RsParamValue::U16(epscan_ms_to_raw(ms)),
        }
    }

    pub const fn set_damper(can_id: u8, disable_off_damping: bool) -> Self {
        Self::SetParam {
            can_id,
            host_id: DEFAULT_HOST_ID,
            key: RsKey::Damper,
            value: RsParamValue::U8(if disable_off_damping { 1 } else { 0 }),
        }
    }

    pub const fn set_add_offset(can_id: u8, offset_rad: f32) -> Self {
        Self::SetParam {
            can_id,
            host_id: DEFAULT_HOST_ID,
            key: RsKey::AddOffset,
            value: RsParamValue::F32(offset_rad),
        }
    }

    pub const fn set_alveolous_open(can_id: u8, enable: bool) -> Self {
        Self::SetParam {
            can_id,
            host_id: DEFAULT_HOST_ID,
            key: RsKey::AlveolousOpen,
            value: RsParamValue::U8(if enable { 1 } else { 0 }),
        }
    }

    pub const fn set_iq_test(can_id: u8, enable: bool) -> Self {
        Self::SetParam {
            can_id,
            host_id: DEFAULT_HOST_ID,
            key: RsKey::IqTest,
            value: RsParamValue::U8(if enable { 1 } else { 0 }),
        }
    }

    pub const fn save_data(can_id: u8) -> Self {
        Self::SaveData {
            can_id,
            host_id: DEFAULT_HOST_ID,
        }
    }

    pub const fn set_baud_rate(can_id: u8, baud_rate: RsBaudRate) -> Self {
        Self::SetBaudRate {
            can_id,
            host_id: DEFAULT_HOST_ID,
            baud_rate,
        }
    }

    pub const fn set_reporting(can_id: u8, enable: bool) -> Self {
        Self::SetReporting {
            can_id,
            host_id: DEFAULT_HOST_ID,
            enable,
        }
    }

    pub const fn set_protocol(can_id: u8, protocol: RsProtocol) -> Self {
        Self::SetProtocol {
            can_id,
            host_id: DEFAULT_HOST_ID,
            protocol,
        }
    }

    pub const fn get_version(can_id: u8) -> Self {
        Self::GetVersion {
            can_id,
            host_id: DEFAULT_HOST_ID,
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
        self.encode_with_limits(None)
    }

    /// Encodes `self` using an optional explicit `MotorLimits` override (useful when controlling
    /// a benchtop motor at non-Thunder CAN IDs such as factory default `127`).
    pub fn encode_with_limits(&self, control_limits: Option<MotorLimits>) -> Result<RawCanFrame> {
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
                let limits = match control_limits {
                    Some(l) => l,
                    None => limits_for_can_id(can_id)?,
                };
                return Ok(Self::encode_control_with_limits(
                    can_id, torque, position, velocity, kp, kd, limits,
                ));
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
            Self::Calibration { can_id } => RsDataFrame {
                mode: 0x05,
                data2: 0x00FD,
                can_id,
                data1: [0u8; 8],
            },
            Self::SetZero { can_id } => RsDataFrame {
                mode: 0x06,
                data2: DEFAULT_HOST_ID as u16,
                can_id,
                data1: [1, 0, 0, 0, 0, 0, 0, 0],
            },
            Self::SetId {
                can_id,
                new_id,
                host_id,
            } => RsDataFrame {
                mode: 0x07,
                data2: (host_id as u16) | ((new_id as u16) << 8),
                can_id,
                data1: [0u8; 8],
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
            Self::SetParam {
                can_id,
                host_id,
                key,
                value,
            } => {
                if !key.is_writable() {
                    bail!(
                        "RobStride parameter {} (0x{:04x}) is read-only",
                        key.name(),
                        key as u16
                    );
                }
                let expected = key.value_type();
                let matches_type = matches!(
                    (expected, value),
                    (RsParamType::U8, RsParamValue::U8(_))
                        | (RsParamType::U16, RsParamValue::U16(_))
                        | (RsParamType::U32, RsParamValue::U32(_))
                        | (RsParamType::F32, RsParamValue::F32(_))
                );
                if !matches_type {
                    bail!(
                        "RobStride parameter {} expects {:?}, got {:?}",
                        key.name(),
                        expected,
                        value
                    );
                }
                let mut data1 = [0u8; 8];
                data1[0..2].copy_from_slice(&(key as u16).to_le_bytes());
                let mut val_bytes = [0u8; 4];
                value.write_le_bytes(&mut val_bytes);
                data1[4..8].copy_from_slice(&val_bytes);
                RsDataFrame {
                    mode: 0x12,
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
                let ticks = cantimeout_us_to_ticks(microseconds);
                data1[4..8].copy_from_slice(&ticks.to_le_bytes());
                RsDataFrame {
                    mode: 0x12,
                    data2: host_id as u16,
                    can_id,
                    data1,
                }
            }
            Self::SaveData { can_id, host_id } => RsDataFrame {
                mode: 0x16,
                data2: host_id as u16,
                can_id,
                data1: [1, 2, 3, 4, 5, 6, 7, 8],
            },
            Self::SetBaudRate {
                can_id,
                host_id,
                baud_rate,
            } => RsDataFrame {
                mode: 0x17,
                data2: host_id as u16,
                can_id,
                data1: [1, 2, 3, 4, 5, 6, baud_rate as u8, 0],
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
            Self::SetProtocol {
                can_id,
                host_id,
                protocol,
            } => RsDataFrame {
                mode: 0x19,
                data2: host_id as u16,
                can_id,
                data1: [1, 2, 3, 4, 5, 6, protocol as u8, 0],
            },
            Self::GetVersion { can_id, host_id } => RsDataFrame {
                mode: 0x04,
                data2: host_id as u16,
                can_id,
                data1: [0x00, 0xC4, 0, 0, 0, 0, 0, 0],
            },
        };
        Ok(df.to_can_frame())
    }

    pub fn encode_control_with_limits(
        can_id: u8,
        torque: f64,
        position: f64,
        velocity: f64,
        kp: f64,
        kd: f64,
        limits: MotorLimits,
    ) -> RawCanFrame {
        let tau_u16 = float_to_u16(torque, -limits.torque_max, limits.torque_max);
        let pos_u16 = float_to_u16(position, -limits.position_max, limits.position_max);
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
        .to_can_frame()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RsDeviceId {
    pub can_id: u8,
    pub mcu_id: [u8; 8],
}

impl RsDeviceId {
    /// Compact 16-char uppercase hex MCU identifier.
    pub fn mcu_hex(&self) -> String {
        let mut out = String::with_capacity(16);
        for b in self.mcu_id {
            use std::fmt::Write;
            let _ = write!(&mut out, "{b:02X}");
        }
        out
    }

    /// Exact Dart `robo_device_proto` decimal string (`BigInt.parse(bytes.map(dec_pad2).join())`).
    pub fn mcu_dart_decimal(&self) -> String {
        let raw: String = self.mcu_id.iter().map(|b| format!("{b:02}")).collect();
        let trimmed = raw.trim_start_matches('0');
        if trimmed.is_empty() {
            "0".to_string()
        } else {
            trimmed.to_string()
        }
    }

    pub fn matches_expected(&self, expected: &str) -> bool {
        let norm = expected
            .trim()
            .trim_start_matches("0x")
            .trim_start_matches("0X");
        self.mcu_hex().eq_ignore_ascii_case(norm) || self.mcu_dart_decimal() == expected.trim()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RsFirmwareVersion {
    pub host_id: u8,
    pub can_id: u8,
    pub status_code: u8,
    pub error_bits: u8,
    pub version: [u8; 4],
}

impl RsFirmwareVersion {
    pub fn display_string(&self) -> String {
        format!(
            "{}.{}.{}.{}",
            self.version[0], self.version[1], self.version[2], self.version[3]
        )
    }

    /// Checks whether the firmware supports Type-22 `SaveData`, Type-24 `SetReporting`,
    /// and `±12.57` position range (`RS04 >= 0.4.1.0`, `RS02 >= 0.2.3.0`).
    pub fn supports_modern_protocol(&self, model: MotorModel) -> bool {
        let min = match model {
            MotorModel::Rs04 => [0, 4, 1, 0],
            MotorModel::Rs02 => [0, 2, 3, 0],
        };
        self.version >= min
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RsDecodedState {
    DeviceId(RsDeviceId),
    FirmwareVersion(RsFirmwareVersion),
    Motion(RsMotionFeedback),
    Param {
        host_id: u8,
        can_id: u8,
        key: RsKey,
        value: RsParamValue,
    },
    FaultFrame {
        host_id: u8,
        can_id: u8,
        fault_bits: u32,
        warning_bits: u32,
    },
}

pub fn decode_rs_frame_with_limits<F>(
    raw: RawCanFrame,
    mut lookup_limits: F,
) -> Result<RsDecodedState>
where
    F: FnMut(u8) -> Result<MotorLimits>,
{
    let df = RsDataFrame::from_can_frame(raw);
    match df.mode {
        0x00 => Ok(RsDecodedState::DeviceId(RsDeviceId {
            can_id: (df.data2 & 0xFF) as u8,
            mcu_id: df.data1,
        })),
        0x01 | 0x02 | 0x18 => {
            let can_id = (df.data2 & 0xFF) as u8;
            let status_code = ((df.data2 >> 14) & 0x03) as u8;
            let error_bits = ((df.data2 >> 8) & 0x3F) as u8;
            // Communication Type 26 firmware-version response uses mode 0x02 with magic header
            // [0x00, 0xC4, 0x56]. Check before motion decoding so version replies never corrupt joint angles.
            if df.mode == 0x02 && df.data1[0] == 0x00 && df.data1[1] == 0xC4 && df.data1[2] == 0x56
            {
                return Ok(RsDecodedState::FirmwareVersion(RsFirmwareVersion {
                    host_id: df.can_id,
                    can_id,
                    status_code,
                    error_bits,
                    version: [df.data1[3], df.data1[4], df.data1[5], df.data1[6]],
                }));
            }
            let limits = lookup_limits(can_id)?;
            let pos_u16 = u16::from_be_bytes([df.data1[0], df.data1[1]]);
            let vel_u16 = u16::from_be_bytes([df.data1[2], df.data1[3]]);
            let tau_u16 = u16::from_be_bytes([df.data1[4], df.data1[5]]);
            let temp_u16 = u16::from_be_bytes([df.data1[6], df.data1[7]]);
            Ok(RsDecodedState::Motion(RsMotionFeedback {
                is_report: df.mode == 0x18,
                host_id: df.can_id,
                can_id,
                status_code,
                error_bits,
                position: u16_to_float(pos_u16, -limits.position_max, limits.position_max),
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
            let value = key.decode_value([df.data1[4], df.data1[5], df.data1[6], df.data1[7]]);
            Ok(RsDecodedState::Param {
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
            let warning_bits =
                u32::from_le_bytes([df.data1[4], df.data1[5], df.data1[6], df.data1[7]]);
            Ok(RsDecodedState::FaultFrame {
                host_id: df.can_id,
                can_id,
                fault_bits,
                warning_bits,
            })
        }
        other => bail!("Unsupported RobStride frame mode 0x{other:02x}"),
    }
}

pub fn decode_rs_frame(raw: RawCanFrame) -> Result<RsDecodedState> {
    decode_rs_frame_with_limits(raw, limits_for_can_id)
}

fn limits_for_can_id(can_id: u8) -> Result<MotorLimits> {
    motor_limits_for_can_id(can_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid Thunder CAN ID {can_id}; expected 1..=4"))
}

/// Decodes the 6-bit compressed fault field (`bit16..21` of Type-2 / Type-24 `data2`) into named conditions.
pub fn decode_compressed_error_names(error_bits: u8) -> Vec<&'static str> {
    let mut out = Vec::new();
    if (error_bits & (1 << 0)) != 0 {
        out.push("underVoltage");
    }
    if (error_bits & (1 << 1)) != 0 {
        out.push("driverFault");
    }
    if (error_bits & (1 << 2)) != 0 {
        out.push("overTemperature");
    }
    if (error_bits & (1 << 3)) != 0 {
        out.push("magneticEncoderFault");
    }
    if (error_bits & (1 << 4)) != 0 {
        out.push("stallOverload");
    }
    if (error_bits & (1 << 5)) != 0 {
        out.push("uncalibrated");
    }
    out
}

/// Decodes the full 32-bit Type-21 (`0x15`) `Byte0..3` fault word into named manual fault conditions.
pub fn decode_fault_bits_32_names(fault_bits: u32) -> Vec<&'static str> {
    const TABLE: &[(u32, &str)] = &[
        (0, "overTemperature"),
        (1, "driverFault"),
        (2, "underVoltage"),
        (3, "overVoltage"),
        (4, "phaseBOverCurrent"),
        (5, "phaseCOverCurrent"),
        (7, "uncalibrated"),
        (8, "hardwareIdFault"),
        (9, "posInitFault"),
        (14, "stallOverload"),
        (16, "phaseAOverCurrent"),
    ];
    let mut out = Vec::new();
    for &(bit, name) in TABLE {
        if (fault_bits & (1u32 << bit)) != 0 {
            out.push(name);
        }
    }
    out
}

/// Decodes the 32-bit Type-21 (`0x15`) `Byte4..7` warning word.
pub fn decode_warning_bits_32_names(warning_bits: u32) -> Vec<&'static str> {
    let mut out = Vec::new();
    if (warning_bits & 1) != 0 {
        out.push("overTemperatureWarning");
    }
    out
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
