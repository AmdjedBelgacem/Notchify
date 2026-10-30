//! Weather via [Open-Meteo](https://open-meteo.com): free, no API key, no
//! account, no tracking. A custom base URL/key can point at a commercial or
//! self-hosted instance.
//!
//! Location resolution order:
//! 1. explicit `weather.latitude/longitude`,
//! 2. `weather.location = "auto"` → CoreLocation fix supplied by the macOS
//!    layer through [`ProviderContext::location`],
//! 3. any other string → Open-Meteo geocoding (cached).

use notchforge_core::config::WeatherConfig;
use notchforge_core::model::{
    Coordinates, HourlyForecast, Permission, ProviderStatus, TemperatureUnit, Weather, WeatherCondition,
};
use notchforge_core::provider::{BoxFuture, Provider, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent, Wake};
use notchforge_core::time::now_ms;
use serde::Deserialize;
use std::time::Duration;

/// WMO weather interpretation codes → condition.
pub fn wmo_condition(code: i64) -> WeatherCondition {
    match code {
        0 => WeatherCondition::Clear,
        1 | 2 => WeatherCondition::PartlyCloudy,
        3 => WeatherCondition::Cloudy,
        45 | 48 => WeatherCondition::Fog,
        51 | 53 | 55 => WeatherCondition::Drizzle,
        56 | 57 | 66 | 67 => WeatherCondition::FreezingRain,
        61 | 63 | 65 => WeatherCondition::Rain,
        71 | 73 | 75 | 77 | 85 | 86 => WeatherCondition::Snow,
        80..=82 => WeatherCondition::Showers,
        95 | 96 | 99 => WeatherCondition::Thunderstorm,
        _ => WeatherCondition::Unknown,
    }
}

#[derive(Debug, Deserialize)]
struct Forecast {
    current: Current,
    hourly: Hourly,
    daily: Daily,
}

#[derive(Debug, Deserialize)]
struct Current {
    time: String,
    temperature_2m: f32,
    apparent_temperature: f32,
    weather_code: i64,
    is_day: i64,
}

#[derive(Debug, Deserialize)]
struct Hourly {
    time: Vec<String>,
    temperature_2m: Vec<f32>,
    weather_code: Vec<i64>,
    #[serde(default)]
    precipitation_probability: Vec<Option<f32>>,
}

#[derive(Debug, Deserialize)]
struct Daily {
    temperature_2m_max: Vec<f32>,
    temperature_2m_min: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct GeoResponse {
    #[serde(default)]
    results: Vec<GeoResult>,
}

#[derive(Debug, Deserialize)]
struct GeoResult {
    name: String,
    latitude: f64,
    longitude: f64,
}

pub fn forecast_url(cfg: &WeatherConfig, at: Coordinates) -> String {
    let mut url = format!(
        "{}?latitude={:.4}&longitude={:.4}\
         &current=temperature_2m,apparent_temperature,weather_code,is_day\
         &hourly=temperature_2m,weather_code,precipitation_probability\
         &daily=temperature_2m_max,temperature_2m_min&forecast_days=2&timezone=auto",
        cfg.api_base, at.latitude, at.longitude
    );
    if cfg.unit == TemperatureUnit::Fahrenheit {
        url.push_str("&temperature_unit=fahrenheit");
    }
    if let Some(key) = cfg.api_key.as_deref().filter(|k| !k.is_empty()) {
        url.push_str("&apikey=");
        url.push_str(key);
    }
    url
}

/// Turn an Open-Meteo forecast response into our model.
pub fn parse_forecast(json: &[u8], place: &str, unit: TemperatureUnit) -> Result<Weather, String> {
    let f: Forecast = serde_json::from_slice(json).map_err(|e| format!("bad forecast: {e}"))?;
    // Hourly times are "YYYY-MM-DDTHH:00" and current is "YYYY-MM-DDTHH:MM"
    // in the same (local) timezone, so prefix comparison finds "this hour".
    let this_hour = f.current.time.get(..13).unwrap_or(&f.current.time);
    let start = f.hourly.time.iter().position(|t| t.as_str() >= this_hour).unwrap_or(0);
    let hourly = (start + 1..start + 7)
        .filter_map(|i| {
            Some(HourlyForecast {
                hour: f.hourly.time.get(i)?.get(11..13)?.to_owned(),
                temperature: *f.hourly.temperature_2m.get(i)?,
                condition: wmo_condition(*f.hourly.weather_code.get(i)?),
                precipitation_chance: f.hourly.precipitation_probability.get(i).copied().flatten().map(|p| p as u8),
            })
        })
        .collect();
    Ok(Weather {
        place: place.to_owned(),
        temperature: f.current.temperature_2m,
        feels_like: f.current.apparent_temperature,
        condition: wmo_condition(f.current.weather_code),
        is_day: f.current.is_day != 0,
        high: f.daily.temperature_2m_max.first().copied().unwrap_or(f.current.temperature_2m),
        low: f.daily.temperature_2m_min.first().copied().unwrap_or(f.current.temperature_2m),
        unit,
        hourly,
        fetched_at_ms: now_ms(),
    })
}

pub struct WeatherProvider;

struct Resolver {
    client: reqwest::Client,
    cached: Option<(String, Coordinates, String)>,
}

impl Resolver {
    /// Returns coordinates and a display name.
    async fn resolve(&mut self, ctx: &mut ProviderContext, cfg: &WeatherConfig) -> Result<(Coordinates, String), ProviderError> {
        if let (Some(latitude), Some(longitude)) = (cfg.latitude, cfg.longitude) {
            let name = if cfg.location.eq_ignore_ascii_case("auto") { "Pinned location".into() } else { cfg.location.clone() };
            return Ok((Coordinates { latitude, longitude }, name));
        }
        if cfg.location.trim().eq_ignore_ascii_case("auto") {
            // Wait briefly for CoreLocation's first fix.
            for _ in 0..2 {
                if let Some(c) = *ctx.location.borrow() {
                    return Ok((c, "Current location".into()));
                }
                tokio::select! {
                    _ = ctx.location.changed() => {},
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {},
                }
            }
            return match *ctx.location.borrow() {
                Some(c) => Ok((c, "Current location".into())),
                None => Err(ProviderError::NeedsPermission(Permission::Location)),
            };
        }
        if let Some((q, c, name)) = &self.cached {
            if *q == cfg.location {
                return Ok((*c, name.clone()));
            }
        }
        let resp: GeoResponse = self
            .client
            .get(&cfg.geocoding_base)
            .query(&[("name", cfg.location.as_str()), ("count", "1"), ("language", "en"), ("format", "json")])
            .send()
            .await
            .map_err(ProviderError::other)?
            .error_for_status()
            .map_err(ProviderError::other)?
            .json()
            .await
            .map_err(ProviderError::other)?;
        let hit = resp
            .results
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::Unavailable(format!("place not found: {}", cfg.location)))?;
        let c = Coordinates { latitude: hit.latitude, longitude: hit.longitude };
        self.cached = Some((cfg.location.clone(), c, hit.name.clone()));
        Ok((c, hit.name))
    }
}

impl Provider for WeatherProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "weather", name: "Weather", permissions: &[Permission::Location] }
    }

    fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent(concat!("NotchForge/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(ProviderError::other)?;
            let mut resolver = Resolver { client: client.clone(), cached: None };
            let mut last_fetch: Option<std::time::Instant>;
            loop {
                let cfg = ctx.config().weather.clone();
                let (coords, place) = resolver.resolve(&mut ctx, &cfg).await?;
                let url = forecast_url(&cfg, coords);
                let body = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(ProviderError::other)?
                    .error_for_status()
                    .map_err(ProviderError::other)?
                    .bytes()
                    .await
                    .map_err(ProviderError::other)?;
                let weather = parse_forecast(&body, &place, cfg.unit).map_err(ProviderError::Other)?;
                last_fetch = Some(std::time::Instant::now());
                ctx.set_status(ProviderStatus::Running).await;
                if !ctx.emit(ProviderEvent::Weather(weather)).await {
                    return Ok(());
                }
                let interval = Duration::from_secs(cfg.refresh_minutes * 60);
                let mut cfg_rx = ctx.config.clone();
                loop {
                    let wake = tokio::select! {
                        w = ctx.sleep_or_refresh(interval) => w,
                        // Location/unit changes refetch right away.
                        _ = cfg_rx.changed() => Wake::Refresh,
                    };
                    match wake {
                        Wake::Shutdown => return Ok(()),
                        Wake::Timeout => break,
                        // Panel opened: only refetch if the data is > 10 min old.
                        Wake::Refresh => {
                            let stale = last_fetch.is_none_or(|t| t.elapsed() > Duration::from_secs(600));
                            if stale || ctx.config().weather != cfg {
                                break;
                            }
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "latitude": 48.86, "longitude": 2.35,
      "current": {"time": "2026-10-30T14:45", "interval": 900, "temperature_2m": 13.4,
                  "apparent_temperature": 11.9, "weather_code": 3, "is_day": 1},
      "hourly": {
        "time": ["2026-10-30T13:00","2026-10-30T14:00","2026-10-30T15:00","2026-10-30T16:00",
                 "2026-10-30T17:00","2026-10-30T18:00","2026-10-30T19:00","2026-10-30T20:00"],
        "temperature_2m": [12.0, 13.0, 13.5, 13.1, 12.2, 11.0, 10.4, 9.9],
        "weather_code": [3, 3, 61, 61, 2, 0, 0, 0],
        "precipitation_probability": [10, 20, 70, 65, null, 5, 0, 0]
      },
      "daily": {"temperature_2m_max": [14.2, 15.0], "temperature_2m_min": [8.1, 7.5]}
    }"#;

    #[test]
    fn parses_forecast() {
        let w = parse_forecast(SAMPLE.as_bytes(), "Paris", TemperatureUnit::Celsius).expect("parse");
        assert_eq!(w.place, "Paris");
        assert_eq!(w.condition, WeatherCondition::Cloudy);
        assert!(w.is_day);
        assert_eq!((w.high, w.low), (14.2, 8.1));
        assert_eq!(w.hourly.len(), 6);
        assert_eq!(w.hourly[0].hour, "15");
        assert_eq!(w.hourly[0].condition, WeatherCondition::Rain);
        assert_eq!(w.hourly[0].precipitation_chance, Some(70));
        assert_eq!(w.hourly[2].precipitation_chance, None);
    }

    #[test]
    fn rejects_bad_json() {
        assert!(parse_forecast(b"{}", "x", TemperatureUnit::Celsius).is_err());
    }

    #[test]
    fn wmo_mapping() {
        assert_eq!(wmo_condition(0), WeatherCondition::Clear);
        assert_eq!(wmo_condition(81), WeatherCondition::Showers);
        assert_eq!(wmo_condition(99), WeatherCondition::Thunderstorm);
        assert_eq!(wmo_condition(1234), WeatherCondition::Unknown);
    }

    #[test]
    fn url_building() {
        let mut cfg = WeatherConfig { unit: TemperatureUnit::Fahrenheit, ..WeatherConfig::default() };
        cfg.api_key = Some("k".into());
        let url = forecast_url(&cfg, Coordinates { latitude: 1.0, longitude: 2.0 });
        assert!(url.starts_with("https://api.open-meteo.com/v1/forecast?latitude=1.0000&longitude=2.0000"));
        assert!(url.contains("temperature_unit=fahrenheit"));
        assert!(url.ends_with("&apikey=k"));
    }
}
