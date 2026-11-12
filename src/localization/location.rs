use crate::{rpc, DosoClient, Result};

use super::Localization;

impl Localization<'_> {
    /// Read the existing GNSS snapshot as WGS84 coordinates and availability.
    pub async fn location(&self) -> Result<rpc::GeographicLocation> {
        DosoClient::read(
            rpc::localization_client::LocalizationClient::new(self.0.channel.clone())
                .location(self.0.request(rpc::Empty {}))
                .await,
        )
    }

    /// Receive GNSS snapshots and availability changes at 1 Hz until dropped.
    pub async fn watch_location(&self) -> Result<tonic::Streaming<rpc::GeographicLocation>> {
        DosoClient::read(
            rpc::localization_client::LocalizationClient::new(self.0.channel.clone())
                .watch_location(self.0.stream_request(rpc::Empty {}))
                .await,
        )
    }
}
