//! Coding-agent hub.
//!
//! Coding agents are first-class citizens in NotchForge. Three transports
//! accept the same JSON event schema ([`payload`]):
//!
//! | Transport | Best for |
//! |---|---|
//! | [`http`] — `POST http://127.0.0.1:47815/v1/notify` | hooks (Claude Code, OpenCode plugins), CI scripts, `curl` |
//! | [`socket`] — `<data dir>/agent.sock` (JSON lines) | long-running local tools; no TCP port needed |
//! | [`inbox`] — `<data dir>/inbox.jsonl` (append lines) | the simplest shell one-liners, editors' task runners |
//!
//! Desktop agents that post regular macOS notifications (Claude, ChatGPT,
//! Cursor, Zed, Qoder…) are additionally picked up by the Notification Center
//! watcher and attributed through [`KnownApp`](notchforge_core::apps::KnownApp).

pub mod http;
pub mod inbox;
pub mod payload;
pub mod socket;

pub use http::HttpAgentProvider;
pub use inbox::InboxAgentProvider;
pub use socket::SocketAgentProvider;
