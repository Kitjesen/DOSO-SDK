use std::collections::VecDeque;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::arbiter::{ControlArbiter, ControlOwner};
use crate::control::gestures::{GestureCatalog, GesturePlayer};
use crate::control::handover::{blend_walk_entry, BodyHeightController};
use crate::control::recovery::{RecoveryRuntime, RecoverySnapshot};
use crate::hardware::HalBackend;
use crate::params::{RobotHardwareParams, NUM_JOINTS, WHEEL_START_INDEX};
use crate::policy::{
    LoadedPolicyPackage, PolicyEvalContext, PolicyKind, PolicyLoadReport, PolicyRegistry,
    ShadowEvalReport, ShadowEvaluator,
};
use crate::safety::{SafetyConfig, SafetyGate};
use crate::types::{JointVec16, MitCmd16, ProprioFrame, RemotePadState, WalkCommand};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum FsmState {
    Grounded = 1,
    Standing = 2,
    Walking = 3,
    Transitioning = 4,
    Damping = 5,
    Recovery = 6,
    External = 7,
}

impl FsmState {
    /// Maps internal FSM state to public `doso.v1.BodyKind` integer wire contract.
    pub const fn to_public_body_kind(self) -> i32 {
        match self {
            Self::Grounded => 1,
            Self::Standing => 2,
            Self::Walking => 3,
            Self::Transitioning | Self::Damping | Self::Recovery | Self::External => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum BodyTransitionKind {
    None = 0,
    StandUp = 1,
    SitDown = 2,
    Gesture = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(i32)]
pub enum SpeedMode {
    #[default]
    Normal = 0,
    High = 1,
}

#[derive(Debug, Clone)]
pub struct HistoryFrameSnapshot {
    pub monotonic_us: u64,
    pub gyro_rad_s: [f64; 3],
    pub projected_gravity: [f64; 3],
    pub walk_cmd: WalkCommand,
    pub joint_pos: JointVec16,
    pub joint_vel: JointVec16,
    pub action: JointVec16,
    pub next_action: JointVec16,
    pub kp: JointVec16,
    pub kd: JointVec16,
    pub observation: Vec<f64>,
}

#[derive(Debug, Clone, Copy)]
pub struct BodyHeightState {
    pub supported: bool,
    pub current_height_m: f64,
    pub target_height_m: f64,
    pub min_height_m: f64,
    pub max_height_m: f64,
    pub rate_mps: f64,
}

#[derive(Debug, Clone, Default)]
pub struct ExternalSessionState {
    pub active: bool,
    pub session_id: String,
    pub last_sequence: u64,
    pub expires_us: u64,
    pub latest_cmd: Option<MitCmd16>,
}

#[derive(Debug, Clone)]
pub struct BrainStatusSnapshot {
    pub fsm_state: FsmState,
    pub transition: BodyTransitionKind,
    pub gesture_name: String,
    pub motor_output_enabled: bool,
    pub critical_motor_fault: bool,
    pub source_owner: ControlOwner,
    pub source_owner_id: String,
    pub grpc_lease_active: bool,
    pub lease_remaining_ms: u32,
    pub last_accepted_sequence: u64,
    pub accepted_count: u64,
    pub rejected_count: u64,
    pub active_profile: String,
}

enum TransitionTask {
    StandUp {
        start: JointVec16,
        target: JointVec16,
        step: usize,
        total: usize,
        lower: [f64; NUM_JOINTS],
        upper: [f64; NUM_JOINTS],
        pending_sit_down: bool,
    },
    SitDown {
        start: JointVec16,
        target: JointVec16,
        step: usize,
        total: usize,
        lower: [f64; NUM_JOINTS],
        upper: [f64; NUM_JOINTS],
    },
    Damp {
        entry: JointVec16,
        entry_kp: JointVec16,
        tick: usize,
        ramp_counts: usize,
        settled_ticks: usize,
        settle_counts: usize,
        disable_on_finish: bool,
    },
    Gesture(GesturePlayer),
}

/// Complete, decoupled deterministic Brainstem control engine.
/// Owns FSM, PolicyRegistry, Gestures, BodyHeight, Recovery, External MIT, SafetyGate, and ring history for Shadow Mode.
pub struct BrainEngine {
    pub hal: Box<dyn HalBackend>,
    pub safety: SafetyGate,
    pub arbiter: ControlArbiter,
    pub policies: PolicyRegistry,
    pub gestures: GestureCatalog,
    pub height: BodyHeightController,
    pub recovery: RecoveryRuntime,
    pub external: ExternalSessionState,

    fsm: FsmState,
    speed_mode: SpeedMode,
    transition_task: Option<TransitionTask>,
    walk_cmd: WalkCommand,
    walk_entry_pose: JointVec16,
    walk_entry_frame: usize,
    last_commanded_pose: JointVec16,
    last_commanded_kp: JointVec16,
    latest_proprio: ProprioFrame,
    has_proprio: bool,
    sim_proprio_override: Option<ProprioFrame>,
    recent_proprio_ring: VecDeque<ProprioFrame>,
    latest_history: Option<HistoryFrameSnapshot>,
    history_seq: u64,
    obs_scratch: Vec<f64>,
    prev_pad_active: bool,
    prev_pad_l1: bool,
    prev_pad_l2: bool,
    prev_pad_r1: bool,
    tick_counter: u64,
}

impl BrainEngine {
    pub fn new(hal: Box<dyn HalBackend>, safety_cfg: SafetyConfig) -> Self {
        let nominal_stand = RobotHardwareParams::thunder_v4().posture.standing_pose;
        Self {
            hal,
            safety: SafetyGate::new(safety_cfg),
            arbiter: ControlArbiter::new(350),
            policies: PolicyRegistry::default(),
            gestures: GestureCatalog::with_builtins(&nominal_stand),
            height: BodyHeightController::default(),
            recovery: RecoveryRuntime::default(),
            external: ExternalSessionState::default(),
            fsm: FsmState::Grounded,
            speed_mode: SpeedMode::Normal,
            transition_task: None,
            walk_cmd: WalkCommand::default(),
            walk_entry_pose: JointVec16(nominal_stand),
            walk_entry_frame: 0,
            last_commanded_pose: JointVec16::zero(),
            last_commanded_kp: JointVec16::zero(),
            latest_proprio: ProprioFrame::default(),
            has_proprio: false,
            sim_proprio_override: None,
            recent_proprio_ring: VecDeque::with_capacity(512),
            latest_history: None,
            history_seq: 0,
            obs_scratch: Vec::with_capacity(600),
            prev_pad_active: false,
            prev_pad_l1: false,
            prev_pad_l2: false,
            prev_pad_r1: false,
            tick_counter: 0,
        }
    }

    pub fn register_policy(
        &mut self,
        pkg: LoadedPolicyPackage,
        slot: Option<&str>,
    ) -> Result<PolicyLoadReport> {
        let kind = pkg.graph.manifest().kind;
        let report = self.policies.register(pkg);
        if kind == PolicyKind::Recovery {
            self.recovery.set_policy(report.name.clone(), true);
        }
        if let Some(s) = slot {
            self.policies.bind_slot(s, &report.name)?;
        }
        self.sync_height_spec();
        Ok(report)
    }

    /// Captures a lightweight snapshot `(frames, walk_cmd, body_height_m)` of the most recent
    /// live proprioception ring for off-lock Shadow Mode evaluation (`< 5 us` mutex hold).
    pub fn snapshot_for_shadow(&self, max_frames: usize) -> (Vec<ProprioFrame>, WalkCommand, f64) {
        let frames: Vec<ProprioFrame> = if self.recent_proprio_ring.is_empty() {
            vec![self.latest_proprio; max_frames.max(10)]
        } else {
            let len = self.recent_proprio_ring.len();
            let count = max_frames.max(1).min(len);
            self.recent_proprio_ring
                .iter()
                .skip(len - count)
                .copied()
                .collect()
        };
        let bh = if self.height.supported {
            self.height.target_m
        } else {
            0.375
        };
        (frames, self.walk_cmd, bh)
    }

    /// Evaluates a candidate policy in Shadow Mode against the most recent live proprioception buffer (zero motor risk).
    pub fn shadow_evaluate_candidate(
        &self,
        candidate: &mut LoadedPolicyPackage,
        max_frames: usize,
    ) -> Result<ShadowEvalReport> {
        let (frames, walk_cmd, bh) = self.snapshot_for_shadow(max_frames);
        ShadowEvaluator::evaluate_frames(candidate, &frames, walk_cmd, bh)
    }

    pub fn switch_profile(&mut self, name_or_slot: &str) -> Result<String> {
        if self.fsm != FsmState::Grounded
            || self.safety.motor_output_enabled()
            || self.recovery.selected()
            || self.external.active
        {
            bail!(
                "Profile switch requires Grounded state, disabled motors, and no active Recovery/External session"
            );
        }
        let prev_pose = self.last_commanded_pose;
        let (prev_kp, prev_kd) = self.active_infer_gains();
        let active = self.policies.switch_active(name_or_slot)?;
        self.sync_height_spec();
        if self.fsm != FsmState::Grounded && self.safety.motor_output_enabled() {
            self.height.start_handover(prev_pose, prev_kp, prev_kd);
        } else {
            self.height.reset_handover();
        }
        if let Some(pkg) = self.policies.active() {
            self.gestures =
                GestureCatalog::with_builtins(&pkg.graph.manifest().posture.standing_pose);
        }
        Ok(active)
    }

    pub fn rollback_slot(&mut self, slot: &str) -> Result<String> {
        if self.fsm != FsmState::Grounded || self.safety.motor_output_enabled() {
            bail!("Policy rollback requires Grounded state and disabled motors");
        }
        let prev = self.policies.rollback_slot(slot)?;
        self.sync_height_spec();
        Ok(prev)
    }

    pub fn set_speed_mode(&mut self, mode: SpeedMode) {
        self.speed_mode = mode;
        self.walk_cmd = Self::clamp_walk_for_mode(self.walk_cmd, mode);
    }

    pub fn speed_mode(&self) -> SpeedMode {
        self.speed_mode
    }

    pub fn walk_command(&self) -> WalkCommand {
        self.walk_cmd
    }

    pub fn latest_history(&self) -> Option<&HistoryFrameSnapshot> {
        self.latest_history.as_ref()
    }

    pub fn history_seq(&self) -> u64 {
        self.history_seq
    }

    pub fn inject_sim_state(&mut self, mut sim_proprio: ProprioFrame) {
        sim_proprio.imu.online = true;
        for j in &mut sim_proprio.joints {
            j.online = true;
        }
        self.record_proprio(sim_proprio);
        self.sim_proprio_override = Some(sim_proprio);
    }

    /// Deterministically steps one full 50Hz control/policy frame in simulation mode and returns its `HistoryFrameSnapshot`.
    pub fn tick_sim(&mut self, now_us: u64) -> HistoryFrameSnapshot {
        let proprio = if let Some(mut p) = self.sim_proprio_override {
            p.monotonic_us = now_us;
            p.imu.timestamp_us = now_us;
            p.imu.online = true;
            for j in &mut p.joints {
                j.last_seen_us = now_us;
                j.online = true;
            }
            self.sim_proprio_override = Some(p);
            p
        } else {
            self.hal.sample_proprio(now_us)
        };
        let pad = self.hal.sample_remote(now_us);
        if !self.tick_counter.is_multiple_of(4) {
            self.tick_counter = self.tick_counter.wrapping_add(4 - (self.tick_counter % 4));
        }
        self.step_with_inputs(proprio, pad, now_us, true);
        self.tick_counter = self.tick_counter.wrapping_add(3);
        self.latest_history
            .clone()
            .expect("50Hz tick_sim always populates latest_history")
    }

    pub fn calibrate_set_zero(&mut self) -> Result<()> {
        if self.fsm != FsmState::Grounded
            || self.safety.motor_output_enabled()
            || !self.hal.motor_disable_confirmed()
            || self.recovery.selected()
            || self.external.active
        {
            bail!(
                "SetZero requires Grounded state, disabled motors, and confirmed hardware disable (fsm={:?}, motors_enabled={}, disable_confirmed={})",
                self.fsm,
                self.safety.motor_output_enabled(),
                self.hal.motor_disable_confirmed()
            );
        }
        self.safety.disable_motors(self.hal.as_mut(), false)?;
        if !self.hal.motor_disable_confirmed() {
            bail!("SetZero refused: hardware motor disable is not confirmed");
        }
        self.hal.calibrate_set_zero()
    }

    pub fn body_height_status(&self) -> BodyHeightState {
        BodyHeightState {
            supported: self.height.supported,
            current_height_m: self.height.current_m,
            target_height_m: self.height.target_m,
            min_height_m: self.height.min_m,
            max_height_m: self.height.max_m,
            rate_mps: self.height.rate_mps,
        }
    }

    pub fn set_body_height(&mut self, height_m: f64) -> Result<BodyHeightState> {
        self.height
            .set_target(height_m, self.fsm == FsmState::Grounded)?;
        Ok(self.body_height_status())
    }

    pub fn recovery_status(&self) -> RecoverySnapshot {
        let grounded_free = self.fsm == FsmState::Grounded
            && !self.safety.motor_output_enabled()
            && self.arbiter.owner() == ControlOwner::None
            && !self.external.active;
        self.recovery.snapshot(grounded_free)
    }

    pub fn fsm_state(&self) -> FsmState {
        self.fsm
    }

    pub fn has_critical_motor_fault(&self) -> bool {
        self.safety.latched_fault().is_some()
    }

    /// Refreshes an existing [`BrainStatusSnapshot`] in place without allocating strings on steady ticks.
    pub fn copy_status_into(&self, now_us: u64, dst: &mut BrainStatusSnapshot) {
        let (transition, gesture_name) = match &self.transition_task {
            Some(TransitionTask::StandUp { .. }) => (BodyTransitionKind::StandUp, ""),
            Some(TransitionTask::SitDown { .. }) => (BodyTransitionKind::SitDown, ""),
            Some(TransitionTask::Gesture(g)) => (BodyTransitionKind::Gesture, g.name()),
            _ => (BodyTransitionKind::None, ""),
        };
        let (acc, rej) = self.arbiter.counts();
        dst.fsm_state = self.fsm;
        dst.transition = transition;
        dst.gesture_name.clear();
        dst.gesture_name.push_str(gesture_name);
        dst.motor_output_enabled = self.safety.motor_output_enabled();
        dst.critical_motor_fault = self.safety.latched_fault().is_some();
        dst.source_owner = self.arbiter.owner();
        dst.source_owner_id.clear();
        dst.source_owner_id.push_str(self.arbiter.owner_id());
        dst.grpc_lease_active = self.arbiter.grpc_lease_active(now_us);
        dst.lease_remaining_ms = self.arbiter.lease_remaining_ms(now_us);
        dst.last_accepted_sequence = self.arbiter.last_sequence();
        dst.accepted_count = acc;
        dst.rejected_count = rej;
        dst.active_profile.clear();
        dst.active_profile.push_str(self.policies.active_name());
    }

    pub fn status_snapshot(&self, now_us: u64) -> BrainStatusSnapshot {
        let (transition, gesture_name) = match &self.transition_task {
            Some(TransitionTask::StandUp { .. }) => (BodyTransitionKind::StandUp, String::new()),
            Some(TransitionTask::SitDown { .. }) => (BodyTransitionKind::SitDown, String::new()),
            Some(TransitionTask::Gesture(g)) => (BodyTransitionKind::Gesture, g.name().to_string()),
            _ => (BodyTransitionKind::None, String::new()),
        };
        let (acc, rej) = self.arbiter.counts();
        BrainStatusSnapshot {
            fsm_state: self.fsm,
            transition,
            gesture_name,
            motor_output_enabled: self.safety.motor_output_enabled(),
            critical_motor_fault: self.safety.latched_fault().is_some(),
            source_owner: self.arbiter.owner(),
            source_owner_id: self.arbiter.owner_id().to_string(),
            grpc_lease_active: self.arbiter.grpc_lease_active(now_us),
            lease_remaining_ms: self.arbiter.lease_remaining_ms(now_us),
            last_accepted_sequence: self.arbiter.last_sequence(),
            accepted_count: acc,
            rejected_count: rej,
            active_profile: self.policies.active_name().to_string(),
        }
    }

    pub fn latest_proprio(&self) -> &ProprioFrame {
        &self.latest_proprio
    }

    pub fn enable_motors(&mut self) -> Result<()> {
        if self.fsm != FsmState::Grounded {
            bail!(
                "EnableMotors requires Grounded state (current={:?})",
                self.fsm
            );
        }
        self.safety.enable_motors(self.hal.as_mut())
    }

    pub fn external_enable_motors(&mut self) -> Result<()> {
        if self.fsm != FsmState::External {
            bail!(
                "External EnableMotors requires External state (current={:?})",
                self.fsm
            );
        }
        self.safety.enable_motors(self.hal.as_mut())
    }

    pub fn clear_motor_fault(&mut self) -> Result<()> {
        self.arbiter.force_revoke();
        self.walk_cmd = WalkCommand::default();
        self.transition_task = None;
        self.external = ExternalSessionState::default();
        self.fsm = FsmState::Grounded;
        self.safety.clear_fault(self.hal.as_mut())
    }

    pub fn recovery_enable(&mut self, token: &str, now_us: u64) -> Result<()> {
        self.recovery.validate_enable(token, now_us)?;
        self.safety.enable_motors(self.hal.as_mut())?;
        let current_pos = self.latest_proprio.positions();
        self.recovery.enable(token, current_pos, now_us)
    }

    pub fn disable_motors(&mut self, clear_errors: bool) -> Result<()> {
        self.arbiter.force_revoke();
        self.walk_cmd = WalkCommand::default();
        self.transition_task = None;
        if self.fsm != FsmState::External && self.fsm != FsmState::Recovery {
            self.fsm = FsmState::Grounded;
        }
        self.safety.disable_motors(self.hal.as_mut(), clear_errors)
    }

    fn transition_leg_bounds(
        &self,
        start: &JointVec16,
        target: &JointVec16,
    ) -> ([f64; NUM_JOINTS], [f64; NUM_JOINTS]) {
        let cfg = self.safety.config();
        let mut low = cfg.deploy_action_min;
        let mut high = cfg.deploy_action_max;
        for i in 0..WHEEL_START_INDEX {
            let span_min = start.0[i].min(target.0[i]) - 0.05;
            let span_max = start.0[i].max(target.0[i]) + 0.05;
            low[i] = low[i].min(span_min).clamp(-3.15, 3.15);
            high[i] = high[i].max(span_max).clamp(-3.15, 3.15);
        }
        (low, high)
    }

    const fn recovery_leg_bounds() -> ([f64; NUM_JOINTS], [f64; NUM_JOINTS]) {
        let mut low = [-3.15; NUM_JOINTS];
        let mut high = [3.15; NUM_JOINTS];
        let mut i = 0;
        while i < WHEEL_START_INDEX {
            if i % 3 == 0 {
                low[i] = -0.45;
                high[i] = 0.45;
            }
            i += 1;
        }
        let mut w = WHEEL_START_INDEX;
        while w < NUM_JOINTS {
            low[w] = -37.7;
            high[w] = 37.7;
            w += 1;
        }
        (low, high)
    }

    pub fn stand_up(&mut self) -> Result<()> {
        if self.fsm != FsmState::Grounded {
            bail!("StandUp requires Grounded state (current={:?})", self.fsm);
        }
        if !self.safety.motor_output_enabled() {
            bail!("StandUp requires motors enabled first");
        }
        if let Some(fault) = self.safety.latched_fault() {
            bail!("StandUp rejected: latched motor fault ({fault:?}); call ClearMotorFault first");
        }
        if self.has_proprio || self.hal.name().starts_with("socketcan") {
            let mask = self.latest_proprio.online_mask();
            if mask != 0xFFFF || !self.latest_proprio.imu.online {
                bail!(
                    "StandUp requires all 16 motors (mask={mask:#06x}) and IMU online (imu={})",
                    self.latest_proprio.imu.online
                );
            }
        }
        let Some(pkg) = self.policies.active() else {
            bail!("No active locomotion policy loaded");
        };
        let target = JointVec16(pkg.graph.manifest().posture.standing_pose).discard_wheels();
        let counts = pkg.graph.manifest().posture.stand_up_counts;
        let start = self.latest_proprio.positions().discard_wheels();
        self.begin_stand_up_from(start, target, counts, false);
        Ok(())
    }

    fn begin_stand_up_from(
        &mut self,
        start: JointVec16,
        target: JointVec16,
        total: usize,
        pending_sit_down: bool,
    ) {
        let (lower, upper) = self.transition_leg_bounds(&start, &target);
        self.height.reset_handover();
        self.walk_cmd = WalkCommand::default();
        self.fsm = FsmState::Transitioning;
        self.transition_task = Some(TransitionTask::StandUp {
            start,
            target,
            step: 0,
            total,
            lower,
            upper,
            pending_sit_down,
        });
    }

    fn begin_sit_down_from(&mut self, start: JointVec16, target: JointVec16, total: usize) {
        let (lower, upper) = self.transition_leg_bounds(&start, &target);
        self.walk_cmd = WalkCommand::default();
        self.fsm = FsmState::Transitioning;
        self.transition_task = Some(TransitionTask::SitDown {
            start,
            target,
            step: 0,
            total,
            lower,
            upper,
        });
    }

    pub fn sit_down(&mut self) -> Result<()> {
        let is_stand_up = self.fsm == FsmState::Transitioning
            && matches!(self.transition_task, Some(TransitionTask::StandUp { .. }));
        if self.fsm != FsmState::Standing && self.fsm != FsmState::Walking && !is_stand_up {
            bail!("SitDown requires Standing, Walking, or active StandUp transition");
        }
        let posture = {
            let Some(pkg) = self.policies.active() else {
                bail!("No active locomotion policy loaded");
            };
            pkg.graph.manifest().posture.clone()
        };
        let start = self.latest_proprio.positions().discard_wheels();
        if self.fsm == FsmState::Walking {
            let standing_target = JointVec16(posture.standing_pose).discard_wheels();
            self.begin_stand_up_from(start, standing_target, posture.stand_up_counts, true);
        } else {
            let sitting_target = JointVec16(posture.sitting_pose).discard_wheels();
            self.begin_sit_down_from(start, sitting_target, posture.sit_down_counts);
        }
        Ok(())
    }

    pub fn damp(&mut self) -> Result<()> {
        if self.arbiter.owner() == ControlOwner::Yunzhuo {
            bail!("Damp rejected: YUNZHUO owns ControlArbiter");
        }
        self.enter_damp();
        Ok(())
    }

    fn enter_damp(&mut self) {
        self.enter_damp_with_disable(false);
    }

    fn enter_damp_with_disable(&mut self, disable_on_finish: bool) {
        self.arbiter.force_revoke();
        self.walk_cmd = WalkCommand::default();
        self.fsm = FsmState::Damping;
        let has_cmd_kp = self.last_commanded_kp.0.iter().any(|&k| k > 0.0);
        let entry = if has_cmd_kp {
            self.last_commanded_pose.discard_wheels()
        } else {
            self.latest_proprio.positions().discard_wheels()
        };
        let entry_kp = if has_cmd_kp {
            self.last_commanded_kp
        } else {
            self.active_stand_gains().0
        };
        self.transition_task = Some(TransitionTask::Damp {
            entry,
            entry_kp,
            tick: 0,
            ramp_counts: 5,
            settled_ticks: 0,
            settle_counts: 25,
            disable_on_finish,
        });
    }

    pub fn play_gesture(&mut self, name: &str) -> Result<()> {
        if self.fsm != FsmState::Standing {
            bail!("PlayGesture requires Standing state");
        }
        let Some(clip) = self.gestures.get(name).cloned() else {
            bail!("Unknown gesture '{name}'");
        };
        self.fsm = FsmState::Transitioning;
        self.transition_task = Some(TransitionTask::Gesture(GesturePlayer::start(
            clip,
            self.last_commanded_pose,
        )));
        Ok(())
    }

    pub fn set_walk_velocity(&mut self, cmd: WalkCommand) -> Result<()> {
        if !cmd.vx.is_finite() || !cmd.vy.is_finite() || !cmd.yaw.is_finite() {
            bail!("WalkCommand contains non-finite component");
        }
        let cmd = Self::clamp_walk_for_mode(cmd, self.speed_mode);
        if self.fsm == FsmState::Standing && !cmd.is_zero() {
            if let Some(pkg) = self.policies.active_mut() {
                pkg.reset();
            }
            self.walk_entry_pose = self.last_commanded_pose.discard_wheels();
            self.walk_entry_frame = 0;
            self.fsm = FsmState::Walking;
        } else if self.fsm == FsmState::Walking && cmd.is_zero() {
            self.walk_cmd = WalkCommand::default();
            self.fsm = FsmState::Standing;
            return Ok(());
        }
        self.walk_cmd = cmd;
        Ok(())
    }

    pub fn external_enter(&mut self, session_id: &str, now_us: u64) -> Result<()> {
        if self.fsm != FsmState::Grounded || self.safety.motor_output_enabled() {
            bail!("ExternalControl.Enter requires Grounded state and disabled motors");
        }
        self.arbiter.force_revoke();
        self.external = ExternalSessionState {
            active: true,
            session_id: session_id.to_string(),
            last_sequence: 0,
            expires_us: now_us + 500_000,
            latest_cmd: None,
        };
        self.fsm = FsmState::External;
        Ok(())
    }

    pub fn external_submit_frame(&mut self, seq: u64, cmd: MitCmd16, now_us: u64) -> Result<()> {
        if !self.external.active {
            bail!("No active ExternalControl session");
        }
        if seq <= self.external.last_sequence {
            bail!("Stale external frame sequence {seq}");
        }
        let cfg = self.safety.config();
        self.safety
            .validate_frame(&cmd, Some((&cfg.deploy_action_min, &cfg.deploy_action_max)))
            .map_err(|r| anyhow::anyhow!("External SafetyGate rejected frame: {r:?}"))?;
        self.external.last_sequence = seq;
        self.external.expires_us = now_us + 500_000;
        self.external.latest_cmd = Some(cmd);
        Ok(())
    }

    pub fn external_exit(&mut self) -> Result<()> {
        let _ = self.safety.disable_motors(self.hal.as_mut(), false);
        self.external = ExternalSessionState::default();
        self.fsm = FsmState::Grounded;
        Ok(())
    }

    /// Deterministic 200Hz main control tick (50Hz policy decimation every 4th tick).
    pub fn tick_200hz(&mut self, now_us: u64) {
        let proprio = if let Some(mut p) = self.sim_proprio_override.take() {
            p.monotonic_us = now_us;
            p.imu.timestamp_us = now_us;
            p.imu.online = true;
            for j in &mut p.joints {
                j.last_seen_us = now_us;
                j.online = true;
            }
            p
        } else {
            self.hal.sample_proprio(now_us)
        };
        let pad = self.hal.sample_remote(now_us);
        let is_policy_tick = self.tick_counter.is_multiple_of(4);
        self.step_with_inputs(proprio, pad, now_us, is_policy_tick);
    }

    fn step_with_inputs(
        &mut self,
        proprio: ProprioFrame,
        pad: RemotePadState,
        now_us: u64,
        is_policy_tick: bool,
    ) {
        self.record_proprio(proprio);

        // Physical remote preemption & rising-edge closed-loop actions (highest priority)
        if pad.active {
            let rising_l1 = pad.l1 && !self.prev_pad_l1;
            let rising_l2 = pad.l2 && !self.prev_pad_l2;
            let rising_r1 = pad.r1 && !self.prev_pad_r1;
            self.prev_pad_l1 = pad.l1;
            self.prev_pad_l2 = pad.l2;
            self.prev_pad_r1 = pad.r1;

            self.arbiter.preempt_yunzhuo(now_us);
            if pad.damp_btn || rising_r1 {
                self.enter_damp();
            } else if rising_l1 && self.fsm == FsmState::Grounded {
                let _ = self.stand_up();
            } else if rising_l2
                && (self.fsm == FsmState::Standing
                    || self.fsm == FsmState::Walking
                    || (self.fsm == FsmState::Transitioning
                        && matches!(self.transition_task, Some(TransitionTask::StandUp { .. }))))
            {
                let _ = self.sit_down();
            } else if self.fsm == FsmState::Standing || self.fsm == FsmState::Walking {
                let _ = self.set_walk_velocity(WalkCommand {
                    vx: pad.ly,
                    vy: -pad.lx,
                    yaw: -pad.rx,
                });
            }
        } else {
            self.prev_pad_l1 = false;
            self.prev_pad_l2 = false;
            self.prev_pad_r1 = false;
            if self.prev_pad_active && self.fsm == FsmState::Walking {
                self.walk_cmd = WalkCommand::default();
                self.fsm = FsmState::Standing;
            }
            if self.arbiter.expire_if_needed(now_us) && self.fsm == FsmState::Walking {
                self.walk_cmd = WalkCommand::default();
                self.fsm = FsmState::Standing;
            }
        }
        self.prev_pad_active = pad.active;

        self.tick_counter = self.tick_counter.wrapping_add(1);
        let prev_action = self.last_commanded_pose;

        // 1. External 200Hz MIT mode
        if self.fsm == FsmState::External {
            if now_us >= self.external.expires_us {
                self.external = ExternalSessionState::default();
                if self.safety.motor_output_enabled() {
                    self.enter_damp_with_disable(true);
                } else {
                    let _ = self.safety.disable_motors(self.hal.as_mut(), false);
                    self.fsm = FsmState::Grounded;
                }
                if is_policy_tick {
                    self.record_history_slice(
                        now_us,
                        &proprio,
                        prev_action,
                        self.last_commanded_pose,
                        JointVec16::zero(),
                        JointVec16::zero(),
                        &[],
                    );
                }
                return;
            }
            if let Some(cmd) = self.external.latest_cmd {
                let ext_min = self.safety.config().deploy_action_min;
                let ext_max = self.safety.config().deploy_action_max;
                let _ = self.safety.filter_and_dispatch(
                    self.hal.as_mut(),
                    &proprio,
                    now_us,
                    false,
                    Some((&ext_min, &ext_max)),
                    &cmd,
                );
                if is_policy_tick {
                    let (pose, kp, kd) = Self::extract_cmd_vectors(&cmd);
                    self.last_commanded_pose = pose;
                    self.last_commanded_kp = kp;
                    self.record_history_slice(now_us, &proprio, prev_action, pose, kp, kd, &[]);
                }
            } else if is_policy_tick {
                self.record_history_slice(
                    now_us,
                    &proprio,
                    prev_action,
                    self.last_commanded_pose,
                    JointVec16::zero(),
                    JointVec16::zero(),
                    &[],
                );
            }
            return;
        }

        // 2. Recovery 50Hz mode
        if self.recovery.selected() {
            self.fsm = FsmState::Recovery;
            if is_policy_tick {
                let mut rec_recorded = false;
                if let Some(rec_name) = self.recovery.policy_name().map(str::to_owned) {
                    if let Some(pkg) = self.policies.get_mut(&rec_name) {
                        match self.recovery.tick(&proprio, pkg, now_us) {
                            Ok(Some(cmd)) => {
                                let (pose, kp, kd) = Self::extract_cmd_vectors(&cmd);
                                self.last_commanded_pose = pose;
                                self.last_commanded_kp = kp;
                                let (rec_min, rec_max) = Self::recovery_leg_bounds();
                                let _ = self.safety.filter_and_dispatch(
                                    self.hal.as_mut(),
                                    &proprio,
                                    now_us,
                                    false,
                                    Some((&rec_min, &rec_max)),
                                    &cmd,
                                );
                                self.record_history_slice(
                                    now_us,
                                    &proprio,
                                    prev_action,
                                    pose,
                                    kp,
                                    kd,
                                    &[],
                                );
                                rec_recorded = true;
                            }
                            Ok(None) => {}
                            Err(_) => {
                                let _ = self.safety.disable_motors(self.hal.as_mut(), false);
                            }
                        }
                    }
                }
                if !rec_recorded {
                    self.record_history_slice(
                        now_us,
                        &proprio,
                        prev_action,
                        self.last_commanded_pose,
                        JointVec16::zero(),
                        JointVec16::zero(),
                        &[],
                    );
                }
            }
            return;
        } else if self.fsm == FsmState::Recovery {
            self.fsm = FsmState::Grounded;
        }

        if !is_policy_tick {
            return;
        }

        // 3. 50Hz Built-in FSM transitions & Locomotion
        let bh_cmd = self.height.tick_height(0.02);
        let (stand_kp, stand_kd) = self.active_stand_gains();
        let (infer_kp, infer_kd) = self.active_infer_gains();

        match self.fsm {
            FsmState::Grounded => {
                self.record_history_slice(
                    now_us,
                    &proprio,
                    prev_action,
                    self.last_commanded_pose,
                    JointVec16::zero(),
                    JointVec16::zero(),
                    &[],
                );
            }
            FsmState::Transitioning | FsmState::Damping => {
                if let Some(mut task) = self.transition_task.take() {
                    match &mut task {
                        TransitionTask::StandUp {
                            start,
                            target,
                            step,
                            total,
                            lower,
                            upper,
                            pending_sit_down,
                        } => {
                            let t = if *total == 0 {
                                1.0
                            } else {
                                (*step as f64 / *total as f64).clamp(0.0, 1.0)
                            };
                            let pose = JointVec16::lerp(start, target, t).discard_wheels();
                            self.last_commanded_pose = pose;
                            self.last_commanded_kp = stand_kp;
                            let cmd = MitCmd16::from_action_and_gains(&pose, &stand_kp, &stand_kd);
                            let _ = self.safety.filter_and_dispatch(
                                self.hal.as_mut(),
                                &proprio,
                                now_us,
                                false,
                                Some((lower, upper)),
                                &cmd,
                            );
                            if *step >= *total {
                                if *pending_sit_down {
                                    if let Some(posture) = self
                                        .policies
                                        .active()
                                        .map(|pkg| pkg.graph.manifest().posture.clone())
                                    {
                                        let sit_target =
                                            JointVec16(posture.sitting_pose).discard_wheels();
                                        let sit_start = proprio.positions().discard_wheels();
                                        self.begin_sit_down_from(
                                            sit_start,
                                            sit_target,
                                            posture.sit_down_counts,
                                        );
                                    } else {
                                        self.fsm = FsmState::Standing;
                                    }
                                } else {
                                    self.fsm = FsmState::Standing;
                                }
                            } else {
                                *step += 1;
                                self.transition_task = Some(task);
                            }
                            self.record_history_slice(
                                now_us,
                                &proprio,
                                prev_action,
                                pose,
                                stand_kp,
                                stand_kd,
                                &[],
                            );
                        }
                        TransitionTask::SitDown {
                            start,
                            target,
                            step,
                            total,
                            lower,
                            upper,
                        } => {
                            let t = if *total == 0 {
                                1.0
                            } else {
                                (*step as f64 / *total as f64).clamp(0.0, 1.0)
                            };
                            let pose = JointVec16::lerp(start, target, t).discard_wheels();
                            self.last_commanded_pose = pose;
                            self.last_commanded_kp = stand_kp;
                            let cmd = MitCmd16::from_action_and_gains(&pose, &stand_kp, &stand_kd);
                            let _ = self.safety.filter_and_dispatch(
                                self.hal.as_mut(),
                                &proprio,
                                now_us,
                                false,
                                Some((lower, upper)),
                                &cmd,
                            );
                            if *step >= *total {
                                self.fsm = FsmState::Grounded;
                            } else {
                                *step += 1;
                                self.transition_task = Some(task);
                            }
                            self.record_history_slice(
                                now_us,
                                &proprio,
                                prev_action,
                                pose,
                                stand_kp,
                                stand_kd,
                                &[],
                            );
                        }
                        TransitionTask::Damp {
                            entry,
                            entry_kp,
                            tick,
                            ramp_counts,
                            settled_ticks,
                            settle_counts,
                            disable_on_finish,
                        } => {
                            let scale = if *tick < *ramp_counts {
                                1.0 - (*tick as f64 / *ramp_counts as f64)
                            } else {
                                0.0
                            };
                            let pose = if *tick < *ramp_counts {
                                *entry
                            } else {
                                proprio.positions().discard_wheels()
                            };
                            let kp = JointVec16(std::array::from_fn(|i| entry_kp.0[i] * scale));
                            self.last_commanded_pose = pose;
                            self.last_commanded_kp = kp;
                            let cmd = MitCmd16::from_action_and_gains(&pose, &kp, &stand_kd);
                            let _ = self.safety.filter_and_dispatch(
                                self.hal.as_mut(),
                                &proprio,
                                now_us,
                                false,
                                None,
                                &cmd,
                            );
                            *tick += 1;
                            let mut finished = false;
                            let should_disable = *disable_on_finish;
                            if *tick >= *ramp_counts {
                                *settled_ticks += 1;
                                if *settled_ticks >= *settle_counts {
                                    self.fsm = FsmState::Grounded;
                                    finished = true;
                                }
                            }
                            if !finished {
                                self.transition_task = Some(task);
                            } else if should_disable {
                                let _ = self.safety.disable_motors(self.hal.as_mut(), false);
                            }
                            self.record_history_slice(
                                now_us,
                                &proprio,
                                prev_action,
                                pose,
                                kp,
                                stand_kd,
                                &[],
                            );
                        }
                        TransitionTask::Gesture(player) => {
                            let (pose, done) = player.step();
                            self.last_commanded_pose = pose;
                            self.last_commanded_kp = stand_kp;
                            let cmd = MitCmd16::from_action_and_gains(&pose, &stand_kp, &stand_kd);
                            let g_min = self.safety.config().deploy_action_min;
                            let g_max = self.safety.config().deploy_action_max;
                            let _ = self.safety.filter_and_dispatch(
                                self.hal.as_mut(),
                                &proprio,
                                now_us,
                                true,
                                Some((&g_min, &g_max)),
                                &cmd,
                            );
                            if done {
                                self.fsm = FsmState::Standing;
                            } else {
                                self.transition_task = Some(task);
                            }
                            self.record_history_slice(
                                now_us,
                                &proprio,
                                prev_action,
                                pose,
                                stand_kp,
                                stand_kd,
                                &[],
                            );
                        }
                    }
                } else {
                    self.record_history_slice(
                        now_us,
                        &proprio,
                        prev_action,
                        self.last_commanded_pose,
                        JointVec16::zero(),
                        JointVec16::zero(),
                        &[],
                    );
                }
            }
            FsmState::Standing => {
                // Keep policy history warm while holding standing pose (or running height policy if active)
                let stand_min = self.safety.config().deploy_action_min;
                let stand_max = self.safety.config().deploy_action_max;
                if self.height.supported {
                    let mut height_step: Option<JointVec16> = None;
                    if let Some(pkg) = self.policies.active_mut() {
                        let ctx = PolicyEvalContext::from_proprio(
                            &proprio,
                            WalkCommand::default(),
                            bh_cmd,
                            true,
                        );
                        if let Ok(out) = pkg.step(&ctx) {
                            height_step = Some(out.target_action.discard_wheels());
                        }
                    }
                    if let Some(raw_target) = height_step {
                        let (pose, kp, kd) =
                            self.height.apply_handover(raw_target, infer_kp, infer_kd);
                        self.last_commanded_pose = pose;
                        self.last_commanded_kp = kp;
                        let cmd = MitCmd16::from_action_and_gains(&pose, &kp, &kd);
                        let _ = self.safety.filter_and_dispatch(
                            self.hal.as_mut(),
                            &proprio,
                            now_us,
                            true,
                            Some((&stand_min, &stand_max)),
                            &cmd,
                        );
                        self.record_history_slice(now_us, &proprio, prev_action, pose, kp, kd, &[]);
                        return;
                    }
                }
                let pose = self.last_commanded_pose;
                self.last_commanded_kp = stand_kp;
                let cmd = MitCmd16::from_action_and_gains(&pose, &stand_kp, &stand_kd);
                let _ = self.safety.filter_and_dispatch(
                    self.hal.as_mut(),
                    &proprio,
                    now_us,
                    true,
                    Some((&stand_min, &stand_max)),
                    &cmd,
                );
                self.record_history_slice(
                    now_us,
                    &proprio,
                    prev_action,
                    pose,
                    stand_kp,
                    stand_kd,
                    &[],
                );
            }
            FsmState::Walking => {
                let walk_cmd = self.walk_cmd;
                let entry_pose = self.walk_entry_pose;
                let entry_frame = self.walk_entry_frame;
                let mut bounds_opt: Option<([f64; NUM_JOINTS], [f64; NUM_JOINTS])> = None;
                let mut target_opt: Option<JointVec16> = None;
                self.obs_scratch.clear();

                if let Some(pkg) = self.policies.active_mut() {
                    bounds_opt = Some((
                        pkg.graph.manifest().action_decode.lower_limits,
                        pkg.graph.manifest().action_decode.upper_limits,
                    ));
                    let ctx = PolicyEvalContext::from_proprio(&proprio, walk_cmd, bh_cmd, false);
                    if let Ok(out) = pkg.step(&ctx) {
                        self.obs_scratch
                            .extend_from_slice(pkg.graph.packed_input_f64(0));
                        target_opt = Some(blend_walk_entry(
                            &entry_pose,
                            &out.target_action,
                            entry_frame,
                            25,
                        ));
                        self.walk_entry_frame = self.walk_entry_frame.saturating_add(1);
                    }
                }

                if let Some(blended) = target_opt {
                    let (pose, kp, kd) = self.height.apply_handover(blended, infer_kp, infer_kd);
                    self.last_commanded_pose = pose;
                    self.last_commanded_kp = kp;
                    let cmd = MitCmd16::from_action_and_gains(&pose, &kp, &kd);
                    let b_ref = bounds_opt.as_ref().map(|(l, u)| (l, u));
                    let _ = self.safety.filter_and_dispatch(
                        self.hal.as_mut(),
                        &proprio,
                        now_us,
                        true,
                        b_ref,
                        &cmd,
                    );
                    Self::write_history_slot(
                        &mut self.latest_history,
                        &mut self.history_seq,
                        self.walk_cmd,
                        now_us,
                        &proprio,
                        prev_action,
                        pose,
                        kp,
                        kd,
                        &self.obs_scratch,
                    );
                } else {
                    self.enter_damp();
                    self.record_history_slice(
                        now_us,
                        &proprio,
                        prev_action,
                        self.last_commanded_pose,
                        JointVec16::zero(),
                        stand_kd,
                        &[],
                    );
                }
            }
            FsmState::External | FsmState::Recovery => {}
        }
    }

    fn record_proprio(&mut self, proprio: ProprioFrame) {
        self.latest_proprio = proprio;
        self.has_proprio = true;
        if self.recent_proprio_ring.len() >= 500 {
            self.recent_proprio_ring.pop_front();
        }
        self.recent_proprio_ring.push_back(proprio);
    }

    #[allow(clippy::too_many_arguments)]
    fn write_history_slot(
        latest_history: &mut Option<HistoryFrameSnapshot>,
        history_seq: &mut u64,
        walk_cmd: WalkCommand,
        now_us: u64,
        proprio: &ProprioFrame,
        action: JointVec16,
        next_action: JointVec16,
        kp: JointVec16,
        kd: JointVec16,
        observation: &[f64],
    ) {
        let joint_pos = proprio.positions();
        let joint_vel = proprio.velocities();
        if let Some(h) = latest_history.as_mut() {
            h.monotonic_us = now_us;
            h.gyro_rad_s = proprio.imu.gyro_rad_s;
            h.projected_gravity = proprio.imu.projected_gravity;
            h.walk_cmd = walk_cmd;
            h.joint_pos = joint_pos;
            h.joint_vel = joint_vel;
            h.action = action;
            h.next_action = next_action;
            h.kp = kp;
            h.kd = kd;
            h.observation.clear();
            h.observation.extend_from_slice(observation);
        } else {
            let mut obs = Vec::with_capacity(observation.len().max(600));
            obs.extend_from_slice(observation);
            *latest_history = Some(HistoryFrameSnapshot {
                monotonic_us: now_us,
                gyro_rad_s: proprio.imu.gyro_rad_s,
                projected_gravity: proprio.imu.projected_gravity,
                walk_cmd,
                joint_pos,
                joint_vel,
                action,
                next_action,
                kp,
                kd,
                observation: obs,
            });
        }
        *history_seq = history_seq.wrapping_add(1);
    }

    #[allow(clippy::too_many_arguments)]
    fn record_history_slice(
        &mut self,
        now_us: u64,
        proprio: &ProprioFrame,
        action: JointVec16,
        next_action: JointVec16,
        kp: JointVec16,
        kd: JointVec16,
        observation: &[f64],
    ) {
        Self::write_history_slot(
            &mut self.latest_history,
            &mut self.history_seq,
            self.walk_cmd,
            now_us,
            proprio,
            action,
            next_action,
            kp,
            kd,
            observation,
        );
    }

    fn extract_cmd_vectors(cmd: &MitCmd16) -> (JointVec16, JointVec16, JointVec16) {
        let pose = JointVec16(std::array::from_fn(|i| {
            if i < WHEEL_START_INDEX {
                cmd.0[i].q
            } else {
                cmd.0[i].dq
            }
        }));
        let kp = JointVec16(std::array::from_fn(|i| cmd.0[i].kp));
        let kd = JointVec16(std::array::from_fn(|i| cmd.0[i].kd));
        (pose, kp, kd)
    }

    fn clamp_walk_for_mode(cmd: WalkCommand, mode: SpeedMode) -> WalkCommand {
        let (lin_lim, yaw_lim) = match mode {
            SpeedMode::Normal => (1.0, 1.5),
            SpeedMode::High => (2.5, 2.5),
        };
        let clean = |v: f64| {
            if v == 0.0 {
                0.0
            } else {
                v
            }
        };
        WalkCommand {
            vx: clean(cmd.vx.clamp(-lin_lim, lin_lim)),
            vy: clean(cmd.vy.clamp(-lin_lim, lin_lim)),
            yaw: clean(cmd.yaw.clamp(-yaw_lim, yaw_lim)),
        }
    }

    fn sync_height_spec(&mut self) {
        let spec = self
            .policies
            .active()
            .and_then(|p| p.graph.manifest().capabilities.body_height.clone());
        self.height.configure_from_spec(spec.as_ref());
    }

    fn active_infer_gains(&self) -> (JointVec16, JointVec16) {
        if let Some(pkg) = self.policies.active() {
            let g = &pkg.graph.manifest().gains;
            (JointVec16(g.infer_kp), JointVec16(g.infer_kd))
        } else {
            (JointVec16::zero(), JointVec16::zero())
        }
    }

    fn active_stand_gains(&self) -> (JointVec16, JointVec16) {
        if let Some(pkg) = self.policies.active() {
            let g = &pkg.graph.manifest().gains;
            (JointVec16(g.stand_kp), JointVec16(g.stand_kd))
        } else {
            (JointVec16::zero(), JointVec16::zero())
        }
    }
}
