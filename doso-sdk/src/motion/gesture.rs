use std::time::Duration;

use crate::{common::text, rpc, CommandReceipt, Result, RobotClient};

use super::Motion;

impl Motion<'_> {
    /// List available onboard gestures.
    pub async fn list_gestures(&self) -> Result<rpc::GestureList> {
        RobotClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .list_gestures(self.0.request(rpc::Empty {}))
                .await,
        )
    }

    /// Trigger a named onboard gesture (admitted by native arbitration while standing or grounded).
    pub async fn play_gesture(
        &self,
        name: &str,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        text(name, "gesture name")?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let timeout = self.0.timeout.max(Duration::from_secs(15));
        RobotClient::receipt(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .play_gesture(self.0.request_timeout(
                    rpc::PlayGestureRequest {
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
