//! 知识库级收敛：交叉链接注入、死链清理、入链重算、索引页重建。
//!
//! 由防抖的 `wiki:finalize` 任务触发，一次执行收敛整批文档带来的变更，
//! 避免每篇文档都重跑一遍全库操作。

use std::collections::HashSet;

use anyhow::Result;
use log::{debug, info, warn};
use sqlx::SqlitePool;

use super::{
    EDIT_SOURCE_PIPELINE, INDEX_SLUG, PAGE_TYPE_CONCEPT, PAGE_TYPE_ENTITY, PAGE_TYPE_INDEX, PAGE_TYPE_SUMMARY,
    STATUS_PUBLISHED,
    ingest::document_title,
    linkify::{self, Linkifier},
    llm::WikiLlm,
    page::{self, PageDraft},
    prompts, resolve_config,
};

/// 索引页里目录段的起始标记。用它把「模型写的导言」与「系统生成的目录」切开，
/// 从而在目录未变时跳过 LLM 调用。HTML 注释在渲染后不可见。
const DIRECTORY_MARKER: &str = "<!-- wiki:directory -->";

/// 索引页每个分组最多列出的条目数。
const MAX_INDEX_ENTRIES_PER_GROUP: usize = 1000;

/// 单次 finalize 处理的页面上限，防止超大知识库把一次任务拖成小时级。
const MAX_PAGES_PER_FINALIZE: usize = 20000;

#[derive(Debug, Clone, Default)]
pub struct FinalizeReport {
    pub pages_scanned: usize,
    pub pages_changed: usize,
    pub dead_links_removed: usize,
    pub index_updated: bool,
}

pub async fn finalize_kb(pool: &SqlitePool, kb_id: i64) -> Result<FinalizeReport> {
    let Some(config) = resolve_config(pool, kb_id).await? else {
        debug!("wiki finalize: kb {} not found", kb_id);
        return Ok(FinalizeReport::default());
    };
    let _kb_lock = super::ingest::acquire_slug_lock(format!("kb-finalize:{kb_id}")).await;
    let mut report = FinalizeReport::default();

    let pages = page::list(pool, kb_id, None, Some(STATUS_PUBLISHED), MAX_PAGES_PER_FINALIZE as i64, None).await?;
    let surfaces = page::list_surfaces(pool, kb_id).await?;
    let live_slugs: HashSet<String> = surfaces.iter().map(|s| s.slug.clone()).collect();
    let linkifier = Linkifier::new(&surfaces);

    for page in pages.iter().filter(|p| !p.is_index()) {
        let _lock = super::ingest::acquire_slug_lock(format!("{}:{}", kb_id, page.slug)).await;
        let Some(page) = page::get_by_id(pool, page.id).await? else {
            continue;
        };
        if page.status != STATUS_PUBLISHED {
            continue;
        }
        report.pages_scanned += 1;
        // 先抹掉指向已删除页面的链接，再注入新的交叉链接。
        let (cleaned, removed) = linkify::strip_dead_links(&page.content, &live_slugs);
        if removed {
            report.dead_links_removed += 1;
        }
        let outcome = linkifier.linkify(&cleaned, &page.slug);
        // 链接维护是机械改写：正文与出链一起更新，但保留页面原有的作者归属，
        // 否则人工编辑过的页面会被标成 pipeline，下次 ingest 就能覆盖掉用户内容。
        if page::update_content(pool, &page, &outcome.content, &outcome.out_links, None).await? {
            report.pages_changed += 1;
        }
    }

    page::rebuild_in_links(pool, kb_id).await?;

    if config.enabled {
        report.index_updated = rebuild_index(
            pool,
            kb_id,
            &config.language,
            pages.iter().filter(|p| !p.is_index()).collect(),
            find_index_page(&pages),
        )
        .await?;
    }

    info!(
        "wiki finalize: kb {} scanned={} changed={} dead_links={} index_updated={}",
        kb_id, report.pages_scanned, report.pages_changed, report.dead_links_removed, report.index_updated
    );
    Ok(report)
}

fn find_index_page<'a>(pages: &'a [super::WikiPage]) -> Option<&'a super::WikiPage> {
    pages.iter().find(|p| p.is_index())
}

/// 重建索引页。目录由系统确定性生成，模型只写导言；目录未变时完全不触发 LLM。
async fn rebuild_index(
    pool: &SqlitePool, kb_id: i64, language: &str, pages: Vec<&super::WikiPage>, existing: Option<&super::WikiPage>,
) -> Result<bool> {
    let directory = render_directory(&pages);
    let existing_directory = existing.and_then(|p| p.content.split_once(DIRECTORY_MARKER)).map(|(_, rest)| rest.trim());
    if let Some(previous) = existing_directory {
        if previous == directory.trim() {
            debug!("wiki finalize: kb {} index directory unchanged, skipping intro generation", kb_id);
            return Ok(false);
        }
    }

    let intro = match generate_intro(pool, kb_id, language, &directory).await {
        Ok(intro) => intro,
        Err(error) => {
            // 导言只是锦上添花：失败时退回已有导言或静态兜底，不让整个 finalize 失败。
            warn!("wiki finalize: intro generation failed for kb {}: {}", kb_id, error);
            existing.map(|p| p.summary.clone()).filter(|s| !s.is_empty()).unwrap_or_else(default_intro)
        }
    };

    let content = format!("{}\n\n{}\n{}", intro.trim(), DIRECTORY_MARKER, directory);
    let draft = PageDraft {
        kb_id,
        slug: INDEX_SLUG.to_string(),
        title: "知识库 Wiki 索引".to_string(),
        page_type: PAGE_TYPE_INDEX.to_string(),
        summary: intro.trim().to_string(),
        content,
        aliases: Vec::new(),
        edit_source: EDIT_SOURCE_PIPELINE.to_string(),
        editor_id: String::new(),
    };
    let _lock = super::ingest::acquire_slug_lock(format!("{}:{}", kb_id, INDEX_SLUG)).await;
    let current = page::list(pool, kb_id, None, Some(STATUS_PUBLISHED), MAX_PAGES_PER_FINALIZE as i64, None).await?;
    anyhow::ensure!(
        render_directory(&current.iter().filter(|p| !p.is_index()).collect::<Vec<_>>()) == directory,
        "Wiki pages changed during index generation; retry finalize"
    );
    let outcome = page::upsert(pool, &draft, &[], &[]).await?;
    Ok(outcome.changed)
}

async fn generate_intro(pool: &SqlitePool, kb_id: i64, language: &str, directory: &str) -> Result<String> {
    let _ = pool;
    let config = crate::config::get();
    let llm = WikiLlm::new(None);
    if !llm.is_enabled() || !config.wiki.enabled {
        anyhow::bail!("wiki LLM not available");
    }
    let _ = kb_id;
    let user = prompts::render(prompts::INDEX_INTRO_USER, &[("Directory", directory), ("Language", language)]);
    let intro = llm.chat(prompts::INDEX_INTRO_SYSTEM, &user, 1024, 0.4).await?;
    let cleaned = intro.trim();
    if cleaned.is_empty() {
        anyhow::bail!("empty intro");
    }
    Ok(cleaned.to_string())
}

fn default_intro() -> String {
    "本 Wiki 由系统根据知识库文档自动整理，按实体、概念与文档摘要分组，页面之间通过内链互相关联。".to_string()
}

/// 确定性生成目录段：按类型分组、组内按标题排序。
///
/// 不让模型生成目录，避免它编造不存在的条目或漏掉真实页面。
pub fn render_directory(pages: &[&super::WikiPage]) -> String {
    let mut out = String::new();
    for (page_type, heading) in
        [(PAGE_TYPE_ENTITY, "实体"), (PAGE_TYPE_CONCEPT, "概念"), (PAGE_TYPE_SUMMARY, "文档摘要")]
    {
        let mut group: Vec<&super::WikiPage> = pages.iter().filter(|p| p.page_type == page_type).copied().collect();
        if group.is_empty() {
            continue;
        }
        group.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.slug.cmp(&b.slug)));
        out.push_str(&format!("## {}（{}）\n\n", heading, group.len()));
        for (index, page) in group.iter().enumerate() {
            if index >= MAX_INDEX_ENTRIES_PER_GROUP {
                out.push_str(&format!("- ……（仅显示前 {} 条，共 {} 条）\n", MAX_INDEX_ENTRIES_PER_GROUP, group.len()));
                break;
            }
            let title = if page.title.trim().is_empty() { document_title(&page.slug) } else { page.title.clone() };
            let summary = page.summary.trim();
            if summary.is_empty() {
                out.push_str(&format!("- [[{}|{}]]\n", page.slug, title));
            } else {
                out.push_str(&format!("- [[{}|{}]] — {}\n", page.slug, title, prompts::truncate_chars(summary, 120)));
            }
        }
        out.push('\n');
    }
    if out.is_empty() {
        out.push_str("_（暂无页面，等待文档生成）_\n");
    }
    out.trim_end().to_string()
}
