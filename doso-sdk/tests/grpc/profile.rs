use super::*;

pub(super) fn info() -> rpc::ProfileInfo {
    rpc::ProfileInfo {
        current: "walk".into(),
        available: vec!["walk".into(), "stairs_up".into()],
        descriptions: vec!["Walking".into(), "Ascending stairs".into()],
        current_description: "Walking".into(),
    }
}

#[tokio::test]
async fn profile_read_is_independent_from_switch_ack() {
    let fixture = Fixture::new().await;
    assert_eq!(fixture.client.motion().profile().await.unwrap(), info());
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
    let receipt = fixture
        .client
        .motion()
        .switch_profile("stairs_up", Some("switch"))
        .await
        .unwrap();
    assert_eq!(receipt.request_id, "switch");
    assert_eq!(receipt.accepted, Some(true));
    assert_eq!(receipt.stage, "profile_switch_acknowledged");
    assert_eq!(
        fixture.client.motion().profile().await.unwrap().current,
        "walk"
    );
    fixture.mode(130);
    assert_eq!(
        fixture.client.motion().profile().await.unwrap_err().code(),
        Code::Unimplemented
    );
}

#[tokio::test]
async fn profile_switch_validates_name_and_preserves_unknown_submission() {
    let fixture = Fixture::new().await;
    for name in ["", "   "] {
        assert_eq!(
            fixture
                .client
                .motion()
                .switch_profile(name, None)
                .await
                .unwrap_err()
                .code(),
            Code::InvalidArgument
        );
    }
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
    fixture.mode(2);
    let error = fixture
        .client
        .motion()
        .switch_profile("stairs_up", Some("switch"))
        .await
        .unwrap_err();
    assert!(error.submission_unknown);
    assert_eq!(error.request_id.as_deref(), Some("switch"));
    fixture.mode(6);
    assert!(
        !fixture
            .client
            .motion()
            .switch_profile("stairs_up", None)
            .await
            .unwrap_err()
            .submission_unknown
    );
}
