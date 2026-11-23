#![forbid(unsafe_code)]

pub mod fsm;
pub mod kinematics;
pub mod types;

pub use fsm::{BodyTransitionKind, ControlArbiter, ControlOwner, FsmState};
pub use kinematics::{
    flat_index_to_leg_can, leg_can_to_flat_index, LegId, MotorModel, MotorProtocolLimits,
    MotorRole, NUM_JOINTS, NUM_LEGS, NUM_WHEELS, WHEEL_START_INDEX,
};
pub use types::{
    ImuState, JointFeedback, JointVec16, MitCmd16, MitTarget, ProprioFrame, RemotePadState,
    WalkCommand,
};
