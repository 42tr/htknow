use axum::{
    Extension, body::Body, extract::{Multipart, Path, Query, State}, http::{StatusCode, header}, response::Json
};
use log::debug;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::{fs, io::AsyncWriteExt as _};

use crate::{
    AuthUser, api::error::{ApiError, ApiResult}, search::SearchEngine
};

#[derive(Serialize, Deserialize, Clone, Debug, sqlx::FromRow)]
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
    pub created_at: i64,
    pub updated_at: i64,
}

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
    let sql = "INSERT INTO files (user_id, hash, filename, path, slice_type, kb_id) VALUES (?, ?, ?, ?, ?, ?)";
    let id = sqlx::query(sql)
        .bind(auth_user.user_id)
        .bind(hash)
        .bind(filename)
        .bind(filepath)
        .bind(slice_type)
        .bind(kb_id)
        .execute(&pool)
        .await?
        .last_insert_rowid();
    let file = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;
    Ok(Json(file))
}

pub async fn get(State(pool): State<SqlitePool>, Path(id): Path<i64>) -> ApiResult<Json<File>> {
    let query = "SELECT * FROM files WHERE id = ?";
    let file = sqlx::query_as(query).bind(id).fetch_one(&pool).await?;
    Ok(Json(file))
}

#[derive(Deserialize)]
pub struct UpdateFileReq {
    pub slice_type: String,
}

pub async fn update(
    State(pool): State<SqlitePool>, Extension(search_engine): Extension<SearchEngine>, Path(id): Path<i64>,
    Json(req): Json<UpdateFileReq>,
) -> ApiResult<Json<File>> {
    search_engine.delete(Some(id), None).await?;
    let sql = "DELETE FROM slices WHERE file_id = ?";
    sqlx::query(sql).bind(id).execute(&pool).await?;
    let sql = "UPDATE files SET slice_type = ?, status = ? WHERE id = ?";
    sqlx::query(sql).bind(req.slice_type).bind(0).bind(id).execute(&pool).await?;
    let file = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;
    Ok(Json(file))
}

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

#[derive(Deserialize)]
pub struct ListQuery {
    pub kb_id: Option<String>,
}

pub async fn list(
    State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>, Query(query): Query<ListQuery>,
) -> ApiResult<Json<Vec<File>>> {
    let files = match query.kb_id.as_deref() {
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
    Ok(Json(files))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct Slice {
    pub id: i64,
    pub file_id: i64,
    pub content: String,
    pub created_at: i64,
    pub updated_at: i64,
}

pub async fn get_slices(State(pool): State<SqlitePool>, Path(id): Path<i64>) -> ApiResult<Json<Vec<Slice>>> {
    let slices = sqlx::query_as("SELECT * FROM slices WHERE file_id = ? ORDER BY id").bind(id).fetch_all(&pool).await?;
    Ok(Json(slices))
}

#[derive(Serialize, Deserialize, Clone, Debug, sqlx::FromRow)]
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
pub async fn get_images(State(pool): State<SqlitePool>, Path(file_id): Path<i64>) -> ApiResult<Json<Vec<PdfImage>>> {
    let images =
        sqlx::query_as("SELECT * FROM pdf_images WHERE file_id = ? ORDER BY id").bind(file_id).fetch_all(&pool).await?;
    Ok(Json(images))
}

/// 获取单个图片文件
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
