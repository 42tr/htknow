//! Runtime application settings persisted in SQLite.

use std::{collections::BTreeMap, sync::{OnceLock, RwLock}};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use serde_json::{Value, json};
use sqlx::{Row, SqlitePool};

use crate::config;

static SETTINGS: OnceLock<RwLock<BTreeMap<String, Value>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SettingItem {
    pub value: Value,
    pub value_type: &'static str,
    pub group: &'static str,
    pub editable: bool,
    pub source: &'static str,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateSettingsRequest {
    pub settings: BTreeMap<String, Value>,
}

pub const IMAGE_PARSE_MODE: &str = "image_parse.mode";
pub const IMAGE_PARSE_URL: &str = "image_parse.url";
pub const IMAGE_OCR_URL: &str = "image_parse.ocr_url";
pub const IMAGE_PARSE_TIMEOUT: &str = "image_parse.timeout_secs";
pub const IMAGE_PARSE_CONCURRENCY: &str = "image_parse.concurrency";

fn definitions() -> BTreeMap<&'static str, (&'static str, &'static str)> {
    BTreeMap::from([
        (IMAGE_PARSE_MODE, ("image_parse", "enum")),
        (IMAGE_PARSE_URL, ("image_parse", "url")),
        (IMAGE_OCR_URL, ("image_parse", "url")),
        (IMAGE_PARSE_TIMEOUT, ("image_parse", "integer")),
        (IMAGE_PARSE_CONCURRENCY, ("image_parse", "integer")),
    ])
}

fn defaults() -> BTreeMap<String, Value> {
    let cfg = config::get();
    BTreeMap::from([
        (IMAGE_PARSE_MODE.to_string(), json!(if cfg.services.image_parse_url.is_some() { "custom" } else { "none" })),
        (IMAGE_PARSE_URL.to_string(), json!(cfg.services.image_parse_url)),
        (IMAGE_OCR_URL.to_string(), json!(std::env::var("HTKNOW_IMAGE_OCR_URL").ok().filter(|v| !v.trim().is_empty()))),
        (IMAGE_PARSE_TIMEOUT.to_string(), json!(cfg.services.image_parse_timeout_secs)),
        (IMAGE_PARSE_CONCURRENCY.to_string(), json!(cfg.services.image_parse_concurrency)),
    ])
}

pub async fn init(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS app_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_by TEXT,
            updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
        )",
    ).execute(pool).await?;

    let mut values = defaults();
    let rows = sqlx::query("SELECT key, value FROM app_settings").fetch_all(pool).await?;
    for row in rows {
        let key: String = row.get("key");
        let raw: String = row.get("value");
        if definitions().contains_key(key.as_str()) {
            if let Ok(value) = serde_json::from_str(&raw) {
                values.insert(key, value);
            }
        }
    }
    SETTINGS.set(RwLock::new(values)).ok();
    Ok(())
}

pub fn get(key: &str) -> Option<Value> {
    SETTINGS.get()?.read().ok()?.get(key).cloned()
}

pub fn image_parse_mode() -> String {
    get(IMAGE_PARSE_MODE).and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_else(|| "none".to_string())
}

pub fn image_parse_url() -> Option<String> {
    get(IMAGE_PARSE_URL).and_then(|v| v.as_str().map(str::to_owned)).filter(|v| !v.trim().is_empty())
}

pub fn image_ocr_url() -> Option<String> {
    get(IMAGE_OCR_URL).and_then(|v| v.as_str().map(str::to_owned)).filter(|v| !v.trim().is_empty())
}

pub fn image_parse_timeout_secs() -> u64 {
    get(IMAGE_PARSE_TIMEOUT).and_then(|v| v.as_u64()).unwrap_or(120)
}

pub fn image_parse_concurrency() -> usize {
    get(IMAGE_PARSE_CONCURRENCY).and_then(|v| v.as_u64()).unwrap_or(5) as usize
}

pub fn validate(updates: &BTreeMap<String, Value>) -> anyhow::Result<()> {
    let defs = definitions();
    for (key, value) in updates {
        let Some((_, value_type)) = defs.get(key.as_str()) else {
            anyhow::bail!("unknown setting: {}", key);
        };
        match (*value_type, value) {
            ("enum", Value::String(v)) if ["ocr", "custom", "none"].contains(&v.as_str()) => {}
            ("url", Value::String(v)) if v.is_empty() || v.starts_with("http://") || v.starts_with("https://") => {}
            ("integer", Value::Number(v)) if v.as_u64().is_some() => {}
            _ => anyhow::bail!("invalid value for setting: {}", key),
        }
    }
    if let Some(mode) = updates.get(IMAGE_PARSE_MODE).and_then(Value::as_str) {
        let (key, configured_url) = if mode == "ocr" {
            (IMAGE_OCR_URL, image_ocr_url())
        } else {
            (IMAGE_PARSE_URL, image_parse_url())
        };
        let url = updates.get(key).and_then(Value::as_str).or(configured_url.as_deref());
        if matches!(mode, "ocr" | "custom") && url.map(str::trim).filter(|v| !v.is_empty()).is_none() {
            anyhow::bail!("{} is required for image parse mode {}", key, mode);
        }
    }
    if let Some(timeout) = updates.get(IMAGE_PARSE_TIMEOUT).and_then(Value::as_u64) {
        if !(1..=600).contains(&timeout) { anyhow::bail!("image_parse.timeout_secs must be between 1 and 600"); }
    }
    if let Some(concurrency) = updates.get(IMAGE_PARSE_CONCURRENCY).and_then(Value::as_u64) {
        if !(1..=50).contains(&concurrency) { anyhow::bail!("image_parse.concurrency must be between 1 and 50"); }
    }
    Ok(())
}

pub async fn update(pool: &SqlitePool, user: &str, updates: &BTreeMap<String, Value>) -> anyhow::Result<()> {
    validate(updates)?;
    let mut tx = pool.begin().await?;
    for (key, value) in updates {
        sqlx::query("INSERT INTO app_settings(key, value, updated_by) VALUES (?, ?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_by=excluded.updated_by, updated_at=strftime('%s','now')")
            .bind(key).bind(serde_json::to_string(value)?).bind(user).execute(&mut *tx).await?;
    }
    tx.commit().await?;

    let current = SETTINGS.get().context("runtime settings are not initialized")?;
    let mut guard = current.write().map_err(|_| anyhow::anyhow!("runtime settings lock poisoned"))?;
    guard.extend(updates.clone());
    Ok(())
}

pub fn list(group: Option<&str>) -> BTreeMap<String, SettingItem> {
    let values = SETTINGS.get().and_then(|s| s.read().ok()).map(|v| v.clone()).unwrap_or_else(defaults);
    definitions().into_iter().filter(|(_, (g, _))| group.is_none_or(|wanted| wanted == *g)).map(|(key, (group, value_type))| {
        (key.to_string(), SettingItem { value: values.get(key).cloned().unwrap_or(Value::Null), value_type, group, editable: true, source: "runtime" })
    }).collect()
}
