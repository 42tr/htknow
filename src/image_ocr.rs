//! External OCR service client.

use std::time::Duration;

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;

use crate::settings;

static HTTP_CLIENT: Lazy<Client> = Lazy::new(Client::new);

#[derive(Debug, Serialize)]
struct OcrRequest {
    figure_base64: String,
}

pub async fn parse_base64(image_base64: &str) -> Result<crate::image_parse::ImageParseResponse> {
    let url = settings::image_ocr_url().ok_or_else(|| anyhow::anyhow!("image OCR URL is not configured"))?;
    let payload = image_base64
        .find("base64,")
        .map(|idx| &image_base64[idx + "base64,".len()..])
        .unwrap_or(image_base64)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    let decoded_data = STANDARD.decode(&payload).context("invalid image base64")?;
    let request = OcrRequest {
        figure_base64: STANDARD.encode(decoded_data),
    };

    let response = HTTP_CLIENT
        .post(&url)
        .timeout(Duration::from_secs(settings::image_parse_timeout_secs()))
        .json(&request)
        .send()
        .await
        .with_context(|| format!("OCR request failed: url={}", url))?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("OCR API error: {} - {}", status, text);
    }

    let raw_response = response.text().await.unwrap_or_default();
    let value: Value = serde_json::from_str(&raw_response).context("invalid OCR response JSON")?;
    let description = value
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("OCR response is missing string field: data"))?
        .trim()
        .to_string();
    Ok(crate::image_parse::ImageParseResponse { description, raw_response })
}
