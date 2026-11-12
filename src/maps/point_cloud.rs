use crate::{common::text, rpc, DosoClient, Result};

use super::Maps;

impl Maps<'_> {
    /// Read a saved map as metadata followed by PCD byte chunks.
    pub async fn point_cloud(&self, map_name: &str) -> Result<tonic::Streaming<rpc::MapContent>> {
        text(map_name, "map_name")?;
        DosoClient::read(
            rpc::maps_client::MapsClient::new(self.0.channel.clone())
                .point_cloud(self.0.stream_request(rpc::MapContentRequest {
                    map_name: map_name.into(),
                }))
                .await,
        )
    }
}
