use crate::{rpc, DosoClient, Error, Result};

use super::Motion;

impl Motion<'_> {
    /// Read body-height mode support, current/target height (m), bounds, and rate limit.
    pub async fn body_height_status(&self) -> Result<rpc::BodyHeightStatus> {
        DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .body_height_status(self.0.request(rpc::Empty {}))
                .await,
        )
    }

    /// Set target body height in meters (requires active height-control profile, e.g. `height`).
    pub async fn set_body_height(
        &self,
        height_m: f64,
        request_id: Option<&str>,
    ) -> Result<rpc::BodyHeightStatus> {
        if !height_m.is_finite() {
            return Err(Error::invalid("height_m must be finite"));
        }
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        rpc::motion_client::MotionClient::new(self.0.channel.clone())
            .set_body_height(self.0.request(rpc::SetBodyHeightRequest {
                command: Some(command),
                height_m,
            }))
            .await
            .map(tonic::Response::into_inner)
            .map_err(|e| Error::rpc(e, Some(&id)))
    }
}
