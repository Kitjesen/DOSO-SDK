use crate::{common::text, rpc, CommandReceipt, DosoClient, Error, Pose, Result};
use std::time::Duration;
mod location;
mod state;
pub struct Localization<'a>(pub(crate) &'a DosoClient);
impl Localization<'_> {
    /// Receive map-pose cache snapshots at a target maximum of 100 Hz.
    pub async fn watch_pose(&self) -> Result<tonic::Streaming<rpc::PoseSample>> {
        DosoClient::read(
            rpc::localization_client::LocalizationClient::new(self.0.channel.clone())
                .watch_pose(self.0.stream_request(rpc::Empty {}))
                .await,
        )
    }
    pub async fn status(&self) -> Result<rpc::Snapshot> {
        DosoClient::read(
            rpc::localization_client::LocalizationClient::new(self.0.channel.clone())
                .status(self.0.request(rpc::Empty {}))
                .await,
        )
    }
    pub async fn pose(&self) -> Result<rpc::PoseSample> {
        let sample = DosoClient::read(
            rpc::localization_client::LocalizationClient::new(self.0.channel.clone())
                .pose(self.0.request(rpc::Empty {}))
                .await,
        )?;
        if sample.frame_id != "map" {
            return Err(Error::protocol("pose is not in map frame"));
        }
        sample
            .pose
            .as_ref()
            .ok_or_else(|| Error::protocol("map pose is unavailable"))?
            .validate()
            .map_err(|_| Error::protocol("map pose contains non-finite coordinates"))?;
        Ok(sample)
    }
    pub async fn relocalize(
        &self,
        map_name: &str,
        initial_pose: Option<Pose>,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        text(map_name, "map_name")?;
        if let Some(pose) = &initial_pose {
            pose.validate()?;
        }
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let request = self.0.request_timeout(
            rpc::Relocalize {
                command: Some(command),
                map_name: map_name.into(),
                initial_pose,
            },
            Duration::from_secs(60),
        );
        DosoClient::receipt(
            rpc::localization_client::LocalizationClient::new(self.0.channel.clone())
                .relocalize(request)
                .await,
            &id,
        )
    }
}
