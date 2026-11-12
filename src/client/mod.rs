use crate::{
    common::text, new_request_id, rpc, CommandReceipt, Error, ErrorKind, Localization, Maps,
    Motion, Navigation, Observations, Result, System, SystemInfo, Tasks,
};
use std::time::Duration;
use tonic::{
    metadata::MetadataValue,
    transport::{Certificate, Channel, ClientTlsConfig, Endpoint},
    Request, Response,
};

const PROTOCOL_VERSION: u32 = 2;

pub struct Config {
    pub endpoint: String,
    pub client_id: String,
    pub api_key: Option<String>,
    pub timeout: Duration,
    /// Additional PEM CA certificate for the robot's HTTPS endpoint.
    pub tls_ca_certificate: Option<Vec<u8>>,
}
impl Config {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            client_id: "doso-sdk".into(),
            api_key: None,
            timeout: Duration::from_secs(10),
            tls_ca_certificate: None,
        }
    }
}

/// All seven domains share this channel and request metadata.
pub struct DosoClient {
    pub(crate) channel: Channel,
    pub(crate) info: SystemInfo,
    client_id: String,
    api_key: Option<MetadataValue<tonic::metadata::Ascii>>,
    pub(crate) timeout: Duration,
}
impl DosoClient {
    /// Connect and complete a read-only protocol handshake.
    pub async fn connect(endpoint: impl Into<String>) -> Result<Self> {
        Self::with_config(Config::new(endpoint)).await
    }
    pub async fn with_config(config: Config) -> Result<Self> {
        text(&config.client_id, "client_id")?;
        if config.timeout.is_zero() {
            return Err(Error::invalid("timeout must be positive"));
        }
        if config.tls_ca_certificate.is_some() && !config.endpoint.starts_with("https://") {
            return Err(Error::invalid(
                "TLS CA certificate requires an https endpoint",
            ));
        }
        let mut endpoint = Endpoint::from_shared(config.endpoint.clone())
            .map_err(|e| Error::invalid(e.to_string()))?
            .connect_timeout(config.timeout);
        if config.endpoint.starts_with("https://") {
            let mut tls = ClientTlsConfig::new().with_native_roots();
            if let Some(certificate) = config.tls_ca_certificate {
                if certificate.is_empty() {
                    return Err(Error::invalid("TLS CA certificate must not be empty"));
                }
                tls = tls.ca_certificate(Certificate::from_pem(certificate));
            }
            endpoint = endpoint
                .tls_config(tls)
                .map_err(|e| Error::invalid(e.to_string()))?;
        } else if !config.endpoint.starts_with("http://") {
            return Err(Error::invalid("gRPC endpoint must use http or https"));
        }
        let api_key = config
            .api_key
            .map(|key| {
                let mut value = MetadataValue::try_from(key)
                    .map_err(|_| Error::invalid("invalid API key metadata"))?;
                value.set_sensitive(true);
                Ok(value)
            })
            .transpose()?;
        let channel = endpoint
            .connect()
            .await
            .map_err(|e| Error::new(ErrorKind::Transport, e.to_string()))?;
        let mut client = Self {
            channel,
            info: SystemInfo::default(),
            client_id: config.client_id,
            api_key,
            timeout: config.timeout,
        };
        let reply = Self::read(
            rpc::system_client::SystemClient::new(client.channel.clone())
                .handshake(client.request(rpc::Handshake {
                    protocol_version: PROTOCOL_VERSION,
                    client_id: client.client_id.clone(),
                    sdk_version: env!("CARGO_PKG_VERSION").into(),
                }))
                .await,
        )?;
        if reply.protocol_version != PROTOCOL_VERSION {
            return Err(Error::protocol(format!(
                "unsupported server protocol version: {}",
                reply.protocol_version
            )));
        }
        client.info = reply
            .info
            .ok_or_else(|| Error::protocol("handshake response is missing system info"))?;
        Ok(client)
    }
    pub fn system(&self) -> System<'_> {
        System(self)
    }
    pub fn motion(&self) -> Motion<'_> {
        Motion(self)
    }
    pub fn localization(&self) -> Localization<'_> {
        Localization(self)
    }
    pub fn maps(&self) -> Maps<'_> {
        Maps(self)
    }
    pub fn navigation(&self) -> Navigation<'_> {
        Navigation(self)
    }
    pub fn observations(&self) -> Observations<'_> {
        Observations(self)
    }
    pub fn tasks(&self) -> Tasks<'_> {
        Tasks(self)
    }

    pub(crate) fn request<T>(&self, body: T) -> Request<T> {
        self.request_timeout(body, self.timeout)
    }
    pub(crate) fn request_timeout<T>(&self, body: T, timeout: Duration) -> Request<T> {
        let mut request = self.stream_request(body);
        request.set_timeout(timeout);
        request
    }
    pub(crate) fn stream_request<T>(&self, body: T) -> Request<T> {
        let mut request = Request::new(body);
        if let Some(key) = &self.api_key {
            request.metadata_mut().insert("x-api-key", key.clone());
        }
        request
    }
    pub(crate) fn command(&self, request_id: Option<&str>) -> Result<rpc::Command> {
        let request_id = match request_id {
            Some(id) => {
                text(id, "request_id")?;
                id.to_owned()
            }
            None => new_request_id()?,
        };
        Ok(rpc::Command {
            request_id,
            client_id: self.client_id.clone(),
        })
    }
    pub(crate) fn receipt(
        result: std::result::Result<Response<CommandReceipt>, tonic::Status>,
        request_id: &str,
    ) -> Result<CommandReceipt> {
        let receipt = result
            .map_err(|e| Error::rpc(e, Some(request_id)))?
            .into_inner();
        if receipt.request_id != request_id {
            return Err(Error::protocol("receipt request_id mismatch").request(request_id));
        }
        if receipt.ok && receipt.accepted == Some(false) {
            return Err(
                Error::protocol("receipt success contradicts rejected admission")
                    .request(request_id),
            );
        }
        Ok(receipt)
    }
    pub(crate) fn read<T>(result: std::result::Result<Response<T>, tonic::Status>) -> Result<T> {
        result
            .map(Response::into_inner)
            .map_err(|e| Error::rpc(e, None))
    }
}
