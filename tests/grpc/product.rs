use super::*;

pub(super) fn snapshot(mode: u8) -> rpc::Snapshot {
    let mut current = Struct {
        fields: [(
            "status".into(),
            Value {
                kind: Some(Kind::StringValue(
                    if mode == 138 { "stopped" } else { "active" }.into(),
                )),
            },
        )]
        .into(),
    };
    if mode != 138 {
        for (name, text) in [
            ("product", "map"),
            ("product_session_id", "product-session-1"),
        ] {
            current.fields.insert(
                name.into(),
                Value {
                    kind: Some(Kind::StringValue(text.into())),
                },
            );
        }
    }
    // Older services may still include diagnostics; the public typed reply ignores them.
    current.fields.insert(
        "processes".into(),
        Value {
            kind: Some(Kind::StringValue("private-diagnostics".into())),
        },
    );
    let available = if mode == 139 {
        Kind::StringValue("true".into())
    } else {
        Kind::BoolValue(true)
    };
    rpc::Snapshot {
        fields: Some(Struct {
            fields: [
                (
                    "available".into(),
                    Value {
                        kind: Some(available),
                    },
                ),
                (
                    "current".into(),
                    Value {
                        kind: Some(if mode == 141 {
                            Kind::NullValue(0)
                        } else {
                            Kind::StructValue(current)
                        }),
                    },
                ),
            ]
            .into(),
        }),
    }
}

#[tokio::test]
async fn product_status_preserves_guarded_switch_identity_and_stopped_state() {
    let fixture = Fixture::new().await;
    let current = fixture.client.system().product_status().await.unwrap();
    assert_eq!(current.available, Some(true));
    assert_eq!(current.status, "active");
    assert_eq!(current.product.as_deref(), Some("map"));
    assert_eq!(
        current.product_session_id.as_deref(),
        Some("product-session-1")
    );
    fixture.mode(138);
    let stopped = fixture.client.system().product_status().await.unwrap();
    assert_eq!(stopped.status, "stopped");
    assert!(stopped.product.is_none() && stopped.product_session_id.is_none());
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn product_status_rejects_malformed_fields_and_preserves_transport_error() {
    let fixture = Fixture::new().await;
    for mode in [139, 141] {
        fixture.mode(mode);
        assert_eq!(
            fixture
                .client
                .system()
                .product_status()
                .await
                .unwrap_err()
                .kind,
            ErrorKind::Protocol
        );
    }
    fixture.mode(140);
    let error = fixture.client.system().product_status().await.unwrap_err();
    assert_eq!(error.code(), Code::Unavailable);
    assert!(!error.submission_unknown);
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}
