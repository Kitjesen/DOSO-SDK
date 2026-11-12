mod body;
mod control;
mod external;
mod motors;
mod odometry;
mod posture;
mod profile;
mod resume;
mod status;
mod stop;
mod teleop;
mod velocity;
mod watch;

pub use external::ExternalControl;
pub use teleop::Teleop;

use crate::DosoClient;

pub struct Motion<'a>(pub(crate) &'a DosoClient);
