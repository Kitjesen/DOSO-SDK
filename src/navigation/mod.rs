use crate::{
    common::text, rpc, CommandReceipt, DosoClient, Error, Pose, Result, Task, TaskState, Terminal,
};
use std::time::Duration;
mod control;
pub struct Navigation<'a>(pub(crate) &'a DosoClient);
impl Navigation<'_> {
    pub async fn go_to(&self, pose: Pose, request_id: Option<&str>) -> Result<CommandReceipt> {
        pose.validate()?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let receipt = DosoClient::receipt(
            rpc::navigation_client::NavigationClient::new(self.0.channel.clone())
                .go(self.0.request(rpc::Goal {
                    command: Some(command),
                    pose: Some(pose),
                }))
                .await,
            &id,
        )?;
        if receipt.accepted == Some(true) && receipt.task_id.as_deref().is_none_or(str::is_empty) {
            return Err(Error::protocol("accepted navigation goal requires task_id").request(&id));
        }
        Ok(receipt)
    }
    pub async fn status(&self) -> Result<rpc::Snapshot> {
        DosoClient::read(
            rpc::navigation_client::NavigationClient::new(self.0.channel.clone())
                .status(self.0.request(rpc::Empty {}))
                .await,
        )
    }
    pub async fn path(&self) -> Result<rpc::Snapshot> {
        DosoClient::read(
            rpc::navigation_client::NavigationClient::new(self.0.channel.clone())
                .path(self.0.request(rpc::Empty {}))
                .await,
        )
    }
    /// Read cached map-frame XYZ points in metres. Compatibility sources can have default zero heights.
    pub async fn route(&self) -> Result<rpc::NavigationPath> {
        DosoClient::read(
            rpc::navigation_client::NavigationClient::new(self.0.channel.clone())
                .route(self.0.request(rpc::Empty {}))
                .await,
        )
    }
    /// Receive route-cache snapshots at a target maximum of 100 Hz.
    pub async fn watch_path(&self) -> Result<tonic::Streaming<rpc::NavigationPath>> {
        DosoClient::read(
            rpc::navigation_client::NavigationClient::new(self.0.channel.clone())
                .watch_path(self.0.stream_request(rpc::Empty {}))
                .await,
        )
    }
    pub async fn task(&self, task_id: &str) -> Result<Task> {
        text(task_id, "task_id")?;
        let task = DosoClient::read(
            rpc::navigation_client::NavigationClient::new(self.0.channel.clone())
                .task(self.0.request(rpc::TaskQuery {
                    task_id: task_id.into(),
                }))
                .await,
        )?;
        check_task(&task, task_id)?;
        Ok(task)
    }
    pub async fn cancel(
        &self,
        task_id: &str,
        reason: &str,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        text(task_id, "task_id")?;
        text(reason, "reason")?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let receipt = DosoClient::receipt(
            rpc::navigation_client::NavigationClient::new(self.0.channel.clone())
                .cancel(self.0.request(rpc::CancelTask {
                    command: Some(command),
                    task_id: task_id.into(),
                    reason: reason.into(),
                }))
                .await,
            &id,
        )?;
        if receipt
            .task_id
            .as_deref()
            .is_some_and(|returned| returned != task_id)
        {
            return Err(Error::protocol("cancel task identity mismatch").request(&id));
        }
        Ok(receipt)
    }
    pub async fn watch(&self, task_id: &str) -> Result<tonic::Streaming<Task>> {
        text(task_id, "task_id")?;
        DosoClient::read(
            rpc::navigation_client::NavigationClient::new(self.0.channel.clone())
                .watch(self.0.stream_request(rpc::TaskQuery {
                    task_id: task_id.into(),
                }))
                .await,
        )
    }
    /// Local timeout ends subscription waiting and does not cancel the remote task.
    pub async fn wait(&self, task_id: &str, timeout: Duration) -> Result<Task> {
        if timeout.is_zero() {
            return Err(Error::invalid("timeout must be positive"));
        }
        tokio::time::timeout(timeout, async {
            let mut stream = self.watch(task_id).await?;
            while let Some(task) = stream.message().await.map_err(|e| Error::rpc(e, None))? {
                if check_task(&task, task_id)? {
                    return if task.state == TaskState::TaskSuccess as i32 {
                        Ok(task)
                    } else {
                        Err(Error::failed(Terminal::Navigation(task)))
                    };
                }
            }
            Err(Error::protocol(
                "task stream ended before a terminal result",
            ))
        })
        .await
        .map_err(|_| Error::timeout())?
    }
}
fn check_task(task: &Task, task_id: &str) -> Result<bool> {
    if task.task_id != task_id {
        return Err(Error::protocol("task identity mismatch"));
    }
    if !task.found {
        return Ok(false);
    }
    match TaskState::try_from(task.state) {
        Ok(TaskState::TaskSuccess | TaskState::TaskFailed | TaskState::TaskCancelled) => Ok(true),
        Ok(
            TaskState::TaskUnknown
            | TaskState::TaskPlanning
            | TaskState::TaskExecuting
            | TaskState::TaskPaused
            | TaskState::TaskRecovering,
        ) => Ok(false),
        _ => Err(Error::protocol("unknown navigation task state")),
    }
}
