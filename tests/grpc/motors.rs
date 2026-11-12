use super::*;

#[tokio::test]
async fn motor_commands_preserve_receipts_and_request_identity() {
    let fixture = Fixture::new().await;
    let enabled = fixture
        .client
        .motion()
        .enable_motors(Some("enable"))
        .await
        .unwrap();
    assert_eq!(enabled.request_id, "enable");
    assert_eq!(enabled.accepted, Some(true));
    assert_eq!(enabled.stage, "motor_power_acknowledged");
    let disabled = fixture
        .client
        .motion()
        .disable_motors(Some("disable"))
        .await
        .unwrap();
    assert_eq!(disabled.request_id, "disable");
    assert_eq!(disabled.accepted, Some(true));
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 2);
    fixture.mode(1);
    assert!(
        fixture
            .client
            .motion()
            .enable_motors(None)
            .await
            .unwrap_err()
            .submission_unknown
    );
}

#[tokio::test]
async fn motor_commands_keep_unknown_ack_and_explicit_rejection_distinct() {
    let fixture = Fixture::new().await;
    fixture.mode(2);
    let error = fixture
        .client
        .motion()
        .disable_motors(Some("disable"))
        .await
        .unwrap_err();
    assert!(error.submission_unknown);
    assert_eq!(error.request_id.as_deref(), Some("disable"));
    fixture.mode(6);
    let error = fixture
        .client
        .motion()
        .enable_motors(Some("enable"))
        .await
        .unwrap_err();
    assert!(!error.submission_unknown);
    assert_eq!(error.code(), Code::FailedPrecondition);
    assert_eq!(error.reason_code(), Some("safety_stop"));
}
