use super::*;

#[tokio::test]
async fn task_watch_decodes_owner_updates_and_absence() {
    let fixture = Fixture::new().await;
    let mut stream = fixture.client.tasks().watch_run("run-1").await.unwrap();
    for expected in [rpc::TaskRunStatus::Running, rpc::TaskRunStatus::Succeeded] {
        let run = stream.message().await.unwrap().unwrap();
        assert_eq!(run.run_id, "run-1");
        assert_eq!(run.state().unwrap().unwrap().status, expected as i32);
    }
    assert!(stream.message().await.unwrap().is_none());
    fixture.mode(80);
    let mut missing = fixture.client.tasks().watch_run("old-run").await.unwrap();
    assert!(missing
        .message()
        .await
        .unwrap()
        .unwrap()
        .state()
        .unwrap()
        .is_none());
    assert!(missing.message().await.unwrap().is_none());
    fixture.mode(81);
    assert_eq!(
        fixture
            .client
            .tasks()
            .watch_run("run-1")
            .await
            .unwrap_err()
            .code(),
        Code::Unavailable
    );
    assert_eq!(
        fixture
            .client
            .tasks()
            .watch_run(" ")
            .await
            .unwrap_err()
            .code(),
        Code::InvalidArgument
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn localization_state_preserves_owner_flags() {
    let fixture = Fixture::new().await;
    let state = fixture.client.localization().state().await.unwrap();
    assert_eq!(state.quality, rpc::LocalizationQuality::Lost as i32);
    assert_eq!(state.source_state, "lost");
    assert_eq!(state.ready, Some(false));
    assert_eq!(state.pose_fresh, Some(false));
    assert_eq!(state.map_name, "lab");
    assert!(state.time.is_none());
}
