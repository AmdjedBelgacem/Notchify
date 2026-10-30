# ChatGPT Codex CLI → NotchForge

Codex runs a `notify` program after each agent turn, passing a JSON object as
the last argument. `notchforge-notify` accepts that directly. Add this to
`~/.codex/config.toml`:

```toml
notify = ["notchforge-notify", "--source", "chatgpt", "--best-effort"]
```

The notch then shows **Finished** with the last assistant message as the body.
The ChatGPT desktop app needs no setup: its notifications come in through the
Notification Center watcher.
