use std::collections::HashMap;

use axum::{
    Extension, extract::{Query, State}, response::Json
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

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

/// 文件信息
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FileInfo {
    pub id: i64,
    pub filename: String,
    pub kb_id: Option<i64>,
}

/// 知识库信息
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KbInfo {
    pub id: i64,
    pub name: String,
}

/// 单个搜索结果项
#[derive(Debug, Serialize)]
pub struct SearchResultItem {
    /// 切片 ID
    pub id: i64,
    /// 切片内容
    pub content: String,
    /// 搜索得分
    pub score: f32,
    /// 文件信息
    pub file: Option<FileInfo>,
    /// 知识库信息
    pub kb: Option<KbInfo>,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub results: Vec<SearchResultItem>,
}

pub async fn search(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Query(params): Query<SearchQuery>,
) -> ApiResult<Json<SearchResult>> {
    let raw_results = search_engine
        .search(&params.query, params.file_id, params.kb_id)
        .await
        .map_err(|e| crate::api::error::ApiError::internal(format!("Search failed: {}", e)))?;

    if raw_results.is_empty() {
        return Ok(Json(SearchResult { results: vec![] }));
    }

    // 收集所有 file_id 和 kb_id
    let file_ids: Vec<i64> = raw_results.iter().map(|r| r.file_id).collect();
    let kb_ids: Vec<i64> = raw_results.iter().filter_map(|r| r.kb_id).collect();

    // 批量查询文件信息
    let file_map = get_files_by_ids(&pool, &file_ids).await?;

    // 批量查询知识库信息
    let kb_map = if !kb_ids.is_empty() { get_kbs_by_ids(&pool, &kb_ids).await? } else { HashMap::new() };

    // 组装结果
    let results = raw_results
        .into_iter()
        .map(|r| {
            let file = file_map.get(&r.file_id).cloned();
            let kb = r.kb_id.and_then(|kb_id| kb_map.get(&kb_id).cloned());
            SearchResultItem { id: r.id, content: r.content, score: r.score, file, kb }
        })
        .collect();

    Ok(Json(SearchResult { results }))
}

/// 使用知识图谱增强的搜索
pub async fn search_with_graph(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Query(params): Query<SearchQuery>,
) -> ApiResult<Json<SearchResult>> {
    let raw_results = search_engine
        .search_with_graph_expansion(&params.query, params.file_id, params.kb_id)
        .await
        .map_err(|e| crate::api::error::ApiError::internal(format!("Graph search failed: {}", e)))?;

    if raw_results.is_empty() {
        return Ok(Json(SearchResult { results: vec![] }));
    }

    // 收集所有 file_id 和 kb_id
    let file_ids: Vec<i64> = raw_results.iter().map(|r| r.file_id).collect();
    let kb_ids: Vec<i64> = raw_results.iter().filter_map(|r| r.kb_id).collect();

    // 批量查询文件信息
    let file_map = get_files_by_ids(&pool, &file_ids).await?;

    // 批量查询知识库信息
    let kb_map = if !kb_ids.is_empty() { get_kbs_by_ids(&pool, &kb_ids).await? } else { HashMap::new() };

    // 组装结果
    let results = raw_results
        .into_iter()
        .map(|r| {
            let file = file_map.get(&r.file_id).cloned();
            let kb = r.kb_id.and_then(|kb_id| kb_map.get(&kb_id).cloned());
            SearchResultItem { id: r.id, content: r.content, score: r.score, file, kb }
        })
        .collect();

    Ok(Json(SearchResult { results }))
}

async fn get_files_by_ids(pool: &SqlitePool, file_ids: &[i64]) -> Result<HashMap<i64, FileInfo>, sqlx::Error> {
    if file_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders: String = file_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let query = format!("SELECT id, filename, kb_id FROM files WHERE id IN ({})", placeholders);

    let mut q = sqlx::query_as::<_, FileInfo>(&query);
    for id in file_ids {
        q = q.bind(id);
    }

    let files: Vec<FileInfo> = q.fetch_all(pool).await?;
    Ok(files.into_iter().map(|f| (f.id, f)).collect())
}

async fn get_kbs_by_ids(pool: &SqlitePool, kb_ids: &[i64]) -> Result<HashMap<i64, KbInfo>, sqlx::Error> {
    if kb_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders: String = kb_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let query = format!("SELECT id, name FROM knowledge_bases WHERE id IN ({})", placeholders);

    let mut q = sqlx::query_as::<_, KbInfo>(&query);
    for id in kb_ids {
        q = q.bind(id);
    }

    let kbs: Vec<KbInfo> = q.fetch_all(pool).await?;
    Ok(kbs.into_iter().map(|k| (k.id, k)).collect())
}
