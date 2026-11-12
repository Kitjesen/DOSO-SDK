mod camera;
mod cloud;
mod imu;
mod joints;
mod motors;
mod scene;
mod voltages;

use crate::DosoClient;

pub use cloud::{CloudStreamKind, PointCloud};

pub struct Observations<'a>(pub(crate) &'a DosoClient);
