//! # notchforge-providers
//!
//! Concrete [`Provider`](notchforge_core::provider::Provider)s. Each lives in
//! its own module and is registered in [`registry`]. To add one, implement the
//! trait (see the docs on `notchforge_core::provider`) and append a
//! [`ProviderSpec`] below.

pub mod agents;
pub mod audio;
pub mod battery;
pub mod bluetooth;
pub mod focus;
pub mod media;
pub mod sys;
pub mod usernoted;
pub mod util;
pub mod weather;

use notchforge_core::provider::{Provider, ProviderSpec};

fn spec<P: Provider + Default>(
    enabled: fn(&notchforge_core::config::Config) -> bool,
    restart_key: fn(&notchforge_core::config::Config) -> String,
) -> ProviderSpec {
    ProviderSpec {
        descriptor: P::default().descriptor(),
        enabled,
        restart_key,
        make: Box::new(|| Box::new(P::default())),
    }
}

macro_rules! unit_default {
    ($($t:ty),*) => { $(impl Default for $t { fn default() -> Self { Self } })* };
}
unit_default!(
    battery::BatteryProvider,
    bluetooth::BluetoothProvider,
    weather::WeatherProvider,
    agents::HttpAgentProvider,
    agents::SocketAgentProvider,
    agents::InboxAgentProvider,
    agents::AgentWatchProvider,
    agents::AgentUsageProvider,
    usernoted::UsernotedProvider,
    focus::FocusProvider,
    audio::AudioProvider,
    media::MediaProvider
);

/// Every built-in provider. The macOS layer adds platform-bound ones
/// (Accessibility banner observer) separately because they need the main run loop.
pub fn registry() -> Vec<ProviderSpec> {
    vec![
        spec::<battery::BatteryProvider>(|c| c.providers.battery, |_| String::new()),
        spec::<bluetooth::BluetoothProvider>(|c| c.providers.bluetooth, |_| String::new()),
        spec::<weather::WeatherProvider>(|c| c.providers.weather, |_| String::new()),
        spec::<agents::HttpAgentProvider>(
            |c| c.providers.agents && c.agents.http_enabled,
            |c| c.agents.http_port.to_string(),
        ),
        spec::<agents::SocketAgentProvider>(
            |c| c.providers.agents && c.agents.socket_enabled,
            |c| format!("{:?}", c.agents.socket_path),
        ),
        spec::<agents::InboxAgentProvider>(
            |c| c.providers.agents && c.agents.inbox_enabled,
            |c| format!("{:?}", c.agents.inbox_path),
        ),
        spec::<agents::AgentWatchProvider>(|c| c.providers.agents && c.agents.watch_sessions, |_| String::new()),
        spec::<agents::AgentUsageProvider>(|c| c.providers.agents && c.agents.token_usage, |_| String::new()),
        spec::<usernoted::UsernotedProvider>(|c| c.providers.notification_center, |_| String::new()),
        spec::<focus::FocusProvider>(|c| c.providers.focus, |_| String::new()),
        spec::<audio::AudioProvider>(|c| c.providers.audio, |_| String::new()),
        spec::<media::MediaProvider>(|c| c.providers.media, |_| String::new()),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn registry_ids_are_unique() {
        let specs = super::registry();
        let mut ids: Vec<_> = specs.iter().map(|s| s.descriptor.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), specs.len());
    }
}
