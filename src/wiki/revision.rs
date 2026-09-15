//! 页面版本快照与裁剪。
//!
//! 约定：`wiki_pages` 里永远是**当前版本**，`wiki_page_revisions` 保存被覆盖前的整份内容。
//! 因此写入顺序固定为「先快照、再更新」，配合 `(page_id, version)` 唯一索引 +
//! `INSERT OR IGNORE`，重试时不会写出两条同版本快照。

use anyhow::Result;
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use utoipa::ToSchema;

use super::EDIT_SOURCE_PIPELINE;

/// 版本元数据（不含正文），列表展示用。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RevisionMeta {
    pub id: i64,
    pub page_id: i64,
    pub kb_id: i64,
    pub version: i64,
    pub slug: String,
    pub title: String,
    pub page_type: String,
    pub summary: String,
    pub edit_source: String,
    pub editor_id: String,
    /// 正文字符数，前端据此提示「这一版有多长」而无需拉全文
    pub content_length: i64,
    pub created_at: i64,
}

/// 完整版本，diff 与回滚用。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Revision {
    pub id: i64,
    pub page_id: i64,
    pub kb_id: i64,
    pub version: i64,
    pub slug: String,
    pub title: String,
    pub page_type: String,
    pub summary: String,
    pub content: String,
    pub aliases: Vec<String>,
    pub edit_source: String,
    pub editor_id: String,
    pub created_at: i64,
}

fn aliases(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

/// 把当前行原样快照为一条历史版本，返回被快照的版本号。
///
/// 页面不存在或该版本已快照过时返回 `None`（幂等）。
pub async fn snapshot_current(pool: &SqlitePool, page_id: i64) -> Result<Option<i64>> {
    let mut conn = pool.acquire().await?;
    snapshot_current_in_conn(&mut conn, page_id).await
}

pub(super) async fn snapshot_current_in_conn(conn: &mut sqlx::SqliteConnection, page_id: i64) -> Result<Option<i64>> {
    let row: Option<(i64,)> = sqlx::query_as(
        "INSERT OR IGNORE INTO wiki_page_revisions
             (page_id, kb_id, version, slug, title, page_type, summary, content, aliases,
              edit_source, editor_id, created_at)
         SELECT id, kb_id, version, slug, title, page_type, summary, content, aliases,
                last_edit_source, last_editor_id, updated_at
           FROM wiki_pages WHERE id = ?
         RETURNING version",
    )
    .bind(page_id)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|(version,)| version))
}

/// 列出一个页面的历史版本（新→旧），不含正文。
pub async fn list(pool: &SqlitePool, page_id: i64, limit: i64) -> Result<Vec<RevisionMeta>> {
    let rows = sqlx::query(
        "SELECT id, page_id, kb_id, version, slug, title, page_type, summary, edit_source, editor_id,
                length(content), created_at
           FROM wiki_page_revisions WHERE page_id = ? ORDER BY version DESC LIMIT ?",
    )
    .bind(page_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| RevisionMeta {
            id: row.get("id"),
            page_id: row.get("page_id"),
            kb_id: row.get("kb_id"),
            version: row.get("version"),
            slug: row.get("slug"),
            title: row.get("title"),
            page_type: row.get("page_type"),
            summary: row.get("summary"),
            edit_source: row.get("edit_source"),
            editor_id: row.get("editor_id"),
            content_length: row.get::<Option<i64>, _>(10).unwrap_or(0),
            created_at: row.get("created_at"),
        })
        .collect())
}

/// 读取某个具体版本。
pub async fn get(pool: &SqlitePool, page_id: i64, version: i64) -> Result<Option<Revision>> {
    let row = sqlx::query(
        "SELECT id, page_id, kb_id, version, slug, title, page_type, summary, content, aliases,
                edit_source, editor_id, created_at
           FROM wiki_page_revisions WHERE page_id = ? AND version = ?",
    )
    .bind(page_id)
    .bind(version)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| Revision {
        id: row.get("id"),
        page_id: row.get("page_id"),
        kb_id: row.get("kb_id"),
        version: row.get("version"),
        slug: row.get("slug"),
        title: row.get("title"),
        page_type: row.get("page_type"),
        summary: row.get("summary"),
        content: row.get("content"),
        aliases: aliases(&row.get::<String, _>("aliases")),
        edit_source: row.get("edit_source"),
        editor_id: row.get("editor_id"),
        created_at: row.get("created_at"),
    }))
}

/// 版本数量，测试与状态展示用。
pub async fn count(pool: &SqlitePool, page_id: i64) -> Result<i64> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wiki_page_revisions WHERE page_id = ?")
        .bind(page_id)
        .fetch_one(pool)
        .await?;
    Ok(count)
}

/// 裁剪历史版本，返回删除条数。两级限额都按「保留最新的 N 条」计算：
///
/// - 软限额只裁**管道生成**的版本：自动改写很频繁，用户版本却值得长期保留；
/// - 硬限额裁所有版本，是单页历史占用的上限。
///
/// 传 0 表示该级不裁剪。
pub async fn prune(pool: &SqlitePool, page_id: i64, soft_limit: usize, hard_limit: usize) -> Result<u64> {
    let mut removed = 0u64;
    if soft_limit > 0 {
        removed += sqlx::query(
            "DELETE FROM wiki_page_revisions
              WHERE page_id = ? AND edit_source = ?
                AND id NOT IN (
                    SELECT id FROM wiki_page_revisions
                     WHERE page_id = ? AND edit_source = ?
                     ORDER BY version DESC LIMIT ?
                )",
        )
        .bind(page_id)
        .bind(EDIT_SOURCE_PIPELINE)
        .bind(page_id)
        .bind(EDIT_SOURCE_PIPELINE)
        .bind(soft_limit as i64)
        .execute(pool)
        .await?
        .rows_affected();
    }
    if hard_limit > 0 {
        removed += sqlx::query(
            "DELETE FROM wiki_page_revisions
              WHERE page_id = ?
                AND id NOT IN (
                    SELECT id FROM wiki_page_revisions WHERE page_id = ? ORDER BY version DESC LIMIT ?
                )",
        )
        .bind(page_id)
        .bind(page_id)
        .bind(hard_limit as i64)
        .execute(pool)
        .await?
        .rows_affected();
    }
    Ok(removed)
}

/// 按全局配置裁剪单个页面的历史。
pub async fn prune_by_config(pool: &SqlitePool, page_id: i64) -> Result<u64> {
    let config = &crate::config::get().wiki;
    prune(pool, page_id, config.revision_soft_limit, config.revision_hard_limit).await
}
