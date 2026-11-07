//! Menu bar extra, built to the *Menu bar extra* board of the redesign twin:
//! what's running, a mini player, four quick toggles as tiles, then timers, a
//! Sound Output submenu that lists each device with its capabilities, Settings
//! and Quit. It is also the fallback entry point
//! when the notch is hidden (no notch, full-screen apps).

use crate::app::{self, with_controller};
use crate::ui::{
    ActionTarget, FlippedView, Targets, Weight, attributed_parts, font, label, ns, rect, rgba, set_accessibility_label,
    set_button_symbol, set_text, shape_view, symbol_button,
};
use notchforge_core::engine::Snapshot;
use notchforge_core::model::MediaAction;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSButton, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSImage, NSImageScaling, NSImageView, NSMenu,
    NSMenuItem, NSStatusBar, NSStatusItem, NSTextField, NSVariableStatusItemLength,
};
use std::cell::{Cell, RefCell};

pub struct MenuBar {
    mtm: MainThreadMarker,
    item: Retained<NSStatusItem>,
    player_item: Retained<NSMenuItem>,
    player_sep: Retained<NSMenuItem>,
    art: Retained<NSImageView>,
    title: Retained<NSTextField>,
    subtitle: Retained<NSTextField>,
    play: Retained<NSButton>,
    header: Retained<NSMenuItem>,
    /// Focus, Pause, Calls and Tray: (disc, subtitle) of each tile.
    toggles: Vec<(Retained<FlippedView>, Retained<NSTextField>)>,
    toggles_item: Retained<NSMenuItem>,
    output: Retained<NSMenu>,
    output_key: RefCell<String>,
    unread: Cell<bool>,
    _targets: Targets,
    output_targets: Targets,
}

fn menu_item(
    mtm: MainThreadMarker,
    targets: &Targets,
    title: &str,
    key: &str,
    f: impl Fn() + 'static,
) -> Retained<NSMenuItem> {
    let t = targets.add(mtm, move |_| f());
    // SAFETY: valid selector; target kept alive by `targets`.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &ns(title),
            Some(ActionTarget::action()),
            &ns(key),
        )
    };
    // SAFETY: target kept alive by `targets`.
    unsafe { item.setTarget(Some(t.as_any())) };
    item
}

/// The status-bar glyph: a bar with a notch, plus an orange dot when unread.
fn notch_glyph(unread: bool) -> Retained<NSImage> {
    use objc2_app_kit::NSBezierPath;
    // The app's logo, with the orange dot when something is unread.
    if let Some(logo) = crate::icons::app_logo() {
        let size = objc2_foundation::NSSize::new(19.0, 19.0);
        let block = block2::RcBlock::new(move |_r: objc2_foundation::NSRect| -> objc2::runtime::Bool {
            logo.drawInRect(rect(0.0, 0.0, 19.0, 19.0));
            if unread {
                rgba(1.0, 0.62, 0.04, 1.0).setFill();
                NSBezierPath::bezierPathWithOvalInRect(rect(12.0, 12.0, 7.0, 7.0)).fill();
            }
            objc2::runtime::Bool::YES
        });
        let img = NSImage::imageWithSize_flipped_drawingHandler(size, false, &block);
        img.setTemplate(false);
        return img;
    }
    let size = objc2_foundation::NSSize::new(22.0, 15.0);
    let block = block2::RcBlock::new(move |_r: objc2_foundation::NSRect| -> objc2::runtime::Bool {
        NSColor::blackColor().setFill();
        // Bar (flipped drawing: y grows downward).
        NSBezierPath::fillRect(rect(1.0, 2.0, 17.0, 1.8));
        let notch = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect(5.5, 2.0, 8.0, 5.0), 2.0, 2.0);
        notch.fill();
        if unread {
            rgba(1.0, 0.62, 0.04, 1.0).setFill();
            NSBezierPath::bezierPathWithOvalInRect(rect(15.0, 8.0, 6.0, 6.0)).fill();
        }
        objc2::runtime::Bool::YES
    });
    let img = NSImage::imageWithSize_flipped_drawingHandler(size, true, &block);
    // A template adapts to light/dark menu bars; the orange dot needs colour.
    img.setTemplate(!unread);
    img
}

impl MenuBar {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            button.setImage(Some(&notch_glyph(false)));
            button.setToolTip(Some(&ns("Notchisland")));
        }
        let targets = Targets::default();
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);

        // ── what's running ──
        let header = NSMenuItem::new(mtm);
        header.setEnabled(false);
        menu.addItem(&header);

        // ── mini player (custom view) ──
        let w = 280.0;
        let pv = FlippedView::new(mtm, rect(0.0, 0.0, w, 58.0));
        let art_box = shape_view(mtm, rect(14.0, 9.0, 40.0, 40.0), &rgba(1.0, 1.0, 1.0, 0.1), 7.0);
        if let Some(l) = art_box.layer() {
            l.setMasksToBounds(true);
        }
        let art = NSImageView::new(mtm);
        art.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        art.setFrame(rect(0.0, 0.0, 40.0, 40.0));
        art_box.addSubview(&art);
        let title = label(
            mtm,
            "",
            &font(13.0, Weight::Semibold),
            &NSColor::labelColor(),
            rect(64.0, 11.0, w - 64.0 - 108.0, 17.0),
        );
        let subtitle = label(
            mtm,
            "",
            &font(11.5, Weight::Regular),
            &NSColor::secondaryLabelColor(),
            rect(64.0, 29.0, w - 64.0 - 108.0, 15.0),
        );
        let t = targets.add(mtm, |_| player_action(MediaAction::Previous));
        let prev = symbol_button(
            mtm,
            "backward.fill",
            12.0,
            "Previous track",
            &NSColor::labelColor(),
            &t,
            rect(w - 104.0, 15.0, 28.0, 28.0),
        );
        let t = targets.add(mtm, |_| player_action(MediaAction::PlayPause));
        let play = symbol_button(
            mtm,
            "pause.fill",
            11.0,
            "Pause",
            &NSColor::labelColor(),
            &t,
            rect(w - 72.0, 15.0, 28.0, 28.0),
        );
        crate::ui::make_card(&play, &rgba(0.5, 0.5, 0.5, 0.22), 14.0);
        let t = targets.add(mtm, |_| player_action(MediaAction::Next));
        let next = symbol_button(
            mtm,
            "forward.fill",
            12.0,
            "Next track",
            &NSColor::labelColor(),
            &t,
            rect(w - 40.0, 15.0, 28.0, 28.0),
        );
        for v in [&*art_box as &objc2_app_kit::NSView, &title, &subtitle, &prev, &play, &next] {
            pv.addSubview(v);
        }
        let player_item = NSMenuItem::new(mtm);
        player_item.setView(Some(&pv));
        menu.addItem(&player_item);
        let player_sep = NSMenuItem::separatorItem(mtm);
        menu.addItem(&player_sep);

        // ── everyday toggles, as four tiles ──
        let tiles: [Toggle; 4] = [
            (
                "moon.fill",
                "Focus",
                Box::new(|| {
                    with_controller(|c| c.toggle_focus());
                }),
            ),
            (
                "bell.slash.fill",
                "Pause",
                Box::new(|| {
                    with_controller(|c| c.toggle_pause());
                }),
            ),
            ("phone.fill", "Calls", Box::new(toggle_calls)),
            (
                "tray.full.fill",
                "Tray",
                Box::new(|| {
                    with_controller(|c| c.toggle());
                }),
            ),
        ];
        let (tile_w, tile_h, gap) = ((w - 28.0 - 8.0) / 2.0, 50.0, 8.0);
        let grid = FlippedView::new(mtm, rect(0.0, 0.0, w, 2.0 * tile_h + gap + 8.0));
        let mut toggles = Vec::new();
        for (i, (sym, name, action)) in tiles.into_iter().enumerate() {
            let (col, row) = ((i % 2) as f64, (i / 2) as f64);
            let tile = shape_view(
                mtm,
                rect(14.0 + col * (tile_w + gap), 4.0 + row * (tile_h + gap), tile_w, tile_h),
                &rgba(0.5, 0.5, 0.5, 0.14),
                12.0,
            );
            let disc = shape_view(mtm, rect(10.0, 11.0, 28.0, 28.0), &rgba(0.5, 0.5, 0.5, 0.3), 14.0);
            disc.addSubview(&crate::ui::symbol_view(
                mtm,
                sym,
                12.0,
                &NSColor::whiteColor(),
                rect(7.0, 7.0, 14.0, 14.0),
                "",
            ));
            tile.addSubview(&disc);
            tile.addSubview(&label(
                mtm,
                name,
                &font(12.5, Weight::Semibold),
                &NSColor::labelColor(),
                rect(46.0, 9.0, tile_w - 52.0, 16.0),
            ));
            let sub = label(
                mtm,
                "",
                &font(11.0, Weight::Regular),
                &NSColor::secondaryLabelColor(),
                rect(46.0, 25.0, tile_w - 52.0, 15.0),
            );
            tile.addSubview(&sub);
            let t = targets.add(mtm, move |_| action());
            tile.addSubview(&crate::ui::click_overlay(mtm, &t, rect(0.0, 0.0, tile_w, tile_h), name));
            grid.addSubview(&tile);
            toggles.push((disc, sub));
        }
        let grid_item = NSMenuItem::new(mtm);
        grid_item.setView(Some(&grid));
        menu.addItem(&grid_item);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let show = menu_item(mtm, &targets, "Open the Panel", "n", || {
            with_controller(|c| c.toggle());
        });
        // ⌃⌥N, registered system-wide by `crate::hotkey`.
        show.setKeyEquivalentModifierMask(
            objc2_app_kit::NSEventModifierFlags::Control | objc2_app_kit::NSEventModifierFlags::Option,
        );
        menu.addItem(&show);
        // ── timers ──
        let timer_root = NSMenuItem::new(mtm);
        timer_root.setTitle(&ns("Start Timer"));
        let timer_menu = NSMenu::new(mtm);
        for (mins, label) in [(1, "1 min"), (5, "5 min"), (10, "10 min"), (25, "25 min"), (45, "45 min")] {
            timer_menu.addItem(&menu_item(mtm, &targets, label, "", move || {
                crate::app::send(notchforge_core::engine::Command::StartTimer {
                    duration_ms: mins * 60_000,
                    label: label.to_owned(),
                });
            }));
        }
        timer_root.setSubmenu(Some(&timer_menu));
        menu.addItem(&timer_root);
        let cancel_timer = menu_item(mtm, &targets, "Cancel Timer", "", || {
            crate::app::send(notchforge_core::engine::Command::CancelTimer);
        });
        menu.addItem(&cancel_timer);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let shelve = menu_item(mtm, &targets, "Shelve Clipboard", "⌃⌥C", || {
            with_controller(|c| c.capture_clipboard());
        });
        menu.addItem(&shelve);
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        // ── Sound Output ▸ ──
        let output = NSMenu::new(mtm);
        output.setAutoenablesItems(false);
        let output_root = NSMenuItem::new(mtm);
        output_root.setTitle(&ns("Sound Output"));
        output_root.setSubmenu(Some(&output));
        menu.addItem(&output_root);
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        menu.addItem(&menu_item(mtm, &targets, "Settings…", ",", app::show_settings));
        menu.addItem(&menu_item(mtm, &targets, "Set Up Notchisland…", "", crate::onboarding::show));
        menu.addItem(&menu_item(mtm, &targets, "Open Config File", "", app::open_config_file));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&menu_item(mtm, &targets, "Quit Notchisland", "q", app::quit));
        item.setMenu(Some(&menu));
        player_item.setHidden(true);
        player_sep.setHidden(true);
        Self {
            mtm,
            item,
            player_item,
            player_sep,
            art,
            title,
            subtitle,
            play,
            header,
            toggles,
            toggles_item: grid_item,
            output,
            output_key: RefCell::new(String::new()),
            unread: Cell::new(false),
            _targets: targets,
            output_targets: Targets::default(),
        }
    }

    pub fn update(&self, snap: &Snapshot) {
        let mtm = self.mtm;
        // What's running, beside the app's name.
        let running = snap.usage.running.iter().filter(|r| r.working).count();
        let status = match running {
            0 => String::new(),
            1 => "  ·  1 session running".to_owned(),
            n => format!("  ·  {n} sessions running"),
        };
        self.header.setAttributedTitle(Some(&attributed_parts(&[
            ("Notchisland", &font(13.0, Weight::Semibold), &NSColor::labelColor()),
            (&status, &font(12.0, Weight::Regular), &NSColor::secondaryLabelColor()),
        ])));

        // The toggles: a coloured disc when on, and what each is doing.
        let focus_on = snap.focus.active == Some(true);
        let calls_on = app::with_config(|c| c.notifications.incoming_calls).unwrap_or(false);
        let focus_sub = match (&snap.focus.mode_name, focus_on) {
            (Some(m), true) if !m.is_empty() => m.clone(),
            (_, true) => "On".to_owned(),
            _ => "Off".to_owned(),
        };
        let files = match snap.tray.len() {
            0 => "Empty".to_owned(),
            1 => "1 file".to_owned(),
            n => format!("{n} files"),
        };
        let states = [
            (focus_on, rgba(0.369, 0.361, 0.902, 1.0), focus_sub),
            (snap.paused, rgba(0.039, 0.518, 1.0, 1.0), if snap.paused { "On" } else { "Off" }.to_owned()),
            (calls_on, rgba(0.188, 0.82, 0.345, 1.0), if calls_on { "Ringing on" } else { "Off" }.to_owned()),
            (!snap.tray.is_empty(), rgba(0.039, 0.518, 1.0, 1.0), files),
        ];
        for ((disc, sub), (on, color, text)) in self.toggles.iter().zip(states) {
            let fill = if on { color } else { rgba(0.5, 0.5, 0.5, 0.3) };
            crate::ui::make_card(disc, &fill, 14.0);
            set_text(sub, &text);
        }

        let unread = snap.unread > 0;
        if let Some(button) = self.item.button(mtm) {
            if self.unread.replace(unread) != unread {
                button.setImage(Some(&notch_glyph(unread)));
            }
            let tip = if unread { format!("Notchisland — {} unread", snap.unread) } else { "Notchisland".into() };
            button.setToolTip(Some(&ns(&tip)));
        }

        // Mini player.
        match snap.media.primary() {
            Some(n) => {
                let img = n.artwork_path.as_deref().and_then(crate::icons::file_image);
                match img {
                    Some(img) => {
                        self.art.setImage(Some(&img));
                        self.art.setContentTintColor(None);
                    }
                    None => {
                        self.art.setImage(crate::ui::symbol_image("music.note", 16.0, "Music").as_deref());
                        self.art.setContentTintColor(Some(&crate::ui::rgb8(n.player.color())));
                    }
                }
                set_text(&self.title, &n.title);
                let who = if n.artist.is_empty() { n.album.clone() } else { n.artist.clone() };
                set_text(&self.subtitle, &format!("{who} · {}", n.player.display_name()));
                let (sym, tip) = if n.is_playing() { ("pause.fill", "Pause") } else { ("play.fill", "Play") };
                set_button_symbol(&self.play, sym, 11.0, tip);
                set_accessibility_label(&self.title, &format!("{}, {}", n.title, n.subtitle()));
                self.player_item.setHidden(false);
                self.player_sep.setHidden(false);
            }
            None => {
                self.player_item.setHidden(true);
                self.player_sep.setHidden(true);
            }
        }

        // Sound Output: rebuilt only when the device list or default changes.
        let key: String = snap.audio.devices.iter().map(|d| format!("{}{}|", d.uid, d.is_default)).collect();
        if *self.output_key.borrow() != key {
            *self.output_key.borrow_mut() = key;
            self.output.removeAllItems();
            self.output_targets.clear();
            for d in &snap.audio.devices {
                let uid = d.uid.clone();
                let t = self.output_targets.add(mtm, move |_| crate::media::set_output(&uid));
                let item = NSMenuItem::new(mtm);
                let (f, fs) = (font(13.0, Weight::Regular), font(11.0, Weight::Regular));
                let caps = format!("\n{}", d.capabilities());
                let title = attributed_parts(&[
                    (&d.name, &f, &NSColor::labelColor()),
                    (&caps, &fs, &NSColor::secondaryLabelColor()),
                ]);
                item.setAttributedTitle(Some(&title));
                item.setState(if d.is_default { NSControlStateValueOn } else { NSControlStateValueOff });
                // SAFETY: target kept alive by `output_targets`.
                unsafe {
                    item.setTarget(Some(t.as_any()));
                    item.setAction(Some(ActionTarget::action()));
                }
                self.output.addItem(&item);
            }
            if snap.audio.devices.is_empty() {
                let none = NSMenuItem::new(mtm);
                none.setTitle(&ns("No output devices"));
                none.setEnabled(false);
                self.output.addItem(&none);
            }
        }
    }

    /// The mini player's view (self-test renders it).
    pub fn player_view(&self) -> Option<Retained<objc2_app_kit::NSView>> {
        self.player_item.view()
    }

    /// The quick toggles' view (self-test renders it).
    pub fn toggles_view(&self) -> Option<Retained<objc2_app_kit::NSView>> {
        self.toggles_item.view()
    }

    pub fn set_visible(&self, visible: bool) {
        self.item.setVisible(visible);
    }
}

/// A quick toggle: its symbol, its name and what clicking it does.
type Toggle = (&'static str, &'static str, Box<dyn Fn()>);

/// Turn ringing calls in the notch on or off.
fn toggle_calls() {
    if let Some(mut cfg) = app::with_config(|c| c.clone()) {
        cfg.notifications.incoming_calls = !cfg.notifications.incoming_calls;
        app::send(notchforge_core::engine::Command::UpdateConfig(Box::new(cfg)));
    }
}

/// Transport from the mini player: the featured player, else media keys.
fn player_action(action: MediaAction) {
    let player = with_controller(|c| c.current_snapshot().media.primary().map(|n| n.player)).flatten();
    match player {
        Some(p) => crate::media::control(p, action),
        None => {
            let key = match action {
                MediaAction::Next => crate::media::MediaKey::Next,
                MediaAction::Previous => crate::media::MediaKey::Previous,
                _ => crate::media::MediaKey::PlayPause,
            };
            crate::media::press(key);
        }
    }
}
