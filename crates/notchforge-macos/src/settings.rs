//! Settings window.
//!
//! Built from native controls (fully VoiceOver-accessible) on a scrolling,
//! flipped form. Every change is applied immediately: the edited draft config
//! is sent to the engine, which persists it and broadcasts it to providers and
//! the UI. The window is rebuilt each time it is shown so permission states are
//! always fresh.

use crate::app;
use crate::permissions;
use crate::ui::{ActionTarget, FlippedView, Targets, Weight, font, ns, rect, wrapping_label};
use notchforge_core::apps::KnownApp;
use notchforge_core::config::{Config, ReducedMotion, ScreenChoice};
use notchforge_core::engine::Command;
use notchforge_core::model::{Permission, PermissionState, Priority, TemperatureUnit};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSColor, NSControlSize, NSControlStateValueOff, NSControlStateValueOn, NSPopUpButton,
    NSScrollView, NSSlider, NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const WIDTH: f64 = 600.0;
const HEIGHT: f64 = 680.0;
const LEFT: f64 = 24.0;
const LABEL_W: f64 = 200.0;
const CONTROL_X: f64 = LEFT + LABEL_W + 12.0;
const CONTROL_W: f64 = WIDTH - CONTROL_X - 32.0;

pub struct SettingsWindow {
    window: Retained<NSWindow>,
    targets: Rc<Targets>,
}

type Draft = Rc<RefCell<Config>>;

/// Incrementally lays out rows top-to-bottom.
struct Form<'a> {
    mtm: MainThreadMarker,
    doc: &'a FlippedView,
    y: Cell<f64>,
    targets: &'a Targets,
    draft: Draft,
}

const PRIORITIES: [(&str, Priority); 4] =
    [("Low", Priority::Low), ("Normal", Priority::Normal), ("High", Priority::High), ("Critical", Priority::Critical)];

impl Form<'_> {
    fn commit(draft: &Draft) {
        app::send(Command::UpdateConfig(Box::new(draft.borrow().clone())));
    }

    fn section(&self, title: &str) {
        self.y.set(self.y.get() + 14.0);
        let l = crate::ui::label(
            self.mtm,
            title,
            &font(13.0, Weight::Semibold),
            &NSColor::labelColor(),
            rect(LEFT, self.y.get(), WIDTH - 2.0 * LEFT, 18.0),
        );
        self.doc.addSubview(&l);
        self.y.set(self.y.get() + 26.0);
    }

    fn row_label(&self, title: &str) {
        let l = crate::ui::label(
            self.mtm,
            title,
            &font(12.0, Weight::Regular),
            &NSColor::labelColor(),
            rect(LEFT + 8.0, self.y.get() + 3.0, LABEL_W - 8.0, 16.0),
        );
        self.doc.addSubview(&l);
    }

    fn advance(&self, h: f64) {
        self.y.set(self.y.get() + h);
    }

    fn note(&self, text: &str) {
        let l = wrapping_label(
            self.mtm,
            text,
            &font(11.0, Weight::Regular),
            &NSColor::secondaryLabelColor(),
            rect(LEFT + 8.0, self.y.get(), WIDTH - 2.0 * LEFT - 16.0, 30.0),
            3,
        );
        self.doc.addSubview(&l);
        self.advance(32.0);
    }

    fn checkbox(&self, title: &str, value: bool, set: impl Fn(&mut Config, bool) + 'static) {
        let draft = self.draft.clone();
        let t = self.targets.add(self.mtm, move |sender: Option<&AnyObject>| {
            if let Some(b) = sender.and_then(|s| s.downcast_ref::<NSButton>()) {
                set(&mut draft.borrow_mut(), b.state() == NSControlStateValueOn);
                Self::commit(&draft);
            }
        });
        // SAFETY: target kept alive by `targets`.
        let b = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &ns(title),
                Some(t.as_any()),
                Some(ActionTarget::action()),
                self.mtm,
            )
        };
        b.setState(if value { NSControlStateValueOn } else { NSControlStateValueOff });
        b.setFrame(rect(LEFT + 8.0, self.y.get(), WIDTH - 2.0 * LEFT - 16.0, 20.0));
        self.doc.addSubview(&b);
        self.advance(24.0);
    }

    fn popup(&self, title: &str, options: &[&str], selected: usize, set: impl Fn(&mut Config, usize) + 'static) {
        self.row_label(title);
        let p = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(self.mtm),
            rect(CONTROL_X, self.y.get(), CONTROL_W.min(220.0), 24.0),
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
        self.doc.addSubview(&p);
        self.advance(30.0);
    }

    fn text(&self, title: &str, value: &str, placeholder: &str, set: impl Fn(&mut Config, String) + 'static) {
        self.row_label(title);
        let f = NSTextField::textFieldWithString(&ns(value), self.mtm);
        f.setPlaceholderString(Some(&ns(placeholder)));
        f.setFrame(rect(CONTROL_X, self.y.get(), CONTROL_W, 22.0));
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
        self.doc.addSubview(&f);
        self.advance(30.0);
    }

    fn slider(&self, title: &str, min: f64, max: f64, value: f64, set: impl Fn(&mut Config, f64) + 'static) {
        self.row_label(title);
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
        s.setFrame(rect(CONTROL_X, self.y.get(), CONTROL_W.min(240.0), 22.0));
        crate::ui::set_accessibility_label(&s, title);
        self.doc.addSubview(&s);
        self.advance(28.0);
    }

    fn button(&self, title: &str, x: f64, w: f64, f: impl Fn() + 'static) -> Retained<NSButton> {
        let t = self.targets.add(self.mtm, move |_| f());
        let b = crate::ui::text_button(self.mtm, title, &t, rect(x, self.y.get(), w, 24.0));
        self.doc.addSubview(&b);
        b
    }

    fn permission(&self, p: Permission) {
        let state = permissions::state(p);
        self.row_label(p.title());
        let color = match state {
            PermissionState::Granted | PermissionState::NotRequired => NSColor::systemGreenColor(),
            PermissionState::Denied => NSColor::systemOrangeColor(),
            _ => NSColor::secondaryLabelColor(),
        };
        let status = crate::ui::label(
            self.mtm,
            permissions::state_label(state),
            &font(12.0, Weight::Medium),
            &color,
            rect(CONTROL_X, self.y.get() + 3.0, 120.0, 16.0),
        );
        self.doc.addSubview(&status);
        if state != PermissionState::NotRequired {
            if state != PermissionState::Granted {
                self.button("Request Access", CONTROL_X + 124.0, 120.0, move || {
                    permissions::request(p);
                    app::send(Command::Refresh);
                });
            }
            self.button("Open Settings", CONTROL_X + 248.0, 110.0, move || permissions::open_settings(p));
        }
        self.advance(26.0);
        self.note(p.rationale());
    }
}

fn index_of<T: PartialEq>(items: &[T], v: &T) -> usize {
    items.iter().position(|x| x == v).unwrap_or(0)
}

impl SettingsWindow {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
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
        // SAFETY: we own the window strongly.
        unsafe { window.setReleasedWhenClosed(false) };
        Self { window, targets: Rc::new(Targets::default()) }
    }

    pub fn show(&self, config: &Config) {
        let mtm = MainThreadMarker::from(&*self.window);
        self.targets.clear();
        let draft: Draft = Rc::new(RefCell::new(config.clone()));
        let doc = FlippedView::new(mtm, rect(0.0, 0.0, WIDTH, 2400.0));
        let form = Form { mtm, doc: &doc, y: Cell::new(6.0), targets: &self.targets, draft: draft.clone() };
        let c = config;

        form.section("General");
        let login_enabled = crate::login_item::is_enabled();
        form.checkbox("Launch at login", login_enabled, |cfg, on| {
            cfg.general.launch_at_login = on;
            if let Err(e) = crate::login_item::set_enabled(on) {
                app::alert("Launch at login", &e);
            }
        });
        form.checkbox("Show menu bar icon", c.general.show_menu_bar_icon, |cfg, on| {
            cfg.general.show_menu_bar_icon = on
        });
        let screens = [ScreenChoice::Notch, ScreenChoice::Main, ScreenChoice::Mouse];
        form.popup(
            "Show on screen",
            &["Built-in (notch)", "Main display", "Display with pointer"],
            index_of(&screens, &c.general.screen),
            move |cfg, i| {
                cfg.general.screen = screens[i];
            },
        );
        form.slider("Hover delay", 0.0, 600.0, c.general.hover_delay_ms as f64, |cfg, v| {
            cfg.general.hover_delay_ms = v.round() as u64
        });
        let prios: Vec<Priority> = PRIORITIES.iter().map(|p| p.1).collect();
        let prio_names: Vec<&str> = PRIORITIES.iter().map(|p| p.0).collect();
        let pl = prios.clone();
        form.popup(
            "Peek for priority ≥",
            &prio_names,
            index_of(&prios, &c.general.peek_min_priority),
            move |cfg, i| {
                cfg.general.peek_min_priority = pl[i];
            },
        );

        form.section("Appearance & Motion");
        let motions = [ReducedMotion::System, ReducedMotion::Always, ReducedMotion::Never];
        form.popup(
            "Reduce motion",
            &["Follow system", "Always", "Never"],
            index_of(&motions, &c.appearance.reduced_motion),
            move |cfg, i| {
                cfg.appearance.reduced_motion = motions[i];
            },
        );
        form.slider("Animation speed", 0.5, 2.0, c.appearance.animation_speed, |cfg, v| {
            cfg.appearance.animation_speed = v
        });
        form.checkbox("Show live indicators beside the notch", c.appearance.show_wings, |cfg, on| {
            cfg.appearance.show_wings = on
        });
        form.checkbox("High contrast text", c.appearance.high_contrast, |cfg, on| cfg.appearance.high_contrast = on);

        form.section("Providers");
        form.checkbox("Mac battery", c.providers.battery, |cfg, on| cfg.providers.battery = on);
        form.checkbox("Accessory batteries (AirPods, keyboards, mice…)", c.providers.bluetooth, |cfg, on| {
            cfg.providers.bluetooth = on
        });
        form.checkbox("Weather", c.providers.weather, |cfg, on| cfg.providers.weather = on);
        form.checkbox("Coding agents (webhook, socket, inbox)", c.providers.agents, |cfg, on| {
            cfg.providers.agents = on
        });
        form.checkbox("Notification Center (needs Full Disk Access)", c.providers.notification_center, |cfg, on| {
            cfg.providers.notification_center = on
        });
        form.checkbox(
            "Banner observer fallback (needs Accessibility)",
            c.providers.accessibility_banners,
            |cfg, on| cfg.providers.accessibility_banners = on,
        );
        form.checkbox("Focus state (needs Full Disk Access)", c.providers.focus, |cfg, on| cfg.providers.focus = on);

        form.section("Weather");
        form.text("Location", &c.weather.location, "auto, or a city name", |cfg, v| {
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
            move |cfg, i| {
                cfg.weather.refresh_minutes = refresh[i];
            },
        );

        form.section("Coding Agents");
        form.text("Webhook port", &c.agents.http_port.to_string(), "47815", |cfg, v| {
            if let Ok(p) = v.parse::<u16>() {
                cfg.agents.http_port = p;
            }
        });
        form.text(
            "Bearer token (optional)",
            c.agents.token.as_deref().unwrap_or(""),
            "leave empty for none",
            |cfg, v| {
                cfg.agents.token = if v.is_empty() { None } else { Some(v) };
            },
        );
        let port = c.agents.http_port;
        form.note(&format!(
            "POST JSON to http://127.0.0.1:{port}/v1/notify — e.g. {{\"source\":\"cursor\",\"kind\":\"task_finished\",\"title\":\"Done\"}}. See README for Claude Code, OpenCode, Zed and Cursor recipes."
        ));
        form.button("Copy curl example", LEFT + 8.0, 160.0, move || {
            crate::sharing::copy_text(&format!(
                "curl -s -X POST http://127.0.0.1:{port}/v1/notify -H 'Content-Type: application/json' -d '{{\"source\":\"agent\",\"kind\":\"task_finished\",\"title\":\"Hello from NotchForge\"}}'"
            ));
        });
        form.advance(32.0);

        form.section("Notifications");
        form.popup(
            "While Focus is on, show ≥",
            &prio_names,
            index_of(&prios, &c.notifications.focus_min_priority),
            move |cfg, i| {
                cfg.notifications.focus_min_priority = prios[i];
            },
        );
        form.checkbox("Show notifications from other apps", c.notifications.allow_other_apps, |cfg, on| {
            cfg.notifications.allow_other_apps = on
        });
        form.checkbox(
            "Close the native banner when the notch shows it (experimental)",
            c.notifications.suppress_native_banners,
            |cfg, on| {
                cfg.notifications.suppress_native_banners = on;
            },
        );
        for app_kind in KnownApp::ALL {
            let key = app_kind.key();
            let rule = c.notifications.apps.get(key).cloned().unwrap_or_default();
            let k1 = key.to_owned();
            form.checkbox(app_kind.display_name(), rule.enabled, move |cfg, on| {
                cfg.notifications.apps.entry(k1.clone()).or_default().enabled = on;
            });
            // Priority override popup on the same row.
            form.y.set(form.y.get() - 24.0);
            let k2 = key.to_owned();
            let options = ["Default priority", "Low", "Normal", "High", "Critical"];
            let selected = rule.priority.map_or(0, |p| 1 + PRIORITIES.iter().position(|x| x.1 == p).unwrap_or(1));
            let p = NSPopUpButton::initWithFrame_pullsDown(
                NSPopUpButton::alloc(mtm),
                rect(CONTROL_X + 60.0, form.y.get() - 2.0, 160.0, 22.0),
                false,
            );
            p.setControlSize(NSControlSize::Small);
            for o in options {
                p.addItemWithTitle(&ns(o));
            }
            p.selectItemAtIndex(selected as isize);
            crate::ui::set_accessibility_label(&p, &format!("{} priority", app_kind.display_name()));
            let d = draft.clone();
            let t = self.targets.add(mtm, move |sender: Option<&AnyObject>| {
                if let Some(p) = sender.and_then(|s| s.downcast_ref::<NSPopUpButton>()) {
                    let i = p.indexOfSelectedItem();
                    let prio = if i <= 0 { None } else { PRIORITIES.get(i as usize - 1).map(|x| x.1) };
                    d.borrow_mut().notifications.apps.entry(k2.clone()).or_default().priority = prio;
                    Form::commit(&d);
                }
            });
            // SAFETY: target kept alive by `targets`.
            unsafe {
                p.setTarget(Some(t.as_any()));
                p.setAction(Some(ActionTarget::action()));
            }
            doc.addSubview(&p);
            form.advance(26.0);
        }
        form.note("Messaging apps are read from Notification Center. To avoid double banners, set an app's alert style to “None” in System Settings › Notifications — it still reaches the notch.");

        form.section("File Tray");
        form.checkbox("Keep tray contents after quitting", c.tray.persist, |cfg, on| cfg.tray.persist = on);

        form.section("Quick Controls");
        form.checkbox("Brightness slider", c.controls.brightness, |cfg, on| cfg.controls.brightness = on);
        form.checkbox("Volume slider", c.controls.volume, |cfg, on| cfg.controls.volume = on);
        form.checkbox("Resolution switcher", c.controls.resolution, |cfg, on| cfg.controls.resolution = on);
        form.text("“Focus on” shortcut", &c.controls.focus_on_shortcut, "Shortcut name", |cfg, v| {
            cfg.controls.focus_on_shortcut = v
        });
        form.text("“Focus off” shortcut", &c.controls.focus_off_shortcut, "Shortcut name", |cfg, v| {
            cfg.controls.focus_off_shortcut = v
        });
        form.note(
            "Create these in the Shortcuts app with the “Set Focus” action; NotchForge runs them to toggle Focus.",
        );

        form.section("Permissions");
        for p in Permission::ALL {
            form.permission(p);
        }

        form.section("Advanced");
        form.button("Open Config File", LEFT + 8.0, 150.0, app::open_config_file);
        form.button("Open Logs", LEFT + 166.0, 120.0, app::open_logs);
        form.button("Reset to Defaults", LEFT + 294.0, 150.0, || {
            app::send(Command::UpdateConfig(Box::default()));
            app::show_settings();
        });
        form.advance(40.0);

        let total = form.y.get();
        doc.setFrame(rect(0.0, 0.0, WIDTH, total));
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(0.0, 0.0, WIDTH, HEIGHT));
        scroll.setHasVerticalScroller(true);
        scroll.setDocumentView(Some(&doc));
        let content: &NSView = &scroll;
        self.window.setContentView(Some(content));
        doc.scrollPoint(objc2_foundation::NSPoint::new(0.0, 0.0));
        if !self.window.isVisible() {
            self.window.center();
        }
        crate::app::activate(mtm);
        self.window.makeKeyAndOrderFront(None);
    }
}
