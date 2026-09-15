use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{
        Arc,
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use log::{debug, info, warn};
use serde::Serialize;
use tantivy::{
    Index, IndexReader, Result, TantivyDocument, TantivyError, Term,
    collector::TopDocs,
    directory::error::LockError,
    doc,
    merge_policy::LogMergePolicy,
    query::{BooleanQuery, BoostQuery, Occur, Query, TermQuery},
    schema::{FAST, Field, INDEXED, IndexRecordOption, STORED, Schema, TextFieldIndexing, TextOptions, Value as _},
};

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
    pub is_image: bool,     // 是否为图片切片
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
        Document { id, file_id, kb_id, content, is_image: false }
    }

    pub fn with_is_image(mut self, is_image: bool) -> Self {
        self.is_image = is_image;
        self
    }
}

/// create new index
///
/// delete if it exists
pub fn init() -> Result<(Schema, Index)> {
    let cfg = config::get();
    init_with_path(&cfg.search.tantivy_index_path)
}

pub fn init_with_path(path: &str) -> Result<(Schema, Index)> {
    let t0 = Instant::now();
    let schema = build_schema();
    info!("Tantivy init substep: build_schema() took {}ms", t0.elapsed().as_millis());

    let path = Path::new(path);
    let t1 = Instant::now();
    let index = if path.exists() {
        match Index::open_in_dir(path) {
            Ok(idx) => {
                // 如果 schema 缺少 is_image 字段，按时间戳备份旧索引并重建
                if !schema_has_is_image(idx.schema()) {
                    warn!(
                        "Tantivy index at '{}' is missing is_image field; backing up and creating fresh index.",
                        path.display()
                    );
                    backup_index_dir(path)?;
                    std::fs::create_dir_all(path)?;
                    Index::create_in_dir(path, schema.clone())?
                } else {
                    idx
                }
            }
            Err(e) => {
                warn!(
                    "Tantivy index at '{}' is corrupted or unreadable: {}. Removing and creating fresh index.",
                    path.display(),
                    e
                );
                std::fs::remove_dir_all(path)?;
                std::fs::create_dir_all(path)?;
                Index::create_in_dir(path, schema.clone())?
            }
        }
    } else {
        std::fs::create_dir_all(path)?;
        Index::create_in_dir(path, schema.clone())?
    };
    info!("Tantivy init substep: open/create index at '{}' took {}ms", path.display(), t1.elapsed().as_millis());

    let t2 = Instant::now();
    register_tokenizers(&index);
    info!("Tantivy init substep: register_tokenizers() took {}ms", t2.elapsed().as_millis());

    Ok((schema, index))
}

fn schema_has_is_image(schema: Schema) -> bool {
    schema.get_field("is_image").is_ok()
}

fn backup_index_dir(path: &Path) -> std::io::Result<()> {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let backup_path = path.with_file_name(format!(
        "{}.backup.{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("tantivy_index"),
        timestamp
    ));
    warn!("Backing up old Tantivy index from {} to {}", path.display(), backup_path.display());
    std::fs::rename(path, &backup_path)?;
    Ok(())
}

fn is_lock_busy_error(err: &TantivyError) -> bool {
    matches!(err, TantivyError::LockFailure(LockError::LockBusy, _))
}

/// Retry opening a Tantivy index writer with exponential backoff.
///
/// `$sleep` is a closure that performs the actual sleep for the given number of
/// milliseconds.
macro_rules! retry_open_writer {
    ($index:expr, $sleep:expr) => {{
        let mut delay_ms = INDEX_WRITER_LOCK_RETRY_BASE_MS;
        let mut attempt = 1;
        loop {
            match open_writer($index) {
                Ok(writer) => break Ok(writer),
                Err(err) if is_lock_busy_error(&err) && attempt < INDEX_WRITER_LOCK_RETRY_MAX_ATTEMPTS => {
                    warn!(
                        "Tantivy index writer lock busy (attempt {}/{}), retry in {}ms",
                        attempt, INDEX_WRITER_LOCK_RETRY_MAX_ATTEMPTS, delay_ms
                    );
                    $sleep(delay_ms);
                    delay_ms = (delay_ms * 2).min(800);
                    attempt += 1;
                }
                Err(err) => break Err(err),
            }
        }
    }};
}

async fn create_writer(index: &Index) -> tantivy::Result<tantivy::IndexWriter> {
    retry_open_writer!(index, |ms| std::thread::sleep(Duration::from_millis(ms)))
}

fn create_writer_blocking(index: &Index) -> tantivy::Result<tantivy::IndexWriter> {
    retry_open_writer!(index, |ms| std::thread::sleep(Duration::from_millis(ms)))
}

fn open_writer(index: &Index) -> tantivy::Result<tantivy::IndexWriter> {
    let writer_memory = config::get().search.tantivy_memory_mb * 1_000_000;
    let writer = index.writer(writer_memory)?;
    // 设置 merge policy，减少小 segment 数量
    writer.set_merge_policy(Box::new(LogMergePolicy::default()));
    Ok(writer)
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

/// 每个 Tantivy 索引对应一个长期存活的 writer 线程。
///
/// `IndexWriter` 不是 `Send`/`Sync`，不能放在 `Arc<Mutex<...>>` 里跨越 await，因此在线程内部持有 writer，
/// 外部通过 channel 发送写/删除/merge 任务，并通过 oneshot 等待结果。每次操作后仍执行 commit，保留原
/// 有的数据安全语义。
pub struct IndexWriterHandle {
    tx: Sender<WriterJob>,
    thread: Option<thread::JoinHandle<()>>,
}

struct WriterJob {
    op: WriterOp,
    reply: Option<tokio::sync::oneshot::Sender<WriterResult>>,
}

enum WriterOp {
    WriteBatch(Vec<Document>),
    DeleteTerms { field_name: &'static str, ids: Vec<i64> },
    ForceMerge,
    Shutdown,
}

enum WriterResult {
    Ok,
    WriteBatch,
    ForceMerge(ForceMergeStats),
    Err(String),
}

impl IndexWriterHandle {
    pub async fn open(index: Index, schema: Schema, label: String) -> anyhow::Result<Arc<Self>> {
        let (init_tx, init_rx) = tokio::sync::oneshot::channel::<anyhow::Result<()>>();
        let (tx, rx) = mpsc::channel::<WriterJob>();
        let thread_name = format!("tantivy-writer-{}", label);
        let thread = thread::Builder::new().name(thread_name).spawn(move || {
            let mut writer = match create_writer_with_timing_blocking(&index, &label) {
                Ok(w) => {
                    let _ = init_tx.send(Ok(()));
                    Some(w)
                }
                Err(e) => {
                    let _ = init_tx.send(Err(anyhow::anyhow!(e)));
                    return;
                }
            };

            while let Ok(job) = rx.recv() {
                let current_writer = match writer.take() {
                    Some(w) => w,
                    None => {
                        let result = WriterResult::Err("tantivy writer is not available".to_string());
                        if let Some(reply) = job.reply {
                            let _ = reply.send(result);
                        }
                        continue;
                    }
                };

                let (result, maybe_writer) = match job.op {
                    WriterOp::Shutdown => {
                        drop(current_writer);
                        break;
                    }
                    WriterOp::WriteBatch(docs) => {
                        if docs.is_empty() {
                            (WriterResult::WriteBatch, Some(current_writer))
                        } else {
                            let mut writer = current_writer;
                            let result = match add_documents(&mut writer, &schema, docs) {
                                Ok(count) => match commit_writer(&mut writer, &label, count) {
                                    Ok(()) => WriterResult::WriteBatch,
                                    Err(e) => WriterResult::Err(e.to_string()),
                                },
                                Err(e) => WriterResult::Err(e.to_string()),
                            };
                            (result, Some(writer))
                        }
                    }
                    WriterOp::DeleteTerms { field_name, ids } => {
                        if ids.is_empty() {
                            (WriterResult::Ok, Some(current_writer))
                        } else {
                            let mut writer = current_writer;
                            let field = get_field(&schema, field_name);
                            let delete_start = Instant::now();
                            for id in &ids {
                                let term = Term::from_field_i64(field, *id);
                                writer.delete_term(term);
                            }
                            debug!(
                                "tantivy_delete_terms target={} count={} duration_ms={}",
                                field_name,
                                ids.len(),
                                delete_start.elapsed().as_millis()
                            );
                            let result = match commit_writer(&mut writer, &label, ids.len()) {
                                Ok(()) => WriterResult::Ok,
                                Err(e) => WriterResult::Err(e.to_string()),
                            };
                            (result, Some(writer))
                        }
                    }
                    WriterOp::ForceMerge => match force_merge_with_writer(&index, current_writer, &label) {
                        Ok((stats, new_writer)) => (WriterResult::ForceMerge(stats), Some(new_writer)),
                        Err(e) => (WriterResult::Err(e.to_string()), None),
                    },
                };

                writer = maybe_writer;
                if let Some(reply) = job.reply {
                    let _ = reply.send(result);
                }
                // writer 为 None 时说明 force_merge 失败且未能重建，退出循环
                if writer.is_none() {
                    warn!("tantivy writer thread {} exiting after force_merge failure", label);
                    break;
                }
            }
            // writer 在这里 drop，释放目录锁
        })?;

        init_rx.await??;
        Ok(Arc::new(Self { tx, thread: Some(thread) }))
    }

    pub async fn write_batch(&self, docs: Vec<Document>) -> anyhow::Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(WriterJob { op: WriterOp::WriteBatch(docs), reply: Some(reply_tx) })
            .map_err(|e| anyhow::anyhow!("tantivy writer channel closed: {e}"))?;
        match reply_rx.await? {
            WriterResult::Ok | WriterResult::WriteBatch => Ok(()),
            WriterResult::ForceMerge(_) => Err(anyhow::anyhow!("unexpected force_merge result from write_batch")),
            WriterResult::Err(e) => Err(anyhow::anyhow!("tantivy write_batch failed: {e}")),
        }
    }

    pub async fn delete_by_field(&self, field_name: &'static str, ids: &[i64]) -> anyhow::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let ids = ids.to_vec();
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(WriterJob { op: WriterOp::DeleteTerms { field_name, ids }, reply: Some(reply_tx) })
            .map_err(|e| anyhow::anyhow!("tantivy writer channel closed: {e}"))?;
        match reply_rx.await? {
            WriterResult::Ok => Ok(()),
            WriterResult::WriteBatch => Err(anyhow::anyhow!("unexpected write result from delete")),
            WriterResult::ForceMerge(_) => Err(anyhow::anyhow!("unexpected force_merge result from delete")),
            WriterResult::Err(e) => Err(anyhow::anyhow!("tantivy delete failed: {e}")),
        }
    }

    pub async fn force_merge(&self) -> anyhow::Result<ForceMergeStats> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(WriterJob { op: WriterOp::ForceMerge, reply: Some(reply_tx) })
            .map_err(|e| anyhow::anyhow!("tantivy writer channel closed: {e}"))?;
        match reply_rx.await? {
            WriterResult::ForceMerge(stats) => Ok(stats),
            WriterResult::Ok => Err(anyhow::anyhow!("unexpected ok result from force_merge")),
            WriterResult::WriteBatch => Err(anyhow::anyhow!("unexpected write result from force_merge")),
            WriterResult::Err(e) => Err(anyhow::anyhow!("tantivy force_merge failed: {e}")),
        }
    }
}

impl Drop for IndexWriterHandle {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.tx.send(WriterJob { op: WriterOp::Shutdown, reply: None });
            let _ = thread.join();
        }
    }
}

fn force_merge_with_writer(
    index: &Index, mut writer: tantivy::IndexWriter, label: &str,
) -> anyhow::Result<(ForceMergeStats, tantivy::IndexWriter)> {
    let start = Instant::now();
    let before_metas = index.searchable_segment_metas()?;
    let before_segments = before_metas.len();
    let before_docs = segment_docs(&before_metas);
    let before_deleted_docs = segment_deleted_docs(&before_metas);

    if before_segments < 2 {
        let (gc_deleted_files, gc_failed_files) = garbage_collect_index_files(&writer, "force_merge_skipped")?;
        writer.wait_merging_threads()?;
        let new_writer = create_writer_with_timing_blocking(index, &format!("{}_gc", label))?;
        return Ok((
            ForceMergeStats {
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
            },
            new_writer,
        ));
    }

    let segment_ids = before_metas.iter().map(|meta| meta.id()).collect::<Vec<_>>();
    writer.merge(&segment_ids).wait()?;
    writer.wait_merging_threads()?;
    let new_writer = create_writer_with_timing_blocking(index, &format!("{}_gc", label))?;
    let (gc_deleted_files, gc_failed_files) = garbage_collect_index_files(&new_writer, "force_merge")?;

    let after_metas = index.searchable_segment_metas()?;
    Ok((
        ForceMergeStats {
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
        },
        new_writer,
    ))
}

pub async fn create_rebuild_writer(index: &Index, label: &str) -> tantivy::Result<tantivy::IndexWriter> {
    create_writer_with_timing(index, label).await
}

pub fn add_documents(
    index_writer: &mut tantivy::IndexWriter, schema: &Schema, docs: impl IntoIterator<Item = Document>,
) -> tantivy::Result<usize> {
    let mut count = 0_usize;
    for doc in docs {
        index_writer.add_document(create_document(doc, schema))?;
        count += 1;
    }
    Ok(count)
}

pub fn commit_writer(index_writer: &mut tantivy::IndexWriter, label: &str, doc_count: usize) -> tantivy::Result<()> {
    let commit_start = Instant::now();
    index_writer.commit()?;
    debug!("tantivy_commit label={} docs={} duration_ms={}", label, doc_count, commit_start.elapsed().as_millis());
    Ok(())
}

pub fn search_sync(
    reader: &IndexReader, schema: &Schema, query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
    filter_is_image: Option<bool>, synonym_map: Option<&SynonymMap>,
) -> anyhow::Result<Vec<SearchResultItem>> {
    search_sync_with_limit(
        reader,
        schema,
        query,
        file_ids,
        kb_ids,
        filter_is_image,
        synonym_map,
        config::get().search.limit,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn search_sync_with_limit(
    reader: &IndexReader, schema: &Schema, query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
    filter_is_image: Option<bool>, synonym_map: Option<&SynonymMap>, limit: usize,
) -> anyhow::Result<Vec<SearchResultItem>> {
    let searcher = reader.searcher();
    let tantivy_query = build_query(query, file_ids, kb_ids, filter_is_image, schema, synonym_map)?;
    let search_start = Instant::now();
    let top_docs = searcher.search(&tantivy_query, &TopDocs::with_limit(limit.max(1)))?;
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
    filter_is_image: Option<bool>, synonym_map: Option<&SynonymMap>,
) -> anyhow::Result<Vec<SearchResultItem>> {
    search_sync(reader, schema, query, file_ids, kb_ids, filter_is_image, synonym_map)
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
    input: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>, filter_is_image: Option<bool>,
    schema: &Schema, synonym_map: Option<&SynonymMap>,
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

    if let Some(ids) = file_ids
        && !ids.is_empty()
    {
        let mut file_id_queries = Vec::new();
        let file_id_field = get_field(schema, "file_id");
        for file_id in ids {
            let file_id_query = TermQuery::new(Term::from_field_i64(file_id_field, *file_id), IndexRecordOption::Basic);
            file_id_queries.push((Occur::Should, Box::new(file_id_query) as Box<dyn Query>));
        }
        if !file_id_queries.is_empty() {
            let file_ids_bool_query = BooleanQuery::new(file_id_queries);
            subqueries.push((Occur::Must, Box::new(file_ids_bool_query)));
        }
    }

    if let Some(ids) = kb_ids
        && !ids.is_empty()
    {
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

    if let Some(is_image) = filter_is_image {
        let is_image_field = get_field(schema, "is_image");
        let term = Term::from_field_i64(is_image_field, i64::from(is_image));
        subqueries.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic)) as Box<dyn Query>));
    }

    Ok(Box::new(BooleanQuery::new(subqueries)))
}

fn create_document(doc: Document, schema: &Schema) -> TantivyDocument {
    let mut document = doc! {
        get_field(schema, "id") => doc.id,
        get_field(schema, "file_id") => doc.file_id,
        get_field(schema, "content") => doc.content,
        get_field(schema, "is_image") => i64::from(doc.is_image),
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
    schema_builder.add_i64_field("is_image", INDEXED | FAST); // 是否为图片切片

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
        if let Some(map) = synonym_map
            && let Some(synonyms) = map.get(trimmed)
        {
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

    if terms.is_empty() {
        let raw = input.trim();
        if !raw.is_empty() {
            terms.push(QueryTerm { term: raw.to_string(), boost: 1.0 });
            if let Some(map) = synonym_map
                && let Some(synonyms) = map.get(raw)
            {
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

    terms
}
