#!/usr/bin/env bash
set -euo pipefail

# Board-A (Sunrise X5 / RDK-S100P ARM64) Extreme Real-Time Tuning Script
#
# Goals:
#   1. Lock all CPUs to the "performance" governor (eliminates DVFS frequency ramp jitter).
#   2. Apply kernel RT sysctls (/etc/sysctl.d/99-doso-realtime.conf):
#      sched_rt_runtime_us=-1, timer_migration=0, swappiness=0, vm.stat_interval=60.
#   3. Shield Domain A cores (Core 4-5, mask 0x30): move ALL movable general IRQs to Core 0-3
#      (mask 0x0f), then pin ONLY dedicated CAN/PCAN controller IRQs to Core 4-5.
#
# Usage: sudo /opt/doso/libexec/tune-realtime-irq.sh

CONTROL_CPUMASK="${CONTROL_CPUMASK:-30}" # hex 0x30 = binary 00110000 = Core 4 & Core 5
AUX_CPUMASK="${AUX_CPUMASK:-0f}"         # hex 0x0f = binary 00001111 = Core 0..3

for gov in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
  [ -w "${gov}" ] && echo performance > "${gov}" || true
done

if [ -r /etc/sysctl.d/99-doso-realtime.conf ]; then
  sysctl -q -p /etc/sysctl.d/99-doso-realtime.conf 2>/dev/null || true
fi

[ -w /proc/irq/default_smp_affinity ] && echo "${AUX_CPUMASK}" > /proc/irq/default_smp_affinity 2>/dev/null || true
moved=0
for aff in /proc/irq/[0-9]*/smp_affinity; do
  if [ -w "${aff}" ] && echo "${AUX_CPUMASK}" > "${aff}" 2>/dev/null; then
    moved=$((moved + 1))
  fi
done

can_moved=0
if [ -r /proc/interrupts ]; then
  while IFS= read -r line; do
    irq="$(echo "${line}" | awk -F: '{gsub(/ /, "", $1); print $1}')"
    if [[ "${irq}" =~ ^[0-9]+$ ]] && [ -w "/proc/irq/${irq}/smp_affinity" ]; then
      if echo "${CONTROL_CPUMASK}" > "/proc/irq/${irq}/smp_affinity" 2>/dev/null; then
        can_moved=$((can_moved + 1))
      fi
    fi
  done < <(grep -iE 'pcan|peak_usb|can[0-3]|m_can|flexcan' /proc/interrupts || true)
fi

echo "doso-realtime-tune: governor=performance, irq_to_0_3=${moved}, can_irq_to_4_5=${can_moved}"
