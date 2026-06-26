use std::{
    collections::{HashMap, HashSet}, path::Component, sync::{Arc, OnceLock}, time::Instant
};

use anyhow::Result as AnyResult;
use axum::{
    Extension, body::Body, extract::{Multipart, Path, Query, State}, http::{StatusCode, header}, response::Json
};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};
use tokio::{
    fs, io::AsyncWriteExt as _, spawn, sync::{OwnedSemaphorePermit, Semaphore}
};
use utoipa::{IntoParams, ToSchema};

use crate::{
    AuthUser, api::error::{ApiError, ApiResult}, archive::{self, ArchiveEntry, ExtractResult}, config, pdf_highlight, processor, search::SearchEngine
};

/// 以流式方式打开文件，返回 (字节数, Body)。
///
/// 避免将整个文件读入内存（大文件下载会按 chunk 流式发送），同时返回文件大小用于 Content-Length。
async fn open_file_stream(path: &std::path::Path) -> Result<(u64, Body), ApiError> {
    let file = fs::File::open(path).await?;
    let len = file.metadata().await?.len();
    let stream = tokio_util::io::ReaderStream::new(file);
    Ok((len, Body::from_stream(stream)))
}

/// `files` 表去掉大字段 `content` 的列清单（content 以 NULL 占位）。
///
/// 列表类查询不需要全文 content，用此清单避免把可能很大的 content 列从 SQLite 读进内存。
pub(crate) const FILE_COLS_NO_CONTENT: &str = "id, user_id, user_name, hash, filename, path, size, NULL as content, \
     tags, status, log, slice_type, kb_id, is_public, meta, created_at, updated_at";

/// Excel 单 sheet 数据
#[derive(Debug, Serialize, ToSchema)]
pub struct ExcelSheetData {
    pub name: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
}

/// Excel 文件数据
#[derive(Debug, Serialize, ToSchema)]
pub struct ExcelData {
    pub filename: String,
    pub sheets: Vec<ExcelSheetData>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ExcelDataQuery {
    pub sheet_name: Option<String>,
    pub row_num: Option<usize>,
}

/// 解压压缩文件请求
#[derive(Debug, Deserialize, ToSchema)]
pub struct ArchiveExtractReq {
    /// 解压密码（加密压缩文件时需要）
    pub password: Option<String>,
}

/// 下载压缩包内文件请求参数
#[derive(Debug, Deserialize, IntoParams)]
pub struct ArchiveDownloadQuery {
    /// 压缩包内文件路径
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, sqlx::FromRow, ToSchema)]
pub struct File {
    pub id: i64,
    pub user_id: String,
    pub user_name: String,
    pub hash: String,
    pub filename: String,
    pub path: String,
    pub size: i64,
    pub content: Option<String>,
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

#[derive(Serialize, Deserialize, Clone, Debug, Default, ToSchema)]
pub struct FileStatusBreakdown {
    /// 所有满足条件的文件总数
    pub total: i64,
    /// status = 0，待处理
    pub pending: i64,
    /// status = 2，处理中
    pub processing: i64,
    /// status = 1，已完成
    pub completed: i64,
    /// status = 3，不解析/跳过
    pub skipped: i64,
    /// status = -1，处理失败
    pub failed: i64,
    /// 其他未知状态
    pub unknown: i64,
    /// 当前正在处理的文件（按更新时间倒序，最多10条）
    pub processing_files: Vec<FileStatusPreview>,
}

#[derive(Serialize, Deserialize, Clone, Debug, sqlx::FromRow, ToSchema)]
pub struct FileStatusPreview {
    pub id: i64,
    pub filename: String,
    pub kb_id: Option<i64>,
    pub kb_name: Option<String>,
    pub updated_at: i64,
}

impl FileStatusBreakdown {
    fn add(&mut self, status: i32, count: i64) {
        self.total += count;
        match status {
            0 => self.pending += count,
            1 => self.completed += count,
            2 => self.processing += count,
            3 => self.skipped += count,
            -1 => self.failed += count,
            _ => self.unknown += count,
        }
    }
}

#[derive(Clone, Copy)]
enum FileStatsScope {
    Global { include_unassigned: bool },
    KnowledgeBase { kb_id: i64, include_descendants: bool },
    UnassignedOnly,
}

const PARSE_ASSETS_META_KEY: &str = "_htknow_parse_assets";
const CUSTOM_IMAGES_META_KEY: &str = "custom_images";
const CUSTOM_IMAGES_META_VERSION: i64 = 1;

async fn query_file_status_breakdown(
    pool: &SqlitePool, scope: FileStatsScope, user_id: &str, is_admin: bool,
) -> AnyResult<FileStatusBreakdown> {
    let mut qb = QueryBuilder::<Sqlite>::new("");

    if let FileStatsScope::KnowledgeBase { kb_id, include_descendants: true } = scope {
        qb.push("WITH RECURSIVE descendants AS (SELECT id FROM knowledge_bases WHERE id = ");
        qb.push_bind(kb_id);
        if !is_admin {
            qb.push(" AND (user_id = ").push_bind(user_id).push(" OR is_public = 1)");
        }
        qb.push(" UNION ALL SELECT kb.id FROM knowledge_bases kb JOIN descendants d ON kb.parent_id = d.id");
        if !is_admin {
            qb.push(" WHERE kb.user_id = ").push_bind(user_id).push(" OR kb.is_public = 1");
        }
        qb.push(") ");
    }

    qb.push("SELECT COALESCE(f.status, -99) AS status, COUNT(*) AS cnt FROM files f WHERE 1=1");

    match scope {
        FileStatsScope::Global { include_unassigned } => {
            if !include_unassigned {
                qb.push(" AND f.kb_id IS NOT NULL");
            }
        }
        FileStatsScope::KnowledgeBase { kb_id, include_descendants } => {
            if include_descendants {
                qb.push(" AND f.kb_id IN (SELECT id FROM descendants)");
            } else {
                qb.push(" AND f.kb_id = ").push_bind(kb_id);
            }
        }
        FileStatsScope::UnassignedOnly => {
            qb.push(" AND f.kb_id IS NULL");
        }
    }

    if !is_admin {
        qb.push(" AND (f.user_id = ").push_bind(user_id).push(" OR f.is_public = 1)");
    }

    qb.push(" GROUP BY f.status");

    let rows = qb.build().fetch_all(pool).await?;
    let mut breakdown = FileStatusBreakdown { processing_files: Vec::new(), ..Default::default() };
    for row in rows {
        let status: i32 = row.get("status");
        let cnt: i64 = row.get("cnt");
        breakdown.add(status, cnt);
    }
    breakdown.processing_files = fetch_processing_files_for_scope(pool, scope, user_id, is_admin).await?;
    Ok(breakdown)
}

async fn fetch_processing_files_for_scope(
    pool: &SqlitePool, scope: FileStatsScope, user_id: &str, is_admin: bool,
) -> AnyResult<Vec<FileStatusPreview>> {
    let mut qb = QueryBuilder::<Sqlite>::new(
        "SELECT f.id, f.filename, f.kb_id, kb.name AS kb_name, f.updated_at FROM files f \
         LEFT JOIN knowledge_bases kb ON kb.id = f.kb_id WHERE f.status = 2",
    );

    match scope {
        FileStatsScope::Global { include_unassigned } => {
            if !include_unassigned {
                qb.push(" AND f.kb_id IS NOT NULL");
            }
        }
        FileStatsScope::KnowledgeBase { kb_id, include_descendants } => {
            if include_descendants {
                qb.push(" AND f.kb_id IN (WITH RECURSIVE descendants AS (SELECT id FROM knowledge_bases WHERE id = ");
                qb.push_bind(kb_id);
                if !is_admin {
                    qb.push(" AND (user_id = ").push_bind(user_id).push(" OR is_public = 1)");
                }
                qb.push(
                    " UNION ALL SELECT kb.id FROM knowledge_bases kb JOIN descendants d ON kb.parent_id = d.id \
                     ) SELECT id FROM descendants)",
                );
            } else {
                qb.push(" AND f.kb_id = ").push_bind(kb_id);
            }
        }
        FileStatsScope::UnassignedOnly => {
            qb.push(" AND f.kb_id IS NULL");
        }
    }

    if !is_admin {
        qb.push(" AND (f.user_id = ").push_bind(user_id).push(" OR f.is_public = 1)");
    }

    qb.push(" ORDER BY f.updated_at DESC LIMIT 10");

    let rows = qb.build_query_as::<FileStatusPreview>().fetch_all(pool).await?;
    Ok(rows)
}

pub async fn get_file_status_breakdown_for_kb(
    pool: &SqlitePool, kb_id: i64, include_descendants: bool, user_id: &str, is_admin: bool,
) -> AnyResult<FileStatusBreakdown> {
    query_file_status_breakdown(pool, FileStatsScope::KnowledgeBase { kb_id, include_descendants }, user_id, is_admin)
        .await
}

pub async fn get_file_status_breakdown_for_all(
    pool: &SqlitePool, include_unassigned: bool, user_id: &str, is_admin: bool,
) -> AnyResult<FileStatusBreakdown> {
    query_file_status_breakdown(pool, FileStatsScope::Global { include_unassigned }, user_id, is_admin).await
}

pub async fn get_file_status_breakdown_for_unassigned(
    pool: &SqlitePool, user_id: &str, is_admin: bool,
) -> AnyResult<FileStatusBreakdown> {
    query_file_status_breakdown(pool, FileStatsScope::UnassignedOnly, user_id, is_admin).await
}

static BACKGROUND_REUSE_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn background_reuse_semaphore() -> Arc<Semaphore> {
    BACKGROUND_REUSE_SEMAPHORE
        .get_or_init(|| {
            let cfg = config::get();
            let limit = cfg.server.process_concurrency.max(1).min(cfg.database.max_connections as usize).max(1);
            Arc::new(Semaphore::new(limit))
        })
        .clone()
}

async fn acquire_background_reuse_permit(semaphore: Arc<Semaphore>, file_id: i64) -> Option<OwnedSemaphorePermit> {
    match semaphore.acquire_owned().await {
        Ok(permit) => Some(permit),
        Err(e) => {
            warn!("Background reuse semaphore closed for file {}: {}", file_id, e);
            None
        }
    }
}

fn map_search_engine_error(err: anyhow::Error) -> ApiError {
    let msg = err.to_string();
    if msg.contains("LockBusy") || msg.contains("Failed to acquire index lock") {
        return ApiError::BadRequest("Search index is busy, please retry shortly.".to_string());
    }
    ApiError::Internal(format!("Internal error: {}", msg))
}

/// 上传文件（支持单个或多个文件）
///
/// form-data 参数：
/// - file: 文件内容（可多次出现）
/// - slice_type: 分片类型
/// - kb_id: 知识库 ID
/// - tags: JSON 数组字符串，例如 ["tag1","tag2"]
/// - is_public: true/false 或 1/0
/// - meta: 元数据字符串
/// - immediate_parse: true/1 时后台解析
/// - sync: true/1 时等待解析完成并返回最新的文件记录
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/files/",
    operation_id = "file_upload",
    tag = "file",
    request_body(
        content_type = "multipart/form-data",
        description = "form-data: file, slice_type, kb_id, tags(JSON array string), is_public, meta, immediate_parse, sync"
    ),
    responses(
        (status = 200, description = "文件上传成功", body = Vec<File>),
        (status = 400, description = "请求参数错误"),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn upload(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Extension(auth_user): Extension<AuthUser>, mut multipart: Multipart,
) -> ApiResult<Json<Vec<File>>> {
    debug!("Starting file upload for user: {}", auth_user.user_id);
    let cfg = config::get();
    let dir = &cfg.storage.files_path;
    tokio::fs::create_dir_all(dir).await?;
    let reuse_duplicates = cfg.server.reuse_duplicate_files;

    let mut files_data: Vec<(String, String, String, i64)> = Vec::new();
    let mut slice_type = String::new();
    let mut kb_id = None;
    let mut is_public = 0i32;
    let mut tags: Vec<String> = Vec::new();
    let mut meta: Option<String> = None;
    let mut immediate_parse = false;
    let mut sync = false;

    loop {
        match multipart.next_field().await {
            Ok(Some(mut field)) => {
                let field_name = field.name().map(|s| s.to_string());
                debug!("Processing field: {:?}", field_name);

                match field_name.as_deref() {
                    Some("file") => {
                        let mut hasher = Sha256::new();
                        let mut size: i64 = 0;
                        let filename = field.file_name().unwrap_or("unknown").to_string();
                        debug!("Uploading file: {}", filename);
                        let tempname = uuid::Uuid::new_v4().to_string();
                        let filepath = format!("{}/{}", dir, tempname);
                        let mut file = tokio::fs::File::create(filepath.clone()).await?;
                        while let Some(chunk) = field.chunk().await? {
                            size += chunk.len() as i64;
                            file.write_all(&chunk).await?;
                            hasher.update(&chunk);
                        }
                        let hash = hex::encode(hasher.finalize());
                        debug!("File saved to: {}", filepath);
                        files_data.push((hash, filename, filepath, size));
                    }
                    Some("slice_type") => {
                        slice_type = field.text().await?;
                        debug!("Slice type: {}", slice_type);
                    }
                    Some("kb_id") => {
                        let kb_id_text = field.text().await?;
                        debug!("KB ID text: {}", kb_id_text);
                        kb_id = Some(kb_id_text.parse::<i64>()?);
                    }
                    Some("tags") => {
                        let tags_text = field.text().await?;
                        debug!("Tags text: {}", tags_text);
                        if !tags_text.is_empty() {
                            tags = serde_json::from_str(&tags_text).unwrap_or_default();
                        }
                    }
                    Some("is_public") => {
                        let is_public_text = field.text().await?;
                        debug!("Is public text: {}", is_public_text);
                        is_public = match is_public_text.as_str() {
                            "true" | "1" => 1,
                            "false" | "0" => 0,
                            _ => is_public_text.parse::<bool>().map_or(0, |b| if b { 1 } else { 0 }),
                        };
                    }
                    Some("meta") => {
                        let meta_text = field.text().await?;
                        debug!("Meta text: {}", meta_text);
                        if !meta_text.is_empty() {
                            meta = Some(meta_text);
                        }
                    }
                    Some("immediate_parse") => {
                        // 立刻开始解析
                        let parse_text = field.text().await?;
                        debug!("Immediate parse text: {}", parse_text);
                        immediate_parse = parse_text == "true" || parse_text == "1";
                    }
                    Some("sync") => {
                        // 等待解析结果
                        let sync_text = field.text().await?;
                        debug!("Sync parse text: {}", sync_text);
                        sync = sync_text == "true" || sync_text == "1";
                    }
                    _ => {
                        debug!("Skipping unknown field: {:?}", field_name);
                    }
                }
            }
            Ok(None) => {
                debug!("No more fields");
                break;
            }
            Err(e) => {
                log::error!("Error reading multipart field: {}", e);
                return Err(ApiError::Internal(format!("Multipart error: {}", e)));
            }
        }
    }

    debug!("File upload completed. Files count: {}", files_data.len());
    if files_data.is_empty() {
        return Err(ApiError::BadRequest("file is required".to_string()));
    }

    let mut kb_type: Option<String> = None;
    if let Some(kb_id_value) = kb_id {
        let kb: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT user_id, kb_type FROM knowledge_bases WHERE id = ?")
                .bind(kb_id_value)
                .fetch_optional(&pool)
                .await?;
        let (_, kb_type_value) = kb.ok_or_else(|| ApiError::NotFound("Knowledge base not found".to_string()))?;
        kb_type = kb_type_value;

        // Check KB permission: need editor or admin to upload
        let perm =
            super::knowledge_base::get_kb_permission(&pool, kb_id_value, &auth_user.user_id, auth_user.is_admin())
                .await;
        if !super::knowledge_base::meets_requirement(perm.as_deref(), "editor") {
            return Err(ApiError::Forbidden(
                "Permission denied. Requires editor or admin to upload files.".to_string(),
            ));
        }
    }

    let is_storage_kb = matches!(kb_type.as_deref(), Some("storage"));
    let tags_json = serde_json::to_string(&tags)?;
    let status = if is_storage_kb { 3 } else { 0 };
    let log_message = if is_storage_kb { "Storage mode: not parsed" } else { "" };

    let mut uploaded_files: Vec<File> = Vec::new();
    let mut uploaded_file_ids: Vec<i64> = Vec::new();
    let mut parse_file_ids: Vec<i64> = Vec::new();

    for (hash, filename, filepath, size) in files_data {
        let sql = "INSERT INTO files (user_id, user_name, hash, filename, path, size, slice_type, kb_id, is_public, tags, status, log, meta) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
        let id = sqlx::query(sql)
            .bind(&auth_user.user_id)
            .bind(&auth_user.user_name)
            .bind(hash)
            .bind(filename)
            .bind(filepath)
            .bind(size)
            .bind(&slice_type)
            .bind(kb_id)
            .bind(is_public)
            .bind(&tags_json)
            .bind(status)
            .bind(log_message)
            .bind(meta.clone())
            .execute(&pool)
            .await?
            .last_insert_rowid();

        let mut file: File = sqlx::query_as(&format!("SELECT {FILE_COLS_NO_CONTENT} FROM files WHERE id = ?"))
            .bind(id)
            .fetch_one(&pool)
            .await?;
        if file.status == 3 {
            uploaded_files.push(file);
            uploaded_file_ids.push(id);
            continue;
        }
        let mut reused_now = false;

        if reuse_duplicates {
            if sync {
                match processor::try_reuse_file_with_file(pool.clone(), search_engine.clone(), file.clone()).await {
                    Ok(true) => {
                        reused_now = true;
                        file = sqlx::query_as(&format!("SELECT {FILE_COLS_NO_CONTENT} FROM files WHERE id = ?"))
                            .bind(id)
                            .fetch_one(&pool)
                            .await?;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        warn!("Immediate reuse failed for file {}: {}", id, e);
                    }
                }
            } else if !immediate_parse {
                let pool_clone = pool.clone();
                let search_engine_clone = search_engine.clone();
                let file_clone = file.clone();
                let file_id = id;
                let semaphore = background_reuse_semaphore();
                spawn(async move {
                    let Some(_permit) = acquire_background_reuse_permit(semaphore, file_id).await else {
                        return;
                    };

                    let reuse_result = processor::try_reuse_file_with_file(
                        pool_clone.clone(),
                        search_engine_clone.clone(),
                        file_clone,
                    )
                    .await;

                    match reuse_result {
                        Ok(true) => {}
                        Ok(false) => {
                            let reuse_failed =
                                match sqlx::query_scalar::<_, String>("SELECT log FROM files WHERE id = ?")
                                    .bind(file_id)
                                    .fetch_optional(&pool_clone)
                                    .await
                                {
                                    Ok(Some(log)) => log.starts_with("Reuse failed:"),
                                    Ok(None) => false,
                                    Err(e) => {
                                        warn!("Failed to read reuse log for file {}: {}", file_id, e);
                                        false
                                    }
                                };
                            if reuse_failed {
                                warn!("Background reuse failed for file {}, falling back to normal parsing", file_id);
                                if let Err(parse_err) =
                                    processor::process_file_immediate(pool_clone, search_engine_clone, file_id).await
                                {
                                    log::error!(
                                        "Fallback parse failed for file {} after reuse failure: {}",
                                        file_id,
                                        parse_err
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("Background reuse failed for file {}: {}", file_id, e);
                            if let Err(parse_err) =
                                processor::process_file_immediate(pool_clone, search_engine_clone, file_id).await
                            {
                                log::error!(
                                    "Fallback parse failed for file {} after reuse error: {}",
                                    file_id,
                                    parse_err
                                );
                            }
                        }
                    }
                });
            }
        }

        if (immediate_parse || sync) && !reused_now {
            parse_file_ids.push(id);
        }

        uploaded_files.push(file);
        uploaded_file_ids.push(id);
    }

    if immediate_parse || sync {
        if sync {
            let reuse_already_tried = reuse_duplicates;
            let concurrency = config::get().server.process_concurrency.max(1);
            let semaphore = Arc::new(Semaphore::new(concurrency));
            let mut handles = Vec::with_capacity(parse_file_ids.len());
            for file_id in parse_file_ids {
                let pool_c = pool.clone();
                let se_c = search_engine.clone();
                let sem_c = semaphore.clone();
                let reuse_tried = reuse_already_tried;
                handles.push(spawn(async move {
                    let _permit = sem_c.acquire().await;
                    if reuse_tried {
                        processor::process_file_immediate_skip_reuse(pool_c, se_c, file_id).await
                    } else {
                        processor::process_file_immediate(pool_c, se_c, file_id).await
                    }
                }));
            }
            for handle in handles {
                handle.await.map_err(|e| ApiError::internal(format!("parse join error: {}", e)))??;
            }
            uploaded_files.clear();
            if !uploaded_file_ids.is_empty() {
                let mut qb = QueryBuilder::<Sqlite>::new("SELECT * FROM files WHERE id IN (");
                let mut sep = qb.separated(", ");
                for id in &uploaded_file_ids {
                    sep.push_bind(*id);
                }
                qb.push(")");
                let rows: Vec<File> = qb.build_query_as().fetch_all(&pool).await?;
                let order: HashMap<i64, usize> = uploaded_file_ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
                let mut sorted = rows;
                sorted.sort_by_key(|f| order.get(&f.id).copied().unwrap_or(usize::MAX));
                uploaded_files = sorted;
            }
        } else {
            let pool = pool.clone();
            let search_engine = search_engine.clone();
            spawn(async move {
                for file_id in parse_file_ids {
                    if let Err(e) =
                        processor::process_file_immediate(pool.clone(), search_engine.clone(), file_id).await
                    {
                        log::error!("Failed to parse file {}: {}", file_id, e);
                    }
                }
            });
        }
    }

    Ok(Json(uploaded_files))
}

/// 获取文件详情
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/files/{id}",
    operation_id = "file_get",
    tag = "file",
    params(
        ("id" = i64, Path, description = "文件 ID")
    ),
    responses(
        (status = 200, description = "成功返回文件详情", body = File),
        (status = 404, description = "文件不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn get(
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<File>> {
    let query = "SELECT * FROM files WHERE id = ?";
    let file: File = sqlx::query_as(query).bind(id).fetch_one(&pool).await?;

    if !auth_user.is_admin() && !file.is_public && file.user_id != auth_user.user_id {
        return Err(ApiError::NotFound("File not found or permission denied".to_string()));
    }

    Ok(Json(file))
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateFileReq {
    pub slice_type: Option<String>,
    pub filename: Option<String>,
    pub tags: Option<Vec<String>>,
    pub is_public: Option<bool>,
    pub meta: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MoveFileReq {
    /// 目标知识库 ID。传 null 表示移出知识库（未分配）。
    pub target_kb_id: Option<i64>,
}

const MAX_BATCH_DELETE_IDS: usize = 200;
const SQLITE_DELETE_CHUNK_SIZE: usize = 900;

#[derive(Debug, Deserialize, ToSchema)]
pub struct BatchDeleteFilesReq {
    /// 待删除文件 ID 列表（会自动去重）
    pub ids: Vec<i64>,
    /// 严格模式：只要有任意文件不可删，则整批拒绝
    #[serde(default)]
    pub strict: bool,
    /// 是否允许删除处理中(status=2)的文件，默认 false
    #[serde(default)]
    pub allow_processing: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BatchDeleteSkippedItem {
    pub id: i64,
    pub reason: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BatchDeleteCleanupFailedItem {
    pub id: i64,
    /// 清理阶段：file/pdf/search
    pub stage: String,
    pub error: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BatchDeleteFilesResp {
    pub requested: i64,
    pub accepted: i64,
    pub deleted: i64,
    pub deleted_ids: Vec<i64>,
    pub skipped: Vec<BatchDeleteSkippedItem>,
    pub cleanup_failed: Vec<BatchDeleteCleanupFailedItem>,
}

const fn default_true_flag() -> bool {
    true
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ReparseFailedFilesReq {
    /// 指定知识库 ID；不传表示全局范围
    pub kb_id: Option<i64>,
    /// 指定知识库时是否包含子知识库，默认 true
    #[serde(default = "default_true_flag")]
    pub include_descendants: bool,
    /// 全局范围时是否包含未分配文件，默认 true
    #[serde(default = "default_true_flag")]
    pub include_unassigned: bool,
    /// 是否只处理未分配知识库中的失败文件，默认 false
    #[serde(default)]
    pub unassigned_only: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReparseFailedFilesResp {
    pub file_count: i64,
}

/// 更新文件
#[utoipa::path(
    put,
    path = "/api/v1/knowledge/files/{id}",
    operation_id = "file_update",
    tag = "file",
    params(
        ("id" = i64, Path, description = "文件 ID")
    ),
    request_body = UpdateFileReq,
    responses(
        (status = 200, description = "成功更新文件", body = File),
        (status = 400, description = "请求参数错误"),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn update(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Extension(auth_user): Extension<AuthUser>, Path(id): Path<i64>, Json(req): Json<UpdateFileReq>,
) -> ApiResult<Json<File>> {
    let is_admin = auth_user.is_admin();

    // Check permission on the file's KB
    let file_kb: Option<Option<i64>> =
        sqlx::query_scalar("SELECT kb_id FROM files WHERE id = ?").bind(id).fetch_optional(&pool).await?;
    let kb_id = file_kb.ok_or_else(|| ApiError::NotFound("File not found".to_string()))?;
    match kb_id {
        Some(kid) => {
            let perm = super::knowledge_base::get_kb_permission(&pool, kid, &auth_user.user_id, is_admin).await;
            if !super::knowledge_base::meets_requirement(perm.as_deref(), "editor") {
                return Err(ApiError::Forbidden("Permission denied. Requires editor or admin.".to_string()));
            }
        }
        None => {
            // Unassigned file: only owner or admin can modify
            if !is_admin {
                let owner: Option<String> =
                    sqlx::query_scalar("SELECT user_id FROM files WHERE id = ?").bind(id).fetch_optional(&pool).await?;
                if owner.as_deref() != Some(&auth_user.user_id) {
                    return Err(ApiError::Forbidden("Permission denied.".to_string()));
                }
            }
        }
    }

    let mut has_updates = false;
    let update_is_public = req.is_public.is_some();
    let mut reset_parse_data = false;
    let mut image_paths_to_remove = Vec::new();
    debug!("update_is_public: {}", update_is_public);

    let mut qb = QueryBuilder::<Sqlite>::new("UPDATE files SET ");
    let mut separated = qb.separated(", ");

    if let Some(slice_type) = req.slice_type.as_deref() {
        let kb_type: Option<String> =
            sqlx::query_scalar("SELECT kb_type FROM knowledge_bases WHERE id = (SELECT kb_id FROM files WHERE id = ?)")
                .bind(id)
                .fetch_optional(&pool)
                .await?;
        if matches!(kb_type.as_deref(), Some("storage")) {
            return Err(ApiError::BadRequest("Storage knowledge base files do not support parsing.".to_string()));
        }

        image_paths_to_remove = collect_image_paths_for_files(&pool, &[id]).await?;
        reset_parse_data = true;

        separated.push("slice_type = ").push_bind_unseparated(slice_type);
        separated.push("status = ").push_bind_unseparated(0);
        separated.push("log = ").push_bind_unseparated("");
        separated.push("content = NULL");
        has_updates = true;
    }

    if let Some(filename) = req.filename.as_deref() {
        separated.push("filename = ").push_bind_unseparated(filename);
        has_updates = true;
    }

    if let Some(tags) = req.tags.as_ref() {
        let tags_json = serde_json::to_string(tags)?;
        debug!("tags_json: {}", tags_json);
        separated.push("tags = ").push_bind_unseparated(tags_json);
        has_updates = true;
    }

    if let Some(is_public) = req.is_public {
        let is_public = if is_public { 1 } else { 0 };
        separated.push("is_public = ").push_bind_unseparated(is_public);
        has_updates = true;
    }

    if let Some(meta) = req.meta {
        let meta_json = serde_json::to_string(&meta)?;
        debug!("meta_json: {}", meta_json);
        separated.push("meta = ").push_bind_unseparated(meta_json);
        has_updates = true;
    }

    if !has_updates {
        return Err(ApiError::BadRequest("No fields to update".to_string()));
    }

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    separated.push("updated_at = ").push_bind_unseparated(now);
    qb.push(" WHERE id = ");
    qb.push_bind(id);
    if reset_parse_data {
        let mut tx = pool.begin().await?;
        clear_file_parse_rows_in_tx(&mut tx, id).await?;
        qb.build().execute(&mut *tx).await?;
        tx.commit().await?;

        if let Err(e) = search_engine.delete(Some(id), None).await {
            warn!("Failed to delete search index for updated file {}: {}", id, e);
        }
        remove_image_files(image_paths_to_remove).await;
        remove_converted_pdfs(&[id]).await;
    } else {
        qb.build().execute(&pool).await?;
    }

    let file = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;
    Ok(Json(file))
}

/// 移动文件到另一个知识库
#[utoipa::path(
    put,
    path = "/api/v1/knowledge/files/{id}/move",
    operation_id = "file_move",
    tag = "file",
    params(
        ("id" = i64, Path, description = "文件 ID")
    ),
    request_body = MoveFileReq,
    responses(
        (status = 200, description = "成功移动文件", body = File),
        (status = 400, description = "请求参数错误"),
        (status = 401, description = "未授权"),
        (status = 404, description = "文件或知识库不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn move_to_kb(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Extension(auth_user): Extension<AuthUser>, Path(id): Path<i64>, Json(req): Json<MoveFileReq>,
) -> ApiResult<Json<File>> {
    if let Some(target_kb_id) = req.target_kb_id
        && target_kb_id <= 0
    {
        return Err(ApiError::BadRequest("Invalid target_kb_id".to_string()));
    }

    let is_admin = auth_user.is_admin();
    let file: File = sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = ?")
        .bind(id)
        .fetch_optional(&pool)
        .await?
        .ok_or_else(|| ApiError::NotFound("File not found".to_string()))?;

    // Check source KB permission
    match file.kb_id {
        Some(src_kb_id) => {
            let perm = super::knowledge_base::get_kb_permission(&pool, src_kb_id, &auth_user.user_id, is_admin).await;
            if !super::knowledge_base::meets_requirement(perm.as_deref(), "editor") {
                return Err(ApiError::Forbidden("Permission denied on source knowledge base.".to_string()));
            }
        }
        None => {
            if !is_admin && file.user_id != auth_user.user_id {
                return Err(ApiError::Forbidden("Permission denied.".to_string()));
            }
        }
    }

    if file.status == 2 {
        return Err(ApiError::BadRequest("File is processing, cannot move now".to_string()));
    }

    if file.kb_id == req.target_kb_id {
        return Ok(Json(file));
    }

    // Check target KB permission
    let target_kb_type: Option<String> = match req.target_kb_id {
        Some(target_kb_id) => {
            let perm =
                super::knowledge_base::get_kb_permission(&pool, target_kb_id, &auth_user.user_id, is_admin).await;
            if !super::knowledge_base::meets_requirement(perm.as_deref(), "editor") {
                return Err(ApiError::Forbidden("Permission denied on target knowledge base.".to_string()));
            }
            let kb_type: Option<String> = sqlx::query_scalar("SELECT kb_type FROM knowledge_bases WHERE id = ?")
                .bind(target_kb_id)
                .fetch_optional(&pool)
                .await?;
            Some(kb_type.ok_or_else(|| ApiError::NotFound("Knowledge base not found".to_string()))?)
        }
        None => None,
    };

    let (next_status, next_log) =
        if matches!(target_kb_type.as_deref(), Some("storage")) { (3, "Storage mode: not parsed") } else { (0, "") };

    let image_paths = collect_image_paths_for_files(&pool, &[id]).await?;

    let mut tx = pool.begin().await?;
    let update_result = sqlx::query(
        "UPDATE files SET kb_id = ?, status = ?, log = ?, content = NULL, updated_at = strftime('%s','now') WHERE id = ? AND status != 2 AND updated_at = ?",
    )
    .bind(req.target_kb_id)
    .bind(next_status)
    .bind(next_log)
    .bind(id)
    .bind(file.updated_at)
    .execute(&mut *tx)
    .await?;
    if update_result.rows_affected() == 0 {
        return Err(ApiError::BadRequest("File state changed, please retry".to_string()));
    }
    clear_file_parse_rows_in_tx(&mut tx, id).await?;
    tx.commit().await?;

    if let Err(e) = search_engine.delete(Some(id), None).await {
        warn!("Failed to delete search index for moved file {}: {}", id, e);
    }

    remove_image_files(image_paths).await;
    let cfg = config::get();
    let pdf_path = std::path::Path::new(&cfg.storage.pdf_path).join(format!("{}.pdf", id));
    if let Err(e) = fs::remove_file(&pdf_path).await
        && !matches!(e.kind(), std::io::ErrorKind::NotFound)
    {
        warn!("Failed to delete converted pdf {} after file move: {}", pdf_path.display(), e);
    }

    let moved: File = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;
    Ok(Json(moved))
}

/// 批量删除文件
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/files/batch-delete",
    operation_id = "file_batch_delete",
    tag = "file",
    request_body = BatchDeleteFilesReq,
    responses(
        (status = 200, description = "批量删除完成（可能部分成功）", body = BatchDeleteFilesResp),
        (status = 400, description = "请求参数错误"),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn batch_delete(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Extension(auth_user): Extension<AuthUser>, Json(req): Json<BatchDeleteFilesReq>,
) -> ApiResult<Json<BatchDeleteFilesResp>> {
    let result =
        execute_batch_delete(&pool, &search_engine, &auth_user, req.ids, req.strict, req.allow_processing).await?;
    Ok(Json(result))
}

/// 重新解析失败文件
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/files/reparse-failed",
    operation_id = "file_reparse_failed",
    tag = "file",
    request_body = ReparseFailedFilesReq,
    responses(
        (status = 200, description = "已提交失败文件重新解析", body = ReparseFailedFilesResp),
        (status = 400, description = "请求参数错误"),
        (status = 401, description = "未授权"),
        (status = 404, description = "知识库不存在或无权限")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn reparse_failed(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Extension(auth_user): Extension<AuthUser>, Json(req): Json<ReparseFailedFilesReq>,
) -> ApiResult<Json<ReparseFailedFilesResp>> {
    let result = execute_reparse_failed(&pool, &search_engine, &auth_user, req).await?;
    Ok(Json(result))
}

/// 删除文件
#[utoipa::path(
    delete,
    path = "/api/v1/knowledge/files/{id}",
    operation_id = "file_delete",
    tag = "file",
    params(
        ("id" = i64, Path, description = "文件 ID")
    ),
    responses(
        (status = 200, description = "成功删除文件"),
        (status = 401, description = "未授权"),
        (status = 404, description = "文件不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn delete(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>,
    Extension(auth_user): Extension<AuthUser>, Path(id): Path<i64>,
) -> ApiResult<()> {
    let result = execute_batch_delete(&pool, &search_engine, &auth_user, vec![id], false, true).await?;
    if result.deleted == 0 {
        return Err(ApiError::NotFound("File not found or permission denied".to_string()));
    }
    Ok(())
}

fn normalize_batch_delete_ids(ids: Vec<i64>) -> ApiResult<Vec<i64>> {
    if ids.is_empty() {
        return Err(ApiError::BadRequest("ids is required".to_string()));
    }
    if ids.len() > MAX_BATCH_DELETE_IDS {
        return Err(ApiError::BadRequest(format!("Too many ids: max {}", MAX_BATCH_DELETE_IDS)));
    }

    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(ids.len());
    for id in ids {
        if id <= 0 {
            return Err(ApiError::BadRequest(format!("Invalid file id: {}", id)));
        }
        if seen.insert(id) {
            normalized.push(id);
        }
    }
    Ok(normalized)
}

fn push_i64_list(qb: &mut QueryBuilder<Sqlite>, ids: &[i64]) {
    let mut separated = qb.separated(", ");
    for id in ids {
        separated.push_bind(*id);
    }
}

async fn query_deletable_files(pool: &SqlitePool, ids: &[i64], auth_user: &AuthUser) -> Result<Vec<File>, sqlx::Error> {
    let is_admin = auth_user.is_admin();
    // Fetch all files by id first
    let mut qb = QueryBuilder::<Sqlite>::new("SELECT * FROM files WHERE id IN (");
    push_i64_list(&mut qb, ids);
    qb.push(")");
    let all_files: Vec<File> = qb.build_query_as::<File>().fetch_all(pool).await?;

    if is_admin {
        return Ok(all_files);
    }

    // Filter: keep files where user has editor permission on the KB, or owns unassigned files.
    // Batch-resolve KB permissions to avoid an N+1 query per file.
    let kb_ids: Vec<i64> = all_files.iter().filter_map(|f| f.kb_id).collect();
    let perms = super::knowledge_base::get_kb_permissions_batch(pool, &kb_ids, &auth_user.user_id, false).await;

    let mut deletable = Vec::new();
    for file in all_files {
        match file.kb_id {
            Some(kb_id) => {
                let perm = perms.get(&kb_id).map(String::as_str);
                if super::knowledge_base::meets_requirement(perm, "editor") {
                    deletable.push(file);
                }
            }
            None => {
                if file.user_id == auth_user.user_id {
                    deletable.push(file);
                }
            }
        }
    }
    Ok(deletable)
}

async fn clear_file_parse_rows_in_tx(tx: &mut sqlx::Transaction<'_, Sqlite>, file_id: i64) -> Result<(), sqlx::Error> {
    clear_file_parse_rows_for_ids_in_tx(tx, &[file_id]).await
}

async fn clear_file_parse_rows_for_ids_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>, file_ids: &[i64],
) -> Result<(), sqlx::Error> {
    if file_ids.is_empty() {
        return Ok(());
    }

    for chunk in file_ids.chunks(SQLITE_DELETE_CHUNK_SIZE) {
        let mut mentions_qb = QueryBuilder::<Sqlite>::new(
            "DELETE FROM entity_mentions WHERE slice_id IN (SELECT id FROM slices WHERE file_id IN (",
        );
        push_i64_list(&mut mentions_qb, chunk);
        mentions_qb.push("))");
        mentions_qb.build().execute(&mut **tx).await?;
    }

    for chunk in file_ids.chunks(SQLITE_DELETE_CHUNK_SIZE) {
        let mut positions_qb = QueryBuilder::<Sqlite>::new(
            "DELETE FROM slice_positions WHERE slice_id IN (SELECT id FROM slices WHERE file_id IN (",
        );
        push_i64_list(&mut positions_qb, chunk);
        positions_qb.push("))");
        positions_qb.build().execute(&mut **tx).await?;
    }

    for chunk in file_ids.chunks(SQLITE_DELETE_CHUNK_SIZE) {
        let mut slices_qb = QueryBuilder::<Sqlite>::new("DELETE FROM slices WHERE file_id IN (");
        push_i64_list(&mut slices_qb, chunk);
        slices_qb.push(")");
        slices_qb.build().execute(&mut **tx).await?;
    }

    for chunk in file_ids.chunks(SQLITE_DELETE_CHUNK_SIZE) {
        let mut pdf_qb = QueryBuilder::<Sqlite>::new("DELETE FROM pdf_contents WHERE file_id IN (");
        push_i64_list(&mut pdf_qb, chunk);
        pdf_qb.push(")");
        pdf_qb.build().execute(&mut **tx).await?;
    }

    Ok(())
}

pub(crate) async fn delete_file_rows_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>, file_ids: &[i64],
) -> Result<(), sqlx::Error> {
    clear_file_parse_rows_for_ids_in_tx(tx, file_ids).await?;

    for chunk in file_ids.chunks(SQLITE_DELETE_CHUNK_SIZE) {
        let mut files_qb = QueryBuilder::<Sqlite>::new("DELETE FROM files WHERE id IN (");
        push_i64_list(&mut files_qb, chunk);
        files_qb.push(")");
        files_qb.build().execute(&mut **tx).await?;
    }

    Ok(())
}

/// 每批文件在独立事务里删除的批大小。SQLite 写串行，单个超大事务会长时间持有写锁、
/// 阻塞其它写入；分批提交把锁持有时间限制在每批范围内。
const FILE_DELETE_TX_BATCH_SIZE: usize = 500;

/// 分批删除文件相关行，每批独立提交。
///
/// 与 [`delete_file_rows_in_tx`] 的区别：后者把全部删除塞进调用方的单个事务（适合少量文件、
/// 需要与其它操作原子提交的场景）；本函数用于删除可能极多的文件（如删除整个知识库），
/// 牺牲整体原子性换取更短的写锁持有时间。失败时已提交的批次不会回滚，重试可继续。
pub(crate) async fn delete_file_rows_batched(pool: &SqlitePool, file_ids: &[i64]) -> Result<(), sqlx::Error> {
    for batch in file_ids.chunks(FILE_DELETE_TX_BATCH_SIZE) {
        let mut tx = pool.begin().await?;
        delete_file_rows_in_tx(&mut tx, batch).await?;
        tx.commit().await?;
    }
    Ok(())
}

async fn ensure_kb_editor_or_admin(pool: &SqlitePool, kb_id: i64, auth_user: &AuthUser) -> ApiResult<()> {
    let perm = super::knowledge_base::get_kb_permission(pool, kb_id, &auth_user.user_id, auth_user.is_admin()).await;
    if !super::knowledge_base::meets_requirement(perm.as_deref(), "editor") {
        return Err(ApiError::Forbidden("Permission denied. Requires editor or admin.".to_string()));
    }
    Ok(())
}

async fn query_failed_file_ids_for_reparse(
    pool: &SqlitePool, auth_user: &AuthUser, req: &ReparseFailedFilesReq,
) -> ApiResult<Vec<i64>> {
    let is_admin = auth_user.is_admin();

    if req.unassigned_only {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT f.id FROM files f WHERE f.status = -1 AND f.kb_id IS NULL");
        if !is_admin {
            qb.push(" AND f.user_id = ").push_bind(&auth_user.user_id);
        }
        qb.push(" ORDER BY f.updated_at DESC");
        let ids = qb.build_query_scalar::<i64>().fetch_all(pool).await?;
        return Ok(ids);
    }

    if let Some(kb_id) = req.kb_id {
        ensure_kb_editor_or_admin(pool, kb_id, auth_user).await?;

        if req.include_descendants {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "WITH RECURSIVE descendants AS (SELECT id, kb_type FROM knowledge_bases WHERE id = ",
            );
            qb.push_bind(kb_id);
            qb.push(
                " UNION ALL SELECT kb.id, kb.kb_type FROM knowledge_bases kb JOIN descendants d ON kb.parent_id = d.id)",
            );
            qb.push(" SELECT id, kb_type FROM descendants WHERE kb_type != ");
            qb.push_bind("storage");
            let rows: Vec<(i64, String)> = qb.build_query_as().fetch_all(pool).await?;
            let desc_kb_ids: Vec<i64> = rows.iter().map(|(id, _)| *id).collect();
            let perms =
                super::knowledge_base::get_kb_permissions_batch(pool, &desc_kb_ids, &auth_user.user_id, is_admin).await;
            let mut allowed_kb_ids: Vec<i64> = Vec::new();
            for (desc_kb_id, _) in rows {
                let perm = perms.get(&desc_kb_id).map(String::as_str);
                if super::knowledge_base::meets_requirement(perm, "editor") {
                    allowed_kb_ids.push(desc_kb_id);
                }
            }
            if allowed_kb_ids.is_empty() {
                return Ok(Vec::new());
            }
            let mut qb2 = QueryBuilder::<Sqlite>::new("SELECT f.id FROM files f WHERE f.status = -1 AND f.kb_id IN (");
            let mut sep = qb2.separated(", ");
            for id in &allowed_kb_ids {
                sep.push_bind(id);
            }
            qb2.push(") ORDER BY f.updated_at DESC");
            let ids = qb2.build_query_scalar::<i64>().fetch_all(pool).await?;
            return Ok(ids);
        }

        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT f.id FROM files f \
             JOIN knowledge_bases kb ON kb.id = f.kb_id \
             WHERE f.status = -1 AND f.kb_id = ",
        );
        qb.push_bind(kb_id);
        qb.push(" AND kb.kb_type != ").push_bind("storage");
        if !is_admin {
            qb.push(" AND f.user_id = ").push_bind(&auth_user.user_id);
        }
        qb.push(" ORDER BY f.updated_at DESC");
        let ids = qb.build_query_scalar::<i64>().fetch_all(pool).await?;
        return Ok(ids);
    }

    // Global reparse-failed: only user's own files + files in KBs where user has editor permission
    let mut qb = QueryBuilder::<Sqlite>::new(
        "SELECT f.id, f.kb_id FROM files f \
         LEFT JOIN knowledge_bases kb ON kb.id = f.kb_id \
         WHERE f.status = -1 AND (f.kb_id IS NULL OR kb.kb_type != ",
    );
    qb.push_bind("storage");
    qb.push(")");
    if !req.include_unassigned {
        qb.push(" AND f.kb_id IS NOT NULL");
    }
    qb.push(" ORDER BY f.updated_at DESC");
    let rows: Vec<(i64, Option<i64>)> = qb.build_query_as().fetch_all(pool).await?;

    // Batch-resolve permissions for all assigned KBs to avoid an N+1 query per file.
    let assigned_kb_ids: Vec<i64> = rows.iter().filter_map(|(_, kb)| *kb).collect();
    let perms =
        super::knowledge_base::get_kb_permissions_batch(pool, &assigned_kb_ids, &auth_user.user_id, is_admin).await;

    let mut allowed_ids = Vec::new();
    for (file_id, file_kb_id) in rows {
        match file_kb_id {
            None => {
                // unassigned file: only own files
                if is_admin {
                    allowed_ids.push(file_id);
                }
            }
            Some(kid) => {
                let perm = perms.get(&kid).map(String::as_str);
                if super::knowledge_base::meets_requirement(perm, "editor") {
                    allowed_ids.push(file_id);
                }
            }
        }
    }
    Ok(allowed_ids)
}

async fn remove_converted_pdfs(file_ids: &[i64]) {
    if file_ids.is_empty() {
        return;
    }

    let cfg = config::get();
    let pdf_root = std::path::Path::new(&cfg.storage.pdf_path);
    for file_id in file_ids {
        let pdf_path = pdf_root.join(format!("{}.pdf", file_id));
        if let Err(e) = fs::remove_file(&pdf_path).await
            && !matches!(e.kind(), std::io::ErrorKind::NotFound)
        {
            warn!("Failed to delete converted pdf {}: {}", pdf_path.display(), e);
        }
    }
}

async fn execute_reparse_failed(
    pool: &SqlitePool, search_engine: &SearchEngine, auth_user: &AuthUser, req: ReparseFailedFilesReq,
) -> ApiResult<ReparseFailedFilesResp> {
    let file_ids = query_failed_file_ids_for_reparse(pool, auth_user, &req).await?;
    if file_ids.is_empty() {
        return Ok(ReparseFailedFilesResp { file_count: 0 });
    }

    let image_paths = collect_image_paths_for_files(pool, &file_ids).await?;
    search_engine.delete_batch(Some(&file_ids), None).await.map_err(map_search_engine_error)?;

    let mut tx = pool.begin().await?;
    clear_file_parse_rows_for_ids_in_tx(&mut tx, &file_ids).await?;

    let mut qb = QueryBuilder::<Sqlite>::new(
        "UPDATE files SET status = 0, log = '', content = NULL, updated_at = strftime('%s','now') WHERE status = -1 AND id IN (",
    );
    push_i64_list(&mut qb, &file_ids);
    qb.push(")");
    let updated = qb.build().execute(&mut *tx).await?.rows_affected() as i64;
    tx.commit().await?;

    remove_image_files(image_paths).await;
    remove_converted_pdfs(&file_ids).await;

    Ok(ReparseFailedFilesResp { file_count: updated })
}

pub(crate) async fn cleanup_deleted_files(
    search_engine: &SearchEngine, files: &[File], image_paths: Vec<String>,
) -> Vec<BatchDeleteCleanupFailedItem> {
    let mut cleanup_failed = Vec::new();
    let cfg = config::get();
    for file in files {
        if let Err(e) = fs::remove_file(&file.path).await
            && !matches!(e.kind(), std::io::ErrorKind::NotFound)
        {
            warn!("Failed to delete file {}: {}", file.path, e);
            cleanup_failed.push(BatchDeleteCleanupFailedItem {
                id: file.id,
                stage: "file".to_string(),
                error: e.to_string(),
            });
        }

        let pdf_path = std::path::Path::new(&cfg.storage.pdf_path).join(format!("{}.pdf", file.id));
        if let Err(e) = fs::remove_file(&pdf_path).await
            && !matches!(e.kind(), std::io::ErrorKind::NotFound)
        {
            warn!("Failed to delete converted pdf {}: {}", pdf_path.display(), e);
            cleanup_failed.push(BatchDeleteCleanupFailedItem {
                id: file.id,
                stage: "pdf".to_string(),
                error: e.to_string(),
            });
        }

        // 清理压缩文件解压目录
        let archive_dir = std::path::Path::new(&cfg.storage.archives_path).join(file.id.to_string());
        if archive_dir.exists()
            && let Err(e) = tokio::fs::remove_dir_all(&archive_dir).await
        {
            warn!("Failed to delete archive dir {}: {}", archive_dir.display(), e);
            cleanup_failed.push(BatchDeleteCleanupFailedItem {
                id: file.id,
                stage: "archive".to_string(),
                error: e.to_string(),
            });
        }

        if let Err(e) = search_engine.delete(Some(file.id), None).await {
            warn!("Failed to delete search index for file {}: {}", file.id, e);
            cleanup_failed.push(BatchDeleteCleanupFailedItem {
                id: file.id,
                stage: "search".to_string(),
                error: e.to_string(),
            });
        }
    }

    remove_image_files(image_paths).await;
    cleanup_failed
}

async fn execute_batch_delete(
    pool: &SqlitePool, search_engine: &SearchEngine, auth_user: &AuthUser, ids: Vec<i64>, strict: bool,
    allow_processing: bool,
) -> ApiResult<BatchDeleteFilesResp> {
    let overall_start = Instant::now();
    let ids = normalize_batch_delete_ids(ids)?;
    let requested = ids.len() as i64;

    let step_start = Instant::now();
    let deletable_files = query_deletable_files(pool, &ids, auth_user).await?;
    debug!(
        "file_batch_delete query_deletable count={} requested={} {}ms",
        deletable_files.len(),
        ids.len(),
        step_start.elapsed().as_millis()
    );

    let mut file_map: HashMap<i64, File> = deletable_files.into_iter().map(|file| (file.id, file)).collect();
    let mut files_to_delete = Vec::new();
    let mut skipped = Vec::new();
    for id in &ids {
        let Some(file) = file_map.remove(id) else {
            skipped.push(BatchDeleteSkippedItem { id: *id, reason: "not_found_or_no_permission".to_string() });
            continue;
        };
        if !allow_processing && file.status == 2 {
            skipped.push(BatchDeleteSkippedItem { id: *id, reason: "processing".to_string() });
            continue;
        }
        files_to_delete.push(file);
    }

    if strict && !skipped.is_empty() {
        let ids = skipped.iter().map(|item| item.id.to_string()).collect::<Vec<_>>().join(", ");
        return Err(ApiError::BadRequest(format!("Batch delete precheck failed for ids: {}", ids)));
    }

    let file_ids: Vec<i64> = files_to_delete.iter().map(|file| file.id).collect();
    let accepted = file_ids.len() as i64;
    if file_ids.is_empty() {
        return Ok(BatchDeleteFilesResp {
            requested,
            accepted,
            deleted: 0,
            deleted_ids: Vec::new(),
            skipped,
            cleanup_failed: Vec::new(),
        });
    }

    let step_start = Instant::now();
    let image_paths = collect_image_paths_for_files(pool, &file_ids).await?;
    debug!("file_batch_delete collect_image_paths ids={} {}ms", file_ids.len(), step_start.elapsed().as_millis());

    let step_start = Instant::now();
    let mut tx = pool.begin().await?;
    delete_file_rows_in_tx(&mut tx, &file_ids).await?;
    tx.commit().await?;
    debug!("file_batch_delete delete_rows ids={} {}ms", file_ids.len(), step_start.elapsed().as_millis());

    let step_start = Instant::now();
    let cleanup_failed = cleanup_deleted_files(search_engine, &files_to_delete, image_paths).await;
    debug!(
        "file_batch_delete cleanup ids={} failed={} {}ms",
        file_ids.len(),
        cleanup_failed.len(),
        step_start.elapsed().as_millis()
    );

    debug!(
        "file_batch_delete total requested={} accepted={} deleted={} {}ms",
        requested,
        accepted,
        file_ids.len(),
        overall_start.elapsed().as_millis()
    );

    Ok(BatchDeleteFilesResp {
        requested,
        accepted,
        deleted: file_ids.len() as i64,
        deleted_ids: file_ids,
        skipped,
        cleanup_failed,
    })
}

#[derive(Default)]
struct FileImagePathState {
    has_pdf_contents: bool,
    meta_checked: bool,
    paths: Vec<String>,
}

pub(crate) async fn collect_image_raw_paths_for_files(
    pool: &SqlitePool, file_ids: &[i64],
) -> Result<Vec<String>, sqlx::Error> {
    if file_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut states: HashMap<i64, FileImagePathState> =
        file_ids.iter().copied().map(|file_id| (file_id, FileImagePathState::default())).collect();

    for chunk in file_ids.chunks(SQLITE_DELETE_CHUNK_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT file_id, img_path FROM pdf_contents WHERE file_id IN (");
        push_i64_list(&mut qb, chunk);
        qb.push(")");
        let rows: Vec<(i64, Option<String>)> = qb.build_query_as().fetch_all(pool).await?;
        for (file_id, img_path) in rows {
            if let Some(state) = states.get_mut(&file_id) {
                state.has_pdf_contents = true;
                if let Some(path) = img_path {
                    let trimmed = path.trim();
                    if !trimmed.is_empty() {
                        state.paths.push(trimmed.to_string());
                    }
                }
            }
        }
    }

    let meta_candidate_ids: Vec<i64> =
        file_ids.iter().copied().filter(|id| !states.get(id).is_some_and(|state| state.has_pdf_contents)).collect();
    for chunk in meta_candidate_ids.chunks(SQLITE_DELETE_CHUNK_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT id, meta FROM files WHERE id IN (");
        push_i64_list(&mut qb, chunk);
        qb.push(")");
        let rows: Vec<(i64, Option<String>)> = qb.build_query_as().fetch_all(pool).await?;
        for (file_id, meta) in rows {
            if let Some(paths) = extract_custom_image_paths_from_meta(meta.as_deref())
                && let Some(state) = states.get_mut(&file_id)
            {
                state.meta_checked = true;
                state.paths.extend(paths);
            }
        }
    }

    let regex_candidate_ids: Vec<i64> = file_ids
        .iter()
        .copied()
        .filter(|id| states.get(id).is_some_and(|state| !state.has_pdf_contents && !state.meta_checked))
        .collect();
    let mut regex_paths = extract_slice_image_paths_by_file(pool, &regex_candidate_ids).await?;
    for file_id in regex_candidate_ids {
        if let Some(paths) = regex_paths.remove(&file_id)
            && let Some(state) = states.get_mut(&file_id)
        {
            state.paths.extend(paths);
        }
    }

    let mut raw_paths = Vec::new();
    let mut seen = HashSet::new();
    for file_id in file_ids {
        let Some(state) = states.get(file_id) else { continue };
        for path in &state.paths {
            let trimmed = path.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                raw_paths.push(trimmed.to_string());
            }
        }
    }
    Ok(raw_paths)
}

pub(crate) async fn collect_image_paths_for_files(
    pool: &SqlitePool, file_ids: &[i64],
) -> Result<Vec<String>, sqlx::Error> {
    let raw_paths = collect_image_raw_paths_for_files(pool, file_ids).await?;
    let mut resolved = Vec::new();
    for raw in raw_paths {
        if let Some(path) = resolve_image_storage_path(&raw) {
            resolved.push(path);
        }
    }
    Ok(resolved)
}

pub(crate) async fn update_file_custom_image_meta(
    pool: &SqlitePool, file_id: i64, image_paths: &[String], source: &str,
) -> AnyResult<()> {
    let current_meta: Option<String> =
        sqlx::query_scalar("SELECT meta FROM files WHERE id = ?").bind(file_id).fetch_optional(pool).await?;
    let merged_meta = merge_custom_image_meta(current_meta.as_deref(), image_paths, source);
    sqlx::query("UPDATE files SET meta = ? WHERE id = ?").bind(merged_meta).bind(file_id).execute(pool).await?;
    Ok(())
}

pub(crate) async fn backfill_missing_image_meta_for_files(
    pool: &SqlitePool, file_ids: &[i64], source: &str,
) -> AnyResult<usize> {
    if file_ids.is_empty() {
        return Ok(0);
    }

    let mut candidate_ids = Vec::new();
    for chunk in file_ids.chunks(SQLITE_DELETE_CHUNK_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT f.id, f.meta FROM files f WHERE f.id IN (");
        push_i64_list(&mut qb, chunk);
        qb.push(") AND NOT EXISTS (SELECT 1 FROM pdf_contents pc WHERE pc.file_id = f.id)");
        let rows: Vec<(i64, Option<String>)> = qb.build_query_as().fetch_all(pool).await?;
        for (file_id, meta) in rows {
            if extract_custom_image_paths_from_meta(meta.as_deref()).is_none() {
                candidate_ids.push(file_id);
            }
        }
    }

    if candidate_ids.is_empty() {
        return Ok(0);
    }

    let mut extracted = extract_slice_image_paths_by_file(pool, &candidate_ids).await?;
    let mut updated = 0usize;
    for file_id in candidate_ids {
        let paths = extracted.remove(&file_id).unwrap_or_default();
        update_file_custom_image_meta(pool, file_id, &paths, source).await?;
        updated += 1;
    }
    Ok(updated)
}

fn extract_custom_image_paths_from_meta(meta: Option<&str>) -> Option<Vec<String>> {
    let raw = meta?.trim();
    if raw.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let custom_images = value.get(PARSE_ASSETS_META_KEY)?.get(CUSTOM_IMAGES_META_KEY)?;
    if !custom_images.get("checked").and_then(|v| v.as_bool()).unwrap_or(false) {
        return None;
    }

    let mut paths = Vec::new();
    if let Some(raw_paths) = custom_images.get("paths").and_then(|v| v.as_array()) {
        for path in raw_paths {
            if let Some(path) = path.as_str() {
                let trimmed = path.trim();
                if !trimmed.is_empty() {
                    paths.push(trimmed.to_string());
                }
            }
        }
    }
    Some(paths)
}

fn merge_custom_image_meta(meta: Option<&str>, image_paths: &[String], source: &str) -> String {
    let mut root = parse_meta_object(meta);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();

    let mut unique_paths = Vec::new();
    let mut seen = HashSet::new();
    for path in image_paths {
        let trimmed = path.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
            unique_paths.push(trimmed.to_string());
        }
    }

    let parse_assets = root.entry(PARSE_ASSETS_META_KEY.to_string()).or_insert_with(|| serde_json::json!({}));
    if !parse_assets.is_object() {
        *parse_assets = serde_json::json!({});
    }
    let parse_assets_obj = parse_assets.as_object_mut().expect("parse assets meta is object");
    parse_assets_obj.insert("version".to_string(), serde_json::json!(CUSTOM_IMAGES_META_VERSION));
    parse_assets_obj.insert(
        CUSTOM_IMAGES_META_KEY.to_string(),
        serde_json::json!({
            "source": source,
            "paths": unique_paths,
            "checked": true,
            "updated_at": now
        }),
    );

    serde_json::Value::Object(root).to_string()
}

fn parse_meta_object(meta: Option<&str>) -> serde_json::Map<String, serde_json::Value> {
    let Some(raw) = meta.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return serde_json::Map::new();
    };

    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(serde_json::Value::Object(map)) => map,
        Ok(value) => {
            let mut map = serde_json::Map::new();
            map.insert("_legacy_meta".to_string(), value);
            map
        }
        Err(_) => {
            let mut map = serde_json::Map::new();
            map.insert("_legacy_meta".to_string(), serde_json::Value::String(raw.to_string()));
            map
        }
    }
}

async fn extract_slice_image_paths_by_file(
    pool: &SqlitePool, file_ids: &[i64],
) -> Result<HashMap<i64, Vec<String>>, sqlx::Error> {
    let mut result: HashMap<i64, Vec<String>> = HashMap::new();
    if file_ids.is_empty() {
        return Ok(result);
    }

    for chunk in file_ids.chunks(SQLITE_DELETE_CHUNK_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT file_id, content FROM slices WHERE file_id IN (");
        push_i64_list(&mut qb, chunk);
        qb.push(")");
        let rows: Vec<(i64, String)> = qb.build_query_as().fetch_all(pool).await?;
        for (file_id, content) in rows {
            let paths = result.entry(file_id).or_default();
            for path in extract_image_paths_from_text(&content) {
                if !paths.iter().any(|existing| existing == &path) {
                    paths.push(path);
                }
            }
        }
    }

    Ok(result)
}

fn extract_image_paths_from_text(content: &str) -> Vec<String> {
    static IMAGE_REF_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = IMAGE_REF_RE.get_or_init(|| {
        regex::Regex::new(r"/api/v1/knowledge/files/([^\s'\)>]+)").expect("valid image reference regex")
    });

    let mut paths = Vec::new();
    for cap in re.captures_iter(content) {
        let Some(matched) = cap.get(1) else { continue };
        let path = matched.as_str().trim();
        if !path.is_empty() && !paths.iter().any(|existing| existing == path) {
            paths.push(path.to_string());
        }
    }
    paths
}

fn is_safe_relative_path(path: &str) -> bool {
    let path = std::path::Path::new(path);
    if path.is_absolute() {
        return false;
    }
    for component in path.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
            _ => {}
        }
    }
    true
}

pub(crate) fn resolve_image_storage_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !is_safe_relative_path(trimmed) {
        log::warn!("Skipping unsafe image path: {}", trimmed);
        return None;
    }

    let cfg = config::get();
    let images_root = std::path::Path::new(&cfg.storage.images_path);

    // 图片实际存储路径为 images_path/trimmed
    // （与 save_mineru_images 的 "{images_path}/{img_name}" 一致）
    let resolved = images_root.join(trimmed);

    Some(resolved.to_string_lossy().to_string())
}

pub(crate) async fn find_reusable_parsed_file(
    pool: &SqlitePool, hash: &str, slice_type: &str, exclude_file_id: Option<i64>,
) -> Result<Option<File>, sqlx::Error> {
    let base_sql = "SELECT * FROM files WHERE hash = ? AND slice_type = ? AND status = 1";
    let sql = if exclude_file_id.is_some() {
        format!("{} AND id != ? ORDER BY updated_at DESC LIMIT 1", base_sql)
    } else {
        format!("{} ORDER BY updated_at DESC LIMIT 1", base_sql)
    };

    let mut query = sqlx::query_as::<_, File>(&sql).bind(hash).bind(slice_type);
    if let Some(file_id) = exclude_file_id {
        query = query.bind(file_id);
    }

    query.fetch_optional(pool).await
}

pub(crate) async fn remove_image_files(image_paths: Vec<String>) {
    info!("Removing image files: {:?}", image_paths);
    if image_paths.is_empty() {
        return;
    }

    let cfg = config::get();
    let images_root = std::path::Path::new(&cfg.storage.images_path);
    let data_root = images_root.parent();
    let mut seen = HashSet::new();
    for img_path in image_paths {
        let trimmed = img_path.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }

        let candidate = std::path::Path::new(trimmed);
        let full_path = if candidate.is_absolute()
            || candidate.starts_with(images_root)
            || data_root.is_some_and(|root| candidate.starts_with(root))
        {
            candidate.to_path_buf()
        } else {
            match resolve_image_storage_path(trimmed) {
                Some(path) => std::path::PathBuf::from(path),
                None => continue,
            }
        };
        let allowed = full_path.starts_with(images_root) || data_root.is_some_and(|root| full_path.starts_with(root));
        if !allowed {
            log::warn!("Skipping unsafe image path: {}", full_path.display());
            continue;
        }
        if let Err(e) = fs::remove_file(&full_path).await {
            if matches!(e.kind(), std::io::ErrorKind::NotFound) {
                if !candidate.is_absolute()
                    && (trimmed.contains('/') || trimmed.contains('\\'))
                    && let Some(file_name) = candidate.file_name()
                {
                    let fallback_path = images_root.join(file_name);
                    if let Err(e) = fs::remove_file(&fallback_path).await
                        && !matches!(e.kind(), std::io::ErrorKind::NotFound)
                    {
                        log::warn!("Failed to delete image {}: {}", fallback_path.display(), e);
                    }
                }
            } else {
                log::warn!("Failed to delete image {}: {}", full_path.display(), e);
            }
        }
    }
}

#[derive(Deserialize, IntoParams)]
pub struct ListQuery {
    pub kb_id: Option<String>,
    pub tag: Option<String>,
}

#[derive(Deserialize, IntoParams)]
pub struct FileStatsQuery {
    /// 指定知识库 ID，统计该知识库以及（可选）子知识库
    pub kb_id: Option<String>,
    /// 是否包含子知识库文件，默认 true
    pub include_descendants: Option<bool>,
    /// 全局统计时是否包含未分配知识库的文件，默认 true
    pub include_unassigned: Option<bool>,
}

/// 获取文件列表
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/files/",
    operation_id = "file_list",
    tag = "file",
    params(ListQuery),
    responses(
        (status = 200, description = "成功返回文件列表", body = Vec<File>),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn list(
    State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>, Query(query): Query<ListQuery>,
) -> ApiResult<Json<Vec<File>>> {
    let is_admin = auth_user.is_admin();
    let mut files = match query.kb_id.as_deref() {
        // 明确指定查询未分配知识库的文件
        Some("null") | Some("unassigned") => {
            if is_admin {
                sqlx::query_as(&format!(
                    "SELECT {FILE_COLS_NO_CONTENT} FROM files WHERE kb_id IS NULL ORDER BY created_at DESC"
                ))
                .fetch_all(&pool)
                .await?
            } else {
                sqlx::query_as(
                    &format!("SELECT {FILE_COLS_NO_CONTENT} FROM files WHERE kb_id IS NULL AND (user_id = ? OR is_public = 1) ORDER BY created_at DESC"),
                )
                .bind(&auth_user.user_id)
                .fetch_all(&pool)
                .await?
            }
        }
        // 查询特定知识库的文件
        Some(kb_id_str) => {
            let kb_id = kb_id_str.parse::<i64>().map_err(|_| ApiError::internal("Invalid kb_id format"))?;
            ensure_kb_accessible(&pool, kb_id, &auth_user.user_id, is_admin).await?;
            if is_admin {
                sqlx::query_as(&format!(
                    "SELECT {FILE_COLS_NO_CONTENT} FROM files WHERE kb_id = ? ORDER BY created_at DESC"
                ))
                .bind(kb_id)
                .fetch_all(&pool)
                .await?
            } else {
                sqlx::query_as(
                    &format!("SELECT {FILE_COLS_NO_CONTENT} FROM files WHERE kb_id = ? AND (user_id = ? OR is_public = 1) ORDER BY created_at DESC"),
                )
                .bind(kb_id)
                .bind(&auth_user.user_id)
                .fetch_all(&pool)
                .await?
            }
        }
        // 不传参数，查询所有文件
        None => {
            if is_admin {
                sqlx::query_as(&format!("SELECT {FILE_COLS_NO_CONTENT} FROM files ORDER BY created_at DESC"))
                    .fetch_all(&pool)
                    .await?
            } else {
                sqlx::query_as(&format!("SELECT {FILE_COLS_NO_CONTENT} FROM files WHERE (user_id = ? OR is_public = 1) ORDER BY created_at DESC"))
                    .bind(&auth_user.user_id)
                    .fetch_all(&pool)
                    .await?
            }
        }
    };

    if let Some(tag) = &query.tag {
        files.retain(|file: &File| {
            if let Ok(tags) = serde_json::from_str::<Vec<String>>(&file.tags) { tags.contains(tag) } else { false }
        });
    }

    for file in &mut files {
        file.content = None;
    }

    Ok(Json(files))
}

#[utoipa::path(
    get,
    path = "/api/v1/knowledge/files/stats",
    operation_id = "file_stats",
    tag = "file",
    params(FileStatsQuery),
    responses(
        (status = 200, description = "成功返回文件状态统计", body = FileStatusBreakdown),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn stats(
    State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>, Query(query): Query<FileStatsQuery>,
) -> ApiResult<Json<FileStatusBreakdown>> {
    let is_admin = auth_user.is_admin();
    let include_descendants = query.include_descendants.unwrap_or(true);
    let include_unassigned = query.include_unassigned.unwrap_or(true);

    let breakdown = match query.kb_id.as_deref() {
        Some("null") | Some("unassigned") => {
            get_file_status_breakdown_for_unassigned(&pool, &auth_user.user_id, is_admin).await?
        }
        Some(kb_id_str) => {
            let kb_id =
                kb_id_str.parse::<i64>().map_err(|_| ApiError::BadRequest("Invalid kb_id format".to_string()))?;
            ensure_kb_accessible(&pool, kb_id, &auth_user.user_id, is_admin).await?;
            get_file_status_breakdown_for_kb(&pool, kb_id, include_descendants, &auth_user.user_id, is_admin).await?
        }
        None => get_file_status_breakdown_for_all(&pool, include_unassigned, &auth_user.user_id, is_admin).await?,
    };

    Ok(Json(breakdown))
}

async fn ensure_kb_accessible(pool: &SqlitePool, kb_id: i64, user_id: &str, is_admin: bool) -> ApiResult<()> {
    let exists = if is_admin {
        sqlx::query_scalar::<_, i64>("SELECT id FROM knowledge_bases WHERE id = ?")
            .bind(kb_id)
            .fetch_optional(pool)
            .await?
    } else {
        sqlx::query_scalar::<_, i64>("SELECT id FROM knowledge_bases WHERE id = ? AND (user_id = ? OR is_public = 1)")
            .bind(kb_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?
    };

    if exists.is_none() {
        return Err(ApiError::NotFound("Knowledge base not found or permission denied".to_string()));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
pub struct Slice {
    pub id: i64,
    pub file_id: i64,
    pub content: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[sqlx(skip)]
    pub positions: Option<Vec<SlicePosition>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SlicePosition {
    pub page_idx: i32,
    pub bbox: [i32; 4],
    pub sheet_name: Option<String>,
    pub row_num: Option<i32>,
}

#[derive(Debug, sqlx::FromRow)]
struct SlicePositionRow {
    slice_id: i64,
    page_idx: i32,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    sheet_name: Option<String>,
    row_num: Option<i32>,
}

#[derive(Debug, sqlx::FromRow)]
struct PageBBoxRow {
    page_idx: i32,
    bbox: String,
}

/// 获取文件的所有切片
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/files/{id}/slices",
    operation_id = "file_get_slices",
    tag = "file",
    params(
        ("id" = i64, Path, description = "文件 ID")
    ),
    responses(
        (status = 200, description = "成功返回切片列表", body = Vec<Slice>),
        (status = 404, description = "文件不存在")
    )
)]
pub async fn get_slices(State(pool): State<SqlitePool>, Path(id): Path<i64>) -> ApiResult<Json<Vec<Slice>>> {
    let mut slices: Vec<Slice> =
        sqlx::query_as("SELECT * FROM slices WHERE file_id = ? ORDER BY id").bind(id).fetch_all(&pool).await?;
    if slices.is_empty() {
        return Ok(Json(slices));
    }

    let slice_ids: Vec<i64> = slices.iter().map(|s| s.id).collect();
    let mut qb = QueryBuilder::<Sqlite>::new(
        "SELECT slice_id, page_idx, x1, y1, x2, y2, sheet_name, row_num FROM slice_positions WHERE slice_id IN (",
    );
    let mut separated = qb.separated(", ");
    for slice_id in slice_ids {
        separated.push_bind(slice_id);
    }
    qb.push(") ORDER BY slice_id, page_idx, id");

    let rows: Vec<SlicePositionRow> = qb.build_query_as().fetch_all(&pool).await?;
    let mut position_map: std::collections::HashMap<i64, Vec<SlicePosition>> = std::collections::HashMap::new();
    for row in rows {
        position_map.entry(row.slice_id).or_default().push(SlicePosition {
            page_idx: row.page_idx,
            bbox: [row.x1, row.y1, row.x2, row.y2],
            sheet_name: row.sheet_name,
            row_num: row.row_num,
        });
    }

    for slice in &mut slices {
        slice.positions = position_map.get(&slice.id).cloned();
    }

    Ok(Json(slices))
}

/// 获取存储目录中的图片
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/files/images/{filename}",
    operation_id = "file_get_image_by_filename",
    tag = "file",
    params(
        ("filename" = String, Path, description = "图片文件名")
    ),
    responses(
        (status = 200, description = "成功返回图片文件", content_type = "image/*"),
        (status = 400, description = "请求参数错误"),
        (status = 404, description = "图片不存在")
    )
)]
pub async fn get_image_by_filename(
    Path(filename): Path<String>,
) -> Result<(StatusCode, [(header::HeaderName, String); 2], Body), ApiError> {
    let mut components = std::path::Path::new(&filename).components();
    match components.next() {
        Some(Component::Normal(_)) if components.next().is_none() => {}
        _ => return Err(ApiError::BadRequest("Invalid filename".to_string())),
    }

    let cfg = config::get();
    let image_path = std::path::Path::new(&cfg.storage.images_path).join(&filename);
    if !fs::try_exists(&image_path).await? {
        return Err(ApiError::NotFound("Image not found".to_string()));
    }

    let (len, body) = open_file_stream(&image_path).await?;
    let mime_type = mime_guess::from_path(&filename).first_or_octet_stream().to_string();

    Ok((StatusCode::OK, [(header::CONTENT_TYPE, mime_type), (header::CONTENT_LENGTH, len.to_string())], body))
}

/// 下载文件
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/files/{id}/download",
    operation_id = "file_download",
    tag = "file",
    params(
        ("id" = i64, Path, description = "文件 ID")
    ),
    responses(
        (status = 200, description = "成功下载文件", content_type = "application/octet-stream"),
        (status = 404, description = "文件不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn download(
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Extension(auth_user): Extension<AuthUser>,
) -> Result<(StatusCode, [(header::HeaderName, String); 3], Body), ApiError> {
    let file: File = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;

    if !auth_user.is_admin() && !file.is_public && file.user_id != auth_user.user_id {
        return Err(ApiError::NotFound("File not found or permission denied".to_string()));
    }

    let (len, body) = open_file_stream(std::path::Path::new(&file.path)).await?;
    let mime_type = mime_guess::from_path(&file.filename).first_or_octet_stream().to_string();
    let content_disposition = format!("attachment; filename=\"{}\"", file.filename);

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime_type),
            (header::CONTENT_DISPOSITION, content_disposition),
            (header::CONTENT_LENGTH, len.to_string()),
        ],
        body,
    ))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct HighlightQuery {
    /// Base64 编码的 positions JSON 数组
    pub positions: Option<String>,
    /// 切片 ID（如果不传 positions，则从数据库查询）
    pub slice_id: Option<i64>,
}

/// 获取带高亮标注的 PDF
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/files/{id}/highlighted-pdf",
    operation_id = "file_get_highlighted_pdf",
    tag = "file",
    params(
        ("id" = i64, Path, description = "文件 ID"),
        HighlightQuery,
    ),
    responses(
        (status = 200, description = "成功返回带高亮的 PDF", content_type = "application/pdf"),
        (status = 400, description = "请求参数错误"),
        (status = 404, description = "文件不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn get_highlighted_pdf(
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Query(params): Query<HighlightQuery>,
    Extension(auth_user): Extension<AuthUser>,
) -> Result<(StatusCode, [(header::HeaderName, String); 2], Body), ApiError> {
    let file: File = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;

    if !auth_user.is_admin() && !file.is_public && file.user_id != auth_user.user_id {
        return Err(ApiError::NotFound("File not found or permission denied".to_string()));
    }

    // 解析高亮位置
    let positions: Vec<pdf_highlight::HighlightPosition> = if let Some(positions_b64) = &params.positions {
        // 从 Base64 解码 positions
        let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, positions_b64)
            .map_err(|e| ApiError::BadRequest(format!("Invalid base64 positions: {}", e)))?;

        serde_json::from_slice(&decoded).map_err(|e| ApiError::BadRequest(format!("Invalid positions JSON: {}", e)))?
    } else if let Some(slice_id) = params.slice_id {
        // 从数据库查询 slice 的 positions
        let rows: Vec<SlicePositionRow> = sqlx::query_as(
            "SELECT slice_id, page_idx, x1, y1, x2, y2, sheet_name, row_num FROM slice_positions WHERE slice_id = ? ORDER BY page_idx, id",
        )
        .bind(slice_id)
        .fetch_all(&pool)
        .await?;

        rows.iter()
            .map(|row| pdf_highlight::HighlightPosition {
                page_idx: row.page_idx,
                bbox: [row.x1, row.y1, row.x2, row.y2],
            })
            .collect()
    } else {
        Vec::new()
    };

    let coord_bounds_by_page = if positions.is_empty() {
        None
    } else {
        let rows: Vec<PageBBoxRow> = sqlx::query_as(
            "SELECT page_idx, bbox FROM pdf_contents WHERE file_id = ? AND bbox IS NOT NULL AND bbox != ''",
        )
        .bind(file.id)
        .fetch_all(&pool)
        .await?;

        let mut bounds: HashMap<i32, pdf_highlight::PageCoordBounds> = HashMap::new();
        for row in rows {
            if let Ok(bbox) = serde_json::from_str::<Vec<f32>>(&row.bbox)
                && bbox.len() == 4
            {
                let x1 = bbox[0];
                let y1 = bbox[1];
                let x2 = bbox[2];
                let y2 = bbox[3];
                let min_x = x1.min(x2);
                let min_y = y1.min(y2);
                let max_x = x1.max(x2);
                let max_y = y1.max(y2);

                bounds
                    .entry(row.page_idx)
                    .and_modify(|b| {
                        b.min_x = b.min_x.min(min_x);
                        b.min_y = b.min_y.min(min_y);
                        b.max_x = b.max_x.max(max_x);
                        b.max_y = b.max_y.max(max_y);
                    })
                    .or_insert(pdf_highlight::PageCoordBounds { min_x, min_y, max_x, max_y });
            }
        }

        bounds.retain(|_, b| {
            b.min_x.is_finite()
                && b.min_y.is_finite()
                && b.max_x.is_finite()
                && b.max_y.is_finite()
                && b.max_x > b.min_x
                && b.max_y > b.min_y
        });

        if bounds.is_empty() { None } else { Some(bounds) }
    };

    // 确定 PDF 文件路径
    let filename_lower = file.filename.to_lowercase();
    let pdf_path = if filename_lower.ends_with(".doc")
        || filename_lower.ends_with(".docx")
        || filename_lower.ends_with(".ppt")
        || filename_lower.ends_with(".pptx")
        || filename_lower.ends_with(".xls")
        || filename_lower.ends_with(".xlsx")
    {
        let cfg = config::get();
        let path = std::path::Path::new(&cfg.storage.pdf_path).join(format!("{}.pdf", file.id));
        if !tokio::fs::try_exists(&path).await? {
            return Err(ApiError::NotFound("Converted PDF not found".to_string()));
        }
        path
    } else if filename_lower.ends_with(".pdf") {
        std::path::PathBuf::from(&file.path)
    } else {
        return Err(ApiError::BadRequest("File is not a PDF, Word, PowerPoint, or Excel document".to_string()));
    };

    // 读取原始 PDF
    let pdf_bytes = tokio::fs::read(&pdf_path).await?;

    // 添加高亮标注
    let highlighted_pdf = if positions.is_empty() {
        pdf_bytes
    } else {
        pdf_highlight::add_highlights_to_pdf_with_bounds(&pdf_bytes, &positions, coord_bounds_by_page.as_ref())
            .map_err(|e| ApiError::Internal(format!("Failed to add highlights: {}", e)))?
    };

    let content_disposition = format!("inline; filename=\"highlighted_{}.pdf\"", file.id);

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/pdf".to_string()), (header::CONTENT_DISPOSITION, content_disposition)],
        Body::from(highlighted_pdf),
    ))
}

const EXCEL_MAX_ROWS_PER_SHEET: usize = 10000;

/// 获取 Excel 文件的结构化数据
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/files/{id}/excel-data",
    operation_id = "file_get_excel_data",
    tag = "file",
    params(
        ("id" = i64, Path, description = "文件 ID"),
        ("sheet_name" = Option<String>, Query, description = "指定 sheet 名称"),
        ("row_num" = Option<usize>, Query, description = "指定 Excel 行号（1-based，包含表头行）"),
    ),
    responses(
        (status = 200, description = "成功返回 Excel 数据", body = ExcelData),
        (status = 400, description = "文件不是 Excel"),
        (status = 404, description = "文件不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn excel_data(
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<ExcelDataQuery>,
) -> ApiResult<Json<ExcelData>> {
    let file: File = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;

    if !auth_user.is_admin() && !file.is_public && file.user_id != auth_user.user_id {
        return Err(ApiError::NotFound("File not found or permission denied".to_string()));
    }

    let filename_lower = file.filename.to_lowercase();
    let is_excel = filename_lower.ends_with(".xls") || filename_lower.ends_with(".xlsx");
    if !is_excel {
        return Err(ApiError::BadRequest("File is not an Excel document".to_string()));
    }

    let filter_sheet = query.sheet_name.clone();
    let filter_row = query.row_num;

    let data = tokio::task::spawn_blocking(move || {
        use calamine::{Reader, open_workbook_auto};

        let path = std::path::Path::new(&file.path);
        let mut workbook: calamine::Sheets<std::io::BufReader<std::fs::File>> =
            open_workbook_auto(path).map_err(|e| ApiError::Internal(format!("Failed to open Excel: {}", e)))?;

        let mut sheets = Vec::new();
        let sheet_names = workbook.sheet_names().to_vec();

        for sheet_name in &sheet_names {
            if let Some(ref target) = filter_sheet
                && sheet_name != target
            {
                continue;
            }

            let range = match workbook.worksheet_range(sheet_name) {
                Ok(r) => r,
                Err(_) => continue,
            };

            let all_rows: Vec<_> = range.rows().collect();
            if all_rows.is_empty() {
                continue;
            }

            let headers: Vec<String> = all_rows[0]
                .iter()
                .enumerate()
                .map(|(idx, cell)| {
                    let s = cell.to_string().trim().to_string();
                    if s.is_empty() { format!("列{}", idx + 1) } else { s }
                })
                .collect();

            let data_rows = &all_rows[1..];
            let truncated = data_rows.len() > EXCEL_MAX_ROWS_PER_SHEET;
            let limit = data_rows.len().min(EXCEL_MAX_ROWS_PER_SHEET);

            let rows: Vec<Vec<String>> = if let Some(row_num) = filter_row {
                if row_num == 0 || row_num > all_rows.len() {
                    return Err(ApiError::BadRequest(format!(
                        "Row number {} out of range for sheet '{}', valid range: 1-{}",
                        row_num,
                        sheet_name,
                        all_rows.len()
                    )));
                }
                vec![all_rows[row_num - 1].iter().map(|cell| cell.to_string()).collect()]
            } else {
                data_rows[..limit].iter().map(|row| row.iter().map(|cell| cell.to_string()).collect()).collect()
            };

            sheets.push(ExcelSheetData {
                name: sheet_name.clone(),
                headers,
                rows,
                truncated: truncated && filter_row.is_none(),
            });
        }

        Ok::<_, ApiError>(ExcelData { filename: file.filename, sheets })
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Excel parsing task failed: {}", e)))??;

    Ok(Json(data))
}

// ============================================================================
// 压缩文件相关 API
// ============================================================================

/// 获取压缩文件内容列表
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/files/{id}/archive-entries",
    operation_id = "file_archive_entries",
    tag = "file",
    params(
        ("id" = i64, Path, description = "文件 ID")
    ),
    responses(
        (status = 200, description = "成功返回压缩文件列表", body = Vec<ArchiveEntry>),
        (status = 400, description = "不是压缩文件"),
        (status = 404, description = "文件不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn archive_entries(
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<Vec<ArchiveEntry>>> {
    let file: File = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;

    if !auth_user.is_admin() && !file.is_public && file.user_id != auth_user.user_id {
        return Err(ApiError::NotFound("File not found or permission denied".to_string()));
    }

    if !archive::is_archive_file(&file.filename) {
        return Err(ApiError::BadRequest("File is not an archive".to_string()));
    }

    let entries: Vec<ArchiveEntry> = sqlx::query_as(
        "SELECT id, file_id, entry_path, size, is_directory FROM archive_entries WHERE file_id = ? ORDER BY entry_path",
    )
    .bind(id)
    .fetch_all(&pool)
    .await?;

    Ok(Json(entries))
}

/// 解压压缩文件
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/files/{id}/archive-extract",
    operation_id = "file_archive_extract",
    tag = "file",
    params(
        ("id" = i64, Path, description = "文件 ID")
    ),
    request_body = ArchiveExtractReq,
    responses(
        (status = 200, description = "解压成功", body = ExtractResult),
        (status = 400, description = "请求参数错误"),
        (status = 404, description = "文件不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn archive_extract(
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<ArchiveExtractReq>,
) -> ApiResult<Json<ExtractResult>> {
    let file: File = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;

    if !auth_user.is_admin() && !file.is_public && file.user_id != auth_user.user_id {
        return Err(ApiError::NotFound("File not found or permission denied".to_string()));
    }

    if !archive::is_archive_file(&file.filename) {
        return Err(ApiError::BadRequest("File is not an archive".to_string()));
    }

    let cfg = config::get();
    let dest_dir = std::path::Path::new(&cfg.storage.archives_path).join(id.to_string());

    // 清理已有解压内容
    if let Err(e) = archive::cleanup_archive_dir(&cfg.storage.archives_path, id) {
        warn!("Failed to cleanup existing archive dir for file {}: {}", id, e);
    }

    // 清理数据库中已有记录
    sqlx::query("DELETE FROM archive_entries WHERE file_id = ?").bind(id).execute(&pool).await?;

    // 执行解压
    let password = req.password.clone();
    let file_path = file.path.clone();
    let filename = file.filename.clone();
    info!(
        "archive_extract: starting extraction for file_id={}, path={:?}, filename={:?}, password_provided={}",
        id,
        file_path,
        filename,
        password.is_some()
    );
    let entries = match tokio::task::spawn_blocking(move || {
        archive::extract_archive(&file_path, dest_dir.to_string_lossy().as_ref(), &filename, password.as_deref(), id)
    })
    .await
    {
        Ok(Ok(entries)) => {
            info!("archive_extract: extraction succeeded for file_id={}, entries={}", id, entries.len());
            entries
        }
        Ok(Err(archive::ArchiveError::PasswordRequired)) => {
            info!("archive_extract: password required for file_id={}", id);
            return Ok(Json(ExtractResult { entries: vec![], needs_password: true }));
        }
        Ok(Err(e)) => {
            warn!("archive_extract: extraction failed for file_id={}: {}", id, e);
            return Err(ApiError::BadRequest(e.to_string()));
        }
        Err(e) => {
            error!("archive_extract: spawn_blocking failed for file_id={}: {}", id, e);
            return Err(ApiError::Internal(format!("Archive extraction task failed: {}", e)));
        }
    };

    // 写入数据库：批量 INSERT 的多个 chunk 与末尾的 meta 更新合并进单个事务，
    // 把多次提交合成一次 fsync，并保证 entries 与 meta 的原子性。
    let meta = serde_json::json!({
        "archive": {
            "extracted": true,
            "needs_password": false,
            "entry_count": entries.len()
        }
    });

    let mut tx = pool.begin().await?;
    if !entries.is_empty() {
        let binds_per_row = 4_usize;
        let max_vars = 999_usize;
        let batch_size = std::cmp::max(1, max_vars / binds_per_row);
        for chunk in entries.chunks(batch_size) {
            let mut chunk_qb =
                QueryBuilder::<Sqlite>::new("INSERT INTO archive_entries (file_id, entry_path, size, is_directory) ");
            chunk_qb.push_values(chunk.iter(), |mut b, entry| {
                b.push_bind(entry.file_id)
                    .push_bind(&entry.entry_path)
                    .push_bind(entry.size)
                    .push_bind(entry.is_directory);
            });
            chunk_qb.build().execute(&mut *tx).await?;
        }
    }
    sqlx::query("UPDATE files SET meta = ?, updated_at = strftime('%s','now') WHERE id = ?")
        .bind(meta.to_string())
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(ExtractResult { entries, needs_password: false }))
}

/// 下载压缩包内的单个文件
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/files/{id}/archive-download",
    operation_id = "file_archive_download",
    tag = "file",
    params(
        ("id" = i64, Path, description = "文件 ID"),
        ArchiveDownloadQuery,
    ),
    responses(
        (status = 200, description = "成功返回文件内容", content_type = "application/octet-stream"),
        (status = 400, description = "请求参数错误"),
        (status = 404, description = "文件不存在")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn archive_download(
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Query(query): Query<ArchiveDownloadQuery>,
    Extension(auth_user): Extension<AuthUser>,
) -> Result<(StatusCode, [(header::HeaderName, String); 2], Body), ApiError> {
    let file: File = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;

    if !auth_user.is_admin() && !file.is_public && file.user_id != auth_user.user_id {
        return Err(ApiError::NotFound("File not found or permission denied".to_string()));
    }

    if !archive::is_archive_file(&file.filename) {
        return Err(ApiError::BadRequest("File is not an archive".to_string()));
    }

    // 安全检查：防止目录遍历
    let entry_path = query.path.trim();
    if entry_path.is_empty() || entry_path.contains("..") || entry_path.starts_with('/') || entry_path.starts_with('\\')
    {
        return Err(ApiError::BadRequest("Invalid entry path".to_string()));
    }

    let cfg = config::get();

    // 尝试从解压目录读取
    if let Some(resolved) = archive::resolve_archive_entry_path(&cfg.storage.archives_path, id, entry_path)
        && resolved.exists()
        && resolved.is_file()
    {
        let file_content = fs::read(&resolved).await?;
        let mime_type = mime_guess::from_path(entry_path).first_or_octet_stream().to_string();
        let filename = std::path::Path::new(entry_path).file_name().and_then(|n| n.to_str()).unwrap_or(entry_path);
        let content_disposition = format!("attachment; filename=\"{}\"", filename);
        return Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime_type), (header::CONTENT_DISPOSITION, content_disposition)],
            Body::from(file_content),
        ));
    }

    // 如果解压目录没有，尝试直接从压缩包读取（ZIP/TAR 支持）
    let src_path = file.path.clone();
    let filename = file.filename.clone();
    let entry_path_owned = entry_path.to_string();
    let buf = match tokio::task::spawn_blocking(move || {
        archive::read_archive_entry(&src_path, &filename, &entry_path_owned, None)
    })
    .await
    {
        Ok(Ok(buf)) => buf,
        Ok(Err(archive::ArchiveError::PasswordRequired)) => {
            return Err(ApiError::BadRequest("Archive is password protected".to_string()));
        }
        Ok(Err(e)) => return Err(ApiError::NotFound(e.to_string())),
        Err(e) => return Err(ApiError::Internal(format!("Archive read task failed: {}", e))),
    };

    let mime_type = mime_guess::from_path(entry_path).first_or_octet_stream().to_string();
    let filename = std::path::Path::new(entry_path).file_name().and_then(|n| n.to_str()).unwrap_or(entry_path);
    let content_disposition = format!("attachment; filename=\"{}\"", filename);
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, mime_type), (header::CONTENT_DISPOSITION, content_disposition)],
        Body::from(buf),
    ))
}
