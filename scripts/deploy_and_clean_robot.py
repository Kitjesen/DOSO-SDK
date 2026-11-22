#!/usr/bin/env python3
"""Turnkey Brainstem deployer & robot semantic directory cleaner.

Deploys the Brainstem bundle versioned from D:\\inovxio\\brain\\brainstem\\VERSION
(currently v0.0.1) to THUNDER-DEV-001 (192.168.66.64), switches
/opt/brainstem/current atomically, archives legacy directories, and converges:
  - /opt/brainstem/releases/     -> ONLY <version> (e.g. v0.0.1) and v0.0.0-baseline
  - /home/sunrise/deploy_staging -> ONLY brainstem-<version> (without build/.dart_tool)

Safety guarantees:
  - Never modifies host network adapters or switches Wi-Fi.
  - Verifies Grounded + motor_output_enabled=false + owner=None before restart.
  - Isolates ARM64 build/tests to Core 0-3 (taskset -c 0-3).
  - Auto-rolls back to baseline if brainstem.service fails to become active.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import io
import os
import pathlib
import socket
import subprocess
import sys
import tarfile

DEFAULT_HOST = "192.168.66.64"
DEFAULT_USER = "sunrise"
DEFAULT_IDENTITY = pathlib.Path.home() / ".ssh" / "inovxio_deploy"
DEFAULT_REPO = pathlib.Path(r"D:\inovxio\brain\brainstem")
DEFAULT_CONVERGE = pathlib.Path(
    r"D:\inovxio\brain\DOSO-SDK\deploy\thunder-dev-001\libexec\converge-release.sh"
)
DEFAULT_ARCHIVE_OUT = pathlib.Path(r"D:\inovxio\archive\robot-cleanup-20261121")
DEFAULT_BASELINE = "v0.0.0-baseline"

EXCLUDED_DIRS = {".git", ".dart_tool", "build", "logs", ".local", "__pycache__", ".pytest_cache"}
EXCLUDED_SUFFIXES = {".log", ".tgz", ".tar.gz", ".tmp"}
TEXT_SUFFIXES = {
    ".sh", ".py", ".dart", ".yaml", ".yml", ".proto", ".md", ".json",
    ".toml", ".txt", ".c", ".cc", ".cpp", ".cxx", ".h", ".hh", ".hpp",
    ".hxx", ".patch", ".diff", ".cmake", ".in", ".lua", ".rs", ".lock",
    ".env", ".service", ".cfg", ".conf", ".ini", ".xml", ".html", ".css",
    ".js", ".ts", ".m", ".mm",
}
TEXT_NAMES = {"VERSION", "CMakeLists.txt", "Makefile", "LICENSE"}


def default_release_version(repo: pathlib.Path) -> str:
    vf = repo / "VERSION"
    if vf.exists():
        raw = vf.read_text(encoding="utf-8").strip()
        if raw:
            return raw if raw.startswith("v") else f"v{raw}"
    return "v0.0.1"


def tcp_reachable(host: str, port: int, timeout: float = 2.0) -> bool:
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True
    except OSError:
        return False


def ssh_base_args(identity: pathlib.Path | None) -> list[str]:
    args = ["-o", "ConnectTimeout=5", "-o", "BatchMode=yes"]
    if identity is not None and identity.exists():
        args.extend(["-i", str(identity), "-o", "IdentitiesOnly=yes"])
    return args


def run_ssh(
    user: str,
    host: str,
    remote_cmd: str,
    *,
    identity: pathlib.Path | None = None,
    stdin_text: str | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    cmd = ["ssh", *ssh_base_args(identity), f"{user}@{host}", remote_cmd]
    raw_in = (
        stdin_text.replace("\r\n", "\n").encode("utf-8")
        if stdin_text is not None
        else None
    )
    proc = subprocess.run(cmd, input=raw_in, check=False, capture_output=True)
    res = subprocess.CompletedProcess(
        args=proc.args,
        returncode=proc.returncode,
        stdout=proc.stdout.decode("utf-8", errors="replace"),
        stderr=proc.stderr.decode("utf-8", errors="replace"),
    )
    if check and res.returncode != 0:
        raise subprocess.CalledProcessError(
            res.returncode, res.args, output=res.stdout, stderr=res.stderr
        )
    return res


def run_scp(
    src: str,
    dst: str,
    *,
    identity: pathlib.Path | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    cmd = ["scp", "-q", *ssh_base_args(identity), src, dst]
    return subprocess.run(cmd, check=check, text=True, capture_output=True)


def check_robot_safety(host: str) -> tuple[bool, str]:
    sdk_root = pathlib.Path(__file__).resolve().parents[1]
    # Prefer unified doso.v1 Base Gateway (:5052) when available so :13145 can stay loopback-only.
    if tcp_reachable(host, 5052):
        py_sdk_dir = str(sdk_root / "python")
        if py_sdk_dir not in sys.path:
            sys.path.insert(0, py_sdk_dir)
        try:
            from doso_sdk import RobotClient  # type: ignore

            with RobotClient.connect(f"http://{host}:5052", timeout_ms=4000) as robot:
                st = robot.sport().control_status()
            grounded = st.body_kind == 1  # CmsStateKind::Grounded
            motors_off = not st.motor_output_enabled
            owner_none = st.source_owner == 0
            summary = (
                f"via=:5052 grounded={grounded} motor_output_enabled={st.motor_output_enabled} "
                f"owner_none={owner_none}"
            )
            return (grounded and motors_off and owner_none), summary
        except Exception as exc:
            # Fall back to direct :13145 probe if FFI library isn't built yet.
            if not tcp_reachable(host, 13145):
                return False, f"gateway(:5052) check failed and :13145 unreachable: {exc}"

    exe_candidates = [
        sdk_root / "target" / "release" / "examples" / "read_brainstem.exe",
        sdk_root / "target" / "debug" / "examples" / "read_brainstem.exe",
        sdk_root / "target" / "release" / "examples" / "read_brainstem",
        sdk_root / "target" / "debug" / "examples" / "read_brainstem",
    ]
    exe = next((p for p in exe_candidates if p.exists()), None)
    if exe is not None:
        cmd = [str(exe), f"http://{host}:13145"]
    else:
        cmd = [
            "cargo",
            "run",
            "--quiet",
            "--example",
            "read_brainstem",
            "--",
            f"http://{host}:13145",
        ]
    res = subprocess.run(cmd, cwd=str(sdk_root), check=False, text=True, capture_output=True)
    out = (res.stdout or "") + (res.stderr or "")
    if res.returncode != 0:
        return False, f"read_brainstem exit={res.returncode}: {out.strip()[:300]}"
    grounded = "kind: Grounded," in out
    motors_off = "motor_output_enabled: false," in out
    owner_none = "owner: None," in out
    summary = f"via=:13145 grounded={grounded} motor_output_enabled={not motors_off} owner_none={owner_none}"
    return (grounded and motors_off and owner_none), summary


def build_clean_source_tar(repo: pathlib.Path, dst_tar: pathlib.Path) -> pathlib.Path:
    dst_tar.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(dst_tar, "w:gz") as tf:
        for root, dirs, files in os.walk(repo):
            dirs[:] = sorted(d for d in dirs if d not in EXCLUDED_DIRS and not d.startswith("."))
            root_path = pathlib.Path(root)
            for fname in sorted(files):
                if any(fname.endswith(sfx) for sfx in EXCLUDED_SUFFIXES):
                    continue
                fpath = root_path / fname
                rel = fpath.relative_to(repo).as_posix()
                data = fpath.read_bytes()
                if fpath.suffix.lower() in TEXT_SUFFIXES or fpath.name in TEXT_NAMES:
                    data = data.replace(b"\r\n", b"\n")
                info = tarfile.TarInfo(name=rel)
                info.size = len(data)
                info.mode = 0o755 if fpath.suffix == ".sh" else 0o644
                tf.addfile(info, io.BytesIO(data))
    return dst_tar


def main() -> int:
    parser = argparse.ArgumentParser(description="Deploy Brainstem release & clean robot directories.")
    parser.add_argument("--host", default=DEFAULT_HOST)
    parser.add_argument("--user", default=DEFAULT_USER)
    parser.add_argument("--identity", type=pathlib.Path, default=DEFAULT_IDENTITY)
    parser.add_argument("--repo", type=pathlib.Path, default=DEFAULT_REPO)
    parser.add_argument("--version", default=None, help="Release tag (default: v<brainstem/VERSION>).")
    parser.add_argument("--baseline", default=DEFAULT_BASELINE)
    parser.add_argument("--converge-script", type=pathlib.Path, default=DEFAULT_CONVERGE)
    parser.add_argument("--archive-out", type=pathlib.Path, default=DEFAULT_ARCHIVE_OUT)
    parser.add_argument("--skip-remote-tests", action="store_true", help="Skip ARM64 dart test (still runs pub get + analyze + build).")
    parser.add_argument("--check-only", action="store_true", help="Only check robot reachability and safety state.")
    args = parser.parse_args()

    release_ver = args.version or default_release_version(args.repo)
    protocol_ver = (args.repo / "brainstem_api" / "VERSION").read_text(encoding="utf-8").strip()

    ssh_ok = tcp_reachable(args.host, 22)
    gw_ok = tcp_reachable(args.host, 5052)
    grpc_ok = tcp_reachable(args.host, 13145)
    print(f"Target release: {release_ver} (brainstem_api {protocol_ver})")
    print(f"Reachability {args.host}: ssh(22)={ssh_ok}, gateway(5052)={gw_ok}, brainstem(13145)={grpc_ok}")
    if not ssh_ok:
        print(
            f"ERROR: Robot SSH ({args.host}:22) is unreachable from this host.\n"
            "Ensure your LAN/Wi-Fi is connected to the robot network (e.g. TopSpeed5GCPE-5G-5525 / 192.168.66.x).",
            file=sys.stderr,
        )
        return 2

    safe, detail = check_robot_safety(args.host)
    print(f"Safety preflight: safe={safe} ({detail})")
    if args.check_only:
        return 0 if safe else 3
    if not safe:
        print(
            "ABORT: Refusing deploy because robot is not verified Grounded with motor_output_enabled=false.",
            file=sys.stderr,
        )
        return 3

    tar_path = args.archive_out / f"brainstem-{release_ver}-src.tar.gz"
    build_clean_source_tar(args.repo, tar_path)
    src_sha = hashlib.sha256(tar_path.read_bytes()).hexdigest()[:12]
    print(f"Packed clean Brainstem source: {tar_path} (sha256:{src_sha})")

    run_ssh(
        args.user,
        args.host,
        f"mkdir -p /home/sunrise/deploy_staging/brainstem-{release_ver}",
        identity=args.identity,
    )
    run_scp(
        str(tar_path),
        f"{args.user}@{args.host}:/home/sunrise/deploy_staging/brainstem-{release_ver}-src.tar.gz",
        identity=args.identity,
    )
    lf_converge = args.archive_out / "converge-release.sh"
    lf_converge.write_bytes(args.converge_script.read_bytes().replace(b"\r\n", b"\n"))
    run_scp(
        str(lf_converge),
        f"{args.user}@{args.host}:/tmp/converge-release.sh",
        identity=args.identity,
    )

    test_block = (
        'echo "Skipping remote ARM64 dart test (--skip-remote-tests)"'
        if args.skip_remote_tests
        else '(cd thunder_brain && taskset -c 0-3 "$DART" test -j 2)\n(cd thunder && taskset -c 0-3 "$DART" test -j 2)'
    )

    remote_build_and_converge = f"""
set -euo pipefail
RELEASE_VERSION="{release_ver}"
BASELINE_VERSION="{args.baseline}"
cd /home/sunrise/deploy_staging
rm -rf "brainstem-${{RELEASE_VERSION}}" && mkdir -p "brainstem-${{RELEASE_VERSION}}"
tar -xzf "brainstem-${{RELEASE_VERSION}}-src.tar.gz" -C "brainstem-${{RELEASE_VERSION}}"
rm -f "brainstem-${{RELEASE_VERSION}}-src.tar.gz"

cd "brainstem-${{RELEASE_VERSION}}"
if command -v dart >/dev/null 2>&1; then
  DART=dart
elif [ -x /usr/local/bin/dart ]; then
  DART=/usr/local/bin/dart
elif [ -x /home/sunrise/dart-sdk/bin/dart ]; then
  DART=/home/sunrise/dart-sdk/bin/dart
else
  echo "ERROR: dart SDK not found on robot" >&2
  exit 1
fi

taskset -c 0-3 "$DART" pub get --offline || taskset -c 0-3 "$DART" pub get
taskset -c 0-3 "$DART" run brainstem_api/tool/version_contract_test.dart brainstem_api
taskset -c 0-3 "$DART" analyze brainstem_api/dart thunder_brain thunder/lib thunder/bin
{test_block}

rm -rf "build/${{RELEASE_VERSION}}"
cd thunder
taskset -c 0-3 "$DART" build cli -o "../build/${{RELEASE_VERSION}}" -t bin/thunder.dart
BUNDLE_DIR="../build/${{RELEASE_VERSION}}/bundle"
mkdir -p "${{BUNDLE_DIR}}/lib"
if [ -f /opt/brainstem/current/lib/libcserialport.so ]; then
  cp -an /opt/brainstem/current/lib/libcserialport.so "${{BUNDLE_DIR}}/lib/" || true
fi
cat > "${{BUNDLE_DIR}}/manifest.json" <<EOF
{{
  "release": "${{RELEASE_VERSION}}",
  "protocol_version": "{protocol_ver}",
  "source_sha256_prefix": "{src_sha}",
  "built_at_utc": "{datetime.datetime.now(datetime.timezone.utc).isoformat()}",
  "capabilities": ["Builtin", "Damp", "RobotExternalControl", "LocalStatusService"]
}}
EOF
(cd "${{BUNDLE_DIR}}" && sha256sum bin/thunder lib/libcserialport.so manifest.json > SHA256SUMS)

sudo rm -rf "/opt/brainstem/releases/${{RELEASE_VERSION}}.new"
sudo cp -a "${{BUNDLE_DIR}}" "/opt/brainstem/releases/${{RELEASE_VERSION}}.new"
if [ "$(readlink -f /opt/brainstem/current || true)" = "/opt/brainstem/releases/${{RELEASE_VERSION}}" ]; then
  if [ -d "/opt/brainstem/releases/${{BASELINE_VERSION}}" ]; then
    sudo ln -sfn "/opt/brainstem/releases/${{BASELINE_VERSION}}" /opt/brainstem/current
  fi
fi
sudo rm -rf "/opt/brainstem/releases/${{RELEASE_VERSION}}"
sudo mv "/opt/brainstem/releases/${{RELEASE_VERSION}}.new" "/opt/brainstem/releases/${{RELEASE_VERSION}}"

sudo install -m 0755 /tmp/converge-release.sh /opt/brainstem/libexec/converge-release.sh
rm -f /tmp/converge-release.sh
sudo RELEASE_VERSION="${{RELEASE_VERSION}}" BASELINE_VERSION="${{BASELINE_VERSION}}" FORCE_RESTART=1 /opt/brainstem/libexec/converge-release.sh
"""
    res = run_ssh(
        args.user,
        args.host,
        "bash -s",
        identity=args.identity,
        stdin_text=remote_build_and_converge,
        check=False,
    )
    print(res.stdout)
    if res.stderr:
        print(res.stderr, file=sys.stderr)
    if res.returncode != 0:
        return res.returncode

    args.archive_out.mkdir(parents=True, exist_ok=True)
    run_scp(
        f"{args.user}@{args.host}:/home/sunrise/archive/pre-{release_ver}-legacy.tar.gz",
        str(args.archive_out / f"pre-{release_ver}-legacy.tar.gz"),
        identity=args.identity,
        check=False,
    )
    post = run_ssh(
        args.user,
        args.host,
        "systemctl is-active brainstem.service && ls -la /opt/brainstem/releases /opt/brainstem/current /home/sunrise/deploy_staging",
        identity=args.identity,
        check=False,
    )
    print(post.stdout)
    return post.returncode


if __name__ == "__main__":
    raise SystemExit(main())
