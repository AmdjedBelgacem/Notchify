# NotchForge — Architecture & Implementation Plan

> Phase 1 deliverable. This plan is what the code in this repository implements.
> Where the implementation deviates, the code comments explain why.

## 1. Name

**NotchForge.** Short, describes the job (forging the dead notch area into a
working surface), and doesn't collide with any existing notch app.

## 2. Tech stack decision

| Option | Idle RSS | Native look | Drag-out to Finder | VoiceOver | Verdict |
|---|---|---|---|---|---|
| Tauri 2 + web frontend | 80–150 MB (WKWebView + WebContent process) | Imitated with CSS | Needs plugins / JS bridges | Web semantics only | ✗ misses the memory target |
| winit + wgpu + custom UI | 30–60 MB (Metal device and pipelines) | Hand-built everything | winit cannot start an outbound file drag | None without AccessKit work | ✗ high effort, weak a11y |
| **Pure Rust + AppKit through `objc2`** | **~20–35 MB** | **Real NSVisualEffectView, SF Symbols, system fonts** | **Native `NSDraggingSource`** | **Native controls are accessible by default** | **✓ chosen** |

**Decision: pure Rust, talking to AppKit directly through the `objc2` crate family**
(`objc2`, `objc2-foundation`, `objc2-app-kit`, `objc2-core-location`,
`objc2-service-management`, `objc2-user-notifications`, `block2`,
`dispatch2`), plus small hand-written FFI for IOKit, CoreGraphics display
modes, CoreAudio, Accessibility (AX) and one private framework (DisplayServices).

Why:
* **Memory and CPU.** There's no web engine or GPU pipeline of our own. Core
  Animation composites layer-backed views in the window server. When the
  notch is collapsed and idle, the process sleeps in `CFRunLoop` with no
  timers running.
* **Animations.** A critically tuned spring (written in Rust and unit-tested)
  runs from a 120 Hz `NSTimer`. The timer exists *only while a spring is
  moving*. Each tick changes one layer frame, which is cheap and stays smooth
  at 60–120 fps.
* **Platform features we need anyway.** We need file drag-in and drag-out,
  AirDrop (`NSSharingService`), the status item (menu bar extra),
  `SMAppService` login items, CoreLocation and Accessibility observers. These
  are AppKit APIs, and every other stack would reach them through bridges.
* **The type checker covers the AppKit code.** `objc2` has typed bindings,
  so `cargo check --target aarch64-apple-darwin` type-checks the whole UI
  layer, even on CI machines that aren't Macs.

Other choices:
* **tokio** (multi-thread runtime, 2 workers) runs providers and the engine.
  AppKit owns the main thread.
* **serde + toml** for configuration, **serde_json** for agent payloads and
  `system_profiler` output, **plist** for `ioreg -a` output and the
  Notification Center record blobs.
* **rusqlite** links the system `libsqlite3` on macOS and opens the
  usernoted DB read-only.
* **reqwest** (native-tls, so Security.framework on macOS) for Open-Meteo,
  which is free and needs no API key.
* **tracing** + `tracing-subscriber` write to stderr and to a rolling log file.
* **thiserror** in libraries and **anyhow** only in the binary. Production
  paths never call `unwrap()`.

## 3. High-level architecture

```
 ┌────────────────────────────── main thread (AppKit) ───────────────────────────────┐
 │  NSApplication (Accessory policy, LSUIElement)                                     │
 │   ├─ NotchController ── NotchMachine (core FSM) ── SpringRect animator (core)      │
 │   │     └─ NotchPanel (NSPanel, non-activating, above menu bar)                    │
 │   │          └─ NotchRootView (drag destination, tracking areas, shape layer)      │
 │   │               ├─ CollapsedView   (wings: battery / unread / activity)          │
 │   │               ├─ CompactView     (hover: weather · battery · latest notif)     │
 │   │               └─ ExpandedView    (full: notifs | tray | status | controls)     │
 │   ├─ StatusItem (menu bar extra)        ├─ SettingsWindow                           │
 │   ├─ GlobalMouseMonitor (hover/drag)    ├─ AxBannerObserver (fallback provider)     │
 │   └─ LocationService (CoreLocation)     └─ System controls (brightness/res/vol/DND)│
 └───────────────▲───────────────────────────────────────────────┬────────────────────┘
       Arc<Snapshot> via GCD main queue                Command (mpsc, Send)
 ┌───────────────┴──────────────── tokio runtime ────────────────▼────────────────────┐
 │  Engine: AppState + NotificationPipeline (filter→dedupe→rate-limit→group→rank)     │
 │          Tray, coalesced snapshot publishing (≤ 1 per 16 ms)                       │
 │                     ▲ ProviderEvent (mpsc)                                         │
 │  ProviderSupervisor ┼─ BatteryProvider (IOKit IOPowerSources)                     │
 │   (restart w/       ├─ BluetoothProvider (ioreg -a + system_profiler -json)        │
 │    backoff,         ├─ WeatherProvider (Open-Meteo, CoreLocation or fixed coords)  │
 │    status reports)  ├─ AgentHub: HTTP webhook · Unix socket · JSONL inbox          │
 │                     ├─ UsernotedDbProvider (Full Disk Access, read-only SQLite)    │
 │                     └─ FocusProvider (DND assertions file, fragile)                │
 └────────────────────────────────────────────────────────────────────────────────────┘
```

Data only flows one way. Providers emit `ProviderEvent`s. The engine is the
**only** owner of mutable app state. The UI receives immutable
`Arc<Snapshot>`s and sends back `Command`s. Nothing is shared under a lock
across threads.

## 4. Workspace layout

```
Cargo.toml                    workspace
crates/
  notchforge-core/            platform-agnostic, 100% unit-testable on any OS
    model.rs                  Notification, Battery, Device, Weather, Tray, Display…
    apps.rs                   KnownApp registry (bundle ids → category/icon/name)
    config.rs                 TOML config, defaults, validation, load/save
    geometry.rs               notch detection math, layout rects, hit zones
    spring.rs                 damped spring + SpringRect
    machine.rs                UI state machine (Collapsed/Peek/Hover/Expanded)
    pipeline.rs               filter, dedupe, rate limit, grouping, ranking
    tray.rs                   temporary file tray (+ optional persistence)
    provider.rs               Provider trait, context, supervisor, BoxFuture
    engine.rs                 state owner, command handling, snapshot coalescing
    time.rs                   relative-time formatting, Mac absolute time
    error.rs                  CoreError
  notchforge-providers/
    battery.rs                IOKit (macOS) / pmset parser / sysfs (Linux dev)
    util.rs                   command runner with timeout, small parsers
    bluetooth.rs              ioreg plist + system_profiler JSON parsers, merger
    weather.rs                Open-Meteo client, WMO code mapping
    agents/{payload,http,socket,inbox,mod}.rs   coding-agent hub
    usernoted.rs              Notification Center DB watcher (FDA)
    focus.rs                  DND/Focus state reader (fragile, isolated)
    sys/{cf,iokit}.rs         raw CoreFoundation / IOKit FFI (macOS only)
  notchforge-macos/           everything AppKit; compiles to nothing off macOS
    app.rs (bootstrap, main-thread state), bridge.rs (snapshots → main queue),
    controller.rs (FSM + springs + timers + input), panel.rs (NSPanel, RootView,
    TrayItemView), screen.rs, ui.rs (closure targets, helpers),
    views/{collapsed,peek,compact,expanded}.rs,
    menubar.rs, settings.rs, permissions.rs, login_item.rs, location.rs,
    accessibility.rs, sharing.rs, icons.rs,
    controls/{brightness,display_modes,volume,focus}.rs,
    private_api/display_services.rs   (dlopen'd, fragile, isolated)
    ffi/{ax,cg,coreaudio}.rs          (CoreFoundation helpers shared from providers::sys::cf)
  notchforge-notify/          tiny CLI that agents call: `notchforge-notify …`
app/                          `notchforge` binary — thin main (cli, logging, headless sink)
packaging/                    Info.plist, entitlements, bundle script
integrations/                 Claude Code hooks, OpenCode plugin, shell helpers
config/notchforge.example.toml
```

## 5. Data models (core)

* `Notification { id, app: AppRef, category, kind, priority, title, subtitle, body, timestamp_ms, thread, url, group_count }`
* `AppRef { known: Option<KnownApp>, bundle_id, display_name }`. `KnownApp`
  covers Claude, ChatGPT, Cursor, Zed, OpenCode, Qoder, GenericAgent,
  WhatsApp, Discord, Instagram, Facebook, Messages, Slack, Telegram and
  Signal.
* `NotificationKind`: Message, TaskFinished, NeedsInput, Error, ToolCall, Info.
* `Priority`: Low < Normal < High < Critical.
* `BatteryStatus { percent, charging, plugged_in, minutes_remaining, cycle_count? }`
* `DeviceBattery { id, name, kind, levels: Vec<(BatteryComponent, u8)> }`
* `Weather { place, temperature, feels_like, condition, high, low, hourly[6], units, fetched_at }`
* `TrayItem { id, path, name, is_dir, bytes, added_at_ms }`
* `ControlsState { brightness, volume, muted, focus_active, displays: Vec<DisplayInfo> }`
* `Snapshot`: everything above plus provider statuses, unread count, the
  "activity" headline and the paused flag. It's built once per coalesced
  change and shared as an `Arc`.

## 6. Provider design

```rust
pub trait Provider: Send + 'static {
    fn descriptor(&self) -> ProviderDescriptor;          // id, name, permission needs
    fn run(self: Box<Self>, ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>>;
}
pub struct ProviderContext {
    pub events: mpsc::Sender<ProviderEvent>,   // Notification | Battery | Devices | Weather | Focus | Status
    pub config: watch::Receiver<Arc<Config>>,  // live config
    pub refresh: Arc<Notify>,                   // "panel opened, refresh now"
    pub location: watch::Receiver<Option<Coordinates>>,
    pub shutdown: Shutdown,                     // cooperative cancellation
}
```

The `ProviderSupervisor` spawns every enabled provider. A provider that
errors is restarted with exponential backoff (1 s → 5 min) and gets
`ProviderStatus::Failing(reason)`. A provider that needs a permission it
doesn't have reports `ProviderStatus::NeedsPermission(Permission)`, and the
Settings window shows a "Request access" button. Adding a provider takes
one struct, one `impl Provider` and one line in `providers::all()`.

## 7. UI state machine (`machine.rs`)

```
            hover-intent(120ms)                 click
Collapsed ───────────────────▶ Hover ───────────────────────▶ Expanded (pinned)
   ▲  ▲  │ notification            │ leave + 350ms grace          │ Esc / click outside /
   │  │  ▼                         ▼                              │ menu "collapse"
   │  └─ Peek (4s, banner) ◀── notification while Collapsed       ▼
   │        └─ hover → Hover; click → Expanded               Collapsed
   └─ drag-enter (files) ─▶ Expanded(drop target) ─ drag-exit + 600ms ─▶ previous
```

The machine is pure. `handle(Event, now) -> Vec<Effect>` returns effects
like `SetLayout(LayoutKind)`, `Schedule(TimerKind, Duration)`,
`CancelTimer`, `SetInteractive(bool)` and `MakeKey(bool)`. Timers come back
as `Event::TimerFired(kind)`. That means every transition is unit-tested
without AppKit.

## 8. Rendering and animation

The panel window is created once at the **maximum** (expanded) size and
anchored to the top-center of the target screen. It's transparent and
**ignores mouse events while collapsed**, so it never steals menu-bar
clicks. A global `NSEvent` monitor (mouse-moved and left-mouse-dragged)
does hit testing against the activation zone computed in `geometry.rs`.
Global mouse monitors don't need Accessibility permission.

The visible black "island" is a layer-backed container view. Its frame,
corner radius and content alpha are driven by `SpringRect`, four springs
stepped with semi-implicit Euler at a fixed 1/240 s sub-step. The animator
stops its timer the moment every spring settles. With Reduce Motion on, the
springs snap and content crossfades.

## 9. Notification pipeline

1. **Normalize.** Any source becomes a `NewNotification`.
2. **Filter.** Per-app rule (`enabled`, `priority`, `muted_keywords`),
   global pause, and a "Focus active ⇒ only High+" option.
3. **Dedupe.** Hash of (app, title, body) inside a sliding window (default
   10 s). The usernoted DB and the AX banner observer can both see the same
   message, and this collapses them.
4. **Rate limit.** One token bucket per app (burst 5, refill 1 per 3 s).
   Anything over the limit is *folded into a group* instead of dropped.
5. **Group.** Same app plus same thread within 5 min makes a single entry
   with `group_count`.
6. **Rank.** Priority descending, then recency. The store is capped
   (default 100).

The engine only *peeks* (expands briefly) for priority ≥ the configured
threshold, with at most one peek per 2 s. Bursts never make the UI flap.

## 10. Permission strategy

| Permission | Used for | Check | Request |
|---|---|---|---|
| Accessibility | AX banner fallback, optional native banner dismiss | `AXIsProcessTrusted` | `AXIsProcessTrustedWithOptions(prompt)` + deep link |
| Full Disk Access | usernoted DB | try `open()` on DB | deep link `Privacy_AllFiles` |
| Location | weather "auto" location | `CLLocationManager.authorizationStatus` | `requestWhenInUseAuthorization` |
| Notifications | NotchForge's own fallback alerts | `UNUserNotificationCenter.getNotificationSettings` | `requestAuthorization` |
| Bluetooth | *not required* (we read IORegistry/system_profiler) | n/a | n/a (key kept in Info.plist) |

Each permission is optional and every feature degrades gracefully. Settings
explains *why* each one is needed next to the button.

## 11. Implementation order (runnable early)

1. Core: model, config, geometry, spring, machine, pipeline, tray, engine (+ tests).
2. Providers: agents hub (HTTP, socket, inbox) → battery → bluetooth → weather → usernoted → focus.
3. `notchforge` binary with a `--headless` mode. You can run it on any OS
   and `curl` agent events into the log.
4. macOS: panel + root view + animator + controller (the collapsed/hover/expanded shell).
5. Compact/expanded content views bound to snapshots.
6. Drag in/out tray and sharing actions.
7. System controls (brightness, display modes, volume, Focus).
8. Status item, settings window, permissions, login item, location, AX observer.
9. `notchforge-notify` CLI, integrations, packaging, README.

## 12. Risks and mitigations

| Risk | Mitigation |
|---|---|
| usernoted DB schema/path changes between macOS versions | Path probing for both known locations; schema probe (`PRAGMA table_info`) before querying; provider reports `Failing` instead of crashing; AX fallback |
| Built-in brightness has no public API on Apple Silicon | `DisplayServices` private framework, `dlopen`ed at runtime in `private_api/`; missing symbols ⇒ slider hidden |
| Focus/DND has no public API | Toggle through user-configured Shortcuts (`shortcuts run`), state read from `~/Library/DoNotDisturb/DB/Assertions.json` (FDA); isolated in `controls/focus.rs` + `providers/focus.rs` |
| Suppressing other apps' banners isn't officially possible | Documented recipe (alert style "None" keeps the DB record); optional AX "Close" action on banners, off by default |
| Notch geometry API only on macOS 12+ | `safeAreaInsets`/`auxiliaryTopLeftArea` with a synthetic pill fallback for non-notched screens |
| Can't run AppKit on Linux CI | All logic lives in `notchforge-core` with tests; the UI is type-checked via `cargo check --target aarch64-apple-darwin` |
| Main-thread stalls | Providers never touch AppKit; snapshots are coalesced; `system_profiler` runs in `spawn_blocking` |
