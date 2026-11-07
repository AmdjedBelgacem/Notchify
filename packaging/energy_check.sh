#!/usr/bin/env bash
# Measure Notchisland's idle cost: CPU, wakeups and memory, sampled by `top`.
#
#   packaging/energy_check.sh                 # the running app, 30 s
#   SECONDS_TO_SAMPLE=60 packaging/energy_check.sh
#   BUDGET_CPU=1.0 BUDGET_WAKEUPS=40 packaging/energy_check.sh   # exit 1 if over
#
# Leave the notch collapsed and the Mac idle while it samples. Budgets are
# averages over the run: CPU in percent of one core, idle wakeups per second.
set -euo pipefail

PID="$(pgrep -x notchforge | head -1 || true)"
if [[ -z "$PID" ]]; then
  echo "Notchisland is not running" >&2
  exit 2
fi
SAMPLES=$(( ${SECONDS_TO_SAMPLE:-30} / 5 ))
(( SAMPLES < 2 )) && SAMPLES=2
BUDGET_CPU="${BUDGET_CPU:-1.0}"
BUDGET_WAKEUPS="${BUDGET_WAKEUPS:-40}"

echo "==> Sampling pid $PID for $(( SAMPLES * 5 )) s (keep the notch collapsed)"
# The first top sample has no CPU delta; skip it.
top -l $(( SAMPLES + 1 )) -s 5 -stats pid,cpu,idlew,power,mem -pid "$PID" \
  | awk -v pid="$PID" '$1 == pid { print }' | tail -n +2 \
  | awk -v bc="$BUDGET_CPU" -v bw="$BUDGET_WAKEUPS" '
      { cpu += $2; wake += $3; power += $4; mem = $5; n++ }
      END {
        if (n == 0) { print "no samples"; exit 2 }
        # top reports idle wakeups per sample interval (5 s).
        printf "    CPU      %.2f %% (budget %.1f)\n", cpu / n, bc
        printf "    wakeups  %.1f /s (budget %d)\n", wake / n / 5, bw
        printf "    energy   %.2f (top power score)\n", power / n
        printf "    memory   %s\n", mem
        over = (cpu / n > bc) || (wake / n / 5 > bw)
        if (over) { print "==> Over budget"; exit 1 }
        print "==> Within budget"
      }'
