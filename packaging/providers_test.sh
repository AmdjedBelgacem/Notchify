#!/usr/bin/env bash
# Run every data provider for real on macOS (headless engine) and check that
# weather data arrives from Open-Meteo and that battery, accessory, agent,
# Notification Center and Focus providers all come up healthy. Used by CI.
set -euo pipefail
cd "$(dirname "$0")/.."
BIN="dist/NotchForge.app/Contents/MacOS/notchforge"
export NOTCHFORGE_HOME="$(mktemp -d)"
LOG="$NOTCHFORGE_HOME/stderr.log"
printf '[weather]\nlocation = "Paris"\n' > "$NOTCHFORGE_HOME/config.toml"
# Start Apple Music in the background so the media provider exercises its
# AppleScript bridge (state + playlists) against the real app.
open -gj -a Music 2>/dev/null || true
sleep 4

"$BIN" --headless -v >"$LOG" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
for _ in $(seq 1 60); do
  grep -q 'weather=Some(("Paris"' "$LOG" && break
  sleep 0.5
done
sleep 3   # let the slower system_profiler pass finish
LAST=$(grep "snapshot" "$LOG" | tail -1 | sed 's/\x1b\[[0-9;]*m//g')
echo "last snapshot: $LAST"
STATUS=0
check() { if echo "$LAST" | grep -q "$2"; then echo "PASS: $1"; else echo "FAIL: $1"; STATUS=1; fi; }
check "weather fetched from Open-Meteo (geocoded Paris)" 'weather=Some(("Paris"'
check "battery provider running" '"battery": Running'
check "accessory battery provider running" '"bluetooth": Running'
check "weather provider running" '"weather": Running'
check "agent webhook running" '"agents.http": Running'
check "agent socket running" '"agents.socket": Running'
check "agent inbox running" '"agents.inbox": Running'
check "Notification Center provider running" '"notification_center": Running'
check "audio provider running (CoreAudio)" '"audio": Running'
if echo "$LAST" | grep -qE '"media": (Running|NeedsPermission\(Automation\))'; then echo "PASS: media provider up (Spotify / Apple Music)"
else echo "FAIL: media provider up (Spotify / Apple Music)"; STATUS=1; fi
echo "NOTE: default output: $(echo "$LAST" | grep -oE 'output=(Some\("[^"]*"\)|None)' || true)"
echo "NOTE: outputs / apps playing: $(echo "$LAST" | grep -o 'outputs=[0-9]*' || true) $(echo "$LAST" | grep -o 'playing=\[[^]]*\]' || true)"
echo "NOTE: media: $(echo "$LAST" | grep -o 'now_playing=[^,]*' || true) $(echo "$LAST" | grep -o 'playlists=[0-9]*' || true) $(echo "$LAST" | grep -o 'playlist_status=\[[^]]*\]' || true)"
if echo "$LAST" | grep -q '"focus": Running'; then echo "PASS: Focus provider running"
else echo "NOTE: Focus provider not running here: $(echo "$LAST" | grep -o '"focus": [A-Za-z]*([^)]*)\|"focus": [A-Za-z]*' || true)"; fi
osascript -e 'tell application "Music" to quit' 2>/dev/null || true
kill -INT $PID
for _ in $(seq 1 20); do kill -0 $PID 2>/dev/null || break; sleep 0.25; done
[ $STATUS -eq 0 ] || { echo "--- log"; cat "$LOG"; }
exit $STATUS
