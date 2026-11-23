use crate::{common::text, rpc, CommandReceipt, DosoClient, Result};

/// Options for verbatim robot speech playback (`doso.v1.Voice/Speak`).
#[derive(Debug, Clone)]
pub struct VoiceSpeakOptions {
    /// `"normal"` (default) or `"safety"` (override priority).
    pub priority: String,
    /// `"enqueue"` (default) or `"replace_noncritical"`.
    pub queue_policy: String,
    /// Optional voice profile id override; empty uses the active profile.
    pub voice_profile_id: String,
    pub speed: Option<f64>,
    pub pitch: Option<f64>,
    pub volume: Option<f64>,
}

impl Default for VoiceSpeakOptions {
    fn default() -> Self {
        Self {
            priority: "normal".to_owned(),
            queue_policy: "enqueue".to_owned(),
            voice_profile_id: String::new(),
            speed: None,
            pitch: None,
            volume: None,
        }
    }
}

/// Unified voice runtime & dialogue management client (`doso.v1.Voice`).
///
/// Proxies cleanly to the robot's onboard voice assistant (e.g. AskMe) when
/// installed; on standalone motion chassis without a voice runtime, calls
/// cleanly return `Code::Unimplemented` and `capabilities().voice == false`.
pub struct Voice<'a>(pub(crate) &'a DosoClient);

impl Voice<'_> {
    /// Read the active voice runtime state (status, LLM/ASR/TTS backends, active profile).
    pub async fn status(&self) -> Result<rpc::VoiceSystemStatus> {
        DosoClient::read(
            rpc::voice_client::VoiceClient::new(self.0.channel.clone())
                .status(self.0.request(rpc::Empty {}))
                .await,
        )
    }

    /// List available TTS voice profiles and the currently selected profile.
    pub async fn list_profiles(&self) -> Result<rpc::VoiceProfileList> {
        DosoClient::read(
            rpc::voice_client::VoiceClient::new(self.0.channel.clone())
                .list_profiles(self.0.request(rpc::Empty {}))
                .await,
        )
    }

    /// Switch the active TTS voice profile by `profile_id`.
    pub async fn switch_profile(
        &self,
        profile_id: &str,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        text(profile_id, "profile_id")?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        DosoClient::receipt(
            rpc::voice_client::VoiceClient::new(self.0.channel.clone())
                .switch_profile(self.0.request(rpc::VoiceSwitchProfileRequest {
                    command: Some(command),
                    profile_id: profile_id.to_owned(),
                }))
                .await,
            &id,
        )
    }

    /// Play literal verbatim text on the robot speaker (never invokes an LLM).
    pub async fn speak(
        &self,
        literal_text: &str,
        options: VoiceSpeakOptions,
        request_id: Option<&str>,
    ) -> Result<rpc::VoicePlayback> {
        text(literal_text, "text")?;
        let command = self.0.command(request_id)?;
        let priority = if options.priority.trim().is_empty() {
            "normal".to_owned()
        } else {
            options.priority
        };
        let queue_policy = if options.queue_policy.trim().is_empty() {
            "enqueue".to_owned()
        } else {
            options.queue_policy
        };
        DosoClient::read(
            rpc::voice_client::VoiceClient::new(self.0.channel.clone())
                .speak(self.0.request(rpc::VoiceSpeakRequest {
                    command: Some(command),
                    text: literal_text.to_owned(),
                    priority,
                    queue_policy,
                    voice_profile_id: options.voice_profile_id,
                    speed: options.speed,
                    pitch: options.pitch,
                    volume: options.volume,
                }))
                .await,
        )
    }

    /// Query the lifecycle state of a previously created playback job.
    pub async fn playback_status(&self, playback_id: &str) -> Result<rpc::VoicePlayback> {
        text(playback_id, "playback_id")?;
        DosoClient::read(
            rpc::voice_client::VoiceClient::new(self.0.channel.clone())
                .playback_status(self.0.request(rpc::VoicePlaybackQuery {
                    playback_id: playback_id.to_owned(),
                }))
                .await,
        )
    }

    /// Interrupt / cancel a queued or currently playing speech job.
    pub async fn cancel_playback(
        &self,
        playback_id: &str,
        reason: &str,
        request_id: Option<&str>,
    ) -> Result<rpc::VoicePlayback> {
        text(playback_id, "playback_id")?;
        let command = self.0.command(request_id)?;
        let reason = if reason.trim().is_empty() {
            "operator_cancelled".to_owned()
        } else {
            reason.to_owned()
        };
        DosoClient::read(
            rpc::voice_client::VoiceClient::new(self.0.channel.clone())
                .cancel_playback(self.0.request(rpc::VoicePlaybackCancelRequest {
                    command: Some(command),
                    playback_id: playback_id.to_owned(),
                    reason,
                }))
                .await,
        )
    }

    /// Hot-switch a voice runtime component (`llm`, `tts`, `asr`, `realtime`).
    pub async fn switch_component(
        &self,
        component: &str,
        provider: &str,
        model: Option<&str>,
        validate: bool,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        text(component, "component")?;
        text(provider, "provider")?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        DosoClient::receipt(
            rpc::voice_client::VoiceClient::new(self.0.channel.clone())
                .switch_component(self.0.request(rpc::VoiceSwitchComponentRequest {
                    command: Some(command),
                    component: component.to_owned(),
                    provider: provider.to_owned(),
                    model: model.unwrap_or("").to_owned(),
                    validate,
                }))
                .await,
            &id,
        )
    }

    /// Hot-update the robot assistant's system prompt / persona instructions.
    pub async fn update_prompt(
        &self,
        system_prompt: &str,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        text(system_prompt, "system_prompt")?;
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        DosoClient::receipt(
            rpc::voice_client::VoiceClient::new(self.0.channel.clone())
                .update_prompt(self.0.request(rpc::VoiceUpdatePromptRequest {
                    command: Some(command),
                    system_prompt: system_prompt.to_owned(),
                }))
                .await,
            &id,
        )
    }

    /// Send a conversational turn to the robot's dialogue engine.
    pub async fn chat(
        &self,
        message: &str,
        thread_id: Option<&str>,
        speak_reply: bool,
        request_id: Option<&str>,
    ) -> Result<rpc::VoiceChatReply> {
        text(message, "text")?;
        let command = self.0.command(request_id)?;
        DosoClient::read(
            rpc::voice_client::VoiceClient::new(self.0.channel.clone())
                .chat(self.0.request(rpc::VoiceChatRequest {
                    command: Some(command),
                    text: message.to_owned(),
                    thread_id: thread_id.unwrap_or("").to_owned(),
                    speak_reply,
                }))
                .await,
        )
    }
}
