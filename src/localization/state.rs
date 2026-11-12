use crate::{rpc, DosoClient, Result};

use super::Localization;

impl Localization<'_> {
    /// Read the existing localization owner's state and known readiness flags.
    pub async fn state(&self) -> Result<rpc::LocalizationState> {
        DosoClient::read(
            rpc::localization_client::LocalizationClient::new(self.0.channel.clone())
                .state(self.0.request(rpc::Empty {}))
                .await,
        )
    }
}
