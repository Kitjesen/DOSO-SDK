use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;

use crate::{rpc, DosoClient, Error, ErrorKind, Result};

use super::Motion;

struct Connection {
    sender: mpsc::Sender<rpc::ExternalControlRequest>,
    reader: JoinHandle<()>,
    heartbeat: JoinHandle<()>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.heartbeat.abort();
        self.reader.abort();
    }
}

#[derive(Clone, Default)]
struct State {
    window: u64,
    controller: Option<rpc::ControllerState>,
    received_at: Option<tokio::time::Instant>,
    reply: Option<rpc::ExternalControlReply>,
    error: Option<tonic::Status>,
}

/// A robot-authorized external joint controller. Model execution belongs to the caller.
/// Drop closes the command stream. Explicit close also returns to builtin idle, with motors off.
pub struct ExternalControl<'a> {
    client: &'a DosoClient,
    connection: Option<Connection>,
    state: watch::Receiver<State>,
    sequence: u64,
}

impl<'a> Motion<'a> {
    pub async fn controller(&self) -> Result<rpc::ControllerState> {
        DosoClient::read(
            rpc::motion_client::MotionClient::new(self.0.channel.clone())
                .controller(self.0.request(rpc::Empty {}))
                .await,
        )
    }

    /// Enter external mode while grounded and disabled. Wait for a robot-issued input window.
    pub async fn external_control(&self, request_id: Option<&str>) -> Result<ExternalControl<'a>> {
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let heartbeat_command = rpc::Command {
            request_id: format!("{id}:heartbeat"),
            client_id: command.client_id.clone(),
        };
        let (sender, receiver) = mpsc::channel(1);
        sender
            .send(rpc::ExternalControlRequest {
                command: Some(command),
                sequence: 1,
                action: Some(rpc::external_control_request::Action::Enter(rpc::Empty {})),
                ..Default::default()
            })
            .await
            .map_err(|_| Error::protocol("external request stream closed"))?;
        let channel = self.0.low_level_channel().await?;
        let mut replies = tokio::time::timeout(self.0.timeout, async {
            rpc::motion_client::MotionClient::new(channel)
                .external_control(self.0.stream_request(ReceiverStream::new(receiver)))
                .await
                .map(|response| response.into_inner())
                .map_err(|status| Error::rpc(status, Some(&id)))
        })
        .await
        .map_err(|_| timeout(&id))??;
        let reply = tokio::time::timeout(self.0.timeout, replies.message())
            .await
            .map_err(|_| timeout(&id))?
            .map_err(|error| Error::rpc(error, Some(&id)))?
            .ok_or_else(|| Error::protocol("external stream ended before Enter").request(&id))?;
        if reply.request_id != id || reply.sequence != 1 {
            return Err(Error::protocol("external Enter identity mismatch").request(&id));
        }
        if !reply.accepted {
            return Err(Error::rpc(
                tonic::Status::failed_precondition(reply.reason),
                Some(&id),
            ));
        }
        let (updates, state) = watch::channel(State {
            window: reply.input_window,
            controller: reply.state,
            received_at: Some(tokio::time::Instant::now()),
            ..Default::default()
        });
        let reader = tokio::spawn(async move {
            loop {
                let reply = match replies.message().await {
                    Ok(Some(reply)) => reply,
                    result => {
                        let error = result
                            .err()
                            .unwrap_or_else(|| tonic::Status::unavailable("external stream ended"));
                        updates.send_modify(|state| state.error = Some(error));
                        return;
                    }
                };
                let update = reply.sequence == 0 && reply.request_id.is_empty();
                if !update && (reply.sequence == 0 || reply.request_id.is_empty()) {
                    updates.send_modify(|state| {
                        state.error =
                            Some(tonic::Status::data_loss("invalid external reply identity"))
                    });
                    return;
                }
                updates.send_modify(|state| {
                    state.window = reply.input_window;
                    state.controller = reply.state.clone();
                    state.received_at = Some(tokio::time::Instant::now());
                    if !update {
                        state.reply = Some(reply);
                    }
                });
            }
        });
        let heartbeat_sender = sender.clone();
        let heartbeat = tokio::spawn(async move {
            let period = std::time::Duration::from_millis(100);
            let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticks.tick().await;
                let request = rpc::ExternalControlRequest {
                    command: Some(heartbeat_command.clone()),
                    action: Some(rpc::external_control_request::Action::Heartbeat(
                        rpc::Empty {},
                    )),
                    ..Default::default()
                };
                if matches!(
                    heartbeat_sender.try_send(request),
                    Err(mpsc::error::TrySendError::Closed(_))
                ) {
                    return;
                }
            }
        });
        let mut session = ExternalControl {
            client: self.0,
            connection: Some(Connection {
                sender,
                reader,
                heartbeat,
            }),
            state,
            sequence: 1,
        };
        tokio::time::timeout(self.0.timeout, session.wait_window())
            .await
            .map_err(|_| timeout(&id))?
            .map_err(|error| error.request(&id))?;
        Ok(session)
    }
}

impl ExternalControl<'_> {
    /// Last robot observation. Network transit time is unknown.
    pub fn state(&self) -> Result<Option<rpc::ControllerState>> {
        let state = self.state.borrow();
        if let Some(error) = &state.error {
            return Err(Error::rpc(error.clone(), None));
        }
        let mut controller = state.controller.clone();
        if let (Some(controller), Some(received_at)) = (&mut controller, state.received_at) {
            let elapsed_ms = received_at.elapsed().as_secs_f64() * 1000.0;
            controller.received_age_ms = controller.received_age_ms.map(|age| age + elapsed_ms);
            if elapsed_ms > 250.0 || controller.received_age_ms.is_some_and(|age| age > 250.0) {
                controller.fresh = false;
                controller.active = false;
            }
        }
        Ok(controller)
    }

    /// Latest command acknowledgement. ControllerState.last_sequence reports robot validation.
    pub fn latest_reply(&self) -> Result<Option<rpc::ExternalControlReply>> {
        let state = self.state.borrow();
        if let Some(error) = &state.error {
            return Err(Error::rpc(error.clone(), None));
        }
        Ok(state.reply.clone())
    }

    /// Queue fresh joint targets without waiting for per-frame acknowledgement.
    pub fn publish_joints(
        &mut self,
        joints: Vec<rpc::JointCommand>,
        request_id: Option<&str>,
    ) -> Result<String> {
        let controller = self
            .state()?
            .ok_or_else(|| Error::protocol("external controller state missing"))?;
        if !controller.fresh || !controller.active || controller.joint_count == 0 {
            return Err(Error::new(
                ErrorKind::Rpc(tonic::Code::Unavailable),
                "external controller is not ready",
            ));
        }
        if joints.len() != controller.joint_count as usize
            || joints.iter().any(|joint| {
                ![joint.q, joint.dq, joint.kp, joint.kd, joint.tau]
                    .iter()
                    .all(|value| value.is_finite())
                    || joint.kp < 0.0
                    || joint.kd < 0.0
            })
        {
            return Err(Error::invalid("external targets must match the robot joint count with finite values and nonnegative gains"));
        }
        let command = self.client.command(request_id)?;
        let id = command.request_id.clone();
        let window = self.state.borrow().window;
        if window == 0 {
            return Err(Error::new(
                ErrorKind::Rpc(tonic::Code::Unavailable),
                "external input window is not ready",
            ));
        }
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| Error::invalid("external session is closed"))?;
        self.sequence += 1;
        connection
            .sender
            .try_send(rpc::ExternalControlRequest {
                command: Some(command),
                sequence: self.sequence,
                input_window: window,
                action: Some(rpc::external_control_request::Action::Frame(
                    rpc::JointTargets { joints },
                )),
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => Error::new(
                    ErrorKind::Rpc(tonic::Code::ResourceExhausted),
                    "external send queue is full; submit the next fresh targets",
                ),
                mpsc::error::TrySendError::Closed(_) => {
                    Error::new(ErrorKind::Transport, "external request stream closed")
                }
            })?;
        Ok(id)
    }

    pub async fn enable(&mut self, request_id: Option<&str>) -> Result<rpc::ExternalControlReply> {
        self.command(
            rpc::external_control_request::Action::Enable(rpc::Empty {}),
            request_id,
        )
        .await
    }

    pub async fn disable(&mut self, request_id: Option<&str>) -> Result<rpc::ExternalControlReply> {
        self.command(
            rpc::external_control_request::Action::Disable(rpc::Empty {}),
            request_id,
        )
        .await
    }

    /// Disable external output and explicitly restore builtin idle, with motors closed.
    pub async fn close(mut self, request_id: Option<&str>) -> Result<rpc::ExternalControlReply> {
        self.command(
            rpc::external_control_request::Action::Close(rpc::Empty {}),
            request_id,
        )
        .await
    }

    async fn wait_window(&mut self) -> Result<()> {
        loop {
            let state = self.state.borrow_and_update().clone();
            if let Some(error) = state.error {
                return Err(Error::rpc(error, None));
            }
            if state.window != 0
                && state
                    .controller
                    .as_ref()
                    .is_some_and(|value| value.fresh && value.active && value.joint_count > 0)
            {
                return Ok(());
            }
            self.state
                .changed()
                .await
                .map_err(|_| Error::protocol("external stream ended"))?;
        }
    }

    async fn command(
        &mut self,
        action: rpc::external_control_request::Action,
        request_id: Option<&str>,
    ) -> Result<rpc::ExternalControlReply> {
        let command = self.client.command(request_id)?;
        let id = command.request_id.clone();
        let connection = self
            .connection
            .take()
            .ok_or_else(|| Error::invalid("external session is closed"))?;
        self.sequence += 1;
        let sequence = self.sequence;
        let reply = tokio::time::timeout(self.client.timeout, async {
            connection
                .sender
                .send(rpc::ExternalControlRequest {
                    command: Some(command),
                    sequence,
                    input_window: 0,
                    action: Some(action),
                })
                .await
                .map_err(|_| {
                    Error::new(ErrorKind::Transport, "external request stream closed").request(&id)
                })?;
            loop {
                let state = self.state.borrow_and_update().clone();
                if let Some(reply) = state.reply {
                    if reply.sequence >= sequence {
                        if reply.sequence != sequence || reply.request_id != id {
                            return Err(
                                Error::protocol("external reply identity mismatch").request(&id)
                            );
                        }
                        return Ok(reply);
                    }
                }
                if let Some(error) = state.error {
                    return Err(Error::rpc(error, Some(&id)));
                }
                self.state
                    .changed()
                    .await
                    .map_err(|_| Error::protocol("external reply stream ended").request(&id))?;
            }
        })
        .await
        .map_err(|_| timeout(&id))??;
        self.connection = Some(connection);
        Ok(reply)
    }
}

fn timeout(id: &str) -> Error {
    Error::new(
        ErrorKind::Timeout,
        "external reply timed out; command acceptance is unknown",
    )
    .request(id)
}

impl rpc::JointCommand {
    /// Position-PD leg command (`dq = 0`, `tau = 0`).
    pub fn leg_pd(q: f64, kp: f64, kd: f64) -> Self {
        Self {
            q,
            dq: 0.0,
            kp,
            kd,
            tau: 0.0,
        }
    }

    /// Wheel-foot velocity command (`q = 0`, `kp = 0`, `tau = 0`, matching Unitree Go2-W RL).
    pub fn wheel_vel(dq: f64, kd: f64) -> Self {
        Self {
            q: 0.0,
            dq,
            kp: 0.0,
            kd,
            tau: 0.0,
        }
    }

    /// Analytic damping command (`q = 0`, `dq = 0`, `kp = 0`, `tau = 0`).
    pub fn damping(kd: f64) -> Self {
        Self {
            q: 0.0,
            dq: 0.0,
            kp: 0.0,
            kd,
            tau: 0.0,
        }
    }
}
