//! 外部图片文本化服务客户端
//!
//! 当配置了 `HTKNOW_IMAGE_PARSE_URL` 时，处理器会把遇到的每张图片发送到该服务，
//! 获取文本化描述并持久化。本模块负责构造请求、解析多种可能的响应格式。

use std::time::Duration;

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;

use crate::config;

static HTTP_CLIENT: Lazy<Client> = Lazy::new(Client::new);

#[derive(Debug, Serialize)]
struct ParseImageRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    filename: String,
    image_base64: String,
}

/// 图片文本化结果
#[derive(Debug, Clone)]
pub struct ImageParseResponse {
    /// 提取后的描述文本
    pub description: String,
    /// 服务返回的原始响应（JSON 或纯文本），用于恢复/排查
    pub raw_response: String,
}

/// 从本地图片文件调用文本化服务
pub async fn parse_image_file(
    path: &std::path::Path, filename: &str, surrounding_content: Option<&str>,
) -> Result<ImageParseResponse> {
    let bytes =
        tokio::fs::read(path).await.with_context(|| format!("failed to read image file: {}", path.display()))?;
    let base64 = STANDARD.encode(&bytes);
    parse_image_base64(filename, &base64, surrounding_content).await
}

/// 从 base64 编码的图片内容调用文本化服务
pub async fn parse_image_base64(
    filename: &str, image_base64: &str, surrounding_content: Option<&str>,
) -> Result<ImageParseResponse> {
    let cfg = config::get();
    let url =
        cfg.services.image_parse_url.as_deref().ok_or_else(|| anyhow::anyhow!("image parse URL is not configured"))?;

    // 兼容 data:image/xxx;base64, 前缀以及空白字符
    let payload = image_base64
        .find("base64,")
        .map(|idx| &image_base64[idx + "base64,".len()..])
        .unwrap_or(image_base64)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();

    let request = ParseImageRequest {
        content: surrounding_content.map(|s| s.to_string()),
        filename: filename.to_string(),
        image_base64: payload,
    };

    let response = HTTP_CLIENT
        .post(url)
        .timeout(Duration::from_secs(cfg.services.image_parse_timeout_secs))
        .json(&request)
        .send()
        .await
        .with_context(|| format!("image parse request failed: url={}, filename={}", url, filename))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("image parse API error: {} - {}", status, text);
    }

    let raw_response = response.text().await.unwrap_or_default();
    let description = extract_description(&raw_response);
    Ok(ImageParseResponse { description, raw_response })
}

/// 从服务原始响应中提取描述文本，兼容多种 JSON 结构，解析失败时返回原字符串
pub fn extract_description(raw_response: &str) -> String {
    let trimmed = raw_response.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let value: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return trimmed.to_string(),
    };

    if let Some(desc) = find_description_value(&value) {
        return desc.trim().to_string();
    }

    trimmed.to_string()
}

fn find_description_value(value: &Value) -> Option<String> {
    // 优先顶层常见字段
    for key in &["image_content", "description", "text", "content", "result"] {
        if let Some(v) = value.get(key).and_then(|v| v.as_str()) {
            return Some(v.to_string());
        }
    }

    // 尝试 data / result 嵌套
    for nested_key in &["data", "result"] {
        if let Some(nested) = value.get(nested_key) {
            if let Some(v) = nested.as_str() {
                return Some(v.to_string());
            }
            for key in &["image_content", "description", "text", "content", "result"] {
                if let Some(v) = nested.get(key).and_then(|v| v.as_str()) {
                    return Some(v.to_string());
                }
            }
        }
    }

    // 本身就是字符串
    value.as_str().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_description_variants() {
        assert_eq!(extract_description(r#"{"description": "  a picture  "}"#), "a picture");
        assert_eq!(extract_description(r#"{"data": {"text": "hello"}}"#), "hello");
        assert_eq!(extract_description(r#"{"result": {"description": "world"}}"#), "world");
        assert_eq!(
            extract_description(r#"{"code":200,"message":"ok","data":{"image_content":"返回内容","filename":""}}"#),
            "返回内容"
        );
        assert_eq!(extract_description("plain text"), "plain text");
        assert_eq!(extract_description(""), "");
    }

    #[test]
    fn test_normalize_base64_payload() {
        let raw = "data:image/png;base64, iVBORw0KGgo=\n";
        let normalized: String = raw
            .find("base64,")
            .map(|idx| &raw[idx + "base64,".len()..])
            .unwrap_or(raw)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert_eq!(normalized, "iVBORw0KGgo=");
    }
}
