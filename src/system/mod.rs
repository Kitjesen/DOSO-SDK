use crate::{rpc, DosoClient, Result};
mod faults;
mod product;
mod readiness;
mod status;
pub struct System<'a>(pub(crate) &'a DosoClient);
impl<'a> System<'a> {
    /// Identity returned by the initial handshake. Live state uses the query methods.
    pub fn info(&self) -> &'a rpc::SystemInfo {
        &self.0.info
    }

    pub async fn ping(&self) -> Result<rpc::Ping> {
        DosoClient::read(
            rpc::system_client::SystemClient::new(self.0.channel.clone())
                .ping(self.0.request(rpc::Empty {}))
                .await,
        )
    }
    pub async fn state(&self) -> Result<rpc::Snapshot> {
        DosoClient::read(
            rpc::system_client::SystemClient::new(self.0.channel.clone())
                .state(self.0.request(rpc::Empty {}))
                .await,
        )
    }
    pub async fn health(&self) -> Result<rpc::Snapshot> {
        DosoClient::read(
            rpc::system_client::SystemClient::new(self.0.channel.clone())
                .health(self.0.request(rpc::Empty {}))
                .await,
        )
    }
    pub async fn readiness(&self) -> Result<rpc::Snapshot> {
        DosoClient::read(
            rpc::system_client::SystemClient::new(self.0.channel.clone())
                .readiness(self.0.request(rpc::Empty {}))
                .await,
        )
    }
    pub async fn capabilities(&self) -> Result<rpc::Snapshot> {
        DosoClient::read(
            rpc::system_client::SystemClient::new(self.0.channel.clone())
                .capabilities(self.0.request(rpc::Empty {}))
                .await,
        )
    }
    pub async fn session(&self) -> Result<rpc::Snapshot> {
        DosoClient::read(
            rpc::system_client::SystemClient::new(self.0.channel.clone())
                .session(self.0.request(rpc::Empty {}))
                .await,
        )
    }
}
