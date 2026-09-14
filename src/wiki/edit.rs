//! 人工编辑：局部改写、手工建页、版本回滚、归档与删除。
//!
//! 与生成管道的分工：管道只写 `edit_source = pipeline` 的内容，人工编辑过的页面
//! 不会被后续 ingest/refresh 覆盖（见 `page::upsert`）；交叉链接维护是机械改写，
//! 对人工页面照常执行，因此内链不会腐烂。

use std::fmt;

use sqlx::SqlitePool;

use super::{
    EDIT_SOURCE_REVERT, EDIT_SOURCE_USER, PAGE_TYPE_CONCEPT, PAGE_TYPE_ENTITY, WikiPage, ingest, linkify, make_slug,
    page, queue, revision, slugify,
};

/// 标题最大字符数。
const MAX_TITLE_CHARS: usize = 200;
/// 摘要最大字符数。
const MAX_SUMMARY_CHARS: usize = 2000;
/// 正文最大字符数：挡住误把整个文件粘进编辑框。
const MAX_CONTENT_CHARS: usize = 400_000;
/// 别名数量上限。
const MAX_ALIASES: usize = 32;

/// 编辑类错误。API 层映射为 4xx，避免用户输入问题变成 500。
#[derive(Debug)]
pub enum EditError {
    NotFound(String),
    Invalid(String),
    Conflict(String),
    Internal(anyhow::Error),
}

pub type EditResult<T> = Result<T, EditError>;

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(message) => write!(f, "NotFound: {}", message),
            Self::Invalid(message) => write!(f, "Invalid: {}", message),
            Self::Conflict(message) => write!(f, "Conflict: {}", message),
            Self::Internal(error) => write!(f, "Internal error: {}", error),
        }
    }
}

impl std::error::Error for EditError {}

impl From<anyhow::Error> for EditError {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

impl From<sqlx::Error> for EditError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl From<serde_json::Error> for EditError {
    fn from(error: serde_json::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// 局部编辑载荷：`None` 字段保持原值。
#[derive(Debug, Clone, Default)]
pub struct PageEdit {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub aliases: Option<Vec<String>>,
    /// draft / published / archived
    pub status: Option<String>,
    pub editor_id: String,
}

impl PageEdit {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.summary.is_none()
            && self.content.is_none()
            && self.aliases.is_none()
            && self.status.is_none()
    }
}

/// 手工建页载荷。
#[derive(Debug, Clone)]
pub struct NewPage {
    pub title: String,
    /// 仅允许 entity / concept，默认 concept
    pub page_type: Option<String>,
    /// 省略时由标题派生
    pub slug: Option<String>,
    pub summary: String,
    pub content: String,
    pub aliases: Vec<String>,
    /// 省略时直接发布
    pub status: Option<String>,
    pub editor_id: String,
}

fn too_long(value: &str, max: usize) -> bool {
    value.chars().count() > max
}

fn check_limits(title: &str, summary: &str, content: &str, aliases: &[String]) -> EditResult<()> {
    if too_long(title, MAX_TITLE_CHARS) {
        return Err(EditError::Invalid(format!("title must be at most {} characters", MAX_TITLE_CHARS)));
    }
    if too_long(summary, MAX_SUMMARY_CHARS) {
        return Err(EditError::Invalid(format!("summary must be at most {} characters", MAX_SUMMARY_CHARS)));
    }
    if too_long(content, MAX_CONTENT_CHARS) {
        return Err(EditError::Invalid(format!("content must be at most {} characters", MAX_CONTENT_CHARS)));
    }
    if aliases.len() > MAX_ALIASES {
        return Err(EditError::Invalid(format!("aliases must be at most {} entries", MAX_ALIASES)));
    }
    for alias in aliases {
        if too_long(alias, MAX_TITLE_CHARS) {
            return Err(EditError::Invalid(format!("alias must be at most {} characters", MAX_TITLE_CHARS)));
        }
    }
    Ok(())
}

fn normalize_status(value: &str) -> EditResult<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        super::STATUS_DRAFT => Ok(super::STATUS_DRAFT.to_string()),
        super::STATUS_PUBLISHED => Ok(super::STATUS_PUBLISHED.to_string()),
        super::STATUS_ARCHIVED => Ok(super::STATUS_ARCHIVED.to_string()),
        other => {
            Err(EditError::Invalid(format!("status must be one of draft / published / archived, got '{}'", other)))
        }
    }
}

/// 清洗别名：去空白、去空项、去重（保序）。
fn clean_aliases(aliases: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(aliases.len());
    for alias in aliases {
        let trimmed = alias.trim();
        if trimmed.is_empty() || out.iter().any(|existing| existing == trimmed) {
            continue;
        }
        out.push(trimmed.to_string());
    }
    out
}

/// 把用户给的 slug 规范化成 `<page_type>/<主体>`。
fn normalize_slug(raw: Option<&str>, page_type: &str, title: &str) -> EditResult<String> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(make_slug(page_type, title));
    };
    let tail = raw.strip_prefix(&format!("{}/", page_type)).unwrap_or(raw);
    let normalized = slugify(tail);
    if normalized.is_empty() {
        return Err(EditError::Invalid("slug must contain at least one letter or digit".to_string()));
    }
    Ok(format!("{}/{}", page_type, normalized))
}

/// 保存一次人工编辑，返回更新后的页面。
///
/// 内容变化会先写版本快照再更新当前行，因此每次保存都可以在版本列表里回滚。
pub async fn apply_user_edit(pool: &SqlitePool, kb_id: i64, slug: &str, edit: &PageEdit) -> EditResult<WikiPage> {
    if edit.is_empty() {
        return Err(EditError::Invalid("nothing to update".to_string()));
    }
    let _lock = ingest::acquire_slug_lock(format!("{}:{}", kb_id, slug)).await;
    let existing = load_page(pool, kb_id, slug).await?;

    let title = match edit.title.as_deref().map(str::trim) {
        Some(title) if !title.is_empty() => title.to_string(),
        Some(_) => return Err(EditError::Invalid("title must not be empty".to_string())),
        None => existing.title.clone(),
    };
    let summary = edit.summary.as_deref().map(str::trim).unwrap_or(&existing.summary).to_string();
    let content = edit.content.as_deref().unwrap_or(&existing.content).to_string();
    let aliases =
        edit.aliases.as_ref().map(|aliases| clean_aliases(aliases)).unwrap_or_else(|| existing.aliases.clone());
    check_limits(&title, &summary, &content, &aliases)?;
    let status = edit.status.as_deref().map(normalize_status).transpose()?;

    let draft = page::PageDraft {
        kb_id,
        slug: existing.slug.clone(),
        title,
        page_type: existing.page_type.clone(),
        summary,
        content,
        aliases,
        edit_source: EDIT_SOURCE_USER.to_string(),
        editor_id: edit.editor_id.clone(),
    };
    let outcome = page::upsert(pool, &draft, &[], &[]).await?;
    if outcome.changed {
        // 立即重算出链，避免保存后到 finalize 之间出现「链接存在但图里没有」。
        let out_links = linkify::out_links(&draft.content, &draft.slug);
        page::set_out_links(pool, outcome.page_id, &out_links).await?;
    }
    if let Some(status) = status {
        page::set_status(pool, outcome.page_id, &status, EDIT_SOURCE_USER, &edit.editor_id).await?;
    }
    // 入链、索引目录与死链清理交给防抖的 finalize 任务收敛。
    queue::enqueue_finalize(pool, kb_id).await?;
    finish(pool, outcome.page_id, existing).await
}

/// 手工建页。slug 由调用方指定或从标题派生，冲突时报 `Conflict`。
pub async fn create_user_page(pool: &SqlitePool, kb_id: i64, new: &NewPage) -> EditResult<WikiPage> {
    let title = new.title.trim();
    if title.is_empty() {
        return Err(EditError::Invalid("title is required".to_string()));
    }
    let page_type = match new.page_type.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
        None => PAGE_TYPE_CONCEPT,
        Some(PAGE_TYPE_CONCEPT) => PAGE_TYPE_CONCEPT,
        Some(PAGE_TYPE_ENTITY) => PAGE_TYPE_ENTITY,
        // 摘要页与索引页由系统维护：前者绑定文档，后者是目录。
        Some(other) => {
            return Err(EditError::Invalid(format!("manual pages must be entity or concept, got '{}'", other)));
        }
    };
    let summary = new.summary.trim().to_string();
    let aliases = clean_aliases(&new.aliases);
    check_limits(title, &summary, &new.content, &aliases)?;
    let status = new.status.as_deref().map(normalize_status).transpose()?;
    let slug = normalize_slug(new.slug.as_deref(), page_type, title)?;

    let _lock = ingest::acquire_slug_lock(format!("{}:{}", kb_id, slug)).await;
    if page::get_by_slug(pool, kb_id, &slug).await?.is_some() {
        return Err(EditError::Conflict(format!("page '{}' already exists", slug)));
    }
    let draft = page::PageDraft {
        kb_id,
        slug: slug.clone(),
        title: title.to_string(),
        page_type: page_type.to_string(),
        summary,
        content: new.content.clone(),
        aliases,
        edit_source: EDIT_SOURCE_USER.to_string(),
        editor_id: new.editor_id.clone(),
    };
    let outcome = page::upsert(pool, &draft, &[], &[]).await?;
    let out_links = linkify::out_links(&draft.content, &draft.slug);
    page::set_out_links(pool, outcome.page_id, &out_links).await?;
    if let Some(status) = status {
        page::set_status(pool, outcome.page_id, &status, EDIT_SOURCE_USER, &new.editor_id).await?;
    }
    queue::enqueue_finalize(pool, kb_id).await?;
    page::get_by_id(pool, outcome.page_id)
        .await?
        .ok_or_else(|| EditError::NotFound(format!("page '{}' vanished", slug)))
}

/// 回滚到某个历史版本：把该版本内容作为**新版本**写入，历史保持只追加。
pub async fn revert(pool: &SqlitePool, kb_id: i64, slug: &str, version: i64, editor_id: &str) -> EditResult<WikiPage> {
    let _lock = ingest::acquire_slug_lock(format!("{}:{}", kb_id, slug)).await;
    let existing = load_page(pool, kb_id, slug).await?;
    let Some(snapshot) = revision::get(pool, existing.id, version).await? else {
        return Err(EditError::NotFound(format!("revision {} of '{}' not found", version, slug)));
    };

    let draft = page::PageDraft {
        kb_id,
        slug: existing.slug.clone(),
        title: snapshot.title.clone(),
        page_type: snapshot.page_type.clone(),
        summary: snapshot.summary.clone(),
        content: snapshot.content.clone(),
        aliases: snapshot.aliases.clone(),
        edit_source: EDIT_SOURCE_REVERT.to_string(),
        editor_id: editor_id.to_string(),
    };
    let outcome = page::upsert(pool, &draft, &[], &[]).await?;
    if outcome.changed {
        let out_links = linkify::out_links(&draft.content, &draft.slug);
        page::set_out_links(pool, outcome.page_id, &out_links).await?;
    }
    queue::enqueue_finalize(pool, kb_id).await?;
    finish(pool, outcome.page_id, existing).await
}

/// 归档页面：内容保留、退出目录与搜索，可随时恢复。
pub async fn archive(pool: &SqlitePool, kb_id: i64, slug: &str, editor_id: &str) -> EditResult<WikiPage> {
    set_page_status(pool, kb_id, slug, super::STATUS_ARCHIVED, editor_id).await
}

/// 恢复归档页面为已发布。
pub async fn restore(pool: &SqlitePool, kb_id: i64, slug: &str, editor_id: &str) -> EditResult<WikiPage> {
    set_page_status(pool, kb_id, slug, super::STATUS_PUBLISHED, editor_id).await
}

pub async fn set_page_status(
    pool: &SqlitePool, kb_id: i64, slug: &str, status: &str, editor_id: &str,
) -> EditResult<WikiPage> {
    let status = normalize_status(status)?;
    let _lock = ingest::acquire_slug_lock(format!("{}:{}", kb_id, slug)).await;
    let existing = load_page(pool, kb_id, slug).await?;
    page::set_status(pool, existing.id, &status, EDIT_SOURCE_USER, editor_id).await?;
    queue::enqueue_finalize(pool, kb_id).await?;
    finish(pool, existing.id, existing).await
}

/// 彻底删除页面。管道仍可能在下次 ingest 时按证据重新生成同名页，
/// 想长期隐藏请用归档。
pub async fn delete_page(pool: &SqlitePool, kb_id: i64, slug: &str) -> EditResult<()> {
    let _lock = ingest::acquire_slug_lock(format!("{}:{}", kb_id, slug)).await;
    let existing = load_page(pool, kb_id, slug).await?;
    page::delete_by_id(pool, existing.id).await?;
    queue::enqueue_finalize(pool, kb_id).await?;
    Ok(())
}

async fn load_page(pool: &SqlitePool, kb_id: i64, slug: &str) -> EditResult<WikiPage> {
    let existing = page::get_by_slug(pool, kb_id, slug)
        .await?
        .ok_or_else(|| EditError::NotFound(format!("wiki page '{}' not found", slug)))?;
    if existing.is_index() {
        // 索引页的目录由数据库确定性生成，手工改一次就会被下次 finalize 覆盖。
        return Err(EditError::Conflict("the index page is maintained by the system".to_string()));
    }
    Ok(existing)
}

async fn finish(pool: &SqlitePool, page_id: i64, fallback: WikiPage) -> EditResult<WikiPage> {
    Ok(page::get_by_id(pool, page_id).await?.unwrap_or(fallback))
}
