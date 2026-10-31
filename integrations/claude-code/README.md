# Claude Code → NotchForge

Merge `settings.json` into `~/.claude/settings.json` (all projects) or
`.claude/settings.json` (one project). Claude Code pipes each hook's JSON
input to `notchforge-notify --stdin`, and NotchForge turns it into:

| Hook | Notch shows |
|---|---|
| `Notification` (permission prompts, idle input) | **Needs your input · project** (high priority, peeks) |
| `Stop` | **Finished · project** |
| `SubagentStop` | **Finished · project** |

Without the CLI on your `PATH`, use curl instead:

```json
{ "type": "command", "command": "curl -s -m 2 -X POST 'http://127.0.0.1:47815/v1/notify?source=claude' -H 'Content-Type: application/json' --data-binary @- || true" }
```
