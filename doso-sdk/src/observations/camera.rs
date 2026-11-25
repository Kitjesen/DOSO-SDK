use crate::{rpc, DosoClient, Error, Result};
use tonic::{metadata::MetadataValue, Request};

use super::Observations;

impl Observations<'_> {
    /// Read source color-image calibration, before JPEG preview transformations.
    pub async fn camera_calibration(
        &self,
        viewer_token: Option<&str>,
    ) -> Result<rpc::CameraCalibration> {
        let mut request = self.camera_request(viewer_token)?;
        request.set_timeout(self.0.timeout);
        DosoClient::read(
            rpc::observations_client::ObservationsClient::new(self.0.channel.clone())
                .camera_calibration(request)
                .await,
        )
    }

    pub async fn camera_status(&self) -> Result<rpc::Snapshot> {
        DosoClient::read(
            rpc::observations_client::ObservationsClient::new(self.0.channel.clone())
                .camera_status(self.0.request(rpc::Empty {}))
                .await,
        )
    }

    /// Read an available JPEG snapshot from the robot's camera service.
    pub async fn camera_frame(&self, viewer_token: Option<&str>) -> Result<rpc::CameraFrame> {
        let mut request = self.camera_request(viewer_token)?;
        request.set_timeout(self.0.timeout);
        DosoClient::read(
            rpc::observations_client::ObservationsClient::new(self.0.channel.clone())
                .camera_frame(request)
                .await,
        )
    }

    /// Receive JPEG frames published after subscription until the stream ends or stalls.
    pub async fn watch_camera(
        &self,
        viewer_token: Option<&str>,
    ) -> Result<tonic::Streaming<rpc::CameraFrame>> {
        DosoClient::read(
            rpc::observations_client::ObservationsClient::new(self.0.channel.clone())
                .watch_camera(self.camera_request(viewer_token)?)
                .await,
        )
    }

    /// Read a JPEG snapshot with available source metadata.
    pub async fn camera_sample(&self, viewer_token: Option<&str>) -> Result<rpc::CameraSample> {
        let mut request = self.camera_request(viewer_token)?;
        request.set_timeout(self.0.timeout);
        DosoClient::read(
            rpc::observations_client::ObservationsClient::new(self.0.channel.clone())
                .camera_sample(request)
                .await,
        )
    }

    /// Receive newly published JPEG frames with the robot service publication counter.
    pub async fn watch_camera_samples(
        &self,
        viewer_token: Option<&str>,
    ) -> Result<tonic::Streaming<rpc::CameraSample>> {
        DosoClient::read(
            rpc::observations_client::ObservationsClient::new(self.0.channel.clone())
                .watch_camera_samples(self.camera_request(viewer_token)?)
                .await,
        )
    }

    fn camera_request(&self, viewer_token: Option<&str>) -> Result<Request<rpc::Empty>> {
        let mut request = self.0.stream_request(rpc::Empty {});
        if let Some(token) = viewer_token {
            if token.is_empty() {
                return Err(Error::invalid("media viewer token must not be empty"));
            }
            let mut authorization = MetadataValue::try_from(format!("Bearer {token}"))
                .map_err(|_| Error::invalid("invalid media viewer token metadata"))?;
            authorization.set_sensitive(true);
            request
                .metadata_mut()
                .insert("authorization", authorization);
        }
        Ok(request)
    }
}
