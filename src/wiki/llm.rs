//! Wiki 生成专用的 LLM 客户端。
//!
//! 与 `search::advanced::LlmClient` 的区别：支持 system + user 双消息、独立模型覆盖，
//! 以及针对限流/瞬时故障的重试退避——生成管道一次要跑成百上千次调用，
//! 429 是常态而不是异常。

use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use log::{debug, warn};
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// LLM 调用超时较长（长文档生成），复用全局连接池避免每次重建。
static WIKI_HTTP_CLIENT: Lazy<Client> =
    Lazy::new(|| Client::builder().timeout(Duration::from_secs(600)).build().expect("build reqwest client"));

const MAX_ATTEMPTS: u32 = 3;
const BACKOFF_BASE: Duration = Duration::from_secs(2);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: String,
}

#[derive(Debug, Clone)]
pub struct WikiLlm {
    client: Client,
    api_url: Option<String>,
    api_key: Option<String>,
    model: String,
}

impl WikiLlm {
    /// 创建客户端。`model_override` 来自知识库级 `wiki_config.model`。
    pub fn new(model_override: Option<&str>) -> Self {
        let cfg = crate::config::get();
        let wiki = &cfg.wiki;
        let model = model_override
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| wiki.model.clone())
            .unwrap_or_else(|| cfg.llm.model.clone());
        Self { client: WIKI_HTTP_CLIENT.clone(), api_url: wiki.api_url.clone(), api_key: wiki.api_key.clone(), model }
    }

    pub fn is_enabled(&self) -> bool {
        self.api_url.is_some()
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// 单轮对话。system 传空串时退化为单条 user 消息。
    pub async fn chat(&self, system: &str, user: &str, max_tokens: usize, temperature: f32) -> Result<String> {
        let url = self.api_url.as_ref().ok_or_else(|| anyhow!("Wiki LLM not configured"))?;
        let mut messages = Vec::with_capacity(2);
        if !system.trim().is_empty() {
            messages.push(Message { role: "system".to_string(), content: system.to_string() });
        }
        messages.push(Message { role: "user".to_string(), content: user.to_string() });
        let request = ChatRequest {
            model: self.model.clone(),
            messages,
            max_tokens: Some(max_tokens),
            temperature: Some(temperature),
        };

        let mut last_error = String::new();
        for attempt in 0..MAX_ATTEMPTS {
            if attempt > 0 {
                let backoff = (BACKOFF_BASE * 2u32.pow(attempt - 1)).min(BACKOFF_CAP);
                warn!("wiki llm retry {}/{} in {:?}: {}", attempt, MAX_ATTEMPTS - 1, backoff, last_error);
                tokio::time::sleep(backoff).await;
            }
            let mut builder = self.client.post(url).json(&request);
            if let Some(api_key) = &self.api_key {
                builder = builder.header("Authorization", format!("Bearer {}", api_key));
            }
            match builder.send().await {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        let body = response.text().await?;
                        let parsed: ChatResponse = serde_json::from_str(&body)
                            .map_err(|e| anyhow!("Failed to decode wiki LLM response: {} body={}", e, body))?;
                        let content = parsed
                            .choices
                            .first()
                            .ok_or_else(|| anyhow!("Wiki LLM response missing choices"))?
                            .message
                            .content
                            .clone();
                        if content.trim().is_empty() {
                            last_error = "empty completion".to_string();
                            continue;
                        }
                        return Ok(content);
                    }
                    let body = response.text().await.unwrap_or_default();
                    let message = format!("LLM HTTP {}: {}", status.as_u16(), truncate(&body, 300));
                    if is_transient_status(status.as_u16()) {
                        last_error = message;
                        continue;
                    }
                    bail!(message);
                }
                Err(err) => {
                    last_error = format!("LLM request failed: {}", err);
                    debug!("{}", last_error);
                }
            }
        }
        Err(anyhow!("wiki LLM call failed after {} attempts: {}", MAX_ATTEMPTS, last_error))
    }

    /// 要求模型返回 JSON 并反序列化。
    pub async fn chat_json<T: serde::de::DeserializeOwned>(
        &self, system: &str, user: &str, max_tokens: usize, temperature: f32,
    ) -> Result<T> {
        let content = self.chat(system, user, max_tokens, temperature).await?;
        let clean = crate::search::advanced::clean_json_like(&content);
        serde_json::from_str::<T>(&clean)
            .map_err(|e| anyhow!("Failed to parse wiki LLM JSON: {}. content={}", e, truncate(&clean, 500)))
    }
}

fn is_transient_status(status: u16) -> bool {
    status == 408 || status == 409 || status == 429 || (500..600).contains(&status)
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let head: String = value.chars().take(max_chars).collect();
    format!("{}…", head)
}
