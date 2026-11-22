#!/usr/bin/env bash
set -euo pipefail

# Board-A (Sunrise X5 / RDK-S100 ARM64) Real-Time Tuning Script (Phase 3)
#
# Goals:
#   1. Lock all CPUs to the "performance" governor (eliminates DVFS frequency ramp jitter).
#   2. Pin CAN/USB-PCAN controller IRQs to Core 4-5 (matching brainstem.service CPUAffinity=4 5)
#      so camera/video workloads on Core 0-3 cannot preempt SocketCAN RX/TX softirqs.
#   3. Verify /dev/shm mount options and RT throttling limits for 200Hz-1kHz closed-loop RL.
#
# Usage: sudo /opt/brainstem/libexec/tune-realtime-irq.sh

CONTROL_CPUMASK="${CONTROL_CPUMASK:-30}" # hex 0x30 = binary 00110000 = Core 4 & Core 5
AUX_CPUMASK="${AUX_CPUMASK:-0f}"         # hex 0x0f = binary 00001111 = Core 0..3

echo "=== [1/4] Locking CPU scaling_governor=performance ==="
for gov in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
  if [ -w "${gov}" ]; then
    echo performance > "${gov}"
  fi
done
head -n 1 /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor 2>/dev/null || true

echo "=== [2/4] Pinning CAN / USB-PCAN IRQs to mask 0x${CONTROL_CPUMASK} (Core 4-5) ==="
if [ -r /proc/interrupts ]; then
  # Match peak_usb / pcan / can / xhci host controllers serving the 4-leg PCAN bus
  while IFS= read -r line; do
    irq="$(echo "${line}" | awk -F: '{gsub(/ /, "", $1); print $1}')"
    if [[ "${irq}" =~ ^[0-9]+$ ]] && [ -w "/proc/irq/${irq}/smp_affinity" ]; then
      echo "${CONTROL_CPUMASK}" > "/proc/irq/${irq}/smp_affinity" 2>/dev/null || true
      echo "  IRQ ${irq} -> mask 0x${CONTROL_CPUMASK} ($(echo "${line}" | awk '{print $NF}'))"
    fi
  done < <(grep -iE 'pcan|peak_usb|can[0-3]|m_can|flexcan|xhci' /proc/interrupts || true)
fi

echo "=== [3/4] Checking /dev/shm and POSIX SHM segments ==="
df -h /dev/shm || true
ls -la /dev/shm/doso_* 2>/dev/null || echo "  (no /dev/shm/doso_* segments active yet)"

echo "=== [4/4] Checking RT scheduling limits ==="
for k in /proc/sys/kernel/sched_rt_runtime_us /proc/sys/kernel/sched_rt_period_us; do
  if [ -r "${k}" ]; then
    echo "  ${k} = $(cat "${k}")"
  fi
done
echo "=== Board-A RT tuning complete ==="
