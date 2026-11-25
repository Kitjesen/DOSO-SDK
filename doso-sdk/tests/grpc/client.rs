use super::*;

#[tokio::test]
async fn reconnect_opens_new_transport_without_commands_or_subscriptions() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let connections = Arc::new(AtomicUsize::new(0));
    let accepted = connections.clone();
    let incoming = TcpListenerStream::new(listener).map(move |connection| {
        if connection.is_ok() {
            accepted.fetch_add(1, Ordering::Relaxed);
        }
        connection
    });
    let service = Service::default();
    let (shutdown, receive_shutdown) = oneshot::channel();
    let server = tonic::transport::Server::builder()
        .add_service(rpc::system_server::SystemServer::new(service.clone()));
    let serving = tokio::spawn(async move {
        server
            .serve_with_incoming_shutdown(incoming, async {
                let _ = receive_shutdown.await;
            })
            .await
            .unwrap();
    });
    let mut config = Config::new(endpoint);
    config.api_key = Some("test-only-key".into());
    let mut client = DosoClient::with_config(config).await.unwrap();
    assert_eq!(connections.load(Ordering::Relaxed), 1);
    client.reconnect().await.unwrap();
    assert_eq!(connections.load(Ordering::Relaxed), 2);
    assert_eq!(client.system().ping().await.unwrap().status, "ok");
    assert_eq!(service.writes.load(Ordering::Relaxed), 0);
    assert_eq!(service.watches.load(Ordering::Relaxed), 0);
    drop(client);
    let _ = shutdown.send(());
    tokio::time::timeout(Duration::from_secs(2), serving)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn reconnect_refreshes_same_robots_identity_snapshot() {
    let mut fixture = Fixture::new().await;
    fixture.mode(120);
    fixture.client.reconnect().await.unwrap();
    assert_eq!(
        fixture.client.system().info().firmware_version.as_deref(),
        Some("updated-firmware")
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn reconnect_rejects_bad_handshake_and_preserves_last_success() {
    let mut fixture = Fixture::new().await;
    let previous = fixture.client.system().info().clone();
    for mode in [17, 18, 22, 23, 121] {
        fixture.mode(mode);
        let error = fixture.client.reconnect().await.unwrap_err();
        assert_eq!(
            error.kind,
            if mode == 18 {
                ErrorKind::Rpc(Code::FailedPrecondition)
            } else {
                ErrorKind::Protocol
            }
        );
        assert!(!error.submission_unknown);
        assert_eq!(fixture.client.system().info(), &previous);
    }
    fixture.mode(120);
    fixture.client.reconnect().await.unwrap();
    assert_eq!(
        fixture.client.system().info().firmware_version.as_deref(),
        Some("updated-firmware")
    );
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn reconnect_can_learn_previously_absent_robot_identity() {
    let fixture = Fixture::new().await;
    fixture.mode(22);
    let mut config = Config::new(&fixture.endpoint);
    config.api_key = Some("test-only-key".into());
    let mut client = DosoClient::with_config(config).await.unwrap();
    assert_eq!(client.system().info().robot_id, None);
    fixture.mode(0);
    client.reconnect().await.unwrap();
    assert_eq!(
        client.system().info().robot_id,
        fixture.client.system().info().robot_id
    );
}

#[tokio::test]
async fn reconnect_handshake_obeys_timeout_without_replacing_snapshot() {
    let fixture = Fixture::new().await;
    let mut config = Config::new(&fixture.endpoint);
    config.api_key = Some("test-only-key".into());
    config.timeout = Duration::from_millis(50);
    let mut client = DosoClient::with_config(config).await.unwrap();
    let previous = client.system().info().clone();
    fixture.mode(19);
    let error = tokio::time::timeout(Duration::from_secs(2), client.reconnect())
        .await
        .expect("reconnect handshake must time out")
        .unwrap_err();
    assert!(matches!(
        error.code(),
        Code::Cancelled | Code::DeadlineExceeded
    ));
    assert_eq!(client.system().info(), &previous);
    assert!(!error.submission_unknown);
    fixture.mode(0);
    client.reconnect().await.unwrap();
    assert_eq!(fixture.service.writes.load(Ordering::Relaxed), 0);
}
