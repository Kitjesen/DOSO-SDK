"""Submit one navigation target. Running this example requests robot motion."""

import argparse
import os

from doso import DosoClient, DosoError, new_request_id


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--x", type=float, required=True)
    parser.add_argument("--y", type=float, required=True)
    parser.add_argument("--z", type=float, required=True)
    parser.add_argument("--yaw", type=float, required=True, help="Map-frame radians")
    args = parser.parse_args()
    robot = DosoClient(args.url, client_id="doso-navigation-example", api_key=os.environ.get("DOSO_API_KEY"))
    request_id = new_request_id()
    print("request_id:", request_id, flush=True)
    try:
        receipt = robot.navigation.go_to(args.x, args.y, z=args.z, yaw=args.yaw, request_id=request_id)
        print("request_id:", receipt.request_id, "task_id:", receipt.task_id, flush=True)
        if not receipt.ok or receipt.accepted is not True or not receipt.task_id:
            parser.exit(1, f"Navigation was not confirmed accepted: {receipt.raw}\n")
        print(robot.navigation.wait(receipt.task_id))
    except (DosoError, TimeoutError) as exc:
        parser.exit(1, f"{exc}\nInspect retained task/request identity; this example does not automatically cancel.\n")


if __name__ == "__main__":
    main()
