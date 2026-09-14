//! 知识库 Wiki：把文档整理为互相链接的 Markdown 页面。
//!
//! 生成管道复用知识图谱已产出的「实体 → 切片证据」（`entity_mentions` /
//! `graph_node_sources`），因此默认不需要 WeKnora 那样的 chunk-citation pass。
//! 图谱不可用时回退到独立的候选抽取 + 引用归类。

pub mod edit;
pub mod finalize;
pub mod ingest;
pub mod linkify;
pub mod lint;
pub mod llm;
pub mod page;
pub mod prompts;
pub mod queue;
pub mod retract;
pub mod revision;
pub mod worker;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

pub const PAGE_TYPE_SUMMARY: &str = "summary";
pub const PAGE_TYPE_ENTITY: &str = "entity";
pub const PAGE_TYPE_CONCEPT: &str = "concept";
pub const PAGE_TYPE_INDEX: &str = "index";

pub const STATUS_DRAFT: &str = "draft";
pub const STATUS_PUBLISHED: &str = "published";
pub const STATUS_ARCHIVED: &str = "archived";

pub const EDIT_SOURCE_PIPELINE: &str = "pipeline";
pub const EDIT_SOURCE_USER: &str = "user";
pub const EDIT_SOURCE_REVERT: &str = "revert";

pub const TASK_INGEST: &str = "wiki:ingest";
pub const TASK_FINALIZE: &str = "wiki:finalize";
pub const TASK_RETRACT: &str = "wiki:retract";
pub const TASK_REFRESH: &str = "wiki:refresh";

pub const INDEX_SLUG: &str = "index";

/// 抽取粒度：控制候选条目数量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    /// 只保留文档主体（人物 + 其项目之类），最激进地裁剪。
    Focused,
    /// 默认：主体 + 被实质讨论的实体/概念。
    Standard,
    /// 抽取所有命名实体与可识别概念，适合把知识库当术语表用。
    Exhaustive,
}

impl Granularity {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "focused" => Some(Self::Focused),
            "standard" => Some(Self::Standard),
            "exhaustive" => Some(Self::Exhaustive),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Focused => "focused",
            Self::Standard => "standard",
            Self::Exhaustive => "exhaustive",
        }
    }

    /// 生成注入 prompt 的抽取范围说明。
    pub fn guidance(&self) -> &'static str {
        match self {
            Self::Focused => {
                "只抽取文档的主体对象（例如一篇简历里的人物及其所属项目），跳过顺带提及的技术名词与泛化概念。"
            }
            Self::Standard => {
                "抽取文档主体，以及被实质讨论（有专门段落或多条要点）的实体与概念。跳过一次性提及和通用商品化术语。"
            }
            Self::Exhaustive => "抽取所有命名实体与可识别概念，包括顺带提及的技术栈与库。",
        }
    }
}

impl Default for Granularity {
    fn default() -> Self {
        Self::Standard
    }
}

/// 知识库级 Wiki 配置，存放在 `knowledge_bases.wiki_config`（JSON 文本）。
/// 未设置的字段回退到全局配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KbWikiConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub granularity: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_pages_per_ingest: Option<usize>,
}

impl KbWikiConfig {
    /// 解析数据库中的 JSON 文本；空值或非法 JSON 视为「全部回退默认」。
    pub fn parse(raw: Option<&str>) -> Self {
        raw.and_then(|value| serde_json::from_str::<Self>(value).ok()).unwrap_or_default()
    }

    pub fn granularity_or(&self, fallback: Granularity) -> Granularity {
        self.granularity.as_deref().and_then(Granularity::parse).unwrap_or(fallback)
    }

    pub fn language_or(&self, fallback: &str) -> String {
        self.language.clone().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| fallback.to_string())
    }
}

/// 生效的 Wiki 配置：全局开关与知识库配置合并后的结果。
#[derive(Debug, Clone)]
pub struct ResolvedWikiConfig {
    pub enabled: bool,
    pub granularity: Granularity,
    pub language: String,
    pub model: Option<String>,
    pub max_pages_per_ingest: usize,
}

/// 一个 Wiki 页面。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiPage {
    pub id: i64,
    pub kb_id: i64,
    pub slug: String,
    pub title: String,
    pub page_type: String,
    pub status: String,
    pub summary: String,
    pub content: String,
    pub aliases: Vec<String>,
    pub out_links: Vec<String>,
    pub in_links: Vec<String>,
    pub version: i64,
    pub last_edit_source: String,
    pub last_editor_id: String,
    pub content_fingerprint: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl WikiPage {
    pub fn is_index(&self) -> bool {
        self.page_type == PAGE_TYPE_INDEX
    }
}

/// 读取某个知识库生效的 Wiki 配置。知识库不存在时返回 `None`。
pub async fn resolve_config(pool: &SqlitePool, kb_id: i64) -> anyhow::Result<Option<ResolvedWikiConfig>> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT wiki_config FROM knowledge_bases WHERE id = ?").bind(kb_id).fetch_optional(pool).await?;
    let Some((raw,)) = row else { return Ok(None) };
    let kb = KbWikiConfig::parse(raw.as_deref());
    let cfg = crate::config::get();
    let enabled = kb.enabled.unwrap_or(cfg.wiki.enabled);
    Ok(Some(ResolvedWikiConfig {
        enabled,
        granularity: kb.granularity_or(cfg.wiki.default_granularity()),
        language: kb.language_or(&cfg.wiki.default_language),
        model: kb.model.clone().filter(|v| !v.trim().is_empty()).or_else(|| cfg.wiki.model.clone()),
        max_pages_per_ingest: kb.max_pages_per_ingest.unwrap_or(cfg.wiki.max_pages_per_ingest),
    }))
}

/// 把名称规范化为 slug 主体：保留中日韩字符，ASCII 转小写，分隔符折叠为 `-`。
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_was_sep = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_sep = false;
            continue;
        }
        // 保留 CJK 等非 ASCII 字母数字，slug 直接可读，URL 层做百分号编码。
        if !ch.is_ascii() && (ch.is_alphabetic() || ch.is_numeric()) {
            out.push(ch);
            last_was_sep = false;
            continue;
        }
        if !last_was_sep && !out.is_empty() {
            out.push('-');
            last_was_sep = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    // 限制长度，避免超长实体名撑爆索引与 URL。
    let truncated: String = out.chars().take(80).collect();
    truncated.trim_end_matches('-').to_string()
}

/// 生成页面 slug。名称规范化后为空时退化为内容哈希，保证 slug 稳定且非空。
pub fn make_slug(page_type: &str, name: &str) -> String {
    let base = slugify(name);
    if base.is_empty() {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        digest.update(name.as_bytes());
        return format!("{}/{}", page_type, &hex::encode(digest.finalize())[..16]);
    }
    format!("{}/{}", page_type, base)
}

pub fn summary_slug(file_id: i64) -> String {
    format!("{}/{}", PAGE_TYPE_SUMMARY, file_id)
}

/// 解析 `entity/xxx` 形式的 slug，返回页面类型前缀。
pub fn slug_prefix(slug: &str) -> &str {
    slug.split_once('/').map(|(prefix, _)| prefix).unwrap_or("")
}

/// Wiki 相关迁移（版本 6 建表、版本 7 版本快照）。
///
/// 与图谱迁移一致：事务内抢占版本号，重复启动不重复执行。
pub async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    for (version, name, sql) in
        [(6, "wiki_pages", include_str!("migration.sql")), (7, "wiki_page_revisions", include_str!("migration_v7.sql"))]
    {
        let claimed = sqlx::query("INSERT OR IGNORE INTO schema_migrations(version, name) VALUES (?, ?)")
            .bind(version)
            .bind(name)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if claimed != 0 {
            sqlx::raw_sql(sql).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests;
