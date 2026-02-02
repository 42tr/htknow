use std::{collections::HashSet, path::Path, time::Instant};

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use log::{debug, info};
use serde::Serialize;
use tantivy::{
    Index, IndexReader, Result, TantivyDocument, Term, collector::TopDocs, doc, merge_policy::LogMergePolicy, query::{BooleanQuery, BoostQuery, Occur, PhraseQuery, Query, TermQuery}, schema::{FAST, Field, INDEXED, IndexRecordOption, STORED, Schema, TextFieldIndexing, TextOptions, Value as _}, tokenizer::{TokenStream, Tokenizer}
};

use super::chinese_tokenizer;
use crate::config;

const ALL_TOKENIZER: &str = "all";
const SENTENCE_SHOULD_BOOST: f32 = 2.0;
const SENTENCE_SLOP: u32 = 12;

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

fn init_with_path(path: &str) -> Result<(Schema, Index)> {
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

fn create_writer(index: &Index) -> tantivy::Result<tantivy::IndexWriter> {
    let writer_memory = config::get().search.tantivy_memory_mb * 1_000_000;
    let writer = index.writer(writer_memory)?;

    // 设置 merge policy，减少小 segment 数量
    writer.set_merge_policy(Box::new(LogMergePolicy::default()));

    Ok(writer)
}

pub async fn write_documents(index: &Index, schema: &Schema, doc: Document) -> tantivy::Result<()> {
    let mut index_writer = create_writer(index)?;
    index_writer.add_document(create_document(doc, schema))?;
    index_writer.commit()?;
    Ok(())
}

/// 批量写入文档，减少 commit 次数，避免产生大量小 segment
pub async fn write_documents_batch(index: &Index, schema: &Schema, docs: Vec<Document>) -> tantivy::Result<()> {
    if docs.is_empty() {
        return Ok(());
    }
    let mut index_writer = create_writer(index)?;
    for doc in docs {
        index_writer.add_document(create_document(doc, schema))?;
    }
    index_writer.commit()?; // 一次 commit
    Ok(())
}

pub async fn search(
    reader: &IndexReader, schema: &Schema, query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
) -> anyhow::Result<Vec<SearchResultItem>> {
    let cfg = config::get();
    let searcher = reader.searcher();
    let tantivy_query = build_query(query, file_ids, kb_ids, schema)?;
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

pub async fn search_with_snippet(
    reader: &IndexReader, schema: &Schema, query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
    max_chars: usize,
) -> anyhow::Result<Vec<FullSearchResultItem>> {
    let cfg = config::get();
    let searcher = reader.searcher();
    let tantivy_query = build_query(query, file_ids, kb_ids, schema)?;
    let search_start = Instant::now();
    let top_docs = searcher.search(&tantivy_query, &TopDocs::with_limit(cfg.search.limit))?;
    debug!("Tantivy full searcher.search {}ms", search_start.elapsed().as_millis());

    let terms = build_snippet_terms(query);
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

pub async fn delete_by_file(index: &Index, schema: &Schema, file_id: i64) -> anyhow::Result<()> {
    let mut writer = create_writer(index)?;
    let file_id_field = get_field(schema, "file_id");
    let term = Term::from_field_i64(file_id_field, file_id);
    writer.delete_term(term);
    writer.commit()?;
    Ok(())
}

pub async fn delete_by_kb(index: &Index, schema: &Schema, kb_id: i64) -> anyhow::Result<()> {
    let mut writer = create_writer(index)?;
    let kb_id_field = get_field(schema, "kb_id");
    let term = Term::from_field_i64(kb_id_field, kb_id);
    writer.delete_term(term);
    writer.commit()?;
    Ok(())
}

fn build_query(
    input: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>, schema: &Schema,
) -> tantivy::Result<Box<dyn Query>> {
    let mut subqueries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
    let segmented_words = perform_segmentation(input, chinese_tokenizer::SegmentationMode::Search);

    // content 至少命中一个
    let mut content_queries = Vec::new();
    for word in segmented_words {
        let tq = TermQuery::new(Term::from_field_text(get_field(schema, "content"), &word), IndexRecordOption::Basic);
        content_queries.push((Occur::Should, Box::new(tq) as Box<dyn Query>));
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
    if let Some(sentence_query) = build_sentence_query(input, schema) {
        subqueries.push((Occur::Should, sentence_query));
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
    info!("Document: {:?}", document);
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

fn build_snippet_terms(query: &str) -> Vec<String> {
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
    }
    terms.sort_by(|a, b| b.len().cmp(&a.len()));
    terms
}

fn build_sentence_query(input: &str, schema: &Schema) -> Option<Box<dyn Query>> {
    let mut tokenizer = chinese_tokenizer::FastChineseTokenizer::new(chinese_tokenizer::SegmentationMode::Search);
    let mut stream = tokenizer.token_stream(input);
    let mut phrase_terms = Vec::new();
    let mut last_term: Option<String> = None;
    while stream.advance() {
        let term_text = stream.token().text.trim();
        if term_text.is_empty() {
            continue;
        }
        if last_term.as_deref() == Some(term_text) {
            continue;
        }
        phrase_terms.push(term_text.to_string());
        last_term = Some(term_text.to_string());
    }
    if phrase_terms.len() < 2 {
        return None;
    }

    let field = get_field(schema, "content");
    let terms: Vec<Term> = phrase_terms.into_iter().map(|t| Term::from_field_text(field, t.as_str())).collect();
    let mut phrase = PhraseQuery::new(terms);
    phrase.set_slop(SENTENCE_SLOP);
    Some(Box::new(BoostQuery::new(Box::new(phrase), SENTENCE_SHOULD_BOOST)))
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
