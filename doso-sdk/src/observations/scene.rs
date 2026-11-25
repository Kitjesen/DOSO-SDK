use crate::{rpc, DosoClient, Result};

use super::Observations;

impl Observations<'_> {
    pub async fn scene(&self) -> Result<rpc::Snapshot> {
        DosoClient::read(
            rpc::observations_client::ObservationsClient::new(self.0.channel.clone())
                .scene(self.0.request(rpc::Empty {}))
                .await,
        )
    }
}
