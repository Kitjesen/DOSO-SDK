use crate::{rpc, DosoClient, Error, Result};

use super::{
    velocity::{freshness, vector},
    Motion,
};

impl Motion<'_> {
    /// Read localization pose in parent_frame, with velocities expressed in child_frame.
    pub async fn odometry(&self) -> Result<rpc::OdometrySample> {
        let sample = DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .odometry(self.0.request(rpc::Empty {}))
                .await,
        )?;
        vector(sample.position_m.as_ref())?;
        vector(sample.linear_velocity_m_s.as_ref())?;
        vector(sample.angular_velocity_rad_s.as_ref())?;
        let orientation = sample
            .orientation
            .as_ref()
            .ok_or_else(|| Error::protocol("odometry orientation is missing"))?;
        let norm = [orientation.w, orientation.x, orientation.y, orientation.z]
            .iter()
            .map(|value| value * value)
            .sum::<f64>();
        if !norm.is_finite() || (norm - 1.0).abs() > 1e-3 {
            return Err(Error::protocol(
                "odometry orientation must be a unit quaternion",
            ));
        }
        if sample.parent_frame.trim().is_empty()
            || sample.child_frame.trim().is_empty()
            || sample.parent_frame == sample.child_frame
            || sample.source.trim().is_empty()
        {
            return Err(Error::protocol("odometry frames or source are invalid"));
        }
        freshness(sample.received_age_ms, sample.fresh)?;
        Ok(sample)
    }

    /// Receive current localization odometry snapshots at a target maximum of 10 Hz.
    pub async fn watch_odometry(&self) -> Result<tonic::Streaming<rpc::OdometrySample>> {
        DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .watch_odometry(self.0.stream_request(rpc::Empty {}))
                .await,
        )
    }
}
