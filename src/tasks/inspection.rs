use prost_types::{value::Kind, Struct};

use crate::{common::text, rpc, CommandReceipt, DosoClient, Error, Result};

use super::Tasks;

impl Tasks<'_> {
    pub async fn inspection_routes(&self, map_id: &str) -> Result<rpc::InspectionRoutes> {
        text(map_id, "map_id")?;
        let routes = DosoClient::read(
            rpc::tasks_client::TasksClient::new(self.0.channel.clone())
                .inspection_routes(self.0.request(rpc::InspectionRouteQuery {
                    map_id: map_id.into(),
                    route_id: String::new(),
                }))
                .await,
        )?;
        if routes.map_id != map_id {
            return Err(Error::protocol("inspection routes map identity mismatch"));
        }
        for route in &routes.routes {
            validate_route(route, map_id, &route.route_id)?;
        }
        Ok(routes)
    }

    pub async fn inspection_route(
        &self,
        map_id: &str,
        route_id: &str,
    ) -> Result<rpc::InspectionRoute> {
        text(map_id, "map_id")?;
        text(route_id, "route_id")?;
        let route = DosoClient::read(
            rpc::tasks_client::TasksClient::new(self.0.channel.clone())
                .inspection_route(self.0.request(rpc::InspectionRouteQuery {
                    map_id: map_id.into(),
                    route_id: route_id.into(),
                }))
                .await,
        )?;
        validate_route(&route, map_id, route_id)?;
        Ok(route)
    }

    /// Store the existing inspection route schema; revision and map epoch are checked by its owner.
    pub async fn put_inspection_route(
        &self,
        route: Struct,
        request_id: Option<&str>,
    ) -> Result<rpc::InspectionRoute> {
        let map_id = route_text(&route, "map_id")?.to_owned();
        let route_id = route_text(&route, "id")?.to_owned();
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let result = rpc::tasks_client::TasksClient::new(self.0.channel.clone())
            .put_inspection_route(self.0.request(rpc::PutInspectionRoute {
                command: Some(command),
                route: Some(route),
            }))
            .await
            .map_err(|error| Error::rpc(error, Some(&id)))?
            .into_inner();
        if result.request_id != id {
            return Err(Error::protocol("inspection route receipt identity mismatch").request(&id));
        }
        validate_route(&result, &map_id, &route_id).map_err(|error| error.request(&id))?;
        Ok(result)
    }

    pub async fn delete_inspection_route(
        &self,
        map_id: &str,
        route_id: &str,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        text(map_id, "map_id")?;
        text(route_id, "route_id")?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        DosoClient::receipt(
            rpc::tasks_client::TasksClient::new(self.0.channel.clone())
                .delete_inspection_route(self.0.request(rpc::DeleteInspectionRoute {
                    command: Some(command),
                    map_id: map_id.into(),
                    route_id: route_id.into(),
                }))
                .await,
            &id,
        )
    }

    /// Submit once. A successful receipt means admission, not inspection completion.
    pub async fn start_inspection(
        &self,
        map_id: &str,
        route_id: &str,
        revision: u64,
        binding: rpc::InspectionBinding,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        text(map_id, "map_id")?;
        text(route_id, "route_id")?;
        validate_binding(&binding)?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let receipt = DosoClient::receipt(
            rpc::tasks_client::TasksClient::new(self.0.channel.clone())
                .start_inspection(self.0.request(rpc::StartInspection {
                    command: Some(command),
                    map_id: map_id.into(),
                    route_id: route_id.into(),
                    revision,
                    binding: Some(binding),
                }))
                .await,
            &id,
        )?;
        if receipt.ok && receipt.task_id.as_deref().is_none_or(str::is_empty) {
            return Err(Error::protocol("inspection receipt has no task identity").request(&id));
        }
        Ok(receipt)
    }

    pub async fn inspection_task(&self, task_id: &str) -> Result<rpc::InspectionTask> {
        text(task_id, "task_id")?;
        let task = DosoClient::read(
            rpc::tasks_client::TasksClient::new(self.0.channel.clone())
                .inspection_task(self.0.request(rpc::InspectionTaskQuery {
                    task_id: task_id.into(),
                }))
                .await,
        )?;
        if task.task_id != task_id {
            return Err(Error::protocol("inspection task identity mismatch"));
        }
        Ok(task)
    }

    /// List retained inspection projections, optionally including terminal tasks.
    /// This is the inspection owner's bounded history, not Agent run history.
    pub async fn inspection_tasks(
        &self,
        map_id: Option<&str>,
        route_id: Option<&str>,
        include_terminal: bool,
        limit: Option<u32>,
    ) -> Result<rpc::InspectionTasks> {
        if let Some(map_id) = map_id {
            text(map_id, "map_id")?;
        }
        if let Some(route_id) = route_id {
            text(route_id, "route_id")?;
        }
        let result = DosoClient::read(
            rpc::tasks_client::TasksClient::new(self.0.channel.clone())
                .inspection_tasks(self.0.request(rpc::InspectionTaskListQuery {
                    map_id: map_id.map(str::to_owned),
                    route_id: route_id.map(str::to_owned),
                    include_terminal,
                    limit,
                }))
                .await,
        )?;
        if result.count as usize != result.tasks.len()
            || result
                .tasks
                .iter()
                .any(|task| task.task_id.trim().is_empty())
        {
            return Err(Error::protocol(
                "invalid inspection task list count or identity",
            ));
        }
        Ok(result)
    }

    /// Observe 50 Hz task snapshots until terminal or not found. Dropping does not cancel work.
    pub async fn watch_inspection(
        &self,
        task_id: &str,
    ) -> Result<tonic::Streaming<rpc::InspectionTask>> {
        text(task_id, "task_id")?;
        DosoClient::read(
            rpc::tasks_client::TasksClient::new(self.0.channel.clone())
                .watch_inspection(self.0.stream_request(rpc::InspectionTaskQuery {
                    task_id: task_id.into(),
                }))
                .await,
        )
    }

    pub async fn pause_inspection(
        &self,
        task_id: &str,
        reason: &str,
        binding: rpc::InspectionBinding,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        self.control_inspection(task_id, reason, binding, request_id, "pause")
            .await
    }

    pub async fn resume_inspection(
        &self,
        task_id: &str,
        reason: &str,
        binding: rpc::InspectionBinding,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        self.control_inspection(task_id, reason, binding, request_id, "resume")
            .await
    }

    pub async fn cancel_inspection(
        &self,
        task_id: &str,
        reason: &str,
        binding: rpc::InspectionBinding,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        self.control_inspection(task_id, reason, binding, request_id, "cancel")
            .await
    }

    async fn control_inspection(
        &self,
        task_id: &str,
        reason: &str,
        binding: rpc::InspectionBinding,
        request_id: Option<&str>,
        action: &str,
    ) -> Result<CommandReceipt> {
        text(task_id, "task_id")?;
        text(reason, "reason")?;
        validate_binding(&binding)?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let request = self.0.request(rpc::InspectionControl {
            command: Some(command),
            task_id: task_id.into(),
            reason: reason.into(),
            binding: Some(binding),
        });
        let mut client = rpc::tasks_client::TasksClient::new(self.0.channel.clone());
        let response = match action {
            "pause" => client.pause_inspection(request).await,
            "resume" => client.resume_inspection(request).await,
            "cancel" => client.cancel_inspection(request).await,
            _ => unreachable!("private inspection action"),
        };
        let receipt = DosoClient::receipt(response, &id)?;
        if receipt.task_id.as_deref().is_some_and(|id| id != task_id)
            || receipt.ok && receipt.task_id.as_deref() != Some(task_id)
        {
            return Err(Error::protocol("inspection control task identity mismatch").request(&id));
        }
        Ok(receipt)
    }

    pub async fn inspection_report(&self, task_id: &str) -> Result<rpc::Snapshot> {
        text(task_id, "task_id")?;
        DosoClient::read(
            rpc::tasks_client::TasksClient::new(self.0.channel.clone())
                .inspection_report(self.0.request(rpc::InspectionTaskQuery {
                    task_id: task_id.into(),
                }))
                .await,
        )
    }
}

fn validate_route(route: &rpc::InspectionRoute, map_id: &str, route_id: &str) -> Result<()> {
    if route.map_id != map_id
        || route.route_id != route_id
        || route.route_id.is_empty()
        || route.revision == 0
        || route.details.is_none()
    {
        return Err(Error::protocol(
            "invalid inspection route identity or details",
        ));
    }
    Ok(())
}

fn route_text<'a>(route: &'a Struct, name: &str) -> Result<&'a str> {
    match route.fields.get(name).and_then(|value| value.kind.as_ref()) {
        Some(Kind::StringValue(value)) => {
            text(value, name)?;
            Ok(value)
        }
        _ => Err(Error::invalid(format!(
            "inspection route {name} must be a string"
        ))),
    }
}

fn validate_binding(binding: &rpc::InspectionBinding) -> Result<()> {
    text(&binding.product_session_id, "product_session_id")?;
    if !matches!(binding.env.as_str(), "" | "real" | "sim") {
        return Err(Error::invalid("inspection env must be real or sim"));
    }
    Ok(())
}
