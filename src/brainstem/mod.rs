//! Low-level client for the Brainstem control service on the robot.
//!
//! This client deliberately exposes the native `brainstem.api.v1` protocol.
//! It is not a transport alias for [`crate::DosoClient`], which speaks the
//! public seven-domain `doso.v1` Gateway contract.

use crate::{Error, ErrorKind, Result};
use std::{error::Error as _, future::Future, time::Duration};
use tonic::{
    transport::{Channel, Endpoint},
    Request, Response,
};

pub use tonic::transport::{Certificate, ClientTlsConfig, Identity};

pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:13145";

pub mod rpc {
    tonic::include_proto!("brainstem.api.v1");
}

/// Connection settings for the robot-local Brainstem service.
pub struct BrainstemConfig {
    pub endpoint: String,
    pub timeout: Duration,
    tls: Option<ClientTlsConfig>,
}

impl BrainstemConfig {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            timeout: Duration::from_secs(10),
            tls: None,
        }
    }

    pub fn local() -> Self {
        Self::new(DEFAULT_ENDPOINT)
    }

    /// Configure trust roots and an optional client identity for HTTPS.
    pub fn with_tls(mut self, tls: ClientTlsConfig) -> Self {
        self.tls = Some(tls);
        self
    }
}

/// Native state, telemetry and built-in motion access to Brainstem's gRPC service.
///
/// The default endpoint is `http://127.0.0.1:13145` when the caller is on the
/// robot. This client never opens CAN or serial devices; Brainstem remains the
/// sole hardware owner.
pub struct BrainstemClient {
    channel: Channel,
    endpoint: Endpoint,
    timeout: Duration,
}

/// Client for Brainstem's explicit external-controller session.
///
/// This is a separate handle only because the native protocol exposes a
/// separate gRPC service. It uses the same channel and endpoint as
/// [`BrainstemClient`]; it does not create another hardware owner.
pub struct BrainstemExternalClient {
    channel: Channel,
    timeout: Duration,
}

impl BrainstemClient {
    pub async fn connect(endpoint: impl Into<String>) -> Result<Self> {
        Self::with_config(BrainstemConfig::new(endpoint)).await
    }

    pub async fn with_config(config: BrainstemConfig) -> Result<Self> {
        if config.timeout.is_zero() {
            return Err(Error::invalid("timeout must be positive"));
        }
        let mut endpoint = Endpoint::from_shared(config.endpoint)
            .map_err(|error| Error::invalid(error.to_string()))?
            .connect_timeout(config.timeout);
        let uri = endpoint.uri();
        if uri.host().is_none()
            || uri
                .authority()
                .is_some_and(|value| value.as_str().contains('@'))
            || !matches!(uri.path(), "" | "/")
            || uri.query().is_some()
        {
            return Err(Error::invalid(
                "Brainstem endpoint must be an HTTP(S) origin",
            ));
        }
        match uri.scheme_str() {
            Some("https") => {
                let tls = config
                    .tls
                    .unwrap_or_else(|| ClientTlsConfig::new().with_native_roots());
                endpoint = endpoint
                    .tls_config(tls)
                    .map_err(|error| Error::invalid(error.to_string()))?;
            }
            Some("http") if config.tls.is_none() => {}
            Some("http") => return Err(Error::invalid("Brainstem TLS requires an HTTPS endpoint")),
            _ => return Err(Error::invalid("Brainstem endpoint must use HTTP or HTTPS")),
        }
        let channel = connect_channel(&endpoint).await?;
        Ok(Self {
            channel,
            endpoint,
            timeout: config.timeout,
        })
    }

    /// Reconnect the transport without replaying commands or subscriptions.
    pub async fn reconnect(&mut self) -> Result<()> {
        self.channel = connect_channel(&self.endpoint).await?;
        Ok(())
    }

    pub async fn control_status(&self) -> Result<rpc::ControlStatus> {
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .get_control_status(self.request(()))
                .await,
        )
    }

    pub async fn profile(&self) -> Result<rpc::ProfileInfo> {
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .get_profile(self.request(()))
                .await,
        )
    }

    pub async fn voltage(&self) -> Result<rpc::Voltage> {
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .get_voltage(self.request(()))
                .await,
        )
    }

    pub async fn motor_status(&self) -> Result<rpc::MotorStatusResponse> {
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .get_motor_status(self.request(()))
                .await,
        )
    }

    pub async fn enable_motors(&self) -> Result<()> {
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .enable(self.request(()))
                .await,
        )
    }

    pub async fn disable_motors(&self) -> Result<()> {
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .disable(self.request(()))
                .await,
        )
    }

    pub async fn stand_up(&self) -> Result<()> {
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .stand_up(self.request(()))
                .await,
        )
    }

    pub async fn sit_down(&self) -> Result<()> {
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .sit_down(self.request(()))
                .await,
        )
    }

    pub async fn acquire_control(&self, client_id: impl Into<String>) -> Result<rpc::ControlLease> {
        let client_id = client_id.into();
        if client_id.trim().is_empty() {
            return Err(Error::invalid("client_id must not be empty"));
        }
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .acquire_control(self.request(rpc::AcquireControlRequest { client_id }))
                .await,
        )
    }

    pub async fn renew_control_lease(&self, token: impl Into<String>) -> Result<rpc::ControlLease> {
        let token = control_token(token.into())?;
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .renew_control_lease(self.request(rpc::ControlLeaseRequest { token }))
                .await,
        )
    }

    pub async fn release_control(&self, token: impl Into<String>) -> Result<rpc::ControlStatus> {
        let token = control_token(token.into())?;
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .release_control(self.request(rpc::ControlLeaseRequest { token }))
                .await,
        )
    }

    pub async fn walk_checked(
        &self,
        token: impl Into<String>,
        sequence: u64,
        direction: rpc::Vector3,
    ) -> Result<rpc::CommandAck> {
        let token = control_token(token.into())?;
        if !direction.x.is_finite() || !direction.y.is_finite() || !direction.z.is_finite() {
            return Err(Error::invalid("walk direction must be finite"));
        }
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .walk_checked(self.request(rpc::WalkRequest {
                    token,
                    sequence,
                    direction: Some(direction),
                }))
                .await,
        )
    }

    pub async fn switch_profile(&self, name: impl Into<String>) -> Result<rpc::ProfileInfo> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid("profile name must not be empty"));
        }
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .switch_profile(self.request(rpc::ProfileRequest { name }))
                .await,
        )
    }

    /// Set the native speed mode.
    pub async fn set_speed_mode(&self, mode: rpc::SpeedMode) -> Result<()> {
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .set_speed_mode(self.request(rpc::SpeedModeRequest { mode: mode as i32 }))
                .await,
        )
    }

    pub async fn speed_mode(&self) -> Result<rpc::SpeedModeRequest> {
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .get_speed_mode(self.request(()))
                .await,
        )
    }

    /// Clear motor faults. An empty list asks Brainstem to clear all faults.
    pub async fn clear_motor_fault(&self, joint_ids: impl IntoIterator<Item = u32>) -> Result<()> {
        let joint_ids = joint_ids.into_iter().collect::<Vec<_>>();
        if joint_ids.iter().any(|id| *id >= 16) {
            return Err(Error::invalid("joint id must be between 0 and 15"));
        }
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .clear_motor_fault(self.request(rpc::ClearFaultRequest { joint_ids }))
                .await,
        )
    }

    pub async fn set_zero(&self) -> Result<()> {
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .set_zero(self.request(()))
                .await,
        )
    }

    pub async fn listen_imu(&self) -> Result<tonic::Streaming<rpc::Imu>> {
        let mut client = rpc::robot_control_client::RobotControlClient::new(self.channel.clone());
        self.open_observation("ListenImu", client.listen_imu(Request::new(())))
            .await
    }

    pub async fn listen_joint(&self) -> Result<tonic::Streaming<rpc::Joint>> {
        let mut client = rpc::robot_control_client::RobotControlClient::new(self.channel.clone());
        self.open_observation("ListenJoint", client.listen_joint(Request::new(())))
            .await
    }

    async fn open_observation<T>(
        &self,
        method: &str,
        response: impl Future<Output = std::result::Result<Response<T>, tonic::Status>>,
    ) -> Result<T> {
        let result = tokio::time::timeout(self.timeout, response)
            .await
            .map_err(|_| {
                Error::new(
                    ErrorKind::Timeout,
                    format!("{method} subscription timed out"),
                )
            })?;
        read(result)
    }

    /// Open the native external-controller service on the same Brainstem
    /// connection. The caller must still enter and explicitly enable a
    /// session before sending joint targets.
    pub fn external(&self) -> BrainstemExternalClient {
        BrainstemExternalClient {
            channel: self.channel.clone(),
            timeout: self.timeout,
        }
    }

    fn request<T>(&self, body: T) -> Request<T> {
        let mut request = Request::new(body);
        request.set_timeout(self.timeout);
        request
    }
}

impl BrainstemExternalClient {
    pub async fn enter(
        &self,
        client_id: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Result<rpc::ExternalStatus> {
        let client_id = client_id.into();
        if client_id.trim().is_empty() {
            return Err(Error::invalid("client_id must not be empty"));
        }
        read(
            rpc::robot_external_control_client::RobotExternalControlClient::new(
                self.channel.clone(),
            )
            .enter(self.request(rpc::ExternalEnterRequest {
                client_id,
                request_id: request_id.into(),
            }))
            .await,
        )
    }

    pub async fn enable(&self, session_token: impl Into<String>) -> Result<rpc::ExternalStatus> {
        let token = control_token(session_token.into())?;
        let mut client = rpc::robot_external_control_client::RobotExternalControlClient::new(
            self.channel.clone(),
        );
        read(
            client
                .enable(self.request(rpc::ExternalSession {
                    session_token: token,
                }))
                .await,
        )
    }

    pub async fn disable(&self, session_token: impl Into<String>) -> Result<rpc::ExternalStatus> {
        let token = control_token(session_token.into())?;
        let mut client = rpc::robot_external_control_client::RobotExternalControlClient::new(
            self.channel.clone(),
        );
        read(
            client
                .disable(self.request(rpc::ExternalSession {
                    session_token: token,
                }))
                .await,
        )
    }

    pub async fn heartbeat(&self, session_token: impl Into<String>) -> Result<rpc::ExternalStatus> {
        let token = control_token(session_token.into())?;
        let mut client = rpc::robot_external_control_client::RobotExternalControlClient::new(
            self.channel.clone(),
        );
        read(
            client
                .heartbeat(self.request(rpc::ExternalSession {
                    session_token: token,
                }))
                .await,
        )
    }

    pub async fn exit(&self, session_token: impl Into<String>) -> Result<rpc::ExternalStatus> {
        let token = control_token(session_token.into())?;
        let mut client = rpc::robot_external_control_client::RobotExternalControlClient::new(
            self.channel.clone(),
        );
        read(
            client
                .exit(self.request(rpc::ExternalSession {
                    session_token: token,
                }))
                .await,
        )
    }

    pub async fn status(&self) -> Result<rpc::ExternalStatus> {
        read(
            rpc::robot_external_control_client::RobotExternalControlClient::new(
                self.channel.clone(),
            )
            .get_status(self.request(()))
            .await,
        )
    }

    /// Start the bidirectional stream for 16-joint external targets.
    ///
    /// The stream itself carries the input window and sequence supplied by
    /// Brainstem. The SDK does not retry or replay frames after a disconnect.
    pub async fn control(
        &self,
        frames: impl tonic::IntoStreamingRequest<Message = rpc::ExternalFrame>,
    ) -> Result<tonic::Streaming<rpc::ExternalStatus>> {
        read(
            rpc::robot_external_control_client::RobotExternalControlClient::new(
                self.channel.clone(),
            )
            .control(frames)
            .await,
        )
    }

    fn request<T>(&self, body: T) -> Request<T> {
        let mut request = Request::new(body);
        request.set_timeout(self.timeout);
        request
    }
}

async fn connect_channel(endpoint: &Endpoint) -> Result<Channel> {
    endpoint.connect().await.map_err(|error| {
        let mut message = format!("Brainstem {}: {error}", endpoint.uri());
        let mut cause = error.source();
        while let Some(error) = cause {
            message.push_str(&format!(": {error}"));
            cause = error.source();
        }
        Error::new(ErrorKind::Transport, message)
    })
}

fn control_token(token: String) -> Result<String> {
    if token.trim().is_empty() {
        Err(Error::invalid("control token must not be empty"))
    } else {
        Ok(token)
    }
}

fn read<T>(result: std::result::Result<Response<T>, tonic::Status>) -> Result<T> {
    result
        .map(Response::into_inner)
        .map_err(|status| Error::rpc(status, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_config_targets_brainstem() {
        assert_eq!(BrainstemConfig::local().endpoint, DEFAULT_ENDPOINT);
    }

    #[tokio::test]
    async fn rejects_invalid_origins_and_tls_over_http() {
        for config in [
            BrainstemConfig::new("ftp://robot.example:13145"),
            BrainstemConfig::new("https://robot.example:13145/status"),
            BrainstemConfig::local().with_tls(ClientTlsConfig::new()),
        ] {
            let error = BrainstemClient::with_config(config).await.err().unwrap();
            assert_eq!(error.kind, ErrorKind::InvalidInput);
        }
    }

    #[tokio::test]
    async fn observation_open_timeout_is_local() {
        let endpoint = Endpoint::from_static(DEFAULT_ENDPOINT);
        let client = BrainstemClient {
            channel: endpoint.connect_lazy(),
            endpoint,
            timeout: Duration::from_millis(10),
        };
        let error = client
            .open_observation::<()>("ListenImu", std::future::pending())
            .await
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::DeadlineExceeded);
        assert_eq!(error.message, "ListenImu subscription timed out");
    }

    #[tokio::test]
    async fn rejects_zero_timeout_before_connecting() {
        let mut config = BrainstemConfig::local();
        config.timeout = Duration::ZERO;
        let error = match BrainstemClient::with_config(config).await {
            Err(error) => error,
            Ok(_) => panic!("zero timeout must be rejected"),
        };
        assert_eq!(error.kind, ErrorKind::InvalidInput);
    }

    #[test]
    fn rejects_empty_control_inputs() {
        assert!(control_token(String::new()).is_err());
        assert!(control_token("  ".to_owned()).is_err());
    }
}
