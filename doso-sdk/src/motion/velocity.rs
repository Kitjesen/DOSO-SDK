use crate::{rpc, DosoClient, Error, Result};

use super::Motion;

impl Motion<'_> {
    /// Read localization-estimated linear and angular velocity in the robot body frame.
    pub async fn velocity(&self) -> Result<rpc::VelocitySample> {
        let sample = DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .velocity(self.0.request(rpc::Empty {}))
                .await,
        )?;
        vector(sample.linear_m_s.as_ref())?;
        vector(sample.angular_rad_s.as_ref())?;
        if sample.frame_id.trim().is_empty() || sample.source.trim().is_empty() {
            return Err(Error::protocol("velocity frame or source is missing"));
        }
        freshness(sample.received_age_ms, sample.fresh)?;
        Ok(sample)
    }

    /// Receive current localization velocity snapshots at a target maximum of 100 Hz.
    pub async fn watch_velocity(&self) -> Result<tonic::Streaming<rpc::VelocitySample>> {
        DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .watch_velocity(self.0.stream_request(rpc::Empty {}))
                .await,
        )
    }
}

pub(super) fn vector(value: Option<&rpc::Vector3>) -> Result<()> {
    let value = value.ok_or_else(|| Error::protocol("kinematic vector is missing"))?;
    if ![value.x, value.y, value.z]
        .iter()
        .all(|value| value.is_finite())
    {
        return Err(Error::protocol(
            "kinematic vector contains non-finite coordinates",
        ));
    }
    Ok(())
}

pub(super) fn freshness(age: Option<f64>, fresh: Option<bool>) -> Result<()> {
    if age.is_some_and(|age| !age.is_finite() || age < 0.0) {
        return Err(Error::protocol("kinematic receipt age is invalid"));
    }
    if fresh == Some(false) {
        return Err(Error::protocol("kinematic source returned stale feedback"));
    }
    Ok(())
}
