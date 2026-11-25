use std::time::Duration;

use crate::{rpc, CommandReceipt, DosoClient, Result};

use super::Motion;

impl Motion<'_> {
    /// Request standing through native motion arbitration. Observe body state for completion.
    pub async fn stand_up(&self, request_id: Option<&str>) -> Result<CommandReceipt> {
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        // Native posture admission can include a stop barrier before the driver ACK.
        let timeout = self.0.timeout.max(Duration::from_secs(15));
        DosoClient::receipt(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .stand_up(self.0.request_timeout(command, timeout))
                .await,
            &id,
        )
    }

    /// Request sitting through native motion arbitration. Admission is not completion.
    pub async fn sit_down(&self, request_id: Option<&str>) -> Result<CommandReceipt> {
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let timeout = self.0.timeout.max(Duration::from_secs(15));
        DosoClient::receipt(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .sit_down(self.0.request_timeout(command, timeout))
                .await,
            &id,
        )
    }

    /// Alias of [`Motion::sit_down`] matching Unitree Sport semantics (`StandDown`).
    pub async fn stand_down(&self, request_id: Option<&str>) -> Result<CommandReceipt> {
        self.sit_down(request_id).await
    }

    /// Trigger analytic damping soft-stop (`Damp`, Kp=0 on 12 leg axes, wheel braking, -> Grounded).
    pub async fn damp(&self, request_id: Option<&str>) -> Result<CommandReceipt> {
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let timeout = self.0.timeout.max(Duration::from_secs(15));
        DosoClient::receipt(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .damp(self.0.request_timeout(command, timeout))
                .await,
            &id,
        )
    }
}
