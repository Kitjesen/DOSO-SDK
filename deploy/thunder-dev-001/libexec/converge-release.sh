#!/usr/bin/env bash
set -euo pipefail

# Run only while the robot is Grounded and motor_output_enabled=false.
RELEASE_VERSION="${RELEASE_VERSION:-v0.0.1}"
BASELINE_VERSION="${BASELINE_VERSION:-v0.0.0-baseline}"
RELEASES_DIR=/opt/brainstem/releases
STAGING_DIR=/home/sunrise/deploy_staging
ARCHIVE_DIR=/home/sunrise/archive
ARCHIVE_TAR="${ARCHIVE_DIR}/pre-${RELEASE_VERSION}-legacy.tar.gz"
ACTIVE_TARGET="$(readlink -f /opt/brainstem/current || true)"

if [ "$(systemctl is-active brainstem.service || true)" != "active" ]; then
  echo "WARNING: brainstem.service was not active before convergence" >&2
fi

if [ ! -d "${RELEASES_DIR}/${RELEASE_VERSION}" ]; then
  if [ -n "${ACTIVE_TARGET}" ] && [ -d "${ACTIVE_TARGET}" ] && [ "${ACTIVE_TARGET}" != "${RELEASES_DIR}/${RELEASE_VERSION}" ]; then
    sudo rm -rf "${RELEASES_DIR}/${RELEASE_VERSION}.tmp"
    sudo cp -a "${ACTIVE_TARGET}" "${RELEASES_DIR}/${RELEASE_VERSION}.tmp"
    if [ -f "${RELEASES_DIR}/${RELEASE_VERSION}.tmp/manifest.json" ]; then
      sudo sed -i -E "s/\"release\":[[:space:]]*\"[^\"]+\"/\"release\": \"${RELEASE_VERSION}\"/" "${RELEASES_DIR}/${RELEASE_VERSION}.tmp/manifest.json"
    fi
    sudo mv "${RELEASES_DIR}/${RELEASE_VERSION}.tmp" "${RELEASES_DIR}/${RELEASE_VERSION}"
  else
    echo "ERROR: ${RELEASES_DIR}/${RELEASE_VERSION} does not exist and no active release to promote" >&2
    exit 1
  fi
fi

# 1. Preserve rollback baseline (rename legacy 31af705 if no baseline exists yet)
if [ -d "${RELEASES_DIR}/31af705" ] && [ ! -d "${RELEASES_DIR}/${BASELINE_VERSION}" ]; then
  sudo mv "${RELEASES_DIR}/31af705" "${RELEASES_DIR}/${BASELINE_VERSION}"
  if [ -f "${RELEASES_DIR}/${BASELINE_VERSION}/manifest.json" ]; then
    sudo sed -i -E "s/\"release\":[[:space:]]*\"[^\"]+\"/\"release\": \"${BASELINE_VERSION}\"/" "${RELEASES_DIR}/${BASELINE_VERSION}/manifest.json"
  fi
fi

# 2. Switch /opt/brainstem/current -> ${RELEASE_VERSION} and restart
if [ "${ACTIVE_TARGET}" != "${RELEASES_DIR}/${RELEASE_VERSION}" ] || [ "${FORCE_RESTART:-0}" = "1" ]; then
  sudo ln -sfn "${RELEASES_DIR}/${RELEASE_VERSION}" /opt/brainstem/current
  sudo systemctl restart brainstem.service
  sleep 2
  if [ "$(systemctl is-active brainstem.service || true)" != "active" ]; then
    echo "ERROR: brainstem.service failed on ${RELEASE_VERSION}; rolling back to ${BASELINE_VERSION}" >&2
    if [ -d "${RELEASES_DIR}/${BASELINE_VERSION}" ]; then
      sudo ln -sfn "${RELEASES_DIR}/${BASELINE_VERSION}" /opt/brainstem/current
      sudo systemctl restart brainstem.service
    fi
    exit 1
  fi
fi

# 3. Remove transitional SHA/step release directories; keep ONLY current release and baseline
shopt -s nullglob
for dir in "${RELEASES_DIR}"/*; do
  name="$(basename "${dir}")"
  if [ "${name}" != "${RELEASE_VERSION}" ] && [ "${name}" != "${BASELINE_VERSION}" ]; then
    sudo rm -rf "${dir}"
  fi
done

# 4. Clean deploy_staging to keep only brainstem-${RELEASE_VERSION} (without build/ or .dart_tool/)
if [ -d "${STAGING_DIR}" ]; then
  for entry in "${STAGING_DIR}"/*; do
    if [ "$(basename "${entry}")" != "brainstem-${RELEASE_VERSION}" ]; then
      rm -rf "${entry}"
    fi
  done
  rm -rf "${STAGING_DIR}/brainstem-${RELEASE_VERSION}/build" "${STAGING_DIR}/brainstem-${RELEASE_VERSION}/.dart_tool"
fi

# 5. Archive and zero out legacy deployment / drop-in folders after verifying archive integrity
LEGACY_ITEMS=()
for item in \
  /home/sunrise/deployments \
  /home/sunrise/deploy_backups \
  /home/sunrise/brainstem_mixed_can_stage_* \
  /home/sunrise/thunder_receiver_* \
  /home/sunrise/voice_local_player_test_* \
  /etc/systemd/system/brainstem.service.d.archived*; do
  if [ -e "${item}" ]; then
    LEGACY_ITEMS+=("${item}")
  fi
done

if [ "${#LEGACY_ITEMS[@]}" -gt 0 ]; then
  mkdir -p "${ARCHIVE_DIR}"
  TMP_TAR="${ARCHIVE_TAR}.tmp"
  sudo tar -czf "${TMP_TAR}" "${LEGACY_ITEMS[@]}"
  sudo tar -tzf "${TMP_TAR}" >/dev/null
  sudo mv "${TMP_TAR}" "${ARCHIVE_TAR}"
  OWNER="${SUDO_USER:-sunrise}"
  sudo chown "${OWNER}:${OWNER}" "${ARCHIVE_TAR}"
  for item in "${LEGACY_ITEMS[@]}"; do
    sudo rm -rf "${item}"
  done
  echo "Archived and removed ${#LEGACY_ITEMS[@]} legacy directories into ${ARCHIVE_TAR}"
fi

echo "=== Semantic convergence complete (${RELEASE_VERSION}) ==="
ls -la "${RELEASES_DIR}" /opt/brainstem/current "${STAGING_DIR}" 2>&1 || true
