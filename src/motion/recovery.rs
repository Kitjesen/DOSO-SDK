use crate::{common::text, rpc, DosoClient, Error, Result};

use super::Motion;

impl Motion<'_> {
    /// Read onboard fall-recovery availability, phase, and session state (read-only).
    pub async fn recovery_status(&self) -> Result<rpc::RecoveryStatus> {
        DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .recovery_status(self.0.request(rpc::Empty {}))
                .await,
        )
    }

    /// Select fall-recovery mode without enabling motors. Caller must send `recovery_heartbeat` every 100 ms.
    pub async fn recovery_enter(
        &self,
        operation_id: &str,
        request_id: Option<&str>,
    ) -> Result<rpc::RecoveryStatus> {
        text(operation_id, "operation_id")?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        rpc::motion_client::MotionClient::new(self.0.channel.clone())
            .recovery_enter(self.0.request(rpc::RecoveryStartRequest {
                command: Some(command),
                operation_id: operation_id.into(),
                session_token: String::new(),
            }))
            .await
            .map(tonic::Response::into_inner)
            .map_err(|e| Error::rpc(e, Some(&id)))
    }

    /// Enable motors under the active fall-recovery session token.
    pub async fn recovery_enable(
        &self,
        session_token: &str,
        request_id: Option<&str>,
    ) -> Result<rpc::RecoveryStatus> {
        text(session_token, "session_token")?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        rpc::motion_client::MotionClient::new(self.0.channel.clone())
            .recovery_enable(self.0.request(rpc::RecoverySessionRequest {
                command: Some(command),
                session_token: session_token.into(),
            }))
            .await
            .map(tonic::Response::into_inner)
            .map_err(|e| Error::rpc(e, Some(&id)))
    }

    /// Renew fall-recovery authority lease (send every 100 ms; expires after 300 ms).
    pub async fn recovery_heartbeat(
        &self,
        session_token: &str,
        request_id: Option<&str>,
    ) -> Result<rpc::RecoveryStatus> {
        text(session_token, "session_token")?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        rpc::motion_client::MotionClient::new(self.0.channel.clone())
            .recovery_heartbeat(self.0.request(rpc::RecoverySessionRequest {
                command: Some(command),
                session_token: session_token.into(),
            }))
            .await
            .map(tonic::Response::into_inner)
            .map_err(|e| Error::rpc(e, Some(&id)))
    }

    /// Start a fall-recovery maneuver within an enabled recovery session.
    pub async fn recovery_start(
        &self,
        operation_id: &str,
        session_token: &str,
        request_id: Option<&str>,
    ) -> Result<rpc::RecoveryActionReply> {
        text(operation_id, "operation_id")?;
        text(session_token, "session_token")?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        rpc::motion_client::MotionClient::new(self.0.channel.clone())
            .recovery_start(self.0.request(rpc::RecoveryStartRequest {
                command: Some(command),
                operation_id: operation_id.into(),
                session_token: session_token.into(),
            }))
            .await
            .map(tonic::Response::into_inner)
            .map_err(|e| Error::rpc(e, Some(&id)))
    }

    /// Cancel an active fall-recovery maneuver and disable motors.
    pub async fn recovery_cancel(
        &self,
        operation_id: &str,
        session_token: &str,
        request_id: Option<&str>,
    ) -> Result<rpc::RecoveryActionReply> {
        text(operation_id, "operation_id")?;
        text(session_token, "session_token")?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        rpc::motion_client::MotionClient::new(self.0.channel.clone())
            .recovery_cancel(self.0.request(rpc::RecoveryCancelRequest {
                command: Some(command),
                operation_id: operation_id.into(),
                session_token: session_token.into(),
            }))
            .await
            .map(tonic::Response::into_inner)
            .map_err(|e| Error::rpc(e, Some(&id)))
    }

    /// Exit fall-recovery mode after confirmed motor disable, restoring normal locomotion gains.
    pub async fn recovery_exit(
        &self,
        session_token: &str,
        request_id: Option<&str>,
    ) -> Result<rpc::RecoveryStatus> {
        text(session_token, "session_token")?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        rpc::motion_client::MotionClient::new(self.0.channel.clone())
            .recovery_exit(self.0.request(rpc::RecoverySessionRequest {
                command: Some(command),
                session_token: session_token.into(),
            }))
            .await
            .map(tonic::Response::into_inner)
            .map_err(|e| Error::rpc(e, Some(&id)))
    }
}
