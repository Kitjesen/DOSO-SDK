use crate::{rpc, CommandReceipt, DosoClient, Result};

use super::Motion;

impl Motion<'_> {
    /// Request a motion hold without latching the emergency stop.
    pub async fn stop(&self, request_id: Option<&str>) -> Result<CommandReceipt> {
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        DosoClient::receipt(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .stop_motion(self.0.request(command))
                .await,
            &id,
        )
    }

    /// Admission does not prove physical standstill.
    pub async fn emergency_stop(&self, request_id: Option<&str>) -> Result<CommandReceipt> {
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        DosoClient::receipt(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .stop(self.0.request(command))
                .await,
            &id,
        )
    }

    /// Release the software emergency-stop latch without resuming motion.
    pub async fn clear_emergency_stop(&self, request_id: Option<&str>) -> Result<CommandReceipt> {
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        DosoClient::receipt(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .clear_emergency_stop(self.0.request(command))
                .await,
            &id,
        )
    }
}
