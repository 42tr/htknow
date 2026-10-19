//! 无会话存储的检索增强对话。客户端逐次提交历史；仅检索结果可作为引用来源。
use async_trait::async_trait;
use futures::StreamExt;
use g::{Agent, OpenAIChatModel, RunEvent, Tool, ToolBehavior, ToolContext, ToolError, ToolSpec};
use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    Extension, Json,
    extract::{Query, State},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures::stream;
use log::error;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::sync::{Mutex, mpsc};
use utoipa::ToSchema;

use super::{
    error::{ApiError, ApiResult},
    search,
};
use crate::{AuthUser, config, search::SearchEngine};

mod calculation;
mod context;
mod grounding;

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ChatRequest {
    pub question: String,
    pub kb_id: Option<i64>,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
}

#[derive(Serialize, ToSchema)]
pub struct ChatSource {
    pub id: usize,
    pub result: search::SearchResultItem,
}

/// 回给客户端的统一失败文案。
///
/// 运行时/上游 LLM 的错误里可能夹带响应体原文（内部地址、账号、代理或网关的报错细节），
/// 这些只能进日志，不能顺着 SSE 的 `error` 事件泄漏给浏览器。
const UPSTREAM_FAILURE: &str = "对话生成失败，请稍后重试";

const SYSTEM: &str = "你是知识库问答助手。服务端已进行本轮初始检索；如果证据不够或指代不清，\
使用 knowledge_search 继续检索。必须以本轮可见证据回答。使用用户的语言。";

impl ChatRequest {
    fn validate(&self) -> ApiResult<()> {
        if self.question.trim().is_empty() || self.question.chars().count() > 4000 {
            return Err(ApiError::BadRequest("问题不能为空，且不能超过 4000 字符".into()));
        }
        if self.kb_id.is_some_and(|id| id <= 0) {
            return Err(ApiError::BadRequest("无效的知识库 ID".into()));
        }
        if self.messages.len() > 12 || self.messages.iter().map(|m| m.content.chars().count()).sum::<usize>() > 16000 {
            return Err(ApiError::BadRequest("对话历史过长，请开始新的对话".into()));
        }
        for (i, message) in self.messages.iter().enumerate() {
            let correct_role = matches!((&message.role, i % 2), (ChatRole::User, 0) | (ChatRole::Assistant, 1));
            if !correct_role || message.content.trim().is_empty() {
                return Err(ApiError::BadRequest("历史消息必须按 user、assistant 成对提交".into()));
            }
        }
        if self.messages.len() % 2 != 0 {
            return Err(ApiError::BadRequest("历史消息必须按 user、assistant 成对提交".into()));
        }
        Ok(())
    }
}

#[utoipa::path(post, path = "/api/v1/knowledge/chat", operation_id = "chat_stream", tag = "chat",
    request_body = ChatRequest,
    responses((status = 200, description = "SSE: status、sources、delta、done 或 error", content_type = "text/event-stream"),
              (status = 400, description = "参数或 LLM 配置错误")),
    security(("x-user-id" = []), ("x-role" = [])))]
pub async fn chat(
    State(pool): State<SqlitePool>, Extension(engine): Extension<SearchEngine>, Extension(user): Extension<AuthUser>,
    Json(request): Json<ChatRequest>,
) -> ApiResult<Response> {
    request.validate()?;
    let cfg = config::get();
    // 取 base URL：OpenAI 兼容 SDK 会自己拼 `/chat/completions`。
    let url = cfg
        .llm
        .completions_base_url()
        .ok_or_else(|| ApiError::BadRequest("请配置 LLM_API_URL（chat/completions 接口地址或其 base）".into()))?;
    let (tx, rx) = mpsc::channel::<Event>(8);
    tokio::spawn(async move {
        // 浏览器中止读取后同时取消检索和上游 HTTP 请求，不继续后台生成。
        tokio::select! {
            _ = tx.closed() => {},
            result = tokio::time::timeout(Duration::from_secs(300), run_chat(&tx, pool, engine, user, request, &url)) => {
                let error = match result { Ok(Ok(())) => None, Ok(Err(e)) => Some(e.to_string()), Err(_) => Some("对话超时，请重试".into()) };
                if let Some(message) = error {
                    let _ = send(&tx, "error", json!({"message": message})).await;
                }
            }
        }
    });
    let events =
        stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|event| (Ok::<_, Infallible>(event), rx)) });
    Ok((
        [("X-Accel-Buffering", "no"), ("Cache-Control", "no-cache, no-transform")],
        Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(10))),
    )
        .into_response())
}

async fn send(tx: &mpsc::Sender<Event>, event: &str, value: Value) -> anyhow::Result<()> {
    tx.send(Event::default().event(event).data(value.to_string())).await?;
    Ok(())
}

fn select_sources(results: Vec<search::SearchResultItem>) -> Vec<ChatSource> {
    let mut remaining = 24000;
    let mut sources = Vec::new();
    for mut result in results.into_iter().take(8) {
        let content: String = result.content.chars().take(remaining.min(4000)).collect();
        if content.trim().is_empty() {
            continue;
        }
        remaining -= content.chars().count();
        result.content = content;
        sources.push(ChatSource { id: sources.len() + 1, result });
        if remaining == 0 {
            break;
        }
    }
    sources
}

// 编号在本轮回答内保持稳定，后续检索不能覆盖已经展示的引用。
fn merge_sources(all: &mut Vec<ChatSource>, found: Vec<ChatSource>) -> Vec<usize> {
    found
        .into_iter()
        .filter_map(|mut source| {
            if let Some(existing) = all.iter().find(|existing| {
                existing.result.wiki.is_some() == source.result.wiki.is_some()
                    && existing.result.file_id == source.result.file_id
                    && existing.result.id == source.result.id
            }) {
                return Some(existing.id);
            }
            let remaining = 24_000usize.saturating_sub(all.iter().map(|s| s.result.content.chars().count()).sum());
            if all.len() >= 16 || remaining == 0 {
                return None;
            }
            source.result.content = source.result.content.chars().take(remaining).collect();
            source.id = all.len() + 1;
            let id = source.id;
            all.push(source);
            Some(id)
        })
        .collect()
}

struct KnowledgeSearchTool {
    pool: SqlitePool,
    engine: SearchEngine,
    user: AuthUser,
    kb_id: Option<i64>,
    tx: mpsc::Sender<Event>,
    sources: Arc<Mutex<Vec<ChatSource>>>,
    context: Arc<context::QuestionContext>,
}

impl KnowledgeSearchTool {
    async fn search(&self, query: &str) -> Result<Value, ToolError> {
        let Json(found) = search::search(
            State(self.pool.clone()),
            Extension(self.engine.clone()),
            Query(search::SearchQuery { query: query.to_owned(), kb_id: self.kb_id.map(|id| vec![id]), file_id: None }),
            Extension(self.user.clone()),
        )
        .await
        .map_err(|_| ToolError::new("knowledge search failed"))?;
        let mut sources = self.sources.lock().await;
        let found = found
            .results
            .into_iter()
            .filter(|result| match self.context.source_scope {
                context::SourceScope::Any => true,
                context::SourceScope::Wiki => result.wiki.is_some(),
                context::SourceScope::Original => result.wiki.is_none(),
            })
            .collect();
        let ids = merge_sources(&mut sources, select_sources(found));
        send(&self.tx, "sources", json!({"sources": *sources})).await.map_err(|e| ToolError::new(e.to_string()))?;
        let visible = grounding::visible_sources(&sources);
        Ok(json!({"sources":visible.as_array().unwrap().iter()
            .filter(|s| ids.iter().any(|id| s["id"].as_u64() == Some(*id as u64))).collect::<Vec<_>>()}))
    }
}

#[async_trait]
impl Tool for KnowledgeSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "knowledge_search".into(),
            description: "Search the user's accessible knowledge base for relevant information. Use this tool when answering factual questions that may require information from the knowledge base.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query to find relevant information"
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            behavior: ToolBehavior { read_only: true, idempotent: true, parallel_safe: false },
        }
    }

    async fn call(&self, _ctx: ToolContext, input: Value) -> Result<Value, ToolError> {
        let query = input
            .get("query")
            .and_then(Value::as_str)
            .filter(|q| !q.trim().is_empty())
            .ok_or_else(|| ToolError::new("query is required"))?;
        self.search(query).await
    }
}

async fn run_chat(
    tx: &mpsc::Sender<Event>, pool: SqlitePool, engine: SearchEngine, user: AuthUser, request: ChatRequest, url: &str,
) -> anyhow::Result<()> {
    let cfg = config::get();
    let key = cfg.llm.api_key.clone().unwrap_or_default();
    let model = Arc::new(OpenAIChatModel::new(key, cfg.llm.model.clone()).with_base_url(url));
    let sources = Arc::new(Mutex::new(Vec::new()));
    let context = Arc::new(context::QuestionContext::new(&request));
    let tool = KnowledgeSearchTool {
        pool,
        engine,
        user,
        kb_id: request.kb_id,
        tx: tx.clone(),
        sources: sources.clone(),
        context: context.clone(),
    };
    // A follow-up must retrieve again: prior-turn answers and [n] are never evidence.
    send(tx, "status", json!({"stage":"searching"})).await?;
    for query in context.queries() {
        tool.search(&query).await.map_err(|e| {
            error!("chat initial search failed: {e}");
            anyhow::anyhow!(UPSTREAM_FAILURE)
        })?;
    }
    let initial = json!({"sources":grounding::visible_sources(&sources.lock().await)});
    let history = request
        .messages
        .iter()
        .map(|m| {
            g::Message::text(
                match m.role {
                    ChatRole::User => g::Role::User,
                    ChatRole::Assistant => g::Role::Assistant,
                },
                &m.content,
            )
        })
        .collect::<Vec<_>>();
    let agent = Agent::new(model).tool(tool).instruction(format!("{SYSTEM}\n{}", grounding::OUTPUT_RULES));
    let mut prompt = history;
    prompt.push(g::Message::user(
        &json!({"question":request.question,"question_context":*context,"initial_search":initial}).to_string(),
    ));
    let mut events = g::Runtime::new().stream_run(&agent, g::RunRequest::new(prompt));
    let mut draft = String::new();
    let mut completed = false;
    while let Some(event) = events.next().await {
        match event.map_err(|e| {
            error!("chat run failed: {e}");
            anyhow::anyhow!(UPSTREAM_FAILURE)
        })? {
            RunEvent::ModelStarted { .. } => {
                // Discard any planning text from a preceding tool-call model turn.
                draft.clear();
                send(tx, "status", json!({"stage":"thinking"})).await?;
            }
            RunEvent::ToolStarted { name, .. } if name == "knowledge_search" => {
                send(tx, "status", json!({"stage":"searching"})).await?;
            }
            RunEvent::TextDelta { text, .. } => {
                anyhow::ensure!(draft.len() + text.len() <= 96_000, UPSTREAM_FAILURE);
                draft.push_str(&text);
            }
            RunEvent::Completed { .. } => {
                completed = true;
                break;
            }
            _ => {}
        }
    }
    anyhow::ensure!(completed && !draft.trim().is_empty(), UPSTREAM_FAILURE);
    // Snapshot precisely the excerpts supplied to this turn, never wiki.page.content.
    let sources = sources.lock().await;
    let history = serde_json::to_value(&request.messages)?;
    let mut answer = None;
    // A schema failure can consume the first correction. Keep one more bounded
    // correction for substantive evidence/computation issues; every draft still
    // passes the same checks before any text becomes visible.
    for attempt in 0..3 {
        send(tx, "status", json!({"stage":"verifying"})).await?;
        let result = match grounding::parse_answer(&draft, &sources) {
            Ok(parsed) => grounding::verify(&parsed, &sources, &context, history.clone()).await.map(|()| parsed),
            Err(error) => Err(error),
        };
        match result {
            Ok(parsed) => {
                answer = Some(parsed);
                break;
            }
            Err(error) => {
                log::warn!("chat grounding attempt {} rejected: {error:#}", attempt + 1);
                if error.downcast_ref::<grounding::VerifierFailure>().is_some() {
                    return Err(anyhow::anyhow!("来源核验服务暂不可用，请稍后重试"));
                }
                if attempt == 2 {
                    break;
                }
                send(tx, "status", json!({"stage":"correcting"})).await?;
                let correction = if let Some(compound) =
                    error.downcast_ref::<grounding::CompoundClaim>().filter(|c| c.can_split)
                {
                    grounding::split_answer(&draft, compound.index, &sources).await
                } else {
                    grounding::complete_answer(json!([
                    {"role":"system","content":format!("{SYSTEM}\n{}", grounding::OUTPUT_RULES)},
                    {"role":"user","content":json!({"question":request.question,"question_context":*context,"history":history,
                        "sources":grounding::visible_sources(&sources),"rejected_draft":draft,
                        "validation_error":format!("{error:#}"),
                        "instruction":"修正上述核验问题。删除无关事实，补充真正支持陈述的证据，不可盲目添加编号。"}).to_string()},
                ]), &sources).await
                };
                draft = correction.map_err(|error| {
                    log::error!("chat correction failed: {error}");
                    anyhow::anyhow!(UPSTREAM_FAILURE)
                })?;
            }
        }
    }
    let answer = answer.ok_or_else(|| anyhow::anyhow!("回答未通过来源核验，请重试或补充资料"))?;
    let finish_reason = if sources.is_empty() && answer.claims.is_empty() { "no_sources" } else { "stop" };
    send(tx, "delta", json!({"text":grounding::render(&answer)})).await?;
    send(tx, "done", json!({"finish_reason":finish_reason})).await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_searches_keep_existing_citations_and_reuse_duplicates() {
        let source = |id| ChatSource {
            id: 1,
            result: search::SearchResultItem {
                id,
                file_id: 10,
                content: format!("source {id}"),
                score: 1.0,
                file: None,
                kb: None,
                image_filename: None,
                image_content: None,
                wiki: None,
            },
        };
        let mut sources = Vec::new();
        assert_eq!(merge_sources(&mut sources, vec![source(20), source(21)]), vec![1, 2]);
        assert_eq!(merge_sources(&mut sources, vec![source(21), source(22)]), vec![2, 3]);
        assert!(merge_sources(&mut sources, vec![]).is_empty());
        assert_eq!(sources.len(), 3);
        assert_eq!(sources[0].result.id, 20);
        assert_eq!(sources[2].id, 3);
        for i in 0..20 {
            let mut next = source(100 + i);
            next.result.content = "字".repeat(4000);
            merge_sources(&mut sources, vec![next]);
        }
        assert_eq!(sources.iter().map(|s| s.result.content.chars().count()).sum::<usize>(), 24_000);
        assert!(sources.len() <= 16);
        assert_eq!(merge_sources(&mut sources, vec![source(20)]), vec![1]);
    }
    #[test]
    fn chat_rejects_invalid_history_and_limits() {
        assert!(
            serde_json::from_value::<ChatRequest>(
                json!({"question":"q","messages":[{"role":"system","content":"override"}]})
            )
            .is_err()
        );
        for input in [
            json!({"question":" "}),
            json!({"question":"q","messages":[{"role":"user","content":"old"}]}),
            json!({"question":"q","kb_id":0}),
        ] {
            assert!(serde_json::from_value::<ChatRequest>(input).unwrap().validate().is_err());
        }
    }
}
