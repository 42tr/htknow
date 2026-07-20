//! 图片描述持久化模块
//!
//! 将外部图片文本化接口返回的描述和原始响应保存到 SQLite，
//! 支持索引重建/恢复时无需再次调用外部服务。

use std::collections::HashMap;

use anyhow::{Context, Result};
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct ImageDescription {
    pub id: i64,
    pub file_id: i64,
    pub image_filename: String,
    pub description: String,
    pub raw_response: String,
    pub source: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 保存或更新单条图片描述
pub async fn save(
    pool: &SqlitePool, file_id: i64, image_filename: &str, description: &str, raw_response: &str, source: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO image_descriptions (file_id, image_filename, description, raw_response, source)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(file_id, image_filename) DO UPDATE SET
             description = excluded.description,
             raw_response = excluded.raw_response,
             source = excluded.source,
             updated_at = strftime('%s','now')",
    )
    .bind(file_id)
    .bind(image_filename)
    .bind(description)
    .bind(raw_response)
    .bind(source)
    .execute(pool)
    .await
    .with_context(|| format!("failed to save image_description for file_id={} filename={}", file_id, image_filename))?;
    Ok(())
}

/// 按文件 ID + 图片文件名查询
pub async fn get(pool: &SqlitePool, file_id: i64, image_filename: &str) -> Result<Option<ImageDescription>> {
    let row = sqlx::query_as::<_, ImageDescription>(
        "SELECT id, file_id, image_filename, description, raw_response, source, created_at, updated_at
         FROM image_descriptions WHERE file_id = ? AND image_filename = ?",
    )
    .bind(file_id)
    .bind(image_filename)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to get image_description for file_id={} filename={}", file_id, image_filename))?;
    Ok(row)
}

/// 按文件 ID 列出所有图片描述（filename -> description）
pub async fn list_by_file(pool: &SqlitePool, file_id: i64) -> Result<HashMap<String, String>> {
    let rows: Vec<ImageDescription> = sqlx::query_as::<_, ImageDescription>(
        "SELECT id, file_id, image_filename, description, raw_response, source, created_at, updated_at
         FROM image_descriptions WHERE file_id = ?",
    )
    .bind(file_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("failed to list image_descriptions for file_id={}", file_id))?;
    Ok(rows.into_iter().map(|row| (row.image_filename, row.description)).collect())
}

#[derive(Debug, sqlx::FromRow)]
struct FileImageDescriptionRow {
    file_id: i64,
    image_filename: String,
    description: String,
}

/// 批量查询多个文件的图片描述，返回 (file_id, image_filename) -> description 映射。
pub async fn list_by_file_ids(pool: &SqlitePool, file_ids: &[i64]) -> Result<HashMap<(i64, String), String>> {
    if file_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut query_builder: QueryBuilder<'_, Sqlite> =
        QueryBuilder::new("SELECT file_id, image_filename, description FROM image_descriptions WHERE file_id IN (");
    let mut separated = query_builder.separated(", ");
    for file_id in file_ids {
        separated.push_bind(file_id);
    }
    separated.push_unseparated(")");

    let rows: Vec<FileImageDescriptionRow> = query_builder.build_query_as().fetch_all(pool).await?;
    Ok(rows.into_iter().map(|row| ((row.file_id, row.image_filename), row.description)).collect())
}

/// 复制源文件的所有图片描述记录到目标文件（旧版解析结果复用场景）
pub async fn copy_for_file(pool: &SqlitePool, src_file_id: i64, dst_file_id: i64) -> Result<()> {
    let rows: Vec<ImageDescription> = sqlx::query_as::<_, ImageDescription>(
        "SELECT id, file_id, image_filename, description, raw_response, source, created_at, updated_at
         FROM image_descriptions WHERE file_id = ?",
    )
    .bind(src_file_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("failed to copy image_descriptions from file_id={}", src_file_id))?;

    for row in rows {
        save(pool, dst_file_id, &row.image_filename, &row.description, &row.raw_response, &row.source).await?;
    }
    Ok(())
}
