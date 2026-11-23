#!/usr/bin/env bash
# Atomic release switcher on RDK S100P under converged `/opt/doso`:
#   - rust -> /opt/doso/releases/<version> (doso-brainstem.service)
#   - dart -> optional legacy fallback /opt/brainstem/releases/v0.0.2-unified
# Verifies systemd active state, TCP :5052 BaseGateway, and HTTP :5054 /health.

set -euo pipefail

RUST_RELEASE="${RUST_RELEASE:-/opt/doso/releases/v0.0.1}"
CURRENT_LINK="${CURRENT_LINK:-/opt/doso/current}"
LAST_GOOD_LINK="${LAST_GOOD_LINK:-/opt/doso/last-known-good}"
DART_RELEASE="${DART_RELEASE:-/opt/brainstem/releases/v0.0.2-unified}"
DART_CURRENT_LINK="${DART_CURRENT_LINK:-/opt/brainstem/current}"
GATEWAY_HOST="${GATEWAY_HOST:-127.0.0.1}"
GATEWAY_PORT="${GATEWAY_PORT:-5052}"
ADMIN_PORT="${ADMIN_PORT:-5054}"
HEALTH_TIMEOUT_SEC="${HEALTH_TIMEOUT_SEC:-15}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUST_UNIT_SRC="${SCRIPT_DIR}/doso-brainstem.service"
RUST_UNIT_DST="/etc/systemd/system/doso-brainstem.service"

if [ "$(id -u)" -eq 0 ]; then
  SUDO=""
else
  SUDO="sudo"
fi

usage() {
  cat >&2 <<EOF
Usage: $0 <rust|dart|status>

Targets:
  rust    Switch ${CURRENT_LINK} -> ${RUST_RELEASE} and start doso-brainstem.service
  dart    Fallback to legacy ${DART_RELEASE} and start brainstem.service
  status  Show active symlink, systemd service states, :${GATEWAY_PORT}, and :${ADMIN_PORT}/health
EOF
  exit 1
}

unit_exists() {
  local unit="$1"
  [ -f "/etc/systemd/system/${unit}" ] || [ -f "/lib/systemd/system/${unit}" ] || \
    systemctl list-unit-files "${unit}" 2>/dev/null | grep -q "^${unit}"
}

check_port_open() {
  local host="$1"
  local port="$2"
  if command -v nc >/dev/null 2>&1; then
    nc -z -w 1 "${host}" "${port}" >/dev/null 2>&1
  else
    timeout 1 bash -c "</dev/tcp/${host}/${port}" >/dev/null 2>&1
  fi
}

check_health_http() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsS --max-time 2 "http://${GATEWAY_HOST}:${ADMIN_PORT}/health" | grep -q '"ok":true'
  else
    check_port_open "${GATEWAY_HOST}" "${ADMIN_PORT}"
  fi
}

wait_for_service_health() {
  local service_name="$1"
  local check_admin="$2"
  local elapsed=0
  echo "[switch_runtime] Waiting up to ${HEALTH_TIMEOUT_SEC}s for ${service_name} and ${GATEWAY_HOST}:${GATEWAY_PORT}..."
  while [ "${elapsed}" -lt "${HEALTH_TIMEOUT_SEC}" ]; do
    if systemctl is-active --quiet "${service_name}" && check_port_open "${GATEWAY_HOST}" "${GATEWAY_PORT}"; then
      if [ "${check_admin}" != "1" ] || check_health_http; then
        echo "[switch_runtime] OK: ${service_name} is active and healthy."
        return 0
      fi
    fi
    sleep 1
    elapsed=$((elapsed + 1))
  done

  echo "[switch_runtime] ERROR: Health check failed for ${service_name} after ${HEALTH_TIMEOUT_SEC}s." >&2
  systemctl status "${service_name}" --no-pager >&2 || true
  return 1
}

show_status() {
  echo "Current symlink : $(readlink -f "${CURRENT_LINK}" 2>/dev/null || echo 'unset')"
  echo "Last-known-good : $(readlink -f "${LAST_GOOD_LINK}" 2>/dev/null || echo 'unset')"
  echo "doso-brainstem  : $(systemctl is-active doso-brainstem.service 2>/dev/null || echo 'inactive')"
  echo "brainstem (dart): $(systemctl is-active brainstem.service 2>/dev/null || echo 'inactive')"
  echo "dds-bridge      : $(systemctl is-active brainstem-dds-bridge.service 2>/dev/null || echo 'inactive')"
  if check_port_open "${GATEWAY_HOST}" "${GATEWAY_PORT}"; then
    echo "Gateway :${GATEWAY_PORT}   : reachable"
  else
    echo "Gateway :${GATEWAY_PORT}   : unreachable"
  fi
  if check_health_http; then
    echo "Health  :${ADMIN_PORT}   : ok"
  else
    echo "Health  :${ADMIN_PORT}   : unhealthy/unreachable"
  fi
}

TARGET="${1:-}"
case "${TARGET}" in
  rust)
    if [ ! -x "${RUST_RELEASE}/bin/doso-brainstem" ]; then
      echo "[switch_runtime] ERROR: Missing executable ${RUST_RELEASE}/bin/doso-brainstem" >&2
      exit 2
    fi
    if [ -f "${RUST_UNIT_SRC}" ]; then
      ${SUDO} install -m 0644 "${RUST_UNIT_SRC}" "${RUST_UNIT_DST}"
    fi

    echo "[switch_runtime] Stopping legacy Dart brainstem services (if running)..."
    for legacy_unit in brainstem-dds-bridge.service brainstem.service; do
      if unit_exists "${legacy_unit}"; then
        ${SUDO} systemctl stop "${legacy_unit}" || true
        ${SUDO} systemctl disable "${legacy_unit}" >/dev/null 2>&1 || true
      fi
    done

    prev_target="$(readlink -f "${CURRENT_LINK}" 2>/dev/null || true)"
    if [ -n "${prev_target}" ] && [ "${prev_target}" != "${RUST_RELEASE}" ] && [ -d "${prev_target}" ]; then
      ${SUDO} ln -sfn "${prev_target}" "${LAST_GOOD_LINK}"
    fi

    echo "[switch_runtime] Linking ${CURRENT_LINK} -> ${RUST_RELEASE}"
    ${SUDO} mkdir -p "$(dirname "${CURRENT_LINK}")"
    ${SUDO} ln -sfn "${RUST_RELEASE}" "${CURRENT_LINK}"
    ${SUDO} systemctl daemon-reload
    ${SUDO} systemctl enable doso-brainstem.service >/dev/null
    ${SUDO} systemctl restart doso-brainstem.service

    wait_for_service_health "doso-brainstem.service" 1
    ;;

  dart)
    if [ ! -d "${DART_RELEASE}" ]; then
      echo "[switch_runtime] ERROR: Missing legacy Dart release directory ${DART_RELEASE}" >&2
      exit 2
    fi

    echo "[switch_runtime] Stopping Rust doso-brainstem.service (if running)..."
    if unit_exists "doso-brainstem.service"; then
      ${SUDO} systemctl stop doso-brainstem.service || true
      ${SUDO} systemctl disable doso-brainstem.service >/dev/null 2>&1 || true
    fi

    echo "[switch_runtime] Linking ${DART_CURRENT_LINK} -> ${DART_RELEASE}"
    ${SUDO} mkdir -p "$(dirname "${DART_CURRENT_LINK}")"
    ${SUDO} ln -sfn "${DART_RELEASE}" "${DART_CURRENT_LINK}"
    ${SUDO} systemctl daemon-reload

    ${SUDO} systemctl enable brainstem.service >/dev/null
    ${SUDO} systemctl restart brainstem.service
    if unit_exists "brainstem-dds-bridge.service"; then
      ${SUDO} systemctl enable brainstem-dds-bridge.service >/dev/null
      ${SUDO} systemctl restart brainstem-dds-bridge.service
    fi

    wait_for_service_health "brainstem.service" 0
    ;;

  status)
    show_status
    ;;

  *)
    usage
    ;;
esac
