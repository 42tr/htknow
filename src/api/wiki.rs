//! Wiki 只读浏览接口 + 配置与重建入口。
//!
//! 权限沿用知识库语义：读要求可访问该知识库，写（改配置、触发重建）要求 editor/admin。

use std::collections::{HashMap, HashSet, VecDeque};

use axum::{
    Extension,
    extract::{Query, State},
    response::Json,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::{IntoParams, ToSchema};

use crate::{
    AuthUser,
    api::{
        common,
        error::{ApiError, ApiResult},
    },
    wiki::{self, Granularity, PAGE_TYPE_INDEX, WikiPage, edit, lint, page, revision},
};

/// 列表项：不含正文，避免目录浏览时传输大量 Markdown。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiPageItem {
    pub id: i64,
    pub kb_id: i64,
    pub slug: String,
    pub title: String,
    pub page_type: String,
    pub status: String,
    pub summary: String,
    pub aliases: Vec<String>,
    pub out_links: Vec<String>,
    pub in_links: Vec<String>,
    pub version: i64,
    pub last_edit_source: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<WikiPage> for WikiPageItem {
    fn from(value: WikiPage) -> Self {
        Self {
            id: value.id,
            kb_id: value.kb_id,
            slug: value.slug,
            title: value.title,
            page_type: value.page_type,
            status: value.status,
            summary: value.summary,
            aliases: value.aliases,
            out_links: value.out_links,
            in_links: value.in_links,
            version: value.version,
            last_edit_source: value.last_edit_source,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiPageSource {
    pub file_id: i64,
    pub filename: String,
}

/// 切片证据。带上「用户可见的文件 ID」，前端可直接跳到原文高亮。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiSliceRef {
    pub slice_id: i64,
    pub file_id: i64,
}

/// 页面详情：正文 + 来源文件 + 切片证据（前端可跳转到原文高亮）。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiPageDetail {
    pub page: WikiPageItem,
    pub content: String,
    pub sources: Vec<WikiPageSource>,
    pub slices: Vec<WikiSliceRef>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct WikiPageListParams {
    pub kb_id: i64,
    pub page_type: Option<String>,
    pub status: Option<String>,
    /// 1-200，默认 50
    pub limit: Option<i64>,
    /// 游标：只返回 id 小于该值的页面
    pub before_id: Option<i64>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct WikiPageParams {
    pub kb_id: i64,
    pub slug: String,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct WikiKbParams {
    pub kb_id: i64,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct WikiSearchParams {
    pub kb_id: i64,
    pub q: String,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiPageListResponse {
    pub items: Vec<WikiPageItem>,
    pub next_before_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiIndexGroup {
    pub page_type: String,
    pub title: String,
    pub count: usize,
    pub items: Vec<WikiIndexEntry>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiIndexEntry {
    pub slug: String,
    pub title: String,
    pub summary: String,
}

/// 结构化索引视图。目录由数据库确定性生成，始终是最新状态；
/// Markdown 版索引页仍可通过 `GET /wiki/page?slug=index` 获取。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiIndexResponse {
    pub kb_id: i64,
    pub intro: String,
    pub groups: Vec<WikiIndexGroup>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiGraphNode {
    pub slug: String,
    pub title: String,
    pub page_type: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiGraphEdge {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiGraphResponse {
    pub nodes: Vec<WikiGraphNode>,
    pub edges: Vec<WikiGraphEdge>,
    pub truncated: bool,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct WikiGraphParams {
    pub kb_id: i64,
    /// ego 模式的中心页面 slug；为空则返回全库概览
    pub center: Option<String>,
    /// ego 模式的跳数，1-3，默认 1
    pub depth: Option<i64>,
    /// 概览模式的节点上限，默认 300，最大 2000
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiBuildBreakdown {
    pub completed: i64,
    pub running: i64,
    pub failed: i64,
}

/// 生成进度，供前端展示「索引中」状态。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiStatusResponse {
    pub kb_id: i64,
    pub enabled: bool,
    pub pending_tasks: i64,
    pub page_count: i64,
    pub builds: WikiBuildBreakdown,
    pub last_error: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiConfigResponse {
    pub kb_id: i64,
    /// 生效值：知识库覆盖与全局默认合并后的结果
    pub enabled: bool,
    pub granularity: String,
    pub language: String,
    pub model: Option<String>,
    pub max_pages_per_ingest: usize,
    /// LLM 是否已配置；未配置时即使开关打开也无法生成
    pub llm_available: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct WikiConfigUpdateReq {
    pub kb_id: i64,
    pub enabled: Option<bool>,
    /// focused / standard / exhaustive
    pub granularity: Option<String>,
    pub language: Option<String>,
    pub model: Option<String>,
    pub max_pages_per_ingest: Option<usize>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct WikiRebuildReq {
    pub kb_id: i64,
    /// 为空则重建整个知识库
    pub file_id: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WikiRebuildResponse {
    pub enqueued: i64,
}

fn bounded_limit(value: Option<i64>, default: i64, max: i64) -> ApiResult<i64> {
    match value {
        None => Ok(default),
        Some(limit) if (1..=max).contains(&limit) => Ok(limit),
        Some(_) => Err(ApiError::BadRequest(format!("limit must be between 1 and {}", max))),
    }
}

/// 把切片映射回页面上可见的来源文件。
///
/// `slices.file_id` 指向的是解析产物文件（共享解析时可能与用户上传的文件不同），
/// 直接拿它调用 `/files/{id}/slices/{slice_id}/highlight` 会被判定为「切片不属于该文件」。
async fn slice_refs(pool: &SqlitePool, source_ids: &[i64], slice_ids: &[i64]) -> ApiResult<Vec<WikiSliceRef>> {
    if slice_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut visible_by_owner: HashMap<i64, i64> = HashMap::new();
    for source_id in source_ids {
        let owner = crate::api::effective_parse_file_id(pool, *source_id).await?;
        visible_by_owner.entry(owner).or_insert(*source_id);
    }
    let fallback = source_ids.first().copied();
    let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new("SELECT id, file_id FROM slices WHERE id IN (");
    let mut separated = qb.separated(", ");
    for id in slice_ids {
        separated.push_bind(id);
    }
    separated.push_unseparated(") ORDER BY id");
    let rows: Vec<(i64, i64)> = qb.build_query_as().fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(slice_id, owner)| WikiSliceRef {
            slice_id,
            file_id: visible_by_owner.get(&owner).copied().or(fallback).unwrap_or(owner),
        })
        .collect())
}

pub(super) async fn build_detail(pool: &SqlitePool, page: WikiPage) -> ApiResult<WikiPageDetail> {
    let source_ids = page::source_file_ids(pool, page.id).await?;
    let mut sources = Vec::with_capacity(source_ids.len());
    if !source_ids.is_empty() {
        let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new("SELECT id, filename FROM files WHERE id IN (");
        let mut separated = qb.separated(", ");
        for id in &source_ids {
            separated.push_bind(id);
        }
        separated.push_unseparated(") ORDER BY id");
        let rows: Vec<(i64, String)> = qb.build_query_as().fetch_all(pool).await?;
        sources = rows.into_iter().map(|(file_id, filename)| WikiPageSource { file_id, filename }).collect();
    }
    let slice_ids = page::slice_ref_ids(pool, page.id).await?;
    let slices = slice_refs(pool, &source_ids, &slice_ids).await?;
    let content = page.content.clone();
    Ok(WikiPageDetail { page: page.into(), content, sources, slices })
}

#[utoipa::path(get, path="/api/v1/knowledge/wiki/pages", operation_id="wiki_list_pages", tag="wiki", params(WikiPageListParams), responses((status=200, body=WikiPageListResponse)))]
pub async fn list_pages(
    Query(params): Query<WikiPageListParams>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
) -> ApiResult<Json<WikiPageListResponse>> {
    common::ensure_kb_accessible(&pool, params.kb_id, &user.user_id, user.is_admin()).await?;
    let limit = bounded_limit(params.limit, 50, 200)?;
    let pages = page::list(
        &pool,
        params.kb_id,
        params.page_type.as_deref(),
        params.status.as_deref(),
        limit + 1,
        params.before_id,
    )
    .await?;
    let has_more = pages.len() as i64 > limit;
    let items: Vec<WikiPageItem> = pages.into_iter().take(limit as usize).map(Into::into).collect();
    let next_before_id = if has_more { items.last().map(|item| item.id) } else { None };
    Ok(Json(WikiPageListResponse { items, next_before_id }))
}

#[utoipa::path(get, path="/api/v1/knowledge/wiki/page", operation_id="wiki_get_page", tag="wiki", params(WikiPageParams), responses((status=200, body=WikiPageDetail)))]
pub async fn get_page(
    Query(params): Query<WikiPageParams>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
) -> ApiResult<Json<WikiPageDetail>> {
    common::ensure_kb_accessible(&pool, params.kb_id, &user.user_id, user.is_admin()).await?;
    if params.slug.trim().is_empty() {
        return Err(ApiError::BadRequest("slug is required".to_string()));
    }
    let found = page::get_by_slug(&pool, params.kb_id, params.slug.trim()).await?;
    match found {
        Some(page) => Ok(Json(build_detail(&pool, page).await?)),
        None => Err(ApiError::NotFound("Wiki page not found".to_string())),
    }
}

#[utoipa::path(get, path="/api/v1/knowledge/wiki/index", operation_id="wiki_get_index", tag="wiki", params(WikiKbParams), responses((status=200, body=WikiIndexResponse)))]
pub async fn get_index(
    Query(params): Query<WikiKbParams>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
) -> ApiResult<Json<WikiIndexResponse>> {
    common::ensure_kb_accessible(&pool, params.kb_id, &user.user_id, user.is_admin()).await?;
    let pages = page::list(&pool, params.kb_id, None, None, i64::MAX, None).await?;
    let index_page = pages.iter().find(|p| p.is_index());
    let intro = index_page.map(|p| p.summary.clone()).unwrap_or_default();
    let updated_at = index_page.map(|p| p.updated_at).unwrap_or(0);

    let mut groups = Vec::new();
    for (page_type, title) in
        [(wiki::PAGE_TYPE_ENTITY, "实体"), (wiki::PAGE_TYPE_CONCEPT, "概念"), (wiki::PAGE_TYPE_SUMMARY, "文档摘要")]
    {
        let mut items: Vec<WikiIndexEntry> = pages
            .iter()
            .filter(|p| p.page_type == page_type)
            .map(|p| WikiIndexEntry { slug: p.slug.clone(), title: p.title.clone(), summary: p.summary.clone() })
            .collect();
        if items.is_empty() {
            continue;
        }
        items.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.slug.cmp(&b.slug)));
        groups.push(WikiIndexGroup {
            page_type: page_type.to_string(),
            title: title.to_string(),
            count: items.len(),
            items,
        });
    }
    Ok(Json(WikiIndexResponse { kb_id: params.kb_id, intro, groups, updated_at }))
}

#[utoipa::path(get, path="/api/v1/knowledge/wiki/stats", operation_id="wiki_get_stats", tag="wiki", params(WikiKbParams), responses((status=200, body=page::WikiStats)))]
pub async fn get_stats(
    Query(params): Query<WikiKbParams>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
) -> ApiResult<Json<page::WikiStats>> {
    common::ensure_kb_accessible(&pool, params.kb_id, &user.user_id, user.is_admin()).await?;
    Ok(Json(page::stats(&pool, params.kb_id).await?))
}

#[utoipa::path(get, path="/api/v1/knowledge/wiki/search", operation_id="wiki_search_pages", tag="wiki", params(WikiSearchParams), responses((status=200, body=WikiPageListResponse)))]
pub async fn search_pages(
    Query(params): Query<WikiSearchParams>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
    Extension(engine): Extension<crate::search::SearchEngine>,
) -> ApiResult<Json<WikiPageListResponse>> {
    common::ensure_kb_accessible(&pool, params.kb_id, &user.user_id, user.is_admin()).await?;
    let query = params.q.trim();
    if query.is_empty() {
        return Err(ApiError::BadRequest("q is required".to_string()));
    }
    let limit = bounded_limit(params.limit, 20, 100)?;
    let hits = engine
        .search_wiki_limited(query, Some(&vec![params.kb_id]), limit as usize)
        .await
        .map_err(|e| ApiError::internal(format!("Wiki search failed: {e}")))?;
    let mut pages = Vec::new();
    for (candidate, _) in hits.into_iter().take(limit as usize) {
        if let Some(page) = page::get_by_id(&pool, candidate.id).await? {
            if page.status == wiki::STATUS_PUBLISHED && page.version == candidate.version && page.kb_id == params.kb_id
            {
                pages.push(page);
            }
        }
    }
    Ok(Json(WikiPageListResponse { items: pages.into_iter().map(Into::into).collect(), next_before_id: None }))
}

#[utoipa::path(get, path="/api/v1/knowledge/wiki/status", operation_id="wiki_get_status", tag="wiki", params(WikiKbParams), responses((status=200, body=WikiStatusResponse)))]
pub async fn get_status(
    Query(params): Query<WikiKbParams>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
) -> ApiResult<Json<WikiStatusResponse>> {
    common::ensure_kb_accessible(&pool, params.kb_id, &user.user_id, user.is_admin()).await?;
    let resolved = wiki::resolve_config(&pool, params.kb_id).await?;
    let enabled = resolved.is_some_and(|config| config.enabled);
    let pending_tasks = wiki::queue::pending_count(&pool, params.kb_id).await?;
    let page_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wiki_pages WHERE status != 'withdrawn' AND kb_id = ? AND page_type != ? AND status != ?",
    )
    .bind(params.kb_id)
    .bind(PAGE_TYPE_INDEX)
    .bind(wiki::STATUS_ARCHIVED)
    .fetch_one(&pool)
    .await?;
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT b.status, COUNT(*) FROM wiki_builds b JOIN files f ON f.id = b.file_id
          WHERE f.kb_id = ? GROUP BY b.status",
    )
    .bind(params.kb_id)
    .fetch_all(&pool)
    .await?;
    let mut builds = WikiBuildBreakdown { completed: 0, running: 0, failed: 0 };
    for (status, count) in rows {
        match status.as_str() {
            "completed" => builds.completed = count,
            "running" => builds.running = count,
            "failed" => builds.failed = count,
            _ => {}
        }
    }
    let last_error: String = sqlx::query_scalar(
        "SELECT COALESCE((SELECT last_error FROM wiki_tasks WHERE kb_id = ? AND last_error != '' ORDER BY updated_at DESC LIMIT 1),
                         (SELECT error FROM wiki_builds b JOIN files f ON f.id = b.file_id WHERE f.kb_id = ? AND b.error != '' ORDER BY b.updated_at DESC LIMIT 1), '')",
    )
    .bind(params.kb_id)
    .bind(params.kb_id)
    .fetch_one(&pool)
    .await?;
    Ok(Json(WikiStatusResponse { kb_id: params.kb_id, enabled, pending_tasks, page_count, builds, last_error }))
}

#[utoipa::path(get, path="/api/v1/knowledge/wiki/graph", operation_id="wiki_get_graph", tag="wiki", params(WikiGraphParams), responses((status=200, body=WikiGraphResponse)))]
pub async fn get_graph(
    Query(params): Query<WikiGraphParams>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
) -> ApiResult<Json<WikiGraphResponse>> {
    common::ensure_kb_accessible(&pool, params.kb_id, &user.user_id, user.is_admin()).await?;
    let pages = page::list(&pool, params.kb_id, None, Some(wiki::STATUS_PUBLISHED), i64::MAX, None).await?;
    let by_slug: HashMap<String, &WikiPage> = pages.iter().map(|p| (p.slug.clone(), p)).collect();

    let selected: HashSet<String> = match params.center.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        Some(center) => {
            if !by_slug.contains_key(center) {
                return Err(ApiError::NotFound("center page not found".to_string()));
            }
            let depth = params.depth.unwrap_or(1).clamp(1, 3) as usize;
            ego_slugs(center, depth, &by_slug)
        }
        None => {
            let limit = bounded_limit(params.limit, 300, 2000)? as usize;
            // 概览模式优先保留链接最多的页面，避免截断后图变成一堆孤立点。
            let mut ranked: Vec<&WikiPage> = pages.iter().filter(|p| !p.is_index()).collect();
            ranked.sort_by(|a, b| {
                (b.out_links.len() + b.in_links.len())
                    .cmp(&(a.out_links.len() + a.in_links.len()))
                    .then_with(|| a.slug.cmp(&b.slug))
            });
            ranked.into_iter().take(limit).map(|p| p.slug.clone()).collect()
        }
    };

    let total_candidates = pages.iter().filter(|p| !p.is_index()).count();
    let truncated = total_candidates > selected.len();
    let nodes: Vec<WikiGraphNode> = selected
        .iter()
        .filter_map(|slug| by_slug.get(slug))
        .map(|p| WikiGraphNode {
            slug: p.slug.clone(),
            title: p.title.clone(),
            page_type: p.page_type.clone(),
            summary: p.summary.clone(),
        })
        .collect();
    let edges: Vec<WikiGraphEdge> = selected
        .iter()
        .filter_map(|slug| by_slug.get(slug))
        .flat_map(|p| {
            p.out_links
                .iter()
                .filter(|target| selected.contains(*target))
                .map(move |target| WikiGraphEdge { source: p.slug.clone(), target: target.clone() })
        })
        .collect();
    Ok(Json(WikiGraphResponse { nodes, edges, truncated }))
}

/// 以 center 为起点做无向 BFS（出链 + 入链），返回 depth 跳内可达的页面。
fn ego_slugs(center: &str, depth: usize, by_slug: &HashMap<String, &WikiPage>) -> HashSet<String> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    visited.insert(center.to_string());
    queue.push_back((center.to_string(), 0));
    while let Some((slug, level)) = queue.pop_front() {
        if level >= depth {
            continue;
        }
        let Some(page) = by_slug.get(&slug) else { continue };
        let neighbors = page.out_links.iter().chain(page.in_links.iter());
        for neighbor in neighbors {
            if by_slug.contains_key(neighbor) && visited.insert(neighbor.clone()) {
                queue.push_back((neighbor.clone(), level + 1));
            }
        }
    }
    visited
}

#[utoipa::path(get, path="/api/v1/knowledge/wiki/config", operation_id="wiki_get_config", tag="wiki", params(WikiKbParams), responses((status=200, body=WikiConfigResponse)))]
pub async fn get_config(
    Query(params): Query<WikiKbParams>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
) -> ApiResult<Json<WikiConfigResponse>> {
    common::ensure_kb_accessible(&pool, params.kb_id, &user.user_id, user.is_admin()).await?;
    let resolved = wiki::resolve_config(&pool, params.kb_id).await?;
    let Some(resolved) = resolved else {
        return Err(ApiError::NotFound("Knowledge base not found".to_string()));
    };
    Ok(Json(WikiConfigResponse {
        kb_id: params.kb_id,
        enabled: resolved.enabled,
        granularity: resolved.granularity.as_str().to_string(),
        language: resolved.language,
        model: resolved.model,
        max_pages_per_ingest: resolved.max_pages_per_ingest,
        llm_available: crate::config::get().wiki.api_url.is_some(),
    }))
}

#[utoipa::path(put, path="/api/v1/knowledge/wiki/config", operation_id="wiki_update_config", tag="wiki", request_body=WikiConfigUpdateReq, responses((status=200, body=WikiConfigResponse)))]
pub async fn update_config(
    State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>, Json(req): Json<WikiConfigUpdateReq>,
) -> ApiResult<Json<WikiConfigResponse>> {
    common::ensure_kb_editor_or_admin(&pool, req.kb_id, &user).await?;
    if let Some(granularity) = req.granularity.as_deref().filter(|v| !v.trim().is_empty()) {
        if Granularity::parse(granularity).is_none() {
            return Err(ApiError::BadRequest("granularity must be one of focused / standard / exhaustive".to_string()));
        }
    }
    let raw: Option<String> = sqlx::query_scalar("SELECT wiki_config FROM knowledge_bases WHERE id = ?")
        .bind(req.kb_id)
        .fetch_optional(&pool)
        .await?;
    let raw = raw.ok_or_else(|| ApiError::NotFound("Knowledge base not found".to_string()))?;
    let mut config = wiki::KbWikiConfig::parse(Some(raw.as_str()));
    if let Some(enabled) = req.enabled {
        config.enabled = Some(enabled);
    }
    if let Some(granularity) = req.granularity {
        config.granularity = Some(granularity);
    }
    if let Some(language) = req.language {
        config.language = Some(language);
    }
    if let Some(model) = req.model {
        config.model = Some(model);
    }
    if let Some(max_pages) = req.max_pages_per_ingest {
        config.max_pages_per_ingest = Some(max_pages);
    }
    sqlx::query("UPDATE knowledge_bases SET wiki_config = ?, updated_at = strftime('%s','now') WHERE id = ?")
        .bind(serde_json::to_string(&config)?)
        .bind(req.kb_id)
        .execute(&pool)
        .await?;
    get_config(Query(WikiKbParams { kb_id: req.kb_id }), State(pool), Extension(user)).await
}

#[utoipa::path(post, path="/api/v1/knowledge/wiki/rebuild", operation_id="wiki_rebuild", tag="wiki", request_body=WikiRebuildReq, responses((status=200, body=WikiRebuildResponse)))]
pub async fn rebuild(
    State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>, Json(req): Json<WikiRebuildReq>,
) -> ApiResult<Json<WikiRebuildResponse>> {
    common::ensure_kb_editor_or_admin(&pool, req.kb_id, &user).await?;
    let resolved = wiki::resolve_config(&pool, req.kb_id).await?;
    let Some(resolved) = resolved else {
        return Err(ApiError::NotFound("Knowledge base not found".to_string()));
    };
    if !resolved.enabled {
        return Err(ApiError::BadRequest("Wiki is disabled for this knowledge base".to_string()));
    }
    if crate::config::get().wiki.api_url.is_none() {
        // 没有 LLM 地址时入队的任务只会失败重试，直接给出可操作的错误。
        return Err(ApiError::BadRequest(
            "Wiki LLM is not configured: set WIKI_LLM_API_URL or LLM_API_URL".to_string(),
        ));
    }

    let enqueued = match req.file_id {
        Some(file_id) => {
            let belongs: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM files WHERE id = ? AND kb_id = ? AND status = 1)")
                    .bind(file_id)
                    .bind(req.kb_id)
                    .fetch_one(&pool)
                    .await?;
            if !belongs {
                return Err(ApiError::NotFound("File not found or not completed".to_string()));
            }
            // 清掉指纹，否则「内容未变」会被直接跳过，重建等于空操作。
            sqlx::query("UPDATE wiki_builds SET status = 'pending', fingerprint = '' WHERE file_id = ?")
                .bind(file_id)
                .execute(&pool)
                .await?;
            if wiki::queue::enqueue_ingest(&pool, req.kb_id, file_id).await? { 1 } else { 0 }
        }
        None => {
            sqlx::query(
                "UPDATE wiki_builds SET status = 'pending', fingerprint = ''
                  WHERE file_id IN (SELECT id FROM files WHERE kb_id = ?)",
            )
            .bind(req.kb_id)
            .execute(&pool)
            .await?;
            // 集合式入队：一条语句覆盖全库，并跳过已在队列中的文件。
            let result = sqlx::query(
                "INSERT INTO wiki_tasks(kb_id, task_type, op, file_id, not_before)
                 SELECT f.kb_id, 'wiki:ingest', 'add', f.id, strftime('%s','now')
                   FROM files f
                  WHERE f.kb_id = ? AND f.status = 1
                    AND NOT EXISTS (
                        SELECT 1 FROM wiki_tasks t
                         WHERE t.task_type = 'wiki:ingest' AND t.status = 'pending' AND t.file_id = f.id
                    )",
            )
            .bind(req.kb_id)
            .execute(&pool)
            .await?;
            result.rows_affected() as i64
        }
    };
    wiki::queue::enqueue_finalize(&pool, req.kb_id).await?;
    Ok(Json(WikiRebuildResponse { enqueued }))
}

// ============================================================================
// P2：人工编辑、版本快照/回滚、归档、体检
// ============================================================================

impl From<edit::EditError> for ApiError {
    fn from(error: edit::EditError) -> Self {
        match error {
            edit::EditError::NotFound(message) => ApiError::NotFound(message),
            edit::EditError::Invalid(message) => ApiError::BadRequest(message),
            // slug 冲突属于「换个名字再来」，按 400 返回比 409 更贴合现有前端处理
            edit::EditError::Conflict(message) => ApiError::BadRequest(message),
            edit::EditError::Internal(error) => ApiError::Internal(format!("Internal error: {}", error)),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct WikiPageUpdateReq {
    pub kb_id: i64,
    pub slug: String,
    /// 省略的字段保持原值
    pub title: Option<String>,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub aliases: Option<Vec<String>>,
    /// draft / published / archived
    pub status: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct WikiPageCreateReq {
    pub kb_id: i64,
    pub title: String,
    /// entity / concept，默认 concept；summary 与 index 由系统维护
    pub page_type: Option<String>,
    /// 省略时由标题派生
    pub slug: Option<String>,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub aliases: Option<Vec<String>>,
    /// 省略时直接发布
    pub status: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct WikiPageDeleteReq {
    pub kb_id: i64,
    pub slug: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct WikiRevertReq {
    pub kb_id: i64,
    pub slug: String,
    /// 要回滚到的历史版本号
    pub version: i64,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct WikiRevisionListParams {
    pub kb_id: i64,
    pub slug: String,
    /// 1-200，默认 50
    pub limit: Option<i64>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct WikiRevisionParams {
    pub kb_id: i64,
    pub slug: String,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiRevisionListResponse {
    pub kb_id: i64,
    pub slug: String,
    /// 当前版本号，前端据此标注「最新」
    pub current_version: i64,
    pub last_edit_source: String,
    pub items: Vec<revision::RevisionMeta>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiRevisionResponse {
    pub revision: revision::Revision,
    /// 当前版本正文，前端可直接做行级 diff，不必再取一次页面
    pub current_content: String,
    pub current_version: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct WikiRebuildLinksReq {
    pub kb_id: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WikiRebuildLinksResponse {
    pub kb_id: i64,
    pub pages_scanned: usize,
    pub pages_changed: usize,
    pub dead_links_removed: usize,
    pub index_updated: bool,
}

/// slug → 页面。归档页也可读、可回滚，因此这里不按状态过滤。
async fn require_page(pool: &SqlitePool, kb_id: i64, slug: &str) -> ApiResult<WikiPage> {
    page::get_by_slug(pool, kb_id, slug)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Wiki page '{}' not found", slug)))
}

#[utoipa::path(put, path="/api/v1/knowledge/wiki/page", operation_id="wiki_update_page", tag="wiki", request_body=WikiPageUpdateReq, responses((status=200, body=WikiPageDetail)))]
pub async fn update_page(
    State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>, Json(req): Json<WikiPageUpdateReq>,
) -> ApiResult<Json<WikiPageDetail>> {
    common::ensure_kb_editor_or_admin(&pool, req.kb_id, &user).await?;
    let edit = edit::PageEdit {
        title: req.title,
        summary: req.summary,
        content: req.content,
        aliases: req.aliases,
        status: req.status,
        editor_id: user.user_id.clone(),
    };
    let page = edit::apply_user_edit(&pool, req.kb_id, &req.slug, &edit).await?;
    Ok(Json(build_detail(&pool, page).await?))
}

#[utoipa::path(post, path="/api/v1/knowledge/wiki/page", operation_id="wiki_create_page", tag="wiki", request_body=WikiPageCreateReq, responses((status=200, body=WikiPageDetail)))]
pub async fn create_page(
    State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>, Json(req): Json<WikiPageCreateReq>,
) -> ApiResult<Json<WikiPageDetail>> {
    common::ensure_kb_editor_or_admin(&pool, req.kb_id, &user).await?;
    let new = edit::NewPage {
        title: req.title,
        page_type: req.page_type,
        slug: req.slug,
        summary: req.summary.unwrap_or_default(),
        content: req.content.unwrap_or_default(),
        aliases: req.aliases.unwrap_or_default(),
        status: req.status,
        editor_id: user.user_id.clone(),
    };
    let page = edit::create_user_page(&pool, req.kb_id, &new).await?;
    Ok(Json(build_detail(&pool, page).await?))
}

#[utoipa::path(delete, path="/api/v1/knowledge/wiki/page", operation_id="wiki_delete_page", tag="wiki", request_body=WikiPageDeleteReq, responses((status=200, description="已删除")))]
pub async fn delete_page(
    State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>, Json(req): Json<WikiPageDeleteReq>,
) -> ApiResult<()> {
    common::ensure_kb_editor_or_admin(&pool, req.kb_id, &user).await?;
    edit::delete_page(&pool, req.kb_id, &req.slug).await?;
    Ok(())
}

#[utoipa::path(get, path="/api/v1/knowledge/wiki/revisions", operation_id="wiki_list_revisions", tag="wiki", params(WikiRevisionListParams), responses((status=200, body=WikiRevisionListResponse)))]
pub async fn list_revisions(
    Query(params): Query<WikiRevisionListParams>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
) -> ApiResult<Json<WikiRevisionListResponse>> {
    common::ensure_kb_accessible(&pool, params.kb_id, &user.user_id, user.is_admin()).await?;
    let limit = bounded_limit(params.limit, 50, 200)?;
    let page = require_page(&pool, params.kb_id, &params.slug).await?;
    let items = revision::list(&pool, page.id, limit).await?;
    Ok(Json(WikiRevisionListResponse {
        kb_id: params.kb_id,
        slug: page.slug.clone(),
        current_version: page.version,
        last_edit_source: page.last_edit_source.clone(),
        items,
    }))
}

#[utoipa::path(get, path="/api/v1/knowledge/wiki/revision", operation_id="wiki_get_revision", tag="wiki", params(WikiRevisionParams), responses((status=200, body=WikiRevisionResponse)))]
pub async fn get_revision(
    Query(params): Query<WikiRevisionParams>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
) -> ApiResult<Json<WikiRevisionResponse>> {
    common::ensure_kb_accessible(&pool, params.kb_id, &user.user_id, user.is_admin()).await?;
    let page = require_page(&pool, params.kb_id, &params.slug).await?;
    let Some(snapshot) = revision::get(&pool, page.id, params.version).await? else {
        return Err(ApiError::NotFound(format!("Revision {} not found", params.version)));
    };
    Ok(Json(WikiRevisionResponse {
        revision: snapshot,
        current_content: page.content.clone(),
        current_version: page.version,
    }))
}

#[utoipa::path(post, path="/api/v1/knowledge/wiki/revert", operation_id="wiki_revert_page", tag="wiki", request_body=WikiRevertReq, responses((status=200, body=WikiPageDetail)))]
pub async fn revert_page(
    State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>, Json(req): Json<WikiRevertReq>,
) -> ApiResult<Json<WikiPageDetail>> {
    common::ensure_kb_editor_or_admin(&pool, req.kb_id, &user).await?;
    let page = edit::revert(&pool, req.kb_id, &req.slug, req.version, &user.user_id).await?;
    Ok(Json(build_detail(&pool, page).await?))
}

#[utoipa::path(get, path="/api/v1/knowledge/wiki/lint", operation_id="wiki_lint", tag="wiki", params(WikiKbParams), responses((status=200, body=lint::LintReport)))]
pub async fn lint_kb(
    Query(params): Query<WikiKbParams>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
) -> ApiResult<Json<lint::LintReport>> {
    common::ensure_kb_accessible(&pool, params.kb_id, &user.user_id, user.is_admin()).await?;
    Ok(Json(lint::lint_kb(&pool, params.kb_id).await?))
}

/// 立即执行一次知识库收敛（交叉链接、死链清理、入链重算、索引目录），
/// 与后台 `wiki:finalize` 是同一份逻辑，只是不等防抖。
#[utoipa::path(post, path="/api/v1/knowledge/wiki/rebuild-links", operation_id="wiki_rebuild_links", tag="wiki", request_body=WikiRebuildLinksReq, responses((status=200, body=WikiRebuildLinksResponse)))]
pub async fn rebuild_links(
    State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>, Json(req): Json<WikiRebuildLinksReq>,
) -> ApiResult<Json<WikiRebuildLinksResponse>> {
    common::ensure_kb_editor_or_admin(&pool, req.kb_id, &user).await?;
    if wiki::resolve_config(&pool, req.kb_id).await?.is_none() {
        return Err(ApiError::NotFound("Knowledge base not found".to_string()));
    }
    let report = wiki::finalize::finalize_kb(&pool, req.kb_id).await?;
    Ok(Json(WikiRebuildLinksResponse {
        kb_id: req.kb_id,
        pages_scanned: report.pages_scanned,
        pages_changed: report.pages_changed,
        dead_links_removed: report.dead_links_removed,
        index_updated: report.index_updated,
    }))
}
