use std::{
    collections::{HashMap, HashSet}, path::Component, time::Instant
};

use axum::{
    Extension, body::Body, extract::{Multipart, Path, Query, State}, http::{StatusCode, header}, response::Json
};
use log::{debug, info};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use tokio::{fs, io::AsyncWriteExt as _, spawn};
use utoipa::{IntoParams, ToSchema};

use crate::{
    AuthUser, api::error::{ApiError, ApiResult}, config, pdf_highlight, processor, search::SearchEngine
};

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
                        let parse_text = field.text().await?;
                        debug!("Immediate parse text: {}", parse_text);
                        immediate_parse = parse_text == "true" || parse_text == "1";
                    }
                    Some("sync") => {
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

        let file = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;
        uploaded_files.push(file);
        uploaded_file_ids.push(id);

        if immediate_parse || sync {
            parse_file_ids.push(id);
        }
    }

    if immediate_parse || sync {
        if sync {
            for file_id in parse_file_ids {
                processor::process_file_immediate(pool.clone(), search_engine.clone(), file_id).await?;
            }
            uploaded_files.clear();
            for file_id in uploaded_file_ids {
                let file = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(file_id).fetch_one(&pool).await?;
                uploaded_files.push(file);
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
    let mut has_updates = false;
    let update_is_public = req.is_public.is_some();
    debug!("update_is_public: {}", update_is_public);
    if update_is_public {
        let owner: Option<String> =
            sqlx::query_scalar("SELECT user_id FROM files WHERE id = ?").bind(id).fetch_optional(&pool).await?;
        let owner = owner.ok_or_else(|| ApiError::NotFound("File not found or permission denied".to_string()))?;
        if owner != auth_user.user_id {
            return Err(ApiError::NotFound("File not found or permission denied".to_string()));
        }
    }
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

        search_engine.delete(Some(id), None).await?;
        let sql = "DELETE FROM slices WHERE file_id = ?";
        sqlx::query(sql).bind(id).execute(&pool).await?;

        separated.push("slice_type = ").push_bind_unseparated(slice_type);
        separated.push("status = ").push_bind_unseparated(0);
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
    if update_is_public {
        qb.push(" AND user_id = ");
        qb.push_bind(&auth_user.user_id);
    }
    qb.build().execute(&pool).await?;

    let file = if update_is_public {
        sqlx::query_as("SELECT * FROM files WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(&auth_user.user_id)
            .fetch_one(&pool)
            .await?
    } else {
        sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?
    };
    Ok(Json(file))
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
        (status = 404, description = "文件不存在")
    )
)]
pub async fn delete(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>, Path(id): Path<i64>,
) -> ApiResult<()> {
    let overall_start = Instant::now();
    let step_start = Instant::now();
    let query = "SELECT * FROM files WHERE id = ?";
    let file: File = sqlx::query_as(query).bind(id).fetch_one(&pool).await?;
    debug!("file_delete id={} fetch_file {}ms", id, step_start.elapsed().as_millis());

    let image_paths = collect_image_paths_for_files(&pool, std::slice::from_ref(&id)).await?;

    let step_start = Instant::now();
    fs::remove_file(file.path).await?;
    debug!("file_delete id={} remove_file {}ms", id, step_start.elapsed().as_millis());

    let cfg = config::get();
    let pdf_path = std::path::Path::new(&cfg.storage.pdf_path).join(format!("{}.pdf", id));
    let step_start = Instant::now();
    if let Err(e) = fs::remove_file(&pdf_path).await {
        log::warn!("Failed to delete converted pdf {}: {}", pdf_path.display(), e);
    }
    debug!("file_delete id={} remove_pdf {}ms", id, step_start.elapsed().as_millis());

    remove_image_files(image_paths).await;

    let step_start = Instant::now();
    search_engine.delete(Some(id), None).await?;
    debug!("file_delete id={} search_delete {}ms", id, step_start.elapsed().as_millis());

    let step_start = Instant::now();
    sqlx::query("DELETE FROM entity_mentions WHERE slice_id IN (SELECT id FROM slices WHERE file_id = ?)")
        .bind(id)
        .execute(&pool)
        .await?;
    debug!("file_delete id={} delete_mentions {}ms", id, step_start.elapsed().as_millis());

    let step_start = Instant::now();
    sqlx::query("DELETE FROM slice_positions WHERE slice_id IN (SELECT id FROM slices WHERE file_id = ?)")
        .bind(id)
        .execute(&pool)
        .await?;
    debug!("file_delete id={} delete_slice_positions {}ms", id, step_start.elapsed().as_millis());

    let step_start = Instant::now();
    sqlx::query("DELETE FROM slices WHERE file_id = ?").bind(id).execute(&pool).await?;
    debug!("file_delete id={} delete_slices {}ms", id, step_start.elapsed().as_millis());

    let step_start = Instant::now();
    sqlx::query("DELETE FROM pdf_contents WHERE file_id = ?").bind(id).execute(&pool).await?;
    debug!("file_delete id={} delete_pdf_contents {}ms", id, step_start.elapsed().as_millis());

    let step_start = Instant::now();
    sqlx::query("DELETE FROM files WHERE id = ?").bind(id).execute(&pool).await?;
    debug!("file_delete id={} delete_file_row {}ms", id, step_start.elapsed().as_millis());

    debug!("file_delete id={} total {}ms", id, overall_start.elapsed().as_millis());
    Ok(())
}

pub(crate) async fn collect_image_paths_for_files(
    pool: &SqlitePool, file_ids: &[i64],
) -> Result<Vec<String>, sqlx::Error> {
    if file_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut qb = QueryBuilder::<Sqlite>::new("SELECT img_path FROM pdf_contents WHERE file_id IN (");
    let mut separated = qb.separated(", ");
    for file_id in file_ids {
        separated.push_bind(file_id);
    }
    qb.push(") AND img_path IS NOT NULL AND img_path != ''");
    let rows: Vec<Option<String>> = qb.build_query_scalar().fetch_all(pool).await?;
    let mut resolved = Vec::new();
    for raw in rows.into_iter().flatten() {
        if let Some(path) = resolve_image_storage_path(&raw) {
            resolved.push(path);
        }
    }
    Ok(resolved)
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

fn resolve_image_storage_path(raw: &str) -> Option<String> {
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
    let data_root = images_root.parent();
    let use_data_root = trimmed.contains('/') || trimmed.contains('\\');

    let resolved = if use_data_root {
        if let Some(root) = data_root { root.join(trimmed) } else { images_root.join(trimmed) }
    } else {
        images_root.join(trimmed)
    };

    Some(resolved.to_string_lossy().to_string())
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
            || data_root.map_or(false, |root| candidate.starts_with(root))
        {
            candidate.to_path_buf()
        } else {
            match resolve_image_storage_path(trimmed) {
                Some(path) => std::path::PathBuf::from(path),
                None => continue,
            }
        };
        let allowed = full_path.starts_with(images_root) || data_root.map_or(false, |root| full_path.starts_with(root));
        if !allowed {
            log::warn!("Skipping unsafe image path: {}", full_path.display());
            continue;
        }
        if let Err(e) = fs::remove_file(&full_path).await {
            if matches!(e.kind(), std::io::ErrorKind::NotFound) {
                if !candidate.is_absolute() && (trimmed.contains('/') || trimmed.contains('\\')) {
                    if let Some(file_name) = candidate.file_name() {
                        let fallback_path = images_root.join(file_name);
                        if let Err(e) = fs::remove_file(&fallback_path).await {
                            if !matches!(e.kind(), std::io::ErrorKind::NotFound) {
                                log::warn!("Failed to delete image {}: {}", fallback_path.display(), e);
                            }
                        }
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
                sqlx::query_as("SELECT * FROM files WHERE kb_id IS NULL ORDER BY created_at DESC")
                    .fetch_all(&pool)
                    .await?
            } else {
                sqlx::query_as(
                        "SELECT * FROM files WHERE kb_id IS NULL AND (user_id = ? OR is_public = 1) ORDER BY created_at DESC",
                    )
                    .bind(&auth_user.user_id)
                    .fetch_all(&pool)
                    .await?
            }
        }
        // 查询特定知识库的文件
        Some(kb_id_str) => {
            let kb_id = kb_id_str.parse::<i64>().map_err(|_| ApiError::internal("Invalid kb_id format"))?;
            // 检查知识库权限
            if is_admin {
                let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM knowledge_bases WHERE id = ?")
                    .bind(kb_id)
                    .fetch_optional(&pool)
                    .await?;
                if exists.is_none() {
                    return Err(ApiError::NotFound("Knowledge base not found".to_string()));
                }
            } else {
                let kb: Option<(String, bool)> =
                    sqlx::query_as("SELECT user_id, is_public FROM knowledge_bases WHERE id = ?")
                        .bind(kb_id)
                        .fetch_optional(&pool)
                        .await?;

                if let Some((kb_owner, is_public)) = kb {
                    if !is_public && kb_owner != auth_user.user_id {
                        return Err(ApiError::NotFound("Knowledge base not found or permission denied".to_string()));
                    }
                } else {
                    return Err(ApiError::NotFound("Knowledge base not found".to_string()));
                }
            }

            if is_admin {
                sqlx::query_as("SELECT * FROM files WHERE kb_id = ? ORDER BY created_at DESC")
                    .bind(kb_id)
                    .fetch_all(&pool)
                    .await?
            } else {
                sqlx::query_as(
                    "SELECT * FROM files WHERE kb_id = ? AND (user_id = ? OR is_public) ORDER BY created_at DESC",
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
                sqlx::query_as("SELECT * FROM files ORDER BY created_at DESC").fetch_all(&pool).await?
            } else {
                sqlx::query_as("SELECT * FROM files WHERE (user_id = ? OR is_public = 1) ORDER BY created_at DESC")
                    .bind(&auth_user.user_id)
                    .fetch_all(&pool)
                    .await?
            }
        }
    };

    // 如果指定了标签，进行过滤
    if let Some(tag) = &query.tag {
        files.retain(|file: &File| {
            if let Ok(tags) = serde_json::from_str::<Vec<String>>(&file.tags) { tags.contains(tag) } else { false }
        });
    }

    Ok(Json(files))
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
}

#[derive(Debug, sqlx::FromRow)]
struct SlicePositionRow {
    slice_id: i64,
    page_idx: i32,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
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
        "SELECT slice_id, page_idx, x1, y1, x2, y2 FROM slice_positions WHERE slice_id IN (",
    );
    let mut separated = qb.separated(", ");
    for slice_id in slice_ids {
        separated.push_bind(slice_id);
    }
    qb.push(") ORDER BY slice_id, page_idx, id");

    let rows: Vec<SlicePositionRow> = qb.build_query_as().fetch_all(&pool).await?;
    let mut position_map: std::collections::HashMap<i64, Vec<SlicePosition>> = std::collections::HashMap::new();
    for row in rows {
        position_map
            .entry(row.slice_id)
            .or_default()
            .push(SlicePosition { page_idx: row.page_idx, bbox: [row.x1, row.y1, row.x2, row.y2] });
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
) -> Result<(StatusCode, [(header::HeaderName, String); 1], Body), ApiError> {
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

    let file_content = fs::read(&image_path).await?;
    let mime_type = mime_guess::from_path(&filename).first_or_octet_stream().to_string();

    Ok((StatusCode::OK, [(header::CONTENT_TYPE, mime_type)], Body::from(file_content)))
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
) -> Result<(StatusCode, [(header::HeaderName, String); 2], Body), ApiError> {
    let file: File = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;

    if !auth_user.is_admin() && !file.is_public && file.user_id != auth_user.user_id {
        return Err(ApiError::NotFound("File not found or permission denied".to_string()));
    }

    let file_content = tokio::fs::read(&file.path).await?;
    let mime_type = mime_guess::from_path(&file.filename).first_or_octet_stream().to_string();
    let content_disposition = format!("attachment; filename=\"{}\"", file.filename);

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, mime_type), (header::CONTENT_DISPOSITION, content_disposition)],
        Body::from(file_content),
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
            "SELECT slice_id, page_idx, x1, y1, x2, y2 FROM slice_positions WHERE slice_id = ? ORDER BY page_idx, id",
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
            if let Ok(bbox) = serde_json::from_str::<Vec<f32>>(&row.bbox) {
                if bbox.len() == 4 {
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
        return Err(ApiError::BadRequest("File is not a PDF, Word, or Excel document".to_string()));
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
