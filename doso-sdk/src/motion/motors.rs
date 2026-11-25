use std::time::Duration;

use crate::{rpc, CommandReceipt, DosoClient, Error, Result};

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

    /// Clear motor faults via the unified robot service. An empty list clears all joints.
    pub async fn clear_motor_fault(
        &self,
        joint_ids: impl IntoIterator<Item = u32>,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        let joint_ids = joint_ids.into_iter().collect::<Vec<_>>();
        if joint_ids.iter().any(|id| *id >= 16) {
            return Err(Error::invalid("joint id must be between 0 and 15"));
        }
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let timeout = self.0.timeout.max(Duration::from_secs(15));
        DosoClient::receipt(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .clear_motor_fault(self.0.request_timeout(
                    rpc::ClearMotorFaultRequest {
                        command: Some(command),
                        joint_ids,
                    },
                    timeout,
                ))
                .await,
            &id,
        )
    }
}
