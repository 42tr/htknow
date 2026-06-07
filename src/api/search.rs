use std::{
    cmp::Ordering, collections::{HashMap, HashSet}, convert::Infallible, time::Instant
};

use anyhow::anyhow;
use axum::{
    Extension, extract::{Multipart, Path, Query, State}, response::{
        Json, sse::{Event, KeepAlive, KeepAliveStream, Sse}
    }
};
use chrono::Utc;
use log::{error, info, warn};
use serde::{Deserialize, Serialize, de};
use serde_json::{Value, json};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use utoipa::{IntoParams, ToSchema};

use super::File;
use crate::{
    AuthUser, api::error::{ApiError, ApiResult}, processor, search::{
        RebuildProgress, SearchEngine, SearchResultItem as EngineSearchResultItem, advanced::{
            ChunkRefiner, ChunkSegment, LlmClient, PlanAction, PlanStep, QueryPlanner, RefineOutcome, RelevanceJudge, assemble_context_chunk
        }
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
    /// 是否启用高级流程（仅切片搜索接口生效）
    #[serde(default)]
    pub advanced: bool,
}

/// 全文搜索查询参数
#[derive(Debug, Deserialize, IntoParams)]
pub struct FullSearchQuery {
    /// 搜索关键词（当 filename 传入时可不填）
    #[serde(default)]
    pub query: String,
    /// 文件 ID（可选，逗号分隔多个，如 1,2）
    #[param(value_type = String, example = "1,2")]
    #[serde(default, deserialize_with = "deserialize_id_list")]
    pub file_id: Option<Vec<i64>>,
    /// 知识库 ID（可选，逗号分隔多个，如 1,2）
    #[param(value_type = String, example = "1,2")]
    #[serde(default, deserialize_with = "deserialize_id_list")]
    pub kb_id: Option<Vec<i64>>,
    /// 按文件名称全匹配过滤
    pub filename: Option<String>,
}

fn default_max_sub_queries() -> usize {
    3
}

fn default_per_query_limit() -> usize {
    10
}

fn default_context_chars() -> usize {
    2000
}

const ADVANCED_JUDGE_EARLY_STOP_SCORE: f32 = 0.8;

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
    #[param(example = 10)]
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
    pub sheet_name: Option<String>,
    pub row_num: Option<i32>,
}

#[derive(Debug, sqlx::FromRow)]
struct SlicePositionRow {
    slice_id: i64,
    page_idx: i32,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    sheet_name: Option<String>,
    row_num: Option<i32>,
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

#[derive(Debug, Deserialize, IntoParams)]
pub struct LexiconListQuery {
    /// 关键字（匹配 term）
    pub q: Option<String>,
    /// 启用状态过滤
    pub enabled: Option<bool>,
    /// 返回数量上限
    #[serde(default = "default_synonym_limit")]
    pub limit: i64,
    /// 偏移量
    #[serde(default)]
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LexiconItem {
    pub id: i64,
    pub term: String,
    pub freq: Option<i64>,
    pub tag: Option<String>,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LexiconListResponse {
    pub total: i64,
    pub items: Vec<LexiconItem>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteLexiconResponse {
    pub id: i64,
    pub deleted: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReloadLexiconResponse {
    pub loaded: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PublishLexiconResponse {
    pub job_id: i64,
    pub status: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateLexiconReq {
    pub term: String,
    pub freq: Option<i64>,
    pub tag: Option<String>,
    #[serde(default = "default_synonym_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateLexiconReq {
    pub term: Option<String>,
    pub freq: Option<i64>,
    pub tag: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ToggleLexiconReq {
    pub enabled: bool,
}

#[derive(Debug, sqlx::FromRow)]
struct LexiconItemRow {
    id: i64,
    term: String,
    freq: Option<i64>,
    tag: Option<String>,
    enabled: i64,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct SynonymListQuery {
    /// 关键字（匹配 term 或 synonym）
    pub q: Option<String>,
    /// 启用状态过滤
    pub enabled: Option<bool>,
    /// 返回数量上限
    #[serde(default = "default_synonym_limit")]
    pub limit: i64,
    /// 偏移量
    #[serde(default)]
    pub offset: i64,
}

fn default_synonym_limit() -> i64 {
    50
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SynonymItem {
    pub id: i64,
    pub term: String,
    pub synonym: String,
    pub weight: f32,
    pub bidirectional: bool,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SynonymListResponse {
    pub total: i64,
    pub items: Vec<SynonymItem>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteSynonymResponse {
    pub id: i64,
    pub deleted: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateSynonymReq {
    pub term: String,
    pub synonym: String,
    #[serde(default = "default_synonym_weight")]
    pub weight: f32,
    #[serde(default = "default_synonym_bidirectional")]
    pub bidirectional: bool,
    #[serde(default = "default_synonym_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateSynonymReq {
    pub term: Option<String>,
    pub synonym: Option<String>,
    pub weight: Option<f32>,
    pub bidirectional: Option<bool>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ToggleSynonymReq {
    pub enabled: bool,
}

fn default_synonym_weight() -> f32 {
    1.0
}

fn default_synonym_bidirectional() -> bool {
    true
}

fn default_synonym_enabled() -> bool {
    true
}

#[derive(Debug, sqlx::FromRow)]
struct SynonymItemRow {
    id: i64,
    term: String,
    synonym: String,
    weight: f32,
    bidirectional: i64,
    enabled: i64,
    created_at: i64,
    updated_at: i64,
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
    let total_started = Instant::now();
    let (_is_admin, user_id, kb_ids_to_search) =
        resolve_scope_for_user(&pool, &auth_user, params.kb_id.as_ref()).await?;
    if no_accessible_kb_scope(kb_ids_to_search.as_deref()) {
        info!(
            "search request completed: user_id={}, query=\"{}\", advanced={}, file_filter={}, kb_scope={}, raw_results=0, final_results=0, elapsed_ms={}",
            user_id,
            preview_for_log(&params.query, 120),
            params.advanced,
            format_id_filter(params.file_id.as_deref()),
            summarize_kb_scope(kb_ids_to_search.as_deref()),
            total_started.elapsed().as_millis()
        );
        return Ok(Json(SearchResult { results: vec![] }));
    }

    if params.advanced {
        let request_id = uuid::Uuid::new_v4().to_string();
        let advanced_params = AdvancedSearchQuery {
            query: params.query.clone(),
            file_id: params.file_id.clone(),
            kb_id: params.kb_id.clone(),
            max_sub_queries: default_max_sub_queries(),
            per_query_limit: default_per_query_limit(),
            context_chars: default_context_chars(),
            debug: false,
        };
        info!(
            "search advanced option enabled: request_id={}, user_id={}, query=\"{}\", file_filter={}, kb_scope={}",
            request_id,
            user_id,
            preview_for_log(&params.query, 120),
            format_id_filter(params.file_id.as_deref()),
            summarize_kb_scope(kb_ids_to_search.as_deref())
        );
        let advanced_results = run_advanced_slice_search_non_stream(
            &pool,
            &search_engine,
            &auth_user,
            &advanced_params,
            kb_ids_to_search.clone(),
            &request_id,
        )
        .await
        .map_err(|e| ApiError::internal(format!("Advanced search failed: {}", e)))?;
        return Ok(Json(SearchResult { results: advanced_results }));
    }

    let raw_results = search_engine
        .search(&params.query, params.file_id.as_ref(), kb_ids_to_search.as_ref())
        .await
        .map_err(|e| crate::api::error::ApiError::internal(format!("Search failed: {}", e)))?;

    let assemble_started = Instant::now();
    let results = build_slice_results_from_raw(&pool, raw_results, &auth_user, true).await?;
    info!(
        "search request completed: user_id={}, query=\"{}\", advanced=false, file_filter={}, kb_scope={}, final_results={}, assemble_elapsed_ms={}, elapsed_ms={}",
        user_id,
        preview_for_log(&params.query, 120),
        format_id_filter(params.file_id.as_deref()),
        summarize_kb_scope(kb_ids_to_search.as_deref()),
        results.len(),
        assemble_started.elapsed().as_millis(),
        total_started.elapsed().as_millis()
    );

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
    let (is_admin, user_id, kb_ids_to_search) =
        resolve_scope_for_user(&pool, &auth_user, params.kb_id.as_ref()).await?;
    let request_id = uuid::Uuid::new_v4().to_string();

    info!(
        "advanced_search_stream request received: request_id={}, user_id={}, is_admin={}, query=\"{}\", file_filter={}, kb_filter={}, max_sub_queries={}, per_query_limit={}, context_chars={}, debug={}",
        request_id,
        user_id,
        is_admin,
        preview_for_log(&params.query, 120),
        format_id_filter(params.file_id.as_deref()),
        format_id_filter(params.kb_id.as_deref()),
        params.max_sub_queries,
        params.per_query_limit,
        params.context_chars,
        params.debug
    );

    let kb_resolve_started = Instant::now();
    info!(
        "advanced_search_stream kb scope resolved: request_id={}, scope={}, elapsed_ms={}",
        request_id,
        summarize_kb_scope(kb_ids_to_search.as_deref()),
        kb_resolve_started.elapsed().as_millis()
    );

    let (tx, rx) = mpsc::channel(32);
    let pool_clone = pool.clone();
    let search_engine_clone = search_engine.clone();
    let auth_user_clone = auth_user.clone();
    let params_clone = params.clone();
    let kb_ids_clone = kb_ids_to_search.clone();
    let request_id_for_task = request_id.clone();

    tokio::spawn(async move {
        if let Err(err) = run_advanced_search_flow(
            pool_clone,
            search_engine_clone,
            auth_user_clone,
            params_clone,
            kb_ids_clone,
            tx,
            request_id_for_task.clone(),
        )
        .await
        {
            error!("advanced search stream failed: request_id={}, error={}", request_id_for_task, err);
        }
    });

    info!("advanced_search_stream task spawned: request_id={}, channel_capacity={}", request_id, 32);

    Ok(Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::new()))
}

async fn run_advanced_search_flow(
    pool: SqlitePool, search_engine: SearchEngine, auth_user: AuthUser, params: AdvancedSearchQuery,
    kb_ids: Option<Vec<i64>>, tx: mpsc::Sender<Result<Event, Infallible>>, request_id: String,
) -> anyhow::Result<()> {
    let flow_started = Instant::now();
    let llm_client = LlmClient::new();
    let llm_enabled = llm_client.is_enabled();

    info!(
        "advanced_search_flow started: request_id={}, user_id={}, kb_scope={}, llm_enabled={}, query=\"{}\"",
        request_id,
        auth_user.user_id,
        summarize_kb_scope(kb_ids.as_deref()),
        llm_enabled,
        preview_for_log(&params.query, 120)
    );

    let planner = QueryPlanner::new(llm_client.clone());
    let judge = RelevanceJudge::new(llm_client.clone());
    let chunk_refiner = ChunkRefiner::new(llm_client);

    let outcome = run_advanced_search_logic(
        pool,
        search_engine,
        auth_user,
        params,
        kb_ids,
        planner,
        judge,
        chunk_refiner,
        &tx,
        &request_id,
    )
    .await;

    match outcome {
        Ok(_) => {
            let _ = send_status_event(&tx, "完成", "高级搜索已完成").await;
            let _ = send_done_event(&tx).await;
            info!(
                "advanced_search_flow completed: request_id={}, elapsed_ms={}",
                request_id,
                flow_started.elapsed().as_millis()
            );
            Ok(())
        }
        Err(err) => {
            let _ = send_error_event(&tx, &err.to_string()).await;
            let _ = send_done_event(&tx).await;
            info!(
                "advanced_search_flow finished with error: request_id={}, elapsed_ms={}",
                request_id,
                flow_started.elapsed().as_millis()
            );
            Err(err)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_advanced_search_logic(
    pool: SqlitePool, search_engine: SearchEngine, auth_user: AuthUser, params: AdvancedSearchQuery,
    kb_ids: Option<Vec<i64>>, planner: QueryPlanner, judge: RelevanceJudge, chunk_refiner: ChunkRefiner,
    tx: &mpsc::Sender<Result<Event, Infallible>>, request_id: &str,
) -> anyhow::Result<()> {
    let logic_started = Instant::now();
    let slice_limit = params.per_query_limit.max(1);
    let context_chars = params.context_chars.max(1);
    let user_id = auth_user.user_id.clone();
    let is_admin = auth_user.is_admin();

    info!(
        "advanced_search_logic started: request_id={}, user_id={}, is_admin={}, file_filter={}, kb_scope={}, slice_limit={}, context_chars={}, debug={}",
        request_id,
        user_id,
        is_admin,
        format_id_filter(params.file_id.as_deref()),
        summarize_kb_scope(kb_ids.as_deref()),
        slice_limit,
        context_chars,
        params.debug
    );

    if no_accessible_kb_scope(kb_ids.as_deref()) {
        info!("advanced_search_logic exits early: request_id={}, reason=no_accessible_kb", request_id);
        send_status_event(tx, "权限校验", "无可访问的知识库，直接结束").await?;
        return Ok(());
    }

    let _selected = run_advanced_plan_steps(
        &pool,
        &search_engine,
        &params,
        kb_ids.as_ref(),
        &planner,
        &judge,
        &chunk_refiner,
        slice_limit,
        context_chars,
        &user_id,
        is_admin,
        Some(tx),
        request_id,
        "advanced_search_logic",
    )
    .await?;

    info!(
        "advanced_search_logic completed: request_id={}, elapsed_ms={}",
        request_id,
        logic_started.elapsed().as_millis()
    );

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
    #[serde(skip_serializing_if = "Option::is_none")]
    refine_reason: Option<String>,
    content: String,
}

#[derive(Clone)]
struct SliceCandidate {
    file: FileInfo,
    kb: Option<KbInfo>,
    slice: EngineSearchResultItem,
}

struct SelectedAnswerCandidate {
    file: FileInfo,
    kb: Option<KbInfo>,
    base_slice_id: i64,
    base_score: f32,
    judge_score: f32,
    judge_reason: String,
    context_segments: Vec<ChunkSegment>,
    context_content: String,
}

struct AdvancedSelectedSliceResult {
    base_slice_id: i64,
    file: FileInfo,
    kb: Option<KbInfo>,
    score: f32,
    content: String,
    slice_ids: Vec<i64>,
}

#[derive(Default)]
struct PageContentStepStats {
    inspected_slices: usize,
    context_error_count: usize,
    judge_rejected_count: usize,
    refine_error_count: usize,
    empty_slice_group_count: usize,
    empty_content_count: usize,
}

struct FinalizedAnswerCandidate {
    file: FileInfo,
    kb: Option<KbInfo>,
    base_slice_id: i64,
    base_score: f32,
    judge_score: f32,
    judge_reason: String,
    final_segments: Vec<ChunkSegment>,
    final_content: String,
    refine_reason: Option<String>,
}

struct PageContentCoreOutcome {
    selected: Option<FinalizedAnswerCandidate>,
    stats: PageContentStepStats,
}

async fn send_event_json<T: Serialize>(
    tx: &mpsc::Sender<Result<Event, Infallible>>, event: &str, payload: &T,
) -> anyhow::Result<()> {
    let evt = Event::default().event(event).json_data(payload).map_err(|e| anyhow!("SSE JSON encode failed: {}", e))?;
    tx.send(Ok(evt)).await.map_err(|_| anyhow!("SSE client disconnected"))
}

async fn maybe_send_event_json<T: Serialize>(
    tx: Option<&mpsc::Sender<Result<Event, Infallible>>>, event: &str, payload: &T,
) -> anyhow::Result<()> {
    if let Some(tx) = tx {
        send_event_json(tx, event, payload).await?;
    }
    Ok(())
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
    if let Value::Object(ref mut map) = payload
        && let Some(details) = details {
            map.insert("details".to_string(), details);
        }
    send_event_json(tx, "step", &payload).await
}

async fn send_status_event(
    tx: &mpsc::Sender<Result<Event, Infallible>>, phase: &str, message: &str,
) -> anyhow::Result<()> {
    send_event_json(tx, "status", &json!({ "phase": phase, "message": message })).await
}

async fn maybe_send_status_event(
    tx: Option<&mpsc::Sender<Result<Event, Infallible>>>, phase: &str, message: &str,
) -> anyhow::Result<()> {
    if let Some(tx) = tx {
        send_status_event(tx, phase, message).await?;
    }
    Ok(())
}

async fn maybe_send_step_event(
    tx: Option<&mpsc::Sender<Result<Event, Infallible>>>, step: &PlanStep, status: &str, details: Option<Value>,
) -> anyhow::Result<()> {
    if let Some(tx) = tx {
        send_step_event(tx, step, status, details).await?;
    }
    Ok(())
}

async fn maybe_send_plan_event(
    tx: Option<&mpsc::Sender<Result<Event, Infallible>>>, steps: &[PlanStep],
) -> anyhow::Result<()> {
    if let Some(tx) = tx {
        send_plan_event(tx, steps).await?;
    }
    Ok(())
}

async fn send_error_event(tx: &mpsc::Sender<Result<Event, Infallible>>, message: &str) -> anyhow::Result<()> {
    send_event_json(tx, "error", &json!({ "message": message })).await
}

async fn send_done_event(tx: &mpsc::Sender<Result<Event, Infallible>>) -> anyhow::Result<()> {
    send_event_json(tx, "done", &json!({})).await
}

fn has_visibility_permission(
    file: Option<(bool, &str)>, kb: Option<(bool, &str)>, user_id: &str, is_admin: bool,
) -> bool {
    if is_admin {
        return true;
    }
    if let Some((is_public, owner_id)) = file
        && !is_public && owner_id != user_id {
            return false;
        }
    if let Some((is_public, owner_id)) = kb
        && !is_public && owner_id != user_id {
            return false;
        }
    true
}

fn has_permission(file: Option<&FileInfo>, kb: Option<&KbInfo>, user_id: &str, is_admin: bool) -> bool {
    has_visibility_permission(
        file.map(|f| (f.is_public, f.user_id.as_str())),
        kb.map(|k| (k.is_public, k.user_id.as_str())),
        user_id,
        is_admin,
    )
}

fn no_accessible_kb_scope(kb_ids: Option<&[i64]>) -> bool {
    matches!(kb_ids, Some(ids) if ids.is_empty())
}

async fn resolve_scope_for_user(
    pool: &SqlitePool, auth_user: &AuthUser, kb_filter: Option<&Vec<i64>>,
) -> ApiResult<(bool, String, Option<Vec<i64>>)> {
    let is_admin = auth_user.is_admin();
    let user_id = auth_user.user_id.clone();
    let kb_ids = resolve_kb_ids_to_search(pool, &user_id, is_admin, kb_filter).await?;
    Ok((is_admin, user_id, kb_ids))
}

async fn build_slice_results_from_raw(
    pool: &SqlitePool, raw_results: Vec<EngineSearchResultItem>, auth_user: &AuthUser, dedupe_by_content: bool,
) -> ApiResult<Vec<SearchResultItem>> {
    if raw_results.is_empty() {
        return Ok(Vec::new());
    }

    let is_admin = auth_user.is_admin();
    let user_id = auth_user.user_id.clone();

    let file_ids: Vec<i64> = raw_results.iter().map(|r| r.file_id).collect();
    let kb_ids: Vec<i64> = raw_results.iter().filter_map(|r| r.kb_id).collect();
    let slice_ids: Vec<i64> = raw_results.iter().map(|r| r.id).collect();

    let file_map = get_files_by_ids(pool, &file_ids).await?;
    let kb_map = if !kb_ids.is_empty() { get_kbs_by_ids(pool, &kb_ids).await? } else { HashMap::new() };
    let slice_positions = get_slice_positions(pool, &slice_ids).await?;

    let mut seen_contents: HashSet<String> = HashSet::new();
    let mut results = Vec::new();
    for r in raw_results {
        let file = file_map.get(&r.file_id).cloned();
        let kb = r.kb_id.and_then(|kb_id| kb_map.get(&kb_id).cloned());
        if !has_permission(file.as_ref(), kb.as_ref(), &user_id, is_admin) {
            continue;
        }
        if dedupe_by_content && !seen_contents.insert(r.content.clone()) {
            continue;
        }
        results.push(SearchResultItem {
            id: r.id,
            file_id: r.file_id,
            content: r.content,
            score: r.score,
            file,
            kb,
            positions: slice_positions.get(&r.id).cloned(),
        });
    }
    Ok(results)
}

#[allow(clippy::too_many_arguments)]
async fn collect_relevant_slices(
    pool: &SqlitePool, search_engine: &SearchEngine, query: &str, file_filter: Option<&Vec<i64>>,
    kb_filter: Option<&Vec<i64>>, slice_limit: usize, user_id: &str, is_admin: bool, request_id: &str,
) -> anyhow::Result<Vec<SliceCandidate>> {
    let collect_started = Instant::now();
    info!(
        "collect_relevant_slices started: request_id={}, query=\"{}\", file_filter={}, kb_filter={}, slice_limit={}, user_id={}, is_admin={}",
        request_id,
        preview_for_log(query, 120),
        format_id_filter(file_filter.map(Vec::as_slice)),
        format_id_filter(kb_filter.map(Vec::as_slice)),
        slice_limit,
        user_id,
        is_admin
    );

    let mut raw_results =
        search_engine.search(query, file_filter, kb_filter).await.map_err(|e| anyhow!("Search failed: {}", e))?;
    let raw_count = raw_results.len();

    if raw_results.is_empty() {
        info!(
            "collect_relevant_slices no results: request_id={}, elapsed_ms={}",
            request_id,
            collect_started.elapsed().as_millis()
        );
        return Ok(Vec::new());
    }

    let file_ids: Vec<i64> = raw_results.iter().map(|r| r.file_id).collect();
    let kb_ids: Vec<i64> = raw_results.iter().filter_map(|r| r.kb_id).collect();
    let file_map = get_files_by_ids(pool, &file_ids).await?;
    let kb_map = if !kb_ids.is_empty() { get_kbs_by_ids(pool, &kb_ids).await? } else { HashMap::new() };

    raw_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));

    let mut selected_slices = Vec::new();
    let mut missing_file_count = 0usize;
    let mut permission_denied_count = 0usize;
    let mut duplicate_slice_count = 0usize;
    let mut seen_slice_ids: HashSet<i64> = HashSet::new();

    for slice in raw_results {
        if selected_slices.len() >= slice_limit {
            break;
        }
        if !seen_slice_ids.insert(slice.id) {
            duplicate_slice_count += 1;
            continue;
        }
        let file_id = slice.file_id;
        let Some(file) = file_map.get(&file_id).cloned() else {
            missing_file_count += 1;
            continue;
        };
        let kb = slice
            .kb_id
            .and_then(|kid| kb_map.get(&kid).cloned())
            .or_else(|| file.kb_id.and_then(|kid| kb_map.get(&kid).cloned()));
        if !has_permission(Some(&file), kb.as_ref(), user_id, is_admin) {
            permission_denied_count += 1;
            continue;
        }
        selected_slices.push(SliceCandidate { file, kb, slice });
    }

    info!(
        "collect_relevant_slices completed: request_id={}, raw_slices={}, selected_slices={}, missing_file={}, permission_denied={}, duplicate_slice={}, elapsed_ms={}",
        request_id,
        raw_count,
        selected_slices.len(),
        missing_file_count,
        permission_denied_count,
        duplicate_slice_count,
        collect_started.elapsed().as_millis()
    );

    Ok(selected_slices)
}

fn summarize_slice_candidates(candidates: &[SliceCandidate]) -> Vec<Value> {
    candidates
        .iter()
        .map(|candidate| {
            let preview = preview_text(&candidate.slice.content, 160);
            json!({
                "slice_id": candidate.slice.id,
                "file_id": candidate.slice.file_id,
                "file": candidate.file.clone(),
                "kb": candidate.kb.clone(),
                "score": candidate.slice.score,
                "preview": preview,
            })
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn run_advanced_plan_steps(
    pool: &SqlitePool, search_engine: &SearchEngine, params: &AdvancedSearchQuery, kb_ids: Option<&Vec<i64>>,
    planner: &QueryPlanner, judge: &RelevanceJudge, chunk_refiner: &ChunkRefiner, slice_limit: usize,
    context_chars: usize, user_id: &str, is_admin: bool, tx: Option<&mpsc::Sender<Result<Event, Infallible>>>,
    request_id: &str, log_prefix: &str,
) -> anyhow::Result<Option<AdvancedSelectedSliceResult>> {
    let planning_started = Instant::now();
    maybe_send_status_event(tx, "初始化", "生成执行计划").await?;
    let steps = planner.plan(&params.query, params.max_sub_queries.max(1)).await;
    let step_actions = steps.iter().map(|step| format!("{:?}", step.action)).collect::<Vec<_>>().join(" -> ");
    info!(
        "{} plan generated: request_id={}, step_count={}, actions=\"{}\", elapsed_ms={}",
        log_prefix,
        request_id,
        steps.len(),
        step_actions,
        planning_started.elapsed().as_millis()
    );

    if steps.is_empty() {
        info!("{} exits early: request_id={}, reason=empty_plan", log_prefix, request_id);
        maybe_send_status_event(tx, "计划", "未生成有效计划").await?;
        return Ok(None);
    }
    maybe_send_plan_event(tx, &steps).await?;

    let mut slice_candidates: Vec<SliceCandidate> = Vec::new();
    let mut selected: Option<AdvancedSelectedSliceResult> = None;

    for (step_idx, step) in steps.into_iter().enumerate() {
        let step_started = Instant::now();
        let step_action = step.action.clone();
        info!(
            "{} step started: request_id={}, step_index={}, action={:?}, comment=\"{}\"",
            log_prefix,
            request_id,
            step_idx + 1,
            step_action,
            preview_for_log(&step.comment, 120)
        );
        maybe_send_step_event(tx, &step, "started", None).await?;

        match step_action {
            PlanAction::RecentDocuments => {
                slice_candidates = collect_relevant_slices(
                    pool,
                    search_engine,
                    &params.query,
                    params.file_id.as_ref(),
                    kb_ids,
                    slice_limit,
                    user_id,
                    is_admin,
                    request_id,
                )
                .await?;

                info!(
                    "{} step completed: request_id={}, step_index={}, action=RecentDocuments, slice_candidates={}, elapsed_ms={}",
                    log_prefix,
                    request_id,
                    step_idx + 1,
                    slice_candidates.len(),
                    step_started.elapsed().as_millis()
                );
                let summary = summarize_slice_candidates(&slice_candidates);
                maybe_send_step_event(
                    tx,
                    &step,
                    "completed",
                    Some(json!({ "count": slice_candidates.len(), "slices": summary })),
                )
                .await?;
            }
            PlanAction::DocumentStructure => {
                info!(
                    "{} step skipped: request_id={}, step_index={}, action=DocumentStructure, reason=flow_simplified",
                    log_prefix,
                    request_id,
                    step_idx + 1
                );
                maybe_send_step_event(tx, &step, "skipped", Some(json!({ "reason": "当前流程不执行文档结构分析" })))
                    .await?;
            }
            PlanAction::PageContent => {
                if slice_candidates.is_empty() {
                    info!(
                        "{} step refilling slices: request_id={}, step_index={}, action=PageContent",
                        log_prefix,
                        request_id,
                        step_idx + 1
                    );
                    slice_candidates = collect_relevant_slices(
                        pool,
                        search_engine,
                        &params.query,
                        params.file_id.as_ref(),
                        kb_ids,
                        slice_limit,
                        user_id,
                        is_admin,
                        request_id,
                    )
                    .await?;
                }

                if slice_candidates.is_empty() {
                    info!(
                        "{} step skipped: request_id={}, step_index={}, action=PageContent, reason=no_slices",
                        log_prefix,
                        request_id,
                        step_idx + 1
                    );
                    maybe_send_step_event(tx, &step, "skipped", Some(json!({ "reason": "未找到相关切片" }))).await?;
                    maybe_send_status_event(tx, "结果", "未找到可用于回答问题的内容").await?;
                    continue;
                }

                let outcome = execute_page_content_step_core(
                    pool,
                    &slice_candidates,
                    slice_limit,
                    context_chars,
                    judge,
                    chunk_refiner,
                    &params.query,
                    request_id,
                    if tx.is_some() { "execute_page_content_step" } else { "execute_page_content_step_non_stream" },
                    tx,
                    tx.is_some() && params.debug,
                    tx.is_some(),
                )
                .await?;

                let mut emitted = 0usize;
                if let Some(finalized) = outcome.selected {
                    let slice_ids = finalized.final_segments.iter().map(|seg| seg.slice_id).collect::<Vec<_>>();
                    let payload = AdvancedResultPayload {
                        step_action: PlanAction::PageContent,
                        file: Some(finalized.file.clone()),
                        kb: finalized.kb.clone(),
                        slice_ids: slice_ids.clone(),
                        score: finalized.base_score,
                        judge_score: finalized.judge_score,
                        judge_reason: finalized.judge_reason.clone(),
                        refine_reason: finalized.refine_reason.clone(),
                        content: finalized.final_content.clone(),
                    };
                    maybe_send_event_json(tx, "result", &payload).await?;
                    if tx.is_some() {
                        info!(
                            "execute_page_content_step emitted result after refine: request_id={}, file_id={}, kb_id={:?}, base_slice_id={}, slice_count={}, score={:.4}, judge_score={:.4}, refine_reason=\"{}\"",
                            request_id,
                            finalized.file.id,
                            finalized.kb.as_ref().map(|kb| kb.id),
                            finalized.base_slice_id,
                            payload.slice_ids.len(),
                            finalized.base_score,
                            finalized.judge_score,
                            preview_for_log(finalized.refine_reason.as_deref().unwrap_or("none"), 120)
                        );
                    }

                    selected = Some(AdvancedSelectedSliceResult {
                        base_slice_id: finalized.base_slice_id,
                        file: finalized.file,
                        kb: finalized.kb,
                        score: finalized.base_score,
                        content: finalized.final_content,
                        slice_ids,
                    });
                    emitted = 1;
                }

                info!(
                    "{} step completed: request_id={}, step_index={}, action=PageContent, emitted={}, inspected_slices={}, judge_rejected={}, context_error={}, refine_error={}, empty_slice_group={}, empty_content={}, elapsed_ms={}",
                    log_prefix,
                    request_id,
                    step_idx + 1,
                    emitted,
                    outcome.stats.inspected_slices,
                    outcome.stats.judge_rejected_count,
                    outcome.stats.context_error_count,
                    outcome.stats.refine_error_count,
                    outcome.stats.empty_slice_group_count,
                    outcome.stats.empty_content_count,
                    step_started.elapsed().as_millis()
                );

                if emitted > 0 {
                    maybe_send_status_event(tx, "结果", "已找到可用于回答问题的内容，停止继续检索").await?;
                } else {
                    maybe_send_status_event(tx, "结果", "未找到可用于回答问题的内容").await?;
                }
                maybe_send_step_event(
                    tx,
                    &step,
                    "completed",
                    Some(json!({ "emitted": emitted, "found": emitted > 0 })),
                )
                .await?;

                if emitted > 0 {
                    break;
                }
            }
        }
    }

    Ok(selected)
}

async fn run_advanced_slice_search_non_stream(
    pool: &SqlitePool, search_engine: &SearchEngine, auth_user: &AuthUser, params: &AdvancedSearchQuery,
    kb_ids: Option<Vec<i64>>, request_id: &str,
) -> anyhow::Result<Vec<SearchResultItem>> {
    if no_accessible_kb_scope(kb_ids.as_deref()) {
        return Ok(Vec::new());
    }

    let llm_client = LlmClient::new();
    let planner = QueryPlanner::new(llm_client.clone());
    let judge = RelevanceJudge::new(llm_client.clone());
    let chunk_refiner = ChunkRefiner::new(llm_client);

    let slice_limit = params.per_query_limit.max(1);
    let context_chars = params.context_chars.max(1);
    let user_id = auth_user.user_id.clone();
    let is_admin = auth_user.is_admin();

    let selected = run_advanced_plan_steps(
        pool,
        search_engine,
        params,
        kb_ids.as_ref(),
        &planner,
        &judge,
        &chunk_refiner,
        slice_limit,
        context_chars,
        &user_id,
        is_admin,
        None,
        request_id,
        "advanced_slice_search_non_stream",
    )
    .await?;

    let Some(selected) = selected else {
        return Ok(Vec::new());
    };

    let slice_positions_map = get_slice_positions(pool, &selected.slice_ids).await?;
    let mut merged_positions = Vec::new();
    for slice_id in &selected.slice_ids {
        if let Some(positions) = slice_positions_map.get(slice_id) {
            merged_positions.extend(positions.iter().cloned());
        }
    }
    let positions = if merged_positions.is_empty() { None } else { Some(merged_positions) };

    Ok(vec![SearchResultItem {
        id: selected.base_slice_id,
        file_id: selected.file.id,
        content: selected.content,
        score: selected.score,
        file: Some(selected.file),
        kb: selected.kb,
        positions,
    }])
}

#[allow(clippy::too_many_arguments)]
async fn execute_page_content_step_core(
    pool: &SqlitePool, candidates: &[SliceCandidate], max_slices: usize, context_chars: usize, judge: &RelevanceJudge,
    chunk_refiner: &ChunkRefiner, query: &str, request_id: &str, log_prefix: &str,
    tx: Option<&mpsc::Sender<Result<Event, Infallible>>>, debug_events: bool, verbose_logs: bool,
) -> anyhow::Result<PageContentCoreOutcome> {
    let mut stats = PageContentStepStats::default();
    let max_slices = max_slices.max(1);
    let mut selected_candidate: Option<SelectedAnswerCandidate> = None;
    let mut best_candidate: Option<SelectedAnswerCandidate> = None;

    for candidate in candidates.iter().take(max_slices) {
        stats.inspected_slices += 1;
        let base_slice = &candidate.slice;
        if verbose_logs {
            info!(
                "{} evaluating candidate before_context: request_id={}, file_id={}, kb_id={:?}, base_slice_id={}, base_score={:.4}, preview=\"{}\"",
                log_prefix,
                request_id,
                candidate.file.id,
                candidate.kb.as_ref().map(|kb| kb.id),
                base_slice.id,
                base_slice.score,
                preview_for_log(&base_slice.content, 120)
            );
        }

        let context_chunk = match assemble_context_chunk(pool, base_slice, context_chars).await {
            Ok(chunk) => chunk,
            Err(err) => {
                stats.context_error_count += 1;
                info!(
                    "{} context assemble failed: request_id={}, file_id={}, base_slice_id={}, error={}",
                    log_prefix, request_id, candidate.file.id, base_slice.id, err
                );
                if debug_events
                    && let Some(tx) = tx {
                        let _ = send_event_json(
                            tx,
                            "candidate",
                            &json!({
                                "step_action": PlanAction::PageContent,
                                "file": candidate.file.clone(),
                                "kb": candidate.kb.clone(),
                                "error": format!("上下文拼接失败: {}", err),
                            }),
                        )
                        .await;
                    }
                continue;
            }
        };

        if debug_events
            && let Some(tx) = tx {
                let preview = preview_text(&context_chunk.content, 160);
                let _ = send_event_json(
                    tx,
                    "candidate",
                    &json!({
                        "step_action": PlanAction::PageContent,
                        "file": candidate.file.clone(),
                        "kb": candidate.kb.clone(),
                        "score": base_slice.score,
                        "slice_ids": context_chunk.slice_ids.clone(),
                        "preview": preview,
                        "stage": "before_refine",
                    }),
                )
                .await;
            }

        let judge_outcome = judge.judge(query, &context_chunk.content).await;
        if !judge_outcome.is_relevant {
            stats.judge_rejected_count += 1;
            if verbose_logs {
                info!(
                    "{} candidate rejected: request_id={}, file_id={}, kb_id={:?}, base_slice_id={}, judge_score={:.4}, reason=\"{}\"",
                    log_prefix,
                    request_id,
                    candidate.file.id,
                    candidate.kb.as_ref().map(|kb| kb.id),
                    base_slice.id,
                    judge_outcome.score,
                    preview_for_log(&judge_outcome.reason, 120)
                );
            }
            if debug_events
                && let Some(tx) = tx {
                    let _ = send_event_json(
                        tx,
                        "filtered",
                        &json!({
                            "step_action": PlanAction::PageContent,
                            "file": candidate.file.clone(),
                            "kb": candidate.kb.clone(),
                            "reason": judge_outcome.reason,
                            "score": judge_outcome.score,
                        }),
                    )
                    .await;
                }
            continue;
        }

        let current_candidate = SelectedAnswerCandidate {
            file: candidate.file.clone(),
            kb: candidate.kb.clone(),
            base_slice_id: base_slice.id,
            base_score: base_slice.score,
            judge_score: judge_outcome.score,
            judge_reason: judge_outcome.reason.clone(),
            context_segments: context_chunk.segments,
            context_content: context_chunk.content,
        };

        if current_candidate.judge_score > ADVANCED_JUDGE_EARLY_STOP_SCORE {
            if verbose_logs {
                info!(
                    "{} selected candidate for early stop: request_id={}, file_id={}, kb_id={:?}, base_slice_id={}, score={:.4}, judge_score={:.4}",
                    log_prefix,
                    request_id,
                    candidate.file.id,
                    candidate.kb.as_ref().map(|kb| kb.id),
                    base_slice.id,
                    base_slice.score,
                    judge_outcome.score
                );
            }
            selected_candidate = Some(current_candidate);
            break;
        }

        let should_replace_best = best_candidate.as_ref().is_none_or(|best| {
            current_candidate.judge_score > best.judge_score
                || ((current_candidate.judge_score - best.judge_score).abs() < f32::EPSILON
                    && current_candidate.base_score > best.base_score)
        });
        if should_replace_best {
            if verbose_logs {
                info!(
                    "{} updated best candidate: request_id={}, file_id={}, kb_id={:?}, base_slice_id={}, score={:.4}, judge_score={:.4}",
                    log_prefix,
                    request_id,
                    candidate.file.id,
                    candidate.kb.as_ref().map(|kb| kb.id),
                    base_slice.id,
                    base_slice.score,
                    judge_outcome.score
                );
            }
            best_candidate = Some(current_candidate);
        }
    }

    if selected_candidate.is_none() {
        selected_candidate = best_candidate;
    }

    let Some(selected) = selected_candidate else {
        return Ok(PageContentCoreOutcome { selected: None, stats });
    };

    if verbose_logs {
        info!(
            "{} refining selected candidate: request_id={}, file_id={}, kb_id={:?}, base_slice_id={}, judge_score={:.4}",
            log_prefix,
            request_id,
            selected.file.id,
            selected.kb.as_ref().map(|kb| kb.id),
            selected.base_slice_id,
            selected.judge_score
        );
    }

    let refine_outcome = match chunk_refiner.refine(query, &selected.context_segments).await {
        Ok(outcome) => outcome,
        Err(err) => {
            stats.refine_error_count += 1;
            info!(
                "{} refine failed: request_id={}, file_id={}, base_slice_id={}, error={}",
                log_prefix, request_id, selected.file.id, selected.base_slice_id, err
            );
            RefineOutcome {
                segments: selected.context_segments.clone(),
                reason: Some("切片筛选失败，保留全部上下文".to_string()),
            }
        }
    };

    let refine_reason = refine_outcome.reason.clone();
    let mut final_segments = refine_outcome.segments;
    if final_segments.is_empty() {
        stats.empty_slice_group_count += 1;
        final_segments = selected.context_segments.clone();
    }
    let mut final_content = final_segments.iter().map(|seg| seg.text.as_str()).collect::<Vec<_>>().join("\n");
    if final_content.trim().is_empty() {
        stats.empty_content_count += 1;
        final_content = selected.context_content.clone();
    }
    if final_content.trim().is_empty() {
        if verbose_logs {
            info!(
                "{} selected candidate has empty content after refine: request_id={}, file_id={}, base_slice_id={}",
                log_prefix, request_id, selected.file.id, selected.base_slice_id
            );
        }
        return Ok(PageContentCoreOutcome { selected: None, stats });
    }

    Ok(PageContentCoreOutcome {
        selected: Some(FinalizedAnswerCandidate {
            base_slice_id: selected.base_slice_id,
            file: selected.file,
            kb: selected.kb,
            base_score: selected.base_score,
            judge_score: selected.judge_score,
            judge_reason: selected.judge_reason,
            final_segments,
            final_content,
            refine_reason,
        }),
        stats,
    })
}

fn format_id_filter(ids: Option<&[i64]>) -> String {
    const MAX_IDS: usize = 12;
    match ids {
        None => "all".to_string(),
        Some([]) => "[]".to_string(),
        Some(list) if list.len() <= MAX_IDS => format!("{:?}", list),
        Some(list) => {
            let head = list.iter().take(MAX_IDS).map(|id| id.to_string()).collect::<Vec<_>>().join(",");
            format!("[{}...](total={})", head, list.len())
        }
    }
}

fn summarize_kb_scope(kb_ids: Option<&[i64]>) -> String {
    match kb_ids {
        None => "all_accessible_kbs".to_string(),
        Some([]) => "no_accessible_kbs".to_string(),
        Some(ids) => format!("{} kbs {}", ids.len(), format_id_filter(Some(ids))),
    }
}

fn preview_for_log(text: &str, max_chars: usize) -> String {
    preview_text(&text.replace('\n', " "), max_chars)
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
    params(FullSearchQuery),
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
    Query(params): Query<FullSearchQuery>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<FullSearchResult>> {
    let (is_admin, user_id, kb_ids_to_search) =
        resolve_scope_for_user(&pool, &auth_user, params.kb_id.as_ref()).await?;
    if no_accessible_kb_scope(kb_ids_to_search.as_deref()) {
        return Ok(Json(FullSearchResult { results: vec![] }));
    }

    let file_ids = if let Some(filename) = params.filename.as_ref().filter(|f| !f.is_empty()) {
        let matched_ids =
            search_file_ids_by_name(&pool, filename, kb_ids_to_search.as_ref(), &user_id, is_admin).await?;
        if matched_ids.is_empty() {
            return Ok(Json(FullSearchResult { results: vec![] }));
        }
        match params.file_id {
            Some(ref explicit_ids) if !explicit_ids.is_empty() => {
                let explicit_set: HashSet<i64> = explicit_ids.iter().copied().collect();
                let intersected: Vec<i64> = matched_ids.into_iter().filter(|id| explicit_set.contains(id)).collect();
                if intersected.is_empty() {
                    return Ok(Json(FullSearchResult { results: vec![] }));
                }
                Some(intersected)
            }
            _ => Some(matched_ids),
        }
    } else {
        params.file_id.clone()
    };

    let results = if params.query.trim().is_empty() {
        if file_ids.is_none() {
            return Ok(Json(FullSearchResult { results: vec![] }));
        }
        let ids = file_ids.unwrap_or_default();
        if ids.is_empty() {
            return Ok(Json(FullSearchResult { results: vec![] }));
        }
        let file_map = get_full_files_by_ids(&pool, &ids).await?;
        let kb_ids_in_files: Vec<i64> = file_map.values().filter_map(|f| f.kb_id).collect();
        let kb_map =
            if !kb_ids_in_files.is_empty() { get_kbs_by_ids(&pool, &kb_ids_in_files).await? } else { HashMap::new() };
        file_map
            .values()
            .filter_map(|f| {
                if !has_visibility_permission(
                    Some((f.is_public, f.user_id.as_str())),
                    f.kb_id.and_then(|kid| kb_map.get(&kid)).map(|k| (k.is_public, k.user_id.as_str())),
                    &user_id,
                    is_admin,
                ) {
                    return None;
                }
                Some(FullSearchResultItem {
                    snippet: String::new(),
                    score: 0.0,
                    file: Some(f.clone()),
                    kb: f.kb_id.and_then(|kid| kb_map.get(&kid).cloned()),
                })
            })
            .collect::<Vec<_>>()
    } else {
        let raw_results = search_engine
            .search_full(&params.query, file_ids.as_ref(), kb_ids_to_search.as_ref())
            .await
            .map_err(|e| crate::api::error::ApiError::internal(format!("Full search failed: {}", e)))?;

        if raw_results.is_empty() {
            return Ok(Json(FullSearchResult { results: vec![] }));
        }

        let file_ids: Vec<i64> = raw_results.iter().map(|r| r.file_id).collect();
        let kb_ids: Vec<i64> = raw_results.iter().filter_map(|r| r.kb_id).collect();
        let file_map = get_full_files_by_ids(&pool, &file_ids).await?;
        let kb_map = if !kb_ids.is_empty() { get_kbs_by_ids(&pool, &kb_ids).await? } else { HashMap::new() };

        raw_results
            .into_iter()
            .filter_map(|r| {
                let file = file_map.get(&r.file_id).cloned();
                let kb = r.kb_id.and_then(|kb_id| kb_map.get(&kb_id).cloned());
                if has_visibility_permission(
                    file.as_ref().map(|f| (f.is_public, f.user_id.as_str())),
                    kb.as_ref().map(|k| (k.is_public, k.user_id.as_str())),
                    &user_id,
                    is_admin,
                ) {
                    Some(FullSearchResultItem { snippet: r.snippet, score: r.score, file, kb })
                } else {
                    None
                }
            })
            .collect()
    };

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
    let (_is_admin, _user_id, kb_ids_to_search) =
        resolve_scope_for_user(&pool, &auth_user, params.kb_id.as_ref()).await?;
    if no_accessible_kb_scope(kb_ids_to_search.as_deref()) {
        return Ok(Json(SearchResult { results: vec![] }));
    }

    let raw_results = search_engine
        .search_with_graph_expansion(&params.query, params.file_id.as_ref(), kb_ids_to_search.as_ref())
        .await
        .map_err(|e| crate::api::error::ApiError::internal(format!("Graph search failed: {}", e)))?;

    let results = build_slice_results_from_raw(&pool, raw_results, &auth_user, false).await?;

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

    let (_is_admin, _user_id, kb_ids_to_search) =
        resolve_scope_for_user(&pool, &auth_user, params.kb_id.as_ref()).await?;
    if no_accessible_kb_scope(kb_ids_to_search.as_deref()) {
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

    let results = build_slice_results_from_raw(&pool, raw_results, &auth_user, false).await?;

    Ok(Json(SearchResult { results }))
}

/// 词表列表（管理员）
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/search/lexicons",
    operation_id = "search_lexicon_list",
    tag = "search",
    params(LexiconListQuery),
    responses(
        (status = 200, description = "词表列表查询成功", body = LexiconListResponse),
        (status = 400, description = "请求参数错误")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn list_lexicons(
    State(pool): State<SqlitePool>, Query(params): Query<LexiconListQuery>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<LexiconListResponse>> {
    ensure_admin(&auth_user)?;

    let q = params.q.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let limit = params.limit.clamp(1, 200);
    let offset = params.offset.max(0);

    let mut count_qb = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM search_lexicon WHERE 1 = 1");
    if let Some(enabled) = params.enabled {
        count_qb.push(" AND enabled = ");
        count_qb.push_bind(if enabled { 1_i64 } else { 0_i64 });
    }
    if let Some(keyword) = &q {
        let pattern = format!("%{}%", keyword);
        count_qb.push(" AND term LIKE ");
        count_qb.push_bind(pattern);
    }
    let total: i64 = count_qb.build_query_scalar().fetch_one(&pool).await?;

    let mut list_qb = QueryBuilder::<Sqlite>::new(
        "SELECT id, term, freq, tag, enabled, created_at, updated_at FROM search_lexicon WHERE 1 = 1",
    );
    if let Some(enabled) = params.enabled {
        list_qb.push(" AND enabled = ");
        list_qb.push_bind(if enabled { 1_i64 } else { 0_i64 });
    }
    if let Some(keyword) = &q {
        let pattern = format!("%{}%", keyword);
        list_qb.push(" AND term LIKE ");
        list_qb.push_bind(pattern);
    }
    list_qb.push(" ORDER BY updated_at DESC, id DESC LIMIT ");
    list_qb.push_bind(limit);
    list_qb.push(" OFFSET ");
    list_qb.push_bind(offset);

    let rows: Vec<LexiconItemRow> = list_qb.build_query_as().fetch_all(&pool).await?;
    let items = rows.into_iter().map(lexicon_row_to_item).collect();
    Ok(Json(LexiconListResponse { total, items }))
}

/// 新增词表词条（管理员）
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/search/lexicons",
    operation_id = "search_lexicon_create",
    tag = "search",
    request_body = CreateLexiconReq,
    responses(
        (status = 200, description = "词条新增成功", body = LexiconItem),
        (status = 400, description = "请求参数错误")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn create_lexicon(
    State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>, Json(req): Json<CreateLexiconReq>,
) -> ApiResult<Json<LexiconItem>> {
    ensure_admin(&auth_user)?;

    let term = normalize_lexicon_term(req.term.as_str())?;
    let freq = normalize_lexicon_freq(req.freq)?;
    let tag = normalize_lexicon_tag(req.tag.as_deref());

    let insert_result = sqlx::query("INSERT INTO search_lexicon (term, freq, tag, enabled) VALUES (?, ?, ?, ?)")
        .bind(term.as_str())
        .bind(freq)
        .bind(tag.as_deref())
        .bind(if req.enabled { 1_i64 } else { 0_i64 })
        .execute(&pool)
        .await;

    let result = match insert_result {
        Ok(result) => result,
        Err(sqlx::Error::Database(db_err)) if db_err.message().contains("UNIQUE") => {
            return Err(ApiError::BadRequest("lexicon term already exists".to_string()));
        }
        Err(e) => return Err(e.into()),
    };

    let id = result.last_insert_rowid();
    let row = fetch_lexicon_row_by_id(&pool, id)
        .await?
        .ok_or_else(|| ApiError::internal("created lexicon term not found"))?;
    Ok(Json(lexicon_row_to_item(row)))
}

/// 更新词表词条（管理员）
#[utoipa::path(
    put,
    path = "/api/v1/knowledge/search/lexicons/{id}",
    operation_id = "search_lexicon_update",
    tag = "search",
    params(
        ("id" = i64, Path, description = "词条 ID")
    ),
    request_body = UpdateLexiconReq,
    responses(
        (status = 200, description = "词条更新成功", body = LexiconItem),
        (status = 400, description = "请求参数错误"),
        (status = 404, description = "词条不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn update_lexicon(
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<UpdateLexiconReq>,
) -> ApiResult<Json<LexiconItem>> {
    ensure_admin(&auth_user)?;

    if req.term.is_none() && req.freq.is_none() && req.tag.is_none() && req.enabled.is_none() {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    let current = fetch_lexicon_row_by_id(&pool, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("lexicon term not found".to_string()))?;
    let term = match req.term.as_deref() {
        Some(value) => normalize_lexicon_term(value)?,
        None => current.term.clone(),
    };
    let freq = match req.freq {
        Some(value) => normalize_lexicon_freq(Some(value))?,
        None => current.freq,
    };
    let tag = match req.tag.as_deref() {
        Some(value) => normalize_lexicon_tag(Some(value)),
        None => current.tag.clone(),
    };
    let enabled = req.enabled.unwrap_or(current.enabled != 0);

    let update_result = sqlx::query(
        "UPDATE search_lexicon SET term = ?, freq = ?, tag = ?, enabled = ?, updated_at = strftime('%s','now') WHERE id = ?",
    )
    .bind(term.as_str())
    .bind(freq)
    .bind(tag.as_deref())
    .bind(if enabled { 1_i64 } else { 0_i64 })
    .bind(id)
    .execute(&pool)
    .await;

    match update_result {
        Ok(result) if result.rows_affected() == 0 => {
            return Err(ApiError::NotFound("lexicon term not found".to_string()));
        }
        Err(sqlx::Error::Database(db_err)) if db_err.message().contains("UNIQUE") => {
            return Err(ApiError::BadRequest("lexicon term already exists".to_string()));
        }
        Err(e) => return Err(e.into()),
        _ => {}
    }

    let row = fetch_lexicon_row_by_id(&pool, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("lexicon term not found".to_string()))?;
    Ok(Json(lexicon_row_to_item(row)))
}

/// 删除词表词条（管理员）
#[utoipa::path(
    delete,
    path = "/api/v1/knowledge/search/lexicons/{id}",
    operation_id = "search_lexicon_delete",
    tag = "search",
    params(
        ("id" = i64, Path, description = "词条 ID")
    ),
    responses(
        (status = 200, description = "词条删除成功", body = DeleteLexiconResponse),
        (status = 404, description = "词条不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn delete_lexicon(
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<DeleteLexiconResponse>> {
    ensure_admin(&auth_user)?;

    let result = sqlx::query("DELETE FROM search_lexicon WHERE id = ?").bind(id).execute(&pool).await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("lexicon term not found".to_string()));
    }

    Ok(Json(DeleteLexiconResponse { id, deleted: true }))
}

/// 启用/停用词表词条（管理员）
#[utoipa::path(
    put,
    path = "/api/v1/knowledge/search/lexicons/{id}/enabled",
    operation_id = "search_lexicon_toggle_enabled",
    tag = "search",
    params(
        ("id" = i64, Path, description = "词条 ID")
    ),
    request_body = ToggleLexiconReq,
    responses(
        (status = 200, description = "词条状态更新成功", body = LexiconItem),
        (status = 404, description = "词条不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn toggle_lexicon_enabled(
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<ToggleLexiconReq>,
) -> ApiResult<Json<LexiconItem>> {
    ensure_admin(&auth_user)?;

    let result = sqlx::query("UPDATE search_lexicon SET enabled = ?, updated_at = strftime('%s','now') WHERE id = ?")
        .bind(if req.enabled { 1_i64 } else { 0_i64 })
        .bind(id)
        .execute(&pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("lexicon term not found".to_string()));
    }

    let row = fetch_lexicon_row_by_id(&pool, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("lexicon term not found".to_string()))?;
    Ok(Json(lexicon_row_to_item(row)))
}

/// 重新加载词表到分词器（管理员）
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/search/lexicons/reload",
    operation_id = "search_lexicon_reload",
    tag = "search",
    responses(
        (status = 200, description = "词表重载成功", body = ReloadLexiconResponse)
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn reload_lexicon(
    Extension(search_engine): Extension<SearchEngine>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<ReloadLexiconResponse>> {
    ensure_admin(&auth_user)?;

    let loaded = search_engine
        .reload_lexicon()
        .await
        .map_err(|e| ApiError::internal(format!("reload lexicon failed: {}", e)))?;
    Ok(Json(ReloadLexiconResponse { loaded }))
}

/// 发布词表并触发索引重建（管理员）
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/search/lexicons/publish",
    operation_id = "search_lexicon_publish",
    tag = "search",
    responses(
        (status = 200, description = "发布成功，返回重建任务信息", body = PublishLexiconResponse),
        (status = 400, description = "已有重建任务在运行或权限不足")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn publish_lexicon(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<PublishLexiconResponse>> {
    ensure_admin(&auth_user)?;

    let running_job_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM index_rebuild_jobs WHERE status = 'running' ORDER BY id DESC LIMIT 1")
            .fetch_optional(&pool)
            .await?;
    if let Some(job_id) = running_job_id {
        return Err(ApiError::BadRequest(format!("index rebuild job {} is already running", job_id)));
    }

    let now = Utc::now().timestamp();
    let result = sqlx::query(
        "INSERT INTO index_rebuild_jobs (status, phase, total_docs, processed_docs, started_at, updated_at, finished_at, error) \
         VALUES ('running', 'queued', 0, 0, ?, ?, NULL, NULL)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await?;
    let job_id = result.last_insert_rowid();

    processor::set_parse_paused(true);

    let pool_clone = pool.clone();
    let search_engine_clone = search_engine.clone();
    tokio::spawn(async move {
        if let Err(err) = run_lexicon_publish_job(pool_clone, search_engine_clone, job_id).await {
            warn!("Lexicon publish job {} failed: {}", job_id, err);
        }
    });

    Ok(Json(PublishLexiconResponse { job_id, status: "running".to_string() }))
}

/// 同义词列表（管理员）
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/search/synonyms",
    operation_id = "search_synonym_list",
    tag = "search",
    params(SynonymListQuery),
    responses(
        (status = 200, description = "同义词列表查询成功", body = SynonymListResponse),
        (status = 400, description = "请求参数错误")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn list_synonyms(
    State(pool): State<SqlitePool>, Query(params): Query<SynonymListQuery>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<SynonymListResponse>> {
    ensure_admin(&auth_user)?;

    let q = params.q.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let limit = params.limit.clamp(1, 200);
    let offset = params.offset.max(0);

    let mut count_qb = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM search_synonyms WHERE 1 = 1");
    if let Some(enabled) = params.enabled {
        count_qb.push(" AND enabled = ");
        count_qb.push_bind(if enabled { 1_i64 } else { 0_i64 });
    }
    if let Some(keyword) = &q {
        let pattern = format!("%{}%", keyword);
        count_qb.push(" AND (term LIKE ");
        count_qb.push_bind(pattern.clone());
        count_qb.push(" OR synonym LIKE ");
        count_qb.push_bind(pattern);
        count_qb.push(")");
    }
    let total: i64 = count_qb.build_query_scalar().fetch_one(&pool).await?;

    let mut list_qb = QueryBuilder::<Sqlite>::new(
        "SELECT id, term, synonym, weight, bidirectional, enabled, created_at, updated_at \
        FROM search_synonyms WHERE 1 = 1",
    );
    if let Some(enabled) = params.enabled {
        list_qb.push(" AND enabled = ");
        list_qb.push_bind(if enabled { 1_i64 } else { 0_i64 });
    }
    if let Some(keyword) = &q {
        let pattern = format!("%{}%", keyword);
        list_qb.push(" AND (term LIKE ");
        list_qb.push_bind(pattern.clone());
        list_qb.push(" OR synonym LIKE ");
        list_qb.push_bind(pattern);
        list_qb.push(")");
    }
    list_qb.push(" ORDER BY updated_at DESC, id DESC LIMIT ");
    list_qb.push_bind(limit);
    list_qb.push(" OFFSET ");
    list_qb.push_bind(offset);

    let rows: Vec<SynonymItemRow> = list_qb.build_query_as().fetch_all(&pool).await?;
    let items = rows.into_iter().map(synonym_row_to_item).collect();
    Ok(Json(SynonymListResponse { total, items }))
}

/// 新增同义词（管理员）
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/search/synonyms",
    operation_id = "search_synonym_create",
    tag = "search",
    request_body = CreateSynonymReq,
    responses(
        (status = 200, description = "同义词新增成功", body = SynonymItem),
        (status = 400, description = "请求参数错误")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn create_synonym(
    State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>, Json(req): Json<CreateSynonymReq>,
) -> ApiResult<Json<SynonymItem>> {
    ensure_admin(&auth_user)?;

    let term = req.term.trim();
    let synonym = req.synonym.trim();
    if term.is_empty() || synonym.is_empty() {
        return Err(ApiError::BadRequest("term and synonym are required".to_string()));
    }
    if term == synonym {
        return Err(ApiError::BadRequest("term and synonym must be different".to_string()));
    }
    if req.weight <= 0.0 {
        return Err(ApiError::BadRequest("weight must be > 0".to_string()));
    }

    let insert_result = sqlx::query(
        "INSERT INTO search_synonyms (term, synonym, weight, bidirectional, enabled) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(term)
    .bind(synonym)
    .bind(req.weight)
    .bind(if req.bidirectional { 1_i64 } else { 0_i64 })
    .bind(if req.enabled { 1_i64 } else { 0_i64 })
    .execute(&pool)
    .await;

    let result = match insert_result {
        Ok(result) => result,
        Err(sqlx::Error::Database(db_err)) if db_err.message().contains("UNIQUE") => {
            return Err(ApiError::BadRequest("synonym pair already exists".to_string()));
        }
        Err(e) => return Err(e.into()),
    };

    let id = result.last_insert_rowid();
    let row =
        fetch_synonym_row_by_id(&pool, id).await?.ok_or_else(|| ApiError::internal("created synonym not found"))?;
    Ok(Json(synonym_row_to_item(row)))
}

/// 更新同义词（管理员）
#[utoipa::path(
    put,
    path = "/api/v1/knowledge/search/synonyms/{id}",
    operation_id = "search_synonym_update",
    tag = "search",
    params(
        ("id" = i64, Path, description = "同义词 ID")
    ),
    request_body = UpdateSynonymReq,
    responses(
        (status = 200, description = "同义词更新成功", body = SynonymItem),
        (status = 400, description = "请求参数错误"),
        (status = 404, description = "同义词不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn update_synonym(
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<UpdateSynonymReq>,
) -> ApiResult<Json<SynonymItem>> {
    ensure_admin(&auth_user)?;

    if req.term.is_none()
        && req.synonym.is_none()
        && req.weight.is_none()
        && req.bidirectional.is_none()
        && req.enabled.is_none()
    {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    let current =
        fetch_synonym_row_by_id(&pool, id).await?.ok_or_else(|| ApiError::NotFound("synonym not found".to_string()))?;
    let term = req.term.as_deref().map(str::trim).unwrap_or(current.term.as_str());
    let synonym = req.synonym.as_deref().map(str::trim).unwrap_or(current.synonym.as_str());
    if term.is_empty() || synonym.is_empty() {
        return Err(ApiError::BadRequest("term and synonym cannot be empty".to_string()));
    }
    if term == synonym {
        return Err(ApiError::BadRequest("term and synonym must be different".to_string()));
    }
    let weight = req.weight.unwrap_or(current.weight);
    if weight <= 0.0 {
        return Err(ApiError::BadRequest("weight must be > 0".to_string()));
    }
    let bidirectional = req.bidirectional.unwrap_or(current.bidirectional != 0);
    let enabled = req.enabled.unwrap_or(current.enabled != 0);

    let update_result = sqlx::query(
        "UPDATE search_synonyms SET term = ?, synonym = ?, weight = ?, bidirectional = ?, enabled = ?, updated_at = strftime('%s','now') WHERE id = ?",
    )
    .bind(term)
    .bind(synonym)
    .bind(weight)
    .bind(if bidirectional { 1_i64 } else { 0_i64 })
    .bind(if enabled { 1_i64 } else { 0_i64 })
    .bind(id)
    .execute(&pool)
    .await;

    match update_result {
        Ok(result) if result.rows_affected() == 0 => {
            return Err(ApiError::NotFound("synonym not found".to_string()));
        }
        Err(sqlx::Error::Database(db_err)) if db_err.message().contains("UNIQUE") => {
            return Err(ApiError::BadRequest("synonym pair already exists".to_string()));
        }
        Err(e) => return Err(e.into()),
        _ => {}
    }

    let row =
        fetch_synonym_row_by_id(&pool, id).await?.ok_or_else(|| ApiError::NotFound("synonym not found".to_string()))?;
    Ok(Json(synonym_row_to_item(row)))
}

/// 删除同义词（管理员）
#[utoipa::path(
    delete,
    path = "/api/v1/knowledge/search/synonyms/{id}",
    operation_id = "search_synonym_delete",
    tag = "search",
    params(
        ("id" = i64, Path, description = "同义词 ID")
    ),
    responses(
        (status = 200, description = "同义词删除成功", body = DeleteSynonymResponse),
        (status = 404, description = "同义词不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn delete_synonym(
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<DeleteSynonymResponse>> {
    ensure_admin(&auth_user)?;

    let result = sqlx::query("DELETE FROM search_synonyms WHERE id = ?").bind(id).execute(&pool).await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("synonym not found".to_string()));
    }

    Ok(Json(DeleteSynonymResponse { id, deleted: true }))
}

/// 启用/停用同义词（管理员）
#[utoipa::path(
    put,
    path = "/api/v1/knowledge/search/synonyms/{id}/enabled",
    operation_id = "search_synonym_toggle_enabled",
    tag = "search",
    params(
        ("id" = i64, Path, description = "同义词 ID")
    ),
    request_body = ToggleSynonymReq,
    responses(
        (status = 200, description = "同义词状态更新成功", body = SynonymItem),
        (status = 404, description = "同义词不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn toggle_synonym_enabled(
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<ToggleSynonymReq>,
) -> ApiResult<Json<SynonymItem>> {
    ensure_admin(&auth_user)?;

    let result = sqlx::query("UPDATE search_synonyms SET enabled = ?, updated_at = strftime('%s','now') WHERE id = ?")
        .bind(if req.enabled { 1_i64 } else { 0_i64 })
        .bind(id)
        .execute(&pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("synonym not found".to_string()));
    }

    let row =
        fetch_synonym_row_by_id(&pool, id).await?.ok_or_else(|| ApiError::NotFound("synonym not found".to_string()))?;
    Ok(Json(synonym_row_to_item(row)))
}

async fn run_lexicon_publish_job(pool: SqlitePool, search_engine: SearchEngine, job_id: i64) -> anyhow::Result<()> {
    let run_result: anyhow::Result<()> = async {
        update_rebuild_job_progress(&pool, job_id, "reload_lexicon", 0, 0).await?;
        let loaded = search_engine.reload_lexicon().await?;
        info!("Search lexicon published for job {}: {} words", job_id, loaded);

        search_engine
            .rebuild_tantivy_indexes(&format!("job-{}", job_id), |progress: RebuildProgress| {
                let pool = pool.clone();
                async move {
                    if let Err(e) = update_rebuild_job_progress(
                        &pool,
                        job_id,
                        progress.phase.as_str(),
                        progress.total_docs,
                        progress.processed_docs,
                    )
                    .await
                    {
                        warn!("Failed to update index rebuild progress for job {}: {}", job_id, e);
                    }
                }
            })
            .await?;

        mark_rebuild_job_completed(&pool, job_id).await?;
        Ok(())
    }
    .await;

    match run_result {
        Ok(()) => {}
        Err(err) => {
            if let Err(update_err) = mark_rebuild_job_failed(&pool, job_id, &err.to_string()).await {
                warn!("Failed to mark rebuild job {} as failed: {}", job_id, update_err);
            }
            processor::set_parse_paused(false);
            return Err(err);
        }
    }

    processor::set_parse_paused(false);
    Ok(())
}

async fn update_rebuild_job_progress(
    pool: &SqlitePool, job_id: i64, phase: &str, total_docs: i64, processed_docs: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE index_rebuild_jobs \
         SET status = 'running', phase = ?, total_docs = ?, processed_docs = ?, updated_at = strftime('%s','now'), finished_at = NULL, error = NULL \
         WHERE id = ?",
    )
    .bind(phase)
    .bind(total_docs.max(0))
    .bind(processed_docs.max(0))
    .bind(job_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_rebuild_job_completed(pool: &SqlitePool, job_id: i64) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE index_rebuild_jobs \
         SET status = 'completed', phase = 'completed', processed_docs = total_docs, updated_at = strftime('%s','now'), finished_at = strftime('%s','now'), error = NULL \
         WHERE id = ?",
    )
    .bind(job_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_rebuild_job_failed(pool: &SqlitePool, job_id: i64, error: &str) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE index_rebuild_jobs \
         SET status = 'failed', phase = 'failed', updated_at = strftime('%s','now'), finished_at = strftime('%s','now'), error = ? \
         WHERE id = ?",
    )
    .bind(error)
    .bind(job_id)
    .execute(pool)
    .await?;
    Ok(())
}

fn normalize_lexicon_term(input: &str) -> ApiResult<String> {
    let term = input.trim();
    if term.is_empty() {
        return Err(ApiError::BadRequest("term cannot be empty".to_string()));
    }
    Ok(term.to_string())
}

fn normalize_lexicon_freq(freq: Option<i64>) -> ApiResult<Option<i64>> {
    match freq {
        Some(value) if value <= 0 => Err(ApiError::BadRequest("freq must be > 0".to_string())),
        Some(value) => Ok(Some(value)),
        None => Ok(None),
    }
}

fn normalize_lexicon_tag(tag: Option<&str>) -> Option<String> {
    tag.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
    })
}

async fn fetch_lexicon_row_by_id(pool: &SqlitePool, id: i64) -> Result<Option<LexiconItemRow>, sqlx::Error> {
    sqlx::query_as::<_, LexiconItemRow>(
        "SELECT id, term, freq, tag, enabled, created_at, updated_at FROM search_lexicon WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

fn lexicon_row_to_item(row: LexiconItemRow) -> LexiconItem {
    LexiconItem {
        id: row.id,
        term: row.term,
        freq: row.freq,
        tag: row.tag,
        enabled: row.enabled != 0,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn ensure_admin(auth_user: &AuthUser) -> ApiResult<()> {
    if auth_user.is_admin() { Ok(()) } else { Err(ApiError::BadRequest("admin role required".to_string())) }
}

async fn fetch_synonym_row_by_id(pool: &SqlitePool, id: i64) -> Result<Option<SynonymItemRow>, sqlx::Error> {
    sqlx::query_as::<_, SynonymItemRow>(
        "SELECT id, term, synonym, weight, bidirectional, enabled, created_at, updated_at FROM search_synonyms WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

fn synonym_row_to_item(row: SynonymItemRow) -> SynonymItem {
    SynonymItem {
        id: row.id,
        term: row.term,
        synonym: row.synonym,
        weight: row.weight,
        bidirectional: row.bidirectional != 0,
        enabled: row.enabled != 0,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

async fn search_file_ids_by_name(
    pool: &SqlitePool, filename: &str, kb_ids: Option<&Vec<i64>>, user_id: &str, is_admin: bool,
) -> Result<Vec<i64>, sqlx::Error> {
    let mut qb = QueryBuilder::<Sqlite>::new("SELECT id FROM files WHERE filename = ");
    qb.push_bind(filename);
    if !is_admin {
        qb.push(" AND (user_id = ");
        qb.push_bind(user_id);
        qb.push(" OR is_public = 1)");
    }
    if let Some(ids) = kb_ids
        && !ids.is_empty() {
            qb.push(" AND kb_id IN (");
            let mut separated = qb.separated(", ");
            for id in ids {
                separated.push_bind(id);
            }
            qb.push(")");
        }
    qb.push(" AND status = 1");
    let ids: Vec<i64> = qb.build_query_scalar().fetch_all(pool).await?;
    Ok(ids)
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
        "SELECT slice_id, page_idx, x1, y1, x2, y2, sheet_name, row_num FROM slice_positions WHERE slice_id IN (",
    );
    let mut separated = qb.separated(", ");
    for slice_id in slice_ids {
        separated.push_bind(slice_id);
    }
    qb.push(") ORDER BY slice_id, page_idx, id");
    let rows: Vec<SlicePositionRow> = qb.build_query_as().fetch_all(pool).await?;

    let mut slice_positions: HashMap<i64, Vec<SlicePosition>> = HashMap::new();
    for row in rows {
        slice_positions.entry(row.slice_id).or_default().push(SlicePosition {
            page_idx: row.page_idx,
            bbox: [row.x1, row.y1, row.x2, row.y2],
            sheet_name: row.sheet_name,
            row_num: row.row_num,
        });
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
