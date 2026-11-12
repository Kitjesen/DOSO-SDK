"""Read-only first connection to an explicitly selected Gateway."""

import argparse
import json
import os

from doso import DosoClient, DosoError


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True, help="Gateway HTTP(S) URL")
    args = parser.parse_args()
    robot = DosoClient(args.url, api_key=os.environ.get("DOSO_API_KEY"))
    try:
        for name, read in (
            ("ping", robot.system.ping),
            ("readiness", robot.system.readiness),
            ("capabilities", robot.system.capabilities),
            ("maps", robot.maps.list),
            ("localization", robot.localization.status),
        ):
            print(name, json.dumps(read(), ensure_ascii=False, indent=2))
    except DosoError as exc:
        parser.exit(1, f"{exc}\n")


if __name__ == "__main__":
    main()
