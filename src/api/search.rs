use std::collections::HashMap;

use axum::{
    Extension, extract::{Query, State}, response::Json
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::{IntoParams, ToSchema};

use crate::{AuthUser, api::error::ApiResult, search::SearchEngine};

#[derive(Debug, Deserialize, IntoParams)]
pub struct SearchQuery {
    /// 搜索关键词
    pub query: String,
    /// 文件 ID（可选）
    pub file_id: Option<i64>,
    /// 知识库 ID（可选）
    pub kb_id: Option<i64>,
}

/// 文件信息
#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
pub struct FileInfo {
    pub id: i64,
    pub filename: String,
    pub kb_id: Option<i64>,
    pub is_public: i32,
    pub user_id: String,
}

/// 知识库信息
#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
pub struct KbInfo {
    pub id: i64,
    pub name: String,
    pub is_public: i32,
    pub user_id: String,
}

/// 单个搜索结果项
#[derive(Debug, Serialize, ToSchema)]
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

#[derive(Debug, Serialize, ToSchema)]
pub struct SearchResult {
    pub results: Vec<SearchResultItem>,
}

/// 搜索内容
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/search/",
    tag = "search",
    params(SearchQuery),
    responses(
        (status = 200, description = "搜索成功", body = SearchResult),
        (status = 400, description = "请求参数错误")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn search(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Query(params): Query<SearchQuery>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<SearchResult>> {
    // If a kb_id is specified, find all its descendants to search within.
    let user_id = auth_user.user_id.clone();
    let kb_ids_to_search = if let Some(root_kb_id) = params.kb_id {
        let descendant_ids: Vec<i64> = sqlx::query_scalar(
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
        .bind(root_kb_id)
        .bind(&user_id)
        .fetch_all(&pool)
        .await?;

        if descendant_ids.is_empty() {
            return Ok(Json(SearchResult { results: vec![] }));
        }
        Some(descendant_ids)
    } else {
        None
    };

    let raw_results = search_engine
        .search(&params.query, params.file_id, kb_ids_to_search.as_ref())
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

    // 克隆 user_id 用于闭包
    let user_id = auth_user.user_id.clone();

    // 组装结果并过滤权限
    let results = raw_results
        .into_iter()
        .filter_map(|r| {
            let file = file_map.get(&r.file_id).cloned();
            let kb = r.kb_id.and_then(|kb_id| kb_map.get(&kb_id).cloned());

            // 权限检查
            let has_permission = if let Some(ref file_info) = file {
                // 如果文件存在，检查文件权限
                // 规则：私有文件（is_public=0）只有所有者可以查看
                if file_info.is_public == 0 && file_info.user_id != user_id { false } else { true }
            } else if let Some(ref kb_info) = kb {
                // 如果没有文件信息但有知识库信息，检查知识库权限
                // 规则：私有知识库（is_public=0）只有所有者可以查看
                if kb_info.is_public == 0 && kb_info.user_id != user_id { false } else { true }
            } else {
                // 没有文件和知识库信息，默认允许
                true
            };

            if has_permission {
                Some(SearchResultItem { id: r.id, content: r.content, score: r.score, file, kb })
            } else {
                None
            }
        })
        .collect();

    Ok(Json(SearchResult { results }))
}

/// 使用知识图谱增强的搜索
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/search/graph",
    tag = "search",
    params(SearchQuery),
    responses(
        (status = 200, description = "图谱搜索成功", body = SearchResult),
        (status = 400, description = "请求参数错误")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn search_with_graph(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Query(params): Query<SearchQuery>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<SearchResult>> {
    // If a kb_id is specified, find all its descendants to search within.
    let user_id = auth_user.user_id.clone();
    let kb_ids_to_search = if let Some(root_kb_id) = params.kb_id {
        let descendant_ids: Vec<i64> = sqlx::query_scalar(
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
        .bind(root_kb_id)
        .bind(&user_id)
        .fetch_all(&pool)
        .await?;

        if descendant_ids.is_empty() {
            return Ok(Json(SearchResult { results: vec![] }));
        }
        Some(descendant_ids)
    } else {
        None
    };

    let raw_results = search_engine
        .search_with_graph_expansion(&params.query, params.file_id, kb_ids_to_search.as_ref())
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

    // 克隆 user_id 用于闭包
    let user_id = auth_user.user_id.clone();

    // 组装结果并过滤权限
    let results = raw_results
        .into_iter()
        .filter_map(|r| {
            let file = file_map.get(&r.file_id).cloned();
            let kb = r.kb_id.and_then(|kb_id| kb_map.get(&kb_id).cloned());

            // 权限检查
            let has_permission = if let Some(ref file_info) = file {
                // 如果文件存在，检查文件权限
                // 规则：私有文件（is_public=0）只有所有者可以查看
                if file_info.is_public == 0 && file_info.user_id != user_id { false } else { true }
            } else if let Some(ref kb_info) = kb {
                // 如果没有文件信息但有知识库信息，检查知识库权限
                // 规则：私有知识库（is_public=0）只有所有者可以查看
                if kb_info.is_public == 0 && kb_info.user_id != user_id { false } else { true }
            } else {
                // 没有文件和知识库信息，默认允许
                true
            };

            if has_permission {
                Some(SearchResultItem { id: r.id, content: r.content, score: r.score, file, kb })
            } else {
                None
            }
        })
        .collect();

    Ok(Json(SearchResult { results }))
}

async fn get_files_by_ids(pool: &SqlitePool, file_ids: &[i64]) -> Result<HashMap<i64, FileInfo>, sqlx::Error> {
    if file_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders: String = file_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let query = format!("SELECT id, filename, kb_id, is_public, user_id FROM files WHERE id IN ({})", placeholders);

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
    let query = format!("SELECT id, name, is_public, user_id FROM knowledge_bases WHERE id IN ({})", placeholders);

    let mut q = sqlx::query_as::<_, KbInfo>(&query);
    for id in kb_ids {
        q = q.bind(id);
    }

    let kbs: Vec<KbInfo> = q.fetch_all(pool).await?;
    Ok(kbs.into_iter().map(|k| (k.id, k)).collect())
}
