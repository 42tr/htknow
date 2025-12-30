use axum::{
    Extension, extract::{Multipart, Path, Query, State}, response::Json
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt as _;

use crate::{
    AuthUser, api::error::{ApiError, ApiResult}
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
    let dir = "data/files";
    tokio::fs::create_dir_all(dir).await?;

    let mut hash = String::new();
    let mut filename = String::new();
    let mut filepath = String::new();
    let mut slice_type = String::new();
    let mut kb_id = None;
    while let Some(mut field) = multipart.next_field().await? {
        match field.name() {
            Some("file") => {
                let mut hasher = Sha256::new();
                filename = field.file_name().unwrap_or("unknown").to_string();
                let tempname = uuid::Uuid::new_v4().to_string();
                filepath = format!("{}/{}", dir, tempname);
                let mut file = tokio::fs::File::create(filepath.clone()).await?;
                while let Some(chunk) = field.chunk().await? {
                    file.write_all(&chunk).await?;
                    hasher.update(&chunk);
                }
                hash = hex::encode(hasher.finalize());
            }
            Some("slice_type") => {
                slice_type = field.text().await?;
            }
            Some("kb_id") => {
                kb_id = Some(field.text().await?.parse::<i64>()?);
            }
            _ => {}
        }
    }
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
    State(pool): State<SqlitePool>, Path(id): Path<i64>, Json(req): Json<UpdateFileReq>,
) -> ApiResult<Json<File>> {
    let sql = "DELETE FROM slices WHERE file_id = ?";
    sqlx::query(sql).bind(id).execute(&pool).await?;
    let sql = "UPDATE files SET slice_type = ?, status = ? WHERE id = ?";
    sqlx::query(sql).bind(req.slice_type).bind(0).bind(id).execute(&pool).await?;
    let file = sqlx::query_as("SELECT * FROM files WHERE id = ?").bind(id).fetch_one(&pool).await?;
    Ok(Json(file))
}

pub async fn delete(State(pool): State<SqlitePool>, Path(id): Path<i64>) -> ApiResult<()> {
    sqlx::query("DELETE FROM files WHERE id = ?").bind(id).execute(&pool).await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub kb_id: Option<i64>,
}

pub async fn list(
    State(pool): State<SqlitePool>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Vec<File>>> {
    let files = match query.kb_id {
        Some(kb_id) => {
            sqlx::query_as("SELECT * FROM files WHERE user_id = ? AND kb_id = ? ORDER BY created_at DESC")
                .bind(&auth_user.user_id)
                .bind(kb_id)
                .fetch_all(&pool)
                .await?
        }
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

pub async fn get_slices(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<Slice>>> {
    let slices = sqlx::query_as("SELECT * FROM slices WHERE file_id = ? ORDER BY id")
        .bind(id)
        .fetch_all(&pool)
        .await?;
    Ok(Json(slices))
}
