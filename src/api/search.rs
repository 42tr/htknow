use axum::{Extension, extract::Query, response::Json};
use log::info;
use serde::{Deserialize, Serialize};

use crate::{api::error::ApiResult, search::SearchEngine};

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    /// 搜索关键词
    pub query: String,
    /// 文件 ID（可选）
    pub file_id: Option<i64>,
    /// 知识库 ID（可选）
    pub kb_id: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub results: Vec<String>,
}

pub async fn search(
    Extension(search_engine): Extension<SearchEngine>, Query(params): Query<SearchQuery>,
) -> ApiResult<Json<SearchResult>> {
    let results = search_engine
        .search(&params.query, params.file_id, params.kb_id)
        .await
        .map_err(|e| crate::api::error::ApiError::internal(format!("Search failed: {}", e)))?;

    Ok(Json(SearchResult { results }))
}
