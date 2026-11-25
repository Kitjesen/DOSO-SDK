use super::*;

#[tokio::test]
async fn camera_calibration_preserves_optics_and_read_only_metadata() {
    let fixture = Fixture::new().await;
    let calibration = fixture
        .client
        .observations()
        .camera_calibration(Some("viewer"))
        .await
        .unwrap();
    assert_eq!((calibration.width_px, calibration.height_px), (640, 480));
    assert_eq!(
        (
            calibration.fx_px,
            calibration.fy_px,
            calibration.cx_px,
            calibration.cy_px
        ),
        (410.0, 411.0, 319.5, 239.5)
    );
    assert_eq!(calibration.frame_id, "source_color");
    assert_eq!(calibration.distortion_model, "plumb_bob");
    assert_eq!(calibration.distortion, vec![0.1, 0.2, 0.003, 0.004, 0.5]);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn camera_calibration_preserves_source_and_compatibility_errors() {
    let fixture = Fixture::new().await;
    for (mode, code) in [
        (140, Code::Unavailable),
        (141, Code::Unimplemented),
        (142, Code::PermissionDenied),
    ] {
        fixture.mode(mode);
        let error = fixture
            .client
            .observations()
            .camera_calibration(Some("viewer"))
            .await
            .unwrap_err();
        assert_eq!(error.code(), code);
        assert!(!error.submission_unknown);
    }
    assert!(fixture
        .client
        .observations()
        .camera_calibration(Some(""))
        .await
        .is_err());
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}
