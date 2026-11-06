# Menu bar never covered + multi-screen islands with a floating iPhone-style notch

## Context

Two asks: **(1)** on the MacBook's notched screen the collapsed island covers menu bar items (screenshot) — root cause: the app lost Accessibility at the re-sign, and the wing-dodge detection *silently treats "can't check" as "clear"*, so the wings render over items; **(2)** multi-screen support — switch the island between displays, or have it on several/all screens, and on notchless external screens render an iPhone-Pro-style **floating** notch: a rounded capsule detached from the top edge, placed smartly, animated smoothly.

Decisions (recommended defaults, user did not override): fail-safe wings; display-only mirrors on other screens; smart pill placement that dodges menu items.

## 1 — Fail-safe wings (the MacBook fix)

In `controller.rs wing_space_free()` (controller.rs:499-513) + `accessibility.rs`:

- New gate: wings render only when **Accessibility is trusted AND both spans are verifiably clear**. `AXIsProcessTrusted() == 0` ⇒ wing mask 0 (island = exactly the physical notch, which covers nothing beyond the hardware). No more "treat as free".
- Settings › Permissions already surfaces Accessibility; the wing logic logs once (`tracing::info!`) when wings are suppressed for lack of Accessibility, so the cause is visible.
- Keep the 2 s occupancy re-check: with Accessibility granted, wings reappear the moment the spans are actually free.

## 2 — Floating iPhone-style notch on notchless screens

Geometry (`crates/notchforge-core/src/geometry.rs`):

- `Notch` gains `floating: bool`. `detect_notch`'s synthetic branch produces a floating capsule: width 204, height 34, radius 17, top gap 6 (constants; the rect's top = `frame.max_y() − gap − height`). Physical notches keep `floating: false`.
- `NotchLayout::shape()`: `Collapsed`/`CollapsedWithWings` on a floating screen use the capsule rect; open layouts (Peek/Compact/Expanded/Incoming) keep their sizes but the notch band is the pill's height (works — views are island-local, confirmed by exploration).
- Top-pinning: `island_in_window()` subtracts the gap when floating (open states keep the same gap — the whole island floats as a rounded card, iPhone-style). `window_frame()` stays top-flush (the window still covers the strip). Zones: `activation_zone`/`retention_zone`/`drag_zone` top outset becomes `gap` for floating so hover works down from the menu bar.
- **Smart placement**: `NotchLayout` gains `x_offset: f64`. `island_global()` adds it; zones/mouse math inherit. The controller computes it (below) and spring-animates it (`Spring` in `tick` alongside `SpringRect`, stepped in `apply_frame` via `layout.borrow().x_offset`).

Menu-bar occupancy with rects:

- `accessibility.rs`: `menu_bar_items_intersect` → `menu_bar_item_rects(pid, band, primary_h) -> Vec<Rect>` (the AX walk already reads AXPosition/AXSize — drop the early return); `app_menus_intersect`/`status_items_intersect` become rect-returning. `windows.rs`: `status_items_in(windows, band) -> Vec<Rect>`.
- New pure helper `sites`-style: `free_spans(band_width, occupied: &[Rect]) -> Vec<(f64, f64)>` (merge intervals, return gaps) — unit-tested. Placement: pick the gap ≥ pill width + 24 margin closest to screen center; fallback: centered. Computed in the controller on the existing refresh triggers (snapshot, app activation, space/screen change, 2 s wing timer) and only for floating screens.

Controller (`controller.rs`):

- `apply_frame`: when floating — no overhang (island frame exactly w×h, full 4-corner radius; the layer mask already rounds it), `place_ears` skipped (ears layers hidden), `glass.apply(w, h, 0.0, radius, t)`.
- `settle_window`: ear padding only for physical notches (floating keeps 2 pt).
- `passes_through` band math: use the island's own top band (unchanged formula, works with the gap since it's island-relative to `island.y`).
- Floating vs physical rides on `NotchLayout`, so `relayout` (screen params), `apply_config`, and the existing equality early-out all keep working. `update_visibility`/fullscreen-hide unchanged (per-screen for mirrors below).

Glass (`glass.rs`): pass `overhang = 0` for floating; `set_notch_height(pill height)` keeps the black band behind the capsule row; the layer corner radius rounds everything (confirmed by exploration). Ears simply never placed on floating screens.

## 3 — Islands on other displays (display-only mirrors)

- New `crates/notchforge-macos/src/mirrors.rs`: for every screen **except the primary**, a lightweight panel built from the same recipe (`NotchPanel::create` pattern, panel.rs:81-110) — same level, `CanJoinAllSpaces | FullScreenAuxiliary`, **always `ignoresMouseEvents`** — containing a minimal view class (own `define_class`, **no `with_controller` routing**) that draws: physical screen → black notch-shape with ears; floating screen → the capsule with ears suppressed; wings content rendered by reusing `CollapsedView::update` with the shared snapshot; peek banners skipped (primary-only); call indicator (phone + count) comes along via the collapsed view.
- Lifecycle: rebuilt on `DidChangeScreenParametersNotification` (add/remove as displays come and go) and on config change; per-poll `update(snapshot)` from `Controller::apply_snapshot` via a `Mirrors` field on `AppState`/`Controller`.
- Per-mirror fullscreen-hide: reuse `windows::fullscreen_covers(windows, &that_screen)` per mirror screen in the existing watcher cycle; order out/in like the primary.
- Config: `[general] island_on_all_screens: bool = false` + Settings › General switch "Show the island on every screen". The primary follows `general.screen` (notch/main/mouse) exactly as today — switching displays = pick Mouse to follow the pointer, or any fixed choice.

## 4 — Self-test & docs

- `check_settled`: floating-aware — pin assertion becomes `island.max_y() == frame.max_y() − gap` and window-width assert drops the ear margin for floating; hover fixture y from the (new) activation zone; pass-through step clicks just outside the rounded corner within the window (ear margin may be 2 pt for floating → click inside window but off-island as today, works since ears exist on the notched VM primary).
- `notched_previews`: add a floating variant (all four corners rounded, capsule) rendered beside the attached ones.
- Config example + README: fail-safe wings note, "Show the island on every screen", floating notch description with the smart-dodge behavior.

## 5 — Verification

1. `cargo test --workspace` (new: free-span math, floating shape/zone tests, wing fail-safe unit tests in the controller where testable) + clippy `-D warnings` + self-test 96/96 with floating previews.
2. Live on this Mac: notched built-in screen — with Accessibility granted, wings dodge; after toggling Accessibility off in Settings the wings disappear (fail-safe provable). External/notchless: this Mac has an external display — verify the floating capsule renders detached below the top edge, glides away from status items, opens into panels, and that "Show the island on every screen" puts a mirror on the other display (and hides it there when that screen is fullscreen).
3. `RELEASE=1 INSTALL=1 packaging/bundle.sh`.

## Out of scope

Fully interactive islands on secondary screens (display-only mirrors); per-screen custom pill sizes; Android-style floating placement anywhere on screen (pill stays in the top strip).