use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;

use crate::{rpc, DosoClient, Error, ErrorKind, Result};

use super::Motion;

struct Connection {
    sender: mpsc::Sender<rpc::TeleopRequest>,
    reader: JoinHandle<()>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

#[derive(Clone, Default)]
struct State {
    window: u64,
    reply: Option<rpc::TeleopReply>,
    control: Option<rpc::ControlState>,
    control_received_at: Option<tokio::time::Instant>,
    error: Option<tonic::Status>,
}

/// One operator connection. Only explicit new input is sent; no velocity is replayed.
/// Dropping it cancels the stream and asks the robot service to hold/release.
pub struct Teleop<'a> {
    client: &'a DosoClient,
    connection: Option<Connection>,
    state: watch::Receiver<State>,
    sequence: u64,
}

impl<'a> Motion<'a> {
    /// Open the operator session and explicitly claim native authority without moving.
    pub async fn claim_control(&self, request_id: Option<&str>) -> Result<Teleop<'a>> {
        let mut session = self.teleop().await?;
        let reply = session.claim(request_id).await?;
        if !reply.accepted {
            return Err(Error::rpc(
                tonic::Status::failed_precondition(reply.reason),
                Some(&reply.request_id),
            ));
        }
        Ok(session)
    }

    /// Alias of [`Motion::claim_control`] matching Zsibot ownership semantics (`TakeControl`).
    pub async fn take_control(&self, request_id: Option<&str>) -> Result<Teleop<'a>> {
        self.claim_control(request_id).await
    }

    /// Open a continuous input stream. Native authority is claimed on the first velocity.
    pub async fn teleop(&self) -> Result<Teleop<'a>> {
        let command = self.0.command(None)?;
        let id = command.request_id.clone();
        let (sender, receiver) = mpsc::channel(1);
        sender
            .send(rpc::TeleopRequest {
                command: Some(command),
                sequence: 1,
                action: Some(rpc::teleop_request::Action::Input(rpc::Empty {})),
                ..Default::default()
            })
            .await
            .map_err(|_| Error::new(ErrorKind::Transport, "teleop request stream closed"))?;
        let mut replies = tokio::time::timeout(self.0.timeout, async {
            DosoClient::read(
                rpc::motion_client::MotionClient::new(self.0.channel.clone())
                    .teleop(self.0.stream_request(ReceiverStream::new(receiver)))
                    .await,
            )
        })
        .await
        .map_err(|_| teleop_timeout(&id))??;
        let reply = tokio::time::timeout(self.0.timeout, replies.message())
            .await
            .map_err(|_| teleop_timeout(&id))?
            .map_err(|status| Error::rpc(status, None))?
            .ok_or_else(|| Error::protocol("teleop stream ended before opening"))?;
        if reply.request_id != id
            || reply.sequence != 1
            || !reply.accepted
            || reply.input_window == 0
        {
            return Err(Error::protocol(
                "robot does not support continuous teleop input",
            ));
        }
        let (updates, state) = watch::channel(State {
            window: reply.input_window,
            control: reply.control,
            control_received_at: Some(tokio::time::Instant::now()),
            ..Default::default()
        });
        let reader = tokio::spawn(async move {
            loop {
                let reply = match replies.message().await {
                    Ok(Some(reply)) => reply,
                    result => {
                        let error = result
                            .err()
                            .unwrap_or_else(|| tonic::Status::unavailable("teleop stream ended"));
                        updates.send_modify(|state| state.error = Some(error));
                        return;
                    }
                };
                if reply.sequence == 0
                    && reply.request_id.is_empty()
                    && reply.accepted
                    && reply.input_window != 0
                {
                    updates.send_modify(|state| {
                        state.window = reply.input_window;
                        state.control = reply.control;
                        state.control_received_at = Some(tokio::time::Instant::now());
                    });
                } else if reply.sequence != 0 && !reply.request_id.is_empty() {
                    updates.send_modify(|state| {
                        if reply.input_window != 0 {
                            state.window = reply.input_window;
                        }
                        state.control = reply.control.clone();
                        state.control_received_at = Some(tokio::time::Instant::now());
                        state.reply = Some(reply);
                    });
                } else {
                    updates.send_modify(|state| {
                        state.error =
                            Some(tonic::Status::data_loss("invalid teleop reply identity"));
                    });
                    return;
                }
            }
        });
        Ok(Teleop {
            client: self.0,
            connection: Some(Connection { sender, reader }),
            state,
            sequence: 1,
        })
    }
}

impl Teleop<'_> {
    /// Explicitly claim or re-claim the session's native lease. Does not send velocity.
    pub async fn claim(&mut self, request_id: Option<&str>) -> Result<rpc::TeleopReply> {
        self.control(
            rpc::teleop_request::Action::Claim(rpc::Empty {}),
            request_id,
        )
        .await
    }

    /// Latest authority snapshot for this session. Intermediate snapshots may be replaced.
    pub fn control_state(&self) -> Result<Option<rpc::ControlState>> {
        let state = self.state.borrow();
        if let Some(error) = &state.error {
            return Err(Error::rpc(error.clone(), None));
        }
        let mut control = state.control.clone();
        if let (Some(control), Some(received_at)) = (&mut control, state.control_received_at) {
            let elapsed_ms = received_at.elapsed().as_secs_f64() * 1000.0;
            control.received_age_ms = control.received_age_ms.map(|age| age + elapsed_ms);
            if elapsed_ms > 250.0 || control.received_age_ms.is_some_and(|age| age > 250.0) {
                control.fresh = false;
                control.available = None;
                control.held_by_client = None;
                control.holding = None;
            }
        }
        Ok(control)
    }

    /// Queue fresh velocity without waiting for a network ACK; suitable for 50 Hz input.
    /// Returns its request ID, not robot admission. A full queue returns ResourceExhausted.
    pub fn publish_velocity(
        &mut self,
        velocity: rpc::TeleopVelocity,
        request_id: Option<&str>,
    ) -> Result<String> {
        if ![velocity.vx_m_s, velocity.vy_m_s, velocity.yaw_rad_s]
            .iter()
            .all(|value| value.is_finite())
        {
            return Err(Error::invalid("teleop velocity must be finite"));
        }
        let command = self.client.command(request_id)?;
        let id = command.request_id.clone();
        let state = self.state.borrow();
        if let Some(error) = &state.error {
            return Err(Error::rpc(error.clone(), None));
        }
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| Error::invalid("teleop session is closed"))?;
        self.sequence += 1;
        connection
            .sender
            .try_send(rpc::TeleopRequest {
                command: Some(command),
                input_window: state.window,
                sequence: self.sequence,
                action: Some(rpc::teleop_request::Action::Velocity(velocity)),
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => Error::new(
                    ErrorKind::Rpc(tonic::Code::ResourceExhausted),
                    "teleop send queue is full; submit the next fresh input",
                ),
                mpsc::error::TrySendError::Closed(_) => {
                    Error::new(ErrorKind::Transport, "teleop request stream closed")
                }
            })?;
        Ok(id)
    }

    /// Latest robot admission/hold reply; older replies may be replaced.
    pub fn latest_reply(&self) -> Result<Option<rpc::TeleopReply>> {
        let state = self.state.borrow();
        if let Some(error) = &state.error {
            return Err(Error::rpc(error.clone(), None));
        }
        Ok(state.reply.clone())
    }

    /// Send a velocity and wait for its admission ACK. For continuous input use publish_velocity.
    pub async fn send_velocity(
        &mut self,
        velocity: rpc::TeleopVelocity,
        request_id: Option<&str>,
    ) -> Result<rpc::TeleopReply> {
        let id = self.publish_velocity(velocity, request_id)?;
        let connection = self
            .connection
            .take()
            .ok_or_else(|| Error::invalid("teleop session is closed"))?;
        let reply = tokio::time::timeout(self.client.timeout, self.wait_reply(&id, self.sequence))
            .await
            .map_err(|_| teleop_timeout(&id))??;
        self.connection = Some(connection);
        Ok(reply)
    }

    /// Hold motion and wait for native final-output confirmation in the reply.
    pub async fn hold(&mut self, request_id: Option<&str>) -> Result<rpc::TeleopReply> {
        self.control(rpc::teleop_request::Action::Hold(rpc::Empty {}), request_id)
            .await
    }

    /// Hold and release the connection, returning the robot's acknowledgement.
    pub async fn close(self, request_id: Option<&str>) -> Result<rpc::TeleopReply> {
        self.release_control(request_id).await
    }

    /// Hold, release native authority, and consume the operator connection.
    pub async fn release_control(mut self, request_id: Option<&str>) -> Result<rpc::TeleopReply> {
        self.control(
            rpc::teleop_request::Action::Close(rpc::Empty {}),
            request_id,
        )
        .await
    }

    async fn control(
        &mut self,
        action: rpc::teleop_request::Action,
        request_id: Option<&str>,
    ) -> Result<rpc::TeleopReply> {
        let command = self.client.command(request_id)?;
        let id = command.request_id.clone();
        // Cancellation of this wait must close both sides of the stream.
        let connection = self
            .connection
            .take()
            .ok_or_else(|| Error::invalid("teleop session is closed"))?;
        self.sequence += 1;
        let sequence = self.sequence;
        let reply = tokio::time::timeout(self.client.timeout, async {
            connection
                .sender
                .send(rpc::TeleopRequest {
                    command: Some(command),
                    sequence,
                    input_window: 0,
                    action: Some(action),
                })
                .await
                .map_err(|_| {
                    Error::new(ErrorKind::Transport, "teleop request stream closed").request(&id)
                })?;
            self.wait_reply(&id, sequence).await
        })
        .await
        .map_err(|_| teleop_timeout(&id))??;
        self.connection = Some(connection);
        Ok(reply)
    }

    async fn wait_reply(&mut self, id: &str, sequence: u64) -> Result<rpc::TeleopReply> {
        loop {
            let state = self.state.borrow_and_update().clone();
            if let Some(reply) = state.reply {
                if reply.sequence >= sequence {
                    if reply.sequence != sequence || reply.request_id != id {
                        return Err(Error::protocol("teleop reply identity mismatch").request(id));
                    }
                    return Ok(reply);
                }
            }
            if let Some(error) = state.error {
                return Err(Error::rpc(error, Some(id)));
            }
            self.state
                .changed()
                .await
                .map_err(|_| Error::protocol("teleop reply stream ended").request(id))?;
        }
    }
}

fn teleop_timeout(request_id: &str) -> Error {
    Error::new(
        ErrorKind::Timeout,
        "teleop reply timed out; command acceptance is unknown",
    )
    .request(request_id)
}
