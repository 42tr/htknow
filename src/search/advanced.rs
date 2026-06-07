use std::{collections::HashSet, time::Duration};

use anyhow::{Context, Result, anyhow};
use log::{info, warn};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::SearchResultItem;
use crate::config;

const MAX_NEIGHBOR_SLICES: i64 = 30;

#[derive(Clone)]
pub struct LlmClient {
    client: Client,
    api_url: Option<String>,
    api_key: Option<String>,
    model: String,
}

impl Default for LlmClient {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmClient {
    pub fn new() -> Self {
        let cfg = config::get();
        let llm = cfg.llm.clone();
        let client = Client::builder().timeout(Duration::from_secs(600)).build().expect("build reqwest client");
        Self { client, api_url: llm.api_url, api_key: llm.api_key, model: llm.model }
    }

    pub fn is_enabled(&self) -> bool {
        self.api_url.is_some()
    }

    pub async fn chat_json<T: serde::de::DeserializeOwned>(
        &self, prompt: &str, max_tokens: usize, temperature: f32,
    ) -> Result<T> {
        let content = self.chat(prompt, max_tokens, temperature).await?;
        let clean = clean_json_like(&content);
        let parsed = serde_json::from_str::<T>(&clean)
            .map_err(|e| anyhow!("Failed to parse LLM JSON response: {}. content={}", e, clean))?;
        Ok(parsed)
    }

    pub async fn chat(&self, prompt: &str, max_tokens: usize, temperature: f32) -> Result<String> {
        let url = self.api_url.as_ref().ok_or_else(|| anyhow!("LLM not configured"))?;
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                Message {
                    role: "system".to_string(),
                    content: "Do not output chain-of-thought. Only output final answer.".to_string(),
                },
                Message { role: "user".to_string(), content: prompt.to_string() },
            ],
            max_tokens: Some(max_tokens),
            temperature: Some(temperature),
        };

        let mut builder = self.client.post(url).json(&request);
        if let Some(api_key) = &self.api_key {
            builder = builder.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = builder.send().await.context("LLM HTTP request failed")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("LLM HTTP error {}: {}", status, body));
        }
        let body = response.text().await.context("Failed to read LLM body")?;
        let parsed: ChatResponse =
            serde_json::from_str(&body).map_err(|e| anyhow!("Failed to decode LLM response: {} body={}", e, body))?;
        let choice =
            parsed.choices.first().ok_or_else(|| anyhow!("LLM response missing choices"))?.message.content.clone();
        Ok(choice)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum PlanAction {
    RecentDocuments,
    DocumentStructure,
    PageContent,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlanStep {
    pub action: PlanAction,
    #[serde(default)]
    pub comment: String,
}

pub struct QueryPlanner {
    _llm: LlmClient,
}

impl QueryPlanner {
    pub fn new(llm: LlmClient) -> Self {
        Self { _llm: llm }
    }

    pub async fn plan(&self, _query: &str, _max_steps: usize) -> Vec<PlanStep> {
        default_plan(2)
    }
}

fn default_plan(max_steps: usize) -> Vec<PlanStep> {
    let mut steps = vec![
        PlanStep {
            action: PlanAction::RecentDocuments, comment: "搜索相关文档并选出候选切片".to_string()
        },
        PlanStep {
            action: PlanAction::PageContent,
            comment: "按候选顺序补充上下文并由 LLM 判断可回答性，命中即停止".to_string(),
        },
    ];
    steps.truncate(max_steps.min(steps.len()));
    steps
}

pub struct RelevanceJudge {
    llm: LlmClient,
}

#[derive(Debug, Clone, Serialize)]
pub struct JudgeOutcome {
    pub is_relevant: bool,
    pub score: f32,
    pub reason: String,
}

impl RelevanceJudge {
    pub fn new(llm: LlmClient) -> Self {
        Self { llm }
    }

    pub async fn judge(&self, question: &str, context: &str) -> JudgeOutcome {
        if question.trim().is_empty() || context.trim().is_empty() {
            return JudgeOutcome { is_relevant: false, score: 0.0, reason: "输入为空".to_string() };
        }
        if !self.llm.is_enabled() {
            return JudgeOutcome { is_relevant: true, score: 1.0, reason: "LLM 未启用，默认通过".to_string() };
        }

        let prompt = format!(
            r#"
你是一名检索质量评估助手。判断候选内容是否可以帮助回答问题，只输出 JSON：
{{"relevant":true/false, "score":0-1之间的小数, "reason":"简要说明"}}

问题：
{}

候选内容：
{}
"#,
            question, context
        );

        match self.llm.chat_json::<JudgeResponse>(&prompt, 50000, 0.2).await {
            Ok(resp) => {
                let mut score = resp.score.unwrap_or(0.5);
                if !(0.0..=1.0).contains(&score) {
                    score = score.clamp(0.0, 1.0);
                }
                info!("result: {}\nscore: {}", context, score);
                JudgeOutcome {
                    is_relevant: resp.relevant,
                    score,
                    reason: resp.reason.unwrap_or_else(|| "LLM 无理由".to_string()),
                }
            }
            Err(err) => {
                warn!("LLM judge failed: {}", err);
                JudgeOutcome { is_relevant: true, score: 0.6, reason: "LLM 判定失败，默认通过".to_string() }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChunkSegment {
    pub slice_id: i64,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct ContextChunk {
    pub slice_ids: Vec<i64>,
    pub content: String,
    pub segments: Vec<ChunkSegment>,
}

#[derive(Clone)]
pub struct ChunkRefiner {
    llm: LlmClient,
}

impl ChunkRefiner {
    pub fn new(llm: LlmClient) -> Self {
        Self { llm }
    }

    pub async fn refine(&self, question: &str, segments: &[ChunkSegment]) -> Result<RefineOutcome> {
        if segments.is_empty() {
            return Ok(RefineOutcome { segments: Vec::new(), reason: None });
        }
        if !self.llm.is_enabled() {
            return Ok(RefineOutcome { segments: segments.to_vec(), reason: None });
        }

        let limited_segments: Vec<RefineSegmentInput> = segments
            .iter()
            .map(|seg| RefineSegmentInput { slice_id: seg.slice_id, text: truncate_text(&seg.text, 400) })
            .collect();

        let prompt = format!(
            r#"
你会得到一个问题和若干切片。请保留对问题最有帮助的切片，其余丢弃。
输出 JSON：{{"keep_slice_ids":[1,2], "reason":"简短说明"}}。至少保留一个切片。

问题：
{}

切片：
{}
"#,
            question,
            serde_json::to_string(&limited_segments)?
        );

        match self.llm.chat_json::<RefineResponse>(&prompt, 50000, 0.2).await {
            Ok(resp) => {
                let keep_ids: HashSet<i64> = if resp.keep_slice_ids.is_empty() {
                    segments.iter().map(|seg| seg.slice_id).collect()
                } else {
                    resp.keep_slice_ids.into_iter().collect()
                };
                let filtered: Vec<ChunkSegment> =
                    segments.iter().filter(|seg| keep_ids.contains(&seg.slice_id)).cloned().collect();
                let final_segments = if filtered.is_empty() { segments.to_vec() } else { filtered };
                Ok(RefineOutcome { segments: final_segments, reason: resp.reason })
            }
            Err(err) => {
                warn!("LLM chunk refine failed: {}", err);
                Ok(RefineOutcome { segments: segments.to_vec(), reason: None })
            }
        }
    }
}

pub struct RefineOutcome {
    pub segments: Vec<ChunkSegment>,
    pub reason: Option<String>,
}

pub async fn assemble_context_chunk(
    pool: &SqlitePool, center: &SearchResultItem, per_side_chars: usize,
) -> Result<ContextChunk> {
    let before_rows = fetch_before_slices(pool, center.file_id, center.id).await?;
    let after_rows = fetch_after_slices(pool, center.file_id, center.id).await?;

    let before = take_with_limit(before_rows, per_side_chars, true);
    let after = take_with_limit(after_rows, per_side_chars, false);

    let mut segments: Vec<ChunkSegment> = Vec::new();
    for row in &before {
        segments.push(ChunkSegment { slice_id: row.id, text: row.content.clone() });
    }
    segments.push(ChunkSegment { slice_id: center.id, text: center.content.clone() });
    for row in &after {
        segments.push(ChunkSegment { slice_id: row.id, text: row.content.clone() });
    }

    let slice_ids = segments.iter().map(|seg| seg.slice_id).collect();
    let combined = segments.iter().map(|seg| seg.text.as_str()).collect::<Vec<_>>().join("\n");
    Ok(ContextChunk { slice_ids, content: combined, segments })
}

#[derive(sqlx::FromRow, Clone)]
struct SliceRow {
    id: i64,
    content: String,
}

#[derive(Debug, Serialize)]
struct RefineSegmentInput {
    slice_id: i64,
    text: String,
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut result = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= max_chars {
            result.push_str("...");
            break;
        }
        result.push(ch);
    }
    result
}

fn take_with_limit(items: Vec<SliceRow>, char_limit: usize, reverse_after: bool) -> Vec<SliceRow> {
    if reverse_after {
        // 当前 items 为倒序
        let mut selected = Vec::new();
        let mut total = 0;
        for row in items.into_iter() {
            total += row.content.chars().count();
            selected.push(row);
            if total >= char_limit {
                break;
            }
        }
        selected.reverse();
        selected
    } else {
        let mut selected = Vec::new();
        let mut total = 0;
        for row in items.into_iter() {
            total += row.content.chars().count();
            selected.push(row);
            if total >= char_limit {
                break;
            }
        }
        selected
    }
}

async fn fetch_before_slices(pool: &SqlitePool, file_id: i64, center_id: i64) -> Result<Vec<SliceRow>> {
    let sql = format!(
        "SELECT id, content FROM slices WHERE file_id = ? AND id < ? ORDER BY id DESC LIMIT {}",
        MAX_NEIGHBOR_SLICES
    );
    let rows = sqlx::query_as::<_, SliceRow>(&sql).bind(file_id).bind(center_id).fetch_all(pool).await?;
    Ok(rows)
}

async fn fetch_after_slices(pool: &SqlitePool, file_id: i64, center_id: i64) -> Result<Vec<SliceRow>> {
    let sql = format!(
        "SELECT id, content FROM slices WHERE file_id = ? AND id > ? ORDER BY id ASC LIMIT {}",
        MAX_NEIGHBOR_SLICES
    );
    let rows = sqlx::query_as::<_, SliceRow>(&sql).bind(file_id).bind(center_id).fetch_all(pool).await?;
    Ok(rows)
}

#[derive(Debug, Deserialize)]
struct RefineResponse {
    #[serde(default)]
    keep_slice_ids: Vec<i64>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JudgeResponse {
    relevant: bool,
    #[serde(default)]
    score: Option<f32>,
    #[serde(default)]
    reason: Option<String>,
}

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
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: String,
}

fn clean_json_like(input: &str) -> String {
    let mut text = input.trim();
    if let Some(rest) = text.split_once("</think>") {
        text = rest.1;
    }
    if let Some(stripped) = text.strip_prefix("```json") {
        text = stripped.trim_start();
    } else if let Some(stripped) = text.strip_prefix("```") {
        text = stripped.trim_start();
    }
    if let Some(stripped) = text.strip_suffix("```") {
        text = stripped.trim_end();
    }
    text.trim().to_string()
}
