use super::*;
use std::task::{Context, Poll};

pub(super) fn source_available(mode: u8) -> Result<(), Status> {
    match mode {
        130 => Err(Status::unavailable("SLAM feedback unavailable")),
        131 => Err(Status::unimplemented("SLAM feedback not supported")),
        _ => Ok(()),
    }
}

pub(super) fn velocity(mode: u8) -> rpc::VelocitySample {
    let mut sample = rpc::VelocitySample {
        linear_m_s: Some(rpc::Vector3 {
            x: 0.2,
            y: -0.3,
            z: 0.4,
        }),
        angular_rad_s: Some(rpc::Vector3 {
            x: -0.5,
            y: 0.6,
            z: -0.7,
        }),
        frame_id: "body".into(),
        time: None,
        received_age_ms: Some(12.5),
        fresh: Some(true),
        source: "localization".into(),
    };
    match mode {
        126 => sample.linear_m_s = None,
        127 => sample.angular_rad_s.as_mut().unwrap().z = f64::NAN,
        128 => sample.frame_id.clear(),
        129 => sample.received_age_ms = Some(-1.0),
        _ => {}
    }
    sample
}

pub(super) fn odometry(mode: u8) -> rpc::OdometrySample {
    let velocity = velocity(0);
    let mut sample = rpc::OdometrySample {
        position_m: Some(rpc::Vector3 {
            x: 1.0,
            y: -2.0,
            z: 3.0,
        }),
        orientation: Some(rpc::Quaternion {
            w: 0.5,
            x: -0.5,
            y: 0.5,
            z: -0.5,
        }),
        linear_velocity_m_s: velocity.linear_m_s,
        angular_velocity_rad_s: velocity.angular_rad_s,
        parent_frame: "map".into(),
        child_frame: "body".into(),
        time: None,
        received_age_ms: Some(12.5),
        fresh: Some(true),
        source: "localization".into(),
    };
    match mode {
        126 => sample.orientation = None,
        127 => sample.position_m.as_mut().unwrap().z = f64::INFINITY,
        128 => sample.parent_frame = sample.child_frame.clone(),
        129 => sample.orientation.as_mut().unwrap().w = 0.0,
        _ => {}
    }
    sample
}

struct TrackedStream<T> {
    inner: Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>,
    active: Arc<AtomicUsize>,
}

impl<T> Stream for TrackedStream<T> {
    type Item = Result<T, Status>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(context)
    }
}

impl<T> Drop for TrackedStream<T> {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(super) fn track<T: Send + 'static>(
    inner: Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>,
    active: Arc<AtomicUsize>,
) -> Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>> {
    active.fetch_add(1, Ordering::Relaxed);
    Box::pin(TrackedStream { inner, active })
}

#[tokio::test]
async fn kinematics_reads_preserve_six_axis_feedback_and_unknown_time() {
    let fixture = Fixture::new().await;
    let velocity = fixture.client.motion().velocity().await.unwrap();
    assert_eq!(velocity, self::velocity(0));
    assert!(velocity.time.is_none());
    let odometry = fixture.client.motion().odometry().await.unwrap();
    assert_eq!(odometry, self::odometry(0));
    assert!(odometry.time.is_none());
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn kinematics_reads_reject_missing_invalid_or_inconsistent_fields() {
    let fixture = Fixture::new().await;
    for mode in 126..=129 {
        fixture.mode(mode);
        assert_eq!(
            fixture.client.motion().velocity().await.unwrap_err().kind,
            ErrorKind::Protocol
        );
        assert_eq!(
            fixture.client.motion().odometry().await.unwrap_err().kind,
            ErrorKind::Protocol
        );
    }
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn kinematics_reads_and_watches_preserve_source_errors() {
    let fixture = Fixture::new().await;
    for (mode, code) in [(130, Code::Unavailable), (131, Code::Unimplemented)] {
        fixture.mode(mode);
        for error in [
            fixture.client.motion().velocity().await.unwrap_err(),
            fixture.client.motion().odometry().await.unwrap_err(),
            fixture.client.motion().watch_velocity().await.unwrap_err(),
            fixture.client.motion().watch_odometry().await.unwrap_err(),
        ] {
            assert_eq!(error.code(), code);
            assert!(!error.submission_unknown);
        }
    }
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn kinematics_watches_preserve_stream_errors_and_cancel_on_drop() {
    let fixture = Fixture::new().await;
    let mut velocity = fixture.client.motion().watch_velocity().await.unwrap();
    assert_eq!(
        velocity.message().await.unwrap().unwrap(),
        self::velocity(0)
    );
    assert_eq!(fixture.service.watches.load(Ordering::Relaxed), 1);
    drop(velocity);
    let mut odometry = fixture.client.motion().watch_odometry().await.unwrap();
    assert_eq!(
        odometry.message().await.unwrap().unwrap(),
        self::odometry(0)
    );
    assert_eq!(
        odometry.message().await.unwrap_err().code(),
        Code::Unavailable
    );
    drop(odometry);
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.service.watches.load(Ordering::Relaxed) != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}
