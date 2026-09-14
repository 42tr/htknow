//! Wiki 页面的持久化与查询。所有写入都是单行短事务，LLM 调用由上层负责放在事务之外。

use anyhow::Result;
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use utoipa::ToSchema;

use super::{EDIT_SOURCE_PIPELINE, STATUS_PUBLISHED, WikiPage};

const PAGE_COLUMNS: &str = "id, kb_id, slug, title, page_type, status, summary, content, aliases, \
                            out_links, in_links, version, last_edit_source, last_editor_id, \
                            content_fingerprint, created_at, updated_at";

fn json_list(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

fn map_row(row: &sqlx::sqlite::SqliteRow) -> WikiPage {
    WikiPage {
        id: row.get("id"),
        kb_id: row.get("kb_id"),
        slug: row.get("slug"),
        title: row.get("title"),
        page_type: row.get("page_type"),
        status: row.get("status"),
        summary: row.get("summary"),
        content: row.get("content"),
        aliases: json_list(&row.get::<String, _>("aliases")),
        out_links: json_list(&row.get::<String, _>("out_links")),
        in_links: json_list(&row.get::<String, _>("in_links")),
        version: row.get("version"),
        last_edit_source: row.get("last_edit_source"),
        last_editor_id: row.get("last_editor_id"),
        content_fingerprint: row.get("content_fingerprint"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

/// 待写入的页面内容。来源文件与切片证据单独传入，便于增量合并。
#[derive(Debug, Clone)]
pub struct PageDraft {
    pub kb_id: i64,
    pub slug: String,
    pub title: String,
    pub page_type: String,
    pub summary: String,
    pub content: String,
    pub aliases: Vec<String>,
    pub edit_source: String,
    pub editor_id: String,
}

impl PageDraft {
    /// 用户可见字段的内容指纹。指纹未变时不递增 version，抑制页面抖动。
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        digest.update(b"htknow-wiki-v1");
        for part in [&self.title, &self.page_type, &self.summary, &self.content] {
            digest.update((part.len() as u64).to_le_bytes());
            digest.update(part.as_bytes());
        }
        let mut aliases = self.aliases.clone();
        aliases.sort();
        for alias in &aliases {
            digest.update(alias.as_bytes());
            digest.update(b"\x1f");
        }
        hex::encode(digest.finalize())
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UpsertOutcome {
    pub page_id: i64,
    /// 内容是否发生变化（指纹不同）。false 表示只补记了来源关系。
    pub changed: bool,
    pub version: i64,
}

/// 幂等写入页面：内容未变则只补记来源，不递增版本。
///
/// 来源与切片证据是**并入**语义（保留其他文档已有的贡献），与 reduce 阶段的
/// 增量合并一致；`set_slice_refs` 才提供整体替换语义。
pub async fn upsert(
    pool: &SqlitePool, draft: &PageDraft, source_file_ids: &[i64], slice_ids: &[i64],
) -> Result<UpsertOutcome> {
    let fingerprint = draft.fingerprint();
    let aliases = serde_json::to_string(&draft.aliases)?;
    let edit_source = if draft.edit_source.is_empty() { EDIT_SOURCE_PIPELINE } else { &draft.edit_source };

    let existing: Option<(i64, String, i64)> =
        sqlx::query_as("SELECT id, content_fingerprint, version FROM wiki_pages WHERE kb_id = ? AND slug = ?")
            .bind(draft.kb_id)
            .bind(&draft.slug)
            .fetch_optional(pool)
            .await?;

    let (page_id, changed, version) = match existing {
        Some((id, existing_fingerprint, existing_version)) => {
            if existing_fingerprint == fingerprint {
                (id, false, existing_version)
            } else {
                let next = existing_version + 1;
                sqlx::query(
                    "UPDATE wiki_pages
                        SET title = ?, page_type = ?, summary = ?, content = ?, aliases = ?,
                            version = ?, last_edit_source = ?, last_editor_id = ?, content_fingerprint = ?,
                            updated_at = strftime('%s','now')
                      WHERE id = ?",
                )
                .bind(&draft.title)
                .bind(&draft.page_type)
                .bind(&draft.summary)
                .bind(&draft.content)
                .bind(&aliases)
                .bind(next)
                .bind(edit_source)
                .bind(&draft.editor_id)
                .bind(&fingerprint)
                .bind(id)
                .execute(pool)
                .await?;
                (id, true, next)
            }
        }
        None => {
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO wiki_pages(kb_id, slug, title, page_type, status, summary, content, aliases,
                                         last_edit_source, last_editor_id, content_fingerprint)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
            )
            .bind(draft.kb_id)
            .bind(&draft.slug)
            .bind(&draft.title)
            .bind(&draft.page_type)
            .bind(STATUS_PUBLISHED)
            .bind(&draft.summary)
            .bind(&draft.content)
            .bind(&aliases)
            .bind(edit_source)
            .bind(&draft.editor_id)
            .bind(&fingerprint)
            .fetch_one(pool)
            .await?;
            (id, true, 1)
        }
    };

    add_sources(pool, page_id, source_file_ids).await?;
    add_slice_refs(pool, page_id, slice_ids).await?;
    Ok(UpsertOutcome { page_id, changed, version })
}

pub async fn add_sources(pool: &SqlitePool, page_id: i64, file_ids: &[i64]) -> Result<()> {
    for file_id in file_ids {
        sqlx::query("INSERT OR IGNORE INTO wiki_page_sources(page_id, file_id) VALUES(?, ?)")
            .bind(page_id)
            .bind(file_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub async fn add_slice_refs(pool: &SqlitePool, page_id: i64, slice_ids: &[i64]) -> Result<()> {
    for slice_id in slice_ids {
        sqlx::query("INSERT OR IGNORE INTO wiki_page_slice_refs(page_id, slice_id) VALUES(?, ?)")
            .bind(page_id)
            .bind(slice_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// 整体替换切片证据（页面被确定性重建时使用）。
pub async fn set_slice_refs(pool: &SqlitePool, page_id: i64, slice_ids: &[i64]) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM wiki_page_slice_refs WHERE page_id = ?").bind(page_id).execute(&mut *tx).await?;
    for slice_id in slice_ids {
        sqlx::query("INSERT OR IGNORE INTO wiki_page_slice_refs(page_id, slice_id) VALUES(?, ?)")
            .bind(page_id)
            .bind(slice_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn get_by_slug(pool: &SqlitePool, kb_id: i64, slug: &str) -> Result<Option<WikiPage>> {
    let sql = format!("SELECT {} FROM wiki_pages WHERE kb_id = ? AND slug = ?", PAGE_COLUMNS);
    let row = sqlx::query(&sql).bind(kb_id).bind(slug).fetch_optional(pool).await?;
    Ok(row.as_ref().map(map_row))
}

pub async fn get_by_id(pool: &SqlitePool, page_id: i64) -> Result<Option<WikiPage>> {
    let sql = format!("SELECT {} FROM wiki_pages WHERE id = ?", PAGE_COLUMNS);
    let row = sqlx::query(&sql).bind(page_id).fetch_optional(pool).await?;
    Ok(row.as_ref().map(map_row))
}

/// 游标分页列出页面（按 id 倒序），可选按类型/状态过滤。
pub async fn list(
    pool: &SqlitePool, kb_id: i64, page_type: Option<&str>, status: Option<&str>, limit: i64, before_id: Option<i64>,
) -> Result<Vec<WikiPage>> {
    let mut qb =
        sqlx::QueryBuilder::<sqlx::Sqlite>::new(format!("SELECT {} FROM wiki_pages WHERE kb_id = ", PAGE_COLUMNS));
    qb.push_bind(kb_id);
    if let Some(page_type) = page_type {
        qb.push(" AND page_type = ");
        qb.push_bind(page_type);
    }
    if let Some(status) = status {
        qb.push(" AND status = ");
        qb.push_bind(status);
    }
    if let Some(before_id) = before_id {
        qb.push(" AND id < ");
        qb.push_bind(before_id);
    }
    qb.push(" ORDER BY id DESC LIMIT ");
    qb.push_bind(limit);
    let rows = qb.build().fetch_all(pool).await?;
    Ok(rows.iter().map(map_row).collect())
}

/// 交叉链接所需的轻量投影：slug、标题、别名。
#[derive(Debug, Clone)]
pub struct PageSurface {
    pub slug: String,
    pub title: String,
    pub page_type: String,
    pub aliases: Vec<String>,
}

pub async fn list_surfaces(pool: &SqlitePool, kb_id: i64) -> Result<Vec<PageSurface>> {
    let rows = sqlx::query("SELECT slug, title, page_type, aliases FROM wiki_pages WHERE kb_id = ? AND status = ?")
        .bind(kb_id)
        .bind(STATUS_PUBLISHED)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .map(|row| PageSurface {
            slug: row.get("slug"),
            title: row.get("title"),
            page_type: row.get("page_type"),
            aliases: json_list(&row.get::<String, _>("aliases")),
        })
        .collect())
}

/// 标题/摘要的包含匹配搜索，限定在单个知识库内。
///
/// 页面正文的全文检索属于 P3（进 Tantivy），这里只覆盖目录浏览场景，
/// 结果集受 kb_id 约束，扫描量有界。
pub async fn search(pool: &SqlitePool, kb_id: i64, query: &str, limit: i64) -> Result<Vec<WikiPage>> {
    let pattern = format!("%{}%", query);
    let sql = format!(
        "SELECT {} FROM wiki_pages
          WHERE kb_id = ? AND (title LIKE ? OR summary LIKE ? OR aliases LIKE ?)
          ORDER BY page_type = 'index' DESC, updated_at DESC LIMIT ?",
        PAGE_COLUMNS
    );
    let rows =
        sqlx::query(&sql).bind(kb_id).bind(&pattern).bind(&pattern).bind(&pattern).bind(limit).fetch_all(pool).await?;
    Ok(rows.iter().map(map_row).collect())
}

pub async fn delete_by_slug(pool: &SqlitePool, kb_id: i64, slug: &str) -> Result<bool> {
    let result =
        sqlx::query("DELETE FROM wiki_pages WHERE kb_id = ? AND slug = ?").bind(kb_id).bind(slug).execute(pool).await?;
    Ok(result.rows_affected() > 0)
}

pub async fn delete_by_id(pool: &SqlitePool, page_id: i64) -> Result<bool> {
    let result = sqlx::query("DELETE FROM wiki_pages WHERE id = ?").bind(page_id).execute(pool).await?;
    Ok(result.rows_affected() > 0)
}

pub async fn source_file_ids(pool: &SqlitePool, page_id: i64) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as("SELECT file_id FROM wiki_page_sources WHERE page_id = ? ORDER BY file_id")
        .bind(page_id)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub async fn slice_ref_ids(pool: &SqlitePool, page_id: i64) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> =
        sqlx::query_as("SELECT slice_id FROM wiki_page_slice_refs WHERE page_id = ? ORDER BY slice_id")
            .bind(page_id)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// 按页面统计。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WikiStats {
    pub kb_id: i64,
    pub total: i64,
    pub published: i64,
    pub by_type: Vec<TypeCount>,
    pub source_file_count: i64,
    pub link_count: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TypeCount {
    pub page_type: String,
    pub count: i64,
}

pub async fn stats(pool: &SqlitePool, kb_id: i64) -> Result<WikiStats> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT page_type, COUNT(*), SUM(CASE WHEN status = 'published' THEN 1 ELSE 0 END)
           FROM wiki_pages WHERE kb_id = ? GROUP BY page_type",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    let by_type: Vec<TypeCount> =
        rows.iter().map(|(page_type, count, _)| TypeCount { page_type: page_type.clone(), count: *count }).collect();
    let published = rows.iter().map(|(_, _, count)| count).sum();
    let total: i64 = rows.iter().map(|(_, count, _)| count).sum();
    let source_file_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT s.file_id) FROM wiki_page_sources s
           JOIN wiki_pages p ON p.id = s.page_id WHERE p.kb_id = ?",
    )
    .bind(kb_id)
    .fetch_one(pool)
    .await?;
    // 不依赖 JSON1：出链数量在应用侧累加，页面数量级下开销可忽略。
    let link_rows: Vec<(String,)> =
        sqlx::query_as("SELECT out_links FROM wiki_pages WHERE kb_id = ?").bind(kb_id).fetch_all(pool).await?;
    let link_count: i64 = link_rows.iter().map(|(raw,)| json_list(raw).len() as i64).sum();
    Ok(WikiStats { kb_id, total, published, by_type, source_file_count, link_count })
}

/// 全量重算知识库内的入链。
///
/// 页面数量级下（数千页）一次聚合比增量维护更不容易出错，也天然覆盖页面删除后
/// 的反向链接残留。出链由各页面写入时自行维护。
pub async fn rebuild_in_links(pool: &SqlitePool, kb_id: i64) -> Result<()> {
    let surfaces = list_surfaces(pool, kb_id).await?;
    let live: std::collections::HashSet<&str> = surfaces.iter().map(|s| s.slug.as_str()).collect();
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT slug, out_links FROM wiki_pages WHERE kb_id = ? AND status = 'published'")
            .bind(kb_id)
            .fetch_all(pool)
            .await?;

    let mut inbound: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for (slug, out_links) in &rows {
        for target in json_list(out_links) {
            if &target == slug || !live.contains(target.as_str()) {
                continue;
            }
            inbound.entry(target).or_default().push(slug.clone());
        }
    }

    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE wiki_pages SET in_links = '[]' WHERE kb_id = ?").bind(kb_id).execute(&mut *tx).await?;
    for (slug, mut sources) in inbound {
        sources.sort();
        sources.dedup();
        sqlx::query("UPDATE wiki_pages SET in_links = ? WHERE kb_id = ? AND slug = ?")
            .bind(serde_json::to_string(&sources)?)
            .bind(kb_id)
            .bind(&slug)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 写入页面出链。
pub async fn set_out_links(pool: &SqlitePool, page_id: i64, out_links: &[String]) -> Result<()> {
    sqlx::query("UPDATE wiki_pages SET out_links = ?, updated_at = strftime('%s','now') WHERE id = ?")
        .bind(serde_json::to_string(out_links)?)
        .bind(page_id)
        .execute(pool)
        .await?;
    Ok(())
}
