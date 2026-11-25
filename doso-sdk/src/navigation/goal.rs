use crate::{rpc, DosoClient, Error, Pose, Result};

use super::Navigation;

/// Optional per-goal limits. The robot remains responsible for planning and arbitration.
#[derive(Clone, Copy, Debug, Default)]
pub struct GoalOptions {
    pub max_speed_mps: Option<f64>,
    pub acceptance_radius_m: Option<f64>,
}

impl GoalOptions {
    pub(super) fn validate(self) -> Result<()> {
        if let Some(speed) = self.max_speed_mps {
            positive(speed, "max_speed_mps", Some(5.0))?;
        }
        if let Some(radius) = self.acceptance_radius_m {
            positive(radius, "acceptance_radius_m", Some(20.0))?;
        }
        Ok(())
    }
}

pub(super) fn positive(value: f64, name: &str, maximum: Option<f64>) -> Result<()> {
    if !value.is_finite() || value <= 0.0 || maximum.is_some_and(|maximum| value > maximum) {
        return Err(Error::invalid(format!(
            "{name} must be finite, positive and within the supported limit"
        )));
    }
    Ok(())
}

impl Navigation<'_> {
    /// Plan in map-frame XYZ without creating or changing a navigation task.
    pub async fn preview_plan(
        &self,
        pose: Pose,
        acceptance_radius_m: Option<f64>,
    ) -> Result<rpc::PlanPreview> {
        pose.validate()?;
        if let Some(radius) = acceptance_radius_m {
            positive(radius, "acceptance_radius_m", Some(20.0))?;
        }
        DosoClient::read(
            rpc::navigation_client::NavigationClient::new(self.0.channel.clone())
                .preview_plan(self.0.request(rpc::PlanQuery {
                    pose: Some(pose),
                    acceptance_radius_m,
                }))
                .await,
        )
    }
}
