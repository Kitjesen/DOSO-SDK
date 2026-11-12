use crate::{rpc, CommandReceipt, DosoClient, Result};

use super::Motion;

impl Motion<'_> {
    /// Release the current mode's motion hold. Send a fresh goal or velocity afterwards.
    pub async fn resume(&self, request_id: Option<&str>) -> Result<CommandReceipt> {
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        DosoClient::receipt(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .resume(self.0.request(command))
                .await,
            &id,
        )
    }
}
