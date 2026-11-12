use super::*;

fn velocity() -> rpc::TeleopVelocity {
    rpc::TeleopVelocity {
        vx_m_s: 0.2,
        deadman: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn publishes_without_per_sample_ack_and_hold_still_waits_for_confirmation() {
    let fixture = Fixture::new().await;
    fixture.mode(123);
    let mut session = fixture.client.motion().teleop().await.unwrap();
    let mut ticks = tokio::time::interval(Duration::from_millis(20));
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    for index in 0..20 {
        ticks.tick().await;
        let id = format!("velocity-{index}");
        assert_eq!(session.publish_velocity(velocity(), Some(&id)).unwrap(), id);
    }
    assert!(session.latest_reply().unwrap().is_none());
    assert!(
        session
            .hold(Some("hold"))
            .await
            .unwrap()
            .final_output_confirmed
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 20);
    let reply = session.latest_reply().unwrap().unwrap();
    assert_eq!(reply.request_id, "hold");
    assert_eq!(reply.sequence, 22);
    assert!(session.close(None).await.unwrap().accepted);
}

#[tokio::test]
async fn full_local_queue_reports_unsent_input() {
    let fixture = Fixture::new().await;
    let mut session = fixture.client.motion().teleop().await.unwrap();
    session.publish_velocity(velocity(), Some("first")).unwrap();
    let error = session
        .publish_velocity(velocity(), Some("unsent"))
        .unwrap_err();
    assert_eq!(error.code(), Code::ResourceExhausted);
    assert!(!error.submission_unknown);
    assert!(session.hold(None).await.unwrap().accepted);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn refuses_old_single_use_protocol_before_motion() {
    let fixture = Fixture::new().await;
    fixture.mode(125);
    assert!(
        matches!(fixture.client.motion().teleop().await, Err(error) if error.kind == ErrorKind::Protocol)
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}
