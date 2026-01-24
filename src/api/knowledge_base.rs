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
    pub user_name: String,
    pub name: String,
    pub description: String,
    pub kb_type: String,
    pub parent_id: Option<i64>,
    pub is_public: bool,
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
    pub file_count: i64,
    pub children_kb_count: i64,
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
    pub file_count: i64,
    pub children_kb_count: i64,
    pub children_kbs: Vec<KnowledgeResponse>,
    pub files: Vec<super::file::File>,
    pub path: Vec<Knowledge>, // For breadcrumbs
}

#[derive(Serialize, Deserialize, Clone, Debug, ToSchema)]
pub struct KnowledgeTreeNode {
    pub id: i64,
    pub user_id: String,
    pub user_name: String,
    pub name: String,
    pub description: String,
    pub kb_type: String,
    pub parent_id: Option<i64>,
    pub is_public: bool,
    pub files: Vec<super::file::File>,
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
    // Determine pagination: default size 10, default page 1
    let size = params.size.unwrap_or(10).max(1);
    let page = params.page.unwrap_or(1).max(1);
    let limit = size;
    let offset = (page - 1) * size;

    // Start building the query
    let mut qb = QueryBuilder::<Sqlite>::new(
        "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE 1=1 ",
    );
    qb.push(" AND (user_id = ").push_bind(auth_user.user_id.clone()).push(" OR is_public = 1)");

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
        get_file_counts(&pool, &knowledge_ids, &auth_user.user_id),
        get_children_kb_counts(&pool, &knowledge_ids, &auth_user.user_id)
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
            file_count: *file_counts.get(&kb.id).unwrap_or(&0),
            children_kb_count: *children_counts.get(&kb.id).unwrap_or(&0),
        })
        .collect();

    Ok(Json(knowledge_responses))
}

async fn get_children_kb_counts(
    pool: &SqlitePool, knowledge_ids: &[i64], user_id: &str,
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
    qb.push(") AND (user_id = ");
    qb.push_bind(user_id);
    qb.push(" OR is_public = 1) UNION ALL SELECT d.root_id, kb.id FROM knowledge_bases kb ");
    qb.push("JOIN descendants d ON kb.parent_id = d.kb_id WHERE (kb.user_id = ");
    qb.push_bind(user_id);
    qb.push(" OR kb.is_public = 1)) ");
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

async fn get_file_counts(pool: &SqlitePool, knowledge_ids: &[i64], user_id: &str) -> anyhow::Result<HashMap<i64, i64>> {
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
    qb.push(") AND (user_id = ");
    qb.push_bind(user_id);
    qb.push(" OR is_public = 1) UNION ALL SELECT d.root_id, kb.id FROM knowledge_bases kb ");
    qb.push("JOIN descendants d ON kb.parent_id = d.kb_id WHERE (kb.user_id = ");
    qb.push_bind(user_id);
    qb.push(" OR kb.is_public = 1)) ");
    qb.push("SELECT d.root_id, COUNT(f.id) AS cnt FROM descendants d ");
    qb.push("LEFT JOIN files f ON f.kb_id = d.kb_id AND (f.user_id = ");
    qb.push_bind(user_id);
    qb.push(" OR f.is_public = 1) GROUP BY d.root_id");

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
    let query = "INSERT INTO knowledge_bases (user_id, user_name, name, description, kb_type, parent_id, is_public) VALUES (?, ?, ?, ?, ?, ?, ?)";
    let id = sqlx::query(query)
        .bind(auth_user.user_id)
        .bind(auth_user.user_name)
        .bind(knowledge.name)
        .bind(knowledge.description.clone())
        .bind(kb_type)
        .bind(knowledge.parent_id)
        .bind(is_public)
        .execute(&pool)
        .await?
        .last_insert_rowid();
    let kb = sqlx::query_as(
        "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE id = ?",
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

    if !has_update {
        // If nothing is being updated, just return the current state of kb, ensuring it exists and belongs to user.
        let kb = sqlx::query_as(
            "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE id = ? AND user_id = ?",
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
        "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE id = ? AND user_id = ?",
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
    // 1. Fetch the main knowledge base and verify ownership
    let main_kb: Knowledge = sqlx::query_as(
        "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE id = ? AND (user_id = ? OR is_public = 1)",
    )
    .bind(id)
    .bind(auth_user.user_id.clone())
    .fetch_one(&pool)
    .await?;

    // 2. Fetch children KBs and files in parallel
    let children_future = sqlx::query_as(
        "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public \
         FROM knowledge_bases WHERE parent_id = ? AND (user_id = ? OR is_public = 1) ORDER BY name",
    )
    .bind(id)
    .bind(auth_user.user_id.clone())
    .fetch_all(&pool);

    let files_future = async {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT * FROM files WHERE kb_id = ");
        qb.push_bind(id);
        qb.push(" AND (user_id = ").push_bind(auth_user.user_id.clone()).push(" OR is_public = 1)");
        if let Some(filename) = query.filename.as_deref() {
            qb.push(" AND filename LIKE ").push_bind(format!("%{}%", filename));
        }
        qb.push(" ORDER BY filename");
        qb.build_query_as::<super::file::File>().fetch_all(&pool).await
    };

    let (children_kbs_res, files_res) = tokio::join!(children_future, files_future);
    let children_kbs: Vec<Knowledge> = children_kbs_res?;
    let mut files: Vec<super::file::File> = files_res?;
    let mut count_ids = Vec::with_capacity(children_kbs.len() + 1);
    count_ids.push(id);
    count_ids.extend(children_kbs.iter().map(|kb| kb.id));
    let (file_counts_res, children_counts_res) = tokio::join!(
        get_file_counts(&pool, &count_ids, &auth_user.user_id),
        get_children_kb_counts(&pool, &count_ids, &auth_user.user_id)
    );
    let file_counts = file_counts_res?;
    let children_counts = children_counts_res?;
    let file_count = *file_counts.get(&id).unwrap_or(&0);
    let children_kb_count = *children_counts.get(&id).unwrap_or(&0);
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
            file_count: *file_counts.get(&kb.id).unwrap_or(&0),
            children_kb_count: *children_counts.get(&kb.id).unwrap_or(&0),
        })
        .collect();

    if let Some(tag) = &query.tag {
        files.retain(|file| {
            if let Ok(tags) = serde_json::from_str::<Vec<String>>(&file.tags) { tags.contains(tag) } else { false }
        });
    }

    // 3. Fetch the breadcrumb path
    let mut path = Vec::new();
    let mut current_parent_id = main_kb.parent_id;
    while let Some(parent_id) = current_parent_id {
        // In a high-depth scenario, this could be slow. A recursive CTE would be faster.
        // But for typical UI breadcrumbs, this iterative approach is simpler and often sufficient.
        let parent_kb: Knowledge = sqlx::query_as(
            "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE id = ? AND (user_id = ? OR is_public = 1)",
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
        user_name: main_kb.user_name,
        name: main_kb.name,
        description: main_kb.description,
        kb_type: main_kb.kb_type,
        parent_id: main_kb.parent_id,
        is_public: main_kb.is_public,
        file_count,
        children_kb_count,
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
    // 1. Get the list of all KB IDs to delete (the given one and all its descendants)
    // Use a recursive CTE to find all descendant knowledge bases, ensuring the root belongs to the user.
    let all_kb_ids: Vec<i64> = sqlx::query_scalar(
        r#"
        WITH RECURSIVE kb_hierarchy AS (
            SELECT id FROM knowledge_bases WHERE id = ? AND (user_id = ? OR is_public = 1)
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
    let result = sqlx::query("DELETE FROM knowledge_bases WHERE id = ? AND (user_id = ? OR is_public = 1)")
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

#[derive(Serialize, ToSchema)]
pub struct ReparseKnowledgeBaseResponse {
    pub kb_count: i64,
    pub file_count: i64,
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
    let analysis_kb_ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM knowledge_bases WHERE user_id = ? AND kb_type != ?")
            .bind(auth_user.user_id.clone())
            .bind(KB_TYPE_STORAGE)
            .fetch_all(&pool)
            .await?;

    let unassigned_file_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM files WHERE user_id = ? AND kb_id IS NULL")
        .bind(auth_user.user_id.clone())
        .fetch_all(&pool)
        .await?;

    let mut kb_file_ids: Vec<i64> = Vec::new();
    if !analysis_kb_ids.is_empty() {
        // 获取这些知识库下的文件
        let mut file_qb = QueryBuilder::<Sqlite>::new("SELECT id FROM files WHERE user_id = ");
        file_qb.push_bind(auth_user.user_id.clone());
        file_qb.push(" AND kb_id IN (");
        let mut file_sep = file_qb.separated(", ");
        for kb_id in &analysis_kb_ids {
            file_sep.push_bind(kb_id);
        }
        file_qb.push(")");
        kb_file_ids = file_qb.build_query_scalar().fetch_all(&pool).await?;
    }

    let mut file_ids = kb_file_ids.clone();
    file_ids.extend(unassigned_file_ids.iter().copied());
    if analysis_kb_ids.is_empty() && unassigned_file_ids.is_empty() {
        return Ok(Json(ReparseKnowledgeBaseResponse { kb_count: 0, file_count: 0 }));
    }

    // 清理搜索索引
    for kb_id in &analysis_kb_ids {
        search_engine.delete(None, Some(*kb_id)).await?;
    }
    for file_id in &unassigned_file_ids {
        search_engine.delete(Some(*file_id), None).await?;
    }

    // 清理知识图谱数据（节点会级联删除边和提及）
    if !analysis_kb_ids.is_empty() {
        let mut del_nodes_qb = QueryBuilder::<Sqlite>::new("DELETE FROM graph_nodes WHERE kb_id IN (");
        let mut del_nodes_sep = del_nodes_qb.separated(", ");
        for kb_id in &analysis_kb_ids {
            del_nodes_sep.push_bind(kb_id);
        }
        del_nodes_qb.push(")");
        del_nodes_qb.build().execute(&pool).await?;

        let mut del_snapshots_qb = QueryBuilder::<Sqlite>::new("DELETE FROM graph_snapshots WHERE kb_id IN (");
        let mut del_snapshots_sep = del_snapshots_qb.separated(", ");
        for kb_id in &analysis_kb_ids {
            del_snapshots_sep.push_bind(kb_id);
        }
        del_snapshots_qb.push(")");
        del_snapshots_qb.build().execute(&pool).await?;
    }
    sqlx::query("DELETE FROM graph_nodes WHERE kb_id IS NULL").execute(&pool).await?;
    sqlx::query("DELETE FROM graph_snapshots WHERE kb_id IS NULL").execute(&pool).await?;

    if !file_ids.is_empty() {
        let mut del_slices_qb = QueryBuilder::<Sqlite>::new("DELETE FROM slices WHERE file_id IN (");
        let mut del_slices_sep = del_slices_qb.separated(", ");
        for file_id in &file_ids {
            del_slices_sep.push_bind(file_id);
        }
        del_slices_qb.push(")");
        del_slices_qb.build().execute(&pool).await?;

        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let mut update_qb = QueryBuilder::<Sqlite>::new("UPDATE files SET status = 0, log = '', updated_at = ");
        update_qb.push_bind(now);
        update_qb.push(" WHERE id IN (");
        let mut update_sep = update_qb.separated(", ");
        for file_id in &file_ids {
            update_sep.push_bind(file_id);
        }
        update_qb.push(")");
        update_qb.build().execute(&pool).await?;
    }

    Ok(Json(ReparseKnowledgeBaseResponse { kb_count: analysis_kb_ids.len() as i64, file_count: file_ids.len() as i64 }))
}

async fn build_tree_recursive(
    pool: &SqlitePool, parent_id: Option<i64>, user_id: &str,
) -> anyhow::Result<Vec<KnowledgeTreeNode>> {
    let children: Vec<Knowledge> = sqlx::query_as(
        "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE parent_id IS NOT DISTINCT FROM ? AND (user_id = ? OR is_public = 1) ORDER BY name",
    )
    .bind(parent_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let mut tree_nodes = Vec::new();
    for kb in children {
        let (sub_children, files) = tokio::join!(
            async { Box::pin(build_tree_recursive(pool, Some(kb.id), user_id)).await },
            get_files_for_kb(pool, kb.id, user_id),
        );
        let sub_children = sub_children?;
        let files = files?;
        tree_nodes.push(KnowledgeTreeNode {
            id: kb.id,
            user_id: kb.user_id,
            user_name: kb.user_name,
            name: kb.name,
            description: kb.description,
            kb_type: kb.kb_type,
            parent_id: kb.parent_id,
            is_public: kb.is_public,
            files,
            children: sub_children,
        });
    }
    Ok(tree_nodes)
}

async fn build_subtree_recursive(
    pool: &SqlitePool, kb_id: i64, user_id: &str,
) -> anyhow::Result<Option<KnowledgeTreeNode>> {
    let kb: Option<Knowledge> = sqlx::query_as(
        "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public FROM knowledge_bases WHERE id = ? AND (user_id = ? OR is_public = 1)",
    )
    .bind(kb_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    match kb {
        Some(kb_info) => {
            let (children, files) = tokio::join!(
                async { Box::pin(build_tree_recursive(pool, Some(kb_info.id), user_id)).await },
                get_files_for_kb(pool, kb_info.id, user_id),
            );
            let children = children?;
            let files = files?;
            Ok(Some(KnowledgeTreeNode {
                id: kb_info.id,
                user_id: kb_info.user_id,
                user_name: kb_info.user_name,
                name: kb_info.name,
                description: kb_info.description,
                kb_type: kb_info.kb_type,
                parent_id: kb_info.parent_id,
                is_public: kb_info.is_public,
                files,
                children,
            }))
        }
        None => Ok(None),
    }
}

async fn get_files_for_kb(pool: &SqlitePool, kb_id: i64, user_id: &str) -> anyhow::Result<Vec<super::file::File>> {
    let files: Vec<super::file::File> =
        sqlx::query_as("SELECT * FROM files WHERE kb_id = ? AND (user_id = ? OR is_public = 1) ORDER BY filename")
            .bind(kb_id)
            .bind(user_id)
            .fetch_all(pool)
            .await?;
    Ok(files)
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
    let tree = match params.kb_id {
        Some(kb_id) => {
            let subtree = build_subtree_recursive(&pool, kb_id, &auth_user.user_id).await?;
            match subtree {
                Some(node) => vec![node],
                None => vec![],
            }
        }
        None => build_tree_recursive(&pool, None, &auth_user.user_id).await?,
    };
    Ok(Json(tree))
}
