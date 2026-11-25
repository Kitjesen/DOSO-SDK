use crate::{rpc, DosoClient, Result};

use super::Motion;

impl Motion<'_> {
    /// Subscribe until the stream is dropped. Closing it ends observation only.
    pub async fn watch_state(&self) -> Result<tonic::Streaming<rpc::Snapshot>> {
        DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .watch_state(self.0.stream_request(rpc::Empty {}))
                .await,
        )
    }

    /// Subscribe at a supported Gateway update rate; this does not change sensor sampling.
    pub async fn watch_state_at(
        &self,
        frequency_hz: u32,
    ) -> Result<tonic::Streaming<rpc::Snapshot>> {
        DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .watch_state_at(
                    self.0
                        .stream_request(rpc::WatchStateRequest { frequency_hz }),
                )
                .await,
        )
    }
}
