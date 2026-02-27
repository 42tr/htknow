use std::{
    cmp::Ordering, collections::{HashMap, HashSet}, convert::Infallible
};

use anyhow::anyhow;
use axum::{
    Extension, extract::{Multipart, Query, State}, response::{
        Json, sse::{Event, KeepAlive, KeepAliveStream, Sse}
    }
};
use log::error;
use serde::{Deserialize, Serialize, de};
use serde_json::{Value, json};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use utoipa::{IntoParams, ToSchema};

use super::File;
use crate::{
    AuthUser, api::error::{ApiError, ApiResult}, search::{
        SearchEngine, SearchResultItem as EngineSearchResultItem, advanced::{LlmClient, PlanAction, PlanStep, QueryPlanner, RelevanceJudge, assemble_context_chunk}
    }
};

#[derive(Debug, Deserialize, IntoParams)]
pub struct SearchQuery {
    /// 搜索关键词
    pub query: String,
    /// 文件 ID（可选，逗号分隔多个，如 1,2）
    #[param(value_type = String, example = "1,2")]
    #[serde(default, deserialize_with = "deserialize_id_list")]
    pub file_id: Option<Vec<i64>>,
    /// 知识库 ID（可选，逗号分隔多个，如 1,2）
    #[param(value_type = String, example = "1,2")]
    #[serde(default, deserialize_with = "deserialize_id_list")]
    pub kb_id: Option<Vec<i64>>,
}

fn default_max_sub_queries() -> usize {
    3
}

fn default_per_query_limit() -> usize {
    5
}

fn default_context_chars() -> usize {
    2000
}

/// 高级搜索参数
#[derive(Debug, Deserialize, IntoParams, Clone)]
pub struct AdvancedSearchQuery {
    /// 搜索关键词
    pub query: String,
    /// 文件 ID（可选，逗号分隔多个，如 1,2）
    #[param(value_type = String, example = "1,2")]
    #[serde(default, deserialize_with = "deserialize_id_list")]
    pub file_id: Option<Vec<i64>>,
    /// 知识库 ID（可选，逗号分隔多个，如 1,2）
    #[param(value_type = String, example = "1,2")]
    #[serde(default, deserialize_with = "deserialize_id_list")]
    pub kb_id: Option<Vec<i64>>,
    /// 最大执行步骤数量
    #[serde(default = "default_max_sub_queries")]
    #[param(example = 3)]
    pub max_sub_queries: usize,
    /// 每步处理的候选文档数量
    #[serde(default = "default_per_query_limit")]
    #[param(example = 5)]
    pub per_query_limit: usize,
    /// 组装上下文时，单侧字符数上限
    #[serde(default = "default_context_chars")]
    #[param(example = 2000)]
    pub context_chars: usize,
    /// 是否输出调试事件
    #[serde(default)]
    pub debug: bool,
}

fn deserialize_id_list<'de, D>(deserializer: D) -> Result<Option<Vec<i64>>, D::Error>
where
    D: serde::Deserializer<'de>, {
    let raw = Option::<String>::deserialize(deserializer)?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }

    let mut ids = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let id = part.parse::<i64>().map_err(de::Error::custom)?;
        ids.push(id);
    }

    if ids.is_empty() { Ok(None) } else { Ok(Some(ids)) }
}

/// 文件信息
#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
pub struct FileInfo {
    pub id: i64,
    pub filename: String,
    pub kb_id: Option<i64>,
    pub is_public: bool,
    pub user_id: String,
    pub created_at: i64,
}

/// 知识库信息
#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
pub struct KbInfo {
    pub id: i64,
    pub name: String,
    pub is_public: bool,
    pub user_id: String,
}

/// 单个搜索结果项
#[derive(Debug, Serialize, ToSchema)]
pub struct SearchResultItem {
    /// 切片 ID
    pub id: i64,
    /// 文件 ID
    pub file_id: i64,
    /// 切片内容
    pub content: String,
    /// 搜索得分
    pub score: f32,
    /// 文件信息
    pub file: Option<FileInfo>,
    /// 知识库信息
    pub kb: Option<KbInfo>,
    /// 切片位置
    pub positions: Option<Vec<SlicePosition>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SearchResult {
    pub results: Vec<SearchResultItem>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SlicePosition {
    pub page_idx: i32,
    pub bbox: [i32; 4],
}

#[derive(Debug, sqlx::FromRow)]
struct SlicePositionRow {
    slice_id: i64,
    page_idx: i32,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
}

/// 全文搜索结果项
#[derive(Debug, Serialize, ToSchema)]
pub struct FullSearchResultItem {
    /// 命中片段（HTML，包含<b>高亮）
    pub snippet: String,
    /// 搜索得分
    pub score: f32,
    /// 文件信息
    pub file: Option<File>,
    /// 知识库信息
    pub kb: Option<KbInfo>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FullSearchResult {
    pub results: Vec<FullSearchResultItem>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ImageSearchQuery {
    /// 文件 ID（可选，逗号分隔多个，如 1,2）
    #[param(value_type = String, example = "1,2")]
    #[serde(default, deserialize_with = "deserialize_id_list")]
    pub file_id: Option<Vec<i64>>,
    /// 知识库 ID（可选，逗号分隔多个，如 1,2）
    #[param(value_type = String, example = "1,2")]
    #[serde(default, deserialize_with = "deserialize_id_list")]
    pub kb_id: Option<Vec<i64>>,
}

/// 搜索内容
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/search/",
    operation_id = "search_query",
    tag = "search",
    params(SearchQuery),
    responses(
        (status = 200, description = "搜索成功", body = SearchResult),
        (status = 400, description = "请求参数错误")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn search(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Query(params): Query<SearchQuery>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<SearchResult>> {
    let is_admin = auth_user.is_admin();
    let user_id = auth_user.user_id.clone();
    let kb_ids_to_search = resolve_kb_ids_to_search(&pool, &user_id, is_admin, params.kb_id.as_ref()).await?;
    if matches!(kb_ids_to_search.as_ref(), Some(ids) if ids.is_empty()) {
        return Ok(Json(SearchResult { results: vec![] }));
    }

    let raw_results = search_engine
        .search(&params.query, params.file_id.as_ref(), kb_ids_to_search.as_ref())
        .await
        .map_err(|e| crate::api::error::ApiError::internal(format!("Search failed: {}", e)))?;

    if raw_results.is_empty() {
        return Ok(Json(SearchResult { results: vec![] }));
    }

    // 收集所有 file_id、kb_id 和 slice_id
    let file_ids: Vec<i64> = raw_results.iter().map(|r| r.file_id).collect();
    let kb_ids: Vec<i64> = raw_results.iter().filter_map(|r| r.kb_id).collect();
    let slice_ids: Vec<i64> = raw_results.iter().map(|r| r.id).collect();

    // 批量查询文件信息
    let file_map = get_files_by_ids(&pool, &file_ids).await?;

    // 批量查询知识库信息
    let kb_map = if !kb_ids.is_empty() { get_kbs_by_ids(&pool, &kb_ids).await? } else { HashMap::new() };
    let slice_positions = get_slice_positions(&pool, &slice_ids).await?;

    // 克隆 user_id 用于闭包
    let user_id = auth_user.user_id.clone();

    // 组装结果并过滤权限
    let mut seen_contents: HashSet<String> = HashSet::new();
    let mut results = Vec::new();
    for r in raw_results {
        let id = r.id;
        let file_id = r.file_id;
        let kb_id = r.kb_id;
        let score = r.score;
        let content = r.content;

        let file = file_map.get(&file_id).cloned();
        let kb = kb_id.and_then(|kb_id| kb_map.get(&kb_id).cloned());

        // 权限检查
        let has_permission = if is_admin {
            true
        } else if let Some(ref file_info) = file {
            // 如果文件存在，检查文件权限
            // 规则：私有文件（is_public=0）只有所有者可以查看
            if !file_info.is_public && file_info.user_id != user_id { false } else { true }
        } else if let Some(ref kb_info) = kb {
            // 如果没有文件信息但有知识库信息，检查知识库权限
            // 规则：私有知识库（is_public=0）只有所有者可以查看
            if !kb_info.is_public && kb_info.user_id != user_id { false } else { true }
        } else {
            // 没有文件和知识库信息，默认允许
            true
        };

        if has_permission {
            if seen_contents.insert(content.clone()) {
                results.push(SearchResultItem {
                    id,
                    file_id,
                    content,
                    score,
                    file,
                    kb,
                    positions: slice_positions.get(&id).cloned(),
                });
            }
        }
    }

    Ok(Json(SearchResult { results }))
}

/// 高级搜索（SSE 流式返回）
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/search/advanced/stream",
    operation_id = "advanced_search_stream",
    tag = "search",
    params(AdvancedSearchQuery),
    responses(
        (status = 200, description = "SSE 流式搜索结果")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn advanced_search_stream(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Query(params): Query<AdvancedSearchQuery>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Sse<KeepAliveStream<ReceiverStream<Result<Event, Infallible>>>>> {
    let is_admin = auth_user.is_admin();
    let user_id = auth_user.user_id.clone();
    let kb_ids_to_search = resolve_kb_ids_to_search(&pool, &user_id, is_admin, params.kb_id.as_ref()).await?;

    let (tx, rx) = mpsc::channel(32);
    let pool_clone = pool.clone();
    let search_engine_clone = search_engine.clone();
    let auth_user_clone = auth_user.clone();
    let params_clone = params.clone();
    let kb_ids_clone = kb_ids_to_search.clone();

    tokio::spawn(async move {
        if let Err(err) =
            run_advanced_search_flow(pool_clone, search_engine_clone, auth_user_clone, params_clone, kb_ids_clone, tx)
                .await
        {
            error!("advanced search stream failed: {}", err);
        }
    });

    Ok(Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::new()))
}

async fn run_advanced_search_flow(
    pool: SqlitePool, search_engine: SearchEngine, auth_user: AuthUser, params: AdvancedSearchQuery,
    kb_ids: Option<Vec<i64>>, tx: mpsc::Sender<Result<Event, Infallible>>,
) -> anyhow::Result<()> {
    let llm_client = LlmClient::new();
    let planner = QueryPlanner::new(llm_client.clone());
    let judge = RelevanceJudge::new(llm_client);

    let outcome = run_advanced_search_logic(pool, search_engine, auth_user, params, kb_ids, planner, judge, &tx).await;

    match outcome {
        Ok(_) => {
            let _ = send_status_event(&tx, "完成", "高级搜索已完成").await;
            let _ = send_done_event(&tx).await;
            Ok(())
        }
        Err(err) => {
            let _ = send_error_event(&tx, &err.to_string()).await;
            let _ = send_done_event(&tx).await;
            Err(err)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_advanced_search_logic(
    pool: SqlitePool, search_engine: SearchEngine, auth_user: AuthUser, params: AdvancedSearchQuery,
    kb_ids: Option<Vec<i64>>, planner: QueryPlanner, judge: RelevanceJudge,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
) -> anyhow::Result<()> {
    if matches!(kb_ids.as_ref(), Some(ids) if ids.is_empty()) {
        send_status_event(tx, "权限校验", "无可访问的知识库，直接结束").await?;
        return Ok(());
    }

    send_status_event(tx, "初始化", "生成执行计划").await?;
    let steps = planner.plan(&params.query, params.max_sub_queries.max(1)).await;
    if steps.is_empty() {
        send_status_event(tx, "计划", "未生成有效计划").await?;
        return Ok(());
    }
    send_plan_event(tx, &steps).await?;

    let doc_limit = params.per_query_limit.max(1);
    let context_chars = params.context_chars.max(1);
    let user_id = auth_user.user_id.clone();
    let is_admin = auth_user.is_admin();
    let mut doc_candidates: Vec<DocumentCandidate> = Vec::new();

    for step in steps {
        send_step_event(tx, &step, "started", None).await?;
        match step.action {
            PlanAction::RecentDocuments => {
                doc_candidates = collect_recent_documents(
                    &pool,
                    &search_engine,
                    &params.query,
                    params.file_id.as_ref(),
                    kb_ids.as_ref(),
                    doc_limit,
                    &user_id,
                    is_admin,
                )
                .await?;
                let summary = summarize_documents(&doc_candidates);
                send_step_event(tx, &step, "completed", Some(json!({ "documents": summary }))).await?;
            }
            PlanAction::DocumentStructure => {
                if doc_candidates.is_empty() {
                    doc_candidates = collect_recent_documents(
                        &pool,
                        &search_engine,
                        &params.query,
                        params.file_id.as_ref(),
                        kb_ids.as_ref(),
                        doc_limit,
                        &user_id,
                        is_admin,
                    )
                    .await?;
                }
                if doc_candidates.is_empty() {
                    send_step_event(tx, &step, "skipped", Some(json!({ "reason": "未找到相关文档" }))).await?;
                    continue;
                }
                let structures = describe_document_structure(&pool, &doc_candidates, doc_limit).await?;
                send_step_event(tx, &step, "completed", Some(json!({ "documents": structures }))).await?;
            }
            PlanAction::PageContent => {
                if doc_candidates.is_empty() {
                    doc_candidates = collect_recent_documents(
                        &pool,
                        &search_engine,
                        &params.query,
                        params.file_id.as_ref(),
                        kb_ids.as_ref(),
                        doc_limit,
                        &user_id,
                        is_admin,
                    )
                    .await?;
                }
                if doc_candidates.is_empty() {
                    send_step_event(tx, &step, "skipped", Some(json!({ "reason": "未找到可提取内容" }))).await?;
                    continue;
                }
                let emitted =
                    execute_page_content_step(&pool, &doc_candidates, doc_limit, context_chars, &judge, &params, tx)
                        .await?;
                send_step_event(tx, &step, "completed", Some(json!({ "emitted": emitted }))).await?;
            }
        }
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct AdvancedResultPayload {
    step_action: PlanAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<FileInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kb: Option<KbInfo>,
    slice_ids: Vec<i64>,
    score: f32,
    judge_score: f32,
    judge_reason: String,
    content: String,
}

#[derive(Clone)]
struct DocumentCandidate {
    file: FileInfo,
    kb: Option<KbInfo>,
    slices: Vec<EngineSearchResultItem>,
}

#[derive(Debug, Serialize)]
struct DocumentStructurePayload {
    file: FileInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    kb: Option<KbInfo>,
    sections: Vec<String>,
}

async fn send_event_json<T: Serialize>(
    tx: &mpsc::Sender<Result<Event, Infallible>>, event: &str, payload: &T,
) -> anyhow::Result<()> {
    let evt = Event::default().event(event).json_data(payload).map_err(|e| anyhow!("SSE JSON encode failed: {}", e))?;
    tx.send(Ok(evt)).await.map_err(|_| anyhow!("SSE client disconnected"))
}

async fn send_plan_event(tx: &mpsc::Sender<Result<Event, Infallible>>, steps: &[PlanStep]) -> anyhow::Result<()> {
    send_event_json(tx, "plan", &json!({ "steps": steps })).await
}

async fn send_step_event(
    tx: &mpsc::Sender<Result<Event, Infallible>>, step: &PlanStep, status: &str, details: Option<Value>,
) -> anyhow::Result<()> {
    let mut payload = json!({
        "action": step.action,
        "comment": step.comment,
        "status": status,
    });
    if let Value::Object(ref mut map) = payload {
        if let Some(details) = details {
            map.insert("details".to_string(), details);
        }
    }
    send_event_json(tx, "step", &payload).await
}

async fn send_status_event(
    tx: &mpsc::Sender<Result<Event, Infallible>>, phase: &str, message: &str,
) -> anyhow::Result<()> {
    send_event_json(tx, "status", &json!({ "phase": phase, "message": message })).await
}

async fn send_error_event(tx: &mpsc::Sender<Result<Event, Infallible>>, message: &str) -> anyhow::Result<()> {
    send_event_json(tx, "error", &json!({ "message": message })).await
}

async fn send_done_event(tx: &mpsc::Sender<Result<Event, Infallible>>) -> anyhow::Result<()> {
    send_event_json(tx, "done", &json!({})).await
}

fn has_permission(file: Option<&FileInfo>, kb: Option<&KbInfo>, user_id: &str, is_admin: bool) -> bool {
    if is_admin {
        return true;
    }
    if let Some(file_info) = file {
        if !file_info.is_public && file_info.user_id != user_id {
            return false;
        }
    }
    if let Some(kb_info) = kb {
        if !kb_info.is_public && kb_info.user_id != user_id {
            return false;
        }
    }
    true
}

async fn collect_recent_documents(
    pool: &SqlitePool, search_engine: &SearchEngine, query: &str, file_filter: Option<&Vec<i64>>,
    kb_filter: Option<&Vec<i64>>, doc_limit: usize, user_id: &str, is_admin: bool,
) -> anyhow::Result<Vec<DocumentCandidate>> {
    let mut raw_results =
        search_engine.search(query, file_filter, kb_filter).await.map_err(|e| anyhow!("Search failed: {}", e))?;

    if raw_results.is_empty() {
        return Ok(Vec::new());
    }

    let file_ids: Vec<i64> = raw_results.iter().map(|r| r.file_id).collect();
    let kb_ids: Vec<i64> = raw_results.iter().filter_map(|r| r.kb_id).collect();
    let file_map = get_files_by_ids(pool, &file_ids).await?;
    let kb_map = if !kb_ids.is_empty() { get_kbs_by_ids(pool, &kb_ids).await? } else { HashMap::new() };

    let mut grouped: HashMap<i64, Vec<EngineSearchResultItem>> = HashMap::new();
    for item in raw_results.drain(..) {
        grouped.entry(item.file_id).or_default().push(item);
    }

    let mut grouped_vec: Vec<(i64, Vec<EngineSearchResultItem>)> = grouped.into_iter().collect();
    grouped_vec.sort_by(|a, b| {
        let score_a = a.1.first().map(|s| s.score).unwrap_or(0.0);
        let score_b = b.1.first().map(|s| s.score).unwrap_or(0.0);
        score_b.partial_cmp(&score_a).unwrap_or(Ordering::Equal)
    });

    let mut documents = Vec::new();
    for (file_id, mut slices) in grouped_vec {
        if documents.len() >= doc_limit {
            break;
        }
        let Some(file) = file_map.get(&file_id).cloned() else {
            continue;
        };
        let kb = file.kb_id.and_then(|kid| kb_map.get(&kid).cloned());
        if !has_permission(Some(&file), kb.as_ref(), user_id, is_admin) {
            continue;
        }
        slices.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
        documents.push(DocumentCandidate { file, kb, slices });
    }

    Ok(documents)
}

fn summarize_documents(docs: &[DocumentCandidate]) -> Vec<Value> {
    docs.iter()
        .map(|doc| {
            let preview = doc.slices.first().map(|s| preview_text(&s.content, 160)).unwrap_or_default();
            json!({
                "file": doc.file.clone(),
                "kb": doc.kb.clone(),
                "top_score": doc.slices.first().map(|s| s.score),
                "preview": preview,
            })
        })
        .collect()
}

async fn describe_document_structure(
    pool: &SqlitePool, docs: &[DocumentCandidate], doc_limit: usize,
) -> anyhow::Result<Vec<DocumentStructurePayload>> {
    let mut snapshots = Vec::new();
    let max_docs = doc_limit.max(1);
    for doc in docs.iter().take(max_docs) {
        let sections = fetch_document_sections(pool, doc.file.id, 3).await?;
        snapshots.push(DocumentStructurePayload { file: doc.file.clone(), kb: doc.kb.clone(), sections });
    }
    Ok(snapshots)
}

async fn fetch_document_sections(pool: &SqlitePool, file_id: i64, limit: usize) -> anyhow::Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT content FROM slices WHERE file_id = ? ORDER BY id LIMIT ?")
        .bind(file_id)
        .bind(limit as i64)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|(content,)| preview_text(&content, 160)).collect())
}

async fn execute_page_content_step(
    pool: &SqlitePool, docs: &[DocumentCandidate], doc_limit: usize, context_chars: usize, judge: &RelevanceJudge,
    params: &AdvancedSearchQuery, tx: &mpsc::Sender<Result<Event, Infallible>>,
) -> anyhow::Result<usize> {
    let mut emitted = 0;
    let mut seen_slice_ids: HashSet<i64> = HashSet::new();
    let max_docs = doc_limit.max(1);

    for doc in docs.iter().take(max_docs) {
        for slice in doc.slices.iter().take(2) {
            if !seen_slice_ids.insert(slice.id) {
                continue;
            }
            let context_chunk = match assemble_context_chunk(pool, slice, context_chars).await {
                Ok(chunk) => chunk,
                Err(err) => {
                    if params.debug {
                        let _ = send_event_json(
                            tx,
                            "candidate",
                            &json!({
                                "step_action": PlanAction::PageContent,
                                "file": doc.file.clone(),
                                "kb": doc.kb.clone(),
                                "error": format!("上下文拼接失败: {}", err),
                            }),
                        )
                        .await;
                    }
                    continue;
                }
            };

            if params.debug {
                let preview = preview_text(&context_chunk.content, 160);
                let _ = send_event_json(
                    tx,
                    "candidate",
                    &json!({
                        "step_action": PlanAction::PageContent,
                        "file": doc.file.clone(),
                        "kb": doc.kb.clone(),
                        "score": slice.score,
                        "slice_ids": context_chunk.slice_ids,
                        "preview": preview,
                    }),
                )
                .await;
            }

            let judge_outcome = judge.judge(&params.query, &context_chunk.content).await;
            if judge_outcome.is_relevant {
                let payload = AdvancedResultPayload {
                    step_action: PlanAction::PageContent,
                    file: Some(doc.file.clone()),
                    kb: doc.kb.clone(),
                    slice_ids: context_chunk.slice_ids.clone(),
                    score: slice.score,
                    judge_score: judge_outcome.score,
                    judge_reason: judge_outcome.reason.clone(),
                    content: context_chunk.content.clone(),
                };
                send_event_json(tx, "result", &payload).await?;
                emitted += 1;
            } else if params.debug {
                let _ = send_event_json(
                    tx,
                    "filtered",
                    &json!({
                        "step_action": PlanAction::PageContent,
                        "file": doc.file.clone(),
                        "kb": doc.kb.clone(),
                        "reason": judge_outcome.reason,
                        "score": judge_outcome.score,
                    }),
                )
                .await;
            }
        }
    }

    Ok(emitted)
}

fn preview_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut buf = String::new();
    for (idx, ch) in trimmed.chars().enumerate() {
        if idx >= max_chars {
            buf.push_str("...");
            break;
        }
        buf.push(ch);
    }
    buf
}

/// 全文搜索（仅 Tantivy 全文索引）
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/search/full",
    operation_id = "search_full",
    tag = "search",
    params(SearchQuery),
    responses(
        (status = 200, description = "全文搜索成功", body = FullSearchResult),
        (status = 400, description = "请求参数错误")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn search_full(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Query(params): Query<SearchQuery>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<FullSearchResult>> {
    let is_admin = auth_user.is_admin();
    let user_id = auth_user.user_id.clone();
    let kb_ids_to_search = resolve_kb_ids_to_search(&pool, &user_id, is_admin, params.kb_id.as_ref()).await?;
    if matches!(kb_ids_to_search.as_ref(), Some(ids) if ids.is_empty()) {
        return Ok(Json(FullSearchResult { results: vec![] }));
    }

    let raw_results = search_engine
        .search_full(&params.query, params.file_id.as_ref(), kb_ids_to_search.as_ref())
        .await
        .map_err(|e| crate::api::error::ApiError::internal(format!("Full search failed: {}", e)))?;

    if raw_results.is_empty() {
        return Ok(Json(FullSearchResult { results: vec![] }));
    }

    // 收集所有 file_id 和 kb_id
    let file_ids: Vec<i64> = raw_results.iter().map(|r| r.file_id).collect();
    let kb_ids: Vec<i64> = raw_results.iter().filter_map(|r| r.kb_id).collect();

    // 批量查询文件信息（完整字段）
    let file_map = get_full_files_by_ids(&pool, &file_ids).await?;

    // 批量查询知识库信息
    let kb_map = if !kb_ids.is_empty() { get_kbs_by_ids(&pool, &kb_ids).await? } else { HashMap::new() };

    // 克隆 user_id 用于闭包
    let user_id = auth_user.user_id.clone();

    // 组装结果并过滤权限
    let results = raw_results
        .into_iter()
        .filter_map(|r| {
            let file = file_map.get(&r.file_id).cloned();
            let kb = r.kb_id.and_then(|kb_id| kb_map.get(&kb_id).cloned());

            // 权限检查
            let has_permission = if is_admin {
                true
            } else if let Some(ref file_info) = file {
                // 如果文件存在，检查文件权限
                // 规则：私有文件（is_public=0）只有所有者可以查看
                if !file_info.is_public && file_info.user_id != user_id { false } else { true }
            } else if let Some(ref kb_info) = kb {
                // 如果没有文件信息但有知识库信息，检查知识库权限
                // 规则：私有知识库（is_public=0）只有所有者可以查看
                if !kb_info.is_public && kb_info.user_id != user_id { false } else { true }
            } else {
                // 没有文件和知识库信息，默认允许
                true
            };

            if has_permission {
                Some(FullSearchResultItem { snippet: r.snippet, score: r.score, file, kb })
            } else {
                None
            }
        })
        .collect();

    Ok(Json(FullSearchResult { results }))
}

/// 使用知识图谱增强的搜索
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/search/graph",
    operation_id = "search_with_graph",
    tag = "search",
    params(SearchQuery),
    responses(
        (status = 200, description = "图谱搜索成功", body = SearchResult),
        (status = 400, description = "请求参数错误")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn search_with_graph(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Query(params): Query<SearchQuery>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<SearchResult>> {
    let is_admin = auth_user.is_admin();
    let user_id = auth_user.user_id.clone();
    let kb_ids_to_search = resolve_kb_ids_to_search(&pool, &user_id, is_admin, params.kb_id.as_ref()).await?;
    if matches!(kb_ids_to_search.as_ref(), Some(ids) if ids.is_empty()) {
        return Ok(Json(SearchResult { results: vec![] }));
    }

    let raw_results = search_engine
        .search_with_graph_expansion(&params.query, params.file_id.as_ref(), kb_ids_to_search.as_ref())
        .await
        .map_err(|e| crate::api::error::ApiError::internal(format!("Graph search failed: {}", e)))?;

    if raw_results.is_empty() {
        return Ok(Json(SearchResult { results: vec![] }));
    }

    // 收集所有 file_id、kb_id 和 slice_id
    let file_ids: Vec<i64> = raw_results.iter().map(|r| r.file_id).collect();
    let kb_ids: Vec<i64> = raw_results.iter().filter_map(|r| r.kb_id).collect();
    let slice_ids: Vec<i64> = raw_results.iter().map(|r| r.id).collect();

    // 批量查询文件信息
    let file_map = get_files_by_ids(&pool, &file_ids).await?;

    // 批量查询知识库信息
    let kb_map = if !kb_ids.is_empty() { get_kbs_by_ids(&pool, &kb_ids).await? } else { HashMap::new() };

    let slice_positions = get_slice_positions(&pool, &slice_ids).await?;

    // 克隆 user_id 用于闭包
    let user_id = auth_user.user_id.clone();

    // 组装结果并过滤权限
    let results = raw_results
        .into_iter()
        .filter_map(|r| {
            let file = file_map.get(&r.file_id).cloned();
            let kb = r.kb_id.and_then(|kb_id| kb_map.get(&kb_id).cloned());

            // 权限检查
            let has_permission = if is_admin {
                true
            } else if let Some(ref file_info) = file {
                // 如果文件存在，检查文件权限
                // 规则：私有文件（is_public=0）只有所有者可以查看
                if !file_info.is_public && file_info.user_id != user_id { false } else { true }
            } else if let Some(ref kb_info) = kb {
                // 如果没有文件信息但有知识库信息，检查知识库权限
                // 规则：私有知识库（is_public=0）只有所有者可以查看
                if !kb_info.is_public && kb_info.user_id != user_id { false } else { true }
            } else {
                // 没有文件和知识库信息，默认允许
                true
            };

            if has_permission {
                Some(SearchResultItem {
                    id: r.id,
                    file_id: r.file_id,
                    content: r.content,
                    score: r.score,
                    file,
                    kb,
                    positions: slice_positions.get(&r.id).cloned(),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(Json(SearchResult { results }))
}

/// 以图搜图
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/search/image",
    operation_id = "search_image",
    tag = "search",
    params(ImageSearchQuery),
    request_body(content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "图片搜索成功", body = SearchResult),
        (status = 400, description = "请求参数错误")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn search_image(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Query(params): Query<ImageSearchQuery>, Extension(auth_user): Extension<AuthUser>, mut multipart: Multipart,
) -> ApiResult<Json<SearchResult>> {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut file_name = "image".to_string();
    let mut content_type: Option<String> = None;
    let mut text: Option<String> = None;

    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or_default().to_string();
                match name.as_str() {
                    "file" => {
                        file_name = field.file_name().unwrap_or("image").to_string();
                        content_type = field.content_type().map(|ct| ct.to_string());
                        file_bytes = Some(field.bytes().await?.to_vec());
                    }
                    "text" => {
                        let value = field.text().await?;
                        if !value.trim().is_empty() {
                            text = Some(value);
                        }
                    }
                    _ => {}
                }
            }
            Ok(None) => break,
            Err(e) => {
                return Err(ApiError::Internal(format!("Multipart error: {}", e)));
            }
        }
    }

    let Some(file_bytes) = file_bytes else {
        return Err(ApiError::BadRequest("file is required".to_string()));
    };

    let is_admin = auth_user.is_admin();
    let user_id = auth_user.user_id.clone();
    let kb_ids_to_search = resolve_kb_ids_to_search(&pool, &user_id, is_admin, params.kb_id.as_ref()).await?;
    if matches!(kb_ids_to_search.as_ref(), Some(ids) if ids.is_empty()) {
        return Ok(Json(SearchResult { results: vec![] }));
    }

    let image_embedding = crate::search::embedding::get_image_embedding_from_bytes(
        &file_name,
        content_type.as_deref(),
        file_bytes,
        text.as_deref(),
    )
    .await
    .map_err(|e| ApiError::Internal(format!("Image embedding failed: {}", e)))?;

    let raw_results = search_engine
        .search_image(image_embedding, params.file_id.as_ref(), kb_ids_to_search.as_ref())
        .await
        .map_err(|e| ApiError::internal(format!("Image search failed: {}", e)))?;

    if raw_results.is_empty() {
        return Ok(Json(SearchResult { results: vec![] }));
    }

    let file_ids: Vec<i64> = raw_results.iter().map(|r| r.file_id).collect();
    let kb_ids: Vec<i64> = raw_results.iter().filter_map(|r| r.kb_id).collect();
    let slice_ids: Vec<i64> = raw_results.iter().map(|r| r.id).collect();

    let file_map = get_files_by_ids(&pool, &file_ids).await?;
    let kb_map = if !kb_ids.is_empty() { get_kbs_by_ids(&pool, &kb_ids).await? } else { HashMap::new() };
    let slice_positions = get_slice_positions(&pool, &slice_ids).await?;

    let user_id = auth_user.user_id.clone();
    let results = raw_results
        .into_iter()
        .filter_map(|r| {
            let file = file_map.get(&r.file_id).cloned();
            let kb = r.kb_id.and_then(|kb_id| kb_map.get(&kb_id).cloned());

            let has_permission = if is_admin {
                true
            } else if let Some(ref file_info) = file {
                if !file_info.is_public && file_info.user_id != user_id { false } else { true }
            } else if let Some(ref kb_info) = kb {
                if !kb_info.is_public && kb_info.user_id != user_id { false } else { true }
            } else {
                true
            };

            if has_permission {
                Some(SearchResultItem {
                    id: r.id,
                    file_id: r.file_id,
                    content: r.content,
                    score: r.score,
                    file,
                    kb,
                    positions: slice_positions.get(&r.id).cloned(),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(Json(SearchResult { results }))
}

async fn get_files_by_ids(pool: &SqlitePool, file_ids: &[i64]) -> Result<HashMap<i64, FileInfo>, sqlx::Error> {
    if file_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders: String = file_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let query =
        format!("SELECT id, filename, kb_id, is_public, user_id, created_at FROM files WHERE id IN ({})", placeholders);

    let mut q = sqlx::query_as::<_, FileInfo>(&query);
    for id in file_ids {
        q = q.bind(id);
    }

    let files: Vec<FileInfo> = q.fetch_all(pool).await?;
    Ok(files.into_iter().map(|f| (f.id, f)).collect())
}

async fn resolve_kb_ids_to_search(
    pool: &SqlitePool, user_id: &str, is_admin: bool, root_kb_ids: Option<&Vec<i64>>,
) -> ApiResult<Option<Vec<i64>>> {
    let Some(root_kb_ids) = root_kb_ids else {
        return Ok(None);
    };
    if root_kb_ids.is_empty() {
        return Ok(Some(Vec::new()));
    }

    let mut qb = QueryBuilder::<Sqlite>::new("WITH RECURSIVE kb_hierarchy AS (");
    qb.push("SELECT id FROM knowledge_bases WHERE id IN (");
    let mut separated = qb.separated(", ");
    for kb_id in root_kb_ids {
        separated.push_bind(kb_id);
    }
    qb.push(")");
    if !is_admin {
        qb.push(" AND (user_id = ");
        qb.push_bind(user_id);
        qb.push(" OR is_public = 1)");
    }
    qb.push(" UNION ALL ");
    qb.push("SELECT kb.id FROM knowledge_bases kb ");
    qb.push("INNER JOIN kb_hierarchy kh ON kb.parent_id = kh.id");
    if !is_admin {
        qb.push(" WHERE kb.user_id = ");
        qb.push_bind(user_id);
        qb.push(" OR kb.is_public = 1");
    }
    qb.push(") SELECT DISTINCT id FROM kb_hierarchy");

    let descendant_ids: Vec<i64> = qb.build_query_scalar().fetch_all(pool).await?;
    if descendant_ids.is_empty() {
        return Ok(Some(Vec::new()));
    }

    Ok(Some(descendant_ids))
}

async fn get_full_files_by_ids(pool: &SqlitePool, file_ids: &[i64]) -> Result<HashMap<i64, File>, sqlx::Error> {
    if file_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders: String = file_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let query = format!("SELECT * FROM files WHERE id IN ({})", placeholders);

    let mut q = sqlx::query_as::<_, File>(&query);
    for id in file_ids {
        q = q.bind(id);
    }

    let files: Vec<File> = q.fetch_all(pool).await?;
    Ok(files.into_iter().map(|f| (f.id, f)).collect())
}

async fn get_slice_positions(
    pool: &SqlitePool, slice_ids: &[i64],
) -> Result<HashMap<i64, Vec<SlicePosition>>, sqlx::Error> {
    if slice_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut qb = QueryBuilder::<Sqlite>::new(
        "SELECT slice_id, page_idx, x1, y1, x2, y2 FROM slice_positions WHERE slice_id IN (",
    );
    let mut separated = qb.separated(", ");
    for slice_id in slice_ids {
        separated.push_bind(slice_id);
    }
    qb.push(") ORDER BY slice_id, page_idx, id");
    let rows: Vec<SlicePositionRow> = qb.build_query_as().fetch_all(pool).await?;

    let mut slice_positions: HashMap<i64, Vec<SlicePosition>> = HashMap::new();
    for row in rows {
        slice_positions
            .entry(row.slice_id)
            .or_default()
            .push(SlicePosition { page_idx: row.page_idx, bbox: [row.x1, row.y1, row.x2, row.y2] });
    }

    Ok(slice_positions)
}

async fn get_kbs_by_ids(pool: &SqlitePool, kb_ids: &[i64]) -> Result<HashMap<i64, KbInfo>, sqlx::Error> {
    if kb_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders: String = kb_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let query = format!("SELECT id, name, is_public, user_id FROM knowledge_bases WHERE id IN ({})", placeholders);

    let mut q = sqlx::query_as::<_, KbInfo>(&query);
    for id in kb_ids {
        q = q.bind(id);
    }

    let kbs: Vec<KbInfo> = q.fetch_all(pool).await?;
    Ok(kbs.into_iter().map(|k| (k.id, k)).collect())
}
