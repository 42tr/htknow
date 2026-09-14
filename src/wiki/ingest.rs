//! Wiki 生成管道：单文档 map + 按 slug reduce。
//!
//! 候选条目有两种来源：
//! - **模式 A（默认）**：复用知识图谱已产出的「实体 → 切片」证据（`graph_node_sources`），
//!   省掉候选抽取与引用归类两遍 LLM 调用。
//! - **模式 B（回退）**：图谱未开启或该文件没有图数据时，独立跑候选抽取 + 引用归类。
//!
//! 所有 LLM 调用都在数据库事务之外完成，写库只用短事务——与图谱构建的既有约定一致。

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Result, anyhow, bail};
use futures::stream::{self, StreamExt};
use log::{debug, info, warn};
use once_cell::sync::Lazy;
use serde::Deserialize;
use sqlx::SqlitePool;
use tokio::sync::Mutex as TokioMutex;

use super::{
    EDIT_SOURCE_PIPELINE, Granularity, PAGE_TYPE_CONCEPT, PAGE_TYPE_ENTITY, PAGE_TYPE_SUMMARY, ResolvedWikiConfig,
    llm::WikiLlm,
    make_slug,
    page::{self, PageDraft},
    prompts::{self, EvidenceChunk},
    resolve_config, summary_slug,
};

/// 生成一篇文档的 Wiki 页面所需的最小正文长度（字符）。低于此值说明解析产物不可用，
/// 不让模型对着空内容编故事。
const MIN_REAL_TEXT_CHARS: usize = 40;

/// 每个切片的证据最多保留的字符数。
const MAX_CHARS_PER_EVIDENCE_SLICE: usize = 4000;

/// 送入模型的候选页面清单上限，防止大知识库把 prompt 撑爆。
const MAX_AVAILABLE_PAGES: usize = 400;

/// 一个候选条目。
#[derive(Debug, Clone)]
pub struct Candidate {
    pub slug: String,
    pub name: String,
    pub page_type: String,
    pub description: String,
    pub aliases: Vec<String>,
    /// 实质性讨论该条目的切片。
    pub slice_ids: Vec<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct IngestReport {
    pub mode: &'static str,
    pub candidates: usize,
    pub pages_written: usize,
    pub pages_changed: usize,
    pub skipped: bool,
}

#[derive(Debug, Deserialize)]
struct CandidateItem {
    #[serde(default)]
    name: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    details: String,
}

#[derive(Debug, Deserialize)]
struct CandidateResponse {
    #[serde(default)]
    entities: Vec<CandidateItem>,
    #[serde(default)]
    concepts: Vec<CandidateItem>,
}

#[derive(Debug, Deserialize)]
struct CitationResponse {
    #[serde(default)]
    citations: HashMap<String, Vec<String>>,
}

// ---------------------------------------------------------------------------
// 同 slug 写入串行化
// ---------------------------------------------------------------------------

static SLUG_LOCKS: Lazy<std::sync::Mutex<HashMap<String, Arc<TokioMutex<()>>>>> =
    Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

/// 页面写锁守卫。持有期间同一 `(kb_id, slug)` 的读-改-写互斥。
pub struct SlugGuard {
    key: String,
    // 用 owned guard：闭包形式的 `with_slug_lock(key, || async {...})` 会让调用方
    // future 的 Send 证明退化成高阶生命周期问题，在 tokio::spawn 中无法通过。
    _permit: tokio::sync::OwnedMutexGuard<()>,
}

impl Drop for SlugGuard {
    fn drop(&mut self) {
        // 无人等待时回收条目，避免 slug 数量随知识库增长而无界膨胀。
        // 字段在本方法之后才析构，因此 strong_count == 2 表示只剩 map 与本守卫。
        let mut map = SLUG_LOCKS.lock().expect("wiki slug lock map poisoned");
        if let Some(lock) = map.get(&self.key) {
            if Arc::strong_count(lock) == 2 {
                map.remove(&self.key);
            }
        }
    }
}

/// 串行化同一个页面的读-改-写。
///
/// 单实例部署下进程内锁就足够；不同文档并发贡献同一实体时，若不串行化，
/// 后写的会用旧内容覆盖前一次的合并结果。
pub async fn acquire_slug_lock(key: String) -> SlugGuard {
    let lock = {
        let mut map = SLUG_LOCKS.lock().expect("wiki slug lock map poisoned");
        map.entry(key.clone()).or_insert_with(|| Arc::new(TokioMutex::new(()))).clone()
    };
    SlugGuard { key, _permit: lock.lock_owned().await }
}

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

/// 为一篇文档生成/更新 Wiki 页面。
pub async fn ingest_file(pool: &SqlitePool, kb_id: i64, file_id: i64) -> Result<IngestReport> {
    let Some(config) = resolve_config(pool, kb_id).await? else {
        debug!("wiki ingest: kb {} not found, skipping file {}", kb_id, file_id);
        return Ok(IngestReport { skipped: true, ..Default::default() });
    };
    if !config.enabled {
        debug!("wiki ingest: kb {} has wiki disabled, skipping file {}", kb_id, file_id);
        return Ok(IngestReport { skipped: true, ..Default::default() });
    }

    let llm = WikiLlm::new(config.model.as_deref());
    if !llm.is_enabled() {
        bail!("wiki ingest: LLM not configured");
    }

    let row: Option<(String, i64)> = sqlx::query_as("SELECT filename, status FROM files WHERE id = ? AND kb_id = ?")
        .bind(file_id)
        .bind(kb_id)
        .fetch_optional(pool)
        .await?;
    let Some((filename, status)) = row else {
        debug!("wiki ingest: file {} not found in kb {}, skipping", file_id, kb_id);
        return Ok(IngestReport { skipped: true, ..Default::default() });
    };
    if status != 1 {
        // 文档还没解析完成（或已失败），此时生成只会产出空页面。
        debug!("wiki ingest: file {} status {} is not completed, skipping", file_id, status);
        return Ok(IngestReport { skipped: true, ..Default::default() });
    }

    let (_effective_file_id, slices) = load_slices(pool, file_id).await?;
    let total_chars: usize = slices.iter().map(|(_, content)| content.chars().count()).sum();
    if slices.is_empty() || total_chars < MIN_REAL_TEXT_CHARS {
        info!("wiki ingest: file {} has insufficient text ({} chars), skipping", file_id, total_chars);
        mark_build(pool, file_id, "completed", "", &llm.model(), 0, "").await?;
        return Ok(IngestReport { skipped: true, ..Default::default() });
    }

    // 指纹覆盖切片内容与生成配置：内容或模型变了才重新生成。
    let fingerprint = build_fingerprint(&llm.model(), &config, &slices);
    if is_build_unchanged(pool, file_id, &fingerprint).await? {
        debug!("wiki ingest: file {} unchanged, skipping", file_id);
        return Ok(IngestReport { skipped: true, ..Default::default() });
    }
    let run_id = mark_build_running(pool, file_id, &fingerprint, &llm.model()).await?;

    let result = run_ingest(pool, &llm, &config, kb_id, file_id, &filename, &slices).await;
    match &result {
        Ok(report) => {
            mark_build_done(pool, file_id, &run_id, report.pages_written, "").await?;
            if !report.skipped {
                super::queue::enqueue_finalize(pool, kb_id).await?;
            }
        }
        Err(error) => {
            mark_build_done(pool, file_id, &run_id, 0, &error.to_string()).await?;
        }
    }
    result
}

async fn run_ingest(
    pool: &SqlitePool, llm: &WikiLlm, config: &ResolvedWikiConfig, kb_id: i64, file_id: i64, filename: &str,
    slices: &[(i64, String)],
) -> Result<IngestReport> {
    let content_by_slice: HashMap<i64, String> = slices.iter().cloned().collect();
    let surfaces = page::list_surfaces(pool, kb_id).await?;
    let available = prompts::available_pages(&surfaces, MAX_AVAILABLE_PAGES);

    // MAP：候选条目。模式 A 失败（例如图谱未建）时自动回退到模式 B。
    let graph_enabled = crate::config::get().server.build_knowledge_graph;
    let (mode, mut candidates) = match candidates_from_graph(pool, kb_id, file_id, config, graph_enabled).await? {
        Some(found) if !found.is_empty() => ("graph", found),
        _ => ("llm", candidates_from_llm(pool, llm, config, kb_id, slices).await?),
    };

    // 证据多的条目优先，配合 max_pages_per_ingest 保证预算花在最值得写的页面上。
    candidates.sort_by(|a, b| b.slice_ids.len().cmp(&a.slice_ids.len()).then_with(|| a.slug.cmp(&b.slug)));
    if config.max_pages_per_ingest > 0 && candidates.len() > config.max_pages_per_ingest {
        debug!(
            "wiki ingest: file {} has {} candidates, capping to {}",
            file_id,
            candidates.len(),
            config.max_pages_per_ingest
        );
        candidates.truncate(config.max_pages_per_ingest);
    }

    let mut report = IngestReport { mode, candidates: candidates.len(), ..Default::default() };

    // 摘要页是文档入库后的头号产物，失败即整体失败并重试；条目页失败只跳过该页。
    match write_summary_page(pool, llm, config, kb_id, file_id, filename, slices, &available).await {
        Ok(changed) => {
            report.pages_written += 1;
            report.pages_changed += changed as usize;
        }
        Err(error) => {
            warn!("wiki ingest: summary page failed for file {}: {}", file_id, error);
            return Err(error);
        }
    }

    // 摘要页写入后，条目页可以链到它；重取一次表面词清单。
    let surfaces = page::list_surfaces(pool, kb_id).await?;
    let available = prompts::available_pages(&surfaces, MAX_AVAILABLE_PAGES);

    // REDUCE：按 slug 并发写页面，同 slug 由 slug 锁串行化。
    //
    // 先把每个 future 造成「拥有全部输入」的形态再交给 `buffer_unordered`。
    // 不用「闭包 + `async move` 借用捕获」的写法：那种 future 的 Send 证明会退化成
    // 高阶生命周期问题，在 `tokio::spawn` 的 worker 链路里报
    // "implementation of `Send` is not general enough"。
    let cfg = crate::config::get();
    let parallel = cfg.wiki.reduce_parallel.max(1);
    let content_by_slice = Arc::new(content_by_slice);
    let mut pending = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        pending.push(write_candidate_page_task(
            pool.clone(),
            llm.clone(),
            config.clone(),
            kb_id,
            file_id,
            candidate,
            content_by_slice.clone(),
            available.clone(),
        ));
    }
    let outcomes: Vec<Result<bool>> = stream::iter(pending).buffer_unordered(parallel).collect().await;

    for outcome in outcomes {
        match outcome {
            Ok(changed) => {
                report.pages_written += 1;
                report.pages_changed += changed as usize;
            }
            Err(error) => warn!("wiki ingest: page generation failed for file {}: {}", file_id, error),
        }
    }

    info!(
        "wiki ingest: file {} mode={} candidates={} pages={} changed={}",
        file_id, report.mode, report.candidates, report.pages_written, report.pages_changed
    );
    Ok(report)
}

/// 读取文档的切片正文。切片可能来自共享解析产物，因此按 `effective_parse_file_id` 读。
async fn load_slices(pool: &SqlitePool, file_id: i64) -> Result<(i64, Vec<(i64, String)>)> {
    let effective_file_id = crate::api::effective_parse_file_id(pool, file_id).await?;
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM slices WHERE file_id = ? ORDER BY id")
        .bind(effective_file_id)
        .fetch_all(pool)
        .await?;
    let contents = crate::slice_content::read_all(effective_file_id).await?;
    let slices = ids
        .into_iter()
        .map(|id| (id, contents.get(&id).cloned().unwrap_or_default()))
        .filter(|(_, content)| !content.trim().is_empty())
        .collect();
    Ok((effective_file_id, slices))
}

fn build_fingerprint(model: &str, config: &ResolvedWikiConfig, slices: &[(i64, String)]) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"htknow-wiki-v1");
    digest.update(model.as_bytes());
    digest.update(config.granularity.as_str().as_bytes());
    digest.update(config.language.as_bytes());
    for (id, content) in slices {
        digest.update(id.to_le_bytes());
        digest.update((content.len() as u64).to_le_bytes());
        digest.update(content.as_bytes());
    }
    hex::encode(digest.finalize())
}

async fn is_build_unchanged(pool: &SqlitePool, file_id: i64, fingerprint: &str) -> Result<bool> {
    let unchanged: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM wiki_builds WHERE file_id = ? AND status = 'completed' AND fingerprint = ?)",
    )
    .bind(file_id)
    .bind(fingerprint)
    .fetch_one(pool)
    .await?;
    Ok(unchanged)
}

async fn mark_build_running(pool: &SqlitePool, file_id: i64, fingerprint: &str, model: &str) -> Result<String> {
    let run_id: String = sqlx::query_scalar(
        "INSERT INTO wiki_builds(file_id, status, run_id, fingerprint, model)
         VALUES(?, 'running', lower(hex(randomblob(16))), ?, ?)
         ON CONFLICT(file_id) DO UPDATE SET status = 'running', run_id = excluded.run_id,
             fingerprint = excluded.fingerprint, model = excluded.model, error = '',
             updated_at = strftime('%s','now')
         RETURNING run_id",
    )
    .bind(file_id)
    .bind(fingerprint)
    .bind(model)
    .fetch_one(pool)
    .await?;
    Ok(run_id)
}

async fn mark_build_done(pool: &SqlitePool, file_id: i64, run_id: &str, page_count: usize, error: &str) -> Result<()> {
    let status = if error.is_empty() { "completed" } else { "failed" };
    sqlx::query(
        "UPDATE wiki_builds SET status = ?, page_count = ?, error = ?, updated_at = strftime('%s','now')
          WHERE file_id = ? AND run_id = ?",
    )
    .bind(status)
    .bind(page_count as i64)
    .bind(error)
    .bind(file_id)
    .bind(run_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_build(
    pool: &SqlitePool, file_id: i64, status: &str, fingerprint: &str, model: &str, pages: usize, error: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO wiki_builds(file_id, status, run_id, fingerprint, model, page_count, error)
         VALUES(?, ?, '', ?, ?, ?, ?)
         ON CONFLICT(file_id) DO UPDATE SET status = excluded.status, fingerprint = excluded.fingerprint,
             model = excluded.model, page_count = excluded.page_count, error = excluded.error,
             updated_at = strftime('%s','now')",
    )
    .bind(file_id)
    .bind(status)
    .bind(fingerprint)
    .bind(model)
    .bind(pages as i64)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 候选条目
// ---------------------------------------------------------------------------

/// 图谱实体类型是 LLM 自定义的自由文本，这里用关键词把它归到 entity / concept 两类，
/// 仅用于索引分组与 slug 前缀，判断不确定时保守归为 entity。
pub fn classify_page_type(entity_type: &str) -> &'static str {
    const CONCEPT_HINTS: &[&str] =
        &["概念", "理论", "方法", "技术", "术语", "主题", "思想", "模式", "原则", "流程", "算法"];
    let normalized = entity_type.trim().to_ascii_lowercase();
    if CONCEPT_HINTS.iter().any(|hint| normalized.contains(hint)) {
        PAGE_TYPE_CONCEPT
    } else if normalized.contains("concept") || normalized.contains("topic") || normalized.contains("method") {
        PAGE_TYPE_CONCEPT
    } else {
        PAGE_TYPE_ENTITY
    }
}

/// 模式 A：从图谱产物构造候选。返回 `None` 表示图谱未启用或该文件没有图数据。
///
/// `graph_enabled` 由调用方传入而不是在函数里读全局配置：这样测试可以直接覆盖两种取值。
pub(crate) async fn candidates_from_graph(
    pool: &SqlitePool, kb_id: i64, file_id: i64, config: &ResolvedWikiConfig, graph_enabled: bool,
) -> Result<Option<Vec<Candidate>>> {
    if !graph_enabled {
        return Ok(None);
    }
    let rows: Vec<(i64, String, String, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT n.id, n.name, n.entity_type, n.properties, s.slice_id
           FROM graph_nodes n
           JOIN graph_node_sources s ON s.node_id = n.id
          WHERE s.file_id = ? AND n.kb_id = ?
          ORDER BY n.id, s.slice_id",
    )
    .bind(file_id)
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }

    // focused 粒度下只保留证据最多的前若干条目，避免顺带提及的技术名词撑爆索引。
    let mut grouped: Vec<(String, String, String, String, HashSet<i64>)> = Vec::new();
    let mut index: HashMap<i64, usize> = HashMap::new();
    for (node_id, name, entity_type, properties, slice_id) in rows {
        let position = *index.entry(node_id).or_insert_with(|| {
            let description = properties
                .as_deref()
                .and_then(|raw| serde_json::from_str::<HashMap<String, String>>(raw).ok())
                .and_then(|map| map.get("description").cloned())
                .unwrap_or_default();
            grouped.push((name, entity_type, description, String::new(), HashSet::new()));
            grouped.len() - 1
        });
        if let Some(slice_id) = slice_id {
            grouped[position].4.insert(slice_id);
        }
    }

    let mut candidates: Vec<Candidate> = grouped
        .into_iter()
        .filter(|(name, _, _, _, slices)| !name.trim().is_empty() && !slices.is_empty())
        .map(|(name, entity_type, description, _, slices)| {
            let page_type = classify_page_type(&entity_type);
            let mut slice_ids: Vec<i64> = slices.into_iter().collect();
            slice_ids.sort_unstable();
            Candidate {
                slug: make_slug(page_type, &name),
                name: name.clone(),
                page_type: page_type.to_string(),
                description: description.trim().to_string(),
                aliases: Vec::new(),
                slice_ids,
            }
        })
        .collect();

    // 同名不同类型的实体可能撞出同一个 slug，合并它们的证据而不是相互覆盖。
    candidates.sort_by(|a, b| a.slug.cmp(&b.slug));
    let mut merged: Vec<Candidate> = Vec::new();
    for candidate in candidates {
        match merged.last_mut() {
            Some(last) if last.slug == candidate.slug => {
                last.slice_ids.extend(candidate.slice_ids);
                last.slice_ids.sort_unstable();
                last.slice_ids.dedup();
                if last.description.is_empty() {
                    last.description = candidate.description;
                }
            }
            _ => merged.push(candidate),
        }
    }
    if config.granularity == Granularity::Focused {
        const FOCUSED_LIMIT: usize = 12;
        merged.sort_by(|a, b| b.slice_ids.len().cmp(&a.slice_ids.len()).then_with(|| a.slug.cmp(&b.slug)));
        merged.truncate(FOCUSED_LIMIT);
    }
    Ok(Some(merged))
}

/// 模式 B：候选抽取（Pass 0）+ 引用归类（Pass 1..N）。
async fn candidates_from_llm(
    pool: &SqlitePool, llm: &WikiLlm, config: &ResolvedWikiConfig, kb_id: i64, slices: &[(i64, String)],
) -> Result<Vec<Candidate>> {
    let existing_slugs: Vec<(String,)> =
        sqlx::query_as("SELECT slug FROM wiki_pages WHERE kb_id = ? AND page_type != 'index' ORDER BY slug LIMIT ?")
            .bind(kb_id)
            .bind(MAX_AVAILABLE_PAGES as i64)
            .fetch_all(pool)
            .await?;
    let existing = if existing_slugs.is_empty() {
        "(暂无)".to_string()
    } else {
        existing_slugs.into_iter().map(|(slug,)| slug).collect::<Vec<_>>().join("\n")
    };

    let document = full_document_text(slices);
    let user = prompts::render(
        prompts::CANDIDATE_EXTRACT_USER,
        &[
            ("Granularity", config.granularity.guidance()),
            ("ExtraInstructions", ""),
            ("ExistingSlugs", &existing),
            ("Content", &document),
            ("Language", &config.language),
        ],
    );
    let response: CandidateResponse =
        llm.chat_json(prompts::CANDIDATE_EXTRACT_SYSTEM, &user, crate::config::get().wiki.llm_max_tokens, 0.2).await?;

    let mut candidates: Vec<Candidate> = Vec::new();
    for (items, page_type) in [(&response.entities, PAGE_TYPE_ENTITY), (&response.concepts, PAGE_TYPE_CONCEPT)] {
        for item in items {
            let name = item.name.trim();
            if name.is_empty() {
                continue;
            }
            let slug = item
                .slug
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty() && value.contains('/'))
                .map(str::to_string)
                .unwrap_or_else(|| make_slug(page_type, name));
            let description = if item.description.trim().is_empty() {
                item.details.trim().to_string()
            } else {
                item.description.trim().to_string()
            };
            candidates.push(Candidate {
                slug,
                name: name.to_string(),
                page_type: page_type.to_string(),
                description,
                aliases: item.aliases.iter().map(|alias| alias.trim().to_string()).filter(|a| !a.is_empty()).collect(),
                slice_ids: Vec::new(),
            });
        }
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    // 同一 slug 只保留一条，避免引用归类结果被拆散。
    let mut seen: HashSet<String> = HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.slug.clone()));

    let citations = classify_citations(llm, config, &candidates, slices).await?;
    for candidate in &mut candidates {
        candidate.slice_ids = citations.get(&candidate.slug).cloned().unwrap_or_default();
    }
    // 没有任何切片支撑的条目退化为「按全文证据生成」，由 write_candidate_page 兜底。
    Ok(candidates)
}

/// 拼接文档全文（受 `max_source_chars` 约束）。
fn full_document_text(slices: &[(i64, String)]) -> String {
    let budget = crate::config::get().wiki.max_source_chars;
    let joined: String = slices.iter().map(|(_, content)| content.as_str()).collect::<Vec<_>>().join("\n\n");
    prompts::truncate_chars(&joined, budget)
}

/// 引用归类：按字符预算切批，批内并发，把模型返回的短句柄映射回真实切片 ID。
async fn classify_citations(
    llm: &WikiLlm, config: &ResolvedWikiConfig, candidates: &[Candidate], slices: &[(i64, String)],
) -> Result<HashMap<String, Vec<i64>>> {
    let budget = crate::config::get().wiki.max_source_chars;
    let mut batches: Vec<Vec<(String, i64, String)>> = Vec::new();
    let mut current: Vec<(String, i64, String)> = Vec::new();
    let mut used = 0usize;
    let mut counter = 0usize;
    let mut handle_to_slice: HashMap<String, i64> = HashMap::new();
    for (slice_id, content) in slices {
        counter += 1;
        let handle = format!("c{:03}", counter);
        let text = prompts::truncate_chars(content, MAX_CHARS_PER_EVIDENCE_SLICE);
        let cost = text.chars().count();
        if !current.is_empty() && budget > 0 && used + cost > budget {
            batches.push(std::mem::take(&mut current));
            used = 0;
        }
        handle_to_slice.insert(handle.clone(), *slice_id);
        current.push((handle, *slice_id, text));
        used += cost;
    }
    if !current.is_empty() {
        batches.push(current);
    }
    if batches.is_empty() {
        return Ok(HashMap::new());
    }

    let candidate_xml = prompts::render_candidates(
        &candidates.iter().map(|c| (c.slug.as_str(), c.name.as_str(), c.description.as_str())).collect::<Vec<_>>(),
    );
    let parallel = crate::config::get().wiki.citation_parallel.max(1);
    // 同 REDUCE：拥有所有权的 future，避免借用批次内容跨 await。
    let mut pending = Vec::with_capacity(batches.len());
    for batch in batches {
        pending.push(citation_batch_task(llm.clone(), candidate_xml.clone(), config.language.clone(), batch));
    }
    let responses: Vec<Result<CitationResponse>> = stream::iter(pending).buffer_unordered(parallel).collect().await;

    let known: HashSet<&str> = candidates.iter().map(|c| c.slug.as_str()).collect();
    let mut merged: HashMap<String, Vec<i64>> = HashMap::new();
    for response in responses {
        // 单批失败不整体放弃：其余批次的引用仍然有效，条目页会退化为按简介生成。
        let parsed = match response {
            Ok(parsed) => parsed,
            Err(error) => {
                warn!("wiki ingest: citation batch failed: {}", error);
                continue;
            }
        };
        for (slug, handles) in parsed.citations {
            if !known.contains(slug.as_str()) {
                debug!("wiki ingest: citation referenced unknown slug {}", slug);
                continue;
            }
            let entry = merged.entry(slug).or_default();
            for handle in handles {
                if let Some(slice_id) = handle_to_slice.get(handle.trim()) {
                    entry.push(*slice_id);
                }
            }
        }
    }
    for slice_ids in merged.values_mut() {
        slice_ids.sort_unstable();
        slice_ids.dedup();
    }
    Ok(merged)
}

// ---------------------------------------------------------------------------
// 页面写入
// ---------------------------------------------------------------------------

fn evidence_chunks(slice_ids: &[i64], content_by_slice: &HashMap<i64, String>) -> Vec<EvidenceChunk> {
    slice_ids
        .iter()
        .enumerate()
        .filter_map(|(index, slice_id)| {
            let content = content_by_slice.get(slice_id)?;
            Some(EvidenceChunk {
                handle: format!("c{:03}", index + 1),
                slice_id: *slice_id,
                content: prompts::truncate_chars(content, MAX_CHARS_PER_EVIDENCE_SLICE),
            })
        })
        .collect()
}

async fn write_summary_page(
    pool: &SqlitePool, llm: &WikiLlm, config: &ResolvedWikiConfig, kb_id: i64, file_id: i64, filename: &str,
    slices: &[(i64, String)], available: &str,
) -> Result<bool> {
    let document = full_document_text(slices);
    let user = prompts::render(
        prompts::DOC_SUMMARY_USER,
        &[("Content", &document), ("AvailablePages", available), ("Language", &config.language)],
    );
    let raw = llm.chat(prompts::DOC_SUMMARY_SYSTEM, &user, crate::config::get().wiki.llm_max_tokens, 0.3).await?;
    let (summary, content) = prompts::split_summary_line(&raw);
    if content.trim().is_empty() {
        bail!("summary page content is empty for file {}", file_id);
    }
    let slug = summary_slug(file_id);
    let title = document_title(filename);
    let summary = if summary.is_empty() { first_sentence(&content) } else { summary };
    let draft = PageDraft {
        kb_id,
        slug: slug.clone(),
        title,
        page_type: PAGE_TYPE_SUMMARY.to_string(),
        summary,
        content: content.clone(),
        aliases: Vec::new(),
        edit_source: EDIT_SOURCE_PIPELINE.to_string(),
        editor_id: String::new(),
    };
    // 摘要页是整篇文档的产物，把全部切片登记为证据，前端才能从摘要页直接跳到原文高亮。
    let slice_ids: Vec<i64> = slices.iter().map(|(slice_id, _)| *slice_id).collect();
    let outcome = page::upsert(pool, &draft, &[file_id], &slice_ids).await?;
    page::set_out_links(pool, outcome.page_id, &super::linkify::out_links(&content, &slug)).await?;
    Ok(outcome.changed)
}

/// `write_candidate_page` 的自有数据版本，供并发 future 使用。
async fn write_candidate_page_task(
    pool: SqlitePool, llm: WikiLlm, config: ResolvedWikiConfig, kb_id: i64, file_id: i64, candidate: Candidate,
    content_by_slice: Arc<HashMap<i64, String>>, available: String,
) -> Result<bool> {
    write_candidate_page(&pool, &llm, &config, kb_id, file_id, &candidate, &content_by_slice, &available).await
}

/// 单个引用归类批次的 LLM 调用。
async fn citation_batch_task(
    llm: WikiLlm, candidate_xml: String, language: String, batch: Vec<(String, i64, String)>,
) -> Result<CitationResponse> {
    let chunks = batch
        .iter()
        .map(|(handle, _, text)| format!("<c id=\"{}\">\n{}\n</c>", handle, text.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    let user = prompts::render(
        prompts::CHUNK_CITATION_USER,
        &[("Candidates", &candidate_xml), ("Chunks", &chunks), ("Language", &language)],
    );
    llm.chat_json::<CitationResponse>(
        prompts::CHUNK_CITATION_SYSTEM,
        &user,
        crate::config::get().wiki.llm_max_tokens,
        0.0,
    )
    .await
}

async fn write_candidate_page(
    pool: &SqlitePool, llm: &WikiLlm, config: &ResolvedWikiConfig, kb_id: i64, file_id: i64, candidate: &Candidate,
    content_by_slice: &HashMap<i64, String>, available: &str,
) -> Result<bool> {
    let slug = candidate.slug.clone();
    let _lock = acquire_slug_lock(format!("{}:{}", kb_id, slug)).await;
    {
        let existing = page::get_by_slug(pool, kb_id, &slug).await?;
        let evidence_slice_ids = if candidate.slice_ids.is_empty() {
            // 引用归类没给出证据时，退化到该文档的全部切片，仍然只依据原文写作。
            content_by_slice.keys().copied().collect::<Vec<i64>>()
        } else {
            candidate.slice_ids.clone()
        };
        let mut ordered: Vec<i64> = evidence_slice_ids.into_iter().collect::<HashSet<_>>().into_iter().collect();
        ordered.sort_unstable();
        let chunks = evidence_chunks(&ordered, content_by_slice);
        let evidence = prompts::render_evidence(&chunks, crate::config::get().wiki.max_source_chars);

        let existing_section = match &existing {
            Some(existing) if !existing.content.trim().is_empty() => {
                prompts::render(prompts::PAGE_BODY_EXISTING_SECTION, &[("ExistingContent", &existing.content)])
            }
            _ => String::new(),
        };
        let user = prompts::render(
            prompts::PAGE_BODY_USER,
            &[
                ("Title", &candidate.name),
                ("PageType", &candidate.page_type),
                ("Description", &candidate.description),
                ("Evidence", &evidence),
                ("ExistingSection", &existing_section),
                ("AvailablePages", available),
                ("Language", &config.language),
            ],
        );
        let content = llm.chat(prompts::PAGE_BODY_SYSTEM, &user, crate::config::get().wiki.llm_max_tokens, 0.3).await?;
        let content = strip_code_fence(content.trim());
        if content.is_empty() {
            bail!("generated empty body for {}", slug);
        }

        let mut aliases = candidate.aliases.clone();
        if let Some(existing) = &existing {
            for alias in &existing.aliases {
                if !aliases.contains(alias) {
                    aliases.push(alias.clone());
                }
            }
        }
        let title = existing
            .as_ref()
            .map(|p| p.title.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| candidate.name.clone());
        let summary = existing
            .as_ref()
            .map(|p| p.summary.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| candidate.description.clone());
        let draft = PageDraft {
            kb_id,
            slug: slug.clone(),
            title,
            page_type: candidate.page_type.clone(),
            summary,
            content: content.clone(),
            aliases,
            edit_source: EDIT_SOURCE_PIPELINE.to_string(),
            editor_id: String::new(),
        };
        let outcome = page::upsert(pool, &draft, &[file_id], &ordered).await?;
        page::set_out_links(pool, outcome.page_id, &super::linkify::out_links(&content, &slug)).await?;
        Ok(outcome.changed)
    }
}

/// 用剩余来源的切片证据确定性重建页面（回撤后调用）。
///
/// 不让模型「减去某个文档的贡献」——那既难验证也容易把仍然成立的内容删掉。
/// 直接按剩余证据重写，结果只取决于当前还活着的来源。
pub async fn refresh_page(pool: &SqlitePool, kb_id: i64, slug: &str) -> Result<bool> {
    let Some(existing) = page::get_by_slug(pool, kb_id, slug).await? else {
        debug!("wiki refresh: page {} not found in kb {}", slug, kb_id);
        return Ok(false);
    };
    let source_ids = page::source_file_ids(pool, existing.id).await?;
    let slice_ids = page::slice_ref_ids(pool, existing.id).await?;
    if source_ids.is_empty() || slice_ids.is_empty() {
        info!("wiki refresh: page {} lost all evidence, deleting", slug);
        page::delete_by_id(pool, existing.id).await?;
        return Ok(true);
    }

    let config = resolve_config(pool, kb_id).await?.ok_or_else(|| anyhow!("kb {} not found", kb_id))?;
    let llm = WikiLlm::new(config.model.as_deref());
    if !llm.is_enabled() {
        bail!("wiki refresh: LLM not configured");
    }

    // 切片可能分散在多个源文件（共享解析产物），逐文件读取后合并。
    let mut content_by_slice: HashMap<i64, String> = HashMap::new();
    for file_id in &source_ids {
        let effective = crate::api::effective_parse_file_id(pool, *file_id).await?;
        for (slice_id, content) in crate::slice_content::read_all(effective).await? {
            content_by_slice.insert(slice_id, content);
        }
    }
    let chunks = evidence_chunks(&slice_ids, &content_by_slice);
    if chunks.is_empty() {
        info!("wiki refresh: page {} has no readable evidence left, deleting", slug);
        page::delete_by_id(pool, existing.id).await?;
        return Ok(true);
    }

    let surfaces = page::list_surfaces(pool, kb_id).await?;
    let available = prompts::available_pages(&surfaces, MAX_AVAILABLE_PAGES);
    let evidence = prompts::render_evidence(&chunks, crate::config::get().wiki.max_source_chars);
    let user = prompts::render(
        prompts::PAGE_BODY_USER,
        &[
            ("Title", &existing.title),
            ("PageType", &existing.page_type),
            ("Description", &existing.summary),
            ("Evidence", &evidence),
            ("ExistingSection", ""),
            ("AvailablePages", &available),
            ("Language", &config.language),
        ],
    );
    let content = strip_code_fence(
        llm.chat(prompts::PAGE_BODY_SYSTEM, &user, crate::config::get().wiki.llm_max_tokens, 0.3).await?.trim(),
    );
    if content.is_empty() {
        bail!("wiki refresh: generated empty body for {}", slug);
    }

    let _lock = acquire_slug_lock(format!("{}:{}", kb_id, slug)).await;
    {
        let draft = PageDraft {
            kb_id,
            slug: slug.to_string(),
            title: existing.title.clone(),
            page_type: existing.page_type.clone(),
            summary: existing.summary.clone(),
            content: content.clone(),
            aliases: existing.aliases.clone(),
            edit_source: EDIT_SOURCE_PIPELINE.to_string(),
            editor_id: String::new(),
        };
        let outcome = page::upsert(pool, &draft, &[], &[]).await?;
        page::set_slice_refs(pool, outcome.page_id, &slice_ids).await?;
        page::set_out_links(pool, outcome.page_id, &super::linkify::out_links(&content, slug)).await?;
        Ok(outcome.changed)
    }
}

/// 文件名去掉扩展名作为摘要页标题。
pub fn document_title(filename: &str) -> String {
    let name = std::path::Path::new(filename).file_stem().and_then(|v| v.to_str()).unwrap_or(filename);
    let trimmed = name.trim();
    if trimmed.is_empty() { filename.to_string() } else { trimmed.to_string() }
}

/// 取正文首句作为兜底摘要。
fn first_sentence(content: &str) -> String {
    let cleaned: String = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or_default()
        .to_string();
    prompts::truncate_chars(&cleaned, 120)
}

/// 模型偶尔会把整篇输出包进代码围栏，这里剥掉最外层。
fn strip_code_fence(content: &str) -> String {
    let trimmed = content.trim();
    let without_language = trimmed
        .strip_prefix("```markdown")
        .or_else(|| trimmed.strip_prefix("```md"))
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    let body = without_language.strip_suffix("```").unwrap_or(without_language);
    body.trim().to_string()
}
