#!/usr/bin/env python3
"""Example: Replace DOSO's built-in walking policy with your own Python RL model.

Works identically on:
  - Standalone Board-A motion chassis (no Board B / no LingTu): connect to http://<robot>:5052
  - Full Board-A + Board-B robot: connect to http://<robot>:5052 (SDK automatically
    redirects low_level() straight to Board A :5053 to bypass Board B Python)

SAFETY:
  - Default runs in `--dry-run` mode (reads live IMU/joints without enabling motors).
  - Pass `--enable-motors` ONLY after placing the robot in a safe area with a physical
    YUNZHUO controller ready to preempt at any moment.
"""

from __future__ import annotations

import argparse
import math
import time
from typing import List

from doso_sdk import ImuSnapshot, JointCmd, JointsSnapshot, RobotClient


def custom_policy_step(imu: ImuSnapshot, joints: JointsSnapshot) -> List[JointCmd]:
    """Replace this function with your PyTorch / ONNX / JAX inference step.

    Canonical 16-joint order:
      0..2   FR hip, thigh, calf
      3..5   FL hip, thigh, calf
      6..8   RR hip, thigh, calf
      9..11  RL hip, thigh, calf
      12..15 FR, FL, RR, RL wheels
    """
    _ = imu  # Use imu.gyro_rad_s, imu.accel_m_s2, imu.quat_wxyz in your policy obs
    # Safe demonstration command: hold current leg positions softly with zero wheel velocity.
    cmds: List[JointCmd] = [
        JointCmd.leg_pd(q=joints.q[i], kp=20.0, kd=2.0) for i in range(12)
    ]
    cmds.extend(JointCmd.wheel_vel(dq=0.0, kd=1.5) for _ in range(4))
    return cmds


def main() -> int:
    parser = argparse.ArgumentParser(description="Run a custom Python RL policy via DOSO SDK.")
    parser.add_argument("--endpoint", default="http://192.168.66.64:5052")
    parser.add_argument("--api-key", default=None)
    parser.add_argument("--hz", type=float, default=50.0)
    parser.add_argument("--duration", type=float, default=5.0)
    parser.add_argument(
        "--enable-motors",
        action="store_true",
        help="Actually enter External mode and enable actuators (default: telemetry dry-run).",
    )
    args = parser.parse_args()

    period = 1.0 / max(args.hz, 1.0)
    with RobotClient.connect(args.endpoint, api_key=args.api_key) as robot:
        caps = robot.capabilities()
        print(
            f"Connected: robot_id={robot.robot_id} fw={robot.firmware_version} "
            f"low_level={caps.low_level} board_b_autonomy={caps.board_b_autonomy}"
        )
        if not caps.low_level:
            raise RuntimeError("Robot does not advertise low_level (External) capability.")

        with robot.observations().telemetry(imu=True, joints=True, stale_after_ms=200) as obs:
            deadline = time.monotonic() + 3.0
            while time.monotonic() < deadline:
                imu = obs.latest_imu()
                joints = obs.latest_joints()
                if imu.available and joints.complete:
                    break
                time.sleep(0.02)
            else:
                raise RuntimeError("Timed out waiting for initial IMU + 16-joint telemetry.")

            if not args.enable_motors:
                imu = obs.latest_imu()
                joints = obs.latest_joints()
                print(
                    f"[DRY-RUN] IMU accel={imu.accel_m_s2} gyro={imu.gyro_rad_s} "
                    f"q[0..4]={tuple(round(v, 3) for v in joints.q[:4])}"
                )
                return 0

            with robot.low_level() as ll:
                # Prime initial frame before enabling hardware (required by Brainstem SafetyGate)
                ll.publish_joints(custom_policy_step(obs.latest_imu(), obs.latest_joints()))
                ll.enable()
                end_time = time.monotonic() + args.duration
                try:
                    while time.monotonic() < end_time:
                        t0 = time.monotonic()
                        imu = obs.latest_imu()
                        joints = obs.latest_joints()
                        if not (imu.fresh and joints.fresh and joints.complete):
                            print("Telemetry stale — stopping external policy safely.")
                            break
                        cmds = custom_policy_step(imu, joints)
                        if any(not math.isfinite(c.q) for c in cmds):
                            break
                        ll.publish_joints(cmds)
                        snap = ll.state()
                        if not snap.active:
                            print("External authority revoked (e.g. YUNZHUO preemption).")
                            break
                        sleep_s = period - (time.monotonic() - t0)
                        if sleep_s > 0:
                            time.sleep(sleep_s)
                finally:
                    ll.disable()
                    ll.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
