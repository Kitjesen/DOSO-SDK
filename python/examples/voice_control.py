#!/usr/bin/env python3
"""Example: Inspect and manage the robot's onboard voice & dialogue runtime via DOSO SDK.

Works over DOSO's unified :5052 endpoint (`doso.v1.Voice` -> onboard AskMe):
  - Default runs strictly READ-ONLY (inspects capabilities, voice status, and profiles).
  - Pass `--speak "..."` to trigger verbatim TTS speech (never invokes an LLM).
  - Pass `--chat "..."` to send a conversational turn to AskMe and print the reply.
  - Pass `--switch-profile <id>`, `--update-prompt "..."`, or `--switch-component` to hot-configure.
"""

from __future__ import annotations

import argparse

from doso_sdk import RobotClient


def main() -> int:
    parser = argparse.ArgumentParser(description="Manage robot voice & dialogue via DOSO SDK.")
    parser.add_argument("--endpoint", default="http://192.168.66.64:5052")
    parser.add_argument("--api-key", default=None)
    parser.add_argument("--speak", default=None, help="Play verbatim literal text on the speaker.")
    parser.add_argument("--priority", default="normal", choices=("normal", "safety"))
    parser.add_argument("--switch-profile", default=None, help="Switch active TTS voice profile id.")
    parser.add_argument("--update-prompt", default=None, help="Hot-update assistant system prompt.")
    parser.add_argument("--chat", default=None, help="Send a text turn to the dialogue engine.")
    parser.add_argument("--thread-id", default=None, help="Optional conversation thread id.")
    parser.add_argument("--speak-reply", action="store_true", help="Speak the chat reply aloud.")
    args = parser.parse_args()

    with RobotClient.connect(args.endpoint, api_key=args.api_key) as robot:
        caps = robot.capabilities()
        print(
            f"Connected: robot_id={robot.robot_id} fw={robot.firmware_version} "
            f"voice={caps.voice} motion={caps.motion}"
        )
        if not caps.voice:
            print("Voice runtime (AskMe) is not enabled on this gateway (capabilities.voice=False).")
            return 0

        status = robot.voice().status()
        profiles = robot.voice().list_profiles()
        print(
            f"Voice status={status.status} service={status.service} "
            f"active_profile={status.active_profile} llm={status.llm_provider}/{status.llm_model} "
            f"asr={status.asr_provider} tts={status.tts_provider}"
        )
        print(
            f"Available profiles ({len(profiles.profiles)}): "
            f"{[p.profile_id for p in profiles.profiles]}"
        )

        if args.switch_profile:
            robot.voice().switch_profile(args.switch_profile)
            print(f"Switched voice profile -> {args.switch_profile}")

        if args.update_prompt:
            robot.voice().update_prompt(args.update_prompt)
            print("Updated assistant system prompt.")

        if args.speak:
            pb_id = robot.voice().speak(args.speak, priority=args.priority)
            pb = robot.voice().playback_status(pb_id)
            print(f"Queued speech: playback_id={pb.playback_id} state={pb.state}")

        if args.chat:
            reply = robot.voice().chat(
                args.chat,
                thread_id=args.thread_id,
                speak_reply=args.speak_reply,
            )
            print(
                f"Chat reply (thread={reply.thread_id}, {reply.latency_ms:.1f}ms): "
                f"{reply.reply_text}"
            )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
