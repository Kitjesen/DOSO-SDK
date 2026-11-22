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

/// Connection settings for a DOSO robot via [`RobotClient`].
pub struct RobotConfig {
    pub endpoint: String,
    pub client_id: String,
    pub api_key: Option<String>,
    pub timeout: Duration,
    /// Additional PEM CA certificate for the robot's HTTPS endpoint.
    pub tls_ca_certificate: Option<Vec<u8>>,
}

/// Backwards-compatible alias for [`RobotConfig`].
pub type Config = RobotConfig;

impl RobotConfig {
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

/// Unified customer client representing a complete DOSO robot.
///
/// All high-level domains and optional low-level RL streams share configuration,
/// authentication metadata, and lifecycle guarantees through this handle.
pub struct RobotClient {
    pub(crate) channel: Channel,
    pub(crate) info: SystemInfo,
    endpoint: Endpoint,
    client_id: String,
    api_key: Option<MetadataValue<tonic::metadata::Ascii>>,
    pub(crate) timeout: Duration,
    tls_ca_certificate: Option<Vec<u8>>,
}

/// Backwards-compatible alias for [`RobotClient`].
pub type DosoClient = RobotClient;

fn build_endpoint(
    raw: &str,
    timeout: Duration,
    tls_ca_certificate: Option<&[u8]>,
) -> Result<Endpoint> {
    if tls_ca_certificate.is_some() && !raw.starts_with("https://") {
        return Err(Error::invalid(
            "TLS CA certificate requires an https endpoint",
        ));
    }
    let mut endpoint = Endpoint::from_shared(raw.to_owned())
        .map_err(|e| Error::invalid(e.to_string()))?
        .connect_timeout(timeout)
        .tcp_nodelay(true);
    if raw.starts_with("https://") {
        let mut tls = ClientTlsConfig::new().with_native_roots();
        if let Some(certificate) = tls_ca_certificate {
            if certificate.is_empty() {
                return Err(Error::invalid("TLS CA certificate must not be empty"));
            }
            tls = tls.ca_certificate(Certificate::from_pem(certificate));
        }
        endpoint = endpoint
            .tls_config(tls)
            .map_err(|e| Error::invalid(e.to_string()))?;
    } else if !raw.starts_with("http://") {
        return Err(Error::invalid("gRPC endpoint must use http or https"));
    }
    Ok(endpoint)
}

impl RobotClient {
    /// Connect and complete a read-only protocol handshake.
    pub async fn connect(endpoint: impl Into<String>) -> Result<Self> {
        Self::with_config(RobotConfig::new(endpoint)).await
    }

    pub async fn with_config(config: RobotConfig) -> Result<Self> {
        text(&config.client_id, "client_id")?;
        if config.timeout.is_zero() {
            return Err(Error::invalid("timeout must be positive"));
        }
        let endpoint = build_endpoint(
            &config.endpoint,
            config.timeout,
            config.tls_ca_certificate.as_deref(),
        )?;
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
            endpoint,
            client_id: config.client_id,
            api_key,
            timeout: config.timeout,
            tls_ca_certificate: config.tls_ca_certificate,
        };
        client.info = client.handshake(client.channel.clone()).await?;
        Ok(client)
    }

    /// Open a new transport and refresh identity after a successful read-only handshake.
    /// Existing streams are not restored, commands are not replayed, and control is not reclaimed.
    /// On failure, the previous channel and last successful identity snapshot are retained.
    pub async fn reconnect(&mut self) -> Result<()> {
        let channel = self
            .endpoint
            .connect()
            .await
            .map_err(|e| Error::new(ErrorKind::Transport, e.to_string()))?;
        let info = self.handshake(channel.clone()).await?;
        if self.info.robot_id.is_some() && self.info.robot_id != info.robot_id {
            return Err(Error::protocol("robot identity changed during reconnect"));
        }
        self.channel = channel;
        self.info = info;
        Ok(())
    }

    async fn handshake(&self, channel: Channel) -> Result<SystemInfo> {
        let reply = Self::read(
            rpc::system_client::SystemClient::new(channel)
                .handshake(self.request(rpc::Handshake {
                    protocol_version: PROTOCOL_VERSION,
                    client_id: self.client_id.clone(),
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
        reply
            .info
            .ok_or_else(|| Error::protocol("handshake response is missing system info"))
    }

    /// Resolve the transport channel for low-level 50-200 Hz external joint control.
    /// When handshake advertised `SystemInfo.low_level_endpoint`, open a direct channel
    /// to that zero-GIL Rust endpoint; otherwise reuse the main gateway channel.
    pub(crate) async fn low_level_channel(&self) -> Result<Channel> {
        let Some(raw) = self
            .info
            .low_level_endpoint
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(self.channel.clone());
        };
        let endpoint = build_endpoint(raw, self.timeout, self.tls_ca_certificate.as_deref())?;
        endpoint
            .connect()
            .await
            .map_err(|e| Error::new(ErrorKind::Transport, e.to_string()))
    }

    pub fn system(&self) -> System<'_> {
        System(self)
    }
    pub fn motion(&self) -> Motion<'_> {
        Motion(self)
    }
    /// Alias of [`RobotClient::motion`] for sport-level posture, locomotion, and control claims.
    pub fn sport(&self) -> Motion<'_> {
        Motion(self)
    }
    /// Open the gated 50-200 Hz 16-axis MIT low-level controller session for custom RL policies.
    pub async fn low_level(&self, request_id: Option<&str>) -> Result<crate::ExternalControl<'_>> {
        self.motion().external_control(request_id).await
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
