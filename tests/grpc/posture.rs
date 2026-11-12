use super::*;

#[tokio::test]
async fn posture_preserves_admission_identity_and_unknown_submission() {
    let fixture = Fixture::new().await;
    let stand = fixture
        .client
        .motion()
        .stand_up(Some("stand-1"))
        .await
        .unwrap();
    assert_eq!(stand.request_id, "stand-1");
    assert_eq!(stand.stage, "posture_requested");
    assert_eq!(stand.accepted, Some(true));
    let sit = fixture
        .client
        .motion()
        .sit_down(Some("sit-1"))
        .await
        .unwrap();
    assert_eq!(sit.request_id, "sit-1");
    fixture.mode(1);
    let error = fixture
        .client
        .motion()
        .stand_up(Some("stand-2"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Protocol);
    fixture.mode(2);
    let error = fixture
        .client
        .motion()
        .sit_down(Some("sit-2"))
        .await
        .unwrap_err();
    assert!(error.submission_unknown);
    assert_eq!(error.request_id.as_deref(), Some("sit-2"));
    fixture.mode(6);
    let error = fixture
        .client
        .motion()
        .stand_up(Some("stand-3"))
        .await
        .unwrap_err();
    assert_eq!(error.code(), Code::FailedPrecondition);
    assert!(!error.submission_unknown);
    let writes = fixture.service.writes.load(Ordering::Relaxed);
    assert_eq!(
        fixture
            .client
            .motion()
            .sit_down(Some(" "))
            .await
            .unwrap_err()
            .code(),
        Code::InvalidArgument
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), writes);
}
