use std::time::Duration;

use crate::{common::text, rpc, CommandReceipt, DosoClient, Result};

use super::Motion;

impl Motion<'_> {
    /// Read the current profile and available names without acquiring control.
    pub async fn profile(&self) -> Result<rpc::ProfileInfo> {
        DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .profile(self.0.request(rpc::Empty {}))
                .await,
        )
    }

    /// Switch while grounded, disabled and free of control ownership. Read profile afterwards.
    pub async fn switch_profile(
        &self,
        name: &str,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        text(name, "profile name")?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let timeout = self.0.timeout.max(Duration::from_secs(15));
        DosoClient::receipt(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .switch_profile(self.0.request_timeout(
                    rpc::SwitchProfileRequest {
                        command: Some(command),
                        name: name.into(),
                    },
                    timeout,
                ))
                .await,
            &id,
        )
    }
}
