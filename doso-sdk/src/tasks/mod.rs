use crate::{common::text, rpc, CommandReceipt, DosoClient, Error, Result};
mod inspection;
mod state;
mod watch;
pub struct Tasks<'a>(pub(crate) &'a DosoClient);
impl Tasks<'_> {
    /// Instructions may start motion through the task owner.
    pub async fn send_instruction(
        &self,
        instruction: &str,
        run_id: Option<&str>,
        control_only: bool,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        text(instruction, "text")?;
        if let Some(run_id) = run_id {
            text(run_id, "run_id")?;
        }
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let request = self.0.request(rpc::Instruction {
            command: Some(command),
            text: instruction.into(),
            run_id: run_id.map(str::to_owned),
            control_only,
        });
        DosoClient::receipt(
            rpc::tasks_client::TasksClient::new(self.0.channel.clone())
                .send(request)
                .await,
            &id,
        )
    }
    pub async fn get_run(&self, run_id: &str) -> Result<rpc::Run> {
        text(run_id, "run_id")?;
        let run = DosoClient::read(
            rpc::tasks_client::TasksClient::new(self.0.channel.clone())
                .get(self.0.request(rpc::RunQuery {
                    run_id: run_id.into(),
                }))
                .await,
        )?;
        if run.run_id != run_id {
            return Err(Error::protocol("run identity mismatch"));
        }
        Ok(run)
    }
    pub async fn cancel_run(
        &self,
        run_id: &str,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        text(run_id, "run_id")?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let receipt = DosoClient::receipt(
            rpc::tasks_client::TasksClient::new(self.0.channel.clone())
                .cancel(self.0.request(rpc::CancelRun {
                    command: Some(command),
                    run_id: run_id.into(),
                }))
                .await,
            &id,
        )?;
        if receipt
            .run_id
            .as_deref()
            .is_some_and(|returned| returned != run_id)
        {
            return Err(Error::protocol("cancel run identity mismatch").request(&id));
        }
        Ok(receipt)
    }
}
