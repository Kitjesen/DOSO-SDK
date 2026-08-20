#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="${BRAINSTEM_VERIFY_WORK_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/DOSO-Brainstem-CPP-SDK.XXXXXX")}"
BUILD="${WORK}/build"
INSTALL="${WORK}/install"
CONSUMER="${WORK}/consumer"

cmake -S "${ROOT}" -B "${BUILD}" \
  -DBUILD_TESTING=ON \
  -DBRAINSTEM_BUILD_EXAMPLES=ON \
  -DCMAKE_BUILD_TYPE=Release
cmake --build "${BUILD}" --parallel
ctest --test-dir "${BUILD}" --output-on-failure

cmake --install "${BUILD}" --prefix "${INSTALL}"
cmake -S "${ROOT}/tests/package_consumer" -B "${CONSUMER}" \
  -DCMAKE_PREFIX_PATH="${INSTALL}" \
  -DCMAKE_BUILD_TYPE=Release
cmake --build "${CONSUMER}" --parallel
"${CONSUMER}/brainstem_client_consumer"

echo "DOSO Brainstem C++ SDK verification passed"
echo "verification work directory: ${WORK}"
