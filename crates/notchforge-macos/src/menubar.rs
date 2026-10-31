//! Menu bar extra, built to the *Menu bar extra* board: a mini player, the
//! everyday toggles, a Sound Output submenu that lists each device with its
//! capabilities, then Settings and Quit. It is also the fallback entry point
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
    pause: Retained<NSMenuItem>,
    focus: Retained<NSMenuItem>,
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
            button.setToolTip(Some(&ns("NotchForge")));
        }
        let targets = Targets::default();
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);

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
            rect(64.0, 11.0, w - 64.0 - 76.0, 17.0),
        );
        let subtitle = label(
            mtm,
            "",
            &font(11.5, Weight::Regular),
            &NSColor::secondaryLabelColor(),
            rect(64.0, 29.0, w - 64.0 - 76.0, 15.0),
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
        for v in [&*art_box as &objc2_app_kit::NSView, &title, &subtitle, &play, &next] {
            pv.addSubview(v);
        }
        let player_item = NSMenuItem::new(mtm);
        player_item.setView(Some(&pv));
        menu.addItem(&player_item);
        let player_sep = NSMenuItem::separatorItem(mtm);
        menu.addItem(&player_sep);

        // ── everyday toggles ──
        let show = menu_item(mtm, &targets, "Show Panel", "n", || {
            with_controller(|c| c.toggle());
        });
        // ⌃⌥N, registered system-wide by `crate::hotkey`.
        show.setKeyEquivalentModifierMask(
            objc2_app_kit::NSEventModifierFlags::Control | objc2_app_kit::NSEventModifierFlags::Option,
        );
        menu.addItem(&show);
        let pause = menu_item(mtm, &targets, "Pause Notifications", "", || {
            with_controller(|c| c.toggle_pause());
        });
        menu.addItem(&pause);
        let focus = menu_item(mtm, &targets, "Focus", "", || {
            with_controller(|c| c.toggle_focus());
        });
        menu.addItem(&focus);
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
        menu.addItem(&menu_item(mtm, &targets, "Open Config File", "", app::open_config_file));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&menu_item(mtm, &targets, "Quit NotchForge", "q", app::quit));
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
            pause,
            focus,
            output,
            output_key: RefCell::new(String::new()),
            unread: Cell::new(false),
            _targets: targets,
            output_targets: Targets::default(),
        }
    }

    pub fn update(&self, snap: &Snapshot) {
        let mtm = self.mtm;
        self.pause.setState(if snap.paused { NSControlStateValueOn } else { NSControlStateValueOff });
        let focus_on = snap.focus.active == Some(true);
        self.focus.setState(if focus_on { NSControlStateValueOn } else { NSControlStateValueOff });
        let focus_title = match (&snap.focus.mode_name, focus_on) {
            (Some(m), true) if !m.is_empty() => format!("Focus · {m}"),
            _ => "Focus".to_owned(),
        };
        self.focus.setTitle(&ns(&focus_title));

        let unread = snap.unread > 0;
        if let Some(button) = self.item.button(mtm) {
            if self.unread.replace(unread) != unread {
                button.setImage(Some(&notch_glyph(unread)));
            }
            let tip = if unread { format!("NotchForge — {} unread", snap.unread) } else { "NotchForge".into() };
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

    pub fn set_visible(&self, visible: bool) {
        self.item.setVisible(visible);
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
                _ => crate::media::MediaKey::PlayPause,
            };
            crate::media::press(key);
        }
    }
}
