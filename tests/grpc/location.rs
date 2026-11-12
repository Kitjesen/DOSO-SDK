use super::*;
use doso_sdk::{GeographicFix, GeographicLocation, GeographicState};

pub(super) fn available(mode: u8) -> Result<(), Status> {
    match mode {
        135 => Err(Status::unimplemented("geographic location not supported")),
        137 => Err(Status::unauthenticated("invalid SDK API key")),
        _ => Ok(()),
    }
}

pub(super) fn sample(state: GeographicState) -> GeographicLocation {
    GeographicLocation {
        state: state as i32,
        fix: (state == GeographicState::Ready).then_some(GeographicFix {
            latitude: 31.2304,
            longitude: 121.4737,
            fix_type: 4,
            observed_at: Some(prost_types::Timestamp {
                seconds: 1_793_700_000,
                nanos: 125_000_000,
            }),
            accuracy_m: None,
        }),
        age_s: match state {
            GeographicState::Ready => Some(0.25),
            GeographicState::Stale => Some(4.0),
            _ => None,
        },
    }
}

#[tokio::test]
async fn location_reads_preserve_wgs84_fix_and_non_ready_state() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture.client.localization().location().await.unwrap(),
        sample(GeographicState::Ready)
    );
    fixture.mode(136);
    let stale = fixture.client.localization().location().await.unwrap();
    assert_eq!(stale, sample(GeographicState::Stale));
    assert!(stale.fix.is_none());
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn location_calls_preserve_authentication_and_unsupported_errors() {
    let fixture = Fixture::new().await;
    for (mode, code) in [(135, Code::Unimplemented), (137, Code::Unauthenticated)] {
        fixture.mode(mode);
        for error in [
            fixture.client.localization().location().await.unwrap_err(),
            fixture
                .client
                .localization()
                .watch_location()
                .await
                .unwrap_err(),
        ] {
            assert_eq!(error.code(), code);
            assert!(!error.submission_unknown);
        }
    }
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn location_watch_recovers_without_fix_and_cancels_on_drop() {
    let fixture = Fixture::new().await;
    let mut stream = fixture
        .client
        .localization()
        .watch_location()
        .await
        .unwrap();
    for state in [
        GeographicState::Ready,
        GeographicState::NoFix,
        GeographicState::Stale,
        GeographicState::Ready,
    ] {
        assert_eq!(stream.message().await.unwrap().unwrap(), sample(state));
    }
    assert_eq!(fixture.service.watches.load(Ordering::Relaxed), 1);
    drop(stream);
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.service.watches.load(Ordering::Relaxed) != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}
