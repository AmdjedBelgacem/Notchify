//! Peek: a banner that drops out of the notch when a notification arrives
//! while collapsed, built to the *Peek · agent needs input* board (440 × 96).
//!
//! ```text
//! ┌──────────────────[ notch ]──────────────────┐
//! │ ░glow░ ╭──╮ ● Claude Code needs your input  │
//! │        │✳ │ Allow running cargo test? (Open)│
//! └─────────────────────────────────────────────┘
//! ```
//!
//! Urgent notifications (needs input, errors, high priority) get an orange
//! wash and an orange summary line; others a quiet grey one.

use super::{app_icon_view, set_app_icon};
use crate::ui::{
    FlippedView, Palette, Targets, Weight, attributed, font, label, pill_button, rect, rgba, set_accessibility_label,
    set_gradient, set_text, shape_view, style_pill, without_animation,
};
use notchforge_core::geometry::{LayoutKind, NotchLayout};
use notchforge_core::i18n::Str;
use notchforge_core::model::{AuthTarget, Notification, NotificationKind, Priority};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSImageView, NSMenu, NSMenuItem, NSTextField};
use objc2_foundation::NSPoint;
use std::cell::RefCell;

pub struct PeekView {
    pub root: Retained<FlippedView>,
    glow: Retained<objc2_quartz_core::CAGradientLayer>,
    tile: Retained<FlippedView>,
    icon: Retained<NSImageView>,
    dot: Retained<FlippedView>,
    summary: Retained<NSTextField>,
    title: Retained<NSTextField>,
    open: Retained<NSButton>,
    reply: Retained<NSButton>,
    /// Second answer to an authorization prompt (Deny).
    deny: Retained<NSButton>,
    cy: f64,
    _target: Retained<crate::ui::ActionTarget>,
    /// Right-click actions for the current peek (rebuilt each show).
    _menu_targets: RefCell<Targets>,
    _menu: RefCell<Option<Retained<NSMenu>>>,
    current: RefCell<Option<Notification>>,
}

/// Open a notification's deep link, else its app.
pub fn open_notification(n: &Notification) {
    let opened = n.url.as_deref().is_some_and(crate::sharing::open_url);
    if !opened {
        if let Some(b) = n.app.bundle_id.as_deref() {
            crate::sharing::open_app(b);
        }
    }
}

fn is_urgent(n: &Notification) -> bool {
    n.priority >= Priority::High || matches!(n.kind, NotificationKind::NeedsInput | NotificationKind::Error)
}

impl PeekView {
    pub fn new(mtm: MainThreadMarker, layout: &NotchLayout, p: &Palette) -> Self {
        let s = layout.shape(LayoutKind::Peek);
        let nh = layout.notch.rect.height;
        let (w, h) = (s.width, s.height);
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        root.setWantsLayer(true);
        let glow = objc2_quartz_core::CAGradientLayer::layer();
        glow.setType(&objc2_foundation::NSString::from_str("radial"));
        glow.setFrame(rect(0.0, 0.0, w, h));
        glow.setStartPoint(NSPoint::new(0.0, 1.0));
        glow.setEndPoint(NSPoint::new(0.42, 1.0 - 0.84));
        let (r, g, b) = p.accent_rgb;
        set_gradient(&glow, &[(r, g, b, 0.24), (r, g, b, 0.0)]);
        crate::ui::fade_in_below(&glow, nh, 18.0);
        if let Some(l) = root.layer() {
            l.addSublayer(&glow);
        }

        // Content row: 8 pt below the notch, 36 pt tall (the board's top: 40 px).
        let cy = nh + 26.0;
        let tile = shape_view(mtm, rect(16.0, cy - 18.0, 36.0, 36.0), &rgba(1.0, 1.0, 1.0, 0.08), 9.0);
        let icon = app_icon_view(mtm, rect(2.0, 2.0, 32.0, 32.0));
        tile.addSubview(&icon);
        // Board: 12 pt padding either side of the label.
        let open_font = font(12.5, Weight::Semibold);
        let ow = (crate::ui::text_width(&attributed("Open", &open_font, &p.primary)) + 24.0).ceil();
        let tx = 64.0;
        let tw = w - tx - ow - 28.0;
        let dot = shape_view(mtm, rect(tx, cy - 11.0, 6.0, 6.0), &p.warning, 3.0);
        let summary =
            label(mtm, "", &font(11.5, Weight::Semibold), &p.secondary, rect(tx + 12.0, cy - 16.0, tw - 12.0, 15.0));
        let title = label(mtm, "", &font(14.0, Weight::Semibold), &p.primary, rect(tx, cy - 1.0, tw, 18.0));
        let target = crate::ui::ActionTarget::new(mtm, |_| {});
        let open = pill_button(
            mtm,
            "Open",
            None,
            &open_font,
            &objc2_app_kit::NSColor::blackColor(),
            &objc2_app_kit::NSColor::whiteColor(),
            14.0,
            &target,
            rect(w - 16.0 - ow, cy - 14.0, ow, 28.0),
        );
        let reply = pill_button(
            mtm,
            "Reply",
            None,
            &open_font,
            &objc2_app_kit::NSColor::whiteColor(),
            &rgba(1.0, 1.0, 1.0, 0.14),
            14.0,
            &target,
            rect(w - 16.0 - ow - 8.0 - 56.0, cy - 14.0, 56.0, 28.0),
        );
        reply.setHidden(true);
        let deny = pill_button(
            mtm,
            "Deny",
            None,
            &open_font,
            &objc2_app_kit::NSColor::whiteColor(),
            &rgba(1.0, 1.0, 1.0, 0.14),
            14.0,
            &target,
            rect(w - 16.0 - ow - 8.0 - 52.0, cy - 14.0, 52.0, 28.0),
        );
        deny.setHidden(true);
        for v in [&*tile as &objc2_app_kit::NSView, &dot, &summary, &title, &open, &deny] {
            root.addSubview(v);
        }
        Self {
            root,
            glow,
            tile,
            icon,
            dot,
            summary,
            title,
            open,
            reply,
            deny,
            cy,
            _target: target,
            _menu_targets: RefCell::new(Targets::default()),
            _menu: RefCell::new(None),
            current: RefCell::new(None),
        }
    }

    pub fn show(&self, n: &Notification, p: &Palette) {
        let mtm = MainThreadMarker::from(&*self.root);
        // Without a real app icon the icon view draws its own brand tile
        // (see `set_app_icon`); the outer tile only frames real icons. A web
        // platform's logo counts as branded even before its favicon arrives.
        let branded = crate::icons::app_icon(&n.app).is_none() || n.app.platform.is_some();
        if let Some(l) = self.tile.layer() {
            let c = if branded { rgba(0.0, 0.0, 0.0, 0.0) } else { rgba(1.0, 1.0, 1.0, 0.08) };
            without_animation(|| l.setBackgroundColor(Some(&c.CGColor())));
        }
        self.icon.setFrame(if branded { rect(0.0, 0.0, 36.0, 36.0) } else { rect(2.0, 2.0, 32.0, 32.0) });
        set_app_icon(&self.icon, &n.app);
        let urgent = is_urgent(n);
        let summary = match n.kind {
            NotificationKind::NeedsInput => format!("{} needs your input", n.app.display_name),
            NotificationKind::TaskFinished => format!("{} finished", n.app.display_name),
            NotificationKind::Error => format!("{} hit an error", n.app.display_name),
            NotificationKind::Authorization => match n.auth_target() {
                Some(AuthTarget::Spotify) => format!("{} sign-in", n.app.display_name),
                _ => format!("{} wants access", n.app.display_name),
            },
            _ => n.app.display_name.clone(),
        };
        let color = if urgent { rgba(1.0, 0.70, 0.25, 1.0) } else { p.secondary.clone() };
        self.summary.setAttributedStringValue(&attributed(&summary, &font(11.5, Weight::Semibold), &color));
        self.dot.setHidden(!urgent);
        let shift = if urgent { 0.0 } else { -12.0 };
        let f = self.summary.frame();
        let base_x = self.dot.frame().origin.x + 12.0 - crate::ui::LABEL_INSET;
        self.summary.setFrameOrigin(NSPoint::new(base_x + shift, f.origin.y));
        without_animation(|| self.glow.setHidden(!urgent));
        let mut title = n.title.clone();
        if n.group_count > 1 {
            title.push_str(&format!("  ·  {}", n.group_count));
        }
        if title.trim().is_empty() {
            title = n.body.replace('\n', " ");
        }
        set_text(&self.title, &title);
        self.layout_buttons(n, p);
        // Right-click: dismiss or mute the app outright, without opening.
        let (id, rule, app_name) = (n.id, n.app.rule_key(), n.app.display_name.clone());
        let targets = Targets::default();
        let eng_t = targets.add(mtm, move |_| {
            crate::app::send(notchforge_core::engine::Command::DismissNotification(id));
        });
        let mute_t = targets.add(mtm, move |_| {
            crate::app::send(notchforge_core::engine::Command::MuteApp(rule.clone()));
            tracing::info!(app = %app_name, "app muted from the peek");
        });
        let menu = NSMenu::new(mtm);
        for (title, t) in [("Dismiss", &eng_t), ("Never show this app", &mute_t)] {
            let item = NSMenuItem::new(mtm);
            item.setTitle(&crate::ui::ns(title));
            // SAFETY: target kept alive by `targets` (dropped after show).
            unsafe {
                item.setTarget(Some(t.as_any()));
                item.setAction(Some(crate::ui::ActionTarget::action()));
            }
            menu.addItem(&item);
        }
        self._menu_targets.replace(targets);
        let menu_cell = self._menu.borrow_mut().replace(menu);
        // SAFETY: the menu lives in `_menu` for as long as the view.
        unsafe {
            if let Some(m) = menu_cell.as_ref() {
                self.root.setMenu(Some(m));
            }
        };
        set_accessibility_label(&self.root, &format!("New notification. {}", n.plain_text()));
        *self.current.borrow_mut() = Some(n.clone());
    }

    /// Size and style the button(s) for this notification: "Open" for an
    /// ordinary one, "Allow" + "Deny" for an agent asking for access, "Cancel"
    /// for a sign-in waiting on the browser.
    fn layout_buttons(&self, n: &Notification, p: &Palette) {
        let lang =
            crate::app::with_config(|c| notchforge_core::i18n::Language::from_config(&c.general)).unwrap_or_default();
        let tr = |k: notchforge_core::i18n::Str| notchforge_core::i18n::tr(lang, k);
        let quick_reply = crate::app::with_config(|c| c.notifications.quick_reply).unwrap_or(true);
        let w = self.root.frame().size.width;
        let target = n.auth_target();
        let replyable = quick_reply
            && matches!(n.kind, NotificationKind::Message)
            && n.app.category() == notchforge_core::apps::AppCategory::Messaging;
        let (first, second) = match &target {
            Some(AuthTarget::Agent(_) | AuthTarget::ToolPermission(_)) => (tr(Str::Allow), Some(tr(Str::Deny))),
            Some(AuthTarget::Spotify) => (tr(Str::Cancel), None),
            None if replyable => (tr(Str::Reply), Some(tr(Str::Open))),
            None => (tr(Str::Open), Some(tr(Str::Snooze))),
        };
        let f = font(12.5, Weight::Semibold);
        let measure = |t: &str| (crate::ui::text_width(&attributed(t, &f, &p.primary)) + 24.0).ceil();
        let (black, white) = (objc2_app_kit::NSColor::blackColor(), objc2_app_kit::NSColor::whiteColor());
        let quiet = rgba(1.0, 1.0, 1.0, 0.14);
        let pw = measure(first);
        if matches!(target, Some(AuthTarget::Spotify)) {
            style_pill(&self.open, first, None, &f, &white, &quiet, 14.0);
        } else {
            style_pill(&self.open, first, None, &f, &black, &white, 14.0);
        }
        self.open.setFrame(rect(w - 16.0 - pw, self.cy - 14.0, pw, 28.0));
        let mut left = w - 16.0 - pw;
        match second {
            Some(t) => {
                let sw = measure(t);
                if replyable {
                    style_pill(&self.reply, t, None, &f, &white, &quiet, 14.0);
                    left -= 8.0 + sw;
                    self.reply.setFrame(rect(left, self.cy - 14.0, sw, 28.0));
                    self.reply.setHidden(false);
                } else {
                    style_pill(&self.deny, t, None, &f, &white, &quiet, 14.0);
                    left -= 8.0 + sw;
                    self.deny.setFrame(rect(left, self.cy - 14.0, sw, 28.0));
                    self.deny.setHidden(false);
                }
            }
            None => {
                self.deny.setHidden(true);
                self.reply.setHidden(true);
            }
        }
        // Text keeps clear of the buttons (labels are widened by their inset).
        let tx = 64.0;
        let tw = (left - 12.0 - tx).max(40.0);
        let inset = crate::ui::LABEL_INSET;
        self.title.setFrame(rect(tx - inset, self.cy - 1.0, tw + 2.0 * inset, 18.0));
    }

    /// Which button is under `window_point`: `Main` (Open / Allow / Cancel /
    /// Reply), `Secondary` (Deny), or `Reply`.
    pub fn button_hit(&self, window_point: NSPoint) -> Option<PeekButton> {
        if self.root.isHidden() {
            return None;
        }
        let inside = |b: &NSButton| {
            if b.isHidden() {
                return false;
            }
            let local = b.convertPoint_fromView(window_point, None);
            let size = b.frame().size;
            (0.0..=size.width).contains(&local.x) && (0.0..=size.height).contains(&local.y)
        };
        if inside(&self.open) {
            Some(PeekButton::Main)
        } else if inside(&self.deny) {
            Some(PeekButton::Secondary)
        } else if inside(&self.reply) {
            Some(PeekButton::Reply)
        } else {
            None
        }
    }

    /// Press a button: open the notification, answer its question, or start a
    /// reply. Returns whether the peek should collapse afterwards.
    pub fn press(&self, kind: PeekButton) -> bool {
        let Some(n) = self.current.borrow().clone() else { return true };
        match (kind, n.auth_target()) {
            (PeekButton::Reply, None) => {
                crate::app::with_controller(|c| c.start_reply(n.id));
                false // the panel pins open for typing
            }
            (_, Some(target)) => {
                crate::app::with_controller(|c| c.answer_auth(&target, kind == PeekButton::Main));
                true
            }
            (PeekButton::Main, None) => {
                open_notification(&n);
                true
            }
            (PeekButton::Secondary, None) => {
                // Snooze: hide it for half an hour, then it peeks again.
                crate::app::send(notchforge_core::engine::Command::SnoozeNotification(n.id));
                true
            }
        }
    }
}

/// Which of the peek banner's buttons was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeekButton {
    Main,
    Secondary,
    Reply,
}
