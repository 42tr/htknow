use std::collections::HashMap;

use axum::{
    Extension, extract::{Path, Query, State}, response::Json
};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};
use tokio::fs;
use utoipa::{IntoParams, ToSchema};

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

#[derive(Serialize, Deserialize, Clone, Debug, sqlx::FromRow, ToSchema)]
pub struct Knowledge {
    pub id: i64,
    pub user_id: String,
    pub name: String,
    pub description: String,
    pub kb_type: String,
    pub parent_id: Option<i64>,
    pub is_public: i32,
}

#[derive(Serialize, Deserialize, Clone, Debug, ToSchema)]
pub struct KnowledgeResponse {
    pub id: i64,
    pub user_id: String,
    pub name: String,
    pub description: String,
    pub kb_type: String,
    pub parent_id: Option<i64>,
    pub is_public: i32,
    pub file_count: i64,
    pub children_kb_count: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, ToSchema)]
pub struct KnowledgeDetailResponse {
    pub id: i64,
    pub user_id: String,
    pub name: String,
    pub description: String,
    pub kb_type: String,
    pub parent_id: Option<i64>,
    pub is_public: i32,
    pub children_kbs: Vec<Knowledge>,
    pub files: Vec<super::file::File>,
    pub path: Vec<Knowledge>, // For breadcrumbs
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

/// 获取知识库列表
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/knowledge_base/",
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
    // Determine pagination: default size 10, default page 1
    let size = params.size.unwrap_or(10).max(1);
    let page = params.page.unwrap_or(1).max(1);
    let limit = size;
    let offset = (page - 1) * size;

    // Start building the query
    let mut qb = QueryBuilder::<Sqlite>::new(
        "SELECT id, user_id, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE 1=1 ",
    );
    qb.push(" AND user_id = ").push_bind(auth_user.user_id);

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
    let (file_counts_res, children_counts_res) =
        tokio::join!(get_file_counts(&pool, &knowledge_ids), get_children_kb_counts(&pool, &knowledge_ids));
    let file_counts = file_counts_res?;
    let children_counts = children_counts_res?;

    let knowledge_responses = knowledges
        .into_iter()
        .map(|kb| KnowledgeResponse {
            id: kb.id,
            user_id: kb.user_id.clone(),
            name: kb.name.clone(),
            description: kb.description.clone(),
            kb_type: kb.kb_type.clone(),
            parent_id: kb.parent_id,
            is_public: kb.is_public,
            file_count: *file_counts.get(&kb.id).unwrap_or(&0),
            children_kb_count: *children_counts.get(&kb.id).unwrap_or(&0),
        })
        .collect();

    Ok(Json(knowledge_responses))
}

async fn get_children_kb_counts(pool: &SqlitePool, knowledge_ids: &[i64]) -> anyhow::Result<HashMap<i64, i64>> {
    if knowledge_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut qb = QueryBuilder::new("SELECT parent_id, COUNT(*) AS cnt FROM knowledge_bases WHERE parent_id IN (");
    let mut separated = qb.separated(", ");
    for id in knowledge_ids {
        separated.push_bind(id);
    }
    qb.push(") GROUP BY parent_id");

    let rows = qb.build().fetch_all(pool).await?;

    let children_counts = rows
        .into_iter()
        .filter_map(|row| {
            // Use get an Option<i64> to be safe, though it should not be None based on the WHERE clause.
            let parent_id: Option<i64> = row.get("parent_id");
            let cnt: i64 = row.get("cnt");
            parent_id.map(|pid| (pid, cnt))
        })
        .collect();

    Ok(children_counts)
}

async fn get_file_counts(pool: &SqlitePool, knowledge_ids: &[i64]) -> anyhow::Result<HashMap<i64, i64>> {
    if knowledge_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut qb = QueryBuilder::new("SELECT kb_id, COUNT(*) AS cnt FROM files WHERE kb_id IN (");

    let mut separated = qb.separated(", ");

    for id in knowledge_ids {
        separated.push_bind(id);
    }

    qb.push(") GROUP BY kb_id");

    let rows = qb.build().fetch_all(pool).await?;

    let file_counts = rows
        .into_iter()
        .filter_map(|row| {
            // Use filter_map for safety
            let kb_id: Option<i64> = row.get("kb_id"); // Get as Option
            let cnt: i64 = row.get("cnt");
            kb_id.map(|id| (id, cnt)) // Discard if kb_id is NULL
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
}

/// 创建知识库
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/knowledge_base/",
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
    let query = "INSERT INTO knowledge_bases (user_id, name, description, kb_type, parent_id, is_public) VALUES (?, ?, ?, ?, ?, ?)";
    let id = sqlx::query(query)
        .bind(auth_user.user_id)
        .bind(knowledge.name)
        .bind(knowledge.description.clone())
        .bind(kb_type)
        .bind(knowledge.parent_id)
        .bind(is_public)
        .execute(&pool)
        .await?
        .last_insert_rowid();
    let kb = sqlx::query_as(
        "SELECT id, user_id, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&pool)
    .await?;
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
}

/// 更新知识库
#[utoipa::path(
    put,
    path = "/api/v1/knowledge/knowledge_base/{id}",
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
    // Prevent moving a knowledge base into itself.
    if let Some(Some(parent_id)) = knowledge.parent_id {
        if parent_id == id {
            return Err(crate::api::error::ApiError::BadRequest(
                "Cannot move a knowledge base into itself.".to_string(),
            ));
        }
        // A full descendant check would be needed for production to prevent moving a KB into its own child.
        // This requires a recursive query and is omitted for this iteration.
    }

    let mut qb = QueryBuilder::<Sqlite>::new("UPDATE knowledge_bases SET ");
    let mut separated = qb.separated(", ");
    let mut has_update = false;

    if let Some(name) = knowledge.name {
        separated.push("name = ");
        separated.push_bind(name);
        has_update = true;
    }
    if let Some(description) = knowledge.description {
        separated.push("description = ");
        separated.push_bind(description);
        has_update = true;
    }
    if let Some(kb_type) = knowledge.kb_type {
        let kb_type = normalize_kb_type(Some(kb_type))?;
        separated.push("kb_type = ");
        separated.push_bind(kb_type);
        has_update = true;
    }
    // With double_option, this correctly distinguishes "not present" from "present and null"
    if let Some(parent_id) = knowledge.parent_id {
        separated.push("parent_id = ");
        separated.push_bind(parent_id); // This binds Option<i64> which sqlx handles (None becomes NULL)
        has_update = true;
    }
    if let Some(is_public) = knowledge.is_public {
        separated.push("is_public = ");
        separated.push_bind(if is_public { 1 } else { 0 });
        has_update = true;
    }

    if !has_update {
        // If nothing is being updated, just return the current state of kb, ensuring it exists and belongs to user.
        let kb = sqlx::query_as(
            "SELECT id, user_id, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE id = ? AND user_id = ?",
        )
        .bind(id)
        .bind(auth_user.user_id)
        .fetch_one(&pool)
        .await?;
        return Ok(Json(kb));
    }

    qb.push(" WHERE id = ");
    qb.push_bind(id);
    qb.push(" AND user_id = ");
    qb.push_bind(auth_user.user_id.clone());

    let result = qb.build().execute(&pool).await?;

    if result.rows_affected() == 0 {
        return Err(crate::api::error::ApiError::NotFound(
            "Knowledge base not found or permission denied.".to_string(),
        ));
    }

    let kb = sqlx::query_as(
        "SELECT id, user_id, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(auth_user.user_id)
    .fetch_one(&pool)
    .await?;
    Ok(Json(kb))
}

/// 获取知识库详情
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/knowledge_base/{id}",
    tag = "knowledge_base",
    params(
        ("id" = i64, Path, description = "知识库 ID")
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
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<KnowledgeDetailResponse>> {
    // 1. Fetch the main knowledge base and verify ownership
    let main_kb: Knowledge = sqlx::query_as(
        "SELECT id, user_id, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(auth_user.user_id.clone())
    .fetch_one(&pool)
    .await?;

    // 2. Fetch children KBs and files in parallel
    let (children_kbs_res, files_res) = tokio::join!(
        // Fetch children KBs
        sqlx::query_as("SELECT id, user_id, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE parent_id = ? AND user_id = ? ORDER BY name")
            .bind(id)
            .bind(auth_user.user_id.clone())
            .fetch_all(&pool),
        // Fetch files in this KB
        sqlx::query_as("SELECT * FROM files WHERE kb_id = ? ORDER BY filename")
            .bind(id)
            .fetch_all(&pool)
    );
    let children_kbs: Vec<Knowledge> = children_kbs_res?;
    // The File struct from file.rs might not have a user_id check, but it's implicitly checked by kb_id belonging to the user.
    let files: Vec<super::file::File> = files_res?;

    // 3. Fetch the breadcrumb path
    let mut path = Vec::new();
    let mut current_parent_id = main_kb.parent_id;
    while let Some(parent_id) = current_parent_id {
        // In a high-depth scenario, this could be slow. A recursive CTE would be faster.
        // But for typical UI breadcrumbs, this iterative approach is simpler and often sufficient.
        let parent_kb: Knowledge = sqlx::query_as(
            "SELECT id, user_id, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE id = ? AND user_id = ?",
        )
        .bind(parent_id)
        .bind(auth_user.user_id.clone())
        .fetch_one(&pool)
        .await?;
        current_parent_id = parent_kb.parent_id;
        path.push(parent_kb);
    }
    path.reverse(); // Reverse to get the correct order from root to parent

    // 4. Construct the response
    let response = KnowledgeDetailResponse {
        id: main_kb.id,
        user_id: main_kb.user_id,
        name: main_kb.name,
        description: main_kb.description,
        kb_type: main_kb.kb_type,
        parent_id: main_kb.parent_id,
        is_public: main_kb.is_public,
        children_kbs,
        files,
        path,
    };

    Ok(Json(response))
}

/// 删除知识库
#[utoipa::path(
    delete,
    path = "/api/v1/knowledge/knowledge_base/{id}",
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
    // 1. Get the list of all KB IDs to delete (the given one and all its descendants)
    // Use a recursive CTE to find all descendant knowledge bases, ensuring the root belongs to the user.
    let all_kb_ids: Vec<i64> = sqlx::query_scalar(
        r#"
        WITH RECURSIVE kb_hierarchy AS (
            SELECT id FROM knowledge_bases WHERE id = ? AND user_id = ?
            UNION ALL
            SELECT kb.id FROM knowledge_bases kb
            INNER JOIN kb_hierarchy kh ON kb.parent_id = kh.id
        )
        SELECT id FROM kb_hierarchy;
        "#,
    )
    .bind(id)
    .bind(auth_user.user_id.clone())
    .fetch_all(&pool)
    .await?;

    if all_kb_ids.is_empty() {
        // This means the initial ID was not found or didn't belong to the user
        return Err(crate::api::error::ApiError::NotFound(
            "Knowledge base not found or permission denied.".to_string(),
        ));
    }

    // 2. Delete all associated data for these KBs

    // Find all files in all these KBs
    let mut files_qb = QueryBuilder::new("SELECT * FROM files WHERE kb_id IN (");
    let mut files_separated = files_qb.separated(", ");
    for kb_id in &all_kb_ids {
        files_separated.push_bind(kb_id);
    }
    files_qb.push(")");
    let files: Vec<super::file::File> = files_qb.build_query_as().fetch_all(&pool).await?;

    if !files.is_empty() {
        let file_ids: Vec<i64> = files.iter().map(|f| f.id).collect();

        // Delete slices for all found files
        let mut slices_qb = QueryBuilder::new("DELETE FROM slices WHERE file_id IN (");
        let mut slices_separated = slices_qb.separated(", ");
        for file_id in &file_ids {
            slices_separated.push_bind(file_id);
        }
        slices_qb.push(")");
        slices_qb.build().execute(&pool).await?;

        // Delete physical files on disk
        for file in &files {
            if let Err(e) = fs::remove_file(&file.path).await {
                log::warn!("Failed to delete file {}: {}", &file.path, e);
            }
        }

        // Delete the file records themselves
        let mut del_files_qb = QueryBuilder::new("DELETE FROM files WHERE id IN (");
        let mut del_files_separated = del_files_qb.separated(", ");
        for file_id in &file_ids {
            del_files_separated.push_bind(file_id);
        }
        del_files_qb.push(")");
        del_files_qb.build().execute(&pool).await?;
    }

    // Delete from search engine for all KBs
    for kb_id in &all_kb_ids {
        search_engine.delete(None, Some(*kb_id)).await?;
    }

    // 3. Delete the top-level KB. The ON DELETE CASCADE will handle the rest in knowledge_bases table.
    let result = sqlx::query("DELETE FROM knowledge_bases WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(auth_user.user_id)
        .execute(&pool)
        .await?;

    if result.rows_affected() == 0 {
        // This case should theoretically be caught by the descendant check, but as a safeguard:
        return Err(crate::api::error::ApiError::NotFound(
            "Knowledge base not found or permission denied.".to_string(),
        ));
    }

    Ok(())
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateKbPublicReq {
    pub is_public: bool,
}

/// 更新知识库公开/私有状态
#[utoipa::path(
    put,
    path = "/api/v1/knowledge/knowledge_base/{id}/public",
    tag = "knowledge_base",
    params(
        ("id" = i64, Path, description = "知识库 ID")
    ),
    request_body = UpdateKbPublicReq,
    responses(
        (status = 200, description = "成功更新公开/私有状态", body = Knowledge),
        (status = 400, description = "请求参数错误"),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn update_public(
    Path(id): Path<i64>, State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<UpdateKbPublicReq>,
) -> ApiResult<Json<Knowledge>> {
    let is_public = if req.is_public { 1 } else { 0 };
    let sql = "UPDATE knowledge_bases SET is_public = ?, updated_at = ? WHERE id = ? AND user_id = ?";
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    sqlx::query(sql).bind(is_public).bind(now).bind(id).bind(&auth_user.user_id).execute(&pool).await?;
    let kb = sqlx::query_as(
        "SELECT id, user_id, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(&auth_user.user_id)
    .fetch_one(&pool)
    .await?;
    Ok(Json(kb))
}
