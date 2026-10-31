//! Settings window, built to the *Settings · Media* board: a dark
//! System-Settings-style window with a sidebar of panes and grouped rows of
//! native controls (switches, pop-ups, fields, buttons), fully VoiceOver
//! accessible.
//!
//! Every change applies immediately: the edited draft config is sent to the
//! engine, which persists it and broadcasts it. A pane is rebuilt each time
//! it is shown so permission states are always fresh.

use crate::app;
use crate::permissions;
use crate::ui::{
    ActionTarget, FlippedView, Targets, Weight, attributed, font, label, ns, rect, rgba, shape_view, text_width,
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
    NSControlStateValueOff, NSControlStateValueOn, NSPopUpButton, NSScrollView, NSSearchField, NSSlider, NSSwitch,
    NSTextAlignment, NSTextField, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectView,
    NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const WIDTH: f64 = 920.0;
const HEIGHT: f64 = 580.0;
const SIDEBAR: f64 = 230.0;
const CONTENT_PAD: f64 = 32.0;
const GROUP_W: f64 = WIDTH - SIDEBAR - 2.0 * CONTENT_PAD;

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
        Pane::Weather,
        Pane::Media,
        Pane::Tray,
        Pane::Controls,
        Pane::Permissions,
    ];

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
            Pane::Agents => "claude cursor zed codex opencode qoder webhook port token socket inbox",
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
type SidebarItems = Rc<RefCell<Vec<(Pane, Retained<FlippedView>)>>>;

fn text_primary() -> Retained<NSColor> {
    rgba(1.0, 1.0, 1.0, 0.92)
}
fn text_secondary() -> Retained<NSColor> {
    rgba(1.0, 1.0, 1.0, 0.5)
}
fn hairline() -> Retained<NSColor> {
    rgba(1.0, 1.0, 1.0, 0.08)
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
}

const PRIORITIES: [(&str, Priority); 4] =
    [("Low", Priority::Low), ("Normal", Priority::Normal), ("High", Priority::High), ("Critical", Priority::Critical)];

impl Form<'_> {
    fn commit(draft: &Draft) {
        app::send(Command::UpdateConfig(Box::new(draft.borrow().clone())));
    }

    fn begin(&self, title: &str) {
        self.end();
        self.y.set(self.y.get() + 6.0);
        if !title.is_empty() {
            let h = label(
                self.mtm,
                title,
                &font(12.0, Weight::Semibold),
                &text_secondary(),
                rect(CONTENT_PAD + 10.0, self.y.get(), GROUP_W, 15.0),
            );
            self.doc.addSubview(&h);
            self.y.set(self.y.get() + 21.0);
        }
        let g =
            shape_view(self.mtm, rect(CONTENT_PAD, self.y.get(), GROUP_W, 0.0), &rgba(0.173, 0.173, 0.18, 1.0), 11.0);
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
            self.y.set(f.origin.y + h + 16.0);
        }
    }

    /// Footnote under the last group.
    fn note(&self, text: &str) {
        self.end();
        let lines = (text_width(&attributed(text, &font(11.5, Weight::Regular), &text_secondary())) / (GROUP_W - 20.0))
            .ceil()
            .max(1.0);
        let h = lines * 16.0;
        let l = crate::ui::wrapping_label(
            self.mtm,
            text,
            &font(11.5, Weight::Regular),
            &text_secondary(),
            rect(CONTENT_PAD + 10.0, self.y.get() - 10.0, GROUP_W - 20.0, h),
            4,
        );
        self.doc.addSubview(&l);
        self.y.set(self.y.get() - 10.0 + h + 14.0);
    }

    /// Append a row of `height` to the open group; returns (row view, width).
    fn row(&self, height: f64) -> (Retained<FlippedView>, f64) {
        let mut g = self.group.borrow_mut();
        let (group, y) = g.as_mut().expect("row outside a group");
        let row = FlippedView::new(self.mtm, rect(0.0, *y, GROUP_W, height));
        if *y > 0.0 {
            row.addSubview(&shape_view(self.mtm, rect(14.0, 0.0, GROUP_W - 14.0, 0.5), &hairline(), 0.0));
        }
        group.addSubview(&row);
        *y += height;
        (row, GROUP_W)
    }

    /// Title (and optional subtitle) on the left of a row; returns row height.
    fn texts(&self, subtitle: Option<&str>, text_w: f64) -> f64 {
        let lines = subtitle.map_or(0.0, |s| {
            (text_width(&attributed(s, &font(11.5, Weight::Regular), &text_secondary())) / text_w)
                .ceil()
                .clamp(1.0, 3.0)
        });
        if lines == 0.0 { 44.0 } else { 34.0 + lines * 15.0 }
    }

    fn put_texts(&self, row: &FlippedView, title: &str, subtitle: Option<&str>, text_w: f64, h: f64) {
        let has_sub = subtitle.is_some();
        let ty = if has_sub { 9.0 } else { (h - 17.0) / 2.0 };
        row.addSubview(&label(
            self.mtm,
            title,
            &font(13.0, Weight::Regular),
            &text_primary(),
            rect(14.0, ty, text_w, 17.0),
        ));
        if let Some(s) = subtitle {
            let l = crate::ui::wrapping_label(
                self.mtm,
                s,
                &font(11.5, Weight::Regular),
                &text_secondary(),
                rect(14.0, 27.0, text_w, h - 29.0),
                3,
            );
            row.addSubview(&l);
        }
    }

    fn switch(&self, title: &str, subtitle: Option<&str>, value: bool, set: impl Fn(&mut Config, bool) + 'static) {
        let text_w = GROUP_W - 90.0;
        let h = self.texts(subtitle, text_w);
        let (row, w) = self.row(h);
        self.put_texts(&row, title, subtitle, text_w, h);
        let draft = self.draft.clone();
        let t = self.targets.add(self.mtm, move |sender: Option<&AnyObject>| {
            if let Some(s) = sender.and_then(|s| s.downcast_ref::<NSSwitch>()) {
                set(&mut draft.borrow_mut(), s.state() == NSControlStateValueOn);
                Self::commit(&draft);
            }
        });
        let sw = NSSwitch::new(self.mtm);
        sw.setState(if value { NSControlStateValueOn } else { NSControlStateValueOff });
        sw.setFrame(rect(w - 14.0 - 40.0, (h - 22.0) / 2.0, 40.0, 22.0));
        // SAFETY: target kept alive by `targets`.
        unsafe {
            sw.setTarget(Some(t.as_any()));
            sw.setAction(Some(ActionTarget::action()));
        }
        crate::ui::set_accessibility_label(&sw, title);
        row.addSubview(&sw);
    }

    fn popup(&self, title: &str, options: &[&str], selected: usize, set: impl Fn(&mut Config, usize) + 'static) {
        let (row, w) = self.row(44.0);
        self.put_texts(&row, title, None, w - 240.0, 44.0);
        let p = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(self.mtm),
            rect(w - 14.0 - 190.0, 9.0, 190.0, 26.0),
            false,
        );
        for o in options {
            p.addItemWithTitle(&ns(o));
        }
        p.selectItemAtIndex(selected as isize);
        crate::ui::set_accessibility_label(&p, title);
        let draft = self.draft.clone();
        let t = self.targets.add(self.mtm, move |sender: Option<&AnyObject>| {
            if let Some(p) = sender.and_then(|s| s.downcast_ref::<NSPopUpButton>()) {
                if let Ok(i) = usize::try_from(p.indexOfSelectedItem()) {
                    set(&mut draft.borrow_mut(), i);
                    Self::commit(&draft);
                }
            }
        });
        // SAFETY: target kept alive by `targets`.
        unsafe {
            p.setTarget(Some(t.as_any()));
            p.setAction(Some(ActionTarget::action()));
        }
        row.addSubview(&p);
    }

    fn text(
        &self,
        title: &str,
        value: &str,
        placeholder: &str,
        mono: bool,
        set: impl Fn(&mut Config, String) + 'static,
    ) {
        let (row, w) = self.row(44.0);
        self.put_texts(&row, title, None, w - 340.0, 44.0);
        let f = NSTextField::textFieldWithString(&ns(value), self.mtm);
        f.setPlaceholderString(Some(&ns(placeholder)));
        f.setFrame(rect(w - 14.0 - 300.0, 11.0, 300.0, 22.0));
        f.setAlignment(NSTextAlignment::Right);
        if mono {
            f.setFont(Some(&objc2_app_kit::NSFont::monospacedSystemFontOfSize_weight(
                12.0,
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
        row.addSubview(&f);
    }

    fn slider(&self, title: &str, min: f64, max: f64, value: f64, set: impl Fn(&mut Config, f64) + 'static) {
        let (row, w) = self.row(44.0);
        self.put_texts(&row, title, None, w - 260.0, 44.0);
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
        s.setFrame(rect(w - 14.0 - 220.0, 12.0, 220.0, 20.0));
        crate::ui::set_accessibility_label(&s, title);
        row.addSubview(&s);
    }

    /// A row with a status word and buttons on the right.
    fn action_row(
        &self,
        title: &str,
        subtitle: Option<&str>,
        status: Option<(&str, Retained<NSColor>)>,
        buttons: Vec<RowButton<'_>>,
        leading_icon: Option<(&str, (u8, u8, u8))>,
    ) -> Vec<Retained<NSButton>> {
        let bw: Vec<f64> = buttons
            .iter()
            .map(|(t, _, _)| {
                (text_width(&attributed(t, &font(13.0, Weight::Regular), &text_primary())) + 26.0).max(70.0)
            })
            .collect();
        let status_w = status
            .as_ref()
            .map_or(0.0, |(s, _)| text_width(&attributed(s, &font(12.5, Weight::Regular), &text_primary())) + 26.0);
        let controls_w = bw.iter().sum::<f64>() + 8.0 * bw.len() as f64 + status_w;
        let icon_w = if leading_icon.is_some() { 42.0 } else { 0.0 };
        let text_w = GROUP_W - 28.0 - controls_w - icon_w;
        let h = self.texts(subtitle, text_w).max(if leading_icon.is_some() { 52.0 } else { 44.0 });
        let (row, w) = self.row(h);
        if let Some((sym, color)) = leading_icon {
            let disc = shape_view(self.mtm, rect(14.0, (h - 30.0) / 2.0, 30.0, 30.0), &crate::ui::rgb8(color), 15.0);
            disc.addSubview(&crate::ui::symbol_view(
                self.mtm,
                sym,
                14.0,
                &NSColor::blackColor(),
                rect(7.0, 7.0, 16.0, 16.0),
                title,
            ));
            row.addSubview(&disc);
            let inner = FlippedView::new(self.mtm, rect(icon_w, 0.0, w - icon_w, h));
            self.put_texts(&inner, title, subtitle, text_w, h);
            row.addSubview(&inner);
        } else {
            self.put_texts(&row, title, subtitle, text_w, h);
        }
        let mut x = w - 14.0;
        let mut made = Vec::new();
        for ((t, primary, f), width) in buttons.into_iter().zip(bw).rev() {
            x -= width;
            let target = self.targets.add(self.mtm, move |_| f());
            let b = crate::ui::text_button(self.mtm, t, &target, rect(x, (h - 24.0) / 2.0, width, 24.0));
            b.setControlSize(NSControlSize::Regular);
            if primary {
                b.setBezelColor(Some(&NSColor::controlAccentColor()));
                b.setKeyEquivalent(&ns(""));
            }
            row.addSubview(&b);
            made.push(b);
            x -= 8.0;
        }
        if let Some((s, color)) = status {
            let l = label(
                self.mtm,
                s,
                &font(12.5, Weight::Regular),
                &color,
                rect(x - status_w + 18.0, (h - 16.0) / 2.0, status_w - 18.0, 16.0),
            );
            l.setAlignment(NSTextAlignment::Right);
            row.addSubview(&l);
        }
        made
    }

    fn permission(&self, p: Permission) {
        let state = permissions::state(p);
        let (word, color) = match state {
            PermissionState::Granted => ("Allowed", NSColor::systemGreenColor()),
            PermissionState::NotRequired => ("Not required", text_secondary()),
            PermissionState::Denied => ("Not allowed", NSColor::systemOrangeColor()),
            PermissionState::NotDetermined => ("Not asked yet", text_secondary()),
            PermissionState::Unknown => ("Unknown", text_secondary()),
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
        self.action_row(p.title(), Some(p.rationale()), Some((word, color)), buttons, None);
    }
}

fn index_of<T: PartialEq>(items: &[T], v: &T) -> usize {
    items.iter().position(|x| x == v).unwrap_or(0)
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
        window.setTitle(&ns("NotchForge Settings"));
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setBackgroundColor(Some(&rgba(0.118, 0.118, 0.125, 1.0)));
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
        // The content draws its own dark ground (it must not depend on the
        // window background, which offscreen renders and some transitions skip).
        let root = shape_view(mtm, rect(0.0, 0.0, WIDTH, HEIGHT), &rgba(0.118, 0.118, 0.125, 1.0), 0.0);
        root.addSubview(&self.sidebar(mtm));

        // ── pane ──
        let pane = self.pane.get();
        let title = label(
            mtm,
            pane.title(),
            &font(15.0, Weight::Semibold),
            &text_primary(),
            rect(SIDEBAR, 14.0, WIDTH - SIDEBAR, 20.0),
        );
        title.setAlignment(NSTextAlignment::Center);
        root.addSubview(&title);
        let doc = FlippedView::new(mtm, rect(0.0, 0.0, WIDTH - SIDEBAR, 2400.0));
        let form = Form {
            mtm,
            doc: &doc,
            y: Cell::new(8.0),
            targets: &self.targets,
            draft: Rc::new(RefCell::new(config.clone())),
            group: RefCell::new(None),
        };
        build_pane(&form, pane, config);
        form.end();
        let total = form.y.get() + 12.0;
        let view_h = HEIGHT - 48.0;
        doc.setFrame(rect(0.0, 0.0, WIDTH - SIDEBAR, total.max(view_h)));
        let scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(SIDEBAR, 48.0, WIDTH - SIDEBAR, view_h));
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
    }

    fn sidebar(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let side = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), rect(0.0, 0.0, SIDEBAR, HEIGHT));
        side.setMaterial(NSVisualEffectMaterial::Sidebar);
        side.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        let inner = FlippedView::new(mtm, rect(0.0, 0.0, SIDEBAR, HEIGHT));
        inner.addSubview(&shape_view(mtm, rect(SIDEBAR - 0.5, 0.0, 0.5, HEIGHT), &hairline(), 0.0));

        // Search filters the pane list in place.
        let items: SidebarItems = Rc::default();
        let search = NSSearchField::initWithFrame(NSSearchField::alloc(mtm), rect(12.0, 46.0, SIDEBAR - 24.0, 28.0));
        search.setPlaceholderString(Some(&ns("Search")));
        crate::ui::set_accessibility_label(&search, "Search settings");
        let it = items.clone();
        let t = self.targets.add(mtm, move |sender: Option<&AnyObject>| {
            let Some(f) = sender.and_then(|s| s.downcast_ref::<NSTextField>()) else { return };
            let q = f.stringValue().to_string().to_lowercase();
            let mut y = 86.0;
            for (pane, v) in it.borrow().iter() {
                let hit = q.is_empty() || pane.title().to_lowercase().contains(&q) || pane.keywords().contains(&q);
                v.setHidden(!hit);
                if hit {
                    v.setFrameOrigin(objc2_foundation::NSPoint::new(10.0, y));
                    y += 32.0;
                }
            }
        });
        // SAFETY: target kept alive by `targets`.
        unsafe {
            search.setTarget(Some(t.as_any()));
            search.setAction(Some(ActionTarget::action()));
        }
        inner.addSubview(&search);

        let current = self.pane.get();
        for (i, pane) in Pane::ALL.into_iter().enumerate() {
            let selected = pane == current;
            let bg = if selected { NSColor::controlAccentColor() } else { rgba(1.0, 1.0, 1.0, 0.0) };
            let item = shape_view(mtm, rect(10.0, 86.0 + i as f64 * 32.0, SIDEBAR - 20.0, 30.0), &bg, 7.0);
            let (sym, color) = pane.icon();
            let tile = shape_view(mtm, rect(8.0, 5.0, 20.0, 20.0), &crate::ui::rgb8(color), 5.0);
            tile.addSubview(&crate::ui::symbol_view(
                mtm,
                sym,
                10.0,
                &NSColor::whiteColor(),
                rect(3.0, 3.0, 14.0, 14.0),
                "",
            ));
            item.addSubview(&tile);
            let fg = if selected { NSColor::whiteColor() } else { text_primary() };
            item.addSubview(&label(
                mtm,
                pane.title(),
                &font(13.0, Weight::Regular),
                &fg,
                rect(37.0, 6.0, SIDEBAR - 60.0, 17.0),
            ));
            let pc = self.pane.clone();
            let t = self.targets.add(mtm, move |_| {
                pc.set(pane);
                // Deferred: rebuilding now would free this button's own target.
                dispatch2::DispatchQueue::main().exec_async(app::show_settings);
            });
            let b = crate::ui::click_overlay(mtm, &t, rect(0.0, 0.0, SIDEBAR - 20.0, 30.0), pane.title());
            item.addSubview(&b);
            if selected {
                // SAFETY: accessibility setter on NSView.
                let _: () = unsafe { objc2::msg_send![&*b, setAccessibilitySelected: true] };
            }
            inner.addSubview(&item);
            items.borrow_mut().push((pane, item));
        }
        side.addSubview(&inner);
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
            form.begin("");
            form.switch("Coding agents", Some("Webhook, Unix socket and inbox file"), c.providers.agents, |cfg, on| {
                cfg.providers.agents = on
            });
            form.text("Webhook port", &c.agents.http_port.to_string(), "47815", true, |cfg, v| {
                if let Ok(p) = v.parse::<u16>() {
                    cfg.agents.http_port = p;
                }
            });
            form.text("Bearer token", c.agents.token.as_deref().unwrap_or(""), "Optional", true, |cfg, v| {
                cfg.agents.token = if v.is_empty() { None } else { Some(v) };
            });
            let port = c.agents.http_port;
            form.action_row(
                "Test the webhook",
                Some("Copies a curl command that posts a sample event to the notch."),
                None,
                vec![(
                    "Copy curl",
                    false,
                    Box::new(move || {
                        crate::sharing::copy_text(&format!(
                            "curl -s -X POST http://127.0.0.1:{port}/v1/notify -H 'Content-Type: application/json' -d '{{\"source\":\"agent\",\"kind\":\"task_finished\",\"title\":\"Hello from NotchForge\"}}'"
                        ));
                    }),
                )],
                None,
            );
            form.note(&format!(
                "Agents POST JSON to http://127.0.0.1:{port}/v1/notify. The README has recipes for Claude Code, Codex, OpenCode, Zed and Cursor."
            ));
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
            form.begin("Show");
            form.switch(
                "What's playing beside the notch",
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
            form.switch(
                "Spotify and Apple Music",
                Some("Now playing, controls and playlists. Needs Automation."),
                c.providers.media,
                |cfg, on| cfg.providers.media = on,
            );
            let polls = [5u64, 10, 30, 60];
            form.popup(
                "Re-read players every",
                &["5 seconds", "10 seconds", "30 seconds", "1 minute"],
                index_of(&polls, &c.media.poll_seconds),
                move |cfg, i| cfg.media.poll_seconds = polls[i],
            );
            form.begin("Spotify playlists");
            let signed_in = app::spotify_signed_in();
            let has_id = !c.media.spotify_client_id.is_empty();
            let (status, status_color) = if signed_in {
                ("Connected", NSColor::systemGreenColor())
            } else {
                ("Not connected", text_secondary())
            };
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
            let made = form.action_row(
                "Spotify account",
                Some(if signed_in {
                    "Your playlists appear in Now Playing."
                } else {
                    "Sign in to list your playlists."
                }),
                Some((status, status_color)),
                vec![button],
                Some(("waveform", (30, 215, 96))),
            );
            if !signed_in && !has_id {
                for b in &made {
                    b.setEnabled(false);
                }
            }
            form.text("Client ID", &c.media.spotify_client_id, "From developer.spotify.com", true, |cfg, v| {
                cfg.media.spotify_client_id = v
            });
            form.note(&format!(
                "Only needed to list playlists; play, pause and skip work without it. Create an app at developer.spotify.com/dashboard and add the redirect URI {}.",
                c.media.spotify_redirect_uri()
            ));
            form.begin("Permissions");
            form.permission(Permission::Automation);
            form.permission(Permission::Accessibility);
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
                "Create these in the Shortcuts app with the “Set Focus” action. NotchForge runs them to toggle Focus.",
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
