use crate::api::error::ApiResult;
use axum::extract::{Query, State};
use axum::response::Json;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, SqlitePool};

#[derive(Serialize, Deserialize, Clone, Debug, sqlx::FromRow)]
pub struct Knowledge {
    pub id: i32,
    pub user_id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    // optional pagination
    pub limit: Option<i64>,
    pub offset: Option<i64>,

    // optional filters
    pub user_id: Option<String>,
    pub name: Option<String>,
}

pub async fn list(
    State(pool): State<SqlitePool>,
    Query(params): Query<ListQuery>,
) -> ApiResult<Json<Vec<Knowledge>>> {
    // Build dynamic SQL depending on provided query params
    let mut qb =
        QueryBuilder::<sqlx::Sqlite>::new("SELECT id, user_id, name, description FROM knowledge");

    // If any filter is present, add a WHERE 1=1 and append conditions
    if params.user_id.is_some() || params.name.is_some() {
        qb.push(" WHERE 1=1");
        if let Some(user_id) = &params.user_id {
            qb.push(" AND user_id = ").push_bind(user_id);
        }
        if let Some(name) = &params.name {
            // use LIKE for name partial matches
            let pattern = format!("%{}%", name);
            qb.push(" AND name LIKE ").push_bind(pattern);
        }
    }

    // ordering
    qb.push(" ORDER BY id");

    // pagination
    if let Some(limit) = params.limit {
        qb.push(" LIMIT ").push_bind(limit);
    }
    if let Some(offset) = params.offset {
        qb.push(" OFFSET ").push_bind(offset);
    }

    // Build and execute
    let query = qb.build_query_as::<Knowledge>();
    let knowledges = query.fetch_all(&pool).await?;

    Ok(Json(knowledges))
}
