use crate::{rpc, DosoClient, Result};

use super::System;

impl System<'_> {
    /// Read robot mode, readiness and known software-stop state.
    /// Unknown health, hardware-stop and source time remain unset/unknown.
    pub async fn robot_status(&self) -> Result<rpc::RobotStatus> {
        DosoClient::read(
            rpc::system_client::SystemClient::new(self.0.channel.clone())
                .robot_status(self.0.request(rpc::Empty {}))
                .await,
        )
    }
}
