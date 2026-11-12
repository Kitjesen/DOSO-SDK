use crate::{rpc, DosoClient, Result};

use super::Observations;

impl Observations<'_> {
    /// Read motor communication, temperature and diagnostic reports.
    pub async fn motors(&self) -> Result<rpc::MotorStates> {
        DosoClient::read(
            rpc::observations_client::ObservationsClient::new(self.0.channel.clone())
                .motors(self.0.request(rpc::Empty {}))
                .await,
        )
    }
}
