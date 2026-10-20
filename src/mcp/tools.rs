//! MCP tool 定义。输出分两层：`structured_content` 放完整 JSON（供 MCP App UI 渲染），
//! `content` 放给模型读的精简 markdown。所有工具都是只读的，且都按 AuthUser 做
//! 与 REST 一致的数据级权限过滤。

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::wrapper::Parameters,
    model::{
        CallToolResult, ContentBlock, Extensions, Implementation, ListResourcesResult, PaginatedRequestParams,
        ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, ResourceContents, ServerCapabilities,
        ServerConfig,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{
    AuthUser,
    api::{common, error::ApiError, file as file_api, graph as graph_api, search as search_api, wiki as wiki_api},
    mcp::app_ui,
    search::SearchEngine,
    wiki,
};

/// 单条切片内容截断长度：tool 结果会直接进入模型上下文，原文必须限长。
const SLICE_CONTENT_MAX_CHARS: usize = 1200;
/// Wiki 页正文截断长度。
const WIKI_CONTENT_MAX_CHARS: usize = 20000;

#[derive(Clone)]
pub struct HtknowMcp {
    pool: SqlitePool,
    search_engine: SearchEngine,
}

impl HtknowMcp {
    pub fn new(pool: SqlitePool, search_engine: SearchEngine) -> Self {
        Self { pool, search_engine }
    }
}

/// 从请求扩展取回 auth 中间件注入的用户身份。/mcp 路由挂在 auth 中间件之后，
/// 正常不可能缺失；缺失说明路由装配错了，按内部错误处理而不是悄悄匿名放行。
fn auth_user(extensions: &Extensions) -> Result<AuthUser, ErrorData> {
    extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<AuthUser>())
        .cloned()
        .ok_or_else(|| ErrorData::internal_error("missing auth context on MCP request", None))
}

fn api_err(err: ApiError) -> ErrorData {
    match err {
        ApiError::BadRequest(msg) => ErrorData::invalid_params(msg, None),
        other => ErrorData::internal_error(other.to_string(), None),
    }
}

/// 按字符数截断，返回（截断后文本，是否截断）。
fn truncate_chars(text: &str, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text.to_owned(), false);
    }
    (text.chars().take(max_chars).collect(), true)
}

fn tool_result(structured: serde_json::Value, text: String) -> Result<CallToolResult, ErrorData> {
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.structured_content = Some(structured);
    Ok(result)
}

// ---------- 输出结构 ----------

#[derive(Debug, Serialize)]
struct KbRef {
    id: i64,
    name: String,
}

#[derive(Debug, Serialize)]
struct FileRef {
    id: i64,
    filename: String,
    kb_id: Option<i64>,
}

#[derive(Debug, Serialize)]
struct WikiRef {
    slug: String,
    title: String,
    summary: String,
    kb_id: i64,
}

#[derive(Debug, Serialize)]
struct SearchHit {
    slice_id: i64,
    file_id: i64,
    score: f32,
    content: String,
    content_truncated: bool,
    file: Option<FileRef>,
    kb: Option<KbRef>,
    wiki: Option<WikiRef>,
}

impl SearchHit {
    fn from_item(item: search_api::SearchResultItem) -> Self {
        let (content, content_truncated) = truncate_chars(&item.content, SLICE_CONTENT_MAX_CHARS);
        Self {
            slice_id: item.id,
            file_id: item.file_id,
            score: item.score,
            content,
            content_truncated,
            file: item.file.map(|f| FileRef { id: f.id, filename: f.filename, kb_id: f.kb_id }),
            kb: item.kb.map(|k| KbRef { id: k.id, name: k.name }),
            wiki: item.wiki.map(|w| WikiRef { slug: w.page.slug, title: w.page.title, summary: w.page.summary, kb_id: w.page.kb_id }),
        }
    }

    fn to_markdown(&self) -> String {
        let mut line = format!("- [slice {}] (score {:.3})", self.slice_id, self.score);
        if let Some(file) = &self.file {
            line += &format!(" 《{}》(file_id={})", file.filename, file.id);
        }
        if let Some(kb) = &self.kb {
            line += &format!(" 库:{}", kb.name);
        }
        if let Some(wiki) = &self.wiki {
            line += &format!(" wiki:{}", wiki.slug);
        }
        line += &format!("\n  {}", self.content.replace('\n', " "));
        line
    }
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct KbEntry {
    id: i64,
    user_id: String,
    user_name: String,
    name: String,
    description: String,
    kb_type: String,
    parent_id: Option<i64>,
    is_public: bool,
}

// ---------- 工具入参 ----------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchKnowledgeParams {
    /// 检索关键词或自然语言问题
    pub query: String,
    /// 限定知识库 ID 列表；不传则在当前用户可见的全部知识库中检索
    pub kb_ids: Option<Vec<i64>>,
    /// 限定文件 ID 列表；不传则跨文件检索
    pub file_ids: Option<Vec<i64>>,
    /// 返回条数上限（默认 10，最大 50）
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetFileParams {
    /// 文件 ID
    pub file_id: i64,
    /// 是否返回切片正文（默认 true）
    pub with_slices: Option<bool>,
    /// 切片返回条数上限（默认 20，最大 100）
    pub slice_limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WikiSearchParams {
    /// 知识库 ID
    pub kb_id: i64,
    /// 检索关键词
    pub query: String,
    /// 返回条数上限（默认 10，最大 50）
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WikiGetPageParams {
    /// 知识库 ID
    pub kb_id: i64,
    /// Wiki 页面 slug
    pub slug: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GraphQueryParams {
    /// 实体名称关键词（至少 3 个字符走全文索引）；不传则返回作用域内全部实体
    pub query: Option<String>,
    /// 限定知识库 ID
    pub kb_id: Option<i64>,
    /// 实体类型过滤
    pub entity_type: Option<String>,
    /// 匹配实体数上限（默认 50，最大 200）；关联边最多返回 400 条
    pub limit: Option<i64>,
}

// ---------- 工具实现 ----------

#[tool_router]
impl HtknowMcp {
    #[tool(
        description = "在 HTKnow 知识库中做混合检索（关键词+向量+Wiki），返回命中的文档切片及来源文件/知识库。回答需要知识库事实依据的问题前先调用它。",
        meta = "crate::mcp::app_ui::tool_meta()"
    )]
    async fn search_knowledge(
        &self, extensions: Extensions, Parameters(params): Parameters<SearchKnowledgeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let user = auth_user(&extensions)?;
        let query = params.query.trim();
        if query.is_empty() {
            return Err(ErrorData::invalid_params("query must not be empty", None));
        }
        let limit = params.limit.unwrap_or(10).clamp(1, 50);

        let items = search_api::search_items_for_user(
            &self.pool,
            &self.search_engine,
            &user,
            query,
            params.file_ids.as_ref(),
            params.kb_ids.as_ref(),
        )
        .await
        .map_err(api_err)?;

        let total = items.len();
        let hits: Vec<SearchHit> = items.into_iter().take(limit).map(SearchHit::from_item).collect();
        let text = if hits.is_empty() {
            format!("未检索到与「{}」相关的内容。", query)
        } else {
            let mut text = format!("检索「{}」命中 {} 条（返回 {} 条）：", query, total, hits.len());
            for hit in &hits {
                text.push('\n');
                text.push_str(&hit.to_markdown());
            }
            text
        };
        tool_result(
            serde_json::json!({ "query": query, "total": total, "results": hits }),
            text,
        )
    }

    #[tool(description = "列出当前用户可见的知识库（自有、公开或被授权的），返回 id/名称/描述/层级关系。检索前可用它发现可搜的知识库。")]
    async fn list_knowledge_bases(&self, extensions: Extensions) -> Result<CallToolResult, ErrorData> {
        let user = auth_user(&extensions)?;
        // 与 api::knowledge_base::list 同一套可见性口径，但返回全层级（含子库），
        // 不做分页：agent 需要一次性看清知识结构。
        let entries: Vec<KbEntry> = if user.is_admin() {
            sqlx::query_as(
                "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public FROM knowledge_bases ORDER BY id",
            )
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as(
                "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public FROM knowledge_bases \
                 WHERE user_id = ? OR is_public = 1 OR id IN (SELECT kb_id FROM kb_permissions WHERE user_id = ?) ORDER BY id",
            )
            .bind(&user.user_id)
            .bind(&user.user_id)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(ApiError::Database)
        .map_err(api_err)?;

        let mut text = format!("可见知识库共 {} 个：", entries.len());
        for kb in &entries {
            text.push_str(&format!(
                "\n- [{}] {}{}{}",
                kb.id,
                kb.name,
                kb.parent_id.map(|p| format!("（父库 {}）", p)).unwrap_or_default(),
                if kb.description.is_empty() { String::new() } else { format!("：{}", kb.description) },
            ));
        }
        tool_result(serde_json::json!({ "total": entries.len(), "knowledge_bases": entries }), text)
    }

    #[tool(description = "按文件 ID 获取文档元信息与切片正文（默认前 20 条切片，每条限长）。需要通读某个具体文档时使用。")]
    async fn get_file_detail(
        &self, extensions: Extensions, Parameters(params): Parameters<GetFileParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let user = auth_user(&extensions)?;
        let file: file_api::File =
            sqlx::query_as(&format!("SELECT {} FROM files WHERE id = ?", file_api::FILE_COLS_NO_CONTENT))
                .bind(params.file_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(ApiError::Database)
                .map_err(api_err)?
                .ok_or_else(|| ErrorData::invalid_params(format!("file {} not found", params.file_id), None))?;
        file_api::ensure_file_readable(&self.pool, &file, &user).await.map_err(api_err)?;

        let with_slices = params.with_slices.unwrap_or(true);
        let slice_limit = params.slice_limit.unwrap_or(20).clamp(1, 100);
        let mut slices_json = Vec::new();
        let mut slice_count = 0i64;
        if with_slices {
            // 与 api::file::get_slices 同一口径：共享解析产物的文件读源文件的切片。
            let source_id = file_api::effective_parse_file_id(&self.pool, params.file_id)
                .await
                .map_err(|e| ErrorData::internal_error(format!("resolve parse file failed: {e}"), None))?;
            let rows: Vec<(i64,)> = sqlx::query_as("SELECT id FROM slices WHERE file_id = ? ORDER BY id")
                .bind(source_id)
                .fetch_all(&self.pool)
                .await
                .map_err(ApiError::Database)
                .map_err(api_err)?;
            slice_count = rows.len() as i64;
            let contents = crate::slice_content::read_all(source_id)
                .await
                .map_err(|e| ErrorData::internal_error(format!("read slices failed: {e}"), None))?;
            for (slice_id,) in rows.into_iter().take(slice_limit) {
                let raw = contents.get(&slice_id).cloned().unwrap_or_default();
                let (content, truncated) = truncate_chars(&raw, SLICE_CONTENT_MAX_CHARS);
                slices_json.push(serde_json::json!({
                    "slice_id": slice_id, "content": content, "content_truncated": truncated,
                }));
            }
        }

        let text = format!(
            "文件 {} 《{}》：{} 字节，标签[{}]，切片 {} 条（返回 {} 条）。",
            file.id,
            file.filename,
            file.size,
            file.tags,
            slice_count,
            slices_json.len()
        );
        tool_result(
            serde_json::json!({
                "file": {
                    "id": file.id, "filename": file.filename, "size": file.size, "tags": file.tags,
                    "kb_id": file.kb_id, "is_public": file.is_public, "status": file.status,
                    "created_at": file.created_at, "updated_at": file.updated_at,
                },
                "slice_count": slice_count,
                "slices": slices_json,
            }),
            text,
        )
    }

    #[tool(description = "在指定知识库的 Wiki 中检索条目页（实体/概念/文档摘要），返回 slug/标题/摘要。适合先定位条目再用 wiki_get_page 读正文。")]
    async fn wiki_search(
        &self, extensions: Extensions, Parameters(params): Parameters<WikiSearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let user = auth_user(&extensions)?;
        common::ensure_kb_accessible(&self.pool, params.kb_id, &user.user_id, user.is_admin()).await.map_err(api_err)?;
        let query = params.query.trim();
        if query.is_empty() {
            return Err(ErrorData::invalid_params("query must not be empty", None));
        }
        let limit = params.limit.unwrap_or(10).clamp(1, 50);

        // 与 api::wiki::search_pages 同一口径：索引版本必须与库内页面版本一致，
        // 否则重建索引期间会短暂放回旧页面内容。
        let hits = self
            .search_engine
            .search_wiki_limited(query, Some(&vec![params.kb_id]), limit)
            .await
            .map_err(|e| ErrorData::internal_error(format!("wiki search failed: {e}"), None))?;
        let ids: Vec<i64> = hits.iter().map(|(candidate, _)| candidate.id).collect();
        let mut by_id: std::collections::HashMap<i64, wiki::WikiPage> = wiki::page::get_by_ids(&self.pool, &ids)
            .await
            .map_err(|e| ErrorData::internal_error(format!("wiki page load failed: {e}"), None))?
            .into_iter()
            .map(|page| (page.id, page))
            .collect();
        let mut pages = Vec::new();
        for (candidate, _) in hits {
            let Some(page) = by_id.remove(&candidate.id) else { continue };
            if page.status == wiki::STATUS_PUBLISHED && page.version == candidate.version && page.kb_id == params.kb_id {
                pages.push(serde_json::json!({
                    "slug": page.slug, "title": page.title, "summary": page.summary,
                    "page_type": page.page_type, "updated_at": page.updated_at,
                }));
            }
        }

        let mut text = format!("Wiki 检索「{}」命中 {} 条：", query, pages.len());
        for page in &pages {
            text.push_str(&format!(
                "\n- {}（{}）：{}",
                page["slug"].as_str().unwrap_or_default(),
                page["title"].as_str().unwrap_or_default(),
                page["summary"].as_str().unwrap_or_default()
            ));
        }
        tool_result(serde_json::json!({ "query": query, "kb_id": params.kb_id, "results": pages }), text)
    }

    #[tool(description = "按知识库 ID 和 slug 读取 Wiki 条目页正文（含来源文件与切片证据）。正文超过 2 万字符会被截断。")]
    async fn wiki_get_page(
        &self, extensions: Extensions, Parameters(params): Parameters<WikiGetPageParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let user = auth_user(&extensions)?;
        common::ensure_kb_accessible(&self.pool, params.kb_id, &user.user_id, user.is_admin()).await.map_err(api_err)?;
        let slug = params.slug.trim();
        if slug.is_empty() {
            return Err(ErrorData::invalid_params("slug must not be empty", None));
        }
        let page = wiki::page::get_by_slug(&self.pool, params.kb_id, slug)
            .await
            .map_err(|e| ErrorData::internal_error(format!("wiki page load failed: {e}"), None))?
            .ok_or_else(|| ErrorData::invalid_params(format!("wiki page {slug} not found in kb {}", params.kb_id), None))?;
        let mut detail = wiki_api::build_detail(&self.pool, page).await.map_err(api_err)?;
        let (content, truncated) = truncate_chars(&detail.content, WIKI_CONTENT_MAX_CHARS);
        detail.content = content;

        let text = format!(
            "# {}\n\n{}{}",
            detail.page.title,
            detail.content,
            if truncated { "\n\n（正文已截断）" } else { "" }
        );
        tool_result(
            serde_json::json!({ "page": detail, "content_truncated": truncated }),
            text,
        )
    }

    #[tool(description = "查询知识图谱：按名称/类型匹配实体，并返回与匹配实体相连的关系子图（最多 400 条边）。适合查实体间关系。")]
    async fn graph_query(
        &self, extensions: Extensions, Parameters(params): Parameters<GraphQueryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let user = auth_user(&extensions)?;
        let entity_params = graph_api::EntitySearchParams {
            q: params.query.clone(),
            entity_type: params.entity_type.clone(),
            kb_id: params.kb_id,
            file_id: None,
            limit: Some(params.limit.unwrap_or(50).clamp(1, 200)),
            before_id: None,
            node_id: None,
        };
        let subgraph = graph_api::subgraph_for_user(&self.pool, &user, &entity_params).await.map_err(api_err)?;

        let mut text = format!(
            "图谱查询命中 {} 个实体、{} 条关系{}：",
            subgraph.matched_ids.len(),
            subgraph.edges.len(),
            if subgraph.truncated { "（已截断）" } else { "" }
        );
        for node in subgraph.nodes.iter().filter(|n| subgraph.matched_ids.contains(&n.id)).take(20) {
            text.push_str(&format!("\n- [{}] {}（{}）", node.id, node.name, node.entity_type));
        }
        tool_result(serde_json::to_value(&subgraph).map_err(|e| ErrorData::internal_error(e.to_string(), None))?, text)
    }
}

#[tool_handler]
impl ServerHandler for HtknowMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_resources().build())
            .with_server_info({
                let mut info = Implementation::from_build_env();
                info.name = "htknow".into();
                info.title = Some("HTKnow 知识库".into());
                // from_build_env 的 name/version 取自 rmcp 自身（env! 在宏定义处展开），
                // name 已覆盖，version 也要显式改回 htknow 的版本。
                info.version = env!("CARGO_PKG_VERSION").into();
                info
            })
            .with_instructions(
                "HTKnow 知识库检索服务。search_knowledge 做混合检索，list_knowledge_bases 发现知识库，\
                 get_file_detail 读文档切片，wiki_search / wiki_get_page 查 Wiki 条目，graph_query 查实体关系。\
                 所有数据按当前用户权限过滤。",
            )
    }

    /// MCP Apps：把知识空间单文件 HTML 声明为 ui:// 资源（同 /mcp/app/knowledge-space 托管的那份）。
    async fn list_resources(
        &self, _request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![app_ui::app_resource()]))
    }

    async fn read_resource(
        &self, request: ReadResourceRequestParams, _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if request.uri != app_ui::APP_RESOURCE_URI {
            return Err(ErrorData::resource_not_found(format!("unknown resource: {}", request.uri), None));
        }
        let html = app_ui::app_html()
            .ok_or_else(|| ErrorData::internal_error("mcp-app not built: run `npm run build` in mcp-app/", None))?;
        Ok(ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
            uri: request.uri,
            mime_type: Some("text/html".to_owned()),
            text: html,
            meta: None,
        }])
        .into())
    }
}
