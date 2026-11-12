use crate::{common::text, rpc, DosoClient, Result};

use super::Tasks;

impl Tasks<'_> {
    /// Watch the current run until terminal or no longer retained by the owner.
    /// Dropping the stream does not cancel the task; Run::state() decodes each update.
    pub async fn watch_run(&self, run_id: &str) -> Result<tonic::Streaming<rpc::Run>> {
        text(run_id, "run_id")?;
        DosoClient::read(
            rpc::tasks_client::TasksClient::new(self.0.channel.clone())
                .watch(self.0.stream_request(rpc::RunQuery {
                    run_id: run_id.into(),
                }))
                .await,
        )
    }
}
