use super::*;

pub(super) fn state(selected: bool, last_sequence: u64) -> rpc::ControllerState {
    rpc::ControllerState {
        mode: if selected {
            rpc::ControllerMode::External
        } else {
            rpc::ControllerMode::Builtin
        } as i32,
        session_id: if selected {
            "external-test".into()
        } else {
            String::new()
        },
        active: selected,
        motor_output_enabled: false,
        last_sequence,
        fresh: true,
        received_age_ms: Some(0.0),
        joint_count: 16,
        ..Default::default()
    }
}

type ReplyStream = Pin<Box<dyn Stream<Item = Result<rpc::ExternalControlReply, Status>> + Send>>;

pub(super) async fn serve(
    service: Service,
    request: Request<tonic::Streaming<rpc::ExternalControlRequest>>,
) -> Result<Response<ReplyStream>, Status> {
    assert_eq!(
        request.metadata().get("x-api-key").unwrap(),
        "test-only-key"
    );
    assert!(request.metadata().get("grpc-timeout").is_none());
    if service.mode.load(Ordering::Relaxed) == 133 {
        return Err(Status::unavailable(
            "external Enter acknowledgement unknown",
        ));
    }
    let mut requests = request.into_inner();
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    service.watches.fetch_add(1, Ordering::Relaxed);
    tokio::spawn(async move {
        let mut dispatched = 0;
        while let Ok(Some(request)) = requests.message().await {
            let command = request.command.unwrap();
            let close = matches!(
                request.action,
                Some(rpc::external_control_request::Action::Close(_))
            );
            let heartbeat = matches!(
                request.action,
                Some(rpc::external_control_request::Action::Heartbeat(_))
            );
            let mut reply = rpc::ExternalControlReply {
                request_id: if heartbeat {
                    String::new()
                } else {
                    command.request_id
                },
                sequence: request.sequence,
                accepted: true,
                input_window: if close { 0 } else { 17 },
                state: Some(state(!close, dispatched)),
                ..Default::default()
            };
            match request.action.unwrap() {
                rpc::external_control_request::Action::Enter(_) => {
                    if service.mode.load(Ordering::Relaxed) == 131 {
                        reply.accepted = false;
                        reply.reason = "motors_must_be_disabled".into();
                    }
                    if service.mode.load(Ordering::Relaxed) == 132 {
                        reply.request_id = "wrong-session".into();
                    }
                    if service.mode.load(Ordering::Relaxed) == 134 {
                        reply.input_window = 0;
                    }
                }
                rpc::external_control_request::Action::Frame(frame) => {
                    assert_eq!(frame.joints.len(), 16);
                    assert_eq!(request.input_window, 17);
                    assert_eq!(frame.joints[0].q, 0.1);
                    service.writes.fetch_add(1, Ordering::Relaxed);
                    dispatched = request.sequence;
                    reply.ingress_accepted = true;
                    reply.state.as_mut().unwrap().last_sequence = dispatched;
                }
                rpc::external_control_request::Action::Heartbeat(_) => {
                    assert_eq!(request.sequence, 0)
                }
                rpc::external_control_request::Action::Enable(_) => {
                    reply.state.as_mut().unwrap().motor_output_enabled = true
                }
                rpc::external_control_request::Action::Disable(_)
                | rpc::external_control_request::Action::Close(_) => {
                    reply.state.as_mut().unwrap().motor_disable_confirmed = true;
                }
            }
            let accepted = reply.accepted;
            if sender.send(Ok(reply)).await.is_err() || close || !accepted {
                break;
            }
            if service.mode.load(Ordering::Relaxed) == 134 {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let _ = sender
                    .send(Err(Status::unavailable("external window stream failed")))
                    .await;
                break;
            }
        }
        service.watches.fetch_sub(1, Ordering::Relaxed);
    });
    Ok(Response::new(Box::pin(
        tokio_stream::wrappers::ReceiverStream::new(receiver),
    )))
}

fn targets() -> Vec<rpc::JointCommand> {
    vec![
        rpc::JointCommand {
            q: 0.1,
            dq: 0.0,
            kp: 10.0,
            kd: 1.0,
            tau: 0.0
        };
        16
    ]
}

#[tokio::test]
async fn external_mode_primes_targets_enables_disables_and_returns_builtin() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture.client.motion().controller().await.unwrap().mode,
        rpc::ControllerMode::Builtin as i32
    );
    let mut session = fixture
        .client
        .motion()
        .external_control(Some("external-test"))
        .await
        .unwrap();
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
    session
        .publish_joints(targets(), Some("initial-targets"))
        .unwrap();
    for _ in 0..100 {
        if session.state().unwrap().unwrap().last_sequence == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(session.state().unwrap().unwrap().last_sequence, 2);
    assert!(session.enable(Some("enable")).await.unwrap().accepted);
    assert!(
        session
            .disable(Some("disable"))
            .await
            .unwrap()
            .state
            .unwrap()
            .motor_disable_confirmed
    );
    let closed = session.close(Some("exit")).await.unwrap();
    assert!(closed.accepted);
    assert_eq!(
        closed.state.unwrap().mode,
        rpc::ControllerMode::Builtin as i32
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn heartbeat_preserves_connection_without_manufacturing_joint_targets() {
    let fixture = Fixture::new().await;
    let session = fixture
        .client
        .motion()
        .external_control(None)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(320)).await;
    assert!(session.state().unwrap().unwrap().fresh);
    assert!(session.latest_reply().unwrap().is_none());
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
    drop(session);
    for _ in 0..100 {
        if fixture.service.watches.load(Ordering::Relaxed) == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(fixture.service.watches.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn invalid_joint_targets_fail_before_submission() {
    let fixture = Fixture::new().await;
    let mut session = fixture
        .client
        .motion()
        .external_control(None)
        .await
        .unwrap();
    for values in [Vec::new(), vec![rpc::JointCommand::default(); 15]] {
        assert_eq!(
            session.publish_joints(values, None).unwrap_err().code(),
            Code::InvalidArgument
        );
    }
    let mut values = targets();
    values[0].tau = f64::NAN;
    assert_eq!(
        session.publish_joints(values, None).unwrap_err().code(),
        Code::InvalidArgument
    );
    let mut values = targets();
    values[0].kd = -1.0;
    assert_eq!(
        session.publish_joints(values, None).unwrap_err().code(),
        Code::InvalidArgument
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
    session.close(None).await.unwrap();
}

#[tokio::test]
async fn external_enter_preserves_rejection_and_unknown_result() {
    let fixture = Fixture::new().await;
    fixture.mode(131);
    let rejected = fixture
        .client
        .motion()
        .external_control(Some("rejected"))
        .await
        .err()
        .unwrap();
    assert_eq!(rejected.code(), Code::FailedPrecondition);
    assert!(!rejected.submission_unknown);
    fixture.mode(132);
    let wrong = fixture
        .client
        .motion()
        .external_control(Some("unknown"))
        .await
        .err()
        .unwrap();
    assert!(wrong.submission_unknown);
    fixture.mode(133);
    let lost = fixture
        .client
        .motion()
        .external_control(Some("lost"))
        .await
        .err()
        .unwrap();
    assert_eq!(lost.request_id.as_deref(), Some("lost"));
    assert!(lost.submission_unknown);
}

#[tokio::test]
async fn external_enter_window_failure_preserves_request() {
    let fixture = Fixture::new().await;
    fixture.mode(134);
    let error = fixture
        .client
        .motion()
        .external_control(Some("window-lost"))
        .await
        .err()
        .unwrap();
    assert_eq!(error.code(), Code::Unavailable);
    assert_eq!(error.message, "external window stream failed");
    assert_eq!(error.request_id.as_deref(), Some("window-lost"));
    assert!(error.submission_unknown);
}
