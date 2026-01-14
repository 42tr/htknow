use axum::{
    Extension, body::Body, extract::{Multipart, Path, Query, State}, http::{StatusCode, header}, response::Json
};
use log::debug;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::{fs, io::AsyncWriteExt as _};
use utoipa::{IntoParams, ToSchema};

use crate::{
    AuthUser, api::error::{ApiError, ApiResult}, search::SearchEngine
};

#[derive(Serialize, Deserialize, Clone, Debug, sqlx::FromRow, ToSchema)]
pub struct File {
    pub id: i64,
    pub user_id: String,
    pub hash: String,
    pub filename: String,
    pub path: String,
    pub content: Option<String>,
    pub tags: String,
    pub status: i32,
    pub log: String,
    pub slice_type: String,
    pub kb_id: Option<i64>,
    pub is_public: i32,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 上传文件
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/files/",
    tag = "file",
    request_body(content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "文件上传成功", body = File),
        (status = 400, description = "请求参数错误"),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn upload(
    State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>, mut multipart: Multipart,
) -> ApiResult<Json<File>> {
    debug!("Starting file upload for user: {}", auth_user.user_id);
    let dir = "data/files";
    tokio::fs::create_dir_all(dir).await?;

    let mut hash = String::new();
    let mut filename = String::new();
    let mut filepath = String::new();
    let mut slice_type = String::new();
    let mut kb_id = None;
    let mut is_public = 0i32;
    let mut tags: Vec<String> = Vec::new();

    loop {
        match multipart.next_field().await {
            Ok(Some(mut field)) => {
                let field_name = field.name().map(|s| s.to_string());
                debug!("Processing field: {:?}", field_name);

                match field_name.as_deref() {
                    Some("file") => {
                        let mut hasher = Sha256::new();
                        filename = field.file_name().unwrap_or("unknown").to_string();
                        debug!("Uploading file: {}", filename);
                        let tempname = uuid::Uuid::new_v4().to_string();
                        filepath = format!("{}/{}", dir, tempname);
                        let mut file = tokio::fs::File::create(filepath.clone()).await?;
                        while let Some(chunk) = field.chunk().await? {
                            file.write_all(&chunk).await?;
                            hasher.update(&chunk);
                        }
                        hash = hex::encode(hasher.finalize());
                        debug!("File saved to: {}", filepath);
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
                        is_public = is_public_text.parse::<i32>().unwrap_or(0);
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

    debug!("File upload completed. Filename: {}", filename);
    if filename.is_empty() {
        return Err(ApiError::BadRequest("file is required".to_string()));
    }

    let mut kb_type: Option<String> = None;
    if let Some(kb_id_value) = kb_id {
        let kb: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT user_id, kb_type FROM knowledge_bases WHERE id = ?")
                .bind(kb_id_value)
                .fetch_optional(&pool)
                .await?;
        let (kb_owner, kb_type_value) = kb.ok_or_else(|| ApiError::NotFound("Knowledge base not found".to_string()))?;
        if kb_owner != auth_user.user_id {
            return Err(ApiError::NotFound("Knowledge base not found or permission denied".to_string()));
        }
        kb_type = kb_type_value;
    }

    let is_storage_kb = matches!(kb_type.as_deref(), Some("storage"));
    let tags_json = serde_json::to_string(&tags)?;
    let status = if is_storage_kb { 3 } else { 0 };
    let log_message = if is_storage_kb { "Storage mode: not parsed" } else { "" };
    let sql = "INSERT INTO files (user_id, hash, filename, path, slice_type, kb_id, is_public, tags, status, log) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
    let id = sqlx::query(sql)
        .bind(auth_user.user_id)
        .bind(hash)
        .bind(filename)
        .bind(filepath)
        .bind(slice_type)
        .bind(kb_id)
        .bind(is_public)
        .bind(tags_json)
        .bind(status)
        .bind(log_message)
        .execute(&pool)
        .await?
        .last_insert_rowid();
    let file = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;
    Ok(Json(file))
}

/// 获取文件详情
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/files/{id}",
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

    if file.is_public == 0 && file.user_id != auth_user.user_id {
        return Err(ApiError::NotFound("File not found or permission denied".to_string()));
    }

    Ok(Json(file))
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateFileReq {
    pub slice_type: String,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateTagsReq {
    pub tags: Vec<String>,
}

/// 更新文件（重新切片）
#[utoipa::path(
    put,
    path = "/api/v1/knowledge/files/{id}",
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
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>, Path(id): Path<i64>,
    Json(req): Json<UpdateFileReq>,
) -> ApiResult<Json<File>> {
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
    let sql = "UPDATE files SET slice_type = ?, status = ? WHERE id = ?";
    sqlx::query(sql).bind(req.slice_type).bind(0).bind(id).execute(&pool).await?;
    let file = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;
    Ok(Json(file))
}

/// 更新文件标签
#[utoipa::path(
    put,
    path = "/api/v1/knowledge/files/{id}/tags",
    tag = "file",
    params(
        ("id" = i64, Path, description = "文件 ID")
    ),
    request_body = UpdateTagsReq,
    responses(
        (status = 200, description = "成功更新标签", body = File),
        (status = 400, description = "请求参数错误"),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn update_tags(
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Json(req): Json<UpdateTagsReq>,
) -> ApiResult<Json<File>> {
    let tags_json = serde_json::to_string(&req.tags)?;
    let sql = "UPDATE files SET tags = ?, updated_at = ? WHERE id = ?";
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    sqlx::query(sql).bind(tags_json).bind(now).bind(id).execute(&pool).await?;
    let file = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;
    Ok(Json(file))
}

#[derive(Deserialize, ToSchema)]
pub struct UpdatePublicReq {
    pub is_public: bool,
}

/// 更新文件公开/私有状态
#[utoipa::path(
    put,
    path = "/api/v1/knowledge/files/{id}/public",
    tag = "file",
    params(
        ("id" = i64, Path, description = "文件 ID")
    ),
    request_body = UpdatePublicReq,
    responses(
        (status = 200, description = "成功更新公开/私有状态", body = File),
        (status = 400, description = "请求参数错误"),
        (status = 401, description = "未授权")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn update_public(
    State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>, Path(id): Path<i64>,
    Json(req): Json<UpdatePublicReq>,
) -> ApiResult<Json<File>> {
    let is_public = if req.is_public { 1 } else { 0 };
    let sql = "UPDATE files SET is_public = ?, updated_at = ? WHERE id = ? AND user_id = ?";
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    sqlx::query(sql).bind(is_public).bind(now).bind(id).bind(&auth_user.user_id).execute(&pool).await?;
    let file = sqlx::query_as("SELECT * FROM files WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(&auth_user.user_id)
        .fetch_one(&pool)
        .await?;
    Ok(Json(file))
}

/// 删除文件
#[utoipa::path(
    delete,
    path = "/api/v1/knowledge/files/{id}",
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
    let query = "SELECT * FROM files WHERE id = ?";
    let file: File = sqlx::query_as(query).bind(id).fetch_one(&pool).await?;
    fs::remove_file(file.path).await?;

    // 删除 PDF 图片
    let images: Vec<PdfImage> =
        sqlx::query_as("SELECT * FROM pdf_images WHERE file_id = ?").bind(id).fetch_all(&pool).await?;

    for image in images {
        // 删除图片文件
        if let Err(e) = fs::remove_file(&image.path).await {
            log::warn!("Failed to delete image file {}: {}", image.path, e);
        }
    }

    // 删除图片目录（如果存在）
    let image_dir = format!("data/pdf_images/{}", id);
    if let Err(e) = fs::remove_dir_all(&image_dir).await {
        log::warn!("Failed to delete image directory {}: {}", image_dir, e);
    }

    // 删除数据库记录
    sqlx::query("DELETE FROM pdf_images WHERE file_id = ?").bind(id).execute(&pool).await?;
    search_engine.delete(Some(id), None).await?;
    sqlx::query("DELETE FROM slices WHERE file_id = ?").bind(id).execute(&pool).await?;
    sqlx::query("DELETE FROM files WHERE id = ?").bind(id).execute(&pool).await?;
    Ok(())
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
    let mut files = match query.kb_id.as_deref() {
        // 明确指定查询未分配知识库的文件
        Some("null") | Some("unassigned") => {
            sqlx::query_as("SELECT * FROM files WHERE user_id = ? AND kb_id IS NULL ORDER BY created_at DESC")
                .bind(&auth_user.user_id)
                .fetch_all(&pool)
                .await?
        }
        // 查询特定知识库的文件
        Some(kb_id_str) => {
            let kb_id = kb_id_str.parse::<i64>().map_err(|_| ApiError::internal("Invalid kb_id format"))?;
            // 检查知识库权限
            let kb: Option<(String, i32)> =
                sqlx::query_as("SELECT user_id, is_public FROM knowledge_bases WHERE id = ?")
                    .bind(kb_id)
                    .fetch_optional(&pool)
                    .await?;

            if let Some((kb_owner, is_public)) = kb {
                if is_public == 0 && kb_owner != auth_user.user_id {
                    return Err(ApiError::NotFound("Knowledge base not found or permission denied".to_string()));
                }
            } else {
                return Err(ApiError::NotFound("Knowledge base not found".to_string()));
            }

            sqlx::query_as("SELECT * FROM files WHERE user_id = ? AND kb_id = ? ORDER BY created_at DESC")
                .bind(&auth_user.user_id)
                .bind(kb_id)
                .fetch_all(&pool)
                .await?
        }
        // 不传参数，查询所有文件
        None => {
            sqlx::query_as("SELECT * FROM files WHERE user_id = ? ORDER BY created_at DESC")
                .bind(&auth_user.user_id)
                .fetch_all(&pool)
                .await?
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

#[derive(Serialize, sqlx::FromRow, ToSchema)]
pub struct Slice {
    pub id: i64,
    pub file_id: i64,
    pub content: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 获取文件的所有切片
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/files/{id}/slices",
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
    let slices = sqlx::query_as("SELECT * FROM slices WHERE file_id = ? ORDER BY id").bind(id).fetch_all(&pool).await?;
    Ok(Json(slices))
}

#[derive(Serialize, Deserialize, Clone, Debug, sqlx::FromRow, ToSchema)]
pub struct PdfImage {
    pub id: i64,
    pub file_id: i64,
    pub filename: String,
    pub path: String,
    pub page_num: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 获取文件的所有图片列表
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/files/{file_id}/images",
    tag = "file",
    params(
        ("file_id" = i64, Path, description = "文件 ID")
    ),
    responses(
        (status = 200, description = "成功返回图片列表", body = Vec<PdfImage>),
        (status = 404, description = "文件不存在")
    )
)]
pub async fn get_images(State(pool): State<SqlitePool>, Path(file_id): Path<i64>) -> ApiResult<Json<Vec<PdfImage>>> {
    let images =
        sqlx::query_as("SELECT * FROM pdf_images WHERE file_id = ? ORDER BY id").bind(file_id).fetch_all(&pool).await?;
    Ok(Json(images))
}

/// 获取单个图片文件
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/files/{file_id}/images/{image_id}",
    tag = "file",
    params(
        ("file_id" = i64, Path, description = "文件 ID"),
        ("image_id" = i64, Path, description = "图片 ID")
    ),
    responses(
        (status = 200, description = "成功返回图片文件", content_type = "image/*"),
        (status = 404, description = "图片不存在")
    )
)]
pub async fn get_image(
    State(pool): State<SqlitePool>, Path((file_id, image_id)): Path<(i64, i64)>,
) -> Result<(StatusCode, [(header::HeaderName, String); 1], Body), ApiError> {
    let image: PdfImage = sqlx::query_as("SELECT * FROM pdf_images WHERE id = ? AND file_id = ?")
        .bind(image_id)
        .bind(file_id)
        .fetch_one(&pool)
        .await?;

    let file_content = tokio::fs::read(&image.path).await?;
    let mime_type = mime_guess::from_path(&image.filename).first_or_octet_stream().to_string();

    Ok((StatusCode::OK, [(header::CONTENT_TYPE, mime_type)], Body::from(file_content)))
}
