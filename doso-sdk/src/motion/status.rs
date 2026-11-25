use crate::{rpc, DosoClient, Result};

use super::Motion;

impl Motion<'_> {
    /// Read the native navigation/control projection, including its gate reasons.
    pub async fn status(&self) -> Result<rpc::Snapshot> {
        DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .status(self.0.request(rpc::Empty {}))
                .await,
        )
    }
}
