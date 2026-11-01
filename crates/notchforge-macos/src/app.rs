//! Application bootstrap and main-thread state.
//!
//! [`run`] takes over the main thread with `NSApplication.run()`. Everything
//! AppKit lives in a thread-local [`AppState`]; callbacks from AppKit, timers,
//! global monitors and the engine bridge reach it through [`with_controller`]
//! and friends.

use crate::bridge;
use crate::controller::Controller;
use crate::menubar::MenuBar;
use crate::settings::SettingsWindow;
use crate::ui::ns;
use block2::RcBlock;
use notchforge_core::config::{Config, Paths};
use notchforge_core::engine::{Command, EngineHandle, Snapshot};
use notchforge_core::model::{Coordinates, Permission, ProviderStatus};
use notchforge_core::provider::ProviderEvent;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::{NSRunLoop, NSRunLoopCommonModes, NSTimer};
use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

/// Everything the macOS front-end needs from the binary.
pub struct Boot {
    pub config: Arc<Config>,
    pub paths: Arc<Paths>,
    pub engine: EngineHandle,
    /// CoreLocation fixes for the weather provider.
    pub location: watch::Sender<Option<Coordinates>>,
    /// For main-thread providers (Accessibility banner observer).
    pub provider_events: mpsc::Sender<ProviderEvent>,
    /// Stops the async runtime; called once when quitting.
    pub on_quit: Box<dyn Fn() + 'static>,
    /// Run the end-to-end UI self-test, writing results to this directory.
    pub self_test: Option<std::path::PathBuf>,
}

pub use bridge::sink;

struct AppState {
    controller: Rc<Controller>,
    menubar: MenuBar,
    settings: RefCell<Option<SettingsWindow>>,
    config: RefCell<Arc<Config>>,
    paths: Arc<Paths>,
    engine: EngineHandle,
    provider_events: mpsc::Sender<ProviderEvent>,
    on_quit: Box<dyn Fn()>,
    _timers: [Retained<NSTimer>; 2],
    _media_observers: crate::media::Observers,
}

thread_local! {
    static APP: RefCell<Option<Rc<AppState>>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&AppState) -> R) -> Option<R> {
    let app = APP.with(|a| a.borrow().clone())?;
    Some(f(&app))
}

/// Run `f` with the notch controller (no-op before startup / after quit).
pub fn with_controller<R>(f: impl FnOnce(&Controller) -> R) -> Option<R> {
    let c = APP.with(|a| a.borrow().as_ref().map(|s| s.controller.clone()))?;
    Some(f(&c))
}

/// Run `f` with the current config.
pub fn with_config<R>(f: impl FnOnce(&Config) -> R) -> Option<R> {
    with_app(|a| f(&a.config.borrow()))
}

pub fn send(cmd: Command) {
    with_app(|a| a.engine.send(cmd));
}

pub(crate) fn apply_snapshot(snap: Arc<Snapshot>) {
    with_app(|a| a.menubar.update(&snap));
    with_controller(|c| c.apply_snapshot(snap));
}

pub(crate) fn apply_config(cfg: Arc<Config>) {
    let Some(old) = with_app(|a| a.config.replace(cfg.clone())) else { return };
    with_controller(|c| c.apply_config(cfg.clone()));
    with_app(|a| {
        a.menubar.set_visible(cfg.general.show_menu_bar_icon);
        if old.providers.accessibility_banners != cfg.providers.accessibility_banners {
            sync_accessibility_observer(&cfg, &a.provider_events);
        }
        if cfg.weather.location.eq_ignore_ascii_case("auto") {
            crate::location::refresh();
        }
    });
}

fn report_status(events: &mpsc::Sender<ProviderEvent>, status: ProviderStatus) {
    let _ = events.try_send(ProviderEvent::Status { provider: "accessibility_banners", status });
}

fn sync_accessibility_observer(cfg: &Config, events: &mpsc::Sender<ProviderEvent>) {
    if !cfg.providers.accessibility_banners {
        crate::accessibility::stop();
        report_status(events, ProviderStatus::Disabled);
        return;
    }
    match crate::accessibility::start(events.clone()) {
        Ok(()) => report_status(events, ProviderStatus::Running),
        Err(e) => {
            tracing::debug!(error = %e, "banner observer not started (retrying)");
            let status = if crate::permissions::state(Permission::Accessibility)
                == notchforge_core::model::PermissionState::Granted
            {
                ProviderStatus::Failing(e)
            } else {
                ProviderStatus::NeedsPermission(Permission::Accessibility)
            };
            report_status(events, status);
        }
    }
}

pub fn show_settings() {
    with_app(|a| {
        let cfg = a.config.borrow().clone();
        let mut s = a.settings.borrow_mut();
        let mtm = MainThreadMarker::from(&*a.controller.panel);
        s.get_or_insert_with(|| SettingsWindow::new(mtm)).show(&cfg);
    });
    // Settings is a separate window away from the notch: opening it closes
    // the island (a pinned or hovering panel would otherwise stay behind it).
    with_controller(|c| c.collapse());
}

/// Update the menu bar extra from a snapshot and return its mini player view
/// (self-test).
pub fn menubar_preview(snap: &Snapshot) -> Option<Retained<objc2_app_kit::NSView>> {
    with_app(|a| {
        a.menubar.update(snap);
        a.menubar.player_view()
    })
    .flatten()
}

/// Show Settings on a given pane.
pub fn show_settings_pane(pane: crate::settings::Pane) {
    with_app(|a| {
        let mtm = MainThreadMarker::from(&*a.controller.panel);
        a.settings.borrow_mut().get_or_insert_with(|| SettingsWindow::new(mtm)).set_pane(pane);
    });
    show_settings();
}

/// Re-open Settings shortly, after the engine has broadcast a config change.
pub fn show_settings_soon() {
    let when = dispatch2::DispatchTime::NOW.time(300_000_000);
    let _ = dispatch2::DispatchQueue::main().after(when, show_settings);
}

/// A Spotify Web API token is stored.
pub fn spotify_signed_in() -> bool {
    with_app(|a| a.paths.spotify_token().exists()).unwrap_or(false)
}

pub fn open_config_file() {
    with_app(|a| {
        crate::sharing::open_path(&a.paths.config_file);
    });
}

pub fn open_logs() {
    with_app(|a| crate::sharing::reveal(std::slice::from_ref(&a.paths.log_dir)));
}

/// Bring NotchForge to the front (needed before showing windows or alerts,
/// since it runs as an accessory app). `activateIgnoringOtherApps:` is used
/// because `-[NSApplication activate]` only exists on macOS 14+.
#[allow(deprecated)]
pub fn activate(mtm: MainThreadMarker) {
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
}

/// Simple informational alert.
pub fn alert(title: &str, message: &str) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&ns(title));
    alert.setInformativeText(&ns(message));
    crate::app::activate(mtm);
    alert.runModal();
}

/// Modal confirmation that resolves to "no" after `secs` seconds.
pub fn confirm_with_timeout(title: &str, message: &str, yes: &str, no: &str, secs: u64) -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&ns(title));
    alert.setInformativeText(&ns(&format!("{message} Reverting in {secs} seconds.")));
    alert.addButtonWithTitle(&ns(yes));
    alert.addButtonWithTitle(&ns(no));
    let block = RcBlock::new(move |_t: NonNull<NSTimer>| {
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).abortModal();
        }
    });
    // SAFETY: documented NSTimer API; added in common modes so it fires inside the modal loop.
    let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(secs as f64, false, &block) };
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    crate::app::activate(mtm);
    let response = alert.runModal();
    timer.invalidate();
    response == NSAlertFirstButtonReturn
}

/// Quit from any thread (used for SIGINT/SIGTERM): hops to the main queue and
/// runs the normal [`quit`] path so providers and the tray shut down cleanly.
pub fn request_quit() {
    dispatch2::DispatchQueue::main().exec_async(quit);
}

fn teardown() {
    if let Some(app) = APP.with(|a| a.borrow_mut().take()) {
        crate::accessibility::stop();
        app.controller.shutdown();
        if let Some(s) = app.settings.borrow().as_ref() {
            s.close();
        }
        (app.on_quit)();
    }
}

pub fn quit() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    teardown();
    NSApplication::sharedApplication(mtm).terminate(None);
}

/// Like [`quit`] but exits with a specific status (used by the self-test).
pub fn quit_with_code(code: i32) {
    teardown();
    std::process::exit(code);
}

pub fn settings_visible() -> bool {
    with_app(|a| a.settings.borrow().as_ref().is_some_and(SettingsWindow::is_visible)).unwrap_or(false)
}

/// The Settings window's content view, when it exists (self-test renders it).
pub fn settings_content_view() -> Option<Retained<objc2_app_kit::NSView>> {
    with_app(|a| a.settings.borrow().as_ref().and_then(SettingsWindow::content_view)).flatten()
}

pub fn close_settings() {
    with_app(|a| {
        if let Some(s) = a.settings.borrow().as_ref() {
            s.close();
        }
    });
}

/// Take over the main thread and run the AppKit event loop. Never returns
/// under normal operation (the process exits from `terminate:`).
pub fn run(boot: Boot) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("NotchForge's UI must run on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    // Accessory: no Dock icon, no main menu, never steals focus by itself.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let controller = Controller::new(mtm, boot.config.clone(), boot.engine.clone())?;
    controller.install();
    let menubar = MenuBar::new(mtm);
    menubar.set_visible(boot.config.general.show_menu_bar_icon);

    crate::hotkey::install();
    crate::location::start(mtm, boot.location);
    if boot.config.providers.weather && boot.config.weather.location.eq_ignore_ascii_case("auto") {
        crate::location::request_if_undetermined();
    }
    // Hourly location refresh for the weather provider.
    let block = RcBlock::new(|_t: NonNull<NSTimer>| crate::location::refresh());
    // SAFETY: documented NSTimer API.
    let location_timer = unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(3600.0, true, &block) };
    // Start the banner observer once Accessibility is granted and the
    // Notification Center UI process exists (macOS launches it lazily). Only does
    // work while the observer is enabled but not yet attached.
    let block = RcBlock::new(|_t: NonNull<NSTimer>| {
        with_app(|a| {
            let cfg = a.config.borrow().clone();
            if cfg.providers.accessibility_banners && !crate::accessibility::is_running() {
                sync_accessibility_observer(&cfg, &a.provider_events);
            }
        });
    });
    // SAFETY: documented NSTimer API.
    let permission_timer = unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(5.0, true, &block) };

    if boot.config.general.launch_at_login != crate::login_item::is_enabled() {
        if let Err(e) = crate::login_item::set_enabled(boot.config.general.launch_at_login) {
            tracing::warn!(error = %e, "could not sync launch-at-login");
        }
    }

    let state = Rc::new(AppState {
        controller,
        menubar,
        settings: RefCell::new(None),
        config: RefCell::new(boot.config.clone()),
        paths: boot.paths,
        engine: boot.engine,
        provider_events: boot.provider_events,
        on_quit: boot.on_quit,
        _timers: [location_timer, permission_timer],
        _media_observers: crate::media::install_observers(),
    });
    sync_accessibility_observer(&boot.config, &state.provider_events);
    APP.with(|a| *a.borrow_mut() = Some(state));
    if let Some(dir) = boot.self_test {
        tracing::info!(dir = %dir.display(), "running UI self-test");
        crate::selftest::start(mtm, dir);
    }

    tracing::info!("NotchForge UI running");
    app.run();
    Ok(())
}
