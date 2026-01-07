use std::collections::HashMap;

use axum::{
    Extension, extract::{Path, Query, State}, response::Json
};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};
use tokio::fs;
use utoipa::{IntoParams, ToSchema};

use crate::{AuthUser, api::error::ApiResult, search::SearchEngine};

#[derive(Serialize, Deserialize, Clone, Debug, sqlx::FromRow, ToSchema)]
pub struct Knowledge {
    pub id: i64,
    pub user_id: String,
    pub name: String,
    pub description: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, sqlx::FromRow, ToSchema)]
pub struct KnowledgeResponse {
    pub id: i64,
    pub user_id: String,
    pub name: String,
    pub description: String,
    pub file_count: i64,
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
    let mut qb = QueryBuilder::<Sqlite>::new("SELECT id, user_id, name, description FROM knowledge_bases WHERE 1=1 ");
    qb.push(" AND user_id = ").push_bind(auth_user.user_id);

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
    let knowledge_ids = knowledges.iter().map(|kb| kb.id).collect::<Vec<i64>>();
    let count_map = get_file_counts(&pool, &knowledge_ids).await?;
    let knowledge_responses = knowledges
        .into_iter()
        .map(|kb| KnowledgeResponse {
            id: kb.id,
            user_id: kb.user_id,
            name: kb.name,
            description: kb.description,
            file_count: *count_map.get(&kb.id).unwrap_or(&0),
        })
        .collect();

    Ok(Json(knowledge_responses))
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
        .map(|row| {
            let kb_id: i64 = row.get("kb_id");
            let cnt: i64 = row.get("cnt");
            (kb_id, cnt)
        })
        .collect();

    Ok(file_counts)
}

#[derive(Deserialize, ToSchema)]
pub struct KnowledgeCreateReq {
    pub name: String,
    pub description: String,
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
    let query = "INSERT INTO knowledge_bases (user_id, name, description) VALUES (?, ?, ?)";
    let id = sqlx::query(query)
        .bind(auth_user.user_id)
        .bind(knowledge.name)
        .bind(knowledge.description.clone())
        .execute(&pool)
        .await?
        .last_insert_rowid();
    let kb = sqlx::query_as("SELECT id, user_id, name, description FROM knowledge_bases WHERE id = ?")
        .bind(id)
        .fetch_one(&pool)
        .await?;
    Ok(Json(kb))
}

#[derive(Deserialize, ToSchema)]
pub struct KnowledgeUpdateReq {
    pub name: Option<String>,
    pub description: Option<String>,
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
    let query = "UPDATE knowledge_bases SET name = ?, description = ? WHERE id = ? AND user_id = ?";
    sqlx::query(query)
        .bind(knowledge.name)
        .bind(knowledge.description)
        .bind(id)
        .bind(auth_user.user_id)
        .execute(&pool)
        .await?;
    let kb = sqlx::query_as("SELECT id, user_id, name, description FROM knowledge_bases WHERE id = ?")
        .bind(id)
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
        (status = 200, description = "成功返回知识库详情", body = Knowledge),
        (status = 404, description = "知识库不存在")
    )
)]
pub async fn get(State(pool): State<SqlitePool>, Path(id): Path<i64>) -> ApiResult<Json<Knowledge>> {
    let query = "SELECT id, user_id, name, description FROM knowledge_bases WHERE id = ?";
    let knowledge = sqlx::query_as(query).bind(id).fetch_one(&pool).await?;
    Ok(Json(knowledge))
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
        (status = 404, description = "知识库不存在")
    )
)]
pub async fn delete(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>, Path(id): Path<i64>,
) -> ApiResult<()> {
    let query = "SELECT * FROM files WHERE kb_id = ?";
    let files: Vec<super::File> = sqlx::query_as(query).bind(id).fetch_all(&pool).await?;
    let mut qb = QueryBuilder::new("DELETE FROM slices WHERE file_id IN (");
    let mut separated = qb.separated(", ");
    for file in files {
        fs::remove_file(file.path).await?;
        separated.push_bind(file.id);
    }
    qb.push(")");
    qb.build().execute(&pool).await?;

    sqlx::query("DELETE FROM files WHERE kb_id = ?").bind(id).execute(&pool).await?;
    search_engine.delete(None, Some(id)).await?;
    let query = "DELETE FROM knowledge_bases WHERE id = ?";
    sqlx::query(query).bind(id).execute(&pool).await?;
    Ok(())
}
