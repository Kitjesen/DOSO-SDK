use super::*;

#[tokio::test]
async fn cancelled_velocity_exchange_closes_session() {
    let fixture = Fixture::new().await;
    fixture.mode(121);
    let mut session = fixture.client.motion().teleop().await.unwrap();
    let waiting = tokio::time::timeout(
        Duration::from_millis(150),
        session.send_velocity(
            rpc::TeleopVelocity {
                vx_m_s: 0.2,
                deadman: true,
                ..Default::default()
            },
            Some("cancelled-velocity"),
        ),
    )
    .await;
    assert!(waiting.is_err());
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 1);
    assert_eq!(
        session.hold(None).await.unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 1);
}
