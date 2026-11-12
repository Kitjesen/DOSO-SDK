use crate::{rpc, DosoClient, Result};

use super::Motion;

impl Motion<'_> {
    /// Read native operator authority. This observer has no session-specific ownership.
    pub async fn control_state(&self) -> Result<rpc::ControlState> {
        DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .control_state(self.0.request(rpc::Empty {}))
                .await,
        )
    }

    /// Observe native authority snapshots at up to 50 Hz, including unknown/stale state.
    pub async fn watch_control_state(&self) -> Result<tonic::Streaming<rpc::ControlState>> {
        DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .watch_control_state(self.0.stream_request(rpc::Empty {}))
                .await,
        )
    }

    /// Read Brainstem's body, motor gate, fault and source ownership status.
    /// This query does not acquire control or identify the SDK caller as owner.
    pub async fn control_status(&self) -> Result<rpc::BodyControlStatus> {
        DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .control_status(self.0.request(rpc::Empty {}))
                .await,
        )
    }
}
