//! Settings window, built to the *Settings · Media* and *Settings · Coding
//! Agents* boards: an ink-black window with a brand header and grouped
//! sidebar, a pane header with a live status chip, and hairline-separated
//! cards of native controls (switches, pop-ups, fields, buttons), fully
//! VoiceOver accessible.
//!
//! Every change applies immediately: the edited draft config is sent to the
//! engine, which persists it and broadcasts it. A pane is rebuilt each time
//! it is shown so permission states are always fresh.

use crate::app;
use crate::permissions;
use crate::ui::{
    ActionTarget, FlippedView, Targets, Weight, attributed, attributed_tracked, font, label, ns, rect, rgba,
    shape_view, text_width,
};
use notchforge_core::apps::KnownApp;
use notchforge_core::config::{Config, ReducedMotion, ScreenChoice};
use notchforge_core::engine::Command;
use notchforge_core::model::{Permission, PermissionState, Priority, TemperatureUnit};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSBackingStoreType, NSButton, NSColor, NSControlSize,
    NSControlStateValueOff, NSControlStateValueOn, NSImageView, NSScrollView, NSSearchField, NSSlider, NSTextAlignment,
    NSTextField, NSView, NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const WIDTH: f64 = 960.0;
const HEIGHT: f64 = 700.0;
const SIDEBAR: f64 = 248.0;
const CONTENT_PAD: f64 = 36.0;
const HEADER_H: f64 = 96.0;
const GROUP_W: f64 = WIDTH - SIDEBAR - 2.0 * CONTENT_PAD;
/// Height of a row without a subtitle.
const ROW_H: f64 = 50.0;
thread_local! {
    /// Self-test only: show the design boards' sample state (Spotify connected
    /// with 24 playlists, Automation allowed, Accessibility not) so renders
    /// compare like for like.
    static PREVIEW: Cell<bool> = const { Cell::new(false) };
}

/// Turn the board sample state on or off (self-test renders).
pub fn set_preview(on: bool) {
    PREVIEW.with(|p| p.set(on));
}

fn preview() -> bool {
    PREVIEW.with(Cell::get)
}

const TOGGLE_W: f64 = 42.0;
const TOGGLE_H: f64 = 26.0;

/// Track colour and knob position of the board's switch (blue on, grey off,
/// white knob).
fn style_toggle(b: &NSButton, on: bool) {
    let Some(l) = b.layer() else { return };
    let track = if on { rgba(0.17, 0.48, 0.97, 1.0) } else { rgba(1.0, 1.0, 1.0, 0.16) };
    l.setBackgroundColor(Some(&track.CGColor()));
    let r = (TOGGLE_H - 4.0) / 2.0;
    let x = if on { TOGGLE_W - 2.0 - r } else { 2.0 + r };
    // SAFETY: plain read of the layer's sublayer list on the main thread.
    if let Some(subs) = unsafe { l.sublayers() } {
        for k in subs.iter() {
            k.setPosition(objc2_foundation::NSPoint::new(x, TOGGLE_H / 2.0));
        }
    }
}

/// The board's button: a dark rounded pill (or blue when primary) with the
/// label centred.
fn pill_button(
    mtm: MainThreadMarker,
    title: &str,
    target: &crate::ui::ActionTarget,
    frame: objc2_foundation::NSRect,
    primary: bool,
) -> Retained<NSButton> {
    // SAFETY: valid target/selector; the target is kept alive by the caller.
    let b = unsafe {
        NSButton::buttonWithTitle_target_action(
            &ns(title),
            Some(target.as_any()),
            Some(crate::ui::ActionTarget::action()),
            mtm,
        )
    };
    b.setBordered(false);
    b.setFrame(frame);
    b.setAlignment(NSTextAlignment::Center);
    b.setAttributedTitle(&attributed(
        title,
        &font(13.0, Weight::Medium),
        &rgba(1.0, 1.0, 1.0, if primary { 1.0 } else { 0.92 }),
    ));
    b.setWantsLayer(true);
    if let Some(l) = b.layer() {
        l.setCornerRadius(8.0);
        if primary {
            l.setBackgroundColor(Some(&rgba(0.2, 0.5, 0.98, 1.0).CGColor()));
        } else {
            l.setBackgroundColor(Some(&rgba(0.11, 0.11, 0.125, 1.0).CGColor()));
            l.setBorderWidth(1.0);
            l.setBorderColor(Some(&rgba(1.0, 1.0, 1.0, 0.10).CGColor()));
        }
    }
    b
}

/// Settings panes, in sidebar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    General,
    Appearance,
    Notifications,
    Agents,
    Weather,
    Media,
    Tray,
    Controls,
    Permissions,
}

impl Pane {
    pub const ALL: [Pane; 9] = [
        Pane::General,
        Pane::Appearance,
        Pane::Notifications,
        Pane::Agents,
        Pane::Media,
        Pane::Weather,
        Pane::Tray,
        Pane::Controls,
        Pane::Permissions,
    ];

    /// The sidebar's captioned sections.
    const SECTIONS: [(&'static str, &'static [Pane]); 3] = [
        ("Surface", &[Pane::General, Pane::Appearance]),
        ("Content", &[Pane::Notifications, Pane::Agents, Pane::Media, Pane::Weather, Pane::Tray]),
        ("System", &[Pane::Controls, Pane::Permissions]),
    ];

    /// One line under the pane title.
    fn blurb(self) -> &'static str {
        match self {
            Pane::General => "Launch, menu bar, display and how the notch reacts.",
            Pane::Appearance => "Motion, animation speed and the live indicators.",
            Pane::Notifications => "Which notifications peek out, and from where.",
            Pane::Agents => "Claude Code, Codex, Cursor, Zed and anything that can post JSON.",
            Pane::Media => "What plays beside the notch, and how your music apps connect.",
            Pane::Weather => "Location, units and how often the forecast refreshes.",
            Pane::Tray => "A temporary shelf for files you drag onto the notch.",
            Pane::Controls => "Brightness, volume, resolution and Focus from the island.",
            Pane::Permissions => "Every permission is optional; features wait until it is granted.",
        }
    }

    /// Top and bottom colour of the pane's icon tile.
    fn gradient(self) -> ((u8, u8, u8), (u8, u8, u8)) {
        match self {
            Pane::General => ((154, 154, 160), (110, 110, 116)),
            Pane::Appearance => ((156, 139, 255), (111, 92, 255)),
            Pane::Notifications => ((255, 106, 96), (232, 55, 47)),
            Pane::Agents => ((238, 149, 119), (217, 119, 87)),
            Pane::Media => ((255, 100, 130), (245, 43, 73)),
            Pane::Weather => ((99, 208, 255), (47, 155, 224)),
            Pane::Tray => ((90, 165, 255), (31, 112, 240)),
            Pane::Controls => ((134, 131, 242), (94, 92, 230)),
            Pane::Permissions => ((69, 214, 234), (20, 166, 196)),
        }
    }

    /// Live status shown at the right of the pane header.
    fn chip(self, c: &Config) -> Option<(String, Tone)> {
        match self {
            Pane::Media => Some(if app::spotify_signed_in() || preview() {
                ("Spotify connected".into(), Tone::Ok)
            } else {
                ("Spotify not connected".into(), Tone::Neutral)
            }),
            Pane::Agents => Some(if c.providers.agents {
                (format!("Listening on {}", c.agents.http_port), Tone::Ok)
            } else {
                ("Off".into(), Tone::Neutral)
            }),
            Pane::Permissions => {
                let all = Permission::ALL;
                let ok = all
                    .iter()
                    .filter(|p| {
                        matches!(permissions::state(**p), PermissionState::Granted | PermissionState::NotRequired)
                    })
                    .count();
                Some((format!("{ok} of {} allowed", all.len()), if ok == all.len() { Tone::Ok } else { Tone::Neutral }))
            }
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Pane::General => "General",
            Pane::Appearance => "Appearance & Motion",
            Pane::Notifications => "Notifications",
            Pane::Agents => "Coding Agents",
            Pane::Weather => "Weather",
            Pane::Media => "Media",
            Pane::Tray => "File Tray",
            Pane::Controls => "Quick Controls",
            Pane::Permissions => "Permissions",
        }
    }

    /// (SF Symbol, tile colour) for the sidebar.
    fn icon(self) -> (&'static str, (u8, u8, u8)) {
        match self {
            Pane::General => ("gearshape.fill", (142, 142, 147)),
            Pane::Appearance => ("circle.lefthalf.filled", (60, 60, 67)),
            Pane::Notifications => ("bell.badge.fill", (255, 59, 48)),
            Pane::Agents => ("chevron.left.forwardslash.chevron.right", (217, 119, 87)),
            Pane::Weather => ("cloud.sun.fill", (50, 173, 230)),
            Pane::Media => ("music.note", (250, 45, 72)),
            Pane::Tray => ("tray.full.fill", (10, 132, 255)),
            Pane::Controls => ("slider.horizontal.3", (94, 92, 230)),
            Pane::Permissions => ("hand.raised.fill", (10, 132, 255)),
        }
    }

    /// Extra words the sidebar search matches.
    fn keywords(self) -> &'static str {
        match self {
            Pane::General => "launch login menu bar screen display hover peek config logs reset battery accessories",
            Pane::Appearance => "motion animation speed wings contrast",
            Pane::Notifications => "focus banners apps whatsapp discord slack messages priority notification center",
            Pane::Agents => {
                "claude cursor zed codex opencode qoder webhook port token socket inbox allow deny approval authorize consent"
            }
            Pane::Weather => "location city units celsius fahrenheit refresh",
            Pane::Media => "spotify apple music audio output playlists client id automation",
            Pane::Tray => "files drop airdrop keep",
            Pane::Controls => "brightness volume resolution focus shortcuts",
            Pane::Permissions => "accessibility full disk access location automation privacy",
        }
    }
}

pub struct SettingsWindow {
    window: Retained<NSWindow>,
    targets: Rc<Targets>,
    pane: Rc<Cell<Pane>>,
}

type Draft = Rc<RefCell<Config>>;
/// (title, primary, action) for a row button.
type RowButton<'a> = (&'a str, bool, Box<dyn Fn()>);

fn text_primary() -> Retained<NSColor> {
    rgba(1.0, 1.0, 1.0, 0.94)
}
fn text_secondary() -> Retained<NSColor> {
    rgba(1.0, 1.0, 1.0, 0.6)
}
fn text_tertiary() -> Retained<NSColor> {
    rgba(1.0, 1.0, 1.0, 0.5)
}
fn hairline() -> Retained<NSColor> {
    rgba(1.0, 1.0, 1.0, 0.07)
}
fn ink() -> Retained<NSColor> {
    rgba(0.043, 0.043, 0.051, 1.0)
}
fn side_bg() -> Retained<NSColor> {
    rgba(0.063, 0.063, 0.075, 1.0)
}
fn card_bg() -> Retained<NSColor> {
    rgba(0.082, 0.082, 0.094, 1.0)
}

/// Meaning of a status capsule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Ok,
    Warn,
    Neutral,
}

/// A status capsule: icon and text on a tinted pill. Returns it and its width.
fn chip_view(mtm: MainThreadMarker, text: &str, tone: Tone) -> (Retained<FlippedView>, f64) {
    let (fg, bg, edge, symbol) = match tone {
        Tone::Ok => (
            rgba(0.247, 0.878, 0.478, 1.0),
            rgba(0.188, 0.82, 0.345, 0.14),
            rgba(0.188, 0.82, 0.345, 0.4),
            Some("checkmark.circle.fill"),
        ),
        Tone::Warn => (
            rgba(1.0, 0.702, 0.251, 1.0),
            rgba(1.0, 0.624, 0.039, 0.14),
            rgba(1.0, 0.624, 0.039, 0.4),
            Some("exclamationmark.triangle.fill"),
        ),
        Tone::Neutral => (text_secondary(), rgba(1.0, 1.0, 1.0, 0.07), rgba(1.0, 1.0, 1.0, 0.1), None),
    };
    let f = font(12.0, Weight::Semibold);
    let tw = text_width(&attributed(text, &f, &fg));
    let text_x = if symbol.is_some() { 28.0 } else { 11.0 };
    let w = text_x + tw + 11.0;
    let v = shape_view(mtm, rect(0.0, 0.0, w, 26.0), &bg, 13.0);
    if let Some(l) = v.layer() {
        l.setBorderWidth(0.5);
        l.setBorderColor(Some(&edge.CGColor()));
    }
    if let Some(sym) = symbol {
        v.addSubview(&crate::ui::symbol_view(mtm, sym, 12.0, &fg, rect(9.0, 6.5, 13.0, 13.0), ""));
    }
    v.addSubview(&label(mtm, text, &f, &fg, rect(text_x, 5.0, tw + 2.0, 16.0)));
    crate::ui::set_accessibility_label(&v, text);
    (v, w)
}

/// A small tile filled with a vertical gradient (pane icons).
fn gradient_tile(
    mtm: MainThreadMarker,
    frame: objc2_foundation::NSRect,
    (a, b): ((u8, u8, u8), (u8, u8, u8)),
    radius: f64,
) -> Retained<FlippedView> {
    let v = shape_view(mtm, frame, &rgba(0.0, 0.0, 0.0, 0.0), radius);
    if let Some(l) = v.layer() {
        let c = |t: (u8, u8, u8)| (f64::from(t.0) / 255.0, f64::from(t.1) / 255.0, f64::from(t.2) / 255.0, 1.0);
        let g = crate::ui::gradient_layer(
            &[c(a), c(b)],
            objc2_foundation::NSPoint::new(0.5, 0.0),
            objc2_foundation::NSPoint::new(0.5, 1.0),
        );
        g.setFrame(l.bounds());
        l.setMasksToBounds(true);
        l.addSublayer(&g);
        l.setBorderWidth(0.5);
        l.setBorderColor(Some(&rgba(1.0, 1.0, 1.0, 0.22).CGColor()));
    }
    v
}

/// An uppercase, letter-spaced caption.
fn caption(mtm: MainThreadMarker, text: &str, size: f64, frame: objc2_foundation::NSRect) -> Retained<NSTextField> {
    let a = attributed_tracked(&text.to_uppercase(), &font(size, Weight::Semibold), &text_tertiary(), size * 0.08);
    crate::ui::rich_label(mtm, &a, frame)
}

/// Lays out groups of rows top-to-bottom inside a pane.
struct Form<'a> {
    mtm: MainThreadMarker,
    doc: &'a FlippedView,
    y: Cell<f64>,
    targets: &'a Targets,
    draft: Draft,
    /// The open group box and the y of its next row.
    group: RefCell<Option<(Retained<FlippedView>, f64)>>,
    /// Colour the next row's subtitle green (a connected account).
    sub_ok: Cell<bool>,
}

const PRIORITIES: [(&str, Priority); 4] =
    [("Low", Priority::Low), ("Normal", Priority::Normal), ("High", Priority::High), ("Critical", Priority::Critical)];

impl Form<'_> {
    fn commit(draft: &Draft) {
        app::send(Command::UpdateConfig(Box::new(draft.borrow().clone())));
    }

    fn begin(&self, title: &str) {
        self.end();
        if !title.is_empty() {
            self.doc.addSubview(&caption(self.mtm, title, 11.0, rect(CONTENT_PAD + 6.0, self.y.get(), GROUP_W, 14.0)));
            self.y.set(self.y.get() + 22.0);
        }
        let g = shape_view(self.mtm, rect(CONTENT_PAD, self.y.get(), GROUP_W, 0.0), &card_bg(), 13.0);
        if let Some(l) = g.layer() {
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&hairline().CGColor()));
        }
        self.doc.addSubview(&g);
        *self.group.borrow_mut() = Some((g, 0.0));
    }

    fn end(&self) {
        if let Some((g, h)) = self.group.borrow_mut().take() {
            let f = g.frame();
            g.setFrame(rect(f.origin.x, f.origin.y, f.size.width, h));
            self.y.set(f.origin.y + h + 22.0);
        }
    }

    /// Footnote under the last group.
    fn note(&self, text: &str) {
        self.end();
        let f = font(12.0, Weight::Regular);
        let lines = (text_width(&attributed(text, &f, &text_tertiary())) / (GROUP_W - 16.0)).ceil().max(1.0);
        let h = lines * 17.0;
        let l = crate::ui::wrapping_label(
            self.mtm,
            text,
            &f,
            &text_tertiary(),
            rect(CONTENT_PAD + 8.0, self.y.get() - 14.0, GROUP_W - 16.0, h),
            4,
        );
        self.doc.addSubview(&l);
        self.y.set(self.y.get() - 14.0 + h + 22.0);
    }

    /// Append a row of `height` to the open group; returns (row view, width).
    fn row(&self, height: f64) -> (Retained<FlippedView>, f64) {
        let mut g = self.group.borrow_mut();
        let (group, y) = g.as_mut().expect("row outside a group");
        let row = FlippedView::new(self.mtm, rect(0.0, *y, GROUP_W, height));
        if *y > 0.0 {
            row.addSubview(&shape_view(self.mtm, rect(16.0, 0.0, GROUP_W - 16.0, 0.5), &hairline(), 0.0));
        }
        group.addSubview(&row);
        *y += height;
        (row, GROUP_W)
    }

    /// Row height for a title with an optional subtitle in `text_w` points.
    fn texts(&self, subtitle: Option<&str>, text_w: f64) -> f64 {
        let lines = subtitle.map_or(0.0, |s| {
            (text_width(&attributed(s, &font(12.0, Weight::Regular), &text_secondary())) / text_w)
                .ceil()
                .clamp(1.0, 3.0)
        });
        if lines == 0.0 { ROW_H } else { 38.0 + lines * 16.0 }
    }

    fn put_texts(&self, row: &FlippedView, title: &str, subtitle: Option<&str>, text_w: f64, h: f64) {
        let has_sub = subtitle.is_some();
        let ty = if has_sub { 10.0 } else { (h - 18.0) / 2.0 };
        row.addSubview(&label(
            self.mtm,
            title,
            &font(13.5, Weight::Medium),
            &text_primary(),
            rect(16.0, ty, text_w, 18.0),
        ));
        if let Some(s) = subtitle {
            let l = crate::ui::wrapping_label(
                self.mtm,
                s,
                &font(12.0, Weight::Regular),
                &*if self.sub_ok.get() { rgba(0.19, 0.82, 0.35, 1.0) } else { text_secondary() },
                rect(16.0, ty + 19.0, text_w, h - 38.0),
                3,
            );
            row.addSubview(&l);
        }
    }

    fn switch(&self, title: &str, subtitle: Option<&str>, value: bool, set: impl Fn(&mut Config, bool) + 'static) {
        let text_w = GROUP_W - 96.0;
        let h = self.texts(subtitle, text_w);
        let (row, w) = self.row(h);
        self.put_texts(&row, title, subtitle, text_w, h);
        let draft = self.draft.clone();
        let t = self.targets.add(self.mtm, move |sender: Option<&AnyObject>| {
            if let Some(b) = sender.and_then(|s| s.downcast_ref::<NSButton>()) {
                let on = b.state() == NSControlStateValueOn;
                style_toggle(b, on);
                set(&mut draft.borrow_mut(), on);
                Self::commit(&draft);
            }
        });
        let sw = NSButton::new(self.mtm);
        sw.setButtonType(objc2_app_kit::NSButtonType::PushOnPushOff);
        sw.setBordered(false);
        sw.setTitle(&ns(""));
        sw.setState(if value { NSControlStateValueOn } else { NSControlStateValueOff });
        sw.setFrame(rect(w - 16.0 - TOGGLE_W, (h - TOGGLE_H) / 2.0, TOGGLE_W, TOGGLE_H));
        sw.setWantsLayer(true);
        if let Some(l) = sw.layer() {
            l.setCornerRadius(TOGGLE_H / 2.0);
            let knob = objc2_quartz_core::CALayer::new();
            knob.setBounds(rect(0.0, 0.0, TOGGLE_H - 4.0, TOGGLE_H - 4.0));
            knob.setCornerRadius((TOGGLE_H - 4.0) / 2.0);
            knob.setBackgroundColor(Some(&rgba(1.0, 1.0, 1.0, 1.0).CGColor()));
            knob.setShadowOpacity(0.3);
            knob.setShadowRadius(1.5);
            knob.setName(Some(&ns("knob")));
            l.addSublayer(&knob);
        }
        style_toggle(&sw, value);
        // SAFETY: target kept alive by `targets`.
        unsafe {
            sw.setTarget(Some(t.as_any()));
            sw.setAction(Some(ActionTarget::action()));
        }
        crate::ui::set_accessibility_label(&sw, title);
        row.addSubview(&sw);
    }

    /// The board's pop-up: a dark rounded pill with the current value and a
    /// chevron; clicking opens a menu of the options.
    fn popup(&self, title: &str, options: &[&str], selected: usize, set: impl Fn(&mut Config, usize) + 'static) {
        use objc2_app_kit::{NSCellImagePosition, NSMenu, NSMenuItem};
        let (row, w) = self.row(ROW_H);
        self.put_texts(&row, title, None, w - 250.0, ROW_H);
        let (pw, ph) = (112.0, 27.0);
        let value_font = font(13.0, Weight::Regular);
        let value_color = rgba(1.0, 1.0, 1.0, 0.92);
        let btn = NSButton::new(self.mtm);
        btn.setButtonType(objc2_app_kit::NSButtonType::MomentaryPushIn);
        btn.setBordered(false);
        btn.setFrame(rect(w - 16.0 - pw, (ROW_H - ph) / 2.0, pw, ph));
        btn.setAlignment(NSTextAlignment::Left);
        btn.setAttributedTitle(&attributed(options.get(selected).copied().unwrap_or(""), &value_font, &value_color));
        btn.setImage(crate::ui::symbol_image("chevron.up.chevron.down", 9.0, "").as_deref());
        btn.setImagePosition(NSCellImagePosition::ImageRight);
        btn.setContentTintColor(Some(&rgba(1.0, 1.0, 1.0, 0.55)));
        btn.setWantsLayer(true);
        if let Some(l) = btn.layer() {
            l.setCornerRadius(8.0);
            l.setBackgroundColor(Some(&rgba(0.11, 0.11, 0.125, 1.0).CGColor()));
            l.setBorderWidth(1.0);
            l.setBorderColor(Some(&rgba(1.0, 1.0, 1.0, 0.10).CGColor()));
        }
        crate::ui::set_accessibility_label(&btn, title);

        let set = Rc::new(set);
        let menu = NSMenu::new(self.mtm);
        let names: Vec<String> = options.iter().map(|o| (*o).to_owned()).collect();
        for (i, name) in names.iter().enumerate() {
            let draft = self.draft.clone();
            let set = set.clone();
            let button = btn.clone();
            let (font, color, title) = (value_font.clone(), value_color.clone(), name.clone());
            let t = self.targets.add(self.mtm, move |_| {
                button.setAttributedTitle(&attributed(&title, &font, &color));
                set(&mut draft.borrow_mut(), i);
                Self::commit(&draft);
            });
            // SAFETY: a valid selector (the shared action) and plain strings.
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm),
                    &ns(name),
                    Some(ActionTarget::action()),
                    &ns(""),
                )
            };
            // SAFETY: target kept alive by `targets`.
            unsafe { item.setTarget(Some(t.as_any())) };
            item.setState(if i == selected { NSControlStateValueOn } else { NSControlStateValueOff });
            menu.addItem(&item);
        }
        let opener = {
            let button = btn.clone();
            self.targets.add(self.mtm, move |_| {
                menu.popUpMenuPositioningItem_atLocation_inView(
                    None,
                    objc2_foundation::NSPoint::new(0.0, ph + 4.0),
                    Some(&button),
                );
            })
        };
        // SAFETY: target kept alive by `targets`.
        unsafe {
            btn.setTarget(Some(opener.as_any()));
            btn.setAction(Some(ActionTarget::action()));
        }
        row.addSubview(&btn);
    }

    fn text(
        &self,
        title: &str,
        value: &str,
        placeholder: &str,
        mono: bool,
        set: impl Fn(&mut Config, String) + 'static,
    ) {
        self.text_row(title, None, value, placeholder, mono, 300.0, false, set);
    }

    /// A row with a text field on the right: dark rounded box, `width` wide.
    #[allow(clippy::too_many_arguments)]
    fn text_row(
        &self,
        title: &str,
        subtitle: Option<&str>,
        value: &str,
        placeholder: &str,
        mono: bool,
        width: f64,
        secure: bool,
        set: impl Fn(&mut Config, String) + 'static,
    ) {
        let h = self.texts(subtitle, GROUP_W - 64.0 - width);
        let (row, w) = self.row(h);
        self.put_texts(&row, title, subtitle, w - 64.0 - width, h);
        let box_frame = rect(w - 16.0 - width, (h - 28.0) / 2.0, width, 28.0);
        let frame = rect(10.0, 4.0, width - 20.0, 20.0);
        let f: Retained<NSTextField> = if secure {
            let sf = objc2_app_kit::NSSecureTextField::initWithFrame(
                <objc2_app_kit::NSSecureTextField as MainThreadOnly>::alloc(self.mtm),
                frame,
            );
            sf.setStringValue(&ns(value));
            Retained::into_super(sf)
        } else {
            let tf = NSTextField::textFieldWithString(&ns(value), self.mtm);
            tf.setFrame(frame);
            tf
        };
        f.setPlaceholderString(Some(&ns(placeholder)));
        f.setAlignment(NSTextAlignment::Right);
        f.setBordered(false);
        f.setBezeled(false);
        f.setDrawsBackground(false);
        f.setFocusRingType(objc2_app_kit::NSFocusRingType::None);
        f.setTextColor(Some(&text_primary()));
        let field_box = shape_view(self.mtm, box_frame, &rgba(0.04, 0.04, 0.05, 1.0), 8.0);
        if let Some(l) = field_box.layer() {
            l.setBorderWidth(1.0);
            l.setBorderColor(Some(&rgba(1.0, 1.0, 1.0, 0.08).CGColor()));
        }
        if mono {
            f.setFont(Some(&objc2_app_kit::NSFont::monospacedSystemFontOfSize_weight(
                13.0,
                crate::ui::weight(Weight::Regular),
            )));
        }
        if let Some(cell) = f.cell() {
            cell.setSendsActionOnEndEditing(true);
        }
        crate::ui::set_accessibility_label(&f, title);
        let draft = self.draft.clone();
        let t = self.targets.add(self.mtm, move |sender: Option<&AnyObject>| {
            if let Some(f) = sender.and_then(|s| s.downcast_ref::<NSTextField>()) {
                set(&mut draft.borrow_mut(), f.stringValue().to_string().trim().to_owned());
                Self::commit(&draft);
            }
        });
        // SAFETY: target kept alive by `targets`.
        unsafe {
            f.setTarget(Some(t.as_any()));
            f.setAction(Some(ActionTarget::action()));
        }
        field_box.addSubview(&f);
        row.addSubview(&field_box);
    }

    fn slider(&self, title: &str, min: f64, max: f64, value: f64, set: impl Fn(&mut Config, f64) + 'static) {
        let (row, w) = self.row(ROW_H);
        self.put_texts(&row, title, None, w - 270.0, ROW_H);
        let draft = self.draft.clone();
        let t = self.targets.add(self.mtm, move |sender: Option<&AnyObject>| {
            if let Some(s) = sender.and_then(|s| s.downcast_ref::<NSSlider>()) {
                set(&mut draft.borrow_mut(), s.doubleValue());
                Self::commit(&draft);
            }
        });
        // SAFETY: target kept alive by `targets`.
        let s = unsafe {
            NSSlider::sliderWithValue_minValue_maxValue_target_action(
                value,
                min,
                max,
                Some(t.as_any()),
                Some(ActionTarget::action()),
                self.mtm,
            )
        };
        s.setControlSize(NSControlSize::Small);
        s.setContinuous(false);
        s.setFrame(rect(w - 16.0 - 220.0, (ROW_H - 20.0) / 2.0, 220.0, 20.0));
        crate::ui::set_accessibility_label(&s, title);
        row.addSubview(&s);
    }

    /// A row with a status capsule and buttons on the right.
    fn action_row(
        &self,
        title: &str,
        subtitle: Option<&str>,
        status: Option<(&str, Tone)>,
        buttons: Vec<RowButton<'_>>,
        leading_icon: Option<(&str, (u8, u8, u8))>,
    ) -> Vec<Retained<NSButton>> {
        let bw: Vec<f64> = buttons
            .iter()
            .map(|(t, _, _)| {
                (text_width(&attributed(t, &font(12.5, Weight::Regular), &text_primary())) + 26.0).max(70.0)
            })
            .collect();
        let chip = status.map(|(t, tone)| chip_view(self.mtm, t, tone));
        let chip_w = chip.as_ref().map_or(0.0, |(_, w)| w + 10.0);
        let controls_w = bw.iter().sum::<f64>() + 8.0 * bw.len() as f64 + chip_w;
        let icon_w = if leading_icon.is_some() { 46.0 } else { 0.0 };
        let text_w = GROUP_W - 32.0 - controls_w - icon_w;
        let h = self.texts(subtitle, text_w).max(if leading_icon.is_some() { 62.0 } else { ROW_H });
        let (row, w) = self.row(h);
        if let Some((sym, color)) = leading_icon {
            let tile =
                shape_view(self.mtm, rect(16.0, (h - 34.0) / 2.0, 34.0, 34.0), &rgba(0.047, 0.047, 0.055, 1.0), 10.0);
            if let Some(l) = tile.layer() {
                l.setBorderWidth(0.5);
                l.setBorderColor(Some(&rgba(1.0, 1.0, 1.0, 0.14).CGColor()));
            }
            let disc = shape_view(self.mtm, rect(6.0, 6.0, 22.0, 22.0), &crate::ui::rgb8(color), 11.0);
            if sym == "spotify" {
                disc.addSubview(&crate::ui::spotify_mark(self.mtm, rect(4.0, 4.0, 14.0, 14.0)));
            } else {
                disc.addSubview(&crate::ui::symbol_view(
                    self.mtm,
                    sym,
                    12.0,
                    &NSColor::blackColor(),
                    rect(4.0, 4.0, 14.0, 14.0),
                    title,
                ));
            }
            tile.addSubview(&disc);
            row.addSubview(&tile);
            let inner = FlippedView::new(self.mtm, rect(icon_w, 0.0, w - icon_w, h));
            self.put_texts(&inner, title, subtitle, text_w, h);
            row.addSubview(&inner);
        } else {
            self.put_texts(&row, title, subtitle, text_w, h);
        }
        let mut x = w - 16.0;
        let mut made = Vec::new();
        for ((t, primary, f), width) in buttons.into_iter().zip(bw).rev() {
            x -= width;
            let target = self.targets.add(self.mtm, move |_| f());
            let b = pill_button(self.mtm, t, &target, rect(x, (h - 28.0) / 2.0, width, 28.0), primary);
            row.addSubview(&b);
            made.push(b);
            x -= 8.0;
        }
        if let Some((chip, cw)) = chip {
            chip.setFrameOrigin(objc2_foundation::NSPoint::new(x - cw - 2.0, (h - 26.0) / 2.0));
            row.addSubview(&chip);
        }
        made
    }

    /// A monospaced snippet in its own dark card, with a Copy button.
    fn code_block(&self, title: &str, runs: &[(&str, Retained<NSColor>)], copy: Box<dyn Fn()>) {
        self.end();
        let lines = 1 + runs.iter().map(|(t, _)| t.matches('\n').count()).sum::<usize>();
        let body_h = lines as f64 * 18.0 + 24.0;
        let h = 34.0 + body_h;
        let card =
            shape_view(self.mtm, rect(CONTENT_PAD, self.y.get(), GROUP_W, h), &rgba(0.039, 0.039, 0.047, 1.0), 12.0);
        if let Some(l) = card.layer() {
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&hairline().CGColor()));
        }
        card.addSubview(&label(
            self.mtm,
            title,
            &font(11.5, Weight::Regular),
            &text_tertiary(),
            rect(14.0, 9.0, GROUP_W - 120.0, 16.0),
        ));
        let target = self.targets.add(self.mtm, move |_| copy());
        let b = pill_button(self.mtm, "Copy", &target, rect(GROUP_W - 8.0 - 76.0, 4.0, 76.0, 26.0), false);
        b.setImage(crate::ui::symbol_image("doc.on.doc", 11.0, "").as_deref());
        b.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageLeft);
        b.setContentTintColor(Some(&rgba(1.0, 1.0, 1.0, 0.7)));
        b.setControlSize(NSControlSize::Regular);
        card.addSubview(&b);
        card.addSubview(&shape_view(self.mtm, rect(0.0, 34.0, GROUP_W, 0.5), &hairline(), 0.0));
        let mono = objc2_app_kit::NSFont::monospacedSystemFontOfSize_weight(11.5, crate::ui::weight(Weight::Regular));
        let parts: Vec<(&str, &objc2_app_kit::NSFont, &NSColor)> =
            runs.iter().map(|(t, c)| (*t, &*mono, &**c)).collect();
        let body = crate::ui::wrapping_label(
            self.mtm,
            "",
            &mono,
            &text_primary(),
            rect(14.0, 46.0, GROUP_W - 28.0, lines as f64 * 18.0),
            0,
        );
        body.setAttributedStringValue(&crate::ui::attributed_parts(&parts));
        card.addSubview(&body);
        self.doc.addSubview(&card);
        self.y.set(self.y.get() + h + 22.0);
    }

    fn permission(&self, p: Permission) {
        self.permission_as(p, p.title(), p.rationale(), None);
    }

    fn permission_as(&self, p: Permission, title: &str, rationale: &str, forced: Option<PermissionState>) {
        let state = forced.unwrap_or_else(|| permissions::state(p));
        let (word, tone) = match state {
            PermissionState::Granted => ("Allowed", Tone::Ok),
            PermissionState::NotRequired => ("Not required", Tone::Neutral),
            PermissionState::Denied => ("Not allowed", Tone::Warn),
            PermissionState::NotDetermined => ("Not asked yet", Tone::Neutral),
            PermissionState::Unknown => ("Unknown", Tone::Neutral),
        };
        let mut buttons: Vec<RowButton<'_>> = Vec::new();
        if !matches!(state, PermissionState::Granted | PermissionState::NotRequired) {
            buttons.push((
                "Allow…",
                true,
                Box::new(move || {
                    permissions::request(p);
                    app::send(Command::Refresh);
                    app::show_settings_soon();
                }),
            ));
        } else if state == PermissionState::Granted {
            buttons.push(("Open Settings", false, Box::new(move || permissions::open_settings(p))));
        }
        self.action_row(title, Some(rationale), Some((word, tone)), buttons, None);
    }
}

fn index_of<T: PartialEq>(items: &[T], v: &T) -> usize {
    items.iter().position(|x| x == v).unwrap_or(0)
}

/// The sidebar's rows and captions, placed (and re-placed by search) together.
struct SideRows {
    items: Vec<(Pane, Retained<FlippedView>)>,
    captions: Vec<Retained<NSTextField>>,
}

const NAV_TOP: f64 = 128.0;
const NAV_ITEM_H: f64 = 32.0;

impl SideRows {
    /// Lay out the captions and the rows that match `query`.
    fn place(&self, query: &str) {
        let q = query.to_lowercase();
        let mut y = NAV_TOP;
        for (si, (_, panes)) in Pane::SECTIONS.iter().enumerate() {
            let hits: Vec<Pane> = panes
                .iter()
                .copied()
                .filter(|p| q.is_empty() || p.title().to_lowercase().contains(&q) || p.keywords().contains(&q))
                .collect();
            let cap = &self.captions[si];
            cap.setHidden(hits.is_empty());
            if hits.is_empty() {
                continue;
            }
            y += 14.0;
            cap.setFrameOrigin(objc2_foundation::NSPoint::new(22.0, y));
            y += 20.0;
            for (pane, view) in &self.items {
                if panes.contains(pane) {
                    let hit = hits.contains(pane);
                    view.setHidden(!hit);
                    if hit {
                        view.setFrameOrigin(objc2_foundation::NSPoint::new(12.0, y));
                        y += NAV_ITEM_H + 2.0;
                    }
                }
            }
        }
    }
}

impl SettingsWindow {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::FullSizeContentView;
        // SAFETY: NSWindow designated initializer.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, WIDTH, HEIGHT),
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&ns("Notchisland Settings"));
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setBackgroundColor(Some(&ink()));
        // SAFETY: AppKit's appearance-name constant.
        if let Some(dark) = NSAppearance::appearanceNamed(unsafe { objc2_app_kit::NSAppearanceNameDarkAqua }) {
            window.setAppearance(Some(&dark));
        }
        // SAFETY: we own the window strongly.
        unsafe { window.setReleasedWhenClosed(false) };
        Self { window, targets: Rc::new(Targets::default()), pane: Rc::new(Cell::new(Pane::General)) }
    }

    pub fn content_view(&self) -> Option<Retained<NSView>> {
        self.window.contentView()
    }

    pub fn is_visible(&self) -> bool {
        self.window.isVisible()
    }

    pub fn close(&self) {
        self.window.orderOut(None);
    }

    pub fn set_pane(&self, pane: Pane) {
        self.pane.set(pane);
    }

    pub fn show(&self, config: &Config) {
        let mtm = MainThreadMarker::from(&*self.window);
        self.targets.clear();
        // The content draws its own ground (it must not depend on the window
        // background, which offscreen renders and some transitions skip).
        let root = shape_view(mtm, rect(0.0, 0.0, WIDTH, HEIGHT), &ink(), 0.0);
        root.addSubview(&self.sidebar(mtm));

        let pane = self.pane.get();
        self.header(mtm, &root, pane, config);
        let doc = FlippedView::new(mtm, rect(0.0, 0.0, WIDTH - SIDEBAR, 2400.0));
        let form = Form {
            sub_ok: Cell::new(false),
            mtm,
            doc: &doc,
            y: Cell::new(22.0),
            targets: &self.targets,
            draft: Rc::new(RefCell::new(config.clone())),
            group: RefCell::new(None),
        };
        build_pane(&form, pane, config);
        form.end();
        let total = form.y.get() + 6.0;
        let view_h = HEIGHT - HEADER_H;
        doc.setFrame(rect(0.0, 0.0, WIDTH - SIDEBAR, total.max(view_h)));
        let scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(SIDEBAR, HEADER_H, WIDTH - SIDEBAR, view_h));
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDocumentView(Some(&doc));
        root.addSubview(&scroll);
        doc.scrollPoint(objc2_foundation::NSPoint::new(0.0, 0.0));

        let content: &NSView = &root;
        self.window.setContentView(Some(content));
        if !self.window.isVisible() {
            self.window.center();
        }
        crate::app::activate(mtm);
        self.window.makeKeyAndOrderFront(None);
        // No text field starts focused (the search field would show a caret).
        self.window.makeFirstResponder(None);
    }

    /// The pane's icon tile, title, one-line description and status chip.
    fn header(&self, mtm: MainThreadMarker, root: &FlippedView, pane: Pane, c: &Config) {
        let x0 = SIDEBAR + CONTENT_PAD;
        let tile = gradient_tile(mtm, rect(x0, 28.0, 40.0, 40.0), pane.gradient(), 11.0);
        tile.addSubview(&crate::ui::symbol_view(
            mtm,
            pane.icon().0,
            20.0,
            &NSColor::whiteColor(),
            rect(8.0, 8.0, 24.0, 24.0),
            "",
        ));
        root.addSubview(&tile);
        let chip = pane.chip(c).map(|(t, tone)| chip_view(mtm, &t, tone));
        let chip_w = chip.as_ref().map_or(0.0, |(_, w)| w + 16.0);
        let text_x = x0 + 54.0;
        let text_w = WIDTH - CONTENT_PAD - text_x - chip_w;
        root.addSubview(&label(
            mtm,
            pane.title(),
            &font(24.0, Weight::Semibold),
            &text_primary(),
            rect(text_x, 26.0, text_w, 30.0),
        ));
        root.addSubview(&label(
            mtm,
            pane.blurb(),
            &font(13.0, Weight::Regular),
            &text_secondary(),
            rect(text_x, 57.0, text_w, 17.0),
        ));
        if let Some((chip, w)) = chip {
            chip.setFrameOrigin(objc2_foundation::NSPoint::new(WIDTH - CONTENT_PAD - w, 35.0));
            root.addSubview(&chip);
        }
        root.addSubview(&shape_view(mtm, rect(SIDEBAR, HEADER_H - 0.5, WIDTH - SIDEBAR, 0.5), &hairline(), 0.0));
    }

    fn sidebar(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let side = shape_view(mtm, rect(0.0, 0.0, SIDEBAR, HEIGHT), &side_bg(), 0.0);
        side.addSubview(&shape_view(mtm, rect(SIDEBAR - 0.5, 0.0, 0.5, HEIGHT), &hairline(), 0.0));

        // Brand: the app icon (it carries its own transparent margin) and name.
        let icon = NSImageView::new(mtm);
        icon.setImage(objc2_app_kit::NSApplication::sharedApplication(mtm).applicationIconImage().as_deref());
        icon.setImageScaling(objc2_app_kit::NSImageScaling::ScaleProportionallyUpOrDown);
        icon.setFrame(rect(12.0, 40.0, 44.0, 44.0));
        side.addSubview(&icon);
        side.addSubview(&label(
            mtm,
            "Notchisland",
            &font(14.0, Weight::Semibold),
            &text_primary(),
            rect(62.0, 49.0, SIDEBAR - 74.0, 18.0),
        ));
        side.addSubview(&label(
            mtm,
            "Settings",
            &font(11.5, Weight::Regular),
            &text_secondary(),
            rect(62.0, 67.0, SIDEBAR - 74.0, 15.0),
        ));

        // Search filters the pane list in place.
        let search = NSSearchField::initWithFrame(NSSearchField::alloc(mtm), rect(14.0, 90.0, SIDEBAR - 28.0, 28.0));
        search.setPlaceholderString(Some(&ns("Search settings")));
        crate::ui::set_accessibility_label(&search, "Search settings");

        let current = self.pane.get();
        let mut rows = SideRows { items: Vec::new(), captions: Vec::new() };
        for (cap, _) in Pane::SECTIONS {
            let c = caption(mtm, cap, 10.5, rect(22.0, 0.0, SIDEBAR - 44.0, 14.0));
            side.addSubview(&c);
            rows.captions.push(c);
        }
        for pane in Pane::ALL {
            let selected = pane == current;
            let bg = if selected { rgba(0.298, 0.553, 1.0, 0.16) } else { rgba(1.0, 1.0, 1.0, 0.0) };
            let item = shape_view(mtm, rect(12.0, 0.0, SIDEBAR - 24.0, NAV_ITEM_H), &bg, 9.0);
            if selected {
                if let Some(l) = item.layer() {
                    l.setBorderWidth(0.5);
                    l.setBorderColor(Some(&rgba(0.486, 0.667, 1.0, 0.38).CGColor()));
                }
            }
            let tile = gradient_tile(mtm, rect(9.0, 5.0, 22.0, 22.0), pane.gradient(), 6.0);
            tile.addSubview(&crate::ui::symbol_view(
                mtm,
                pane.icon().0,
                11.0,
                &NSColor::whiteColor(),
                rect(4.0, 4.0, 14.0, 14.0),
                "",
            ));
            item.addSubview(&tile);
            let (fg, weight) =
                if selected { (NSColor::whiteColor(), Weight::Semibold) } else { (text_primary(), Weight::Regular) };
            item.addSubview(&label(
                mtm,
                pane.title(),
                &font(13.0, weight),
                &fg,
                rect(41.0, 7.0, SIDEBAR - 24.0 - 52.0, 17.0),
            ));
            let pc = self.pane.clone();
            let t = self.targets.add(mtm, move |_| {
                pc.set(pane);
                // Deferred: rebuilding now would free this button's own target.
                dispatch2::DispatchQueue::main().exec_async(app::show_settings);
            });
            let b = crate::ui::click_overlay(mtm, &t, rect(0.0, 0.0, SIDEBAR - 24.0, NAV_ITEM_H), pane.title());
            item.addSubview(&b);
            if selected {
                // SAFETY: accessibility setter on NSView.
                let _: () = unsafe { objc2::msg_send![&*b, setAccessibilitySelected: true] };
            }
            side.addSubview(&item);
            rows.items.push((pane, item));
        }
        rows.place("");
        let rows = Rc::new(rows);
        let r = rows.clone();
        let t = self.targets.add(mtm, move |sender: Option<&AnyObject>| {
            let Some(f) = sender.and_then(|s| s.downcast_ref::<NSTextField>()) else { return };
            r.place(&f.stringValue().to_string());
        });
        // SAFETY: target kept alive by `targets`.
        unsafe {
            search.setTarget(Some(t.as_any()));
            search.setAction(Some(ActionTarget::action()));
        }
        side.addSubview(&search);

        // Footer: provider health and the version.
        side.addSubview(&shape_view(mtm, rect(14.0, HEIGHT - 46.0, SIDEBAR - 28.0, 0.5), &hairline(), 0.0));
        let (down, total) = app::with_controller(|c| {
            let snap = c.current_snapshot();
            (snap.providers.values().filter(|s| !s.is_healthy()).count(), snap.providers.len())
        })
        .unwrap_or((0, 0));
        let ok = down == 0 || total == 0;
        let dot = if ok { rgba(0.188, 0.82, 0.345, 1.0) } else { rgba(1.0, 0.624, 0.039, 1.0) };
        side.addSubview(&shape_view(mtm, rect(20.0, HEIGHT - 28.0, 7.0, 7.0), &dot, 3.5));
        let status = if ok { "All providers running".to_owned() } else { format!("{down} need attention") };
        side.addSubview(&label(
            mtm,
            &status,
            &font(11.5, Weight::Regular),
            &text_secondary(),
            rect(34.0, HEIGHT - 32.0, SIDEBAR - 100.0, 15.0),
        ));
        let ver = label(
            mtm,
            concat!("v", env!("CARGO_PKG_VERSION")),
            &font(11.5, Weight::Regular),
            &text_tertiary(),
            rect(SIDEBAR - 14.0 - 56.0, HEIGHT - 32.0, 56.0, 15.0),
        );
        ver.setAlignment(NSTextAlignment::Right);
        side.addSubview(&ver);
        Retained::into_super(side)
    }
}

fn build_pane(form: &Form<'_>, pane: Pane, c: &Config) {
    let prios: Vec<Priority> = PRIORITIES.iter().map(|p| p.1).collect();
    let prio_names: Vec<&str> = PRIORITIES.iter().map(|p| p.0).collect();
    match pane {
        Pane::General => {
            form.begin("");
            let login_enabled = crate::login_item::is_enabled();
            form.switch("Launch at login", None, login_enabled, |cfg, on| {
                cfg.general.launch_at_login = on;
                if let Err(e) = crate::login_item::set_enabled(on) {
                    app::alert("Launch at login", &e);
                }
            });
            form.switch("Show menu bar icon", None, c.general.show_menu_bar_icon, |cfg, on| {
                cfg.general.show_menu_bar_icon = on
            });
            let screens = [ScreenChoice::Notch, ScreenChoice::Main, ScreenChoice::Mouse];
            form.popup(
                "Show on screen",
                &["Built-in (notch)", "Main display", "Display with pointer"],
                index_of(&screens, &c.general.screen),
                move |cfg, i| cfg.general.screen = screens[i],
            );
            form.slider("Hover delay", 0.0, 600.0, c.general.hover_delay_ms as f64, |cfg, v| {
                cfg.general.hover_delay_ms = v.round() as u64
            });
            let pl = prios.clone();
            form.popup(
                "Peek for priority at least",
                &prio_names,
                index_of(&prios, &c.general.peek_min_priority),
                move |cfg, i| cfg.general.peek_min_priority = pl[i],
            );
            form.begin("Batteries");
            form.switch("Mac battery", None, c.providers.battery, |cfg, on| cfg.providers.battery = on);
            form.switch(
                "Accessory batteries",
                Some("AirPods, keyboards, mice and other connected devices"),
                c.providers.bluetooth,
                |cfg, on| cfg.providers.bluetooth = on,
            );
            form.begin("Media players");
            form.switch(
                "Spotify and Apple Music",
                Some("Now playing, controls and playlists. Needs Automation."),
                c.providers.media,
                |cfg, on| cfg.providers.media = on,
            );
            form.begin("Advanced");
            form.action_row(
                "Configuration",
                Some("Settings are stored in a TOML file you can edit by hand."),
                None,
                vec![
                    ("Open Config File", false, Box::new(app::open_config_file)),
                    ("Open Logs", false, Box::new(app::open_logs)),
                    (
                        "Reset…",
                        false,
                        Box::new(|| {
                            app::send(Command::UpdateConfig(Box::default()));
                            app::show_settings_soon();
                        }),
                    ),
                ],
                None,
            );
        }
        Pane::Appearance => {
            form.begin("");
            let motions = [ReducedMotion::System, ReducedMotion::Always, ReducedMotion::Never];
            form.popup(
                "Reduce motion",
                &["Follow system", "Always", "Never"],
                index_of(&motions, &c.appearance.reduced_motion),
                move |cfg, i| cfg.appearance.reduced_motion = motions[i],
            );
            form.slider("Animation speed", 0.5, 2.0, c.appearance.animation_speed, |cfg, v| {
                cfg.appearance.animation_speed = v
            });
            form.switch(
                "Live indicators beside the notch",
                Some("Charging, unread count, tray files and Focus in the collapsed notch"),
                c.appearance.show_wings,
                |cfg, on| cfg.appearance.show_wings = on,
            );
            form.switch("High contrast text", None, c.appearance.high_contrast, |cfg, on| {
                cfg.appearance.high_contrast = on
            });
        }
        Pane::Notifications => {
            form.begin("Behaviour");
            let pl = prios.clone();
            form.popup(
                "While Focus is on, show at least",
                &prio_names,
                index_of(&prios, &c.notifications.focus_min_priority),
                move |cfg, i| cfg.notifications.focus_min_priority = pl[i],
            );
            form.switch("Show notifications from other apps", None, c.notifications.allow_other_apps, |cfg, on| {
                cfg.notifications.allow_other_apps = on
            });
            form.switch(
                "Close the native banner",
                Some("Experimental. Uses Accessibility to dismiss a banner the notch already shows."),
                c.notifications.suppress_native_banners,
                |cfg, on| cfg.notifications.suppress_native_banners = on,
            );
            form.begin("Sources");
            form.switch(
                "Notification Center",
                Some("Reads delivered banners. Needs Full Disk Access."),
                c.providers.notification_center,
                |cfg, on| cfg.providers.notification_center = on,
            );
            form.switch(
                "Banner reader fallback",
                Some("Reads banners on screen. Needs Accessibility."),
                c.providers.accessibility_banners,
                |cfg, on| cfg.providers.accessibility_banners = on,
            );
            form.switch("Focus state", Some("Needs Full Disk Access."), c.providers.focus, |cfg, on| {
                cfg.providers.focus = on
            });
            form.begin("Apps");
            for app_kind in KnownApp::ALL {
                let key = app_kind.key();
                let rule = c.notifications.apps.get(key).cloned().unwrap_or_default();
                let k1 = key.to_owned();
                form.switch(app_kind.display_name(), None, rule.enabled, move |cfg, on| {
                    cfg.notifications.apps.entry(k1.clone()).or_default().enabled = on;
                });
            }
            form.note("Messaging apps are read from Notification Center. To avoid double banners, set an app's alert style to “None” in System Settings › Notifications; it still reaches the notch.");
        }
        Pane::Agents => {
            form.begin("Listening");
            form.switch("Coding agents", Some("Webhook, Unix socket and inbox file"), c.providers.agents, |cfg, on| {
                cfg.providers.agents = on
            });
            form.text_row(
                "Webhook port",
                Some("Loopback only"),
                &c.agents.http_port.to_string(),
                "47815",
                true,
                86.0,
                false,
                |cfg, v| {
                    if let Ok(p) = v.parse::<u16>() {
                        cfg.agents.http_port = p;
                    }
                },
            );
            form.text_row(
                "Bearer token",
                Some("Optional. Requests then need Authorization: Bearer"),
                if preview() { "board-sample" } else { c.agents.token.as_deref().unwrap_or("") },
                "Optional",
                true,
                190.0,
                true,
                |cfg, v| {
                    cfg.agents.token = if v.is_empty() { None } else { Some(v) };
                },
            );
            form.begin("Approval");
            form.switch(
                "Ask before a new agent shows notifications",
                Some("The notch asks Allow or Deny the first time an agent sends something"),
                c.agents.require_approval,
                |cfg, on| cfg.agents.require_approval = on,
            );
            form.action_row(
                "Remembered decisions",
                Some("Forget every Allow and Deny so agents are asked again"),
                None,
                vec![(
                    "Forget",
                    false,
                    Box::new(|| crate::app::send(notchforge_core::engine::Command::ForgetAgentDecisions)),
                )],
                None,
            );
            form.note(
                "This is consent, not authentication: an agent is recognised by the source name it sends. Set a bearer token to keep other local processes from posting.",
            );
            let port = c.agents.http_port;
            let cmd = format!(
                "curl -s -X POST http://127.0.0.1:{port}/v1/notify -H 'Content-Type: application/json' -d '{{\"source\":\"claude\",\"kind\":\"task_finished\",\"title\":\"Tests passed\"}}'"
            );
            let (blue, green) = (rgba(0.498, 0.69, 1.0, 1.0), rgba(0.482, 0.878, 0.627, 1.0));
            let l1 = format!(" -s -X POST http://127.0.0.1:{port}/v1/notify \\\n  -H ");
            let json = "'{\"source\":\"claude\",\"kind\":\"task_finished\",\"title\":\"Tests passed\"}'";
            form.code_block(
                "Send a test event",
                &[
                    ("curl", blue),
                    (&l1, text_primary()),
                    ("'Content-Type: application/json'", green.clone()),
                    (" \\\n  -d ", text_primary()),
                    (json, green),
                ],
                Box::new(move || {
                    crate::sharing::copy_text(&cmd);
                }),
            );
        }
        Pane::Weather => {
            form.begin("");
            form.switch("Weather", None, c.providers.weather, |cfg, on| cfg.providers.weather = on);
            form.text("Location", &c.weather.location, "auto, or a city", false, |cfg, v| {
                cfg.weather.location = if v.is_empty() { "auto".into() } else { v };
                cfg.weather.latitude = None;
                cfg.weather.longitude = None;
            });
            let units = [TemperatureUnit::Celsius, TemperatureUnit::Fahrenheit];
            form.popup("Units", &["Celsius", "Fahrenheit"], index_of(&units, &c.weather.unit), move |cfg, i| {
                cfg.weather.unit = units[i]
            });
            let refresh = [10u64, 20, 30, 60];
            form.popup(
                "Refresh every",
                &["10 minutes", "20 minutes", "30 minutes", "60 minutes"],
                index_of(&refresh, &c.weather.refresh_minutes),
                move |cfg, i| cfg.weather.refresh_minutes = refresh[i],
            );
            form.note("Forecasts come from Open-Meteo. “auto” uses your approximate location.");
        }
        Pane::Media => {
            form.begin("Beside the notch");
            form.switch(
                "Now playing beside the notch",
                Some("Cover on the left, an equaliser on the right"),
                c.media.show_in_wings,
                |cfg, on| cfg.media.show_in_wings = on,
            );
            form.switch(
                "Audio outputs and apps playing sound",
                Some("Reads CoreAudio. No permission needed."),
                c.providers.audio,
                |cfg, on| cfg.providers.audio = on,
            );
            let polls = [5u64, 10, 30, 60];
            form.popup(
                "Re-read players every",
                &["5 seconds", "10 seconds", "30 seconds", "1 minute"],
                index_of(&polls, &c.media.poll_seconds),
                move |cfg, i| cfg.media.poll_seconds = polls[i],
            );
            form.begin("Spotify");
            let signed_in = app::spotify_signed_in() || preview();
            let has_id = !c.media.spotify_client_id.is_empty();
            let (status, status_tone) =
                if signed_in { ("Connected", Tone::Ok) } else { ("Not connected", Tone::Neutral) };
            let button: RowButton<'_> = if signed_in {
                (
                    "Disconnect",
                    false,
                    Box::new(|| {
                        crate::media::send(notchforge_core::provider::ProviderCommand::SpotifyDisconnect);
                        app::show_settings_soon();
                    }),
                )
            } else {
                (
                    "Connect…",
                    true,
                    Box::new(|| crate::media::send(notchforge_core::provider::ProviderCommand::SpotifyConnect)),
                )
            };
            form.sub_ok.set(preview());
            let made = form.action_row(
                "Spotify account",
                Some(if preview() {
                    "Connected · 24 playlists"
                } else if signed_in {
                    "Your playlists appear in Now Playing."
                } else {
                    "Sign in to list your playlists."
                }),
                if preview() { None } else { Some((status, status_tone)) },
                vec![button],
                Some(("spotify", (30, 215, 96))),
            );
            if !signed_in && !has_id {
                for b in &made {
                    b.setEnabled(false);
                }
            }
            form.sub_ok.set(false);
            let client_id = if preview() { "3f9c2e7b1a4d4c0e9b8f6a21" } else { c.media.spotify_client_id.as_str() };
            form.text("Client ID", client_id, "From developer.spotify.com", true, |cfg, v| {
                cfg.media.spotify_client_id = v
            });
            form.note(&format!(
                "Only needed to list playlists; play, pause and skip work without it. Create an app at developer.spotify.com/dashboard and add the redirect URI {}.",
                c.media.spotify_redirect_uri()
            ));
            form.begin("Permissions");
            let forced =
                |ok: bool| preview().then_some(if ok { PermissionState::Granted } else { PermissionState::Denied });
            form.permission_as(
                Permission::Automation,
                "Automation · Spotify and Music",
                "Read what's playing, control playback, list playlists",
                forced(true),
            );
            form.permission_as(
                Permission::Accessibility,
                "Accessibility",
                "Play and skip in other apps through the media keys",
                forced(false),
            );
        }
        Pane::Tray => {
            form.begin("");
            form.switch(
                "Keep tray contents after quitting",
                Some("Files are referenced, never copied."),
                c.tray.persist,
                |cfg, on| cfg.tray.persist = on,
            );
        }
        Pane::Controls => {
            form.begin("Show");
            form.switch("Brightness slider", None, c.controls.brightness, |cfg, on| cfg.controls.brightness = on);
            form.switch("Volume slider", None, c.controls.volume, |cfg, on| cfg.controls.volume = on);
            form.switch("Resolution switcher", None, c.controls.resolution, |cfg, on| cfg.controls.resolution = on);
            form.begin("Focus");
            form.text("“Focus on” shortcut", &c.controls.focus_on_shortcut, "Shortcut name", false, |cfg, v| {
                cfg.controls.focus_on_shortcut = v
            });
            form.text("“Focus off” shortcut", &c.controls.focus_off_shortcut, "Shortcut name", false, |cfg, v| {
                cfg.controls.focus_off_shortcut = v
            });
            form.note(
                "Create these in the Shortcuts app with the “Set Focus” action. Notchisland runs them to toggle Focus.",
            );
        }
        Pane::Permissions => {
            form.begin("");
            for p in Permission::ALL {
                form.permission(p);
            }
            form.note("Every permission is optional. Features that need one stay off until it is granted.");
        }
    }
}
