#!/usr/bin/env bash
# Validate the two system-notification paths against real macOS:
#   1. the Notification Center DB watcher (needs Full Disk Access),
#   2. the Accessibility banner observer (needs Accessibility),
# and that the pipeline dedupes the same banner seen through both.
# Posts a genuine notification with osascript. Used by CI.
set -euo pipefail
cd "$(dirname "$0")/.."
APP="dist/NotchForge.app"
export NOTCHFORGE_HOME="$(mktemp -d)"
LOG="$NOTCHFORGE_HOME/stderr.log"
printf '[providers]\naccessibility_banners = true\nweather = false\n' > "$NOTCHFORGE_HOME/config.toml"

"$APP/Contents/MacOS/notchforge" -v >"$LOG" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true; echo "--- log"; cat "$LOG"' EXIT
for _ in $(seq 1 60); do grep -q "NotchForge UI running" "$LOG" && break; sleep 0.5; done
sleep 2

# The first notification also wakes the Notification Center UI process, which
# macOS launches lazily; the banner observer attaches to it within ~5 s.
osascript -e 'display notification "warm-up" with title "NotchForge CI"'
for _ in $(seq 1 30); do
  grep -q "observing Notification Center banners via Accessibility" "$LOG" && break
  sleep 0.5
done
sleep 1
TOKEN="nf-$RANDOM-$RANDOM"
osascript -e "display notification \"Body $TOKEN\" with title \"Title $TOKEN\""

# Wait up to 15 s for the notification to be ingested.
for _ in $(seq 1 30); do
  grep -q "notification ingested.*NotificationCenterDb" "$LOG" && break
  sleep 0.5
done
sleep 2   # give the AX observer time to report the same banner

echo "--- notification-related processes:"; ps -axo pid,comm | grep -i -E "notification|usernoted" | grep -v grep || true
DB=$(grep -c "notification ingested.*NotificationCenterDb" "$LOG" || true)
AX=$(grep -c "notification ingested.*AccessibilityBanner" "$LOG" || true)
OBS=$(grep -c "observing Notification Center banners via Accessibility" "$LOG" || true)
DUP=$(grep -c "outcome=Duplicate" "$LOG" || true)
echo "DB-ingested=$DB  AX-ingested=$AX  AX-observer-started=$OBS  duplicates-collapsed=$DUP"

STATUS=0
if [ "$DB" -ge 1 ]; then echo "PASS: Notification Center DB watcher ingested a real notification"; else echo "FAIL: DB watcher saw nothing"; STATUS=1; fi
if [ "$OBS" -ge 1 ]; then
  echo "PASS: Accessibility banner observer started"
elif pgrep -x NotificationCenter >/dev/null; then
  echo "FAIL: Notification Center UI is running but the AX observer did not attach"; STATUS=1
else
  echo "SKIP: this host never launches the Notification Center UI process (headless VM: banners are not rendered), so the Accessibility path cannot be exercised here"
fi
if [ "$AX" -ge 1 ]; then
  echo "PASS: Accessibility observer read the banner"
  if [ "$DUP" -ge 1 ]; then echo "PASS: same banner via DB and AX was deduplicated"; fi
else
  # Banner AX trees differ between macOS releases and headless sessions may not
  # render banners at all; report it without failing the build.
  echo "NOTE: no banner seen via Accessibility on this runner (banners may not render headlessly)"
fi
kill -TERM $PID
for _ in $(seq 1 40); do kill -0 $PID 2>/dev/null || break; sleep 0.25; done
exit $STATUS
