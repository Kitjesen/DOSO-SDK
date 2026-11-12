use crate::{common::text, rpc, CommandReceipt, DosoClient, Error, Result};

use super::Navigation;

impl Navigation<'_> {
    /// Request a pause for this task. Observe its state to confirm the pause.
    pub async fn pause(
        &self,
        task_id: &str,
        reason: &str,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        let request = self.control_request(task_id, reason, request_id)?;
        let id = request.command.as_ref().unwrap().request_id.clone();
        let receipt = DosoClient::receipt(
            rpc::navigation_client::NavigationClient::new(self.0.channel.clone())
                .pause(self.0.request(request))
                .await,
            &id,
        )?;
        check_receipt(receipt, task_id, &id)
    }

    /// Continue the same paused task through the robot's motion guard.
    pub async fn resume(
        &self,
        task_id: &str,
        reason: &str,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        let request = self.control_request(task_id, reason, request_id)?;
        let id = request.command.as_ref().unwrap().request_id.clone();
        let receipt = DosoClient::receipt(
            rpc::navigation_client::NavigationClient::new(self.0.channel.clone())
                .resume(self.0.request(request))
                .await,
            &id,
        )?;
        check_receipt(receipt, task_id, &id)
    }

    fn control_request(
        &self,
        task_id: &str,
        reason: &str,
        request_id: Option<&str>,
    ) -> Result<rpc::TaskControl> {
        text(task_id, "task_id")?;
        text(reason, "reason")?;
        let command = self.0.command(request_id)?;
        if command.request_id == task_id {
            return Err(Error::invalid("request_id must differ from task_id"));
        }
        Ok(rpc::TaskControl {
            command: Some(command),
            task_id: task_id.into(),
            reason: reason.into(),
        })
    }
}

fn check_receipt(
    receipt: CommandReceipt,
    task_id: &str,
    request_id: &str,
) -> Result<CommandReceipt> {
    if receipt.task_id.as_deref().is_some_and(|id| id != task_id)
        || (receipt.accepted == Some(true) && receipt.task_id.as_deref() != Some(task_id))
    {
        return Err(Error::protocol("task control receipt identity mismatch").request(request_id));
    }
    Ok(receipt)
}
