use crate::api::error::ApiResult;
use axum::Extension;
use axum::extract::{Query, State};
use axum::response::Json;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

#[derive(Serialize, Deserialize, Clone, Debug, sqlx::FromRow)]
pub struct Knowledge {
    pub id: i32,
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
    State(pool): State<SqlitePool>,
    Query(params): Query<ListQuery>,
    Extension(user_id): Extension<String>,
) -> ApiResult<Json<Vec<Knowledge>>> {
    println!("user_id = {}", user_id);
    // Determine pagination: default size 10, default page 1
    let size = params.size.unwrap_or(10).max(1);
    let page = params.page.unwrap_or(1).max(1);
    let limit = size;
    let offset = (page - 1) * size;

    // Start building the query
    let mut qb = QueryBuilder::<Sqlite>::new(
        "SELECT id, user_id, name, description FROM knowledge WHERE 1=1 ",
    );

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
