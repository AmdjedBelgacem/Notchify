//! Antigravity's Gemini usage, read from its own language server.
//!
//! Antigravity meters agent work in **prompt credits** (per model), not raw
//! tokens, and keeps the numbers only in its language server process — nothing
//! is written to disk. While Antigravity runs, that process exposes a local
//! Connect endpoint:
//!
//! ```text
//! POST https://127.0.0.1:<port>/exa.language_server_pb.LanguageServerService/GetUserStatus
//! X-Codeium-Csrf-Token: <token from the process arguments>
//! ```
//!
//! The port comes from `lsof` on the server's pid, the CSRF token from its
//! command line — the same discovery Antigravity's own quota extension uses.
//! The response carries the plan's monthly/available prompt credits and, per
//! model, a `remainingFraction` with a reset time. Everything stays local; no
//! new permission is needed.

use crate::agents::usage::parse_time;
use notchforge_core::model::{AgentLimits, LimitGauge, ProviderStatus, UsageAgent};
use notchforge_core::provider::{
    BoxFuture, Provider, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent, Wake,
};
use serde::Deserialize;
use std::time::Duration;

pub struct AntigravityProvider;

/// Poll while Antigravity runs; its language server answers in milliseconds.
const POLL: Duration = Duration::from_secs(60);
/// Discovery retry while it doesn't.
const IDLE_POLL: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Default)]
struct Endpoint {
    port: u16,
    csrf: String,
}

/// Find the language server: its command line carries the CSRF token and marks
/// it as Antigravity's (`--app_data_dir antigravity`). Returns one candidate
/// per listening port — only one answers `GetUserStatus`, so callers try them
/// in order.
fn discover() -> Vec<Endpoint> {
    let Ok(out) = std::process::Command::new("pgrep").args(["-fl", "language_server"]).output() else {
        return Vec::new();
    };
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let Some(line) = stdout.lines().find(|l| l.contains("--app_data_dir antigravity")) else {
        return Vec::new();
    };
    let Some(csrf) = line.split("--csrf_token").nth(1).and_then(|t| t.split_whitespace().next()).map(str::to_owned)
    else {
        return Vec::new();
    };
    let Ok(pid) = line.split_whitespace().next().unwrap_or("").parse::<i32>() else {
        return Vec::new();
    };
    // The server listens on random loopback ports; find them all.
    let Ok(out) = std::process::Command::new("lsof")
        .args(["-nP", "-a", "-iTCP", "-sTCP:LISTEN", "-p", &pid.to_string()])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.rsplit(':').next()?.split_whitespace().next()?.parse::<u16>().ok())
        .map(|port| Endpoint { port, csrf: csrf.clone() })
        .collect()
}

#[derive(Debug, Clone, Deserialize)]
struct StatusResponse {
    #[serde(rename = "userStatus")]
    user_status: UserStatus,
}

#[derive(Debug, Clone, Deserialize)]
struct UserStatus {
    #[serde(rename = "planStatus", default)]
    plan_status: Option<PlanStatus>,
    #[serde(rename = "cascadeModelConfigData", default)]
    models: Option<ModelConfigData>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PlanStatus {
    #[serde(rename = "availablePromptCredits", default)]
    available_credits: Option<f64>,
    #[serde(rename = "planInfo", default)]
    plan_info: Option<PlanInfo>,
}

#[derive(Debug, Clone, Deserialize)]
struct PlanInfo {
    #[serde(rename = "monthlyPromptCredits", default)]
    monthly_credits: Option<f64>,
    #[serde(rename = "planName", default)]
    name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelConfigData {
    #[serde(rename = "clientModelConfigs", default)]
    configs: Vec<ModelConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelConfig {
    #[serde(rename = "label", default)]
    label: Option<String>,
    #[serde(rename = "quotaInfo", default)]
    quota_info: Option<QuotaInfo>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuotaInfo {
    #[serde(rename = "remainingFraction", default)]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime", default)]
    reset_time: Option<String>,
}

/// Ask the language server for the user status, trying each candidate port.
async fn get_user_status(http: &reqwest::Client, endpoints: &[Endpoint]) -> Result<StatusResponse, String> {
    let mut last = "no candidate ports".to_owned();
    for ep in endpoints {
        let url = format!("https://127.0.0.1:{}/exa.language_server_pb.LanguageServerService/GetUserStatus", ep.port);
        let resp = http
            .post(&url)
            .header("Connect-Protocol-Version", "1")
            .header("X-Codeium-Csrf-Token", &ep.csrf)
            .json(&serde_json::json!({
                "metadata": {"ideName": "antigravity", "extensionName": "antigravity", "locale": "en"}
            }))
            .timeout(Duration::from_secs(5))
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                last = e.to_string();
                continue;
            }
        };
        match resp.error_for_status() {
            Ok(r) => match r.json::<StatusResponse>().await {
                Ok(status) => return Ok(status),
                Err(e) => last = e.to_string(),
            },
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

/// Turn the response into the Agents tab's limits tile: one gauge for the
/// plan's prompt credits, one per model with quota info.
fn to_limits(plan: &PlanStatus, models: &[ModelConfig]) -> AgentLimits {
    let mut gauges = Vec::new();
    if let (Some(monthly), Some(available)) =
        (plan.plan_info.as_ref().and_then(|p| p.monthly_credits).filter(|m| *m > 0.0), plan.available_credits)
    {
        let used = ((monthly - available) / monthly * 100.0).round().clamp(0.0, 100.0);
        gauges.push(LimitGauge { label: "Prompt credits".into(), percent: used as u8, resets_at_ms: None });
    }
    let mut seen: Vec<(u64, i64)> = Vec::new();
    for m in models {
        let Some(q) = &m.quota_info else { continue };
        let Some(fraction) = q.remaining_fraction else { continue };
        let used = ((1.0 - fraction) * 100.0).round().clamp(0.0, 100.0) as u8;
        let resets_at_ms = q.reset_time.as_deref().and_then(parse_time);
        // The same model in High/Medium/Low variants shares one quota: one ring.
        let key = (used as u64, resets_at_ms.unwrap_or(0));
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        gauges.push(LimitGauge {
            label: m.label.clone().unwrap_or_else(|| "Model".into()),
            percent: used,
            resets_at_ms,
        });
    }
    AgentLimits {
        agent: UsageAgent::Antigravity,
        connected: true,
        plan: plan.plan_info.as_ref().and_then(|p| p.name.clone()),
        running: true,
        project: None,
        gauges,
        context_percent: None,
    }
}

impl Provider for AntigravityProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "antigravity", name: "Antigravity Gemini", permissions: &[] }
    }

    fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            let http = reqwest::Client::builder()
                .danger_accept_invalid_certs(true) // the language server self-signs
                .timeout(Duration::from_secs(5))
                .user_agent(concat!("NotchForge/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(ProviderError::other)?;
            ctx.set_status(ProviderStatus::Running).await;
            let mut endpoints: Vec<Endpoint> = Vec::new();
            let refresh = ctx.refresh.clone();
            loop {
                let poll = if endpoints.is_empty() { IDLE_POLL } else { POLL };
                let wake = tokio::select! {
                    w = ctx.sleep(poll) => w,
                    _ = refresh.notified() => Wake::Refresh,
                };
                if wake == Wake::Shutdown {
                    return Ok(());
                }
                if endpoints.is_empty() {
                    endpoints = tokio::task::spawn_blocking(discover).await.unwrap_or_default();
                    if endpoints.is_empty() {
                        // Antigravity is closed: report that in the tile.
                        let _ = ctx
                            .emit(ProviderEvent::Limits(AgentLimits {
                                agent: UsageAgent::Antigravity,
                                connected: false,
                                plan: None,
                                running: false,
                                project: None,
                                gauges: Vec::new(),
                                context_percent: None,
                            }))
                            .await;
                        continue;
                    }
                    tracing::info!(ports = endpoints.len(), "found Antigravity's language server");
                }
                match get_user_status(&http, &endpoints).await {
                    Ok(status) => {
                        let plan = status.user_status.plan_status.clone().unwrap_or_default();
                        let models = status.user_status.models.map(|m| m.configs).unwrap_or_default();
                        let _ = ctx.emit(ProviderEvent::Limits(to_limits(&plan, &models))).await;
                    }
                    Err(e) => {
                        // The server rotates ports/tokens between restarts: rediscover.
                        tracing::debug!(error = %e, "Antigravity status read failed; rediscovering");
                        endpoints = Vec::new();
                        let _ = ctx
                            .emit(ProviderEvent::Limits(AgentLimits {
                                agent: UsageAgent::Antigravity,
                                connected: false,
                                plan: None,
                                running: false,
                                project: None,
                                gauges: Vec::new(),
                                context_percent: None,
                            }))
                            .await;
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A captured GetUserStatus response (Pro plan, 500/50000 credits,
    /// per-model quotas with reset times).
    #[test]
    fn parses_the_plan_and_model_quotas() {
        let data = r#"{"userStatus":{"name":"Test","planStatus":{"planInfo":{"teamsTier":"TEAMS_TIER_PRO","planName":"Pro","monthlyPromptCredits":50000},"availablePromptCredits":500},"cascadeModelConfigData":{"clientModelConfigs":[
            {"label":"Gemini 3.8 Flash (High)","quotaInfo":{"remainingFraction":0.9120825,"resetTime":"2026-11-05T02:25:34Z"}},
            {"label":"Gemini 3.8 Flash (Medium)","quotaInfo":{"remainingFraction":0.9120825,"resetTime":"2026-11-05T02:25:34Z"}},
            {"label":"Claude Sonnet 4.6 (Thinking)","quotaInfo":{"remainingFraction":1.0,"resetTime":"2026-11-05T02:56:14Z"}},
            {"label":"Gemini 4 Argon"}
        ]}}}"#;
        let status: StatusResponse = serde_json::from_str(data).unwrap();
        let plan = status.user_status.plan_status.clone().unwrap_or_default();
        let models = status.user_status.models.map(|m| m.configs).unwrap_or_default();
        let limits = to_limits(&plan, &models);
        assert_eq!(limits.agent, UsageAgent::Antigravity);
        assert!(limits.connected);
        assert_eq!(limits.plan.as_deref(), Some("Pro"));
        // The credits gauge: (50000-500)/50000 = 99%.
        assert_eq!(limits.gauges[0].label, "Prompt credits");
        assert_eq!(limits.gauges[0].percent, 99);
        // Same-fraction variants collapse into one ring.
        assert_eq!(limits.gauges[1].label, "Gemini 3.8 Flash (High)");
        assert_eq!(limits.gauges[1].percent, 9);
        assert!(limits.gauges[1].resets_at_ms.is_some());
        assert!(limits.gauges.iter().any(|g| g.label.contains("Claude")), "no duplicate rings: {:?}", limits.gauges);
        assert_eq!(limits.gauges.len(), 3, "credits + 2 deduped models: {:?}", limits.gauges);
    }
}
