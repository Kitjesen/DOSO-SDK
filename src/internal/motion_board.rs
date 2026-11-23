//! Internal Board-A client for onboard diagnostic and bridge use only.
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
pub struct MotionBoardConfig {
    pub endpoint: String,
    pub timeout: Duration,
    tls: Option<ClientTlsConfig>,
}

impl MotionBoardConfig {
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
pub struct MotionBoardClient {
    channel: Channel,
    endpoint: Endpoint,
    timeout: Duration,
}

/// Client for Brainstem's explicit external-controller session.
///
/// This is a separate handle only because the native protocol exposes a
/// separate gRPC service. It uses the same channel and endpoint as
/// [`MotionBoardClient`]; it does not create another hardware owner.
pub struct MotionBoardExternalClient {
    channel: Channel,
    timeout: Duration,
}

/// Client for Brainstem's explicit fall-recovery service (`brainstem.api.v1.RobotRecovery`).
pub struct MotionBoardRecoveryClient {
    channel: Channel,
    timeout: Duration,
}

impl MotionBoardClient {
    pub async fn connect(endpoint: impl Into<String>) -> Result<Self> {
        Self::with_config(MotionBoardConfig::new(endpoint)).await
    }

    pub async fn with_config(config: MotionBoardConfig) -> Result<Self> {
        if config.timeout.is_zero() {
            return Err(Error::invalid("timeout must be positive"));
        }
        let mut endpoint = Endpoint::from_shared(config.endpoint)
            .map_err(|error| Error::invalid(error.to_string()))?
            .connect_timeout(config.timeout)
            .tcp_nodelay(true);
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

    pub async fn cms_state(&self) -> Result<rpc::CmsState> {
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .get_cms_state(self.request(()))
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

    pub async fn list_gestures(&self) -> Result<rpc::GestureList> {
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .list_gestures(self.request(()))
                .await,
        )
    }

    /// Play a named onboard gesture.
    pub async fn play_gesture(&self, name: impl Into<String>) -> Result<()> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid("gesture name must not be empty"));
        }
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .play_gesture(self.request(rpc::GestureRequest { name }))
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

    /// Trigger an analytic damping soft-stop (`Damp`).
    pub async fn damp(&self) -> Result<()> {
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .damp(self.request(()))
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

    pub async fn body_height_status(&self) -> Result<rpc::BodyHeightStatus> {
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .get_body_height_status(self.request(()))
                .await,
        )
    }

    pub async fn set_body_height(&self, height_m: f64) -> Result<rpc::BodyHeightStatus> {
        if !height_m.is_finite() {
            return Err(Error::invalid("height_m must be finite"));
        }
        read(
            rpc::robot_control_client::RobotControlClient::new(self.channel.clone())
                .set_body_height(self.request(rpc::BodyHeightCommand { height_m }))
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

    pub async fn listen_cms_state(&self) -> Result<tonic::Streaming<rpc::CmsState>> {
        let mut client = rpc::robot_control_client::RobotControlClient::new(self.channel.clone());
        self.open_observation("ListenCmsState", client.listen_cms_state(Request::new(())))
            .await
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
    pub fn external(&self) -> MotionBoardExternalClient {
        MotionBoardExternalClient {
            channel: self.channel.clone(),
            timeout: self.timeout,
        }
    }

    /// Open the native fall-recovery service (`RobotRecovery`) on the same Brainstem channel.
    pub fn recovery(&self) -> MotionBoardRecoveryClient {
        MotionBoardRecoveryClient {
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

impl MotionBoardExternalClient {
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

impl MotionBoardRecoveryClient {
    pub async fn status(&self) -> Result<rpc::RecoveryStatus> {
        read(
            rpc::robot_recovery_client::RobotRecoveryClient::new(self.channel.clone())
                .get_status(self.request(()))
                .await,
        )
    }

    pub async fn enter(&self, operation_id: impl Into<String>) -> Result<rpc::RecoveryStatus> {
        let operation_id = operation_id.into();
        if operation_id.trim().is_empty() {
            return Err(Error::invalid("operation_id must not be empty"));
        }
        read(
            rpc::robot_recovery_client::RobotRecoveryClient::new(self.channel.clone())
                .enter(self.request(rpc::RecoveryStartRequest {
                    operation_id,
                    session_token: String::new(),
                }))
                .await,
        )
    }

    pub async fn enable(&self, session_token: impl Into<String>) -> Result<rpc::RecoveryStatus> {
        let token = control_token(session_token.into())?;
        read(
            rpc::robot_recovery_client::RobotRecoveryClient::new(self.channel.clone())
                .enable(self.request(rpc::RecoverySessionRequest {
                    session_token: token,
                }))
                .await,
        )
    }

    pub async fn heartbeat(&self, session_token: impl Into<String>) -> Result<rpc::RecoveryStatus> {
        let token = control_token(session_token.into())?;
        read(
            rpc::robot_recovery_client::RobotRecoveryClient::new(self.channel.clone())
                .heartbeat(self.request(rpc::RecoverySessionRequest {
                    session_token: token,
                }))
                .await,
        )
    }

    pub async fn start(
        &self,
        operation_id: impl Into<String>,
        session_token: impl Into<String>,
    ) -> Result<rpc::RecoveryActionReply> {
        let operation_id = operation_id.into();
        if operation_id.trim().is_empty() {
            return Err(Error::invalid("operation_id must not be empty"));
        }
        let token = control_token(session_token.into())?;
        read(
            rpc::robot_recovery_client::RobotRecoveryClient::new(self.channel.clone())
                .start(self.request(rpc::RecoveryStartRequest {
                    operation_id,
                    session_token: token,
                }))
                .await,
        )
    }

    pub async fn cancel(
        &self,
        operation_id: impl Into<String>,
        session_token: impl Into<String>,
    ) -> Result<rpc::RecoveryActionReply> {
        let operation_id = operation_id.into();
        if operation_id.trim().is_empty() {
            return Err(Error::invalid("operation_id must not be empty"));
        }
        let token = control_token(session_token.into())?;
        read(
            rpc::robot_recovery_client::RobotRecoveryClient::new(self.channel.clone())
                .cancel(self.request(rpc::RecoveryCancelRequest {
                    operation_id,
                    session_token: token,
                }))
                .await,
        )
    }

    pub async fn exit(&self, session_token: impl Into<String>) -> Result<rpc::RecoveryStatus> {
        let token = control_token(session_token.into())?;
        read(
            rpc::robot_recovery_client::RobotRecoveryClient::new(self.channel.clone())
                .exit(self.request(rpc::RecoverySessionRequest {
                    session_token: token,
                }))
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
        assert_eq!(MotionBoardConfig::local().endpoint, DEFAULT_ENDPOINT);
    }

    #[tokio::test]
    async fn rejects_invalid_origins_and_tls_over_http() {
        for config in [
            MotionBoardConfig::new("ftp://robot.example:13145"),
            MotionBoardConfig::new("https://robot.example:13145/status"),
            MotionBoardConfig::local().with_tls(ClientTlsConfig::new()),
        ] {
            let error = MotionBoardClient::with_config(config).await.err().unwrap();
            assert_eq!(error.kind, ErrorKind::InvalidInput);
        }
    }

    #[tokio::test]
    async fn observation_open_timeout_is_local() {
        let endpoint = Endpoint::from_static(DEFAULT_ENDPOINT);
        let client = MotionBoardClient {
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
        let mut config = MotionBoardConfig::local();
        config.timeout = Duration::ZERO;
        let error = match MotionBoardClient::with_config(config).await {
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

    #[tokio::test]
    async fn rejects_empty_gesture_name_before_connecting() {
        let endpoint = Endpoint::from_static(DEFAULT_ENDPOINT);
        let client = MotionBoardClient {
            channel: endpoint.connect_lazy(),
            endpoint,
            timeout: Duration::from_millis(50),
        };
        for name in ["", "   "] {
            let error = client.play_gesture(name).await.unwrap_err();
            assert_eq!(error.kind, ErrorKind::InvalidInput);
        }
    }

    struct MockRobotControl {
        played: std::sync::Mutex<Vec<String>>,
        damped: std::sync::Mutex<usize>,
    }

    type MockStream<T> =
        tokio_stream::wrappers::ReceiverStream<std::result::Result<T, tonic::Status>>;

    #[tonic::async_trait]
    impl rpc::robot_control_server::RobotControl for MockRobotControl {
        type ListenHistoryStream = MockStream<rpc::History>;
        type ListenCmsStateStream = MockStream<rpc::CmsState>;
        type ListenImuStream = MockStream<rpc::Imu>;
        type ListenJointStream = MockStream<rpc::Joint>;

        async fn enable(&self, _: Request<()>) -> std::result::Result<Response<()>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn disable(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<()>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn walk(
            &self,
            _: Request<rpc::Vector3>,
        ) -> std::result::Result<Response<()>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn walk_checked(
            &self,
            _: Request<rpc::WalkRequest>,
        ) -> std::result::Result<Response<rpc::CommandAck>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn acquire_control(
            &self,
            _: Request<rpc::AcquireControlRequest>,
        ) -> std::result::Result<Response<rpc::ControlLease>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn renew_control_lease(
            &self,
            _: Request<rpc::ControlLeaseRequest>,
        ) -> std::result::Result<Response<rpc::ControlLease>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn release_control(
            &self,
            _: Request<rpc::ControlLeaseRequest>,
        ) -> std::result::Result<Response<rpc::ControlStatus>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn stand_up(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<()>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn sit_down(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<()>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn damp(&self, _: Request<()>) -> std::result::Result<Response<()>, tonic::Status> {
            *self.damped.lock().unwrap() += 1;
            Ok(Response::new(()))
        }

        async fn set_speed_mode(
            &self,
            _: Request<rpc::SpeedModeRequest>,
        ) -> std::result::Result<Response<()>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn get_speed_mode(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::SpeedModeRequest>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn tick(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::History>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn step(
            &self,
            _: Request<rpc::SimState>,
        ) -> std::result::Result<Response<()>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn get_start_time(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<prost_types::Timestamp>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn get_cms_state(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::CmsState>, tonic::Status> {
            Ok(Response::new(rpc::CmsState {
                kind: rpc::CmsStateKind::Standing as i32,
                transition: rpc::CmsTransitionKind::None as i32,
                gesture_name: String::new(),
            }))
        }

        async fn get_control_status(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::ControlStatus>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn listen_history(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<Self::ListenHistoryStream>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn listen_cms_state(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<Self::ListenCmsStateStream>, tonic::Status> {
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tx.send(Ok(rpc::CmsState {
                kind: rpc::CmsStateKind::Standing as i32,
                transition: rpc::CmsTransitionKind::None as i32,
                gesture_name: String::new(),
            }))
            .await
            .unwrap();
            tx.send(Ok(rpc::CmsState {
                kind: rpc::CmsStateKind::Transitioning as i32,
                transition: rpc::CmsTransitionKind::Gesture as i32,
                gesture_name: "bow".into(),
            }))
            .await
            .unwrap();
            tx.send(Ok(rpc::CmsState {
                kind: rpc::CmsStateKind::Damping as i32,
                transition: rpc::CmsTransitionKind::None as i32,
                gesture_name: String::new(),
            }))
            .await
            .unwrap();
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
                rx,
            )))
        }

        async fn listen_imu(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<Self::ListenImuStream>, tonic::Status> {
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tx.send(Ok(rpc::Imu {
                gyroscope: Some(rpc::Vector3 {
                    x: 0.01,
                    y: -0.02,
                    z: 0.03,
                }),
                quaternion: Some(rpc::Quaternion {
                    w: 1.0,
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                }),
                linear_acceleration: Some(rpc::Vector3 {
                    x: 0.0,
                    y: 0.0,
                    z: 9.81,
                }),
                timestamp: Some(prost_types::Duration {
                    seconds: 0,
                    nanos: 10_000_000,
                }),
            }))
            .await
            .unwrap();
            tx.send(Ok(rpc::Imu {
                gyroscope: Some(rpc::Vector3 {
                    x: 0.02,
                    y: -0.01,
                    z: 0.0,
                }),
                quaternion: Some(rpc::Quaternion {
                    w: 1.0,
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                }),
                linear_acceleration: Some(rpc::Vector3 {
                    x: 0.1,
                    y: -0.1,
                    z: 9.80,
                }),
                timestamp: Some(prost_types::Duration {
                    seconds: 0,
                    nanos: 20_000_000,
                }),
            }))
            .await
            .unwrap();
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
                rx,
            )))
        }

        async fn listen_joint(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<Self::ListenJointStream>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn get_params(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::Params>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn switch_profile(
            &self,
            _: Request<rpc::ProfileRequest>,
        ) -> std::result::Result<Response<rpc::ProfileInfo>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn set_body_height(
            &self,
            request: Request<rpc::BodyHeightCommand>,
        ) -> std::result::Result<Response<rpc::BodyHeightStatus>, tonic::Status> {
            let height_m = request.into_inner().height_m;
            Ok(Response::new(rpc::BodyHeightStatus {
                supported: true,
                current_height_m: height_m,
                target_height_m: height_m,
                min_height_m: 0.25,
                max_height_m: 0.50,
                rate_mps: 0.05,
            }))
        }

        async fn get_body_height_status(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::BodyHeightStatus>, tonic::Status> {
            Ok(Response::new(rpc::BodyHeightStatus {
                supported: true,
                current_height_m: 0.375,
                target_height_m: 0.375,
                min_height_m: 0.25,
                max_height_m: 0.50,
                rate_mps: 0.05,
            }))
        }

        async fn get_profile(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::ProfileInfo>, tonic::Status> {
            Ok(Response::new(rpc::ProfileInfo {
                current: "flat_w5".into(),
                available: vec!["flat_w5".into(), "cts_v39".into()],
                descriptions: vec!["Flat".into(), "Stairs".into()],
                current_description: "Flat".into(),
                mode_capabilities: vec![rpc::RobotModeCapability {
                    id: "flat".into(),
                    kind: rpc::RobotModeKind::LocomotionProfile as i32,
                    availability: rpc::RobotModeAvailability::Available as i32,
                    profile_id: "flat_w5".into(),
                    unavailable_reason: String::new(),
                    body_height: None,
                }],
                active_mode_id: "flat".into(),
            }))
        }

        async fn play_gesture(
            &self,
            request: Request<rpc::GestureRequest>,
        ) -> std::result::Result<Response<()>, tonic::Status> {
            self.played.lock().unwrap().push(request.into_inner().name);
            Ok(Response::new(()))
        }

        async fn list_gestures(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::GestureList>, tonic::Status> {
            Ok(Response::new(rpc::GestureList {
                gestures: vec![rpc::GestureInfo {
                    name: "bow".into(),
                    description: "Bow".into(),
                    duration_ms: 1800,
                }],
            }))
        }

        async fn get_voltage(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::Voltage>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn get_motor_status(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::MotorStatusResponse>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn clear_motor_fault(
            &self,
            _: Request<rpc::ClearFaultRequest>,
        ) -> std::result::Result<Response<()>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }

        async fn set_zero(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<()>, tonic::Status> {
            Err(tonic::Status::unimplemented("unused"))
        }
    }

    #[tokio::test]
    async fn v2_3_profile_cms_state_and_gestures_work_over_grpc() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let service = std::sync::Arc::new(MockRobotControl {
            played: std::sync::Mutex::new(Vec::new()),
            damped: std::sync::Mutex::new(0),
        });
        let (shutdown, rx_shutdown) = tokio::sync::oneshot::channel();
        let server = tonic::transport::Server::builder()
            .add_service(rpc::robot_control_server::RobotControlServer::from_arc(
                service.clone(),
            ))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async {
                    let _ = rx_shutdown.await;
                },
            );
        let serving = tokio::spawn(server);

        let client = MotionBoardClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();

        let profile = client.profile().await.unwrap();
        assert_eq!(profile.active_mode_id, "flat");
        assert_eq!(profile.mode_capabilities.len(), 1);
        assert_eq!(profile.mode_capabilities[0].profile_id, "flat_w5");

        let cms = client.cms_state().await.unwrap();
        assert_eq!(cms.kind, rpc::CmsStateKind::Standing as i32);

        let mut cms_stream = client.listen_cms_state().await.unwrap();
        let first = cms_stream.message().await.unwrap().unwrap();
        let second = cms_stream.message().await.unwrap().unwrap();
        assert_eq!(first.kind, rpc::CmsStateKind::Standing as i32);
        assert_eq!(second.transition, rpc::CmsTransitionKind::Gesture as i32);
        assert_eq!(second.gesture_name, "bow");

        let gestures = client.list_gestures().await.unwrap();
        assert_eq!(gestures.gestures.len(), 1);
        assert_eq!(gestures.gestures[0].name, "bow");

        client.play_gesture("bow").await.unwrap();
        assert_eq!(*service.played.lock().unwrap(), vec!["bow"]);

        shutdown.send(()).unwrap();
        serving.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn v2_4_damp_and_imu_stream_work_over_grpc() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let service = std::sync::Arc::new(MockRobotControl {
            played: std::sync::Mutex::new(Vec::new()),
            damped: std::sync::Mutex::new(0),
        });
        let (shutdown, rx_shutdown) = tokio::sync::oneshot::channel();
        let server = tonic::transport::Server::builder()
            .add_service(rpc::robot_control_server::RobotControlServer::from_arc(
                service.clone(),
            ))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async {
                    let _ = rx_shutdown.await;
                },
            );
        let serving = tokio::spawn(server);

        let client = MotionBoardClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();

        client.damp().await.unwrap();
        assert_eq!(*service.damped.lock().unwrap(), 1);

        let mut cms_stream = client.listen_cms_state().await.unwrap();
        let _ = cms_stream.message().await.unwrap().unwrap();
        let _ = cms_stream.message().await.unwrap().unwrap();
        let third = cms_stream.message().await.unwrap().unwrap();
        assert_eq!(third.kind, rpc::CmsStateKind::Damping as i32);

        let mut imu_stream = client.listen_imu().await.unwrap();
        let first_imu = imu_stream.message().await.unwrap().unwrap();
        let second_imu = imu_stream.message().await.unwrap().unwrap();
        assert_eq!(first_imu.linear_acceleration.as_ref().unwrap().z, 9.81);
        assert_eq!(second_imu.linear_acceleration.as_ref().unwrap().z, 9.80);
        let t1 = first_imu.timestamp.unwrap();
        let t2 = second_imu.timestamp.unwrap();
        assert!((t2.seconds, t2.nanos) > (t1.seconds, t1.nanos));

        shutdown.send(()).unwrap();
        serving.await.unwrap().unwrap();
    }

    struct MockRobotExternalControl {
        state: std::sync::Arc<std::sync::Mutex<ExternalMockState>>,
    }

    struct ExternalMockState {
        selected: bool,
        active: bool,
        motor_output_enabled: bool,
        last_sequence: u64,
        heartbeat_count: usize,
        reason: String,
    }

    fn external_snapshot(
        state: &std::sync::Mutex<ExternalMockState>,
        include_token: bool,
    ) -> rpc::ExternalStatus {
        let guard = state.lock().unwrap();
        rpc::ExternalStatus {
            selected: guard.selected,
            active: guard.active,
            session_id: if guard.selected {
                "req-ext-1".into()
            } else {
                String::new()
            },
            session_token: if include_token {
                "tok-ext-1".into()
            } else {
                String::new()
            },
            motor_output_enabled: guard.motor_output_enabled,
            last_sequence: guard.last_sequence,
            reason: guard.reason.clone(),
            motor_disable_confirmed: !guard.motor_output_enabled,
            client_id: if guard.selected {
                "cust-ext".into()
            } else {
                String::new()
            },
            input_window: if guard.active { 7 } else { 0 },
            joint_count: 16,
        }
    }

    #[tonic::async_trait]
    impl rpc::robot_external_control_server::RobotExternalControl for MockRobotExternalControl {
        type ControlStream = MockStream<rpc::ExternalStatus>;

        async fn enter(
            &self,
            _: Request<rpc::ExternalEnterRequest>,
        ) -> std::result::Result<Response<rpc::ExternalStatus>, tonic::Status> {
            {
                let mut guard = self.state.lock().unwrap();
                guard.selected = true;
                guard.active = true;
                guard.reason = "external_ready".into();
            }
            Ok(Response::new(external_snapshot(&self.state, true)))
        }

        async fn enable(
            &self,
            request: Request<rpc::ExternalSession>,
        ) -> std::result::Result<Response<rpc::ExternalStatus>, tonic::Status> {
            if request.into_inner().session_token != "tok-ext-1" {
                return Err(tonic::Status::failed_precondition("session mismatch"));
            }
            {
                let mut guard = self.state.lock().unwrap();
                guard.motor_output_enabled = true;
                guard.reason = "external_running".into();
            }
            Ok(Response::new(external_snapshot(&self.state, false)))
        }

        async fn disable(
            &self,
            request: Request<rpc::ExternalSession>,
        ) -> std::result::Result<Response<rpc::ExternalStatus>, tonic::Status> {
            if request.into_inner().session_token != "tok-ext-1" {
                return Err(tonic::Status::failed_precondition("session mismatch"));
            }
            {
                let mut guard = self.state.lock().unwrap();
                guard.motor_output_enabled = false;
                guard.reason = "external_disabled".into();
            }
            Ok(Response::new(external_snapshot(&self.state, false)))
        }

        async fn heartbeat(
            &self,
            request: Request<rpc::ExternalSession>,
        ) -> std::result::Result<Response<rpc::ExternalStatus>, tonic::Status> {
            if request.into_inner().session_token != "tok-ext-1" {
                return Err(tonic::Status::failed_precondition("session mismatch"));
            }
            self.state.lock().unwrap().heartbeat_count += 1;
            Ok(Response::new(external_snapshot(&self.state, false)))
        }

        async fn exit(
            &self,
            request: Request<rpc::ExternalSession>,
        ) -> std::result::Result<Response<rpc::ExternalStatus>, tonic::Status> {
            if request.into_inner().session_token != "tok-ext-1" {
                return Err(tonic::Status::failed_precondition("session mismatch"));
            }
            {
                let mut guard = self.state.lock().unwrap();
                guard.selected = false;
                guard.active = false;
                guard.motor_output_enabled = false;
                guard.reason = "builtin".into();
            }
            Ok(Response::new(external_snapshot(&self.state, false)))
        }

        async fn get_status(
            &self,
            _: Request<()>,
        ) -> std::result::Result<Response<rpc::ExternalStatus>, tonic::Status> {
            Ok(Response::new(external_snapshot(&self.state, false)))
        }

        async fn control(
            &self,
            request: Request<tonic::Streaming<rpc::ExternalFrame>>,
        ) -> std::result::Result<Response<Self::ControlStream>, tonic::Status> {
            use tokio_stream::StreamExt as _;
            let mut inbound = request.into_inner();
            let handshake = inbound
                .next()
                .await
                .ok_or_else(|| tonic::Status::invalid_argument("missing handshake"))??;
            if handshake.session_token != "tok-ext-1" || !handshake.targets.is_empty() {
                return Err(tonic::Status::invalid_argument("invalid handshake"));
            }
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            tx.send(Ok(external_snapshot(&self.state, false)))
                .await
                .unwrap();
            let state = self.state.clone();
            tokio::spawn(async move {
                while let Some(item) = inbound.next().await {
                    match item {
                        Ok(frame) => {
                            if frame.targets.len() != 16 || frame.input_window != 7 {
                                let _ = tx
                                    .send(Err(tonic::Status::invalid_argument("invalid frame")))
                                    .await;
                                break;
                            }
                            {
                                let mut guard = state.lock().unwrap();
                                guard.last_sequence = frame.sequence;
                            }
                            if tx.send(Ok(external_snapshot(&state, false))).await.is_err() {
                                break;
                            }
                        }
                        Err(status) => {
                            let _ = tx.send(Err(status)).await;
                            break;
                        }
                    }
                }
            });
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
                rx,
            )))
        }
    }

    #[tokio::test]
    async fn external_session_lifecycle_and_windowed_frames_work_over_grpc() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let service = std::sync::Arc::new(MockRobotExternalControl {
            state: std::sync::Arc::new(std::sync::Mutex::new(ExternalMockState {
                selected: false,
                active: false,
                motor_output_enabled: false,
                last_sequence: 0,
                heartbeat_count: 0,
                reason: "builtin".into(),
            })),
        });
        let (shutdown, rx_shutdown) = tokio::sync::oneshot::channel();
        let server = tonic::transport::Server::builder()
            .add_service(
                rpc::robot_external_control_server::RobotExternalControlServer::from_arc(
                    service.clone(),
                ),
            )
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async {
                    let _ = rx_shutdown.await;
                },
            );
        let serving = tokio::spawn(server);

        let client = MotionBoardClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();
        let ext = client.external();

        let entered = ext.enter("cust-ext", "req-ext-1").await.unwrap();
        assert!(entered.selected);
        assert!(entered.active);
        let token = entered.session_token;
        assert_eq!(token, "tok-ext-1");

        let (frame_tx, frame_rx) = tokio::sync::mpsc::channel(4);
        frame_tx
            .send(rpc::ExternalFrame {
                session_token: token.clone(),
                sequence: 0,
                targets: Vec::new(),
                input_window: 0,
            })
            .await
            .unwrap();
        let mut status_stream = ext
            .control(tokio_stream::wrappers::ReceiverStream::new(frame_rx))
            .await
            .unwrap();
        let win_status = status_stream.message().await.unwrap().unwrap();
        assert_eq!(win_status.input_window, 7);

        let targets = vec![
            rpc::ExternalJointCommand {
                q: 0.1,
                dq: 0.2,
                kp: 40.0,
                kd: 2.0,
                tau: 0.5,
            };
            16
        ];
        frame_tx
            .send(rpc::ExternalFrame {
                session_token: token.clone(),
                sequence: 1,
                targets,
                input_window: win_status.input_window,
            })
            .await
            .unwrap();
        let ack = status_stream.message().await.unwrap().unwrap();
        assert_eq!(ack.last_sequence, 1);

        let enabled = ext.enable(&token).await.unwrap();
        assert!(enabled.motor_output_enabled);
        let hb = ext.heartbeat(&token).await.unwrap();
        assert_eq!(hb.reason, "external_running");
        assert_eq!(service.state.lock().unwrap().heartbeat_count, 1);

        let disabled = ext.disable(&token).await.unwrap();
        assert!(!disabled.motor_output_enabled);
        assert!(disabled.motor_disable_confirmed);

        drop(frame_tx);
        let exited = ext.exit(&token).await.unwrap();
        assert!(!exited.selected);

        shutdown.send(()).unwrap();
        serving.await.unwrap().unwrap();
    }
}
