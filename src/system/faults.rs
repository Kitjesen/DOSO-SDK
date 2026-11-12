use crate::{rpc, DosoClient, Result};

use super::System;

impl System<'_> {
    /// Known active driver/navigation faults and explicit source coverage.
    pub async fn faults(&self) -> Result<rpc::FaultReport> {
        DosoClient::read(
            rpc::system_client::SystemClient::new(self.0.channel.clone())
                .faults(self.0.request(rpc::Empty {}))
                .await,
        )
    }

    /// Observe fault snapshots at up to 50 Hz. Sources may update more slowly.
    pub async fn watch_faults(&self) -> Result<tonic::Streaming<rpc::FaultReport>> {
        DosoClient::read(
            rpc::system_client::SystemClient::new(self.0.channel.clone())
                .watch_faults(self.0.stream_request(rpc::Empty {}))
                .await,
        )
    }
}
