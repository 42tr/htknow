use std::{
    collections::{HashMap, HashSet}, path::Path, thread, time::{Duration, Instant}
};

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use log::{debug, warn};
use serde::Serialize;
use tantivy::{
    Index, IndexReader, Result, TantivyDocument, TantivyError, Term, collector::TopDocs, directory::error::LockError, doc, merge_policy::LogMergePolicy, query::{BooleanQuery, BoostQuery, Occur, Query, TermQuery}, schema::{FAST, Field, INDEXED, IndexRecordOption, STORED, Schema, TextFieldIndexing, TextOptions, Value as _}
};
use tokio::time::sleep;

use super::chinese_tokenizer;
use crate::config;

const ALL_TOKENIZER: &str = "all";
const INDEX_WRITER_LOCK_RETRY_MAX_ATTEMPTS: usize = 8;
const INDEX_WRITER_LOCK_RETRY_BASE_MS: u64 = 40;

#[derive(Debug, Clone)]
pub struct SynonymTerm {
    pub term: String,
    pub boost: f32,
}

pub type SynonymMap = HashMap<String, Vec<SynonymTerm>>;

#[derive(Clone)]
pub struct Document {
    pub id: i64,            // 切片 ID
    pub file_id: i64,       // 文件 ID
    pub kb_id: Option<i64>, // 知识库 ID
    pub content: String,    // 内容
}

/// 搜索结果项
#[derive(Debug, Clone, Serialize)]
pub struct SearchResultItem {
    pub id: i64,            // 切片 ID
    pub file_id: i64,       // 文件 ID
    pub kb_id: Option<i64>, // 知识库 ID
    pub content: String,    // 内容
    pub score: f32,         // 搜索得分
}

/// 全文索引搜索结果项
#[derive(Debug, Clone, Serialize)]
pub struct FullSearchResultItem {
    pub file_id: i64,       // 文件 ID
    pub kb_id: Option<i64>, // 知识库 ID
    pub snippet: String,    // 高亮片段
    pub score: f32,         // 搜索得分
}

#[derive(Debug, Clone, Serialize)]
pub struct ForceMergeStats {
    pub before_segments: usize,
    pub after_segments: usize,
    pub before_docs: u64,
    pub after_docs: u64,
    pub before_deleted_docs: u64,
    pub after_deleted_docs: u64,
    pub gc_deleted_files: usize,
    pub gc_failed_files: usize,
    pub skipped: bool,
    pub duration_ms: u128,
}

impl Document {
    pub fn new(id: i64, file_id: i64, kb_id: Option<i64>, content: String) -> Self {
        Document { id, file_id, kb_id, content }
    }
}

/// create new index
///
/// delete if it exists
pub fn init() -> Result<(Schema, Index)> {
    let cfg = config::get();
    init_with_path(&cfg.search.tantivy_index_path)
}

pub fn init_full() -> Result<(Schema, Index)> {
    let cfg = config::get();
    init_with_path(&cfg.search.tantivy_full_index_path)
}

pub fn init_with_path(path: &str) -> Result<(Schema, Index)> {
    let schema = build_schema();
    let path = Path::new(path);
    let index = if path.exists() {
        Index::open_in_dir(path)?
    } else {
        std::fs::create_dir_all(path)?;
        Index::create_in_dir(path, schema.clone())?
    };
    register_tokenizers(&index);
    Ok((schema, index))
}

fn is_lock_busy_error(err: &TantivyError) -> bool {
    matches!(err, TantivyError::LockFailure(LockError::LockBusy, _))
}

async fn create_writer(index: &Index) -> tantivy::Result<tantivy::IndexWriter> {
    let mut delay_ms = INDEX_WRITER_LOCK_RETRY_BASE_MS;

    for attempt in 1..=INDEX_WRITER_LOCK_RETRY_MAX_ATTEMPTS {
        match open_writer(index) {
            Ok(writer) => return Ok(writer),
            Err(err) if is_lock_busy_error(&err) && attempt < INDEX_WRITER_LOCK_RETRY_MAX_ATTEMPTS => {
                warn!(
                    "Tantivy index writer lock busy (attempt {}/{}), retry in {}ms",
                    attempt, INDEX_WRITER_LOCK_RETRY_MAX_ATTEMPTS, delay_ms
                );
                sleep(Duration::from_millis(delay_ms)).await;
                delay_ms = (delay_ms * 2).min(800);
            }
            Err(err) => return Err(err),
        }
    }

    unreachable!("create_writer retry loop should always return or error")
}

fn open_writer(index: &Index) -> tantivy::Result<tantivy::IndexWriter> {
    let writer_memory = config::get().search.tantivy_memory_mb * 1_000_000;
    let writer = index.writer(writer_memory)?;
    // 设置 merge policy，减少小 segment 数量
    writer.set_merge_policy(Box::new(LogMergePolicy::default()));
    Ok(writer)
}

fn create_writer_blocking(index: &Index) -> tantivy::Result<tantivy::IndexWriter> {
    let mut delay_ms = INDEX_WRITER_LOCK_RETRY_BASE_MS;

    for attempt in 1..=INDEX_WRITER_LOCK_RETRY_MAX_ATTEMPTS {
        match open_writer(index) {
            Ok(writer) => return Ok(writer),
            Err(err) if is_lock_busy_error(&err) && attempt < INDEX_WRITER_LOCK_RETRY_MAX_ATTEMPTS => {
                warn!(
                    "Tantivy index writer lock busy (attempt {}/{}), retry in {}ms",
                    attempt, INDEX_WRITER_LOCK_RETRY_MAX_ATTEMPTS, delay_ms
                );
                thread::sleep(Duration::from_millis(delay_ms));
                delay_ms = (delay_ms * 2).min(800);
            }
            Err(err) => return Err(err),
        }
    }

    unreachable!("create_writer_blocking retry loop should always return or error")
}

pub async fn create_writer_with_timing(index: &Index, label: &str) -> tantivy::Result<tantivy::IndexWriter> {
    let start = Instant::now();
    let writer = create_writer(index).await?;
    debug!("tantivy_writer_create label={} duration_ms={}", label, start.elapsed().as_millis());
    Ok(writer)
}

fn create_writer_with_timing_blocking(index: &Index, label: &str) -> tantivy::Result<tantivy::IndexWriter> {
    let start = Instant::now();
    let writer = create_writer_blocking(index)?;
    debug!("tantivy_writer_create label={} duration_ms={}", label, start.elapsed().as_millis());
    Ok(writer)
}

pub async fn write_documents(index: &Index, schema: &Schema, doc: Document) -> tantivy::Result<()> {
    let mut index_writer = create_writer(index).await?;
    index_writer.add_document(create_document(doc, schema))?;
    index_writer.commit()?;
    Ok(())
}

/// 批量写入文档，减少 commit 次数，避免产生大量小 segment
pub async fn write_documents_batch(index: &Index, schema: &Schema, docs: Vec<Document>) -> tantivy::Result<()> {
    if docs.is_empty() {
        return Ok(());
    }
    let mut index_writer = create_writer(index).await?;
    for doc in docs {
        index_writer.add_document(create_document(doc, schema))?;
    }
    index_writer.commit()?; // 一次 commit
    Ok(())
}

pub fn search_sync(
    reader: &IndexReader, schema: &Schema, query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
    synonym_map: Option<&SynonymMap>,
) -> anyhow::Result<Vec<SearchResultItem>> {
    let cfg = config::get();
    let searcher = reader.searcher();
    let tantivy_query = build_query(query, file_ids, kb_ids, schema, synonym_map)?;
    let search_start = Instant::now();
    let top_docs = searcher.search(&tantivy_query, &TopDocs::with_limit(cfg.search.limit))?;
    debug!("Tantivy searcher.search {}ms", search_start.elapsed().as_millis());

    let mut results = vec![];
    let mut doc_total_ms: u128 = 0;
    let mut doc_max_ms: u128 = 0;
    let mut doc_count: usize = 0;
    for (score, doc_address) in top_docs {
        let doc_start = Instant::now();
        let retrieved_doc: TantivyDocument = searcher.doc(doc_address)?;
        let doc_elapsed = doc_start.elapsed().as_millis();
        doc_total_ms += doc_elapsed;
        if doc_elapsed > doc_max_ms {
            doc_max_ms = doc_elapsed;
        }
        doc_count += 1;
        let id = retrieved_doc.get_first(get_field(schema, "id")).and_then(|v| v.as_i64()).unwrap_or(0);
        let file_id = retrieved_doc.get_first(get_field(schema, "file_id")).and_then(|v| v.as_i64()).unwrap_or(0);
        let kb_id = retrieved_doc.get_first(get_field(schema, "kb_id")).and_then(|v| v.as_i64());
        let content = retrieved_doc
            .get_first(get_field(schema, "content"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();
        results.push(SearchResultItem { id, file_id, kb_id, content, score });
    }
    debug!("Tantivy searcher.doc total={}ms max={}ms count={}", doc_total_ms, doc_max_ms, doc_count);

    Ok(results)
}

pub async fn search(
    reader: &IndexReader, schema: &Schema, query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
    synonym_map: Option<&SynonymMap>,
) -> anyhow::Result<Vec<SearchResultItem>> {
    search_sync(reader, schema, query, file_ids, kb_ids, synonym_map)
}

pub fn search_with_snippet_sync(
    reader: &IndexReader, schema: &Schema, query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
    max_chars: usize, synonym_map: Option<&SynonymMap>,
) -> anyhow::Result<Vec<FullSearchResultItem>> {
    let cfg = config::get();
    let searcher = reader.searcher();
    let tantivy_query = build_query(query, file_ids, kb_ids, schema, synonym_map)?;
    let search_start = Instant::now();
    let top_docs = searcher.search(&tantivy_query, &TopDocs::with_limit(cfg.search.limit))?;
    debug!("Tantivy full searcher.search {}ms", search_start.elapsed().as_millis());

    let terms = build_snippet_terms(query, synonym_map);
    let matcher = build_matcher(&terms);

    let mut results = vec![];
    let mut doc_total_ms: u128 = 0;
    let mut doc_max_ms: u128 = 0;
    let mut doc_count: usize = 0;
    for (score, doc_address) in top_docs {
        let doc_start = Instant::now();
        let retrieved_doc: TantivyDocument = searcher.doc(doc_address)?;
        let doc_elapsed = doc_start.elapsed().as_millis();
        doc_total_ms += doc_elapsed;
        if doc_elapsed > doc_max_ms {
            doc_max_ms = doc_elapsed;
        }
        doc_count += 1;
        let file_id = retrieved_doc.get_first(get_field(schema, "file_id")).and_then(|v| v.as_i64()).unwrap_or(0);
        let kb_id = retrieved_doc.get_first(get_field(schema, "kb_id")).and_then(|v| v.as_i64());
        let content = retrieved_doc.get_first(get_field(schema, "content")).and_then(|v| v.as_str()).unwrap_or("");
        let snippet = build_best_snippet(content, matcher.as_ref(), terms.len(), max_chars);
        results.push(FullSearchResultItem { file_id, kb_id, snippet, score });
    }
    debug!("Tantivy full searcher.doc total={}ms max={}ms count={}", doc_total_ms, doc_max_ms, doc_count);

    Ok(results)
}

pub async fn search_with_snippet(
    reader: &IndexReader, schema: &Schema, query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
    max_chars: usize, synonym_map: Option<&SynonymMap>,
) -> anyhow::Result<Vec<FullSearchResultItem>> {
    search_with_snippet_sync(reader, schema, query, file_ids, kb_ids, max_chars, synonym_map)
}

pub async fn delete_by_file(index: &Index, schema: &Schema, file_id: i64) -> anyhow::Result<()> {
    delete_by_files(index, schema, &[file_id]).await
}

pub async fn delete_by_files(index: &Index, schema: &Schema, file_ids: &[i64]) -> anyhow::Result<()> {
    delete_by_i64_terms(index, schema, "file_id", file_ids, "delete_by_files").await
}

pub async fn delete_by_kb(index: &Index, schema: &Schema, kb_id: i64) -> anyhow::Result<()> {
    delete_by_kbs(index, schema, &[kb_id]).await
}

pub async fn delete_by_kbs(index: &Index, schema: &Schema, kb_ids: &[i64]) -> anyhow::Result<()> {
    delete_by_i64_terms(index, schema, "kb_id", kb_ids, "delete_by_kbs").await
}

pub async fn force_merge(index: &Index) -> anyhow::Result<ForceMergeStats> {
    let index = index.clone();
    tokio::task::spawn_blocking(move || force_merge_blocking(&index))
        .await
        .map_err(|err| anyhow::anyhow!("tantivy force merge task failed: {}", err))?
}

async fn delete_by_i64_terms(
    index: &Index, schema: &Schema, field_name: &'static str, ids: &[i64], writer_label: &'static str,
) -> anyhow::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }

    let index = index.clone();
    let schema = schema.clone();
    let ids = ids.to_vec();
    tokio::task::spawn_blocking(move || delete_by_i64_terms_blocking(&index, &schema, field_name, &ids, writer_label))
        .await
        .map_err(|err| anyhow::anyhow!("tantivy delete task failed: {}", err))?
}

fn delete_by_i64_terms_blocking(
    index: &Index, schema: &Schema, field_name: &str, ids: &[i64], writer_label: &str,
) -> anyhow::Result<()> {
    let mut writer = create_writer_with_timing_blocking(index, writer_label)?;
    let field = get_field(schema, field_name);
    let delete_start = Instant::now();
    for id in ids {
        let term = Term::from_field_i64(field, *id);
        writer.delete_term(term);
    }
    debug!(
        "tantivy_delete_terms target={} count={} duration_ms={}",
        field_name,
        ids.len(),
        delete_start.elapsed().as_millis()
    );
    let commit_start = Instant::now();
    writer.commit()?;
    debug!(
        "tantivy_commit target={} count={} duration_ms={}",
        field_name,
        ids.len(),
        commit_start.elapsed().as_millis()
    );
    Ok(())
}

fn force_merge_blocking(index: &Index) -> anyhow::Result<ForceMergeStats> {
    let start = Instant::now();
    let before_metas = index.searchable_segment_metas()?;
    let before_segments = before_metas.len();
    let before_docs = segment_docs(&before_metas);
    let before_deleted_docs = segment_deleted_docs(&before_metas);

    if before_segments < 2 {
        let writer = create_writer_with_timing_blocking(index, "force_merge_gc")?;
        let (gc_deleted_files, gc_failed_files) = garbage_collect_index_files(&writer, "force_merge_skipped")?;
        return Ok(ForceMergeStats {
            before_segments,
            after_segments: before_segments,
            before_docs,
            after_docs: before_docs,
            before_deleted_docs,
            after_deleted_docs: before_deleted_docs,
            gc_deleted_files,
            gc_failed_files,
            skipped: true,
            duration_ms: start.elapsed().as_millis(),
        });
    }

    let segment_ids = before_metas.iter().map(|meta| meta.id()).collect::<Vec<_>>();
    let mut writer = create_writer_with_timing_blocking(index, "force_merge")?;
    writer.merge(&segment_ids).wait()?;
    writer.wait_merging_threads()?;
    let writer = create_writer_with_timing_blocking(index, "force_merge_gc")?;
    let (gc_deleted_files, gc_failed_files) = garbage_collect_index_files(&writer, "force_merge")?;

    let after_metas = index.searchable_segment_metas()?;
    Ok(ForceMergeStats {
        before_segments,
        after_segments: after_metas.len(),
        before_docs,
        after_docs: segment_docs(&after_metas),
        before_deleted_docs,
        after_deleted_docs: segment_deleted_docs(&after_metas),
        gc_deleted_files,
        gc_failed_files,
        skipped: false,
        duration_ms: start.elapsed().as_millis(),
    })
}

fn garbage_collect_index_files(writer: &tantivy::IndexWriter, label: &str) -> anyhow::Result<(usize, usize)> {
    let start = Instant::now();
    let gc_result = writer.garbage_collect_files().wait()?;
    let deleted_files = gc_result.deleted_files.len();
    let failed_files = gc_result.failed_to_delete_files.len();
    if failed_files > 0 {
        warn!(
            "tantivy_gc label={} deleted_files={} failed_files={} duration_ms={}",
            label,
            deleted_files,
            failed_files,
            start.elapsed().as_millis()
        );
    } else {
        debug!(
            "tantivy_gc label={} deleted_files={} failed_files={} duration_ms={}",
            label,
            deleted_files,
            failed_files,
            start.elapsed().as_millis()
        );
    }
    Ok((deleted_files, failed_files))
}

fn segment_docs(metas: &[tantivy::SegmentMeta]) -> u64 {
    metas.iter().map(|meta| u64::from(meta.num_docs())).sum()
}

fn segment_deleted_docs(metas: &[tantivy::SegmentMeta]) -> u64 {
    metas.iter().map(|meta| u64::from(meta.num_deleted_docs())).sum()
}

fn build_query(
    input: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>, schema: &Schema,
    synonym_map: Option<&SynonymMap>,
) -> tantivy::Result<Box<dyn Query>> {
    let mut subqueries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
    let query_terms = build_query_terms(input, synonym_map);

    // content 至少命中一个
    let mut content_queries = Vec::new();
    for query_term in query_terms {
        let tq = TermQuery::new(
            Term::from_field_text(get_field(schema, "content"), query_term.term.as_str()),
            IndexRecordOption::Basic,
        );
        if (query_term.boost - 1.0).abs() > f32::EPSILON {
            content_queries
                .push((Occur::Should, Box::new(BoostQuery::new(Box::new(tq), query_term.boost)) as Box<dyn Query>));
        } else {
            content_queries.push((Occur::Should, Box::new(tq) as Box<dyn Query>));
        }
    }
    let content_bool = BooleanQuery::new(content_queries);
    subqueries.push((Occur::Must, Box::new(content_bool)));

    if let Some(ids) = file_ids {
        if !ids.is_empty() {
            let mut file_id_queries = Vec::new();
            let file_id_field = get_field(schema, "file_id");
            for file_id in ids {
                let file_id_query =
                    TermQuery::new(Term::from_field_i64(file_id_field, *file_id), IndexRecordOption::Basic);
                file_id_queries.push((Occur::Should, Box::new(file_id_query) as Box<dyn Query>));
            }
            if !file_id_queries.is_empty() {
                let file_ids_bool_query = BooleanQuery::new(file_id_queries);
                subqueries.push((Occur::Must, Box::new(file_ids_bool_query)));
            }
        }
    }

    if let Some(ids) = kb_ids {
        if !ids.is_empty() {
            let mut kb_id_queries = Vec::new();
            let kb_id_field = get_field(schema, "kb_id");
            for kb_id in ids {
                let kb_id_query = TermQuery::new(Term::from_field_i64(kb_id_field, *kb_id), IndexRecordOption::Basic);
                kb_id_queries.push((Occur::Should, Box::new(kb_id_query) as Box<dyn Query>));
            }
            if !kb_id_queries.is_empty() {
                let kb_ids_bool_query = BooleanQuery::new(kb_id_queries);
                subqueries.push((Occur::Must, Box::new(kb_ids_bool_query)));
            }
        }
    }
    Ok(Box::new(BooleanQuery::new(subqueries)))
}

fn create_document(doc: Document, schema: &Schema) -> TantivyDocument {
    let mut document = doc! {
        get_field(schema, "id") => doc.id,
        get_field(schema, "file_id") => doc.file_id,
        get_field(schema, "content") => doc.content,
    };
    if let Some(kb_id) = doc.kb_id {
        document.add_i64(get_field(schema, "kb_id"), kb_id);
    }
    document
}

fn get_field(schema: &Schema, field: &str) -> Field {
    schema.get_field(field).unwrap_or_else(|_| panic!("Field '{}' not found in schema", field))
}

fn perform_segmentation(text: &str, mode: chinese_tokenizer::SegmentationMode) -> Vec<String> {
    chinese_tokenizer::FastChineseTokenizer::new(mode).segment(text)
}

fn register_tokenizers(index: &Index) {
    let all_tokenizer = chinese_tokenizer::FastChineseTokenizer::all();
    index.tokenizers().register(ALL_TOKENIZER, all_tokenizer);
}

fn build_schema() -> Schema {
    let mut schema_builder = Schema::builder();
    schema_builder.add_i64_field("id", INDEXED | STORED | FAST); // 切片 id
    schema_builder.add_i64_field("file_id", INDEXED | STORED | FAST); // 文件 id
    schema_builder.add_i64_field("kb_id", INDEXED | STORED | FAST); // 知识库 id

    let text_options = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(ALL_TOKENIZER)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
        .set_stored();
    schema_builder.add_text_field("content", text_options); // 内容
    schema_builder.build()
}

#[derive(Debug, Clone, Copy)]
struct MatchInfo {
    start_char: usize,
    end_char: usize,
    term_idx: usize,
}

fn build_snippet_terms(query: &str, synonym_map: Option<&SynonymMap>) -> Vec<String> {
    let mut terms = Vec::new();
    let mut seen = HashSet::new();
    let segmented = perform_segmentation(query, chinese_tokenizer::SegmentationMode::Search);
    for term in segmented.into_iter().chain(std::iter::once(query.trim().to_string())) {
        let trimmed = term.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_string()) {
            terms.push(trimmed.to_string());
        }
        if let Some(map) = synonym_map {
            if let Some(synonyms) = map.get(trimmed) {
                for synonym in synonyms {
                    let syn = synonym.term.trim();
                    if syn.is_empty() {
                        continue;
                    }
                    if seen.insert(syn.to_string()) {
                        terms.push(syn.to_string());
                    }
                }
            }
        }
    }
    terms.sort_by(|a, b| b.len().cmp(&a.len()));
    terms
}

#[derive(Debug, Clone)]
struct QueryTerm {
    term: String,
    boost: f32,
}

fn build_query_terms(input: &str, synonym_map: Option<&SynonymMap>) -> Vec<QueryTerm> {
    let mut terms = Vec::new();
    let mut seen = HashSet::new();
    let segmented = perform_segmentation(input, chinese_tokenizer::SegmentationMode::Search);
    for term in segmented {
        let trimmed = term.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_string()) {
            terms.push(QueryTerm { term: trimmed.to_string(), boost: 1.0 });
        }
        if let Some(map) = synonym_map {
            if let Some(synonyms) = map.get(trimmed) {
                for synonym in synonyms {
                    let syn = synonym.term.trim();
                    if syn.is_empty() || syn == trimmed {
                        continue;
                    }
                    if seen.insert(syn.to_string()) {
                        terms.push(QueryTerm { term: syn.to_string(), boost: synonym.boost.max(0.01) });
                    }
                }
            }
        }
    }

    if terms.is_empty() {
        let raw = input.trim();
        if !raw.is_empty() {
            terms.push(QueryTerm { term: raw.to_string(), boost: 1.0 });
            if let Some(map) = synonym_map {
                if let Some(synonyms) = map.get(raw) {
                    for synonym in synonyms {
                        let syn = synonym.term.trim();
                        if syn.is_empty() || syn == raw {
                            continue;
                        }
                        if seen.insert(syn.to_string()) {
                            terms.push(QueryTerm { term: syn.to_string(), boost: synonym.boost.max(0.01) });
                        }
                    }
                }
            }
        }
    }

    terms
}

fn build_matcher(terms: &[String]) -> Option<AhoCorasick> {
    if terms.is_empty() {
        return None;
    }
    AhoCorasickBuilder::new().ascii_case_insensitive(true).match_kind(MatchKind::LeftmostLongest).build(terms).ok()
}

fn build_best_snippet(content: &str, matcher: Option<&AhoCorasick>, term_count: usize, max_chars: usize) -> String {
    if content.is_empty() {
        return String::new();
    }
    let char_to_byte = build_char_index(content);
    let char_len = char_to_byte.len().saturating_sub(1);
    let max_chars = max_chars.max(1);

    if char_len <= max_chars {
        return highlight_range(content, matcher, &char_to_byte, 0, char_len, false, false);
    }

    let matches =
        if let Some(matcher) = matcher { collect_matches(content, matcher, &char_to_byte) } else { Vec::new() };

    if matches.is_empty() {
        return highlight_range(content, matcher, &char_to_byte, 0, max_chars, false, true);
    }

    let (best_left, best_right) = select_best_window(&matches, term_count, max_chars);
    let left_start = matches[best_left].start_char;
    let right_end = matches[best_right].end_char;
    let span = right_end.saturating_sub(left_start);
    let extra = max_chars.saturating_sub(span);
    let desired_start = left_start.saturating_sub(extra / 2);
    let lower_bound = right_end.saturating_sub(max_chars);
    let upper_bound = left_start.min(char_len.saturating_sub(max_chars));
    let start = desired_start.clamp(lower_bound, upper_bound);
    let end = start.saturating_add(max_chars).min(char_len);

    highlight_range(content, matcher, &char_to_byte, start, end, start > 0, end < char_len)
}

fn collect_matches(content: &str, matcher: &AhoCorasick, char_to_byte: &[usize]) -> Vec<MatchInfo> {
    let mut matches = Vec::new();
    for m in matcher.find_iter(content) {
        let start_char = byte_to_char_start(char_to_byte, m.start());
        let end_char = byte_to_char_end(char_to_byte, m.end());
        if end_char > start_char {
            matches.push(MatchInfo { start_char, end_char, term_idx: m.pattern().as_usize() });
        }
    }
    matches
}

fn select_best_window(matches: &[MatchInfo], term_count: usize, max_chars: usize) -> (usize, usize) {
    let mut counts = vec![0usize; term_count];
    let mut unique = 0usize;
    let mut right = 0usize;
    let mut best_left = 0usize;
    let mut best_right = 0usize;
    let mut best_unique = 0usize;
    let mut best_total = 0usize;
    let mut best_span = usize::MAX;

    for left in 0..matches.len() {
        if right < left {
            right = left;
        }
        let window_start = matches[left].start_char;
        let window_end = window_start.saturating_add(max_chars);
        while right < matches.len() && matches[right].end_char <= window_end {
            let term_idx = matches[right].term_idx;
            if term_idx < term_count {
                if counts[term_idx] == 0 {
                    unique += 1;
                }
                counts[term_idx] += 1;
            }
            right += 1;
        }
        let total = right.saturating_sub(left);
        if total > 0 {
            let rightmost_end = matches[right - 1].end_char;
            let span = rightmost_end.saturating_sub(matches[left].start_char);
            let better = unique > best_unique
                || (unique == best_unique && total > best_total)
                || (unique == best_unique && total == best_total && span < best_span)
                || (unique == best_unique
                    && total == best_total
                    && span == best_span
                    && matches[left].start_char < matches[best_left].start_char);
            if better {
                best_unique = unique;
                best_total = total;
                best_span = span;
                best_left = left;
                best_right = right - 1;
            }
        }
        let term_idx = matches[left].term_idx;
        if term_idx < term_count {
            counts[term_idx] = counts[term_idx].saturating_sub(1);
            if counts[term_idx] == 0 {
                unique = unique.saturating_sub(1);
            }
        }
    }

    (best_left, best_right)
}

fn build_char_index(text: &str) -> Vec<usize> {
    let mut char_to_byte = Vec::with_capacity(text.chars().count() + 1);
    for (byte_idx, _) in text.char_indices() {
        char_to_byte.push(byte_idx);
    }
    char_to_byte.push(text.len());
    char_to_byte
}

fn byte_to_char_start(char_to_byte: &[usize], byte_idx: usize) -> usize {
    match char_to_byte.binary_search(&byte_idx) {
        Ok(idx) => idx,
        Err(idx) => idx.saturating_sub(1),
    }
}

fn byte_to_char_end(char_to_byte: &[usize], byte_idx: usize) -> usize {
    match char_to_byte.binary_search(&byte_idx) {
        Ok(idx) => idx,
        Err(idx) => idx,
    }
}

fn highlight_range(
    content: &str, matcher: Option<&AhoCorasick>, char_to_byte: &[usize], start_char: usize, end_char: usize,
    prefix: bool, suffix: bool,
) -> String {
    let start = *char_to_byte.get(start_char).unwrap_or(&0);
    let end = *char_to_byte.get(end_char).unwrap_or(&content.len());
    let slice = &content[start..end.min(content.len())];

    let mut output = String::new();
    if prefix {
        output.push_str("...");
    }
    if let Some(matcher) = matcher {
        let mut last = 0usize;
        for m in matcher.find_iter(slice) {
            let start_idx = m.start();
            let end_idx = m.end();
            if start_idx > last {
                output.push_str(&escape_html(&slice[last..start_idx]));
            }
            output.push_str("<b>");
            output.push_str(&escape_html(&slice[start_idx..end_idx]));
            output.push_str("</b>");
            last = end_idx;
        }
        if last < slice.len() {
            output.push_str(&escape_html(&slice[last..]));
        }
    } else {
        output.push_str(&escape_html(slice));
    }
    if suffix {
        output.push_str("...");
    }
    output
}

fn escape_html(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}
