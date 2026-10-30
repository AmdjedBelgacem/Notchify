//! Menu bar extra — secondary entry point and fallback when the notch is
//! hidden (e.g. on a screen without one, in a full-screen app).

use crate::app::{self, with_controller};
use crate::ui::{ActionTarget, Targets, ns, symbol_image};
use notchforge_core::engine::Command;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSControlStateValueOff, NSControlStateValueOn, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength};

pub struct MenuBar {
    item: Retained<NSStatusItem>,
    pause: Retained<NSMenuItem>,
    _targets: Targets,
}

fn menu_item(mtm: MainThreadMarker, targets: &Targets, title: &str, key: &str, f: impl Fn() + 'static) -> Retained<NSMenuItem> {
    let t = targets.add(mtm, move |_| f());
    // SAFETY: valid selector; target kept alive by `targets`.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(title), Some(ActionTarget::action()), &ns(key))
    };
    // SAFETY: target kept alive by `targets`.
    unsafe { item.setTarget(Some(t.as_any())) };
    item
}

impl MenuBar {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            button.setImage(symbol_image("rectangle.topthird.inset.filled", 14.0, "NotchForge").as_deref());
            button.setToolTip(Some(&ns("NotchForge")));
        }
        let targets = Targets::default();
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        menu.addItem(&menu_item(mtm, &targets, "Show Notch Panel", "", || {
            with_controller(|c| c.toggle());
        }));
        let pause = menu_item(mtm, &targets, "Pause Notifications", "", || {
            with_controller(|c| c.toggle_pause());
        });
        menu.addItem(&pause);
        menu.addItem(&menu_item(mtm, &targets, "Clear Notifications", "", || app::send(Command::ClearNotifications)));
        menu.addItem(&menu_item(mtm, &targets, "Clear Tray", "", || app::send(Command::TrayClear)));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&menu_item(mtm, &targets, "Settings…", ",", app::show_settings));
        menu.addItem(&menu_item(mtm, &targets, "Open Config File", "", app::open_config_file));
        menu.addItem(&menu_item(mtm, &targets, "Open Logs", "", app::open_logs));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&menu_item(mtm, &targets, "Quit NotchForge", "q", app::quit));
        item.setMenu(Some(&menu));
        Self { item, pause, _targets: targets }
    }

    pub fn update(&self, paused: bool, unread: usize) {
        self.pause.setState(if paused { NSControlStateValueOn } else { NSControlStateValueOff });
        let Some(mtm) = MainThreadMarker::new() else { return };
        if let Some(button) = self.item.button(mtm) {
            let tip = if unread > 0 { format!("NotchForge — {unread} unread") } else { "NotchForge".into() };
            button.setToolTip(Some(&ns(&tip)));
        }
    }

    pub fn set_visible(&self, visible: bool) {
        self.item.setVisible(visible);
    }
}
