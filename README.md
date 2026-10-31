# NotchForge

**The MacBook notch, forged into a smart, always-available control and notification surface.**

NotchForge is a native macOS app written in pure Rust. It turns the camera
housing into a Dynamic-Island-style surface. When idle it hugs the notch, when
you hover it opens a glanceable panel, and when you click it pins a full
control panel. It surfaces:

- **Coding-agent events, as first-class notifications.** Covers Claude (Code and
  desktop), ChatGPT and the Codex CLI, Cursor, Zed, OpenCode and Qoder, plus a
  generic webhook, socket and file inbox for any other tool.
- **Messaging notifications** from WhatsApp, Discord, Instagram, Facebook
  and Messenger, Messages, Slack, Telegram and Signal, read from Notification
  Center.
- **Weather**: current conditions, high and low, and the next 5 hours (Open-Meteo,
  free with no key).
- **Batteries**: your Mac's, plus AirPods (left, right and case),
  Magic Keyboard, Mouse, Trackpad and other HID accessories.
- **A temporary file tray.** Drag files onto the notch, then drag them back out
  to anywhere, AirDrop them, reveal them in Finder or copy their paths.
- **Quick controls**: brightness, volume, display resolution (with a safe
  automatic revert) and a Focus / Do Not Disturb toggle.

It runs as an accessory app with no Dock icon, and adds a menu bar extra as a
secondary entry point.

> **Status:** every push is built, linked, tested and run on an Apple Silicon
> macOS runner (`.github/workflows/ci.yml`):
>
> * **Smoke test** (`packaging/smoke_test.sh`). Launches the signed `.app`,
>   delivers agent events over HTTP, as a Claude Code hook payload and through
>   the inbox file, checks they are ingested, then sends SIGTERM and verifies a
>   clean shutdown (agent socket removed). About 42 MB resident after launch.
> * **Live providers** (`packaging/providers_test.sh`). Runs every provider
>   against real macOS and the network. A forecast is fetched from Open-Meteo
>   for a geocoded city, and the battery, accessory, agent, Notification
>   Center and Focus providers must all report `Running`.
> * **System notification paths** (`packaging/system_paths_test.sh`). Posts a
>   real macOS notification and verifies the Notification Center database
>   watcher ingests it. The Accessibility banner check is skipped on hosts that
>   never launch the Notification Center UI, which includes headless CI VMs.
> * **UI self-test** (`notchforge --self-test <dir>`, 53 checks). Drives the
>   running app with synthetic pointer, click and Escape events: hover-expand,
>   click-to-pin, file drop into the tray, Escape-to-collapse, a notification
>   peek, the Settings window and a screen re-layout. At each stage it asserts
>   the state, island geometry and click-through behaviour. It also measures
>   the springs (≈50–60 fps on a GPU-less VM) and renders the island's own layers to
>   check its pixels: an opaque black shape, content actually drawn, a square
>   top edge and rounded bottom corners. ASCII previews of each stage go to
>   the CI log.
>
> Run `notchforge --self-test /tmp/nf` on your own Mac to repeat it there.

---

## Table of contents

1. [How it behaves](#how-it-behaves)
2. [Architecture](#architecture)
3. [Build & run](#build--run)
4. [Permissions](#permissions)
5. [Connecting coding agents](#connecting-coding-agents)
6. [Messaging apps](#messaging-apps)
7. [Configuration](#configuration)
8. [Adding a new provider](#adding-a-new-provider)
9. [Performance](#performance)
10. [Testing](#testing)
11. [Fragile / private APIs](#fragile--private-apis)
12. [Known limitations](#known-limitations)
13. [Project layout](#project-layout)

---

## How it behaves

| State | Trigger | What you see |
|---|---|---|
| **Collapsed** | idle | The black island exactly covers the notch. With *wings* on, small live indicators sit either side: charging, low battery, number of tray files, unread count with the latest app's icon, and Focus. |
| **Peek** | a notification at or above `peek_min_priority` arrives | A short banner drops out of the notch for 4 s. A burst of notifications extends one peek; it doesn't flap. |
| **Hover** (compact) | the pointer rests near the notch for 120 ms | Three cards: weather, battery (plus your lowest accessory), and the latest notification. |
| **Expanded** (pinned) | click | Notification list with open, copy and dismiss actions; the file tray; weather with an hourly forecast; accessory batteries; brightness and volume sliders; a resolution picker; Focus, pause and settings buttons. |
| **Drop target** | dragging files toward the notch | The panel opens with the tray highlighted. Dropping pins it open. |

Press <kbd>Esc</kbd> or click anywhere outside to collapse.
<kbd>⌘</kbd><kbd>,</kbd> opens Settings while the panel is pinned.
On displays without a notch, NotchForge draws a slim pill of the same height
as the menu bar in the same spot.

Transitions use a spring with a slight bounce when opening and critical
damping when closing, so the island never overshoots below the notch. With
*Reduce motion* on, it snaps and crossfades.

---

## Architecture

The full design rationale is in [`PLAN.md`](PLAN.md).

### Stack: pure Rust + AppKit via `objc2`

| Option | Idle memory | Native look & a11y | Drag files *out* | Verdict |
|---|---|---|---|---|
| Tauri 2 + web UI | 80–150 MB (WebKit) | imitated | needs bridges | ✗ |
| winit + wgpu | 30–60 MB | hand-drawn, no VoiceOver | not supported | ✗ |
| **AppKit via `objc2`** | **~20–35 MB** | **real controls, SF Symbols, VoiceOver** | **`NSDraggingSource`** | **✓** |

The needed platform features are AppKit APIs anyway: a non-activating panel
above the menu bar, file drag and drop in both directions, AirDrop, status
items, `SMAppService`, CoreLocation and Accessibility. `objc2` gives typed
bindings to all of them with no web engine or GPU pipeline of our own.

### Data flow

```
 ┌───────────────────────── main thread (AppKit) ──────────────────────────┐
 │ NotchController ── NotchMachine (pure FSM) ── SpringRect (pure physics) │
 │   NotchPanel ▸ RootView ▸ island ▸ {Collapsed, Peek, Compact, Expanded} │
 │ MenuBar · SettingsWindow · global mouse monitor · AX banner observer    │
 └──────────────▲──────────────────────────────────────────┬───────────────┘
   Arc<Snapshot> via GCD (latest-only, coalesced)    Command (mpsc)
 ┌──────────────┴───────────── tokio (2 workers) ──────────▼───────────────┐
 │ Engine: sole owner of state · NotificationPipeline · Tray               │
 │          ▲ ProviderEvent (mpsc)                                         │
 │ Supervisor (restart + backoff, live config) ─┬─ battery   (IOKit)       │
 │                                              ├─ bluetooth (ioreg/sp)    │
 │                                              ├─ weather   (Open-Meteo)  │
 │                                              ├─ agents.http / .socket / │
 │                                              │  .inbox                  │
 │                                              ├─ notification_center (DB)│
 │                                              └─ focus     (DND state)   │
 └─────────────────────────────────────────────────────────────────────────┘
```

- **One owner of state.** Providers only *emit* events. The engine applies
  events and UI commands and publishes immutable snapshots. No mutable state
  is shared across threads under a lock.
- **The pure core is fully tested.** `notchforge-core` has no OS
  dependencies. Notch geometry, springs, the UI state machine, the
  notification pipeline, the tray, config and the engine are all unit-tested
  on any platform.
- **The UI is thin.** The macOS crate only translates input into state
  machine events and renders snapshots.

### Notification pipeline

Every notification passes through these stages in order:

1. **Filter**: pause, per-app enable, muted keywords, unknown apps, and a priority floor while Focus is on.
2. **Dedupe**: same app, title and body within 10 s. The DB watcher and the banner observer can see the same message; this keeps one.
3. **Group**: same app and thread within 5 minutes become one entry with a message count.
4. **Rate limit**: one token bucket per app. Overflow is *folded* into the latest entry, never dropped.
5. **Rank**: by priority, then recency, with capped storage.

The engine publishes at most one snapshot per 16 ms. The main-thread bridge
keeps only the newest snapshot, so a flood of 200 notifications causes a
handful of renders.

---

## Build & run

**Requirements:** an Apple Silicon Mac on macOS 13 Ventura or later, and Rust
1.85 or later (`rustup`). Xcode command line tools are needed only for
`codesign`.

```bash
git clone https://github.com/AmdjedBelgacem/Notchify && cd Notchify
```

```bash
cargo run --release
```

That's enough for development. Permissions get attributed to your terminal
app, and location and app-notification prompts need a bundle.

**Build the app bundle (recommended).** The script signs ad hoc by default.
See its header for Developer ID signing, notarization and universal builds.

```bash
packaging/bundle.sh
```

```bash
cp -R dist/NotchForge.app /Applications/ && open /Applications/NotchForge.app
```

To put the agent CLI on your `PATH`:

```bash
sudo ln -sf /Applications/NotchForge.app/Contents/MacOS/notchforge-notify /usr/local/bin/
```

**Headless mode** runs the engine and every provider without the UI and logs
what the notch would show. It works on any OS, which makes it handy for
provider development:

```bash
cargo run -- --headless -v
```

Then, from another terminal:

```bash
curl -s -X POST localhost:47815/v1/notify -d '{"source":"cursor","kind":"task_finished","title":"Done"}'
```

Other flags: `--config <path>`, `--print-config`, `--version`. Setting
`NOTCHFORGE_HOME=/some/dir` relocates config, data and logs, which is useful
for running test instances.

Files live in:

| What | Where |
|---|---|
| Config | `~/Library/Application Support/NotchForge/config.toml` |
| Agent socket / inbox | `~/Library/Application Support/NotchForge/agent.sock`, `inbox.jsonl` |
| Tray state (if `tray.persist`) | `~/Library/Application Support/NotchForge/tray.json` |
| Logs | `~/Library/Logs/NotchForge/notchforge.log` |

---

## Permissions

Every permission is **optional**. NotchForge explains each one in
**Settings › Permissions**, which shows its live status with *Request Access*
and *Open Settings* buttons.

| Permission | Why | Without it |
|---|---|---|
| **Full Disk Access** | Read the Notification Center database (messaging apps and desktop agents) and the Focus state | Agent webhooks still work. Turn on the Accessibility fallback for messaging. |
| **Accessibility** | Fallback banner reader; optional dismissal of the native banner | Not needed if Full Disk Access is granted |
| **Location** | "auto" weather location | Type a city in Settings, or set `latitude` and `longitude` |
| **Notifications** | NotchForge's own fallback alerts | Not needed |
| **Bluetooth** | *Not required.* Accessory batteries come from the IORegistry and `system_profiler`. | — |

To grant Full Disk Access manually: **System Settings › Privacy & Security ›
Full Disk Access**, then **+**, then choose `NotchForge.app` (or your
terminal, when using `cargo run`). Restart NotchForge afterwards.

> macOS ties permissions to the code signature. Ad-hoc-signed rebuilds look
> like a new app, so sign with a stable identity
> (`CODESIGN_IDENTITY=… packaging/bundle.sh`) to keep permissions across builds.

Global mouse monitoring (for hover and drag detection) does **not** require
Accessibility.

---

## Connecting coding agents

All transports accept the same JSON. Every field is optional and aliases are
accepted:

```json
{
  "source":   "claude | chatgpt | cursor | zed | opencode | qoder | agent | <any name>",
  "kind":     "message | task_finished | needs_input | error | tool_call | info",
  "title":    "Refactor complete",
  "body":     "12 files changed",
  "priority": "low | normal | high | critical",
  "thread":   "session-id (groups related events)",
  "url":      "https://… or /path/to/file (opened by the Open action)"
}
```

The notch picks a priority from `kind` when you don't send one: `needs_input`
and `error` are **high** and peek immediately. Batches are accepted as JSON arrays.

| Transport | Endpoint |
|---|---|
| HTTP (loopback only) | `POST http://127.0.0.1:47815/v1/notify[?source=…&kind=…]`; `GET /v1/health` |
| Unix socket (mode 0600) | `~/Library/Application Support/NotchForge/agent.sock`, one JSON object per line, replies `ok` |
| Inbox file | append JSON lines to `~/Library/Application Support/NotchForge/inbox.jsonl` |
| CLI | `notchforge-notify` tries HTTP, then the socket, then the inbox, so nothing is lost while NotchForge restarts |

Set `agents.token` to require `Authorization: Bearer <token>`. The CLI reads
`NOTCHFORGE_TOKEN`.

### Recipes

- **Claude Code.** Merge [`integrations/claude-code/settings.json`](integrations/claude-code/settings.json)
  into `~/.claude/settings.json`. Hook payloads are understood natively:
  permission prompts show as **Needs your input · project**, and turn ends
  show as **Finished · project**.
- **ChatGPT Codex CLI.** Add `notify = ["notchforge-notify", "--source", "chatgpt", "--best-effort"]`
  to `~/.codex/config.toml` ([details](integrations/codex/README.md)).
- **OpenCode.** Copy [`integrations/opencode/notchforge.js`](integrations/opencode/notchforge.js)
  to `~/.config/opencode/plugin/`.
- **Claude, ChatGPT, Cursor, Zed and Qoder desktop apps.** No setup. Their
  macOS notifications arrive through the Notification Center watcher and are
  attributed to the right agent by bundle id. Unknown builds can be mapped
  with `[[notifications.aliases]]`.
- **Cursor, Zed and VS Code tasks, CI, or any script.** Use
  `notchforge-notify --source zed --kind task_finished --title "Tests passed"`.
- **Any long-running command.** Source
  [`integrations/shell/notchforge.sh`](integrations/shell/notchforge.sh) and run
  `nfwrap --source aider aider …`. You're notified with the exit status and
  duration.

---

## Messaging apps

The **Notification Center watcher** reads every banner macOS delivers. It
needs Full Disk Access, opens the database read-only, and starts at the
newest record so it never replays your history. WhatsApp, Discord, Messages,
Slack, Telegram, Signal and Messenger are recognized by bundle id. Instagram
and Facebook web push from Safari, Chrome, Arc, Brave, Firefox and Edge are
recognized from the notification text.

Per-app switches, priority overrides and muted keywords are in Settings and in
`[notifications.apps.<key>]`.

**Avoiding double banners.** macOS doesn't let one app suppress another app's
banners. The reliable recipe is to set an app's alert style to **None** under
**System Settings › Notifications**. The notification is still recorded, so
it still reaches the notch, but macOS no longer shows its own banner. An
experimental Accessibility-based "close the native banner" option exists
(`suppress_native_banners`), and it's off by default.

---

## Configuration

See the fully commented [`config/notchforge.example.toml`](config/notchforge.example.toml),
or run `notchforge --print-config`. The Settings window writes the same file.
Hand edits are applied live within about 2 s. Invalid edits are logged and
ignored rather than crashing the app or resetting your config.

---

## Adding a new provider

A provider is an async task that emits `ProviderEvent`s. The supervisor
starts and stops it as the config changes, and restarts it with exponential
backoff (1 s up to 5 min) if it returns an error. Return
`ProviderError::NeedsPermission(..)` and the Settings window shows a
*Request Access* button for you.

1. Create `crates/notchforge-providers/src/github.rs`:

```rust
use notchforge_core::apps::{AppRef, KnownApp};
use notchforge_core::model::{NewNotification, NotificationKind, Origin, ProviderStatus};
use notchforge_core::provider::*;
use notchforge_core::time::now_ms;
use std::time::Duration;

#[derive(Default)]
pub struct GithubProvider;

impl Provider for GithubProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "github", name: "GitHub", permissions: &[] }
    }

    fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            ctx.set_status(ProviderStatus::Running).await;
            loop {
                // … poll an API, watch a file, listen on a socket …
                let n = NewNotification {
                    app: AppRef { known: Some(KnownApp::GenericAgent), bundle_id: None, display_name: "GitHub".into() },
                    origin: Origin::Internal,
                    kind: NotificationKind::TaskFinished,
                    priority: None, // let the pipeline decide from `kind` and config
                    title: "CI passed".into(),
                    subtitle: None,
                    body: "main · 3m 12s".into(),
                    timestamp_ms: now_ms(),
                    thread: Some("ci-main".into()),
                    url: Some("https://github.com/…".into()),
                };
                if !ctx.emit(ProviderEvent::Notification(n)).await {
                    return Ok(()); // engine gone: app is quitting
                }
                // Sleeps, but wakes early when the panel opens (refresh) or on shutdown.
                if ctx.sleep_or_refresh(Duration::from_secs(300)).await == Wake::Shutdown {
                    return Ok(());
                }
            }
        })
    }
}
```

2. Register it in `crates/notchforge-providers/src/lib.rs`:

```rust
pub mod github;
// in registry():
spec::<github::GithubProvider>(|c| c.providers.agents, |_| String::new()),
```

The second closure is a *restart key*: return something like a port number
when a config change needs a restart rather than a live re-read.

3. Add parsing tests next to it. Every built-in provider keeps its parsing
   logic in pure functions (`parse_pmset`, `parse_ioreg`, `parse_forecast`,
   `decode_record`, …) with fixture-based tests.

Providers that need the AppKit main thread, such as the Accessibility
observer, live in `notchforge-macos`. They send into the same event channel
(`Boot::provider_events`).

---

## Performance

| Target | How it's met |
|---|---|
| Near-zero CPU when idle | No timers run while collapsed. The animation timer exists only while a spring is moving. Providers sleep between polls: battery 30 s, accessories 60 s / 180 s, weather 20 min, DB `stat` 1 s, Focus 3 s. |
| Low memory | No web view and no GPU pipeline. Two tokio workers. Collections are capped (notifications at 100, tray at 50). Views are created once and reused; only the expanded panel's lists rebuild, and only while visible. |
| 60–120 fps | Each frame changes one layer frame and one corner radius, composited by the window server. Springs are integrated at a fixed 240 Hz sub-step, so motion is identical at 60 Hz and on 120 Hz ProMotion. |
| No jank under bursts | Folding, grouping and rate limiting happen before the UI. Snapshots are coalesced (≤ 1 per 16 ms) and the main thread keeps only the latest one. |
| Instant hover | Global mouse monitors and an always-active tracking area feed a pure state machine. The hover-intent delay is configurable and can be 0. |
| Clean shutdown | A shutdown signal is shared by every task. Providers remove their socket and the engine persists the tray. The binary waits for every task to finish (up to 1 s) before exiting. |

---

## Testing

```bash
cargo test --workspace
```

This runs 100+ tests on any OS.

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

This type-checks and lints the AppKit layer from any OS:

```bash
rustup target add aarch64-apple-darwin
```

```bash
cargo clippy --workspace --target aarch64-apple-darwin -- -D warnings
```

Coverage includes:

- notch geometry from real MacBook Pro 14″ screen metrics, including fallbacks and screen selection
- spring stability, overshoot bounds and frame-rate independence
- every state machine transition, including hover intent, grace periods, peeks, drag in and out, and pinning
- the pipeline's filter, dedupe, grouping, rate-limit folding and capacity eviction
- the engine's coalescing under a 200-event burst and low-battery alerts
- the provider supervisor's restart, backoff and enable-on-config behaviour
- the HTTP server end to end over TCP, the inbox tailer's truncation and partial lines, and the Claude Code and Codex payload mappings
- the `pmset`, `ioreg`, `system_profiler`, Open-Meteo, usernoted plist, schema probing and Focus parsers

---

## Fragile / private APIs

Each one is isolated in its own module, documented there, and fails soft: a
missing feature is hidden or reported, and the app never crashes because of
one.

| Feature | Technique | Module | If it breaks |
|---|---|---|---|
| Messaging notifications | Private usernoted SQLite schema | `providers/src/usernoted.rs` | Schema probe reports *Failing*; use the AX fallback |
| Banner fallback / dismiss | Undocumented Notification Center AX tree | `macos/src/accessibility.rs` | Heuristic text extraction degrades |
| Built-in brightness | `DisplayServices` private framework (`dlopen`) | `macos/src/private_api/display_services.rs` | Slider hidden |
| Focus state | `~/Library/DoNotDisturb/DB/*.json` | `providers/src/focus.rs` | State shows as unknown |
| Focus toggle | User-created Shortcuts via the `shortcuts` CLI | `macos/src/controls/focus.rs` | Guided setup alert |

The other APIs used are public: IOKit power sources, CoreGraphics display
modes, CoreAudio, CoreLocation, `SMAppService`, `NSSharingService` and
AppKit.

---

## Known limitations

- **Visual polish.** CI exercises every interaction and captures screenshots,
  but on a virtual 1024×768 display without a notch. Expect some tuning of
  spacing, sizes and hover feel on a physical notched MacBook.
- **The Accessibility banner fallback** is implemented but can't be
  exercised in CI: headless VMs never render banners. The primary
  Notification Center database path is verified end to end.
- **Apple Watch battery.** macOS has no public interface for it.
- **External display brightness.** This needs DDC/CI, which isn't
  implemented. Only the built-in panel is supported.
- **Night Shift.** Not implemented. It's only reachable through the private
  CoreBrightness framework.
- **Focus.** Schedule-based Focus without an assertion isn't detected.
  Toggling requires two Shortcuts (Settings explains how).
- **Instagram and Facebook** have no native macOS app. Only their browser
  web-push notifications are recognized.
- **Drag detection while collapsed** uses the drag pasteboard's change count.
  Dragging a non-file item past the notch never opens it, but a drag that
  starts very close to the notch may need a moment to register.
- **Unbundled `cargo run`.** Location and app notifications require the `.app`
  bundle, and privacy permissions are attributed to the terminal.
- **Apple Silicon is the primary target.** Intel builds via `UNIVERSAL=1` but
  isn't tested.

---

## Project layout

```
app/                       notchforge binary (thin): CLI, logging, runtime wiring, headless sink
crates/notchforge-core/    models, config, geometry, spring, state machine, pipeline, tray, provider trait, engine
crates/notchforge-providers/ battery, bluetooth, weather, agents/{http,socket,inbox,payload}, usernoted, focus, sys/ (IOKit FFI)
crates/notchforge-macos/   AppKit UI: app, bridge, controller, panel, views/, menubar, settings, permissions,
                           login_item, location, accessibility, sharing, icons, controls/, private_api/, ffi/
crates/notchforge-notify/  agent CLI (std-only networking; HTTP → socket → inbox fallback)
config/                    documented example configuration
integrations/              Claude Code hooks, Codex CLI, OpenCode plugin, shell helpers
packaging/                 Info.plist, entitlements, bundle/sign/notarize script
PLAN.md                    architecture & implementation plan
```

## License

MIT. See [LICENSE](LICENSE).
