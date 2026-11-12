use crate::{rpc, DosoClient, Result};

use super::Motion;

impl Motion<'_> {
    /// Read the source body's current finite-state-machine state.
    pub async fn body_state(&self) -> Result<rpc::BodyState> {
        DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .body_state(self.0.request(rpc::Empty {}))
                .await,
        )
    }

    /// Receive the initial body state followed by source state events.
    pub async fn watch_body_state(&self) -> Result<tonic::Streaming<rpc::BodyState>> {
        DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .watch_body_state(self.0.stream_request(rpc::Empty {}))
                .await,
        )
    }
}
