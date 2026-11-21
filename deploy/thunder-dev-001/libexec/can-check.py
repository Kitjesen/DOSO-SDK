#!/usr/bin/env python3
"""Prepare the existing four-PCAN mapping; never send motor/CAN frames."""

import argparse
import glob
import json
import os
from pathlib import Path
import subprocess
import time


ROUTES = {"can0": "1-1.1", "can1": "1-2.1", "can2": "1-1.2", "can3": "1-1.3"}
IP = "/usr/sbin/ip"
TARGET_TXQLEN = 1000


def plan_links(links, devices, *, check_only=False):
    indexed = {link["ifname"]: link for link in links}
    commands = []
    for name, usb in ROUTES.items():
        if name not in indexed or Path(devices.get(name, "")).name.split(":")[0] != usb:
            raise RuntimeError(f"{name}: missing device or USB mapping is not {usb}")
        link = indexed[name]
        if link.get("linkinfo", {}).get("info_kind") != "can":
            raise RuntimeError(f"{name}: not a CAN interface")
        if "UP" in link.get("flags", []):
            bitrate = link["linkinfo"].get("info_data", {}).get("bittiming", {}).get("bitrate")
            if bitrate != 1000000:
                # Do not interrupt an already running controller to fix its link.
                raise RuntimeError(f"{name}: already UP at unexpected bitrate {bitrate}")
            if not check_only and link.get("txqlen") != TARGET_TXQLEN:
                commands.append([IP, "link", "set", "dev", name, "txqueuelen", str(TARGET_TXQLEN)])
        else:
            commands.append([IP, "link", "set", "dev", name, "type", "can", "bitrate", "1000000"])
            if check_only:
                commands.append([IP, "link", "set", "dev", name, "up"])
            else:
                commands.append([IP, "link", "set", "dev", name, "txqueuelen", str(TARGET_TXQLEN), "up"])
    return commands


def inspect_links(*, check_only=False):
    links = json.loads(subprocess.check_output(
        [IP, "-details", "-json", "link", "show", "type", "can"], text=True,
    ))
    devices = {name: str(Path(f"/sys/class/net/{name}/device").resolve(strict=True))
               for name in ROUTES}
    return plan_links(links, devices, check_only=check_only)


def set_cpu_performance_governor():
    for path in sorted(glob.glob("/sys/devices/system/cpu/cpu*/cpufreq/scaling_governor")):
        try:
            with open(path, "w", encoding="utf-8") as handle:
                handle.write("performance\n")
        except OSError:
            pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Read-only: require all links UP at 1 Mbps")
    args = parser.parse_args()
    imu = os.environ.get("THUNDER_IMU_PORT")
    if not imu:
        raise RuntimeError("THUNDER_IMU_PORT must name the configured IMU")
    required = [Path(f"/sys/class/net/{name}/device") for name in ROUTES] + [Path(imu)]
    deadline = time.monotonic() + (0 if args.check else 30)
    while missing := [str(path) for path in required if not path.exists()]:
        if time.monotonic() >= deadline:
            raise RuntimeError(f"Devices unavailable: {', '.join(missing)}")
        time.sleep(0.2)
    commands = inspect_links(check_only=args.check)
    if args.check and commands:
        raise RuntimeError("CAN link is DOWN; read-only check will not configure it")
    for command in commands:
        subprocess.run(command, check=True, timeout=5)
    if inspect_links(check_only=args.check):
        raise RuntimeError("CAN links did not become UP")
    if not args.check:
        set_cpu_performance_governor()
    print(json.dumps({"check_only": args.check, "all_up_1mbps": True,
                      "link_configuration_commands": len(commands)}))


if __name__ == "__main__":
    main()
