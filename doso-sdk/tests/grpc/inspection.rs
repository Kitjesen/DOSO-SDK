use super::*;

pub(super) fn route(service: &Service, map_id: &str, route_id: &str) -> rpc::InspectionRoute {
    rpc::InspectionRoute {
        map_id: map_id.into(),
        route_id: if service.mode.load(Ordering::Relaxed) == 160 {
            "other-route".into()
        } else {
            route_id.into()
        },
        revision: 7,
        details: Some(route_fields()),
        request_id: String::new(),
    }
}

pub(super) fn task(service: &Service, task_id: &str, terminal: bool) -> rpc::InspectionTask {
    rpc::InspectionTask {
        task_id: if service.mode.load(Ordering::Relaxed) == 161 {
            "other-task".into()
        } else {
            task_id.into()
        },
        found: true,
        current_state: if terminal { "SUCCESS" } else { "EXECUTING" }.into(),
        state_available: true,
        terminal,
        execution_confirmed: true,
        details: Some(evidence(task_id)),
        ..Default::default()
    }
}

pub(super) fn control(
    service: &Service,
    request: rpc::InspectionControl,
) -> Result<Response<rpc::Receipt>, Status> {
    assert_eq!(request.task_id, "task-1");
    assert!(!request.reason.is_empty());
    assert_eq!(request.binding.unwrap().product_session_id, "session-1");
    let mut reply = service.receipt(request.command.unwrap())?;
    if service.mode.load(Ordering::Relaxed) == 164 {
        reply.get_mut().task_id = Some("other-task".into());
    }
    Ok(reply)
}

fn route_fields() -> Struct {
    Struct {
        fields: [
            (
                "id".into(),
                Value {
                    kind: Some(Kind::StringValue("route-1".into())),
                },
            ),
            (
                "map_id".into(),
                Value {
                    kind: Some(Kind::StringValue("map-1".into())),
                },
            ),
        ]
        .into(),
    }
}

fn binding() -> rpc::InspectionBinding {
    rpc::InspectionBinding {
        product_session_id: "session-1".into(),
        env: "sim".into(),
    }
}

#[tokio::test]
async fn inspection_route_crud_preserves_identity() {
    let fixture = Fixture::new().await;
    let tasks = fixture.client.tasks();
    let routes = tasks.inspection_routes("map-1").await.unwrap();
    assert_eq!(routes.routes[0].route_id, "route-1");
    assert_eq!(
        tasks
            .inspection_route("map-1", "route-1")
            .await
            .unwrap()
            .revision,
        7
    );
    let stored = tasks
        .put_inspection_route(route_fields(), Some("put-1"))
        .await
        .unwrap();
    assert_eq!(stored.request_id, "put-1");
    assert_eq!(stored.route_id, "route-1");
    let deleted = tasks
        .delete_inspection_route("map-1", "route-1", Some("delete-1"))
        .await
        .unwrap();
    assert_eq!(deleted.request_id, "delete-1");
    assert!(deleted.ok);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn inspection_lifecycle_preserves_request_and_task_identity() {
    let fixture = Fixture::new().await;
    let tasks = fixture.client.tasks();
    let start = tasks
        .start_inspection("map-1", "route-1", 7, binding(), Some("start-1"))
        .await
        .unwrap();
    assert_eq!(start.task_id.as_deref(), Some("task-1"));
    assert_eq!(start.accepted, Some(true));
    for receipt in [
        tasks
            .pause_inspection("task-1", "operator", binding(), Some("pause-1"))
            .await
            .unwrap(),
        tasks
            .resume_inspection("task-1", "operator", binding(), Some("resume-1"))
            .await
            .unwrap(),
        tasks
            .cancel_inspection("task-1", "operator", binding(), Some("cancel-1"))
            .await
            .unwrap(),
    ] {
        assert_eq!(receipt.task_id.as_deref(), Some("task-1"));
        assert_eq!(receipt.accepted, Some(true));
    }
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 4);
}

#[tokio::test]
async fn inspection_queries_and_stream_keep_terminal_fact_without_commands() {
    let fixture = Fixture::new().await;
    let tasks = fixture.client.tasks();
    let state = tasks.inspection_task("task-1").await.unwrap();
    assert!(state.found && state.execution_confirmed && !state.terminal);
    let mut stream = tasks.watch_inspection("task-1").await.unwrap();
    assert!(!stream.message().await.unwrap().unwrap().terminal);
    let terminal = stream.message().await.unwrap().unwrap();
    assert!(terminal.terminal);
    assert_eq!(terminal.current_state, "SUCCESS");
    assert!(stream.message().await.unwrap().is_none());
    assert!(tasks
        .inspection_report("task-1")
        .await
        .unwrap()
        .fields
        .is_some());
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn inspection_invalid_identity_and_binding_do_not_submit() {
    let fixture = Fixture::new().await;
    let tasks = fixture.client.tasks();
    assert_eq!(
        tasks.inspection_routes(" ").await.unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(
        tasks.inspection_route("map-1", "").await.unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(
        tasks
            .put_inspection_route(Struct::default(), None)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(
        tasks
            .delete_inspection_route("", "route-1", None)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(
        tasks
            .start_inspection(
                "map-1",
                "route-1",
                7,
                rpc::InspectionBinding::default(),
                None
            )
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    );
    let mut wrong_env = binding();
    wrong_env.env = "cloud".into();
    assert_eq!(
        tasks
            .resume_inspection("task-1", "operator", wrong_env, None)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(
        tasks
            .pause_inspection("task-1", "", binding(), None)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(
        tasks.inspection_task("").await.unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(
        tasks.watch_inspection("").await.unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(
        tasks.inspection_report("").await.unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn inspection_rejects_mismatched_route_and_task_replies() {
    let fixture = Fixture::new().await;
    fixture.mode(160);
    assert_eq!(
        fixture
            .client
            .tasks()
            .inspection_route("map-1", "route-1")
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Protocol
    );
    let error = fixture
        .client
        .tasks()
        .put_inspection_route(route_fields(), Some("put-identity"))
        .await
        .unwrap_err();
    assert!(error.submission_unknown);
    assert_eq!(error.request_id.as_deref(), Some("put-identity"));
    fixture.mode(161);
    assert_eq!(
        fixture
            .client
            .tasks()
            .inspection_task("task-1")
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Protocol
    );
    fixture.mode(162);
    let error = fixture
        .client
        .tasks()
        .start_inspection("map-1", "route-1", 7, binding(), Some("start-identity"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Protocol);
    assert!(error.submission_unknown);
    fixture.mode(164);
    let error = fixture
        .client
        .tasks()
        .cancel_inspection("task-1", "operator", binding(), Some("cancel-identity"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Protocol);
    assert!(error.submission_unknown);
}

#[tokio::test]
async fn inspection_transport_failure_preserves_unknown_submission() {
    let fixture = Fixture::new().await;
    fixture.mode(163);
    let error = fixture
        .client
        .tasks()
        .start_inspection("map-1", "route-1", 7, binding(), Some("start-unknown"))
        .await
        .unwrap_err();
    assert_eq!(error.code(), Code::Unavailable);
    assert!(error.submission_unknown);
    assert_eq!(error.request_id.as_deref(), Some("start-unknown"));
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn inspection_task_list_retains_owner_identity_and_terminal_history() {
    let fixture = Fixture::new().await;
    let tasks = fixture.client.tasks();
    let active = tasks
        .inspection_tasks(Some("map-1"), Some("route-1"), false, None)
        .await
        .unwrap();
    assert_eq!(active.count, 1);
    assert_eq!(active.tasks[0].task_id, "task-1");
    assert!(!active.tasks[0].terminal);
    let history = tasks
        .inspection_tasks(Some("map-1"), Some("route-1"), true, None)
        .await
        .unwrap();
    assert_eq!(history.count, 2);
    assert_eq!(history.tasks[1].task_id, "task-2");
    assert!(history.tasks[1].terminal);
    assert_eq!(history.retention, "process_local_gateway_projection");
    assert_eq!(history.observed_at_unix_s, 1234.0);
    assert_eq!(
        tasks
            .inspection_tasks(Some(" "), None, false, None)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}
