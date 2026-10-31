//! Audio provider: output devices with their capabilities, the default output,
//! and which apps are playing audio (and through which device).
//!
//! macOS: CoreAudio only (public API, no permissions). Device and process
//! changes are pushed by CoreAudio property listeners, so the provider sleeps
//! until something actually changes; a slow poll is only a safety net.
//! Per-app activity uses the process-object API added in macOS 14.2; on older
//! systems [`AudioState::sources_supported`] is `false`.

use notchforge_core::model::{AudioDevice, AudioSource, AudioTransport, ProviderStatus};
use notchforge_core::provider::{BoxFuture, Provider, ProviderContext, ProviderDescriptor, ProviderError};
use std::collections::BTreeMap;

pub struct AudioProvider;

/// Safety-net poll; real changes arrive through CoreAudio listeners.
#[cfg(target_os = "macos")]
const FALLBACK_POLL: std::time::Duration = std::time::Duration::from_secs(20);
/// Minimum spacing between reads, so a storm of listener callbacks (a device
/// reconfiguring) costs one read.
#[cfg(target_os = "macos")]
const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(120);

/// Process names that never count as "playing" (system plumbing).
const IGNORED: &[&str] = &["coreaudiod", "systemsoundserverd", "audiomxd", "callservicesd", "avconferenced"];

/// One process producing audio, already mapped to its app.
#[derive(Debug, Clone)]
pub struct RawSource {
    pub name: String,
    pub bundle_id: Option<String>,
    pub app_path: Option<String>,
    pub pid: i32,
    pub device_uids: Vec<String>,
}

/// Group per-process audio into one entry per app (helpers merged into their
/// parent), dropping system plumbing.
pub fn group_sources(raw: Vec<RawSource>) -> Vec<AudioSource> {
    let mut by_app: BTreeMap<String, AudioSource> = BTreeMap::new();
    for RawSource { name, bundle_id, app_path, pid, device_uids: uids } in raw {
        if IGNORED.contains(&name.as_str()) {
            continue;
        }
        let key = bundle_id.clone().unwrap_or_else(|| name.clone());
        let entry = by_app.entry(key).or_insert_with(|| AudioSource {
            pid,
            name: name.clone(),
            bundle_id: bundle_id.clone(),
            app_path: app_path.clone(),
            device_uids: Vec::new(),
        });
        for u in uids {
            if !entry.device_uids.contains(&u) {
                entry.device_uids.push(u);
            }
        }
    }
    let mut v: Vec<AudioSource> = by_app.into_values().collect();
    v.sort_by_key(|s| s.name.to_lowercase());
    v
}

/// Sort devices: default first, then by transport, then name.
pub fn sort_devices(devices: &mut [AudioDevice]) {
    let rank = |t: AudioTransport| match t {
        AudioTransport::BuiltIn => 0,
        AudioTransport::Bluetooth => 1,
        AudioTransport::AirPlay => 2,
        AudioTransport::Usb => 3,
        _ => 4,
    };
    devices.sort_by(|a, b| {
        b.is_default.cmp(&a.is_default).then(rank(a.transport).cmp(&rank(b.transport))).then(a.name.cmp(&b.name))
    });
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use crate::sys::coreaudio as ca;
    use crate::sys::process;
    use notchforge_core::model::AudioState;
    use std::collections::{HashMap, HashSet};
    use std::sync::{Mutex, OnceLock};
    use tokio::sync::Notify;

    /// Woken by CoreAudio listener callbacks (any thread).
    pub fn changed() -> &'static Notify {
        static N: OnceLock<Notify> = OnceLock::new();
        N.get_or_init(Notify::new)
    }

    unsafe extern "C" fn on_change(
        _id: ca::AudioObjectID,
        _n: u32,
        _a: *const ca::AudioObjectPropertyAddress,
        _c: *mut std::ffi::c_void,
    ) -> ca::OSStatus {
        changed().notify_one();
        0
    }

    const SYSTEM_PROPS: [u32; 3] = [
        ca::kAudioHardwarePropertyDevices,
        ca::kAudioHardwarePropertyDefaultOutputDevice,
        ca::kAudioHardwarePropertyProcessObjectList,
    ];

    /// Listener registrations, removed when the provider stops.
    pub struct Listeners {
        processes: HashSet<ca::AudioObjectID>,
        devices: HashSet<ca::AudioObjectID>,
    }

    impl Listeners {
        pub fn install() -> Self {
            for sel in SYSTEM_PROPS {
                ca::add_listener(ca::kAudioObjectSystemObject, ca::global(sel), on_change);
            }
            Self { processes: HashSet::new(), devices: HashSet::new() }
        }

        /// Watch each process's "is running output" flag and each device's
        /// sample rate / volume, adding listeners for new objects.
        pub fn track(&mut self, processes: &[ca::AudioObjectID], devices: &[ca::AudioObjectID]) {
            for p in processes {
                if self.processes.insert(*p) {
                    ca::add_listener(*p, ca::global(ca::kAudioProcessPropertyIsRunningOutput), on_change);
                    ca::add_listener(*p, ca::output(ca::kAudioProcessPropertyDevices), on_change);
                }
            }
            self.processes.retain(|p| processes.contains(p));
            for d in devices {
                if self.devices.insert(*d) {
                    ca::add_listener(*d, ca::global(ca::kAudioDevicePropertyNominalSampleRate), on_change);
                    ca::add_listener(*d, ca::output(ca::kVirtualMainVolume), on_change);
                    ca::add_listener(*d, ca::output(ca::kAudioDevicePropertyMute), on_change);
                }
            }
            self.devices.retain(|d| devices.contains(d));
        }
    }

    impl Drop for Listeners {
        fn drop(&mut self) {
            for sel in SYSTEM_PROPS {
                ca::remove_listener(ca::kAudioObjectSystemObject, ca::global(sel), on_change);
            }
            for p in &self.processes {
                ca::remove_listener(*p, ca::global(ca::kAudioProcessPropertyIsRunningOutput), on_change);
                ca::remove_listener(*p, ca::output(ca::kAudioProcessPropertyDevices), on_change);
            }
            for d in &self.devices {
                ca::remove_listener(*d, ca::global(ca::kAudioDevicePropertyNominalSampleRate), on_change);
                ca::remove_listener(*d, ca::output(ca::kVirtualMainVolume), on_change);
                ca::remove_listener(*d, ca::output(ca::kAudioDevicePropertyMute), on_change);
            }
        }
    }

    /// (display name, bundle id, app path).
    type Identity = (String, Option<String>, Option<String>);

    /// App identity per pid; processes are long-lived so this saves path and
    /// plist reads on every refresh.
    fn app_identity(pid: i32) -> Identity {
        static CACHE: OnceLock<Mutex<HashMap<i32, Identity>>> = OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        if let Some(hit) = cache.lock().ok().and_then(|c| c.get(&pid).cloned()) {
            return hit;
        }
        let owner = process::responsible_pid(pid);
        let exe = process::executable_path(owner).or_else(|| process::executable_path(pid));
        let ident = match exe.as_deref().and_then(process::outer_app_bundle) {
            Some(app) => {
                let (name, bundle) = process::bundle_info(&app);
                (name, bundle, Some(app.to_string_lossy().into_owned()))
            }
            None => {
                let name = exe
                    .as_deref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .or_else(|| process::name(pid))
                    .unwrap_or_else(|| format!("pid {pid}"));
                (name, None, None)
            }
        };
        if let Ok(mut c) = cache.lock() {
            if c.len() > 512 {
                c.clear();
            }
            c.insert(pid, ident.clone());
        }
        ident
    }

    pub struct Read {
        pub state: AudioState,
        pub process_objects: Vec<ca::AudioObjectID>,
        pub device_objects: Vec<ca::AudioObjectID>,
    }

    pub fn read() -> Read {
        let default = ca::default_output();
        let me = std::process::id() as i32;
        let mut uid_of: HashMap<ca::AudioObjectID, String> = HashMap::new();
        let mut devices = Vec::new();
        let mut device_objects = Vec::new();
        for id in ca::device_ids() {
            let channels = ca::output_channels(id);
            if channels == 0 {
                continue; // input-only device
            }
            if ca::get::<u32>(id, ca::global(ca::kAudioDevicePropertyIsHidden)).unwrap_or(0) != 0 {
                continue;
            }
            let Some(uid) = ca::device_uid(id) else { continue };
            device_objects.push(id);
            uid_of.insert(id, uid.clone());
            let transport = ca::get::<u32>(id, ca::global(ca::kAudioDevicePropertyTransportType))
                .map(AudioTransport::from_fourcc)
                .unwrap_or_default();
            devices.push(AudioDevice {
                id,
                uid,
                name: ca::get_string(id, ca::global(ca::kAudioObjectPropertyName)).unwrap_or_else(|| "Output".into()),
                transport,
                sample_rate: ca::get::<f64>(id, ca::global(ca::kAudioDevicePropertyNominalSampleRate))
                    .filter(|r| *r > 0.0),
                max_sample_rate: ca::max_sample_rate(id),
                bit_depth: ca::bit_depth(id),
                output_channels: channels,
                volume: ca::get::<f32>(id, ca::output(ca::kVirtualMainVolume)).map(|v| v.clamp(0.0, 1.0)),
                muted: ca::get::<u32>(id, ca::output(ca::kAudioDevicePropertyMute)).map(|m| m != 0),
                is_default: Some(id) == default,
            });
        }
        sort_devices(&mut devices);

        let objects = ca::process_objects();
        let sources_supported = objects.is_some();
        let process_objects = objects.unwrap_or_default();
        let raw = process_objects
            .iter()
            .filter_map(|o| ca::process_info(*o))
            .filter(|p| p.running_output && p.pid != me)
            .map(|p| {
                let (name, bundle, app_path) = app_identity(p.pid);
                let bundle = bundle.or(p.bundle_id);
                let device_uids = p.output_devices.iter().filter_map(|d| uid_of.get(d).cloned()).collect();
                RawSource { name, bundle_id: bundle, app_path, pid: p.pid, device_uids }
            })
            .collect();
        Read {
            state: AudioState { devices, sources: group_sources(raw), sources_supported },
            process_objects,
            device_objects,
        }
    }

    pub fn set_output(uid: &str) -> Result<(), String> {
        let dev = ca::find_device_by_uid(uid).ok_or_else(|| format!("no output device {uid}"))?;
        ca::set_default_output(dev).map_err(|s| format!("CoreAudio error {s}"))
    }
}

impl Provider for AudioProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "audio", name: "Audio outputs", permissions: &[] }
    }

    fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            #[cfg(target_os = "macos")]
            {
                let mut listeners = mac::Listeners::install();
                ctx.set_status(ProviderStatus::Running).await;
                let mut last: Option<notchforge_core::model::AudioState> = None;
                loop {
                    let r = tokio::task::spawn_blocking(mac::read).await.map_err(ProviderError::other)?;
                    listeners.track(&r.process_objects, &r.device_objects);
                    if last.as_ref() != Some(&r.state) {
                        if !ctx.emit(notchforge_core::provider::ProviderEvent::Audio(r.state.clone())).await {
                            return Ok(());
                        }
                        last = Some(r.state);
                    }
                    let refresh = ctx.refresh.clone();
                    tokio::select! {
                        _ = mac::changed().notified() => {}
                        _ = refresh.notified() => {}
                        _ = tokio::time::sleep(FALLBACK_POLL) => {}
                        cmd = ctx.next_command() => match cmd {
                            None => return Ok(()),
                            Some(notchforge_core::provider::ProviderCommand::SetOutputDevice(uid)) => {
                                let res = tokio::task::spawn_blocking(move || mac::set_output(&uid))
                                    .await
                                    .map_err(ProviderError::other)?;
                                if let Err(e) = res {
                                    tracing::warn!(error = %e, "could not switch audio output");
                                }
                            }
                            Some(_) => continue,
                        },
                    }
                    tokio::time::sleep(DEBOUNCE).await;
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                ctx.set_status(ProviderStatus::Failing("audio devices are only available on macOS".into())).await;
                ctx.shutdown.wait().await;
                Ok(())
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, t: AudioTransport, default: bool) -> AudioDevice {
        AudioDevice {
            id: 1,
            uid: name.into(),
            name: name.into(),
            transport: t,
            sample_rate: None,
            max_sample_rate: None,
            bit_depth: None,
            output_channels: 2,
            volume: None,
            muted: None,
            is_default: default,
        }
    }

    #[test]
    fn devices_sort_default_first() {
        let mut d = vec![
            dev("USB DAC", AudioTransport::Usb, false),
            dev("AirPods", AudioTransport::Bluetooth, true),
            dev("MacBook Pro Speakers", AudioTransport::BuiltIn, false),
        ];
        sort_devices(&mut d);
        let names: Vec<_> = d.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["AirPods", "MacBook Pro Speakers", "USB DAC"]);
    }

    #[test]
    fn helpers_merge_into_their_app() {
        let src = |name: &str, bundle: Option<&str>, pid, uids: &[&str]| RawSource {
            name: name.into(),
            bundle_id: bundle.map(str::to_owned),
            app_path: None,
            pid,
            device_uids: uids.iter().map(|u| u.to_string()).collect(),
        };
        let raw = vec![
            src("Google Chrome", Some("com.google.Chrome"), 10, &["A"]),
            src("Google Chrome", Some("com.google.Chrome"), 11, &["A", "B"]),
            src("coreaudiod", None, 5, &[]),
            src("Spotify", Some("com.spotify.client"), 20, &[]),
        ];
        let s = group_sources(raw);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name, "Google Chrome");
        assert_eq!(s[0].device_uids, ["A", "B"]);
        assert_eq!(s[1].name, "Spotify");
    }
}
