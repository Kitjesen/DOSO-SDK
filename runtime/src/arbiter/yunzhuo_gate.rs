use crate::hardware::yunzhuo::{SBUS_CENTER, SBUS_HIGH, SBUS_LOW};
use crate::types::RemotePadState;

pub const YUNZHUO_DEBOUNCE_US: u64 = 100_000;
pub const YUNZHUO_NEUTRAL_GATE_US: u64 = 300_000;
pub const YUNZHUO_DOG_RAMP_DOWN_US: u64 = 400_000;
pub const YUNZHUO_DOG_RAMP_UP_US: u64 = 200_000;
pub const YUNZHUO_WATCHDOG_TIMEOUT_US: u64 = 150_000;
pub const YUNZHUO_STICK_CENTER_DEADZONE: f64 = 0.10;
pub const YUNZHUO_DIRECTION_ARM_DEADZONE: f64 = 0.02;

pub const JS_EVENT_BUTTON: u8 = 0x01;
pub const JS_EVENT_AXIS: u8 = 0x02;
pub const JS_EVENT_INIT: u8 = 0x80;

/// Three-position `CH5` (`G/S`) selector switch on YunZhuo transmitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GSState {
    Up,
    Middle,
    Down,
}

impl GSState {
    pub const fn to_sbus_raw(self) -> u16 {
        match self {
            Self::Up => SBUS_HIGH,
            Self::Middle => SBUS_CENTER,
            Self::Down => SBUS_LOW,
        }
    }

    pub const fn from_sbus_raw(raw: u16) -> Self {
        if raw >= 1362 {
            Self::Up
        } else if raw <= 642 {
            Self::Down
        } else {
            Self::Middle
        }
    }
}

/// Active control domain gated by [`YunZhuoModeStateMachine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum YunZhuoControlMode {
    Dog,
    #[default]
    Neutral,
    Arm,
}

/// Decoded 16-channel YunZhuo frame matching `robo_device_proto/lib/src/yun_zhuo/state.dart`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct YunZhuoState {
    pub healthy: bool,
    pub l1: bool,
    pub l2: bool,
    pub r1: bool,
    pub r2: bool,
    pub left_stick: [f64; 2],
    pub right_stick: [f64; 2],
    pub h: bool,
    pub g_s: GSState,
    pub red: bool,
    pub lt: bool,
    pub rt: bool,
    pub knob: f64,
    pub raw_channels: [u16; 16],
}

impl Default for YunZhuoState {
    fn default() -> Self {
        let mut raw_channels = [SBUS_LOW; 16];
        raw_channels[0] = SBUS_CENTER;
        raw_channels[1] = SBUS_CENTER;
        raw_channels[2] = SBUS_CENTER;
        raw_channels[3] = SBUS_CENTER;
        raw_channels[4] = SBUS_CENTER;
        raw_channels[13] = SBUS_CENTER;
        Self {
            healthy: true,
            l1: false,
            l2: false,
            r1: false,
            r2: false,
            left_stick: [0.0, 0.0],
            right_stick: [0.0, 0.0],
            h: false,
            g_s: GSState::Middle,
            red: false,
            lt: false,
            rt: false,
            knob: 0.0,
            raw_channels,
        }
    }
}

impl YunZhuoState {
    pub fn from_channels(channels: &[u16; 16], flags: u8) -> Self {
        let healthy = (flags & 0x0C) == 0;
        Self {
            healthy,
            l1: channels[10] == SBUS_HIGH,
            l2: channels[6] == SBUS_HIGH,
            r1: channels[14] == SBUS_HIGH,
            r2: channels[15] == SBUS_HIGH,
            left_stick: [
                (channels[3] as f64 - SBUS_CENTER as f64) / 720.0,
                (channels[2] as f64 - SBUS_CENTER as f64) / 720.0,
            ],
            right_stick: [
                (channels[0] as f64 - SBUS_CENTER as f64) / 720.0,
                -(channels[1] as f64 - SBUS_CENTER as f64) / 720.0,
            ],
            h: channels[5] == SBUS_HIGH,
            g_s: GSState::from_sbus_raw(channels[4]),
            red: channels[12] == SBUS_HIGH,
            lt: channels[7] == SBUS_HIGH,
            rt: channels[11] == SBUS_HIGH,
            knob: (channels[13] as f64 - SBUS_CENTER as f64) / 720.0,
            raw_channels: *channels,
        }
    }

    #[inline]
    pub fn sticks_centered(&self, deadzone: f64) -> bool {
        self.left_stick[0].abs() < deadzone
            && self.left_stick[1].abs() < deadzone
            && self.right_stick[0].abs() < deadzone
            && self.right_stick[1].abs() < deadzone
    }

    #[inline]
    pub fn speed_scale(&self) -> f64 {
        if self.lt {
            0.5
        } else if self.rt {
            1.5
        } else {
            1.0
        }
    }
}

/// Snapshot produced by [`YunZhuoModeStateMachine::update`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct YunZhuoModeSnapshot {
    pub mode: YunZhuoControlMode,
    pub dog_drive_scale: f64,
    pub speed_scale: f64,
    pub neutral_gate_ready: bool,
    pub mode_changed: bool,
    pub direction_armed: bool,
    pub standup_triggered: bool,
    pub sitdown_triggered: bool,
    pub damp_triggered: bool,
    pub pad: RemotePadState,
}

/// Latched toggle-switch edge detector matching `enabledButtonChanges` in `real_controller.dart`:
/// records baseline when `H` becomes active without firing, then fires on any subsequent edge.
#[derive(Debug, Clone, Copy, Default)]
struct EnabledSwitchEdge {
    baseline: Option<bool>,
}

impl EnabledSwitchEdge {
    fn reset(&mut self) {
        self.baseline = None;
    }

    fn update(&mut self, h_enabled: bool, current: bool) -> bool {
        if !h_enabled {
            self.baseline = None;
            return false;
        }
        let prev = self.baseline;
        self.baseline = Some(current);
        matches!(prev, Some(p) if p != current)
    }
}

/// Portable YunZhuo `CH5/G_S` mode gate, ramp generator, deadzone armer, and button edge detector
/// matching `yunzhuo_control_mode.dart` + `real_controller.dart`.
pub struct YunZhuoModeStateMachine {
    pub debounce_us: u64,
    pub neutral_gate_us: u64,
    pub dog_ramp_down_us: u64,
    pub dog_ramp_up_us: u64,
    pub watchdog_timeout_us: u64,
    pub stick_deadzone: f64,
    pub arm_deadzone: f64,

    arm_admitted: bool,
    mode: YunZhuoControlMode,
    mode_before_neutral: Option<YunZhuoControlMode>,
    candidate: Option<GSState>,
    candidate_since_us: Option<u64>,
    neutral_centered_since_us: Option<u64>,
    neutral_gate_ready: bool,
    last_update_us: Option<u64>,

    ramp_start: f64,
    ramp_target: f64,
    ramp_started_us: Option<u64>,
    ramp_duration_us: u64,

    direction_armed: bool,
    l1_edge: EnabledSwitchEdge,
    l2_edge: EnabledSwitchEdge,
    r1_edge: EnabledSwitchEdge,
    r2_edge: EnabledSwitchEdge,
}

impl Default for YunZhuoModeStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl YunZhuoModeStateMachine {
    pub fn new() -> Self {
        Self {
            debounce_us: YUNZHUO_DEBOUNCE_US,
            neutral_gate_us: YUNZHUO_NEUTRAL_GATE_US,
            dog_ramp_down_us: YUNZHUO_DOG_RAMP_DOWN_US,
            dog_ramp_up_us: YUNZHUO_DOG_RAMP_UP_US,
            watchdog_timeout_us: YUNZHUO_WATCHDOG_TIMEOUT_US,
            stick_deadzone: YUNZHUO_STICK_CENTER_DEADZONE,
            arm_deadzone: YUNZHUO_DIRECTION_ARM_DEADZONE,
            arm_admitted: true,
            mode: YunZhuoControlMode::Neutral,
            mode_before_neutral: None,
            candidate: None,
            candidate_since_us: None,
            neutral_centered_since_us: None,
            neutral_gate_ready: false,
            last_update_us: None,
            ramp_start: 0.0,
            ramp_target: 0.0,
            ramp_started_us: None,
            ramp_duration_us: 0,
            direction_armed: false,
            l1_edge: EnabledSwitchEdge::default(),
            l2_edge: EnabledSwitchEdge::default(),
            r1_edge: EnabledSwitchEdge::default(),
            r2_edge: EnabledSwitchEdge::default(),
        }
    }

    pub fn mode(&self) -> YunZhuoControlMode {
        self.mode
    }

    pub fn neutral_gate_ready(&self) -> bool {
        self.neutral_gate_ready
    }

    pub fn direction_armed(&self) -> bool {
        self.direction_armed
    }

    pub fn set_arm_admitted(&mut self, admitted: bool) {
        self.arm_admitted = admitted;
    }

    /// Resets all mode/ramp/button history to safe `Neutral` on input loss or watchdog expiry.
    pub fn reset_for_input_loss(&mut self) {
        self.mode = YunZhuoControlMode::Neutral;
        self.mode_before_neutral = None;
        self.candidate = None;
        self.candidate_since_us = None;
        self.neutral_centered_since_us = None;
        self.neutral_gate_ready = false;
        self.last_update_us = None;
        self.ramp_start = 0.0;
        self.ramp_target = 0.0;
        self.ramp_started_us = None;
        self.ramp_duration_us = 0;
        self.direction_armed = false;
        self.l1_edge.reset();
        self.l2_edge.reset();
        self.r1_edge.reset();
        self.r2_edge.reset();
    }

    /// Checks 150ms frame-loss watchdog at `now_us`; returns `true` if link timed out and reset.
    pub fn check_watchdog(&mut self, now_us: u64) -> bool {
        let Some(prev_us) = self.last_update_us else {
            return true;
        };
        if now_us < prev_us || now_us.saturating_sub(prev_us) > self.watchdog_timeout_us {
            self.reset_for_input_loss();
            return true;
        }
        false
    }

    pub fn scale_at(&self, now_us: u64) -> f64 {
        let Some(started_us) = self.ramp_started_us else {
            return self.ramp_target;
        };
        if self.ramp_duration_us == 0 {
            return self.ramp_target;
        }
        if now_us < started_us {
            return if self.ramp_target == 0.0 {
                0.0
            } else {
                self.ramp_start
            };
        }
        let elapsed_us = now_us - started_us;
        if elapsed_us == 0 {
            return self.ramp_start;
        }
        if elapsed_us >= self.ramp_duration_us {
            return self.ramp_target;
        }
        let t = (elapsed_us as f64) / (self.ramp_duration_us as f64);
        self.ramp_start + (self.ramp_target - self.ramp_start) * t
    }

    fn set_ramp(&mut self, now_us: u64, target: f64, duration_us: u64) {
        let current = self.scale_at(now_us);
        self.ramp_start = if duration_us == 0 { target } else { current };
        self.ramp_target = target;
        self.ramp_started_us = Some(now_us);
        self.ramp_duration_us = duration_us;
    }

    fn dog_exit_ramp_active(&self, now_us: u64) -> bool {
        let Some(started_us) = self.ramp_started_us else {
            return false;
        };
        if self.ramp_target != 0.0
            || self.ramp_duration_us != self.dog_ramp_down_us
            || self.ramp_duration_us == 0
        {
            return false;
        }
        now_us >= started_us && (now_us - started_us) < self.ramp_duration_us
    }

    pub fn update(&mut self, state: &YunZhuoState, now_us: u64) -> YunZhuoModeSnapshot {
        if let Some(prev_us) = self.last_update_us {
            if now_us < prev_us || now_us.saturating_sub(prev_us) > self.watchdog_timeout_us {
                self.reset_for_input_loss();
            }
        }
        if !state.healthy {
            let was_non_neutral = self.mode != YunZhuoControlMode::Neutral;
            self.reset_for_input_loss();
            return YunZhuoModeSnapshot {
                mode: YunZhuoControlMode::Neutral,
                dog_drive_scale: 0.0,
                speed_scale: 1.0,
                neutral_gate_ready: false,
                mode_changed: was_non_neutral,
                direction_armed: false,
                standup_triggered: false,
                sitdown_triggered: false,
                damp_triggered: false,
                pad: RemotePadState {
                    active: false,
                    timestamp_us: now_us,
                    ..RemotePadState::default()
                },
            };
        }

        self.last_update_us = Some(now_us);
        let mut changed = false;
        let detent = state.g_s;

        if self.candidate != Some(detent) {
            self.candidate = Some(detent);
            self.candidate_since_us = Some(now_us);
        }

        let centered = state.sticks_centered(self.stick_deadzone);
        if detent == GSState::Middle && centered {
            if self.neutral_centered_since_us.is_none() {
                self.neutral_centered_since_us = Some(now_us);
            }
        } else if detent == GSState::Middle {
            self.neutral_centered_since_us = None;
            if self.neutral_gate_ready {
                self.mode_before_neutral = None;
            }
            self.neutral_gate_ready = false;
        } else {
            self.neutral_centered_since_us = None;
            if self.neutral_gate_ready && !centered {
                self.neutral_gate_ready = false;
                self.mode_before_neutral = None;
            }
        }

        // Immediate Neutral drop without debounce if leaving Arm!
        if self.mode == YunZhuoControlMode::Arm && detent != GSState::Down {
            self.mode_before_neutral = Some(YunZhuoControlMode::Arm);
            self.mode = YunZhuoControlMode::Neutral;
            self.neutral_gate_ready = false;
            self.set_ramp(now_us, 0.0, 0);
            changed = true;
        }

        let candidate_stable = self
            .candidate_since_us
            .is_some_and(|since| now_us >= since && (now_us - since) >= self.debounce_us);

        if candidate_stable {
            match detent {
                GSState::Middle => {
                    if self.mode != YunZhuoControlMode::Neutral {
                        let old_mode = self.mode;
                        self.mode_before_neutral = Some(old_mode);
                        self.mode = YunZhuoControlMode::Neutral;
                        self.neutral_gate_ready = false;
                        if old_mode == YunZhuoControlMode::Dog {
                            self.set_ramp(now_us, 0.0, self.dog_ramp_down_us);
                        } else {
                            self.set_ramp(now_us, 0.0, 0);
                        }
                        changed = true;
                    }
                    if let Some(centered_since) = self.neutral_centered_since_us {
                        if now_us.saturating_sub(centered_since) >= self.neutral_gate_us {
                            self.neutral_gate_ready = true;
                        }
                    }
                }
                GSState::Up => {
                    if self.mode == YunZhuoControlMode::Neutral {
                        let cancelling_dog_exit = self.mode_before_neutral
                            == Some(YunZhuoControlMode::Dog)
                            && !self.neutral_gate_ready
                            && self.dog_exit_ramp_active(now_us);
                        let selecting_after_gate = self.neutral_gate_ready;
                        if selecting_after_gate {
                            self.neutral_gate_ready = false;
                            self.mode_before_neutral = None;
                        }
                        if (selecting_after_gate && centered) || cancelling_dog_exit {
                            self.mode = YunZhuoControlMode::Dog;
                            self.mode_before_neutral = None;
                            self.set_ramp(now_us, 1.0, self.dog_ramp_up_us);
                            changed = true;
                        }
                    } else if self.mode == YunZhuoControlMode::Arm {
                        self.mode_before_neutral = Some(YunZhuoControlMode::Arm);
                        self.mode = YunZhuoControlMode::Neutral;
                        self.neutral_gate_ready = false;
                        self.set_ramp(now_us, 0.0, 0);
                        changed = true;
                    }
                }
                GSState::Down => {
                    if self.mode == YunZhuoControlMode::Neutral && self.neutral_gate_ready {
                        self.neutral_gate_ready = false;
                        self.mode_before_neutral = None;
                        if centered && self.arm_admitted {
                            self.mode = YunZhuoControlMode::Arm;
                            self.mode_before_neutral = None;
                            self.set_ramp(now_us, 0.0, 0);
                            changed = true;
                        }
                    } else if self.mode == YunZhuoControlMode::Dog {
                        self.mode_before_neutral = Some(YunZhuoControlMode::Dog);
                        self.mode = YunZhuoControlMode::Neutral;
                        self.neutral_gate_ready = false;
                        self.set_ramp(now_us, 0.0, self.dog_ramp_down_us);
                        changed = true;
                    }
                }
            }
        }

        let dog_drive_scale = self.scale_at(now_us).clamp(0.0, 1.0);
        let speed_scale = state.speed_scale();

        // Deadzone arming (< 0.02) and stick direction gating when H is enabled
        let (lx_out, ly_out, rx_out, ry_out) = if !state.h {
            self.direction_armed = false;
            (0.0, 0.0, 0.0, 0.0)
        } else {
            let raw_lx = state.left_stick[0] * speed_scale;
            let raw_ly = state.left_stick[1] * speed_scale;
            let raw_yaw = (state.knob + state.right_stick[0] * 0.5).clamp(-1.0, 1.0);
            if raw_lx.abs() < self.arm_deadzone
                && raw_ly.abs() < self.arm_deadzone
                && raw_yaw.abs() < self.arm_deadzone
            {
                self.direction_armed = true;
            }
            if self.direction_armed && dog_drive_scale > 0.0 {
                let yaw_src = if state.knob.abs() > 1e-6 {
                    state.knob.clamp(-1.0, 1.0)
                } else {
                    raw_yaw
                };
                (
                    raw_lx * dog_drive_scale,
                    raw_ly * dog_drive_scale,
                    yaw_src * dog_drive_scale,
                    state.right_stick[1] * dog_drive_scale,
                )
            } else {
                (0.0, 0.0, 0.0, 0.0)
            }
        };

        // Edge-triggered button detection (L1 standup, L2 sitdown, R1 damp) when H is enabled
        let dog_buttons_allowed = state.h && self.mode != YunZhuoControlMode::Arm;
        let standup_triggered = self.l1_edge.update(dog_buttons_allowed, state.l1);
        let sitdown_triggered = self.l2_edge.update(dog_buttons_allowed, state.l2);
        let r1_damp_triggered = self.r1_edge.update(dog_buttons_allowed, state.r1);
        let r2_triggered = self.r2_edge.update(dog_buttons_allowed, state.r2);
        let damp_triggered = state.red || r1_damp_triggered;

        let pad = RemotePadState {
            active: true,
            lx: lx_out,
            ly: ly_out,
            rx: rx_out,
            ry: ry_out,
            l1: standup_triggered,
            l2: sitdown_triggered,
            r1: r1_damp_triggered,
            r2: r2_triggered,
            damp_btn: state.red,
            timestamp_us: now_us,
        };

        YunZhuoModeSnapshot {
            mode: self.mode,
            dog_drive_scale,
            speed_scale,
            neutral_gate_ready: self.neutral_gate_ready,
            mode_changed: changed,
            direction_armed: self.direction_armed,
            standup_triggered,
            sitdown_triggered,
            damp_triggered,
            pad,
        }
    }
}

/// Raw 8-byte Linux `/dev/input/js*` `struct js_event`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxJsEvent {
    pub time: u32,
    pub value: i16,
    pub event_type: u8,
    pub number: u8,
}

impl LinuxJsEvent {
    pub const SIZE: usize = 8;

    pub const fn from_le_bytes(b: &[u8; 8]) -> Self {
        Self {
            time: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            value: i16::from_le_bytes([b[4], b[5]]),
            event_type: b[6],
            number: b[7],
        }
    }

    pub const fn is_button(&self) -> bool {
        (self.event_type & JS_EVENT_BUTTON) != 0
    }

    pub const fn is_axis(&self) -> bool {
        (self.event_type & JS_EVENT_AXIS) != 0
    }

    pub const fn is_init(&self) -> bool {
        (self.event_type & JS_EVENT_INIT) != 0
    }
}

/// Stateful mapper from Linux `/dev/input/js*` events (Xbox / NC500 standard XInput layout)
/// to [`RemotePadState`], matching `xbox_controller.dart`.
pub struct JoystickPadMapper {
    pub deadzone: f64,
    axes: [f64; 16],
    buttons: [bool; 16],
    connected: bool,
    last_event_us: u64,
}

impl Default for JoystickPadMapper {
    fn default() -> Self {
        Self::new()
    }
}

impl JoystickPadMapper {
    pub fn new() -> Self {
        let mut axes = [0.0; 16];
        // Triggers rest at -1.0 before pressed
        axes[2] = -1.0;
        axes[5] = -1.0;
        Self {
            deadzone: 0.08,
            axes,
            buttons: [false; 16],
            connected: false,
            last_event_us: 0,
        }
    }

    pub fn mark_connected(&mut self, now_us: u64) {
        self.connected = true;
        self.last_event_us = now_us;
        self.axes = [0.0; 16];
        self.axes[2] = -1.0;
        self.axes[5] = -1.0;
        self.buttons = [false; 16];
    }

    pub fn mark_disconnected(&mut self) {
        self.connected = false;
        self.axes = [0.0; 16];
        self.axes[2] = -1.0;
        self.axes[5] = -1.0;
        self.buttons = [false; 16];
    }

    pub fn apply_event(&mut self, ev: LinuxJsEvent, now_us: u64) {
        self.connected = true;
        self.last_event_us = now_us;
        let idx = ev.number as usize;
        if ev.is_axis() && idx < self.axes.len() {
            self.axes[idx] = (ev.value as f64 / 32767.0).clamp(-1.0, 1.0);
        } else if ev.is_button() && !ev.is_init() && idx < self.buttons.len() {
            self.buttons[idx] = ev.value != 0;
        }
    }

    pub fn sample(&self, now_us: u64) -> RemotePadState {
        if !self.connected {
            return RemotePadState {
                timestamp_us: now_us,
                ..RemotePadState::default()
            };
        }
        let dz = |v: f64| if v.abs() < self.deadzone { 0.0 } else { v };
        let lt = (self.axes[2] + 1.0) * 0.5;
        let rt = (self.axes[5] + 1.0) * 0.5;
        let scale = if lt > 0.3 {
            0.5
        } else if rt > 0.3 {
            1.5
        } else {
            1.0
        };
        let lx = (dz(self.axes[0]) * scale).clamp(-1.0, 1.0);
        let ly = (-dz(self.axes[1]) * scale).clamp(-1.0, 1.0);
        let rx = (dz(self.axes[3]) * scale).clamp(-1.0, 1.0);
        let ry = (-dz(self.axes[4]) * scale).clamp(-1.0, 1.0);

        let l1 = self.buttons[0] || self.buttons[4];
        let l2 = self.buttons[2];
        let r1 = self.buttons[5];
        let r2 = self.buttons[7];
        let damp_btn = self.buttons[1];
        let active =
            lx != 0.0 || ly != 0.0 || rx != 0.0 || ry != 0.0 || l1 || l2 || r1 || r2 || damp_btn;

        RemotePadState {
            active,
            lx,
            ly,
            rx,
            ry,
            l1,
            l2,
            r1,
            r2,
            damp_btn,
            timestamp_us: now_us,
        }
    }
}
