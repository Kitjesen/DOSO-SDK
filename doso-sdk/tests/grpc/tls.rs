use super::{Config, DosoClient, ErrorKind, Service};
use doso_sdk::internal::motion_board::{rpc::Voltage, MotionBoardClient, MotionBoardConfig};
use std::{
    convert::Infallible,
    fs,
    future::{ready, Ready},
    path::PathBuf,
    process::Command,
    task::{Context, Poll},
    time::Duration,
};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::body::Body;
use tonic::codegen::{http, BoxFuture, Service as HttpService};
use tonic::server::{Grpc, NamedService, UnaryService};
use tonic::transport::{Certificate, ClientTlsConfig, Identity, Server, ServerTlsConfig};
use tonic::{Request, Response, Status};

struct Certificates(PathBuf);

impl Certificates {
    fn generate() -> Option<Self> {
        if Command::new("openssl").arg("version").output().is_err() {
            eprintln!("TLS fixture generation requires the installed openssl tool");
            return None;
        }
        let certificates = Self(std::env::temp_dir().join(format!(
            "doso-sdk-tls-{}",
            doso_sdk::new_request_id().unwrap()
        )));
        fs::create_dir(&certificates.0).unwrap();
        fs::write(
            certificates.0.join("openssl.cnf"),
            "[req]\ndistinguished_name=dn\n[dn]\n",
        )
        .unwrap();
        fs::write(
            certificates.0.join("server.ext"),
            "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\n\
             extendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost\n",
        )
        .unwrap();
        for arguments in [
            vec![
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-days",
                "1",
                "-config",
                "openssl.cnf",
                "-subj",
                "/CN=DOSO SDK Test CA",
                "-addext",
                "basicConstraints=critical,CA:TRUE",
                "-keyout",
                "ca.key",
                "-out",
                "ca.pem",
            ],
            vec![
                "req",
                "-new",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-config",
                "openssl.cnf",
                "-subj",
                "/CN=localhost",
                "-keyout",
                "server.key",
                "-out",
                "server.csr",
            ],
            vec![
                "x509",
                "-req",
                "-in",
                "server.csr",
                "-CA",
                "ca.pem",
                "-CAkey",
                "ca.key",
                "-CAcreateserial",
                "-days",
                "1",
                "-extfile",
                "server.ext",
                "-out",
                "server.pem",
            ],
        ] {
            certificates.openssl(&arguments);
        }
        Some(certificates)
    }

    fn openssl(&self, arguments: &[&str]) {
        let result = Command::new("openssl")
            .args(arguments)
            .current_dir(&self.0)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "test certificate generation failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    fn client_identity(&self) -> Identity {
        fs::write(
            self.0.join("client.ext"),
            "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=clientAuth\n",
        )
        .unwrap();
        self.openssl(&[
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-config",
            "openssl.cnf",
            "-subj",
            "/CN=DOSO SDK Test Client",
            "-keyout",
            "client.key",
            "-out",
            "client.csr",
        ]);
        self.openssl(&[
            "x509",
            "-req",
            "-in",
            "client.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-CAcreateserial",
            "-days",
            "1",
            "-extfile",
            "client.ext",
            "-out",
            "client.pem",
        ]);
        Identity::from_pem(self.read("client.pem"), self.read("client.key"))
    }

    fn read(&self, filename: &str) -> Vec<u8> {
        fs::read(self.0.join(filename)).unwrap()
    }
}

impl Drop for Certificates {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[tokio::test]
async fn private_ca_requires_trust_and_matching_hostname() {
    let Some(certificates) = Certificates::generate() else {
        return;
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let identity = Identity::from_pem(
        certificates.read("server.pem"),
        certificates.read("server.key"),
    );
    let (shutdown, receive_shutdown) = oneshot::channel();
    let server = Server::builder()
        .tls_config(ServerTlsConfig::new().identity(identity))
        .unwrap()
        .add_service(super::rpc::system_server::SystemServer::new(
            Service::default(),
        ));
    let serving = tokio::spawn(async move {
        server
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = receive_shutdown.await;
            })
            .await
            .unwrap();
    });
    let mut config = Config::new(format!("https://localhost:{port}"));
    config.api_key = Some("test-only-key".into());
    config.timeout = Duration::from_secs(3);
    config.tls_ca_certificate = Some(certificates.read("ca.pem"));
    let client = DosoClient::with_config(config).await.unwrap();
    drop(client);

    for (host, trusted) in [("localhost", false), ("127.0.0.1", true)] {
        let mut config = Config::new(format!("https://{host}:{port}"));
        config.api_key = Some("test-only-key".into());
        config.timeout = Duration::from_secs(3);
        config.tls_ca_certificate = trusted.then(|| certificates.read("ca.pem"));
        let error = DosoClient::with_config(config).await.err().unwrap();
        assert_eq!(error.kind, ErrorKind::Transport);
    }
    shutdown.send(()).unwrap();
    serving.await.unwrap();
}

#[tokio::test]
async fn ca_configuration_requires_https_and_nonempty_certificate() {
    for endpoint in ["http://127.0.0.1:1", "https://localhost:1"] {
        let mut config = Config::new(endpoint);
        config.tls_ca_certificate = Some(Vec::new());
        let error = DosoClient::with_config(config).await.err().unwrap();
        assert_eq!(error.kind, ErrorKind::InvalidInput);
    }
}

#[derive(Clone)]
struct BrainstemVoltage;

impl NamedService for BrainstemVoltage {
    const NAME: &'static str = "brainstem.api.v1.RobotControl";
}

impl UnaryService<()> for BrainstemVoltage {
    type Response = Voltage;
    type Future = Ready<Result<Response<Voltage>, Status>>;

    fn call(&mut self, _: Request<()>) -> Self::Future {
        ready(Ok(Response::new(Voltage { values: vec![52.5] })))
    }
}

impl HttpService<http::Request<Body>> for BrainstemVoltage {
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<Body>) -> Self::Future {
        Box::pin(async move {
            if request.uri().path() != "/brainstem.api.v1.RobotControl/GetVoltage" {
                return Ok(Status::unimplemented("fixture serves GetVoltage only").into_http());
            }
            Ok(Grpc::new(tonic_prost::ProstCodec::default())
                .unary(Self, request)
                .await)
        })
    }
}

#[tokio::test]
async fn brainstem_mtls_requires_client_identity_and_matching_hostname() {
    let Some(certificates) = Certificates::generate() else {
        return;
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let ca = Certificate::from_pem(certificates.read("ca.pem"));
    let identity = Identity::from_pem(
        certificates.read("server.pem"),
        certificates.read("server.key"),
    );
    let (shutdown, receive_shutdown) = oneshot::channel();
    let server = Server::builder()
        .tls_config(
            ServerTlsConfig::new()
                .identity(identity)
                .client_ca_root(ca.clone()),
        )
        .unwrap()
        .add_service(BrainstemVoltage);
    let serving = tokio::spawn(async move {
        server
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = receive_shutdown.await;
            })
            .await
            .unwrap();
    });

    async fn read(endpoint: String, tls: ClientTlsConfig) -> doso_sdk::Result<Voltage> {
        let mut config = MotionBoardConfig::new(endpoint).with_tls(tls);
        config.timeout = Duration::from_secs(3);
        MotionBoardClient::with_config(config)
            .await?
            .voltage()
            .await
    }

    let local = format!("https://localhost:{port}");
    let trusted = ClientTlsConfig::new().ca_certificate(ca);
    let authenticated = trusted.clone().identity(certificates.client_identity());
    assert_eq!(
        read(local.clone(), authenticated.clone())
            .await
            .unwrap()
            .values,
        [52.5]
    );
    assert!(read(local, trusted).await.is_err());
    assert!(read(format!("https://127.0.0.1:{port}"), authenticated)
        .await
        .is_err());

    shutdown.send(()).unwrap();
    serving.await.unwrap();
}
