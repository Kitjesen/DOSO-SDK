use prost_types::{value::Kind, Struct};

use crate::{rpc, Error, Result};

use super::Tasks;

impl Tasks<'_> {
    /// Read the current AgentPlanner state, summary and failure reason.
    /// Succeeded reflects task-owner completion; motion evidence belongs to its domain.
    /// Missing Agent support or a run that is no longer current returns None.
    pub async fn get_run_state(&self, run_id: &str) -> Result<Option<rpc::TaskRunState>> {
        self.get_run(run_id).await?.state()
    }
}

impl rpc::Run {
    /// Decode the task owner's state from either a query or a watched run.
    pub fn state(&self) -> Result<Option<rpc::TaskRunState>> {
        if !self.found {
            return Ok(None);
        }
        let fields = self
            .details
            .as_ref()
            .ok_or_else(|| Error::protocol("task run has no details"))?;
        let status = match string(fields, "state")? {
            "running" => rpc::TaskRunStatus::Running,
            "done" => rpc::TaskRunStatus::Succeeded,
            "failed" => rpc::TaskRunStatus::Failed,
            "cancelled" => rpc::TaskRunStatus::Cancelled,
            _ => return Err(Error::protocol("unsupported task run state")),
        };
        Ok(Some(rpc::TaskRunState {
            run_id: self.run_id.clone(),
            status: status as i32,
            reason: string(fields, "failure_reason")?.into(),
            result: string(fields, "summary")?.into(),
            ..Default::default()
        }))
    }
}

fn string<'a>(fields: &'a Struct, name: &str) -> Result<&'a str> {
    match fields
        .fields
        .get(name)
        .and_then(|value| value.kind.as_ref())
    {
        Some(Kind::StringValue(value)) => Ok(value),
        None | Some(Kind::NullValue(_)) => Ok(""),
        _ => Err(Error::protocol(format!("task run {name} must be a string"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_types::Value;

    fn run(state: &str) -> rpc::Run {
        rpc::Run {
            run_id: "run-1".into(),
            found: true,
            details: Some(Struct {
                fields: [
                    (
                        "state".into(),
                        Value {
                            kind: Some(Kind::StringValue(state.into())),
                        },
                    ),
                    (
                        "summary".into(),
                        Value {
                            kind: Some(Kind::StringValue("owner summary".into())),
                        },
                    ),
                    (
                        "failure_reason".into(),
                        Value {
                            kind: Some(Kind::StringValue("owner reason".into())),
                        },
                    ),
                ]
                .into(),
            }),
        }
    }

    #[test]
    fn preserves_owner_states_and_summary() {
        for (name, status) in [
            ("running", rpc::TaskRunStatus::Running),
            ("done", rpc::TaskRunStatus::Succeeded),
            ("failed", rpc::TaskRunStatus::Failed),
            ("cancelled", rpc::TaskRunStatus::Cancelled),
        ] {
            let state = run(name).state().unwrap().unwrap();
            assert_eq!(state.status, status as i32);
            assert_eq!(state.reason, "owner reason");
            assert_eq!(state.result, "owner summary");
            assert!(state.task_type.is_empty());
            assert!(state.phase.is_empty());
            assert!(state.reason_code.is_empty());
            assert_eq!(state.progress, None);
            assert_eq!(state.time, None);
        }
    }

    #[test]
    fn missing_run_is_absent_and_malformed_run_fails() {
        assert!(rpc::Run::default().state().unwrap().is_none());
        assert!(run("accepted").state().is_err());
        assert!(rpc::Run {
            found: true,
            ..Default::default()
        }
        .state()
        .is_err());
        let mut malformed = run("running");
        malformed.details.as_mut().unwrap().fields.insert(
            "summary".into(),
            Value {
                kind: Some(Kind::BoolValue(true)),
            },
        );
        assert!(malformed.state().is_err());
    }
}
