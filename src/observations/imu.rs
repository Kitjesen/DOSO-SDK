use crate::{rpc, DosoClient, Result};

use super::Observations;

impl Observations<'_> {
    /// Receive source IMU events until the stream is dropped.
    pub async fn watch_imu(&self) -> Result<tonic::Streaming<rpc::ImuSample>> {
        DosoClient::read(
            rpc::observations_client::ObservationsClient::new(self.0.channel.clone())
                .watch_imu(self.0.stream_request(rpc::Empty {}))
                .await,
        )
    }
}
