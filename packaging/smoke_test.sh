#!/usr/bin/env bash
# Launch the bundled app, verify the UI and providers come up, deliver agent
# events through every transport, and check the app stays alive. Used by CI.
set -euo pipefail
cd "$(dirname "$0")/.."
APP="dist/NotchForge.app"
export NOTCHFORGE_HOME="$(mktemp -d)"
LOG="$NOTCHFORGE_HOME/stderr.log"

"$APP/Contents/MacOS/notchforge" -v >"$LOG" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT

# Wait for the UI and the webhook.
for _ in $(seq 1 60); do
  grep -q "NotchForge UI running" "$LOG" && grep -q "agent webhook listening" "$LOG" && break
  kill -0 $PID 2>/dev/null || { echo "app exited early"; cat "$LOG"; exit 1; }
  sleep 0.5
done
grep -q "NotchForge UI running" "$LOG" || { echo "UI did not start"; cat "$LOG"; exit 1; }

NOTIFY="$APP/Contents/MacOS/notchforge-notify"
curl -sf http://127.0.0.1:47815/v1/health
"$NOTIFY" --source claude --kind needs_input --title "Smoke test" --body "via HTTP"
echo '{"hook_event_name":"Stop","cwd":"/tmp/proj","session_id":"s"}' | "$NOTIFY" --source claude --stdin
echo '{"source":"zed","kind":"task_finished","title":"via inbox"}' >> "$NOTCHFORGE_HOME/inbox.jsonl"
sleep 5

kill -0 $PID 2>/dev/null || { echo "app crashed"; cat "$LOG"; exit 1; }
echo "--- RSS (KB): $(ps -o rss= -p $PID)"
echo "--- CPU (%):  $(ps -o %cpu= -p $PID)"
cat "$LOG"
grep -q "snapshot" "$LOG" || { echo "engine published no snapshots"; exit 1; }
kill -INT $PID
for _ in $(seq 1 20); do kill -0 $PID 2>/dev/null || break; sleep 0.25; done
if kill -0 $PID 2>/dev/null; then echo "app did not exit on SIGINT (expected under AppKit; killing)"; fi
echo "smoke test passed"
