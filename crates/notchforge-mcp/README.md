# notchforge-mcp

An [MCP](https://modelcontextprotocol.io) server (stdio, protocol `2025-06-18`, also `2025-03-26` / `2024-11-05`)
that lets any MCP-capable coding agent post to the **Notchisland** notch. It forwards tool calls to the app's
loopback webhook, `POST http://127.0.0.1:47815/v1/notify`.

## Tools

| Tool | Arguments | Posted event |
| --- | --- | --- |
| `notch_notify` | `title` (required), `body`, `kind` (`message`, `task_finished`, `needs_input`, `error`, `tool_call`, `info`; default `info`), `url` | `{source, kind, title, body?, url?}` |
| `notch_progress` | `task` (required), `done`, `total` (required numbers), `note` | `kind: "info"`, title `"<task> — done/total (pct%)"`, `body: note`, `thread: "mcp-progress:<task>"` |
| `notch_finished` | `summary` (required), `files_changed` (string[]), `tests` (`passed` / `failed` / `none`) | `kind: "task_finished"`, title = summary, body lists the changed files and the test outcome; failed tests raise `priority: "high"` |

`source` comes from the client's `clientInfo.name`: names containing `claude` → `claude`, `codex` → `chatgpt`,
`cursor` → `cursor`, `gemini` → `gemini`, anything else → `mcp`.

If the app isn't running, the tool result has `isError: true` and the text `Notchisland is not running`.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `NOTCHFORGE_PORT` | `47815` | Webhook port |
| `NOTCHFORGE_TOKEN` | none | Bearer token, if you turned one on for the webhook |

## Registering it

The binary ships inside the app bundle at `/Applications/Notchisland.app/Contents/MacOS/notchforge-mcp`.

**Claude Code**

```sh
claude mcp add notchisland -- /Applications/Notchisland.app/Contents/MacOS/notchforge-mcp
# with a token:
claude mcp add notchisland -e NOTCHFORGE_TOKEN=… -- /Applications/Notchisland.app/Contents/MacOS/notchforge-mcp
```

**Codex CLI**: `~/.codex/config.toml`

```toml
[mcp_servers.notchisland]
command = "/Applications/Notchisland.app/Contents/MacOS/notchforge-mcp"
# env = { NOTCHFORGE_TOKEN = "…" }
```

**Cursor**: `~/.cursor/mcp.json`

```json
{
  "mcpServers": {
    "notchisland": {
      "command": "/Applications/Notchisland.app/Contents/MacOS/notchforge-mcp"
    }
  }
}
```

**Gemini CLI**: `~/.gemini/settings.json`

```json
{
  "mcpServers": {
    "notchisland": {
      "command": "/Applications/Notchisland.app/Contents/MacOS/notchforge-mcp"
    }
  }
}
```

Any other MCP client that can launch a stdio server works the same way: point it at the binary.

## Trying it by hand

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude-code","version":"1"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"notch_notify","arguments":{"title":"Hello from MCP"}}}' \
  | notchforge-mcp
```
