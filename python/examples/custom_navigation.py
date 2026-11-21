#!/usr/bin/env python3
"""Example: Replace LingTu navigation with your own Python planner / velocity controller.

Disconnects / bypasses LingTu navigation and drives Brainstem's built-in locomotion
directly through DOSO's unified `RobotClient` (:5052). Works identically on a
standalone Board-A chassis (no Board B installed) or full robot.

SAFETY:
  - Default runs in `--dry-run` mode (inspects capabilities, body status & telemetry only).
  - Pass `--run-motion` ONLY in an open test area with a physical YUNZHUO controller ready.
"""

from __future__ import annotations

import argparse
import time

from doso_sdk import RobotClient


def compute_planner_velocity(elapsed_s: float) -> tuple[float, float, float]:
    """Replace this with your custom navigation / ROS2 / trajectory tracker output."""
    if elapsed_s < 1.5:
        return (0.15, 0.0, 0.0)  # gentle forward velocity (m/s)
    return (0.0, 0.0, 0.0)


def main() -> int:
    parser = argparse.ArgumentParser(description="Drive DOSO built-in locomotion from a custom planner.")
    parser.add_argument("--endpoint", default="http://192.168.66.64:5052")
    parser.add_argument("--api-key", default=None)
    parser.add_argument("--duration", type=float, default=2.0)
    parser.add_argument(
        "--run-motion",
        action="store_true",
        help="Acquire control and send gentle velocity commands (default: read-only dry-run).",
    )
    args = parser.parse_args()

    with RobotClient.connect(args.endpoint, api_key=args.api_key) as robot:
        caps = robot.capabilities()
        status = robot.sport().control_status()
        print(
            f"Connected: robot_id={robot.robot_id} profile={robot.sport().current_profile()} "
            f"body_kind={status.body_kind} motors_enabled={status.motor_output_enabled} "
            f"board_b_autonomy={caps.board_b_autonomy}"
        )

        with robot.observations().telemetry(imu=True, joints=True) as obs:
            time.sleep(0.1)
            imu = obs.latest_imu()
            joints = obs.latest_joints()
            print(
                f"Telemetry: imu_available={imu.available} joints_complete={joints.complete} "
                f"voltages={robot.observations().motor_voltages()[:4]}"
            )
            if not args.run_motion:
                print("[DRY-RUN] Exiting without sending motion commands.")
                return 0

            with robot.sport().take_control() as teleop:
                start = time.monotonic()
                try:
                    while time.monotonic() - start < args.duration:
                        vx, vy, wz = compute_planner_velocity(time.monotonic() - start)
                        teleop.send_velocity(vx, vy, wz, deadman=True)
                        time.sleep(0.05)
                finally:
                    teleop.send_velocity(0.0, 0.0, 0.0, deadman=False)
                    teleop.release()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
