#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ndk_root="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
if [[ -z "$ndk_root" && -n "${ANDROID_HOME:-}" ]]; then
  ndk_root="$ANDROID_HOME/ndk/27.1.12297006"
fi
output_dir="$script_dir/jniLibs"
abi="arm64-v8a"
while (($#)); do
  case "$1" in
    --ndk-root) ndk_root="$2"; shift 2 ;;
    --output-dir) output_dir="$2"; shift 2 ;;
    --abi) abi="$2"; shift 2 ;;
    *) echo "Unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$abi" in
  arm64-v8a) target="aarch64-linux-android" ;;
  x86_64) target="x86_64-linux-android" ;;
  *) echo "Supported ABIs: arm64-v8a, x86_64" >&2; exit 2 ;;
esac
case "$(uname -s)" in
  Linux) host="linux-x86_64" ;;
  Darwin) host="darwin-x86_64" ;;
  *) echo "Use build.ps1 on Windows." >&2; exit 2 ;;
esac
if [[ -z "$ndk_root" ]]; then
  echo "Set ANDROID_NDK_HOME or pass --ndk-root." >&2
  exit 2
fi
if ! rustup target list --installed | grep -Fxq "$target"; then
  echo "Install the Rust target before building: rustup target add $target" >&2
  exit 2
fi
llvm="$ndk_root/toolchains/llvm/prebuilt/$host/bin"
clang="$llvm/${target}24-clang"
archive="$llvm/llvm-ar"
if [[ ! -x "$clang" || ! -x "$archive" ]]; then
  echo "The NDK toolchain is unavailable: $llvm" >&2
  exit 2
fi
suffix="${target//-/_}"
linker="CARGO_TARGET_$(printf '%s' "$suffix" | tr '[:lower:]' '[:upper:]')_LINKER"
env "$linker=$clang" "CC_$suffix=$clang" "AR_$suffix=$archive" \
  cargo build --manifest-path "$script_dir/Cargo.toml" --target-dir "$script_dir/target" \
  --locked --release --target "$target"
mkdir -p -- "$output_dir/$abi"
cp -- "$script_dir/target/$target/release/libdoso_sdk_android.so" "$output_dir/$abi/libdoso_sdk_android.so"
printf '%s\n' "$output_dir/$abi/libdoso_sdk_android.so"
