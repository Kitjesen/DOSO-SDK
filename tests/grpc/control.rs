use super::*;

pub(super) fn state(held: bool) -> rpc::ControlState {
    rpc::ControlState {
        available: Some(!held),
        held_by_client: held.then_some(true),
        owner: if held {
            "sdk:session".into()
        } else {
            String::new()
        },
        owner_epoch: u64::from(held),
        received_age_ms: Some(0.0),
        fresh: true,
        ..Default::default()
    }
}

pub(super) fn faults() -> rpc::FaultReport {
    rpc::FaultReport {
        health: rpc::HealthStatus::Error as i32,
        faults: vec![rpc::Fault {
            source: "driver.control".into(),
            code: "critical_motor_fault".into(),
            severity: rpc::FaultSeverity::Error as i32,
            active: Some(true),
            ..Default::default()
        }],
        observed_sources: vec!["driver.control".into()],
        unavailable_sources: vec!["driver.health".into(), "navigation.control_loop".into()],
    }
}

#[tokio::test]
async fn explicit_claim_and_release_do_not_publish_velocity() {
    let fixture = Fixture::new().await;
    let session = fixture
        .client
        .motion()
        .claim_control(Some("claim"))
        .await
        .unwrap();
    assert_eq!(session.latest_reply().unwrap().unwrap().request_id, "claim");
    assert_eq!(
        session.control_state().unwrap().unwrap().held_by_client,
        Some(true)
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
    assert!(
        session
            .release_control(Some("release"))
            .await
            .unwrap()
            .final_output_confirmed
    );
}

#[tokio::test]
async fn rejected_claim_preserves_native_reason() {
    let fixture = Fixture::new().await;
    fixture.mode(126);
    let error = fixture
        .client
        .motion()
        .claim_control(Some("claim"))
        .await
        .err()
        .unwrap();
    assert_eq!(error.code(), Code::FailedPrecondition);
    assert!(error.to_string().contains("authority_in_use"));
    assert_eq!(error.request_id.as_deref(), Some("claim"));
    assert!(!error.submission_unknown);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn missing_claim_ack_preserves_unknown_submission() {
    let fixture = Fixture::new().await;
    fixture.mode(127);
    let error = fixture
        .client
        .motion()
        .claim_control(Some("claim"))
        .await
        .err()
        .unwrap();
    assert_eq!(error.code(), Code::Unavailable);
    assert_eq!(error.request_id.as_deref(), Some("claim"));
    assert!(error.submission_unknown);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn cached_authority_expires_when_feedback_stalls() {
    let fixture = Fixture::new().await;
    let session = fixture.client.motion().claim_control(None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(280)).await;
    let state = session.control_state().unwrap().unwrap();
    assert!(!state.fresh);
    assert_eq!(state.held_by_client, None);
    assert_eq!(state.available, None);
    session.release_control(None).await.unwrap();
}

#[tokio::test]
async fn observes_authority_and_fault_coverage() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture
            .client
            .motion()
            .control_state()
            .await
            .unwrap()
            .available,
        Some(true)
    );
    let mut control = fixture.client.motion().watch_control_state().await.unwrap();
    assert_eq!(
        control.message().await.unwrap().unwrap().held_by_client,
        None
    );
    let report = fixture.client.system().faults().await.unwrap();
    assert_eq!(report, faults());
    let mut faults = fixture.client.system().watch_faults().await.unwrap();
    assert_eq!(faults.message().await.unwrap().unwrap(), report);
    assert!(
        fixture
            .client
            .motion()
            .resume(Some("resume"))
            .await
            .unwrap()
            .accepted
            == Some(true)
    );
    fixture.mode(2);
    assert!(
        fixture
            .client
            .motion()
            .resume(Some("unknown"))
            .await
            .unwrap_err()
            .submission_unknown
    );
}
