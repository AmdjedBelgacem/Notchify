//! `notchforge` — thin entry point.
//!
//! Wires configuration, logging, the tokio runtime, the engine and the
//! provider supervisor together, then hands the main thread to the AppKit UI
//! (or, with `--headless`, just runs the engine and logs what it sees — handy
//! for developing providers on any OS).

mod cli;
mod logging;
mod sinks;

use anyhow::Context;
use notchforge_core::config::{Config, Paths};
use notchforge_core::engine::{Command, Engine, EngineHandle, UiSink};
use notchforge_core::provider::{ProviderChannels, ShutdownTrigger, Supervisor, shutdown_pair};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinHandle;

fn main() -> ExitCode {
    let args = match cli::Args::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("notchforge: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: cli::Args) -> anyhow::Result<()> {
    if args.help {
        println!("{}", cli::USAGE);
        return Ok(());
    }
    if args.version {
        println!("notchforge {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let mut paths = Paths::resolve().context("resolving data directories")?;
    if let Some(cfg) = &args.config {
        paths.config_file = cfg.clone();
    }
    paths.ensure().context("creating data directories")?;
    if args.print_config {
        print!("{}", Config::default().to_toml()?);
        return Ok(());
    }
    let _log_guard = logging::init(&paths.log_dir, args.verbose);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), config = %paths.config_file.display(), "starting NotchForge");

    let config = match Config::load_or_init(&paths.config_file) {
        Ok(c) => c,
        Err(e) => {
            // Never overwrite a config the user is editing; fall back to defaults.
            tracing::error!(error = %e, "invalid config, using defaults for this run");
            Config::default()
        }
    };
    let config = Arc::new(config);
    let paths = Arc::new(paths);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("notchforge-rt")
        .enable_all()
        .build()
        .context("starting async runtime")?;

    let (events_tx, events_rx) = mpsc::channel(1024);
    let (config_tx, config_rx) = watch::channel(config.clone());
    let (location_tx, location_rx) = watch::channel(None);
    let refresh = Arc::new(Notify::new());
    let (trigger, shutdown) = shutdown_pair();

    let headless = args.headless || !cfg!(target_os = "macos");
    let sink: Arc<dyn UiSink> = ui_sink(headless);
    let (engine, handle) = Engine::new(config_tx, paths.clone(), sink, refresh.clone(), events_rx);

    let mut tasks = Vec::new();
    {
        let _guard = runtime.enter();
        tasks.push(runtime.spawn(engine.run(shutdown.clone())));
        tasks.extend(Supervisor::spawn_all(
            notchforge_providers::registry(),
            ProviderChannels {
                events: events_tx.clone(),
                config: config_rx.clone(),
                paths: paths.clone(),
                refresh,
                location: location_rx,
                shutdown: shutdown.clone(),
            },
        ));
        tasks.push(runtime.spawn(watch_config_file(paths.clone(), config_rx, handle.clone(), shutdown.clone())));
    }

    if headless {
        if !args.headless {
            tracing::warn!("the notch UI needs macOS; running headless");
        }
        tracing::info!(port = config.agents.http_port, "headless mode: press Ctrl-C to quit");
        runtime.block_on(wait_for_termination());
        stop(&trigger, runtime, tasks);
        return Ok(());
    }

    run_ui(Launch {
        config,
        paths,
        engine: handle,
        location: location_tx,
        provider_events: events_tx,
        trigger,
        runtime,
        tasks,
    })
}

/// Resolves on SIGINT (Ctrl-C) or SIGTERM (`kill`, launchd, logout).
async fn wait_for_termination() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Everything the UI needs to take over the main thread.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct Launch {
    config: Arc<Config>,
    paths: Arc<Paths>,
    engine: EngineHandle,
    location: watch::Sender<Option<notchforge_core::model::Coordinates>>,
    provider_events: mpsc::Sender<notchforge_core::provider::ProviderEvent>,
    trigger: ShutdownTrigger,
    runtime: tokio::runtime::Runtime,
    tasks: Vec<JoinHandle<()>>,
}

/// Signal shutdown and give every task up to a second to finish cleanly
/// (providers remove their socket, the engine persists the tray).
fn stop(trigger: &ShutdownTrigger, runtime: tokio::runtime::Runtime, tasks: Vec<JoinHandle<()>>) {
    tracing::info!("shutting down");
    trigger.trigger();
    runtime.block_on(async {
        let all = async {
            for t in tasks {
                let _ = t.await;
            }
        };
        match tokio::time::timeout(Duration::from_secs(1), all).await {
            Ok(()) => tracing::debug!("all tasks stopped"),
            Err(_) => tracing::warn!("some tasks did not stop in time"),
        }
    });
    runtime.shutdown_timeout(Duration::from_millis(200));
}

#[cfg(target_os = "macos")]
fn ui_sink(headless: bool) -> Arc<dyn UiSink> {
    if headless { Arc::new(sinks::LogSink::default()) } else { notchforge_macos::bridge::sink() }
}

#[cfg(not(target_os = "macos"))]
fn ui_sink(_headless: bool) -> Arc<dyn UiSink> {
    Arc::new(sinks::LogSink::default())
}

#[cfg(target_os = "macos")]
fn run_ui(l: Launch) -> anyhow::Result<()> {
    // AppKit owns the main thread, so turn Ctrl-C / SIGTERM into a regular quit.
    l.runtime.spawn(async {
        wait_for_termination().await;
        notchforge_macos::app::request_quit();
    });
    let state = std::cell::RefCell::new(Some((l.trigger, l.runtime, l.tasks)));
    let on_quit = Box::new(move || {
        if let Some((trigger, rt, tasks)) = state.borrow_mut().take() {
            stop(&trigger, rt, tasks);
        }
    });
    notchforge_macos::app::run(notchforge_macos::app::Boot {
        config: l.config,
        paths: l.paths,
        engine: l.engine,
        location: l.location,
        provider_events: l.provider_events,
        on_quit,
    })
    .map_err(anyhow::Error::msg)
}

#[cfg(not(target_os = "macos"))]
fn run_ui(_l: Launch) -> anyhow::Result<()> {
    unreachable!("non-macOS builds always run headless")
}

/// Pick up hand edits to the config file (Settings writes go through the
/// engine, which rewrites the file; re-reading an identical config is a no-op).
async fn watch_config_file(
    paths: Arc<Paths>,
    config: watch::Receiver<Arc<Config>>,
    engine: EngineHandle,
    mut shutdown: notchforge_core::provider::Shutdown,
) {
    let mtime = || std::fs::metadata(&paths.config_file).and_then(|m| m.modified()).ok();
    let mut last = mtime();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            _ = shutdown.wait() => return,
        }
        let now = mtime();
        if now == last {
            continue;
        }
        last = now;
        let Ok(text) = std::fs::read_to_string(&paths.config_file) else { continue };
        match Config::from_toml(&text) {
            Ok(c) if c != **config.borrow() => {
                tracing::info!("config file changed on disk; reloading");
                engine.send(Command::UpdateConfig(Box::new(c)));
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "ignoring invalid config edit"),
        }
    }
}
