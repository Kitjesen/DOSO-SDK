use super::{Config, DosoClient, ErrorKind, Service};
use std::{fs, path::PathBuf, process::Command, time::Duration};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Identity, Server, ServerTlsConfig};

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
            let result = Command::new("openssl")
                .args(arguments)
                .current_dir(&certificates.0)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "test certificate generation failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        Some(certificates)
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
