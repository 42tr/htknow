use std::collections::HashMap;

use axum::{
    Extension, extract::{Path, Query, State}, response::Json
};
use log::warn;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};
use utoipa::{IntoParams, ToSchema};

use super::file::{self, FileStatusBreakdown};
use crate::{
    AuthUser, api::error::{ApiError, ApiResult}, search::SearchEngine
};

const KB_TYPE_ANALYSIS: &str = "analysis";
const KB_TYPE_STORAGE: &str = "storage";

fn normalize_kb_type(kb_type: Option<String>) -> Result<String, ApiError> {
    let raw = kb_type.unwrap_or_else(|| KB_TYPE_ANALYSIS.to_string());
    let normalized = raw.trim().to_lowercase();
    match normalized.as_str() {
        KB_TYPE_ANALYSIS | KB_TYPE_STORAGE => Ok(normalized),
        _ => Err(ApiError::BadRequest("Invalid kb_type. Use 'analysis' or 'storage'.".to_string())),
    }
}

fn normalize_parse_priority(parse_priority: Option<i64>) -> Result<i64, ApiError> {
    let value = parse_priority.unwrap_or(50);
    if !(0..=100).contains(&value) {
        return Err(ApiError::BadRequest("Invalid parse_priority. Use integer in [0, 100].".to_string()));
    }
    Ok(value)
}

// ---------------------------------------------------------------------------
// KB permission helpers
// ---------------------------------------------------------------------------

/// Get the highest permission level a user has on a knowledge base.
/// Priority: global admin > owner > explicit permission > is_public.
/// Returns None if the user has no access at all.
pub async fn get_kb_permission(pool: &SqlitePool, kb_id: i64, user_id: &str, is_admin: bool) -> Option<String> {
    if is_admin {
        return Some("admin".to_string());
    }

    // 1. Check if owner
    let owner: Option<String> = sqlx::query_scalar("SELECT user_id FROM knowledge_bases WHERE id = ?")
        .bind(kb_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    if owner.as_deref() == Some(user_id) {
        return Some("admin".to_string());
    }

    // 2. Check explicit permission
    let explicit: Option<String> =
        sqlx::query_scalar("SELECT permission FROM kb_permissions WHERE kb_id = ? AND user_id = ?")
            .bind(kb_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    if let Some(perm) = explicit {
        return Some(perm);
    }

    // 3. Check if public
    let is_public: Option<i64> = sqlx::query_scalar("SELECT is_public FROM knowledge_bases WHERE id = ?")
        .bind(kb_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    if is_public == Some(1) {
        return Some("viewer".to_string());
    }

    None
}

/// Batch version of [`get_kb_permission`]: resolve permissions for many KBs in a fixed
/// number of queries (2 instead of 3*N), avoiding the N+1 pattern in bulk operations.
///
/// Returns a map kb_id -> highest permission. KBs the user cannot access are absent from the map.
pub async fn get_kb_permissions_batch(
    pool: &SqlitePool, kb_ids: &[i64], user_id: &str, is_admin: bool,
) -> HashMap<i64, String> {
    let mut result = HashMap::new();
    if kb_ids.is_empty() {
        return result;
    }

    if is_admin {
        for &id in kb_ids {
            result.insert(id, "admin".to_string());
        }
        return result;
    }

    // Dedupe ids to keep the IN lists small.
    let unique_ids: Vec<i64> = {
        let set: std::collections::HashSet<i64> = kb_ids.iter().copied().collect();
        set.into_iter().collect()
    };

    // Query 1: owner + is_public for each KB.
    let mut kb_qb = QueryBuilder::<Sqlite>::new("SELECT id, user_id, is_public FROM knowledge_bases WHERE id IN (");
    {
        let mut sep = kb_qb.separated(", ");
        for id in &unique_ids {
            sep.push_bind(*id);
        }
    }
    kb_qb.push(")");
    let kb_rows: Vec<(i64, String, i64)> = kb_qb.build_query_as().fetch_all(pool).await.unwrap_or_default();

    // Query 2: explicit permissions for this user.
    let mut perm_qb = QueryBuilder::<Sqlite>::new("SELECT kb_id, permission FROM kb_permissions WHERE user_id = ");
    perm_qb.push_bind(user_id);
    perm_qb.push(" AND kb_id IN (");
    {
        let mut sep = perm_qb.separated(", ");
        for id in &unique_ids {
            sep.push_bind(*id);
        }
    }
    perm_qb.push(")");
    let perm_rows: Vec<(i64, String)> = perm_qb.build_query_as().fetch_all(pool).await.unwrap_or_default();
    let explicit: HashMap<i64, String> = perm_rows.into_iter().collect();

    for (id, owner, is_public) in kb_rows {
        // Priority: owner > explicit permission > is_public.
        let perm = if owner == user_id {
            Some("admin".to_string())
        } else if let Some(p) = explicit.get(&id) {
            Some(p.clone())
        } else if is_public == 1 {
            Some("viewer".to_string())
        } else {
            None
        };
        if let Some(perm) = perm {
            result.insert(id, perm);
        }
    }

    result
}

/// Numeric level for comparison: admin=3, editor=2, viewer=1, none=0.
pub fn perm_level(perm: &str) -> i32 {
    match perm {
        "admin" => 3,
        "editor" => 2,
        "viewer" => 1,
        _ => 0,
    }
}

/// Check whether `actual` permission meets at least `required` permission.
pub fn meets_requirement(actual: Option<&str>, required: &str) -> bool {
    let actual_level = actual.map(perm_level).unwrap_or(0);
    let required_level = perm_level(required);
    actual_level >= required_level
}

/// Get all KB ids that the user has at least viewer access to.
pub async fn get_user_viewable_kb_ids(pool: &SqlitePool, user_id: &str, is_admin: bool) -> Vec<i64> {
    if is_admin {
        return sqlx::query_scalar("SELECT id FROM knowledge_bases").fetch_all(pool).await.unwrap_or_default();
    }
    sqlx::query_scalar(
        "SELECT id FROM knowledge_bases WHERE user_id = ?1 OR is_public = 1 \
         UNION \
         SELECT kb_id FROM kb_permissions WHERE user_id = ?1",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Data models
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, sqlx::FromRow, ToSchema)]
pub struct Knowledge {
    pub id: i64,
    pub user_id: String,
    pub user_name: String,
    pub name: String,
    pub description: String,
    pub kb_type: String,
    pub parent_id: Option<i64>,
    pub is_public: bool,
    pub parse_priority: i64,
    #[serde(default)]
    #[sqlx(default)]
    pub current_user_permission: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, ToSchema)]
pub struct KnowledgeResponse {
    pub id: i64,
    pub user_id: String,
    pub user_name: String,
    pub name: String,
    pub description: String,
    pub kb_type: String,
    pub parent_id: Option<i64>,
    pub is_public: bool,
    pub parse_priority: i64,
    pub file_count: i64,
    pub children_kb_count: i64,
    pub current_user_permission: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, ToSchema)]
pub struct KnowledgeDetailResponse {
    pub id: i64,
    pub user_id: String,
    pub user_name: String,
    pub name: String,
    pub description: String,
    pub kb_type: String,
    pub parent_id: Option<i64>,
    pub is_public: bool,
    pub parse_priority: i64,
    pub file_count: i64,
    pub children_kb_count: i64,
    pub children_kbs: Vec<KnowledgeResponse>,
    pub files: Vec<FileWithoutContent>,
    pub path: Vec<Knowledge>, // For breadcrumbs
    pub status_breakdown: FileStatusBreakdown,
    pub current_user_permission: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, sqlx::FromRow, ToSchema)]
pub struct FileWithoutContent {
    pub id: i64,
    pub user_id: String,
    pub user_name: String,
    pub hash: String,
    pub filename: String,
    pub path: String,
    pub size: i64,
    pub tags: String,
    pub status: i32,
    pub log: String,
    pub slice_type: String,
    pub kb_id: Option<i64>,
    pub is_public: bool,
    pub meta: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, sqlx::FromRow, ToSchema)]
pub struct KnowledgeTreeFile {
    pub id: i64,
    pub size: i64,
    pub filename: String,
    pub meta: Option<String>,
    pub kb_id: Option<i64>,
    pub is_public: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, ToSchema)]
pub struct KnowledgeTreeNode {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub is_public: bool,
    pub kb_type: String,
    pub files: Vec<KnowledgeTreeFile>,
    #[schema(no_recursion)]
    pub children: Vec<KnowledgeTreeNode>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListQuery {
    /// 页码，从1开始
    pub page: Option<i64>,
    /// 每页条数
    pub size: Option<i64>,
    // /// 关键词搜索信息（在 name + description 中搜索）
    // pub keyword: Option<String>,
    /// 模糊搜索 name 字段
    pub name: Option<String>,
    /// 知识库 ID（精确匹配）
    pub id: Option<String>,
    /// 根据父知识库ID筛选，若不传则获取顶级知识库
    pub parent_id: Option<i64>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct TreeQuery {
    /// 知识库 ID（可选），传入则返回该知识库的子树，不传则返回完整树
    pub kb_id: Option<i64>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct KnowledgeDetailQuery {
    /// 文件名模糊搜索（%filename%）
    pub filename: Option<String>,
    /// 根据标签筛选
    pub tag: Option<String>,
}

/// 获取知识库列表
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/knowledge_base/",
    operation_id = "knowledge_base_list",
    tag = "knowledge_base",
    params(ListQuery),
    responses(
        (status = 200, description = "成功返回知识库列表", body = Vec<KnowledgeResponse>),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn list(
    State(pool): State<SqlitePool>, Query(params): Query<ListQuery>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<Vec<KnowledgeResponse>>> {
    let is_admin = auth_user.is_admin();
    // Determine pagination: default size 10, default page 1
    let size = params.size.unwrap_or(10).max(1);
    let page = params.page.unwrap_or(1).max(1);
    let limit = size;
    let offset = (page - 1) * size;

    // Start building the query
    let mut qb = QueryBuilder::<Sqlite>::new(
        "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public, parse_priority FROM knowledge_bases WHERE 1=1 ",
    );
    if !is_admin {
        let user_id = auth_user.user_id.clone();
        qb.push(" AND (user_id = ")
            .push_bind(user_id.clone())
            .push(" OR is_public = 1 OR id IN (SELECT kb_id FROM kb_permissions WHERE user_id = ")
            .push_bind(user_id)
            .push("))");
    }

    // Filter by parent_id
    if let Some(parent_id) = params.parent_id {
        qb.push(" AND parent_id = ").push_bind(parent_id);
    } else {
        qb.push(" AND parent_id IS NULL");
    }

    // If `id` provided, try to parse as integer id and filter by id
    if let Some(id_str) = params.id.as_deref() {
        qb.push(" AND id = ").push_bind(id_str);
    }

    // name fuzzy search (only name column)
    if let Some(name) = &params.name {
        qb.push("AND name LIKE ").push_bind(format!("%{}%", name));
    }

    // ordering and pagination
    qb.push(" ORDER BY id");
    qb.push(" LIMIT ").push_bind(limit);
    qb.push(" OFFSET ").push_bind(offset);

    // Execute
    let query = qb.build_query_as::<Knowledge>();
    let knowledges = query.fetch_all(&pool).await?;
    let knowledge_ids: Vec<i64> = knowledges.iter().map(|kb| kb.id).collect();

    // Get file counts and children counts in parallel
    let (file_counts_res, children_counts_res) = tokio::join!(
        get_file_counts(&pool, &knowledge_ids, &auth_user.user_id, is_admin),
        get_children_kb_counts(&pool, &knowledge_ids, &auth_user.user_id, is_admin)
    );
    let file_counts = file_counts_res?;
    let children_counts = children_counts_res?;

    let knowledge_responses = knowledges
        .into_iter()
        .map(|kb| KnowledgeResponse {
            id: kb.id,
            user_id: kb.user_id.clone(),
            user_name: kb.user_name.clone(),
            name: kb.name.clone(),
            description: kb.description.clone(),
            kb_type: kb.kb_type.clone(),
            parent_id: kb.parent_id,
            is_public: kb.is_public,
            parse_priority: kb.parse_priority,
            file_count: *file_counts.get(&kb.id).unwrap_or(&0),
            children_kb_count: *children_counts.get(&kb.id).unwrap_or(&0),
            current_user_permission: if kb.user_id == auth_user.user_id || is_admin {
                "admin".to_string()
            } else if kb.is_public {
                "viewer".to_string()
            } else {
                // fallback - should not happen since query already filters
                "viewer".to_string()
            },
        })
        .collect();

    Ok(Json(knowledge_responses))
}

/// Build the access-filter clause used in CTEs.
fn push_kb_access_filter<'a>(qb: &mut QueryBuilder<'a, Sqlite>, user_id: &'a str) {
    qb.push(" AND (user_id = ");
    qb.push_bind(user_id);
    qb.push(" OR is_public = 1 OR id IN (SELECT kb_id FROM kb_permissions WHERE user_id = ");
    qb.push_bind(user_id);
    qb.push(")");
    qb.push(")");
}

fn push_kb_access_filter_where<'a>(qb: &mut QueryBuilder<'a, Sqlite>, user_id: &'a str) {
    qb.push(" WHERE (kb.user_id = ");
    qb.push_bind(user_id);
    qb.push(" OR kb.is_public = 1 OR kb.id IN (SELECT kb_id FROM kb_permissions WHERE user_id = ");
    qb.push_bind(user_id);
    qb.push(")");
    qb.push(")");
}

async fn get_children_kb_counts(
    pool: &SqlitePool, knowledge_ids: &[i64], user_id: &str, is_admin: bool,
) -> anyhow::Result<HashMap<i64, i64>> {
    if knowledge_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut qb = QueryBuilder::<Sqlite>::new(
        "WITH RECURSIVE descendants(root_id, kb_id) AS (SELECT id AS root_id, id AS kb_id FROM knowledge_bases WHERE id IN (",
    );
    let mut separated = qb.separated(", ");
    for id in knowledge_ids {
        separated.push_bind(id);
    }
    qb.push(")");
    if !is_admin {
        push_kb_access_filter(&mut qb, user_id);
    }
    qb.push(" UNION ALL SELECT d.root_id, kb.id FROM knowledge_bases kb ");
    qb.push("JOIN descendants d ON kb.parent_id = d.kb_id");
    if !is_admin {
        push_kb_access_filter_where(&mut qb, user_id);
    }
    qb.push(") ");
    qb.push("SELECT root_id, COUNT(*) - 1 AS cnt FROM descendants GROUP BY root_id");

    let rows = qb.build().fetch_all(pool).await?;

    let children_counts = rows
        .into_iter()
        .filter_map(|row| {
            let root_id: Option<i64> = row.get("root_id");
            let cnt: i64 = row.get("cnt");
            root_id.map(|id| (id, cnt))
        })
        .collect();

    Ok(children_counts)
}

async fn get_file_counts(
    pool: &SqlitePool, knowledge_ids: &[i64], user_id: &str, is_admin: bool,
) -> anyhow::Result<HashMap<i64, i64>> {
    if knowledge_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut qb = QueryBuilder::<Sqlite>::new(
        "WITH RECURSIVE descendants(root_id, kb_id) AS (SELECT id AS root_id, id AS kb_id FROM knowledge_bases WHERE id IN (",
    );
    let mut separated = qb.separated(", ");
    for id in knowledge_ids {
        separated.push_bind(id);
    }
    qb.push(")");
    if !is_admin {
        push_kb_access_filter(&mut qb, user_id);
    }
    qb.push(" UNION ALL SELECT d.root_id, kb.id FROM knowledge_bases kb ");
    qb.push("JOIN descendants d ON kb.parent_id = d.kb_id");
    if !is_admin {
        push_kb_access_filter_where(&mut qb, user_id);
    }
    qb.push(") ");
    qb.push("SELECT d.root_id, COUNT(f.id) AS cnt FROM descendants d ");
    qb.push("LEFT JOIN files f ON f.kb_id = d.kb_id");
    if !is_admin {
        qb.push(" AND (f.user_id = ");
        qb.push_bind(user_id);
        qb.push(" OR f.is_public = 1)");
    }
    qb.push(" GROUP BY d.root_id");

    let rows = qb.build().fetch_all(pool).await?;

    let file_counts = rows
        .into_iter()
        .filter_map(|row| {
            let root_id: Option<i64> = row.get("root_id");
            let cnt: i64 = row.get("cnt");
            root_id.map(|id| (id, cnt))
        })
        .collect();

    Ok(file_counts)
}

#[derive(Deserialize, ToSchema)]
pub struct KnowledgeCreateReq {
    pub name: String,
    pub description: String,
    pub kb_type: Option<String>,
    pub parent_id: Option<i64>,
    pub is_public: Option<bool>,
    pub parse_priority: Option<i64>,
}

/// 创建知识库
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/knowledge_base/",
    operation_id = "knowledge_base_create",
    tag = "knowledge_base",
    request_body = KnowledgeCreateReq,
    responses(
        (status = 200, description = "成功创建知识库", body = Knowledge),
        (status = 400, description = "请求参数错误"),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn create(
    State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>,
    Json(knowledge): Json<KnowledgeCreateReq>,
) -> ApiResult<Json<Knowledge>> {
    let is_public = if knowledge.is_public.unwrap_or(false) { 1 } else { 0 };
    let kb_type = normalize_kb_type(knowledge.kb_type)?;
    let parse_priority = normalize_parse_priority(knowledge.parse_priority)?;
    let query = "INSERT INTO knowledge_bases (user_id, user_name, name, description, kb_type, parent_id, is_public, parse_priority) VALUES (?, ?, ?, ?, ?, ?, ?, ?)";
    let id = sqlx::query(query)
        .bind(auth_user.user_id)
        .bind(auth_user.user_name)
        .bind(knowledge.name)
        .bind(knowledge.description.clone())
        .bind(kb_type)
        .bind(knowledge.parent_id)
        .bind(is_public)
        .bind(parse_priority)
        .execute(&pool)
        .await?
        .last_insert_rowid();
    let mut kb: Knowledge = sqlx::query_as(
        "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public, parse_priority FROM knowledge_bases WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&pool)
    .await?;
    kb.current_user_permission = "admin".to_string();
    Ok(Json(kb))
}

#[derive(Deserialize, ToSchema)]
pub struct KnowledgeUpdateReq {
    pub name: Option<String>,
    pub description: Option<String>,
    pub kb_type: Option<String>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub parent_id: Option<Option<i64>>,
    pub is_public: Option<bool>,
    pub parse_priority: Option<i64>,
}

/// 更新知识库
#[utoipa::path(
    put,
    path = "/api/v1/knowledge/knowledge_base/{id}",
    operation_id = "knowledge_base_update",
    tag = "knowledge_base",
    params(
        ("id" = i64, Path, description = "知识库 ID")
    ),
    request_body = KnowledgeUpdateReq,
    responses(
        (status = 200, description = "成功更新知识库", body = Knowledge),
        (status = 400, description = "请求参数错误"),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn update(
    Path(id): Path<i64>, State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>,
    Json(knowledge): Json<KnowledgeUpdateReq>,
) -> ApiResult<Json<Knowledge>> {
    let is_admin = auth_user.is_admin();
    let user_perm = get_kb_permission(&pool, id, &auth_user.user_id, is_admin).await;
    let perm_str = user_perm.as_deref().unwrap_or("");

    if !meets_requirement(user_perm.as_deref(), "editor") {
        return Err(ApiError::Forbidden("Permission denied. Requires editor or admin.".to_string()));
    }

    // Prevent moving a knowledge base into itself.
    if let Some(Some(parent_id)) = knowledge.parent_id
        && parent_id == id
    {
        return Err(crate::api::error::ApiError::BadRequest("Cannot move a knowledge base into itself.".to_string()));
    }
    // A full descendant check would be needed for production to prevent moving a KB into its own child.
    // This requires a recursive query and is omitted for this iteration.

    // Only admin-level can change sensitive fields: is_public, parent_id, kb_type
    let is_kb_admin = meets_requirement(Some(perm_str), "admin");
    if let Some(ref _kb_type) = knowledge.kb_type {
        if !is_kb_admin {
            return Err(ApiError::Forbidden("Only admin can change kb_type.".to_string()));
        }
    }
    if knowledge.parent_id.is_some() {
        if !is_kb_admin {
            return Err(ApiError::Forbidden("Only admin can change parent_id.".to_string()));
        }
    }
    if knowledge.is_public.is_some() {
        if !is_kb_admin {
            return Err(ApiError::Forbidden("Only admin can change visibility.".to_string()));
        }
    }

    let mut qb = QueryBuilder::<Sqlite>::new("UPDATE knowledge_bases SET ");
    let mut has_update = false;

    if let Some(name) = knowledge.name {
        if has_update {
            qb.push(", ");
        }
        qb.push("name = ");
        qb.push_bind(name);
        has_update = true;
    }
    if let Some(description) = knowledge.description {
        if has_update {
            qb.push(", ");
        }
        qb.push("description = ");
        qb.push_bind(description);
        has_update = true;
    }
    if let Some(kb_type) = knowledge.kb_type {
        let kb_type = normalize_kb_type(Some(kb_type))?;
        if has_update {
            qb.push(", ");
        }
        qb.push("kb_type = ");
        qb.push_bind(kb_type);
        has_update = true;
    }
    // With double_option, this correctly distinguishes "not present" from "present and null"
    if let Some(parent_id) = knowledge.parent_id {
        if has_update {
            qb.push(", ");
        }
        qb.push("parent_id = ");
        qb.push_bind(parent_id); // This binds Option<i64> which sqlx handles (None becomes NULL)
        has_update = true;
    }
    if let Some(is_public) = knowledge.is_public {
        if has_update {
            qb.push(", ");
        }
        qb.push("is_public = ");
        qb.push_bind(if is_public { 1 } else { 0 });
        has_update = true;
    }
    if let Some(parse_priority) = knowledge.parse_priority {
        let parse_priority = normalize_parse_priority(Some(parse_priority))?;
        if has_update {
            qb.push(", ");
        }
        qb.push("parse_priority = ");
        qb.push_bind(parse_priority);
        has_update = true;
    }

    if !has_update {
        let mut kb: Knowledge = sqlx::query_as(
            "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public, parse_priority FROM knowledge_bases WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&pool)
        .await?;
        kb.current_user_permission = "admin".to_string();
        return Ok(Json(kb));
    }

    qb.push(" WHERE id = ");
    qb.push_bind(id);

    let result = qb.build().execute(&pool).await?;

    if result.rows_affected() == 0 {
        return Err(crate::api::error::ApiError::NotFound(
            "Knowledge base not found or permission denied.".to_string(),
        ));
    }

    let mut kb: Knowledge = sqlx::query_as(
        "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public, parse_priority FROM knowledge_bases WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&pool)
    .await?;
    kb.current_user_permission = "admin".to_string();
    Ok(Json(kb))
}

/// 获取知识库详情
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/knowledge_base/{id}",
    operation_id = "knowledge_base_get",
    tag = "knowledge_base",
    params(
        ("id" = i64, Path, description = "知识库 ID"),
        KnowledgeDetailQuery
    ),
    responses(
        (status = 200, description = "成功返回知识库详情", body = KnowledgeDetailResponse),
        (status = 401, description = "未授权"),
        (status = 404, description = "知识库不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn get(
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Query(query): Query<KnowledgeDetailQuery>,
    Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<KnowledgeDetailResponse>> {
    let is_admin = auth_user.is_admin();
    let user_id = auth_user.user_id.clone();

    // 0. Check permission
    let user_perm = get_kb_permission(&pool, id, &user_id, is_admin).await;
    if user_perm.is_none() {
        return Err(ApiError::NotFound("Knowledge base not found or permission denied.".to_string()));
    }
    let current_user_permission = user_perm.clone().unwrap();

    // 1. Fetch the main knowledge base (already permission-checked above)
    let main_kb: Knowledge = sqlx::query_as(
        "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public, parse_priority FROM knowledge_bases WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&pool)
    .await?;

    // 2. Fetch children KBs and files in parallel
    let children_future = async {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public, parse_priority \
             FROM knowledge_bases WHERE parent_id = ",
        );
        qb.push_bind(id);
        if !is_admin {
            let uid = user_id.clone();
            qb.push(" AND (user_id = ")
                .push_bind(uid.clone())
                .push(" OR is_public = 1 OR id IN (SELECT kb_id FROM kb_permissions WHERE user_id = ")
                .push_bind(uid)
                .push("))");
        }
        qb.push(" ORDER BY name");
        qb.build_query_as::<Knowledge>().fetch_all(&pool).await
    };

    let files_future = async {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT id, user_id, user_name, hash, filename, path, size, tags, status, log, slice_type, kb_id, is_public, meta, created_at, updated_at FROM files WHERE kb_id = ",
        );
        qb.push_bind(id);
        if !is_admin {
            qb.push(" AND (user_id = ").push_bind(user_id.clone()).push(" OR is_public = 1)");
        }
        if let Some(filename) = query.filename.as_deref() {
            qb.push(" AND filename LIKE ").push_bind(format!("%{}%", filename));
        }
        qb.push(" ORDER BY updated_at DESC");
        qb.build_query_as::<FileWithoutContent>().fetch_all(&pool).await
    };

    let (children_kbs_res, files_res) = tokio::join!(children_future, files_future);
    let children_kbs: Vec<Knowledge> = children_kbs_res?;
    let mut files: Vec<FileWithoutContent> = files_res?;
    let mut count_ids = Vec::with_capacity(children_kbs.len() + 1);
    count_ids.push(id);
    count_ids.extend(children_kbs.iter().map(|kb| kb.id));
    let (file_counts_res, children_counts_res) = tokio::join!(
        get_file_counts(&pool, &count_ids, &auth_user.user_id, is_admin),
        get_children_kb_counts(&pool, &count_ids, &auth_user.user_id, is_admin)
    );
    let file_counts = file_counts_res?;
    let children_counts = children_counts_res?;
    let file_count = *file_counts.get(&id).unwrap_or(&0);
    let children_kb_count = *children_counts.get(&id).unwrap_or(&0);
    let status_breakdown =
        file::get_file_status_breakdown_for_kb(&pool, id, true, &auth_user.user_id, is_admin).await?;

    // Compute permission for each child KB in a fixed number of queries (avoid N+1).
    let child_ids: Vec<i64> = children_kbs.iter().map(|kb| kb.id).collect();
    let child_perms = get_kb_permissions_batch(&pool, &child_ids, &user_id, is_admin).await;

    let children_kbs: Vec<KnowledgeResponse> = children_kbs
        .into_iter()
        .map(|kb| KnowledgeResponse {
            id: kb.id,
            user_id: kb.user_id,
            user_name: kb.user_name,
            name: kb.name,
            description: kb.description,
            kb_type: kb.kb_type,
            parent_id: kb.parent_id,
            is_public: kb.is_public,
            parse_priority: kb.parse_priority,
            file_count: *file_counts.get(&kb.id).unwrap_or(&0),
            children_kb_count: *children_counts.get(&kb.id).unwrap_or(&0),
            current_user_permission: child_perms.get(&kb.id).cloned().unwrap_or_else(|| "viewer".to_string()),
        })
        .collect();

    if let Some(tag) = &query.tag {
        files.retain(|file| {
            if let Ok(tags) = serde_json::from_str::<Vec<String>>(&file.tags) { tags.contains(tag) } else { false }
        });
    }

    // 3. Fetch the breadcrumb path in a single recursive CTE query (root -> parent),
    //    avoiding one round-trip per ancestor level.
    let path: Vec<Knowledge> = sqlx::query_as(
        "WITH RECURSIVE ancestors(id, user_id, user_name, name, description, kb_type, parent_id, is_public, parse_priority, depth) AS ( \
             SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public, parse_priority, 0 \
             FROM knowledge_bases WHERE id = ? \
             UNION ALL \
             SELECT k.id, k.user_id, k.user_name, k.name, k.description, k.kb_type, k.parent_id, k.is_public, k.parse_priority, a.depth + 1 \
             FROM knowledge_bases k INNER JOIN ancestors a ON k.id = a.parent_id \
         ) \
         SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public, parse_priority \
         FROM ancestors ORDER BY depth DESC",
    )
    .bind(main_kb.parent_id)
    .fetch_all(&pool)
    .await?;

    // 4. Construct the response
    let response = KnowledgeDetailResponse {
        id: main_kb.id,
        user_id: main_kb.user_id,
        user_name: main_kb.user_name,
        name: main_kb.name,
        description: main_kb.description,
        kb_type: main_kb.kb_type,
        parent_id: main_kb.parent_id,
        is_public: main_kb.is_public,
        parse_priority: main_kb.parse_priority,
        file_count,
        children_kb_count,
        children_kbs,
        files,
        path,
        status_breakdown,
        current_user_permission,
    };

    Ok(Json(response))
}

/// 删除知识库
#[utoipa::path(
    delete,
    path = "/api/v1/knowledge/knowledge_base/{id}",
    operation_id = "knowledge_base_delete",
    tag = "knowledge_base",
    params(
        ("id" = i64, Path, description = "知识库 ID")
    ),
    responses(
        (status = 200, description = "成功删除知识库"),
        (status = 401, description = "未授权"),
        (status = 404, description = "知识库不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn delete(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>, Path(id): Path<i64>,
    Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<()> {
    let is_admin = auth_user.is_admin();

    // Check permission: only admin can delete
    let user_perm = get_kb_permission(&pool, id, &auth_user.user_id, is_admin).await;
    if !meets_requirement(user_perm.as_deref(), "admin") {
        return Err(ApiError::Forbidden("Permission denied. Admin role required.".to_string()));
    }

    let all_kb_ids: Vec<i64> = sqlx::query_scalar(
        r#"
        WITH RECURSIVE kb_hierarchy AS (
            SELECT id FROM knowledge_bases WHERE id = ?
            UNION ALL
            SELECT kb.id FROM knowledge_bases kb
            INNER JOIN kb_hierarchy kh ON kb.parent_id = kh.id
        )
        SELECT id FROM kb_hierarchy;
        "#,
    )
    .bind(id)
    .fetch_all(&pool)
    .await?;

    if all_kb_ids.is_empty() {
        return Err(crate::api::error::ApiError::NotFound(
            "Knowledge base not found or permission denied.".to_string(),
        ));
    }

    let mut files_qb =
        QueryBuilder::new(format!("SELECT {} FROM files WHERE kb_id IN (", super::file::FILE_COLS_NO_CONTENT));
    let mut files_separated = files_qb.separated(", ");
    for kb_id in &all_kb_ids {
        files_separated.push_bind(kb_id);
    }
    files_qb.push(")");
    let files: Vec<super::file::File> = files_qb.build_query_as().fetch_all(&pool).await?;

    let file_ids: Vec<i64> = files.iter().map(|f| f.id).collect();
    let image_paths = super::file::collect_image_paths_for_files(&pool, &file_ids).await?;

    // 先分批删除文件相关行（每批独立提交）。删除整个知识库时文件数可能极多，
    // 单个事务级联删除会长时间持有 SQLite 写锁、阻塞其它写入；分批提交把锁持有时间限制在每批内。
    super::file::delete_file_rows_batched(&pool, &file_ids).await?;

    // 文件已删除，再在一个短事务里删除知识库本身（含子库由外键级联处理，与原行为一致）。
    let mut tx = pool.begin().await?;
    let result = sqlx::query("DELETE FROM knowledge_bases WHERE id = ?").bind(id).execute(&mut *tx).await?;

    if result.rows_affected() == 0 {
        return Err(crate::api::error::ApiError::NotFound(
            "Knowledge base not found or permission denied.".to_string(),
        ));
    }

    tx.commit().await?;

    let cleanup_failed = super::file::cleanup_deleted_files(&search_engine, &files, image_paths).await;
    for failure in cleanup_failed {
        log::warn!(
            "Knowledge base delete cleanup failed for file {} at {}: {}",
            failure.id,
            failure.stage,
            failure.error
        );
    }

    for kb_id in &all_kb_ids {
        if let Err(e) = search_engine.delete(None, Some(*kb_id)).await {
            log::warn!("Failed to delete search index for knowledge base {}: {}", kb_id, e);
        }
    }

    Ok(())
}

#[derive(Serialize, ToSchema)]
pub struct ReparseKnowledgeBaseResponse {
    pub kb_count: i64,
    pub file_count: i64,
}

fn push_i64_list(qb: &mut QueryBuilder<Sqlite>, ids: &[i64]) {
    let mut separated = qb.separated(", ");
    for id in ids {
        separated.push_bind(*id);
    }
}

async fn query_file_ids_for_kbs(
    pool: &SqlitePool, kb_ids: &[i64], user_id_filter: Option<&str>,
) -> Result<Vec<i64>, sqlx::Error> {
    if kb_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut qb = QueryBuilder::<Sqlite>::new("SELECT id FROM files WHERE kb_id IN (");
    push_i64_list(&mut qb, kb_ids);
    qb.push(")");
    if let Some(user_id) = user_id_filter {
        qb.push(" AND user_id = ").push_bind(user_id);
    }
    qb.build_query_scalar().fetch_all(pool).await
}

async fn reset_reparse_scope(
    pool: &SqlitePool, search_engine: &SearchEngine, analysis_kb_ids: &[i64], unassigned_file_ids: &[i64],
    file_ids: &[i64], clear_unassigned_graph: bool,
) -> ApiResult<()> {
    // 清理搜索索引
    search_engine.delete_batch(None, Some(analysis_kb_ids)).await?;
    search_engine.delete_batch(Some(unassigned_file_ids), None).await?;

    // 清理知识图谱数据（节点会级联删除边和提及）
    if !analysis_kb_ids.is_empty() {
        let mut del_nodes_qb = QueryBuilder::<Sqlite>::new("DELETE FROM graph_nodes WHERE kb_id IN (");
        push_i64_list(&mut del_nodes_qb, analysis_kb_ids);
        del_nodes_qb.push(")");
        del_nodes_qb.build().execute(pool).await?;

        let mut del_snapshots_qb = QueryBuilder::<Sqlite>::new("DELETE FROM graph_snapshots WHERE kb_id IN (");
        push_i64_list(&mut del_snapshots_qb, analysis_kb_ids);
        del_snapshots_qb.push(")");
        del_snapshots_qb.build().execute(pool).await?;
    }
    if clear_unassigned_graph {
        sqlx::query("DELETE FROM graph_nodes WHERE kb_id IS NULL").execute(pool).await?;
        sqlx::query("DELETE FROM graph_snapshots WHERE kb_id IS NULL").execute(pool).await?;
    }

    if !file_ids.is_empty() {
        let mut del_slices_qb = QueryBuilder::<Sqlite>::new("DELETE FROM slices WHERE file_id IN (");
        push_i64_list(&mut del_slices_qb, file_ids);
        del_slices_qb.push(")");
        del_slices_qb.build().execute(pool).await?;

        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let mut update_qb = QueryBuilder::<Sqlite>::new("UPDATE files SET status = 0, log = '', updated_at = ");
        update_qb.push_bind(now);
        update_qb.push(" WHERE id IN (");
        push_i64_list(&mut update_qb, file_ids);
        update_qb.push(")");
        update_qb.build().execute(pool).await?;
    }

    Ok(())
}

/// 重新解析所有知识库
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/knowledge_base/reparse",
    operation_id = "knowledge_base_reparse",
    tag = "knowledge_base",
    responses(
        (status = 200, description = "已提交重新解析", body = ReparseKnowledgeBaseResponse),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn reparse(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<ReparseKnowledgeBaseResponse>> {
    // Get analysis-type KBs owned by or explicitly editable by the user
    let mut analysis_kb_ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM knowledge_bases WHERE user_id = ? AND kb_type != ?")
            .bind(auth_user.user_id.clone())
            .bind(KB_TYPE_STORAGE)
            .fetch_all(&pool)
            .await?;

    // Also include KBs where user has editor/admin via kb_permissions
    let perm_kb_ids: Vec<i64> =
        sqlx::query_scalar("SELECT kb_id FROM kb_permissions WHERE user_id = ? AND permission IN ('editor', 'admin')")
            .bind(&auth_user.user_id)
            .fetch_all(&pool)
            .await?;

    for kb_id in perm_kb_ids {
        if !analysis_kb_ids.contains(&kb_id) {
            let is_analysis: Option<i64> =
                sqlx::query_scalar("SELECT 1 FROM knowledge_bases WHERE id = ? AND kb_type != ?")
                    .bind(kb_id)
                    .bind(KB_TYPE_STORAGE)
                    .fetch_optional(&pool)
                    .await?;
            if is_analysis.is_some() {
                analysis_kb_ids.push(kb_id);
            }
        }
    }

    let unassigned_file_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM files WHERE user_id = ? AND kb_id IS NULL")
        .bind(auth_user.user_id.clone())
        .fetch_all(&pool)
        .await?;

    let kb_file_ids = query_file_ids_for_kbs(&pool, &analysis_kb_ids, Some(&auth_user.user_id)).await?;

    let mut file_ids = kb_file_ids.clone();
    file_ids.extend(unassigned_file_ids.iter().copied());
    if analysis_kb_ids.is_empty() && unassigned_file_ids.is_empty() {
        return Ok(Json(ReparseKnowledgeBaseResponse { kb_count: 0, file_count: 0 }));
    }

    reset_reparse_scope(&pool, &search_engine, &analysis_kb_ids, &unassigned_file_ids, &file_ids, true).await?;

    Ok(Json(ReparseKnowledgeBaseResponse { kb_count: analysis_kb_ids.len() as i64, file_count: file_ids.len() as i64 }))
}

/// 重新解析指定知识库（包含子知识库）
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/knowledge_base/{id}/reparse",
    operation_id = "knowledge_base_reparse_by_id",
    tag = "knowledge_base",
    params(
        ("id" = i64, Path, description = "知识库 ID")
    ),
    responses(
        (status = 200, description = "已提交指定知识库重新解析", body = ReparseKnowledgeBaseResponse),
        (status = 404, description = "知识库不存在或无权限"),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn reparse_by_id(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>, Path(id): Path<i64>,
    Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<ReparseKnowledgeBaseResponse>> {
    let is_admin = auth_user.is_admin();
    let user_perm = get_kb_permission(&pool, id, &auth_user.user_id, is_admin).await;
    if !meets_requirement(user_perm.as_deref(), "editor") {
        return Err(ApiError::Forbidden("Permission denied. Requires editor or admin.".to_string()));
    }

    let analysis_kb_ids: Vec<i64> = sqlx::query_scalar(
        r#"
        WITH RECURSIVE descendants AS (
            SELECT id, kb_type FROM knowledge_bases WHERE id = ?
            UNION ALL
            SELECT kb.id, kb.kb_type
            FROM knowledge_bases kb
            INNER JOIN descendants d ON kb.parent_id = d.id
        )
        SELECT id FROM descendants WHERE kb_type != ?;
        "#,
    )
    .bind(id)
    .bind(KB_TYPE_STORAGE)
    .fetch_all(&pool)
    .await?;

    let file_ids = query_file_ids_for_kbs(&pool, &analysis_kb_ids, None).await?;
    if analysis_kb_ids.is_empty() {
        return Ok(Json(ReparseKnowledgeBaseResponse { kb_count: analysis_kb_ids.len() as i64, file_count: 0 }));
    }

    reset_reparse_scope(&pool, &search_engine, &analysis_kb_ids, &[], &file_ids, false).await?;
    Ok(Json(ReparseKnowledgeBaseResponse { kb_count: analysis_kb_ids.len() as i64, file_count: file_ids.len() as i64 }))
}

async fn load_tree_knowledges(
    pool: &SqlitePool, root_kb_id: Option<i64>, user_id: &str, is_admin: bool,
) -> anyhow::Result<Vec<TreeKnowledge>> {
    let access_clause = "(user_id = ? OR is_public = 1 OR id IN (SELECT kb_id FROM kb_permissions WHERE user_id = ?))";
    let access_where =
        "WHERE kb.user_id = ? OR kb.is_public = 1 OR kb.id IN (SELECT kb_id FROM kb_permissions WHERE user_id = ?)";

    let rows = match (root_kb_id, is_admin) {
        (Some(kb_id), true) => {
            sqlx::query_as(
                r#"
                WITH RECURSIVE tree AS (
                    SELECT id, name, description, kb_type, parent_id, is_public
                    FROM knowledge_bases
                    WHERE id = ?
                    UNION ALL
                    SELECT kb.id, kb.name, kb.description, kb.kb_type, kb.parent_id, kb.is_public
                    FROM knowledge_bases kb
                    INNER JOIN tree t ON kb.parent_id = t.id
                )
                SELECT id, name, description, kb_type, parent_id, is_public
                FROM tree
                ORDER BY name
                "#,
            )
            .bind(kb_id)
            .fetch_all(pool)
            .await?
        }
        (Some(kb_id), false) => {
            sqlx::query_as(
                r#"
                WITH RECURSIVE tree AS (
                    SELECT id, name, description, kb_type, parent_id, is_public
                    FROM knowledge_bases
                    WHERE id = ? AND #ACCESS#
                    UNION ALL
                    SELECT kb.id, kb.name, kb.description, kb.kb_type, kb.parent_id, kb.is_public
                    FROM knowledge_bases kb
                    INNER JOIN tree t ON kb.parent_id = t.id
                    #ACCESS_WHERE#
                )
                SELECT id, name, description, kb_type, parent_id, is_public
                FROM tree
                ORDER BY name
                "#
                .replace("#ACCESS#", access_clause)
                .replace("#ACCESS_WHERE#", access_where)
                .as_str(),
            )
            .bind(kb_id)
            .bind(user_id)
            .bind(user_id)
            .bind(user_id)
            .bind(user_id)
            .fetch_all(pool)
            .await?
        }
        (None, true) => {
            sqlx::query_as(
                r#"
                WITH RECURSIVE tree AS (
                    SELECT id, name, description, kb_type, parent_id, is_public
                    FROM knowledge_bases
                    WHERE parent_id IS NULL
                    UNION ALL
                    SELECT kb.id, kb.name, kb.description, kb.kb_type, kb.parent_id, kb.is_public
                    FROM knowledge_bases kb
                    INNER JOIN tree t ON kb.parent_id = t.id
                )
                SELECT id, name, description, kb_type, parent_id, is_public
                FROM tree
                ORDER BY name
                "#,
            )
            .fetch_all(pool)
            .await?
        }
        (None, false) => {
            sqlx::query_as(
                r#"
                WITH RECURSIVE tree AS (
                    SELECT id, name, description, kb_type, parent_id, is_public
                    FROM knowledge_bases
                    WHERE parent_id IS NULL AND #ACCESS#
                    UNION ALL
                    SELECT kb.id, kb.name, kb.description, kb.kb_type, kb.parent_id, kb.is_public
                    FROM knowledge_bases kb
                    INNER JOIN tree t ON kb.parent_id = t.id
                    #ACCESS_WHERE#
                )
                SELECT id, name, description, kb_type, parent_id, is_public
                FROM tree
                ORDER BY name
                "#
                .replace("#ACCESS#", access_clause)
                .replace("#ACCESS_WHERE#", access_where)
                .as_str(),
            )
            .bind(user_id)
            .bind(user_id)
            .bind(user_id)
            .bind(user_id)
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows)
}

#[derive(Clone, Debug, sqlx::FromRow)]
struct TreeKnowledge {
    id: i64,
    name: String,
    description: String,
    kb_type: String,
    parent_id: Option<i64>,
    is_public: bool,
}

async fn load_tree_files_by_kb(
    pool: &SqlitePool, kb_ids: &[i64], user_id: &str, is_admin: bool,
) -> anyhow::Result<HashMap<i64, Vec<KnowledgeTreeFile>>> {
    if kb_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut qb =
        QueryBuilder::<Sqlite>::new("SELECT id, size, filename, meta, kb_id, is_public FROM files WHERE kb_id IN (");
    push_i64_list(&mut qb, kb_ids);
    qb.push(")");
    if !is_admin {
        qb.push(" AND (user_id = ").push_bind(user_id).push(" OR is_public = 1)");
    }
    qb.push(" ORDER BY kb_id, filename");

    let files: Vec<KnowledgeTreeFile> = qb.build_query_as().fetch_all(pool).await?;
    let mut files_by_kb = HashMap::<i64, Vec<KnowledgeTreeFile>>::new();
    for file in files {
        if let Some(kb_id) = file.kb_id {
            files_by_kb.entry(kb_id).or_default().push(file);
        }
    }
    Ok(files_by_kb)
}

fn build_tree_node(
    kb_id: i64, knowledges: &HashMap<i64, TreeKnowledge>, children_map: &HashMap<Option<i64>, Vec<i64>>,
    files_by_kb: &mut HashMap<i64, Vec<KnowledgeTreeFile>>,
) -> Option<KnowledgeTreeNode> {
    let kb = knowledges.get(&kb_id)?;
    let child_ids = children_map.get(&Some(kb_id)).cloned().unwrap_or_default();
    let mut children = Vec::with_capacity(child_ids.len());
    for child_id in child_ids {
        if let Some(child) = build_tree_node(child_id, knowledges, children_map, files_by_kb) {
            children.push(child);
        }
    }

    Some(KnowledgeTreeNode {
        id: kb.id,
        name: kb.name.clone(),
        description: kb.description.clone(),
        is_public: kb.is_public,
        kb_type: kb.kb_type.clone(),
        files: files_by_kb.remove(&kb_id).unwrap_or_default(),
        children,
    })
}

fn assemble_tree(
    knowledges: Vec<TreeKnowledge>, mut files_by_kb: HashMap<i64, Vec<KnowledgeTreeFile>>, root_kb_id: Option<i64>,
) -> Vec<KnowledgeTreeNode> {
    if knowledges.is_empty() {
        return Vec::new();
    }

    let mut knowledge_map = HashMap::<i64, TreeKnowledge>::with_capacity(knowledges.len());
    let mut children_map = HashMap::<Option<i64>, Vec<i64>>::new();
    for kb in knowledges {
        children_map.entry(kb.parent_id).or_default().push(kb.id);
        knowledge_map.insert(kb.id, kb);
    }

    for child_ids in children_map.values_mut() {
        child_ids.sort_by(|left_id, right_id| {
            let left_name = knowledge_map.get(left_id).map(|kb| kb.name.as_str()).unwrap_or_default();
            let right_name = knowledge_map.get(right_id).map(|kb| kb.name.as_str()).unwrap_or_default();
            left_name.cmp(right_name)
        });
    }

    let root_ids = match root_kb_id {
        Some(root_id) => {
            if knowledge_map.contains_key(&root_id) {
                vec![root_id]
            } else {
                Vec::new()
            }
        }
        None => children_map.get(&None).cloned().unwrap_or_default(),
    };

    let mut tree = Vec::with_capacity(root_ids.len());
    for root_id in root_ids {
        if let Some(node) = build_tree_node(root_id, &knowledge_map, &children_map, &mut files_by_kb) {
            tree.push(node);
        }
    }
    tree
}

/// 获取知识库树结构
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/knowledge_base/tree",
    operation_id = "knowledge_base_tree",
    tag = "knowledge_base",
    params(TreeQuery),
    responses(
        (status = 200, description = "成功返回知识库树结构", body = Vec<KnowledgeTreeNode>),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn tree(
    State(pool): State<SqlitePool>, Query(params): Query<TreeQuery>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<Vec<KnowledgeTreeNode>>> {
    let is_admin = auth_user.is_admin();
    let knowledges = load_tree_knowledges(&pool, params.kb_id, &auth_user.user_id, is_admin).await?;
    let kb_ids: Vec<i64> = knowledges.iter().map(|kb| kb.id).collect();
    let files_by_kb = load_tree_files_by_kb(&pool, &kb_ids, &auth_user.user_id, is_admin).await?;
    let tree = assemble_tree(knowledges, files_by_kb, params.kb_id);
    Ok(Json(tree))
}

#[derive(Serialize, ToSchema)]
pub struct ExportKbResponse {
    pub export_path: String,
    pub manifest: crate::export::ExportManifest,
}

#[derive(Deserialize, ToSchema)]
pub struct BatchExportKbRequest {
    pub kb_ids: Vec<i64>,
    #[serde(default)]
    pub include_children: bool,
}

/// 批量导出多个知识库
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/knowledge_base/export",
    operation_id = "knowledge_base_batch_export",
    tag = "knowledge_base",
    request_body = BatchExportKbRequest,
    responses(
        (status = 200, description = "成功导出知识库", body = ExportKbResponse),
        (status = 400, description = "请求参数错误"),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn batch_export_kb(
    State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>, Json(req): Json<BatchExportKbRequest>,
) -> ApiResult<Json<ExportKbResponse>> {
    let is_admin = auth_user.is_admin();

    if req.kb_ids.is_empty() {
        return Err(ApiError::BadRequest("kb_ids cannot be empty".to_string()));
    }

    // Verify all knowledge bases exist and user has access (viewer or above)
    let mut allowed_ids = Vec::new();
    for kb_id in &req.kb_ids {
        let perm = get_kb_permission(&pool, *kb_id, &auth_user.user_id, is_admin).await;
        if perm.is_some() {
            allowed_ids.push(*kb_id);
        }
    }
    if allowed_ids.is_empty() {
        return Err(ApiError::NotFound("No knowledge bases found or permission denied.".to_string()));
    }

    if allowed_ids.len() != req.kb_ids.len() {
        let missing: Vec<i64> = req.kb_ids.iter().filter(|id| !allowed_ids.contains(id)).copied().collect();
        warn!("User {} tried to export inaccessible KBs: {:?}", auth_user.user_id, missing);
    }

    let export_path = crate::export::export_knowledge_bases(&pool, &allowed_ids, req.include_children)
        .await
        .map_err(|e| ApiError::Internal(format!("Export failed: {}", e)))?;

    // Read manifest
    let manifest_path = std::path::Path::new(&export_path).join("manifest.json");
    let manifest_json = tokio::fs::read_to_string(&manifest_path)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to read manifest: {}", e)))?;
    let manifest: crate::export::ExportManifest = serde_json::from_str(&manifest_json)
        .map_err(|e| ApiError::Internal(format!("Failed to parse manifest: {}", e)))?;

    Ok(Json(ExportKbResponse { export_path, manifest }))
}

// ---------------------------------------------------------------------------
// KB Permission management endpoints
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, sqlx::FromRow, ToSchema)]
pub struct KbPermissionItem {
    pub user_id: String,
    pub permission: String,
    pub created_at: i64,
}

#[derive(Deserialize, Clone, Debug, ToSchema)]
pub struct KbPermissionCreateReq {
    pub user_id: String,
    pub permission: String,
}

/// 获取知识库权限列表
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/knowledge_base/{id}/permissions",
    operation_id = "kb_permission_list",
    tag = "knowledge_base",
    params(
        ("id" = i64, Path, description = "知识库 ID")
    ),
    responses(
        (status = 200, description = "成功返回权限列表", body = Vec<KbPermissionItem>),
        (status = 403, description = "无权限"),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn list_permissions(
    Path(id): Path<i64>, State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<Vec<KbPermissionItem>>> {
    let is_admin = auth_user.is_admin();
    let user_perm = get_kb_permission(&pool, id, &auth_user.user_id, is_admin).await;
    if !meets_requirement(user_perm.as_deref(), "admin") {
        return Err(ApiError::Forbidden("Permission denied. Admin role required.".to_string()));
    }

    let rows: Vec<KbPermissionItem> = sqlx::query_as(
        "SELECT user_id, permission, created_at FROM kb_permissions WHERE kb_id = ? ORDER BY created_at",
    )
    .bind(id)
    .fetch_all(&pool)
    .await?;

    Ok(Json(rows))
}

/// 添加或更新知识库权限
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/knowledge_base/{id}/permissions",
    operation_id = "kb_permission_add",
    tag = "knowledge_base",
    params(
        ("id" = i64, Path, description = "知识库 ID")
    ),
    request_body = KbPermissionCreateReq,
    responses(
        (status = 200, description = "成功添加/更新权限", body = KbPermissionItem),
        (status = 400, description = "请求参数错误"),
        (status = 403, description = "无权限"),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn add_permission(
    Path(id): Path<i64>, State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<KbPermissionCreateReq>,
) -> ApiResult<Json<KbPermissionItem>> {
    let is_admin = auth_user.is_admin();
    let user_perm = get_kb_permission(&pool, id, &auth_user.user_id, is_admin).await;
    if !meets_requirement(user_perm.as_deref(), "admin") {
        return Err(ApiError::Forbidden("Permission denied. Admin role required.".to_string()));
    }

    // Validate permission value
    let perm = req.permission.trim().to_lowercase();
    if !matches!(perm.as_str(), "viewer" | "editor" | "admin") {
        return Err(ApiError::BadRequest("Invalid permission. Use 'viewer', 'editor', or 'admin'.".to_string()));
    }

    // Prevent adding permission for the owner (owner already has admin implicitly)
    let owner: Option<String> =
        sqlx::query_scalar("SELECT user_id FROM knowledge_bases WHERE id = ?").bind(id).fetch_optional(&pool).await?;
    if owner.as_deref() == Some(&req.user_id) {
        return Err(ApiError::BadRequest("Cannot set permission for the owner.".to_string()));
    }

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;

    sqlx::query(
        "INSERT INTO kb_permissions (kb_id, user_id, permission, created_at, updated_at) VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(kb_id, user_id) DO UPDATE SET permission = excluded.permission, updated_at = excluded.updated_at",
    )
    .bind(id)
    .bind(&req.user_id)
    .bind(&perm)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await?;

    Ok(Json(KbPermissionItem { user_id: req.user_id, permission: perm, created_at: now }))
}

/// 删除知识库权限
#[utoipa::path(
    delete,
    path = "/api/v1/knowledge/knowledge_base/{id}/permissions/{user_id}",
    operation_id = "kb_permission_remove",
    tag = "knowledge_base",
    params(
        ("id" = i64, Path, description = "知识库 ID"),
        ("user_id" = String, Path, description = "用户 ID")
    ),
    responses(
        (status = 200, description = "成功删除权限"),
        (status = 403, description = "无权限"),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn remove_permission(
    Path((id, target_user_id)): Path<(i64, String)>, State(pool): State<SqlitePool>,
    Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<()> {
    let is_admin = auth_user.is_admin();
    let user_perm = get_kb_permission(&pool, id, &auth_user.user_id, is_admin).await;
    if !meets_requirement(user_perm.as_deref(), "admin") {
        return Err(ApiError::Forbidden("Permission denied. Admin role required.".to_string()));
    }

    let result = sqlx::query("DELETE FROM kb_permissions WHERE kb_id = ? AND user_id = ?")
        .bind(id)
        .bind(target_user_id)
        .execute(&pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Permission not found.".to_string()));
    }

    Ok(())
}
