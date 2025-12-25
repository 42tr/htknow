use axum::{
    Extension, extract::{Path, Query, State}, response::Json
};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::{AuthUser, api::error::ApiResult};

#[derive(Serialize, Deserialize, Clone, Debug, sqlx::FromRow)]
pub struct Knowledge {
    pub id: i64,
    pub user_id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// 页码，从1开始
    pub page: Option<i64>,
    /// 每页条数
    pub size: Option<i64>,
    /// 关键词搜索信息（在 name + description 中搜索）
    pub keyword: Option<String>,
    /// 模糊搜索 name 字段
    pub name: Option<String>,
    /// 知识库 ID（精确匹配）
    pub id: Option<String>,
}

pub async fn list(
    State(pool): State<SqlitePool>, Query(params): Query<ListQuery>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<Vec<Knowledge>>> {
    println!("user_id = {}, role = {}", auth_user.user_id, auth_user.role);
    // Determine pagination: default size 10, default page 1
    let size = params.size.unwrap_or(10).max(1);
    let page = params.page.unwrap_or(1).max(1);
    let limit = size;
    let offset = (page - 1) * size;

    // Start building the query
    let mut qb = QueryBuilder::<Sqlite>::new("SELECT id, user_id, name, description FROM knowledge_base WHERE 1=1 ");
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

    Ok(Json(knowledges))
}

#[derive(Deserialize)]
pub struct KnowledgeCreateReq {
    pub name: String,
    pub description: String,
}

pub async fn create(
    State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>,
    Json(knowledge): Json<KnowledgeCreateReq>,
) -> ApiResult<Json<Knowledge>> {
    let query = "INSERT INTO knowledge_base (user_id, name, description) VALUES (?, ?, ?)";
    let id = sqlx::query(query)
        .bind(auth_user.user_id)
        .bind(knowledge.name)
        .bind(knowledge.description.clone())
        .execute(&pool)
        .await?
        .last_insert_rowid();
    let kb = sqlx::query_as("SELECT id, user_id, name, description FROM knowledge_base WHERE id = ?")
        .bind(id)
        .fetch_one(&pool)
        .await?;
    Ok(Json(kb))
}

#[derive(Deserialize)]
pub struct KnowledgeUpdateReq {
    pub id: i64,
    pub name: Option<String>,
    pub description: Option<String>,
}

pub async fn update(
    State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>,
    Json(knowledge): Json<KnowledgeUpdateReq>,
) -> ApiResult<Json<Knowledge>> {
    let query = "UPDATE knowledge_base SET name = ?, description = ? WHERE id = ? AND user_id = ?";
    sqlx::query(query)
        .bind(knowledge.name)
        .bind(knowledge.description)
        .bind(knowledge.id)
        .bind(auth_user.user_id)
        .execute(&pool)
        .await?;
    let kb = sqlx::query_as("SELECT id, user_id, name, description FROM knowledge_base WHERE id = ?")
        .bind(knowledge.id)
        .fetch_one(&pool)
        .await?;
    Ok(Json(kb))
}

pub async fn get(State(pool): State<SqlitePool>, Path(id): Path<String>) -> ApiResult<Json<Knowledge>> {
    let id = id.parse::<u32>().unwrap();
    let query = "SELECT id, user_id, name, description FROM knowledge_base WHERE id = ?";
    let knowledge = sqlx::query_as(query).bind(id).fetch_one(&pool).await?;
    Ok(Json(knowledge))
}

pub async fn delete(State(pool): State<SqlitePool>, Path(id): Path<String>) -> ApiResult<()> {
    let id = id.parse::<u32>().unwrap();
    let query = "DELETE FROM knowledge_base WHERE id = ?";
    sqlx::query(query).bind(id).execute(&pool).await?;
    Ok(())
}
