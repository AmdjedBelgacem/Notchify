//! Settings window, built to the *Settings* board of the Notchisland redesign:
//! the island's own true black, a plain sidebar that mirrors the island (how
//! it behaves, how it looks, one page per space, then the Mac), and pages of
//! grouped lists. Every setting is one row: what it is on the left, one line
//! of why under it, its control on the right. One cyan accent (or the accent
//! the user picked) marks what is on.
//!
//! Every change applies immediately: the edited draft config is sent to the
//! engine, which persists it and broadcasts it. A page is rebuilt each time
//! it is shown so permission and connection states are always fresh.

use crate::app;
use crate::permissions;
use crate::ui::{
    ActionTarget, FlippedView, Targets, Weight, attributed, font, label, ns, rect, rgba, shape_view, text_width,
};
use crate::views::kit::{self, Btn};
use notchforge_core::apps::KnownApp;
use notchforge_core::config::{AppearanceConfig, Config, ReducedMotion, ScreenChoice};
use notchforge_core::engine::Command;
use notchforge_core::model::{MediaPlayer, Permission, PermissionState, Priority, TemperatureUnit};
use notchforge_providers::agents::connect;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSBackingStoreType, NSButton, NSColor, NSControlSize,
    NSControlStateValueOff, NSControlStateValueOn, NSScrollView, NSSearchField, NSSlider, NSTextAlignment, NSTextField,
    NSView, NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::NSPoint;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const WIDTH: f64 = 860.0;
const HEIGHT: f64 = 620.0;
const SIDEBAR: f64 = 208.0;
/// Space either side of a page's content.
const MARGIN: f64 = 28.0;
/// Width of a page's groups.
const MAIN_W: f64 = WIDTH - SIDEBAR - 2.0 * MARGIN;
/// Where a page's scrolling content starts, under its title.
const HEADER_H: f64 = 100.0;
/// Inside a group: the row's side padding.
const ROW_PAD: f64 = 14.0;
const GROUP_R: f64 = 12.0;
/// Between a group and the next section's heading.
const SECTION_GAP: f64 = 22.0;
const SWITCH_W: f64 = 38.0;
const SWITCH_H: f64 = 22.0;
const CONTROL_H: f64 = 26.0;
const BUTTON_H: f64 = 28.0;

thread_local! {
    /// Self-test only: show the design board's sample state (Spotify connected
    /// with 24 playlists, a sample token) so renders compare like for like.
    static PREVIEW: Cell<bool> = const { Cell::new(false) };
}

/// Kept for the self-test, which used to ask for an opaque ground; the window
/// is always solid black now.
pub fn set_solid_ground(_on: bool) {}

/// Turn the board sample state on or off (self-test renders).
pub fn set_preview(on: bool) {
    PREVIEW.with(|p| p.set(on));
}

fn preview() -> bool {
    PREVIEW.with(Cell::get)
}

/// Row separators, a step lighter than the group.
fn separator() -> Retained<NSColor> {
    rgba(0.137, 0.137, 0.149, 1.0)
}
/// The sidebar's edge.
fn edge() -> Retained<NSColor> {
    rgba(0.11, 0.11, 0.122, 1.0)
}
/// A switch that is off, and a slider's empty track.
fn off_track() -> Retained<NSColor> {
    rgba(0.227, 0.227, 0.243, 1.0)
}
/// Sidebar captions.
fn text4() -> Retained<NSColor> {
    rgba(0.388, 0.388, 0.4, 1.0)
}

/// Settings pages, in sidebar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Island,
    Look,
    Now,
    Music,
    Agents,
    Day,
    Shelf,
    Permissions,
    Advanced,
}

impl Pane {
    pub const ALL: [Pane; 9] = [
        Pane::Island,
        Pane::Look,
        Pane::Now,
        Pane::Music,
        Pane::Agents,
        Pane::Day,
        Pane::Shelf,
        Pane::Permissions,
        Pane::Advanced,
    ];

    /// The sidebar's sections; the first has no caption.
    const SECTIONS: [(&'static str, &'static [Pane]); 3] = [
        ("", &[Pane::Island, Pane::Look]),
        ("Spaces", &[Pane::Now, Pane::Music, Pane::Agents, Pane::Day, Pane::Shelf]),
        ("Mac", &[Pane::Permissions, Pane::Advanced]),
    ];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Island => "Island",
            Pane::Look => "Look",
            Pane::Now => "Now",
            Pane::Music => "Music",
            Pane::Agents => "Agents",
            Pane::Day => "Day",
            Pane::Shelf => "Shelf",
            Pane::Permissions => "Permissions",
            Pane::Advanced => "Advanced",
        }
    }

    /// One line under the page title.
    fn blurb(self) -> &'static str {
        match self {
            Pane::Island => "Where the island shows and how it opens.",
            Pane::Look => "The accent colour, motion and contrast.",
            Pane::Now => "Notifications and calls, and which apps may show them.",
            Pane::Music => "What plays in the island, and how music apps connect.",
            Pane::Agents => "Coding agents report to the island and ask it for permission.",
            Pane::Day => "Weather, calendar, batteries and Focus.",
            Pane::Shelf => "A shelf for files you drop on the island.",
            Pane::Permissions => "Every permission is optional; a feature waits until it is allowed.",
            Pane::Advanced => "The configuration file, logs and starting over.",
        }
    }

    /// SF Symbol in the sidebar.
    fn symbol(self) -> &'static str {
        match self {
            Pane::Island => "capsule.fill",
            Pane::Look => "paintpalette",
            Pane::Now => "bell",
            Pane::Music => "music.note",
            Pane::Agents => "terminal",
            Pane::Day => "sun.max",
            Pane::Shelf => "tray",
            Pane::Permissions => "hand.raised",
            Pane::Advanced => "gearshape",
        }
    }

    /// Extra words the sidebar search matches.
    fn keywords(self) -> &'static str {
        match self {
            Pane::Island => {
                "screen display hover delay peek glance priority fullscreen shoulders wings menu bar launch login icon"
            }
            Pane::Look => "accent colour color motion animation speed contrast",
            Pane::Now => {
                "notifications focus banners apps whatsapp discord slack messages quick reply calls ring answer facetime"
            }
            Pane::Music => "spotify apple music audio output playlists browser youtube",
            Pane::Agents => {
                "claude codex cursor gemini zed opencode webhook port token allow deny approval limits usage"
            }
            Pane::Day => "weather location units celsius fahrenheit calendar battery airpods focus schedule shortcuts",
            Pane::Shelf => "files tray drop downloads screenshots airdrop",
            Pane::Permissions => {
                "accessibility full disk access location automation privacy bluetooth calendar microphone screen recording"
            }
            Pane::Advanced => "config file toml export import logs reset",
        }
    }

    /// Live status at the right of the page title.
    fn chip(self, c: &Config) -> Option<(String, Tone)> {
        match self {
            Pane::Agents => Some(if c.providers.agents {
                (format!("Listening on {}", c.agents.http_port), Tone::Ok)
            } else {
                ("Not listening".into(), Tone::Neutral)
            }),
            Pane::Permissions => {
                let ok = Permission::ALL.iter().filter(|p| allowed(permissions::state(**p))).count();
                let all = Permission::ALL.len();
                let calm = Permission::ALL.iter().all(|p| settled(*p));
                Some((format!("{ok} of {all} allowed"), if calm { Tone::Ok } else { Tone::Warn }))
            }
            _ => None,
        }
    }
}

fn allowed(s: PermissionState) -> bool {
    matches!(s, PermissionState::Granted | PermissionState::NotRequired)
}

/// Needs nothing from the user right now: allowed, or an on-demand one
/// (microphone, screen recording) that simply hasn't been asked yet.
fn settled(p: Permission) -> bool {
    let s = permissions::state(p);
    allowed(s) || (p.on_demand() && s == PermissionState::NotDetermined)
}

/// Whether any permission still waits for the user (a dot in the sidebar).
fn permissions_need_you() -> bool {
    Permission::ALL.iter().any(|p| !settled(*p))
}

/// Meaning of a status: a dot's colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Ok,
    Warn,
    Neutral,
}

impl Tone {
    fn color(self) -> Retained<NSColor> {
        match self {
            Tone::Ok => kit::green(),
            Tone::Warn => kit::amber(),
            Tone::Neutral => kit::text3(),
        }
    }
}

/// A dot and a word. Returns the view and its width.
fn status_view(mtm: MainThreadMarker, text: &str, tone: Tone, h: f64) -> (Retained<FlippedView>, f64) {
    let f = font(12.0, Weight::Regular);
    let tw = text_width(&attributed(text, &f, &kit::text2())).ceil();
    let w = 13.0 + tw + 2.0;
    let v = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
    v.addSubview(&kit::dot(mtm, 0.0, (h - 7.0) / 2.0, 7.0, &tone.color()));
    v.addSubview(&label(mtm, text, &f, &kit::text2(), rect(13.0, (h - 16.0) / 2.0, tw + 2.0, 16.0)));
    crate::ui::set_accessibility_label(&v, text);
    (v, w)
}

/// Track colour and knob position of a switch.
fn style_switch(b: &NSButton, on: bool) {
    let Some(l) = b.layer() else { return };
    let track = if on { kit::accent() } else { off_track() };
    l.setBackgroundColor(Some(&track.CGColor()));
    let r = (SWITCH_H - 4.0) / 2.0;
    let x = if on { SWITCH_W - 2.0 - r } else { 2.0 + r };
    // SAFETY: plain read of the layer's sublayer list on the main thread.
    if let Some(subs) = unsafe { l.sublayers() } {
        for k in subs.iter() {
            k.setPosition(NSPoint::new(x, SWITCH_H / 2.0));
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

/// What leads a row: an SF Symbol, or a brand colour dot (agents, players).
enum Mark<'a> {
    Symbol(&'a str),
    Dot((u8, u8, u8)),
    Spotify,
}

/// Lays a page out top to bottom: sections of grouped rows, notes, cards.
struct Form<'a> {
    mtm: MainThreadMarker,
    doc: &'a FlippedView,
    y: Cell<f64>,
    targets: &'a Targets,
    draft: Draft,
    /// The open group and the height of its rows so far.
    group: RefCell<Option<(Retained<FlippedView>, f64)>>,
}

const PRIORITIES: [(&str, Priority); 4] = [
    ("Everything", Priority::Low),
    ("Normal and up", Priority::Normal),
    ("High and up", Priority::High),
    ("Critical only", Priority::Critical),
];

impl Form<'_> {
    fn commit(draft: &Draft) {
        app::send(Command::UpdateConfig(Box::new(draft.borrow().clone())));
    }

    /// Start a section: its heading, then a group the next rows go in.
    fn section(&self, title: &str) {
        self.end();
        if !title.is_empty() {
            self.doc.addSubview(&kit::heading(self.mtm, title, rect(MARGIN + 2.0, self.y.get(), MAIN_W, 16.0)));
            self.y.set(self.y.get() + 23.0);
        }
        let card = shape_view(self.mtm, rect(MARGIN, self.y.get(), MAIN_W, 0.0), &kit::tile(), GROUP_R);
        self.doc.addSubview(&card);
        *self.group.borrow_mut() = Some((card, 0.0));
    }

    /// Close the open group, sizing it to its rows.
    fn end(&self) {
        if let Some((card, h)) = self.group.borrow_mut().take() {
            card.setFrameSize(objc2_foundation::NSSize::new(MAIN_W, h));
            self.y.set(self.y.get() + h + SECTION_GAP);
        }
    }

    /// A row `h` tall in the open group; returns the group and the row's top.
    fn row(&self, h: f64) -> (Retained<FlippedView>, f64) {
        if self.group.borrow().is_none() {
            self.section("");
        }
        let mut g = self.group.borrow_mut();
        let (card, used) = g.as_mut().expect("a group is open");
        let top = *used;
        if top > 0.0 {
            card.addSubview(&shape_view(self.mtm, rect(ROW_PAD, top, MAIN_W - ROW_PAD, 1.0), &separator(), 0.0));
        }
        *used += h;
        (card.clone(), top)
    }

    /// A small grey paragraph under the last group.
    fn note(&self, text: &str) {
        self.end();
        let f = font(12.0, Weight::Regular);
        let w = MAIN_W - 4.0;
        let lines = (text_width(&attributed(text, &f, &kit::text3())) / (w - 12.0)).ceil().max(1.0);
        let h = lines * 17.0;
        let top = self.y.get() - SECTION_GAP + 8.0;
        let l = crate::ui::wrapping_label(self.mtm, text, &f, &kit::text3(), rect(MARGIN + 2.0, top, w, h), 5);
        self.doc.addSubview(&l);
        self.y.set(top + h + SECTION_GAP);
    }

    /// Lines a subtitle takes in `w` points.
    fn lines(subtitle: Option<&str>, w: f64) -> f64 {
        subtitle.map_or(0.0, |s| {
            (text_width(&attributed(s, &font(12.0, Weight::Regular), &kit::text2())) / (w - 4.0)).ceil().clamp(1.0, 3.0)
        })
    }

    /// Lay out a row: the optional mark, the title and subtitle, and room on
    /// the right for `control_w` points of controls. Returns the group, the
    /// row's top and its height.
    fn texts(
        &self,
        title: &str,
        subtitle: Option<&str>,
        control_w: f64,
        mark: Option<Mark<'_>>,
    ) -> (Retained<FlippedView>, f64, f64) {
        let lead = if mark.is_some() { 40.0 } else { 0.0 };
        let text_w = MAIN_W - 2.0 * ROW_PAD - lead - control_w - if control_w > 0.0 { 16.0 } else { 0.0 };
        let lines = Self::lines(subtitle, text_w);
        let block = 17.0 + if subtitle.is_some() { 3.0 + lines * 16.0 } else { 0.0 };
        let h = (block + 24.0).max(if subtitle.is_some() { 52.0 } else { 44.0 });
        let (card, top) = self.row(h);
        if let Some(m) = mark {
            let tile =
                shape_view(self.mtm, rect(ROW_PAD, top + (h - 28.0) / 2.0, 28.0, 28.0), &NSColor::blackColor(), 8.0);
            if let Some(l) = tile.layer() {
                l.setBorderWidth(1.0);
                l.setBorderColor(Some(&kit::raised().CGColor()));
            }
            match m {
                Mark::Symbol(s) => {
                    tile.addSubview(&crate::ui::symbol_view(
                        self.mtm,
                        s,
                        13.0,
                        &kit::text2(),
                        rect(6.0, 6.0, 16.0, 16.0),
                        "",
                    ));
                }
                Mark::Dot(c) => tile.addSubview(&kit::dot(self.mtm, 7.0, 7.0, 14.0, &crate::ui::rgb8(c))),
                Mark::Spotify => {
                    let disc = kit::dot(self.mtm, 5.0, 5.0, 18.0, &crate::ui::rgb8((30, 215, 96)));
                    disc.addSubview(&crate::ui::spotify_mark(self.mtm, rect(3.0, 3.0, 12.0, 12.0)));
                    tile.addSubview(&disc);
                }
            }
            card.addSubview(&tile);
        }
        let x = ROW_PAD + lead;
        let ty = top + (h - block) / 2.0;
        card.addSubview(&label(self.mtm, title, &font(13.0, Weight::Medium), &kit::text(), rect(x, ty, text_w, 17.0)));
        if let Some(s) = subtitle {
            card.addSubview(&crate::ui::wrapping_label(
                self.mtm,
                s,
                &font(12.0, Weight::Regular),
                &kit::text2(),
                rect(x, ty + 20.0, text_w, lines * 16.0),
                3,
            ));
        }
        (card, top, h)
    }

    fn switch(&self, title: &str, subtitle: Option<&str>, value: bool, set: impl Fn(&mut Config, bool) + 'static) {
        let (card, top, h) = self.texts(title, subtitle, SWITCH_W, None);
        let draft = self.draft.clone();
        let t = self.targets.add(self.mtm, move |sender: Option<&AnyObject>| {
            if let Some(b) = sender.and_then(|s| s.downcast_ref::<NSButton>()) {
                let on = b.state() == NSControlStateValueOn;
                style_switch(b, on);
                set(&mut draft.borrow_mut(), on);
                Self::commit(&draft);
            }
        });
        let sw = NSButton::new(self.mtm);
        sw.setButtonType(objc2_app_kit::NSButtonType::PushOnPushOff);
        sw.setBordered(false);
        sw.setTitle(&ns(""));
        sw.setState(if value { NSControlStateValueOn } else { NSControlStateValueOff });
        sw.setFrame(rect(MAIN_W - ROW_PAD - SWITCH_W, top + (h - SWITCH_H) / 2.0, SWITCH_W, SWITCH_H));
        sw.setWantsLayer(true);
        if let Some(l) = sw.layer() {
            l.setCornerRadius(SWITCH_H / 2.0);
            let knob = objc2_quartz_core::CALayer::new();
            knob.setBounds(rect(0.0, 0.0, SWITCH_H - 4.0, SWITCH_H - 4.0));
            knob.setCornerRadius((SWITCH_H - 4.0) / 2.0);
            knob.setBackgroundColor(Some(&NSColor::whiteColor().CGColor()));
            knob.setShadowOpacity(0.25);
            knob.setShadowRadius(1.5);
            l.addSublayer(&knob);
        }
        style_switch(&sw, value);
        // SAFETY: target kept alive by `targets`.
        unsafe {
            sw.setTarget(Some(t.as_any()));
            sw.setAction(Some(ActionTarget::action()));
        }
        crate::ui::set_accessibility_label(&sw, title);
        card.addSubview(&sw);
    }

    /// A pop-up, drawn the way System Settings draws one: the current value in
    /// secondary text with a small chevron, right-aligned in the row, no box.
    fn popup(
        &self,
        title: &str,
        subtitle: Option<&str>,
        options: &[&str],
        selected: usize,
        set: impl Fn(&mut Config, usize) + 'static,
    ) {
        use objc2_app_kit::{NSCellImagePosition, NSMenu, NSMenuItem};
        let value_font = font(13.0, Weight::Regular);
        let widest =
            options.iter().map(|o| text_width(&attributed(o, &value_font, &kit::text()))).fold(0.0_f64, f64::max);
        let (card, top, h) = self.texts(title, subtitle, (widest + 24.0).ceil(), None);
        let right = MAIN_W - ROW_PAD;
        let btn = NSButton::new(self.mtm);
        btn.setButtonType(objc2_app_kit::NSButtonType::MomentaryChange);
        btn.setBordered(false);
        btn.setImage(crate::ui::symbol_image("chevron.up.chevron.down", 9.0, "").as_deref());
        btn.setImagePosition(NSCellImagePosition::ImageRight);
        btn.setContentTintColor(Some(&kit::text3()));
        crate::ui::set_accessibility_label(&btn, title);
        // Shows `name` and hugs it, keeping the right edge on the row's margin.
        let show = {
            let f = value_font.clone();
            move |b: &NSButton, name: &str| {
                b.setAttributedTitle(&attributed(name, &f, &kit::text2()));
                b.sizeToFit();
                let w = b.frame().size.width.ceil() + 4.0;
                b.setFrame(rect(right - w, top + (h - CONTROL_H) / 2.0, w, CONTROL_H));
            }
        };
        show(&btn, options.get(selected).copied().unwrap_or(""));
        let show = Rc::new(show);

        let set = Rc::new(set);
        let menu = NSMenu::new(self.mtm);
        let items: Rc<RefCell<Vec<Retained<NSMenuItem>>>> = Rc::default();
        for (i, name) in options.iter().enumerate() {
            let (draft, set, button, items_c) = (self.draft.clone(), set.clone(), btn.clone(), items.clone());
            let (show, name_owned) = (show.clone(), (*name).to_owned());
            let t = self.targets.add(self.mtm, move |_| {
                show(&button, &name_owned);
                for (j, it) in items_c.borrow().iter().enumerate() {
                    it.setState(if j == i { NSControlStateValueOn } else { NSControlStateValueOff });
                }
                set(&mut draft.borrow_mut(), i);
                Self::commit(&draft);
            });
            // SAFETY: a valid selector (the shared action) and plain strings.
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm),
                    &ns(&crate::i18n::tr(name)),
                    Some(ActionTarget::action()),
                    &ns(""),
                )
            };
            // SAFETY: target kept alive by `targets`.
            unsafe { item.setTarget(Some(t.as_any())) };
            item.setState(if i == selected { NSControlStateValueOn } else { NSControlStateValueOff });
            menu.addItem(&item);
            items.borrow_mut().push(item);
        }
        let opener = {
            let button = btn.clone();
            let items = items.clone();
            self.targets.add(self.mtm, move |_| {
                // Like a native pop-up, the menu opens with the chosen item over the value.
                let at = items.borrow().iter().find(|it| it.state() == NSControlStateValueOn).cloned();
                menu.popUpMenuPositioningItem_atLocation_inView(at.as_deref(), NSPoint::new(-18.0, 0.0), Some(&button));
            })
        };
        // SAFETY: target kept alive by `targets`.
        unsafe {
            btn.setTarget(Some(opener.as_any()));
            btn.setAction(Some(ActionTarget::action()));
        }
        card.addSubview(&btn);
    }

    /// A slider with its value written beside it. The value follows the knob;
    /// the setting is saved when the knob is let go.
    #[allow(clippy::too_many_arguments)]
    fn slider(
        &self,
        title: &str,
        subtitle: Option<&str>,
        (min, max): (f64, f64),
        value: f64,
        show: impl Fn(f64) -> String + 'static,
        set: impl Fn(&mut Config, f64) + 'static,
    ) {
        let (sw, vw) = (170.0, 56.0);
        let (card, top, h) = self.texts(title, subtitle, sw + 8.0 + vw, None);
        let shown = label(
            self.mtm,
            &show(value),
            &crate::ui::mono_digits(12.0, Weight::Regular),
            &kit::text2(),
            rect(MAIN_W - ROW_PAD - vw, top + (h - 16.0) / 2.0, vw, 16.0),
        );
        shown.setAlignment(NSTextAlignment::Right);
        let draft = self.draft.clone();
        let (mtm, shown_c) = (self.mtm, shown.clone());
        let t = self.targets.add(self.mtm, move |sender: Option<&AnyObject>| {
            let Some(s) = sender.and_then(|s| s.downcast_ref::<NSSlider>()) else { return };
            let v = s.doubleValue();
            crate::ui::set_text(&shown_c, &show(v));
            let dragging = objc2_app_kit::NSApplication::sharedApplication(mtm)
                .currentEvent()
                .is_some_and(|e| e.r#type() == objc2_app_kit::NSEventType::LeftMouseDragged);
            if !dragging {
                set(&mut draft.borrow_mut(), v);
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
        s.setContinuous(true);
        s.setTrackFillColor(Some(&kit::accent()));
        s.setFrame(rect(MAIN_W - ROW_PAD - vw - 8.0 - sw, top + (h - 20.0) / 2.0, sw, 20.0));
        crate::ui::set_accessibility_label(&s, title);
        card.addSubview(&s);
        card.addSubview(&shown);
    }

    /// A text box on the right: black, hairline edge, `width` wide.
    #[allow(clippy::too_many_arguments)]
    fn field(
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
        let (card, top, h) = self.texts(title, subtitle, width, None);
        let frame = rect(8.0, 4.0, width - 16.0, 18.0);
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
        f.setTextColor(Some(&kit::text()));
        f.setFont(Some(&*if mono {
            objc2_app_kit::NSFont::monospacedSystemFontOfSize_weight(12.0, crate::ui::weight(Weight::Regular))
        } else {
            font(13.0, Weight::Regular)
        }));
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
        let field_box = shape_view(
            self.mtm,
            rect(MAIN_W - ROW_PAD - width, top + (h - CONTROL_H) / 2.0, width, CONTROL_H),
            &NSColor::blackColor(),
            7.0,
        );
        if let Some(l) = field_box.layer() {
            l.setBorderWidth(1.0);
            l.setBorderColor(Some(&kit::raised().CGColor()));
        }
        field_box.addSubview(&f);
        card.addSubview(&field_box);
    }

    /// A row with an optional status (dot and word) and pill buttons.
    fn action(
        &self,
        title: &str,
        subtitle: Option<&str>,
        status: Option<(&str, Tone)>,
        buttons: Vec<RowButton<'_>>,
        mark: Option<Mark<'_>>,
    ) {
        let widths: Vec<f64> = buttons.iter().map(|(t, _, _)| kit::button_width(t, BUTTON_H).max(64.0)).collect();
        let status_w = status.map_or(0.0, |(t, _)| {
            13.0 + text_width(&attributed(t, &font(12.0, Weight::Regular), &kit::text2())).ceil() + 2.0
        });
        let gaps = if status.is_some() && !buttons.is_empty() { 14.0 } else { 0.0 };
        let control_w = widths.iter().sum::<f64>() + 8.0 * (widths.len().saturating_sub(1)) as f64 + status_w + gaps;
        let (card, top, h) = self.texts(title, subtitle, control_w, mark);
        let mut x = MAIN_W - ROW_PAD;
        for ((t, primary, f), w) in buttons.into_iter().zip(widths).rev() {
            x -= w;
            let kind = if primary { Btn::Primary } else { Btn::Secondary };
            card.addSubview(&kit::button(
                self.mtm,
                self.targets,
                t,
                kind,
                rect(x, top + (h - BUTTON_H) / 2.0, w, BUTTON_H),
                f,
            ));
            x -= 8.0;
        }
        if let Some((t, tone)) = status {
            let (v, w) = status_view(self.mtm, t, tone, 18.0);
            let right = if gaps > 0.0 { x + 8.0 - gaps } else { MAIN_W - ROW_PAD };
            v.setFrameOrigin(NSPoint::new(right - w, top + (h - 18.0) / 2.0));
            card.addSubview(&v);
        }
    }

    /// The accent: one swatch per preset and a well for any other colour.
    fn accents(&self, current: &str) {
        let d = 22.0;
        let n = kit::ACCENTS.len() as f64;
        let well_w = d;
        let control_w = n * (d + 10.0) + 6.0 + well_w;
        let (card, top, h) =
            self.texts("Accent", Some("Switches, sliders and the main button in every space"), control_w, None);
        let mut x = MAIN_W - ROW_PAD - control_w;
        let rings: Rc<RefCell<Vec<Retained<FlippedView>>>> = Rc::default();
        for (i, (key, name, (r, g, b))) in kit::ACCENTS.iter().enumerate() {
            let ring = shape_view(
                self.mtm,
                rect(x, top + (h - d - 6.0) / 2.0, d + 6.0, d + 6.0),
                &NSColor::clearColor(),
                (d + 6.0) / 2.0,
            );
            if let Some(l) = ring.layer() {
                l.setBorderWidth(2.0);
                l.setBorderColor(Some(&kit::text().CGColor()));
            }
            ring.setHidden(*key != current);
            card.addSubview(&ring);
            rings.borrow_mut().push(ring);
            let sw = kit::dot(self.mtm, x + 3.0, top + (h - d) / 2.0, d, &rgba(*r, *g, *b, 1.0));
            let (draft, rings_c, value) = (self.draft.clone(), rings.clone(), (*key).to_owned());
            let t = self.targets.add(self.mtm, move |_| {
                for (j, ring) in rings_c.borrow().iter().enumerate() {
                    ring.setHidden(j != i);
                }
                draft.borrow_mut().appearance.accent = value.clone();
                Self::commit(&draft);
                app::show_settings_soon();
            });
            sw.addSubview(&crate::ui::click_overlay(self.mtm, &t, rect(0.0, 0.0, d, d), name));
            card.addSubview(&sw);
            x += d + 10.0;
        }
        let well = objc2_app_kit::NSColorWell::colorWellWithStyle(objc2_app_kit::NSColorWellStyle::Minimal, self.mtm);
        // A round swatch like the presets: grey until a colour of one's own is picked.
        let custom = current.starts_with('#');
        let ring = shape_view(
            self.mtm,
            rect(x + 3.0, top + (h - d - 6.0) / 2.0, d + 6.0, d + 6.0),
            &NSColor::clearColor(),
            (d + 6.0) / 2.0,
        );
        if let Some(l) = ring.layer() {
            l.setBorderWidth(2.0);
            l.setBorderColor(Some(&kit::text().CGColor()));
        }
        ring.setHidden(!custom);
        card.addSubview(&ring);
        rings.borrow_mut().push(ring);
        well.setFrame(rect(x + 6.0, top + (h - d) / 2.0, d, d));
        well.setWantsLayer(true);
        if let Some(l) = well.layer() {
            l.setCornerRadius(d / 2.0);
            l.setMasksToBounds(true);
        }
        well.setColor(&crate::ui::hex_to_color(if custom { current } else { "#8E8E93" }));
        crate::ui::set_accessibility_label(&well, "Custom accent colour");
        let (draft, rings_c) = (self.draft.clone(), rings.clone());
        let t = self.targets.add(self.mtm, move |sender: Option<&AnyObject>| {
            if let Some(w) = sender.and_then(|s| s.downcast_ref::<objc2_app_kit::NSColorWell>()) {
                let rings = rings_c.borrow();
                for (j, ring) in rings.iter().enumerate() {
                    ring.setHidden(j + 1 != rings.len());
                }
                draft.borrow_mut().appearance.accent = crate::ui::color_to_hex(&w.color());
                Self::commit(&draft);
            }
        });
        // SAFETY: target kept alive by `targets`.
        unsafe {
            well.setTarget(Some(t.as_any()));
            well.setAction(Some(ActionTarget::action()));
        }
        card.addSubview(&well);
    }

    /// A monospaced snippet on black, with a Copy button.
    fn code(&self, title: &str, runs: &[(&str, Retained<NSColor>)], copy: Box<dyn Fn()>) {
        self.end();
        let lines = 1 + runs.iter().map(|(t, _)| t.matches('\n').count()).sum::<usize>();
        let h = 44.0 + lines as f64 * 18.0 + 14.0;
        let card = shape_view(self.mtm, rect(MARGIN, self.y.get(), MAIN_W, h), &NSColor::blackColor(), GROUP_R);
        if let Some(l) = card.layer() {
            l.setBorderWidth(1.0);
            l.setBorderColor(Some(&kit::raised().CGColor()));
        }
        card.addSubview(&kit::heading(self.mtm, title, rect(ROW_PAD, 14.0, MAIN_W - 120.0, 16.0)));
        let w = kit::button_width("Copy", 26.0);
        card.addSubview(&kit::button(
            self.mtm,
            self.targets,
            "Copy",
            Btn::Secondary,
            rect(MAIN_W - 10.0 - w, 9.0, w, 26.0),
            copy,
        ));
        let mono = objc2_app_kit::NSFont::monospacedSystemFontOfSize_weight(11.5, crate::ui::weight(Weight::Regular));
        let parts: Vec<(&str, &objc2_app_kit::NSFont, &NSColor)> =
            runs.iter().map(|(t, c)| (*t, &*mono, &**c)).collect();
        let body = crate::ui::wrapping_label(
            self.mtm,
            "",
            &mono,
            &kit::text(),
            rect(ROW_PAD, 44.0, MAIN_W - 2.0 * ROW_PAD, lines as f64 * 18.0),
            0,
        );
        body.setAttributedStringValue(&crate::ui::attributed_parts(&parts));
        card.addSubview(&body);
        self.doc.addSubview(&card);
        self.y.set(self.y.get() + h + SECTION_GAP);
    }

    fn permission(&self, p: Permission, title: &str, why: &str, forced: Option<PermissionState>) {
        let state = forced.unwrap_or_else(|| permissions::state(p));
        let (word, tone) = match state {
            PermissionState::Granted => ("Allowed", Tone::Ok),
            PermissionState::NotRequired => ("Not needed", Tone::Neutral),
            PermissionState::Denied => ("Not allowed", Tone::Warn),
            PermissionState::NotDetermined => ("Not asked yet", Tone::Neutral),
            PermissionState::Unknown => ("Unknown", Tone::Neutral),
        };
        let mut buttons: Vec<RowButton<'_>> = Vec::new();
        if !allowed(state) {
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
            buttons.push(("Open", false, Box::new(move || permissions::open_settings(p))));
        }
        let symbol = match p {
            Permission::Accessibility => "accessibility",
            Permission::FullDiskAccess => "internaldrive",
            Permission::Location => "location",
            Permission::Notifications => "bell.badge",
            Permission::Bluetooth => "dot.radiowaves.left.and.right",
            Permission::Automation => "gearshape.2",
            Permission::Calendar => "calendar",
            Permission::Microphone => "mic",
            Permission::ScreenRecording => "rectangle.dashed.badge.record",
        };
        self.action(title, Some(why), Some((word, tone)), buttons, Some(Mark::Symbol(symbol)));
    }
}

fn index_of<T: PartialEq>(items: &[T], v: &T) -> usize {
    items.iter().position(|x| x == v).unwrap_or(0)
}

/// The sidebar's rows and captions, placed (and re-placed by search) together.
struct SideRows {
    items: Vec<(Pane, Retained<FlippedView>)>,
    captions: Vec<Option<Retained<NSTextField>>>,
}

const NAV_TOP: f64 = 84.0;
const NAV_ITEM_H: f64 = 30.0;

impl SideRows {
    /// Lay out the captions and the rows that match `query`.
    fn place(&self, query: &str) {
        let q = query.trim().to_lowercase();
        let mut y = NAV_TOP;
        for (si, (_, panes)) in Pane::SECTIONS.iter().enumerate() {
            let hits: Vec<Pane> = panes
                .iter()
                .copied()
                .filter(|p| q.is_empty() || p.title().to_lowercase().contains(&q) || p.keywords().contains(&q))
                .collect();
            if let Some(cap) = &self.captions[si] {
                cap.setHidden(hits.is_empty());
                if !hits.is_empty() {
                    y += 12.0;
                    cap.setFrameOrigin(NSPoint::new(20.0, y));
                    y += 20.0;
                }
            }
            for (pane, view) in &self.items {
                if panes.contains(pane) {
                    let hit = hits.contains(pane);
                    view.setHidden(!hit);
                    if hit {
                        view.setFrameOrigin(NSPoint::new(10.0, y));
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
        window.setMovableByWindowBackground(true);
        window.setBackgroundColor(Some(&NSColor::blackColor()));
        // SAFETY: AppKit's appearance-name constant.
        if let Some(dark) = NSAppearance::appearanceNamed(unsafe { objc2_app_kit::NSAppearanceNameDarkAqua }) {
            window.setAppearance(Some(&dark));
        }
        // SAFETY: we own the window strongly.
        unsafe { window.setReleasedWhenClosed(false) };
        Self { window, targets: Rc::new(Targets::default()), pane: Rc::new(Cell::new(Pane::Island)) }
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
        // Controls tint with the accent the user picked.
        kit::set_accent(&config.appearance.accent);
        crate::i18n::set_language(&config.appearance.language);
        let root = shape_view(mtm, rect(0.0, 0.0, WIDTH, HEIGHT), &NSColor::blackColor(), 0.0);
        root.addSubview(&self.sidebar(mtm));

        let pane = self.pane.get();
        self.header(mtm, &root, pane, config);
        let doc = FlippedView::new(mtm, rect(0.0, 0.0, WIDTH - SIDEBAR, 2400.0));
        let form = Form {
            mtm,
            doc: &doc,
            y: Cell::new(4.0),
            targets: &self.targets,
            draft: Rc::new(RefCell::new(config.clone())),
            group: RefCell::new(None),
        };
        build_pane(&form, pane, config);
        form.end();
        let total = form.y.get() + 8.0;
        let view_h = HEIGHT - HEADER_H;
        doc.setFrame(rect(0.0, 0.0, WIDTH - SIDEBAR, total.max(view_h)));
        let scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(SIDEBAR, HEADER_H, WIDTH - SIDEBAR, view_h));
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setScrollerStyle(objc2_app_kit::NSScrollerStyle::Overlay);
        scroll.setDocumentView(Some(&doc));
        root.addSubview(&scroll);
        doc.scrollPoint(NSPoint::new(0.0, 0.0));

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

    /// The page's title, one line under it, and its live status.
    fn header(&self, mtm: MainThreadMarker, root: &FlippedView, pane: Pane, c: &Config) {
        let x0 = SIDEBAR + MARGIN;
        let chip = pane.chip(c).map(|(t, tone)| {
            let f = font(12.0, Weight::Semibold);
            let tw = text_width(&attributed(&t, &f, &kit::text())).ceil();
            let w = 11.0 + 7.0 + 7.0 + tw + 11.0;
            let v = shape_view(mtm, rect(WIDTH - MARGIN - w, 44.0, w, 24.0), &kit::tile(), 12.0);
            v.addSubview(&kit::dot(mtm, 11.0, 8.5, 7.0, &tone.color()));
            v.addSubview(&label(mtm, &t, &f, &kit::text(), rect(25.0, 4.0, tw + 2.0, 16.0)));
            crate::ui::set_accessibility_label(&v, &t);
            (v, w)
        });
        let text_w = WIDTH - MARGIN - x0 - chip.as_ref().map_or(0.0, |(_, w)| w + 16.0);
        root.addSubview(&label(
            mtm,
            pane.title(),
            &font(22.0, Weight::Semibold),
            &kit::text(),
            rect(x0, 38.0, text_w, 28.0),
        ));
        root.addSubview(&label(
            mtm,
            pane.blurb(),
            &font(13.0, Weight::Regular),
            &kit::text2(),
            rect(x0, 68.0, text_w, 17.0),
        ));
        if let Some((v, _)) = chip {
            root.addSubview(&v);
        }
    }

    fn sidebar(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let side = FlippedView::new(mtm, rect(0.0, 0.0, SIDEBAR, HEIGHT));
        side.addSubview(&shape_view(mtm, rect(SIDEBAR - 1.0, 0.0, 1.0, HEIGHT), &edge(), 0.0));

        // Search filters the page list in place. (The traffic lights sit above it.)
        let search = NSSearchField::initWithFrame(NSSearchField::alloc(mtm), rect(10.0, 44.0, SIDEBAR - 20.0, 28.0));
        search.setPlaceholderString(Some(&ns("Search")));
        crate::ui::set_accessibility_label(&search, "Search settings");

        let current = self.pane.get();
        let attention = permissions_need_you();
        let mut rows = SideRows { items: Vec::new(), captions: Vec::new() };
        for (cap, _) in Pane::SECTIONS {
            rows.captions.push((!cap.is_empty()).then(|| {
                let c = label(mtm, cap, &font(11.0, Weight::Semibold), &text4(), rect(20.0, 0.0, SIDEBAR - 40.0, 14.0));
                side.addSubview(&c);
                c
            }));
        }
        let item_w = SIDEBAR - 20.0;
        for pane in Pane::ALL {
            let selected = pane == current;
            let bg = if selected { kit::raised() } else { NSColor::clearColor() };
            let item = shape_view(mtm, rect(10.0, 0.0, item_w, NAV_ITEM_H), &bg, 8.0);
            let tint = if selected { kit::accent() } else { kit::text3() };
            item.addSubview(&crate::ui::symbol_view(mtm, pane.symbol(), 13.0, &tint, rect(9.0, 7.0, 16.0, 16.0), ""));
            let (fg, weight) =
                if selected { (kit::text(), Weight::Semibold) } else { (rgba(0.82, 0.82, 0.84, 1.0), Weight::Regular) };
            item.addSubview(&label(mtm, pane.title(), &font(13.0, weight), &fg, rect(34.0, 6.5, item_w - 60.0, 17.0)));
            if pane == Pane::Permissions && attention {
                item.addSubview(&kit::dot(mtm, item_w - 18.0, 11.5, 7.0, &kit::amber()));
            }
            let pc = self.pane.clone();
            let t = self.targets.add(mtm, move |_| {
                pc.set(pane);
                // Deferred: rebuilding now would free this button's own target.
                dispatch2::DispatchQueue::main().exec_async(app::show_settings);
            });
            let b = crate::ui::click_overlay(mtm, &t, rect(0.0, 0.0, item_w, NAV_ITEM_H), pane.title());
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

        // Footer: whether every source is running, and the build.
        let (down, total) = app::with_controller(|c| {
            let snap = c.current_snapshot();
            (snap.providers.values().filter(|s| !s.is_healthy()).count(), snap.providers.len())
        })
        .unwrap_or((0, 0));
        let ok = down == 0 || total == 0;
        let fy = HEIGHT - 30.0;
        side.addSubview(&kit::dot(mtm, 20.0, fy + 4.5, 7.0, &*if ok { kit::green() } else { kit::amber() }));
        let status = if ok { "All sources running".to_owned() } else { format!("{down} need attention") };
        side.addSubview(&label(mtm, &status, &font(12.0, Weight::Regular), &kit::text3(), rect(33.0, fy, 110.0, 16.0)));
        let build = objc2_foundation::NSBundle::mainBundle()
            .objectForInfoDictionaryKey(&ns("CFBundleVersion"))
            .and_then(|v| v.downcast::<objc2_foundation::NSString>().ok())
            .map(|v| v.to_string());
        let ver_text = match build {
            Some(b) => format!("{} ({b})", env!("CARGO_PKG_VERSION")),
            None => env!("CARGO_PKG_VERSION").to_owned(),
        };
        let ver =
            label(mtm, &ver_text, &font(12.0, Weight::Regular), &text4(), rect(SIDEBAR - 20.0 - 60.0, fy, 60.0, 16.0));
        ver.setAlignment(NSTextAlignment::Right);
        side.addSubview(&ver);
        Retained::into_super(side)
    }
}

/// A row button that re-opens Settings after `f`, so its state is fresh.
fn and_refresh(f: impl Fn() + 'static) -> Box<dyn Fn()> {
    Box::new(move || {
        f();
        app::show_settings_soon();
    })
}

fn build_pane(form: &Form<'_>, pane: Pane, c: &Config) {
    let prios: Vec<Priority> = PRIORITIES.iter().map(|p| p.1).collect();
    let prio_names: Vec<&str> = PRIORITIES.iter().map(|p| p.0).collect();
    match pane {
        Pane::Island => {
            form.section("Where it shows");
            let screens = [ScreenChoice::Notch, ScreenChoice::Main, ScreenChoice::Mouse];
            form.popup(
                "Screen",
                None,
                &["Built-in (notch)", "Main display", "Display with pointer"],
                index_of(&screens, &c.general.screen),
                move |cfg, i| cfg.general.screen = screens[i],
            );
            form.switch(
                "Copy on other screens",
                Some("A display-only island at the top of each other screen"),
                c.general.island_on_all_screens,
                |cfg, on| cfg.general.island_on_all_screens = on,
            );
            form.switch(
                "Hide in fullscreen apps",
                Some("So the menu bar stays usable. A ringing call still shows"),
                c.general.hide_in_fullscreen,
                |cfg, on| cfg.general.hide_in_fullscreen = on,
            );

            form.section("Opening");
            form.slider(
                "Open on hover after",
                None,
                (0.0, 600.0),
                c.general.hover_delay_ms as f64,
                |v| format!("{} ms", (v / 10.0).round() as u64 * 10),
                |cfg, v| cfg.general.hover_delay_ms = (v / 10.0).round() as u64 * 10,
            );
            let pl = prios.clone();
            form.popup(
                "Glance for",
                Some("A notification below this waits quietly in Now"),
                &prio_names,
                index_of(&prios, &c.general.peek_min_priority),
                move |cfg, i| cfg.general.peek_min_priority = pl[i],
            );

            form.section("Shoulders");
            form.switch(
                "Live status beside the notch",
                Some("A timer, charging, the playing song or a waiting agent"),
                c.appearance.show_wings,
                |cfg, on| cfg.appearance.show_wings = on,
            );
            form.switch(
                "Playing music in the shoulders",
                Some("Cover art on the left, a level meter on the right"),
                c.media.show_in_wings,
                |cfg, on| cfg.media.show_in_wings = on,
            );
            form.slider(
                "Shoulder width",
                Some("How far the closed island reaches each side of the notch"),
                (AppearanceConfig::WING_MIN, AppearanceConfig::WING_MAX),
                c.appearance.wing(),
                |v| format!("{} pt", v.round() as u64),
                |cfg, v| cfg.appearance.wing_width = v.round(),
            );
            form.switch(
                "Stay clear of menu bar items",
                Some("A shoulder with no room is hidden, not drawn over them"),
                c.appearance.dodge_menu_bar,
                |cfg, on| cfg.appearance.dodge_menu_bar = on,
            );

            form.section("External displays");
            form.slider(
                "Notch width",
                Some("The floating notch on displays without a hardware one"),
                (AppearanceConfig::EXTERNAL_MIN, AppearanceConfig::EXTERNAL_MAX),
                c.appearance.external_notch(),
                |v| format!("{} pt", v.round() as u64),
                |cfg, v| cfg.appearance.external_width = v.round(),
            );
            form.slider(
                "Shoulder width",
                Some("How far it reaches each side on those displays"),
                (AppearanceConfig::WING_MIN, AppearanceConfig::WING_MAX),
                c.appearance.external_wing(),
                |v| format!("{} pt", v.round() as u64),
                |cfg, v| cfg.appearance.external_wing_width = v.round(),
            );

            form.section("Startup");
            form.switch("Open at login", None, crate::login_item::is_enabled(), |cfg, on| {
                cfg.general.launch_at_login = on;
                if let Err(e) = crate::login_item::set_enabled(on) {
                    app::alert("Open at login", &e);
                }
            });
            form.switch(
                "Menu bar icon",
                Some("Settings, pause and quit, and a small player"),
                c.general.show_menu_bar_icon,
                |cfg, on| cfg.general.show_menu_bar_icon = on,
            );
        }
        Pane::Look => {
            form.section("Language");
            let langs: Vec<&str> = crate::i18n::CHOICES.iter().map(|(_, name)| *name).collect();
            let at = crate::i18n::CHOICES.iter().position(|(v, _)| *v == c.appearance.language).unwrap_or(0);
            form.popup(
                "Language",
                Some("Notchisland's own text; Arabic keeps the left-to-right layout"),
                &langs,
                at,
                |cfg, i| {
                    cfg.appearance.language = crate::i18n::CHOICES[i].0.to_owned();
                },
            );
            form.section("Theme");
            let themes = ["Black", "Liquid Glass"];
            form.popup(
                "Island",
                Some("Liquid Glass flows out of the black notch as the island opens"),
                &themes,
                usize::from(c.appearance.theme == "glass"),
                |cfg, i| cfg.appearance.theme = if i == 1 { "glass" } else { "black" }.to_owned(),
            );
            form.section("Colour");
            form.accents(&c.appearance.accent);
            form.switch(
                "Higher contrast",
                Some("Brighter secondary text and surfaces"),
                c.appearance.high_contrast,
                |cfg, on| cfg.appearance.high_contrast = on,
            );
            form.section("Motion");
            let motions = [ReducedMotion::System, ReducedMotion::Always, ReducedMotion::Never];
            form.popup(
                "Reduce motion",
                Some("Fade instead of spring when the island opens"),
                &["Follow the Mac", "Always", "Never"],
                index_of(&motions, &c.appearance.reduced_motion),
                move |cfg, i| cfg.appearance.reduced_motion = motions[i],
            );
            form.slider(
                "Animation speed",
                None,
                (0.5, 2.0),
                c.appearance.animation_speed,
                |v| format!("{:.1}×", (v * 10.0).round() / 10.0),
                |cfg, v| cfg.appearance.animation_speed = (v * 10.0).round() / 10.0,
            );
        }
        Pane::Now => {
            form.section("Notifications");
            form.switch(
                "Show notifications from apps",
                Some("Messages, mail and anything else in Notification Center"),
                c.notifications.allow_other_apps,
                |cfg, on| cfg.notifications.allow_other_apps = on,
            );
            let pl = prios.clone();
            form.popup(
                "During Focus, show",
                None,
                &prio_names,
                index_of(&prios, &c.notifications.focus_min_priority),
                move |cfg, i| cfg.notifications.focus_min_priority = pl[i],
            );
            form.switch(
                "Quick reply",
                Some("Answer Messages, Discord and others from the island"),
                c.notifications.quick_reply,
                |cfg, on| cfg.notifications.quick_reply = on,
            );
            form.switch(
                "Close the Mac's own banner",
                Some("Experimental. When the island already shows it. Needs Accessibility"),
                c.notifications.suppress_native_banners,
                |cfg, on| cfg.notifications.suppress_native_banners = on,
            );

            form.section("Calls");
            form.switch(
                "Ring in the island",
                Some("Calls from apps and browser tabs, with answer, decline and hang up"),
                c.notifications.incoming_calls,
                |cfg, on| cfg.notifications.incoming_calls = on,
            );
            form.action(
                "Try it",
                Some("Rings a sample call"),
                None,
                vec![("Test call", false, Box::new(|| app::send(Command::TestCall)))],
                None,
            );

            form.section("Read from");
            form.switch(
                "Notification Center",
                Some("Delivered notifications. Needs Full Disk Access"),
                c.providers.notification_center,
                |cfg, on| cfg.providers.notification_center = on,
            );
            form.switch(
                "Banners on screen",
                Some("A fallback that reads banners as they appear. Needs Accessibility"),
                c.providers.accessibility_banners,
                |cfg, on| cfg.providers.accessibility_banners = on,
            );
            form.switch(
                "Focus",
                Some("Whether a Focus is on. Needs Full Disk Access"),
                c.providers.focus,
                |cfg, on| cfg.providers.focus = on,
            );

            form.section("Apps");
            for app_kind in KnownApp::ALL {
                let key = app_kind.key().to_owned();
                let rule = c.notifications.apps.get(&key).cloned().unwrap_or_default();
                form.switch(app_kind.display_name(), None, rule.enabled, move |cfg, on| {
                    cfg.notifications.apps.entry(key.clone()).or_default().enabled = on;
                });
            }
            form.note("To avoid seeing a message twice, set the app's alert style to None in System Settings › Notifications. It still reaches the island.");
        }
        Pane::Music => {
            form.section("Players");
            form.switch(
                "Spotify and Apple Music",
                Some("What's playing, controls and playlists"),
                c.providers.media,
                |cfg, on| cfg.providers.media = on,
            );
            let signed_in = app::spotify_signed_in() || preview();
            form.action(
                "Spotify",
                Some(if preview() {
                    "Signed in · 24 playlists"
                } else if signed_in {
                    "Your playlists show in Music"
                } else {
                    "Sign in to list your playlists. Playback works without it"
                }),
                Some(if signed_in { ("Signed in", Tone::Ok) } else { ("Not signed in", Tone::Neutral) }),
                vec![if signed_in {
                    (
                        "Sign out",
                        false,
                        and_refresh(|| {
                            crate::media::send(notchforge_core::provider::ProviderCommand::SpotifyDisconnect)
                        }),
                    )
                } else {
                    (
                        "Sign in…",
                        true,
                        Box::new(|| crate::media::send(notchforge_core::provider::ProviderCommand::SpotifyConnect)),
                    )
                }],
                Some(Mark::Spotify),
            );
            {
                let snap = app::with_controller(|c| c.current_snapshot());
                let installed = crate::media::is_installed(MediaPlayer::Music);
                let running = snap.as_deref().is_some_and(|s| s.media.running.contains(&MediaPlayer::Music));
                let automation = permissions::state(Permission::Automation);
                let (word, tone, detail, buttons): (&str, Tone, &str, Vec<RowButton<'_>>) = if !installed {
                    ("Not installed", Tone::Neutral, "The Music app isn't on this Mac", vec![])
                } else if preview() || (running && automation == PermissionState::Granted) {
                    ("Connected", Tone::Ok, "What's playing, controls and playlists", vec![])
                } else if running && automation == PermissionState::Denied {
                    (
                        "Not allowed",
                        Tone::Warn,
                        "Automation for Music was turned off in System Settings",
                        vec![("Open", true, Box::new(|| permissions::open_settings(Permission::Automation)))],
                    )
                } else {
                    (
                        "Not connected",
                        Tone::Neutral,
                        "Opens Music and asks the Mac to let the island control it",
                        vec![("Connect", true, and_refresh(crate::media::connect_music))],
                    )
                };
                form.action("Apple Music", Some(detail), Some((word, tone)), buttons, Some(Mark::Dot((250, 45, 72))));
            }

            form.section("More sound");
            form.switch(
                "Browser tabs",
                Some("YouTube, SoundCloud and other tabs play as music. The Mac asks once per browser"),
                c.media.browser_audio,
                |cfg, on| cfg.media.browser_audio = on,
            );
            form.switch(
                "Outputs and apps playing sound",
                Some("Choose where sound goes from Music. No permission needed"),
                c.providers.audio,
                |cfg, on| cfg.providers.audio = on,
            );
            let polls = [5u64, 10, 30, 60];
            form.popup(
                "Check players every",
                None,
                &["5 seconds", "10 seconds", "30 seconds", "Minute"],
                index_of(&polls, &c.media.poll_seconds),
                move |cfg, i| cfg.media.poll_seconds = polls[i],
            );
        }
        Pane::Agents => {
            form.section("Connected agents");
            let env = connect::Env::current(c.agents.http_port, c.agents.token.clone());
            for def in connect::AGENTS {
                let color = match def.key {
                    "claude" => (217, 119, 87),
                    "chatgpt" => (16, 163, 127),
                    "gemini" => (66, 133, 244),
                    _ => (142, 142, 147),
                };
                let (def, env) = (*def, env.clone());
                let (word, tone, detail, button): (&str, Tone, String, RowButton<'_>) =
                    match connect::status(&def, &env) {
                        connect::Status::Connected => (
                            "Connected",
                            Tone::Ok,
                            "Reports finished turns and asks for input".to_owned(),
                            (
                                "Disconnect",
                                false,
                                and_refresh(move || {
                                    if let Err(e) = connect::disconnect(&def, &env) {
                                        app::alert(&format!("Disconnect {}", def.name), &e);
                                    }
                                }),
                            ),
                        ),
                        connect::Status::NotConnected => (
                            "",
                            Tone::Neutral,
                            def.detail.to_owned(),
                            (
                                "Connect",
                                true,
                                and_refresh(move || match connect::connect(&def, &env) {
                                    Ok(note) => app::alert(&format!("{} connected", def.name), &note),
                                    Err(e) => app::alert(&format!("Connect {}", def.name), &e),
                                }),
                            ),
                        ),
                        connect::Status::Blocked(why) => {
                            ("Needs you", Tone::Warn, why, ("Check again", false, Box::new(app::show_settings_soon)))
                        }
                    };
                let status = (!word.is_empty()).then_some((word, tone));
                form.action(def.name, Some(&detail), status, vec![button], Some(Mark::Dot(color)));
            }
            let connected = app::claude_limits_connected();
            form.action(
                "Claude Code limits",
                Some("Reads them from its status line, which hides hints such as \u{201c}esc to interrupt\u{201d}"),
                connected.then_some(("Connected", Tone::Ok)),
                vec![if connected {
                    ("Disconnect", false, and_refresh(app::disconnect_claude_limits))
                } else {
                    ("Connect", true, and_refresh(app::connect_claude_limits))
                }],
                Some(Mark::Symbol("gauge.with.needle")),
            );
            form.note("Connecting edits that agent's own settings (a .notchforge.bak copy is kept) so it reports here. It signs in to nothing.");

            form.section("Asking");
            form.field(
                "Ask in the island for",
                Some("Seconds before the terminal asks instead. 0 turns it off"),
                &c.agents.permission_wait_secs.to_string(),
                "45",
                true,
                64.0,
                false,
                |cfg, v| {
                    if let Ok(n) = v.parse::<u64>() {
                        cfg.agents.permission_wait_secs = n.min(570);
                    }
                },
            );
            form.switch(
                "Approve new agents first",
                Some("The island asks the first time an agent sends something"),
                c.agents.require_approval,
                |cfg, on| cfg.agents.require_approval = on,
            );
            form.action(
                "Remembered answers",
                Some("Forget every Allow and Deny so agents are asked again"),
                None,
                vec![("Forget", false, Box::new(|| app::send(Command::ForgetAgentDecisions)))],
                None,
            );

            form.section("Reporting");
            form.switch(
                "Say when a session ends",
                Some("Claude Code, Codex, OpenCode, Cursor Agent, Gemini CLI and others, with no setup"),
                c.agents.watch_sessions,
                |cfg, on| cfg.agents.watch_sessions = on,
            );
            form.switch(
                "Token usage",
                Some("Counts from Claude Code and Codex logs on this Mac. Nothing is kept or sent"),
                c.agents.token_usage,
                |cfg, on| cfg.agents.token_usage = on,
            );

            form.section("Listening");
            form.switch(
                "Listen for agents",
                Some("A local webhook, a Unix socket and an inbox file"),
                c.providers.agents,
                |cfg, on| cfg.providers.agents = on,
            );
            form.field(
                "Port",
                Some("This Mac only"),
                &c.agents.http_port.to_string(),
                "47815",
                true,
                80.0,
                false,
                |cfg, v| {
                    if let Ok(p) = v.parse::<u16>() {
                        cfg.agents.http_port = p;
                    }
                },
            );
            form.field(
                "Bearer token",
                Some("Optional. Keeps other programs on this Mac from posting"),
                if preview() { "board-sample" } else { c.agents.token.as_deref().unwrap_or("") },
                "None",
                true,
                170.0,
                true,
                |cfg, v| cfg.agents.token = (!v.is_empty()).then_some(v),
            );
            let port = c.agents.http_port;
            let cmd = format!(
                "curl -s -X POST http://127.0.0.1:{port}/v1/notify -H 'Content-Type: application/json' -d '{{\"source\":\"claude\",\"kind\":\"task_finished\",\"title\":\"Tests passed\"}}'"
            );
            let (blue, green) = (kit::accent(), rgba(0.482, 0.878, 0.627, 1.0));
            let l1 = format!(" -s -X POST http://127.0.0.1:{port}/v1/notify \\\n  -H ");
            let json = "'{\"source\":\"claude\",\"kind\":\"task_finished\",\"title\":\"Tests passed\"}'";
            form.code(
                "Send a test event",
                &[
                    ("curl", blue),
                    (&l1, kit::text()),
                    ("'Content-Type: application/json'", green.clone()),
                    (" \\\n  -d ", kit::text()),
                    (json, green),
                ],
                Box::new(move || crate::sharing::copy_text(&cmd)),
            );
        }
        Pane::Day => {
            form.section("Weather");
            form.switch("Weather", None, c.providers.weather, |cfg, on| cfg.providers.weather = on);
            form.field(
                "Location",
                Some("“auto” uses your approximate location"),
                &c.weather.location,
                "auto, or a city",
                false,
                170.0,
                false,
                |cfg, v| {
                    cfg.weather.location = if v.is_empty() { "auto".into() } else { v };
                    cfg.weather.latitude = None;
                    cfg.weather.longitude = None;
                },
            );
            let units = [TemperatureUnit::Celsius, TemperatureUnit::Fahrenheit];
            form.popup("Units", None, &["Celsius", "Fahrenheit"], index_of(&units, &c.weather.unit), move |cfg, i| {
                cfg.weather.unit = units[i]
            });
            let refresh = [10u64, 20, 30, 60];
            form.popup(
                "Refresh every",
                None,
                &["10 minutes", "20 minutes", "30 minutes", "Hour"],
                index_of(&refresh, &c.weather.refresh_minutes),
                move |cfg, i| cfg.weather.refresh_minutes = refresh[i],
            );
            form.note("Forecasts come from Open-Meteo.");

            form.section("Calendar");
            form.switch(
                "Today's events",
                Some("From the calendar accounts on this Mac"),
                c.providers.calendar,
                |cfg, on| cfg.providers.calendar = on,
            );
            form.switch(
                "Focus during meetings",
                Some("On 15 minutes before a meeting, off when it ends"),
                c.controls.auto_focus_meetings,
                |cfg, on| cfg.controls.auto_focus_meetings = on,
            );

            form.section("Batteries");
            form.switch("This Mac", None, c.providers.battery, |cfg, on| cfg.providers.battery = on);
            form.switch(
                "Accessories",
                Some("AirPods, keyboards, mice and other connected devices"),
                c.providers.bluetooth,
                |cfg, on| cfg.providers.bluetooth = on,
            );

            form.section("Focus");
            const HOURS: [&str; 25] = [
                "Off", "00:00", "01:00", "02:00", "03:00", "04:00", "05:00", "06:00", "07:00", "08:00", "09:00",
                "10:00", "11:00", "12:00", "13:00", "14:00", "15:00", "16:00", "17:00", "18:00", "19:00", "20:00",
                "21:00", "22:00", "23:00",
            ];
            let (from, to) = c.controls.focus_schedule.unwrap_or((0, 0));
            let idx = |h: u8| if c.controls.focus_schedule.is_none() { 0 } else { h as usize + 1 };
            form.popup("Every day from", None, &HOURS, idx(from), move |cfg, i| {
                cfg.controls.focus_schedule = (i > 0).then(|| (i as u8 - 1, to));
            });
            form.popup("Until", None, &HOURS, idx(to), move |cfg, i| {
                cfg.controls.focus_schedule = (i > 0).then(|| (from, i as u8 - 1));
            });
            form.field(
                "Focus on shortcut",
                None,
                &c.controls.focus_on_shortcut,
                "Shortcut name",
                false,
                170.0,
                false,
                |cfg, v| cfg.controls.focus_on_shortcut = v,
            );
            form.field(
                "Focus off shortcut",
                None,
                &c.controls.focus_off_shortcut,
                "Shortcut name",
                false,
                170.0,
                false,
                |cfg, v| cfg.controls.focus_off_shortcut = v,
            );
            form.note("Make these two in the Shortcuts app with the Set Focus action; the island runs them to turn Focus on and off.");
        }
        Pane::Shelf => {
            form.section("Fills itself with");
            form.switch(
                "Downloads",
                Some("A finished download lands on the shelf, ready to drag anywhere"),
                c.tray.downloads,
                |cfg, on| cfg.tray.downloads = on,
            );
            form.switch(
                "Screenshots",
                Some("Each new screenshot on the Desktop lands on the shelf"),
                c.tray.screenshots,
                |cfg, on| cfg.tray.screenshots = on,
            );
            form.section("Keeping");
            form.switch(
                "Keep the shelf after quitting",
                Some("Files are only referenced, never copied"),
                c.tray.persist,
                |cfg, on| cfg.tray.persist = on,
            );
        }
        Pane::Permissions => {
            form.section("");
            for p in Permission::ALL {
                form.permission(p, p.title(), p.rationale(), None);
            }
            form.note("Nothing here is required. A feature that needs a permission stays off until it is allowed.");
        }
        Pane::Advanced => {
            form.section("Configuration");
            form.action(
                "Settings file",
                Some("Everything here is stored in a TOML file you can edit by hand"),
                None,
                vec![("Open", false, Box::new(app::open_config_file))],
                Some(Mark::Symbol("doc.text")),
            );
            form.action(
                "Back up",
                Some("Save your settings to a file, or load them from one"),
                None,
                vec![
                    ("Import…", false, Box::new(app::import_config)),
                    ("Export…", false, Box::new(app::export_config)),
                ],
                Some(Mark::Symbol("arrow.up.arrow.down")),
            );
            form.section("Trouble");
            form.action(
                "Logs",
                Some("What the island has been doing, for a bug report"),
                None,
                vec![("Open", false, Box::new(app::open_logs))],
                Some(Mark::Symbol("list.bullet.rectangle")),
            );
            form.action(
                "Reset all settings",
                Some("Back to how the island came. Connected agents stay connected"),
                None,
                vec![(
                    "Reset…",
                    false,
                    and_refresh(|| {
                        if app::confirm(
                            "Reset all settings?",
                            "Every setting goes back to how the island came. This can't be undone.",
                            "Reset",
                        ) {
                            app::send(Command::UpdateConfig(Box::default()));
                        }
                    }),
                )],
                Some(Mark::Symbol("arrow.counterclockwise")),
            );
        }
    }
}
