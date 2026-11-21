"""Generate LingTu's Python bindings from the SDK's domain protocols."""

import argparse
import re
import subprocess
import sys
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lingtu", type=Path, required=True)
    args = parser.parse_args()
    source = Path(__file__).resolve().parents[1] / "proto"
    protos = sorted(source.rglob("*.proto"))
    target = args.lingtu.resolve() / "src/gateway/rpc/generated"
    if not (args.lingtu / "src/gateway/gateway_module.py").is_file():
        parser.error("--lingtu must identify a LingTu checkout")
    target.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            sys.executable,
            "-m",
            "grpc_tools.protoc",
            f"-I{source}",
            f"--python_out={target}",
            f"--grpc_python_out={target}",
            *(str(path) for path in protos),
        ],
        check=True,
    )
    paths = [path.relative_to(source).with_suffix("") for path in protos]
    expected = {path.parent / f"{path.name}{suffix}.py" for path in paths for suffix in ("_pb2", "_pb2_grpc")}
    modules = {".".join((*path.parent.parts, f"{path.name}_pb2")) for path in paths}
    packages = {parent for path in paths for parent in path.parents}
    for bindings in [*target.rglob("*_pb2.py"), *target.rglob("*_pb2_grpc.py")]:
        relative = bindings.relative_to(target)
        if relative not in expected:
            bindings.unlink()
            continue
        prefix = "." * (len(relative.parent.parts) + 1)
        text = bindings.read_text(encoding="utf-8")
        text = re.sub(r"^import (\w+_pb2) as ", rf"from {prefix} import \1 as ", text, flags=re.MULTILINE)

        def package_import(match, prefix=prefix):
            module, names = match.groups()
            if module in modules or f"{module}.{names.split()[0]}" in modules:
                return f"from {prefix}{module} import {names}"
            return match.group()

        text = re.sub(r"^from ([\w.]+) import (.+)$", package_import, text, flags=re.MULTILINE)
        bindings.write_text(text, encoding="utf-8")
    for initializer in target.rglob("__init__.py"):
        if initializer.parent.relative_to(target) not in packages:
            initializer.unlink()
    for package in packages:
        (target / package / "__init__.py").write_text(
            '"""Generated DOSO protocol bindings; do not edit by hand."""\n', encoding="utf-8"
        )
    source = Path(__file__).resolve().parents[1] / "brainstem/protocol"
    target = args.lingtu.resolve() / "src/runtime/adapters/brainstem/generated"
    target.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            sys.executable,
            "-m",
            "grpc_tools.protoc",
            f"-I{source}",
            f"--python_out={target}",
            *(str(path) for path in sorted((source / "brainstem_api").glob("*.proto"))),
        ],
        check=True,
    )
    for bindings in (target / "brainstem_api").glob("*_pb2*.py"):
        bindings.write_text(
            re.sub(
                r"^from brainstem_api import ",
                "from . import ",
                bindings.read_text(encoding="utf-8"),
                flags=re.MULTILINE,
            ),
            encoding="utf-8",
        )
    for package in (target, target / "brainstem_api"):
        (package / "__init__.py").write_text(
            '"""Generated Brainstem protocol bindings; do not edit by hand."""\n', encoding="utf-8"
        )


if __name__ == "__main__":
    main()
