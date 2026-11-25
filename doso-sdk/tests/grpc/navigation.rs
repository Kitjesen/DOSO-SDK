use super::*;
use doso_sdk::GoalOptions;

fn target() -> Pose {
    Pose {
        x: 1.0,
        y: 2.0,
        z: 3.0,
        yaw: 0.5,
    }
}

#[tokio::test]
async fn goal_options_reach_grpc() {
    let fixture = Fixture::new().await;
    fixture.mode(150);
    let receipt = fixture
        .client
        .navigation()
        .go_to_with_options(
            target(),
            GoalOptions {
                max_speed_mps: Some(0.8),
                acceptance_radius_m: Some(0.25),
            },
            Some("goal-request"),
        )
        .await
        .unwrap();
    assert_eq!(receipt.request_id, "goal-request");
    assert_eq!(receipt.task_id.as_deref(), Some("task-1"));
}

#[tokio::test]
async fn unsupported_goal_options_do_not_fall_back_to_go() {
    let fixture = Fixture::new().await;
    fixture.mode(152);
    let error = fixture
        .client
        .navigation()
        .go_to_with_options(
            target(),
            GoalOptions {
                max_speed_mps: Some(0.8),
                acceptance_radius_m: Some(0.25),
            },
            Some("unsupported-options"),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), Code::Unimplemented);
    assert_eq!(error.request_id.as_deref(), Some("unsupported-options"));
    assert!(!error.submission_unknown);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
    fixture
        .client
        .navigation()
        .go_to(target(), None)
        .await
        .unwrap();
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn preview_is_read_only_and_preserves_xyz() {
    let fixture = Fixture::new().await;
    let preview = fixture
        .client
        .navigation()
        .preview_plan(target(), Some(0.25))
        .await
        .unwrap();
    assert!(preview.ok && preview.feasible);
    assert_eq!(preview.path.unwrap().points[0].z, 3.0);
    assert_eq!(preview.plan_ms, Some(2.5));
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn invalid_limits_do_not_send_commands() {
    let fixture = Fixture::new().await;
    for speed in [0.0, -1.0, f64::NAN, f64::INFINITY, 5.1] {
        assert_eq!(
            fixture
                .client
                .navigation()
                .go_to_with_options(
                    target(),
                    GoalOptions {
                        max_speed_mps: Some(speed),
                        ..Default::default()
                    },
                    None
                )
                .await
                .unwrap_err()
                .code(),
            Code::InvalidArgument
        );
    }
    for radius in [0.0, -1.0, f64::NAN, f64::INFINITY, 20.1] {
        assert_eq!(
            fixture
                .client
                .navigation()
                .preview_plan(target(), Some(radius))
                .await
                .unwrap_err()
                .code(),
            Code::InvalidArgument
        );
    }
    for speed in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            fixture
                .client
                .navigation()
                .set_speed_limit("task-1", speed, None)
                .await
                .unwrap_err()
                .code(),
            Code::InvalidArgument
        );
    }
    assert_eq!(
        fixture
            .client
            .navigation()
            .set_speed_limit("task-1", 0.7, Some("task-1"))
            .await
            .unwrap_err()
            .code(),
        Code::InvalidArgument
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn speed_limit_checks_request_and_task_identity() {
    let fixture = Fixture::new().await;
    let receipt = fixture
        .client
        .navigation()
        .set_speed_limit("task-1", 0.7, Some("speed-request"))
        .await
        .unwrap();
    assert_eq!(receipt.request_id, "speed-request");
    fixture.mode(151);
    assert_eq!(
        fixture
            .client
            .navigation()
            .set_speed_limit("task-1", 0.7, None)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Protocol
    );
    fixture.mode(1);
    assert_eq!(
        fixture
            .client
            .navigation()
            .set_speed_limit("task-1", 0.7, None)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Protocol
    );
}
