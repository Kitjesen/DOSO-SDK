use std::time::Duration;

use crate::{rpc, CommandReceipt, DosoClient, Result};

use super::Motion;

impl Motion<'_> {
    /// Enable the motor output gate through native arbitration. Standing is a separate action.
    pub async fn enable_motors(&self, request_id: Option<&str>) -> Result<CommandReceipt> {
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let timeout = self.0.timeout.max(Duration::from_secs(15));
        DosoClient::receipt(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .enable_motors(self.0.request_timeout(command, timeout))
                .await,
            &id,
        )
    }

    /// Disable the output gate and revoke the driver lease. Does not request sitting.
    pub async fn disable_motors(&self, request_id: Option<&str>) -> Result<CommandReceipt> {
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let timeout = self.0.timeout.max(Duration::from_secs(15));
        DosoClient::receipt(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .disable_motors(self.0.request_timeout(command, timeout))
                .await,
            &id,
        )
    }
}
