use crate::{rpc, DosoClient, Result};

use super::Observations;

impl Observations<'_> {
    /// Query motor bus voltages in joint-index order; zero can mean no response.
    pub async fn motor_voltages(&self) -> Result<rpc::MotorVoltagesResponse> {
        DosoClient::read(
            rpc::observations_client::ObservationsClient::new(self.0.channel.clone())
                .motor_voltages(self.0.request(rpc::Empty {}))
                .await,
        )
    }
}
