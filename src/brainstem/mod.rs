//! Deprecated backwards-compatibility shims for onboard Board-A access.
//!
//! New customer code should connect via [`crate::RobotClient`] (serving `:5052`).
//! Onboard diagnostics and bridge processes should import from
//! [`crate::internal::motion_board`].

pub use crate::internal::motion_board::{
    rpc, Certificate, ClientTlsConfig, Identity, DEFAULT_ENDPOINT,
};

pub type BrainstemConfig = crate::internal::motion_board::MotionBoardConfig;
pub type BrainstemClient = crate::internal::motion_board::MotionBoardClient;
pub type BrainstemExternalClient = crate::internal::motion_board::MotionBoardExternalClient;
