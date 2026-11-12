use crate::{rpc, DosoClient, Result};

use super::Observations;

impl Observations<'_> {
    /// Receive each upstream single-joint event or full joint frame.
    pub async fn watch_joints(&self) -> Result<tonic::Streaming<rpc::JointSample>> {
        DosoClient::read(
            rpc::observations_client::ObservationsClient::new(self.0.channel.clone())
                .watch_joints(self.0.stream_request(rpc::Empty {}))
                .await,
        )
    }
}
