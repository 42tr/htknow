use std::{
    collections::{HashMap, HashSet}, fs, path::Path, sync::Arc, time::{Duration, Instant, SystemTime, UNIX_EPOCH}
};

use anyhow::{Context, anyhow};
use log::{debug, info, warn};
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use tantivy::{Index, IndexReader, ReloadPolicy, schema::Schema};
use tokio::sync::Mutex;

use crate::config;

pub mod advanced;
mod chinese_tokenizer;
pub mod embedding;
mod lancedb;
pub mod tantivy_engine;

pub use tantivy_engine::{FullSearchResultItem, SearchResultItem};

const FULL_SNIPPET_MAX_CHARS: usize = 400;
const MAX_QUERY_TERMS_FOR_SYNONYM_LOOKUP: usize = 100;
const DEFAULT_REBUILD_BATCH_SIZE: i64 = 100;
static RERANK_HTTP_CLIENT: Lazy<Client> = Lazy::new(Client::new);

/// /v1/rerank 格式的请求体
#[derive(Debug, Serialize)]
struct RerankRequest {
    model: String,
    query: String,
    documents: Vec<String>,
}

/// /v1/rerank 格式的响应体
#[derive(Debug, Deserialize)]
struct RerankResponse {
    results: Vec<RerankResult>,
}

#[derive(Debug, Deserialize)]
struct RerankResult {
    index: usize,
    relevance_score: f32,
}

/// /rerank 格式的请求体
#[derive(Debug, Serialize)]
struct SimpleRerankRequest {
    query: String,
    texts: Vec<String>,
}

/// /rerank 格式的响应体（数组）
#[derive(Debug, Deserialize)]
struct SimpleRerankResult {
    index: usize,
    score: f32,
}

#[derive(Debug, sqlx::FromRow)]
struct SynonymRow {
    term: String,
    synonym: String,
    weight: f32,
    bidirectional: i64,
}

/// 同义词表 TTL 缓存：避免每次查询都扫库。变更会在 TTL 窗口内生效。
const SYNONYM_CACHE_TTL: Duration = Duration::from_secs(60);

struct SynonymCache {
    loaded_at: Instant,
    rows: Vec<SynonymRow>,
    /// row.term -> rows 下标
    by_term: HashMap<String, Vec<usize>>,
    /// row.synonym -> rows 下标
    by_synonym: HashMap<String, Vec<usize>>,
}

impl SynonymCache {
    fn build(rows: Vec<SynonymRow>) -> Self {
        let mut by_term: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_synonym: HashMap<String, Vec<usize>> = HashMap::new();
        for (idx, row) in rows.iter().enumerate() {
            by_term.entry(row.term.clone()).or_default().push(idx);
            by_synonym.entry(row.synonym.clone()).or_default().push(idx);
        }
        Self { loaded_at: Instant::now(), rows, by_term, by_synonym }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct LexiconRow {
    term: String,
    freq: Option<i64>,
    tag: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct RebuildSliceRow {
    id: i64,
    file_id: i64,
    kb_id: Option<i64>,
    content: String,
}

#[derive(Debug, sqlx::FromRow)]
struct RebuildLanceDbSliceRow {
    id: i64,
    file_id: i64,
    kb_id: Option<i64>,
    content: String,
    filename: String,
    path: String,
}

#[derive(Debug, sqlx::FromRow)]
struct RebuildFullMetaRow {
    id: i64,
    kb_id: Option<i64>,
    filename: String,
}

#[derive(Debug, Clone)]
pub struct RebuildProgress {
    pub phase: String,
    pub total_docs: i64,
    pub processed_docs: i64,
}

/// 根据文件名后缀判断是否为图片文件。
pub fn is_image_file(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".png")
        || lower.ends_with(".gif")
        || lower.ends_with(".bmp")
        || lower.ends_with(".webp")
        || lower.ends_with(".tiff")
        || lower.ends_with(".tif")
        || lower.ends_with(".svg")
        || lower.ends_with(".ico")
        || lower.ends_with(".heic")
        || lower.ends_with(".heif")
}

async fn fetch_rebuild_lancedb_rows(
    pool: &SqlitePool, slice_ids: &[i64],
) -> anyhow::Result<Vec<RebuildLanceDbSliceRow>> {
    if slice_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut query_builder: QueryBuilder<'_, Sqlite> = QueryBuilder::new(
        "SELECT s.id, s.file_id, f.kb_id, s.content, f.filename, f.path FROM slices s JOIN files f ON f.id = s.file_id WHERE s.id IN (",
    );
    let mut separated = query_builder.separated(", ");
    for slice_id in slice_ids {
        separated.push_bind(slice_id);
    }
    separated.push_unseparated(") ORDER BY s.id ASC");
    Ok(query_builder.build_query_as().fetch_all(pool).await?)
}

#[derive(Clone)]
pub struct SearchEngine {
    schema: Schema,
    index_reader: IndexReader,
    index_write_lock: Arc<Mutex<()>>,
    index_writer: Arc<tantivy_engine::IndexWriterHandle>,
    full_schema: Schema,
    full_index_reader: IndexReader,
    full_index_write_lock: Arc<Mutex<()>>,
    full_index_writer: Arc<tantivy_engine::IndexWriterHandle>,
    rebuild_lock: Arc<Mutex<()>>,
    pool: Option<SqlitePool>,
    synonym_cache: Arc<tokio::sync::RwLock<Option<SynonymCache>>>,
    /// LanceDB 在本次启动时是否被新建或从损坏中恢复，true 表示需要从 SQLite 回填向量。
    lancedb_recreated: bool,
}

impl SearchEngine {
    pub async fn init() -> Self {
        let lancedb_recreated = lancedb::init().await.expect("init lancedb failed");
        let (schema, index) = tantivy_engine::init().unwrap();
        let (full_schema, full_index) = tantivy_engine::init_full().unwrap();
        let index_reader = build_reader(&index, "index");
        let full_index_reader = build_reader(&full_index, "full_index");
        let index_writer = tantivy_engine::IndexWriterHandle::open(index, schema.clone(), "index".to_string())
            .await
            .expect("open tantivy index writer failed");
        let full_index_writer =
            tantivy_engine::IndexWriterHandle::open(full_index, full_schema.clone(), "full_index".to_string())
                .await
                .expect("open tantivy full index writer failed");
        Self {
            schema,
            index_reader,
            index_write_lock: Arc::new(Mutex::new(())),
            index_writer,
            full_schema,
            full_index_reader,
            full_index_write_lock: Arc::new(Mutex::new(())),
            full_index_writer,
            rebuild_lock: Arc::new(Mutex::new(())),
            pool: None,
            synonym_cache: Arc::new(tokio::sync::RwLock::new(None)),
            lancedb_recreated,
        }
    }

    pub fn with_pool(mut self, pool: SqlitePool) -> Self {
        self.pool = Some(pool);
        self
    }

    /// 检查 SQLite 的切片数量与 LanceDB 有效文档数是否一致，不一致则增量补齐缺失的切片。
    /// 图片文件会重新调用图片 embedding 服务恢复 image_vector。
    pub async fn maybe_rebuild_lancedb_from_db(&self) -> anyhow::Result<()> {
        let Some(pool) = &self.pool else {
            return Err(anyhow!("search engine db pool not set"));
        };

        let total_slices: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM slices").fetch_one(pool).await?;
        info!("Checking LanceDB document ids against {} SQLite slices...", total_slices);
        let mut unmatched_lancedb_ids = if self.lancedb_recreated {
            HashSet::new()
        } else {
            lancedb::load_existing_ids(total_slices as u64).await?
        };
        let lancedb_count = unmatched_lancedb_ids.len();

        if total_slices == 0 {
            if lancedb_count > 0 {
                warn!("SQLite has no slices but LanceDB has {} documents; clearing LanceDB", lancedb_count);
                lancedb::clear_all_documents().await?;
            }
            return Ok(());
        }

        let sqlite_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM slices ORDER BY id ASC").fetch_all(pool).await?;
        let mut missing_ids = Vec::with_capacity(total_slices.saturating_sub(lancedb_count as i64).max(0) as usize);
        for id in sqlite_ids {
            if !unmatched_lancedb_ids.remove(&id) {
                missing_ids.push(id);
            }
        }
        let mut extra_ids: Vec<i64> = unmatched_lancedb_ids.into_iter().collect();
        extra_ids.sort_unstable();
        info!(
            "LanceDB comparison completed: sqlite={} lancedb={} missing={} extra={}",
            total_slices,
            lancedb_count,
            missing_ids.len(),
            extra_ids.len()
        );

        if missing_ids.is_empty() && extra_ids.is_empty() {
            return Ok(());
        }

        if self.lancedb_recreated {
            info!("LanceDB was recreated; rebuilding vectors from SQLite slices...");
        } else {
            warn!(
                "LanceDB differs from SQLite; restoring {} missing slices and removing {} extra slices...",
                missing_ids.len(),
                extra_ids.len()
            );
        }

        let cfg = config::get();
        let batch_size = cfg.search.lancedb_rebuild_batch_size.max(1);
        let rebuild_started_at = Instant::now();

        let mut removed = 0usize;
        for id_batch in extra_ids.chunks(batch_size) {
            lancedb::delete_by_slices_for_rebuild(id_batch).await?;
            removed += id_batch.len();
            info!(
                "LanceDB cleanup progress: removed={}/{} total={}s",
                removed,
                extra_ids.len(),
                rebuild_started_at.elapsed().as_secs()
            );
        }

        // 只为缺失切片所属的图片文件重建 image_embedding。
        let mut image_embeddings: HashMap<i64, Arc<Vec<f32>>> = HashMap::new();
        let mut attempted_image_embeddings = HashSet::new();
        let mut processed = 0usize;

        for id_batch in missing_ids.chunks(batch_size) {
            let rows = fetch_rebuild_lancedb_rows(pool, id_batch).await?;
            if rows.len() != id_batch.len() {
                return Err(anyhow!(
                    "Failed to load all missing SQLite slices: requested={}, loaded={}, ids={}-{}",
                    id_batch.len(),
                    rows.len(),
                    id_batch.first().copied().unwrap_or_default(),
                    id_batch.last().copied().unwrap_or_default()
                ));
            }
            let batch_first_id = id_batch.first().copied().unwrap_or_default();
            let batch_last_id = id_batch.last().copied().unwrap_or_default();
            let batch_started_at = Instant::now();

            for row in &rows {
                if attempted_image_embeddings.insert(row.file_id) && is_image_file(&row.filename) {
                    match embedding::get_image_embedding_from_path(&row.path, Some(&row.filename)).await {
                        Ok(image_embedding) => {
                            image_embeddings.insert(row.file_id, Arc::new(image_embedding));
                        }
                        Err(err) => {
                            warn!(
                                "Failed to rebuild image embedding for file {} ({} at {}): {}",
                                row.file_id, row.filename, row.path, err
                            );
                        }
                    }
                }
            }

            let docs: Vec<lancedb::Document> = rows
                .into_iter()
                .map(|row| {
                    let mut doc = lancedb::Document::new(row.id, row.file_id, row.kb_id, row.content);
                    if let Some(image_embedding) = image_embeddings.get(&row.file_id) {
                        doc = doc.with_image_embedding(image_embedding.clone());
                    }
                    doc
                })
                .collect();
            let restored = docs.len();
            lancedb::write_documents_batch_for_rebuild(docs).await?;
            processed += restored;
            info!(
                "LanceDB rebuild progress: restored={}/{} ids={}-{} batch={}ms total={}s",
                processed,
                missing_ids.len(),
                batch_first_id,
                batch_last_id,
                batch_started_at.elapsed().as_millis(),
                rebuild_started_at.elapsed().as_secs()
            );
        }

        // 全部批次写完后只触发一次索引刷新。
        lancedb::schedule_optimize_after_rebuild();
        info!(
            "LanceDB reconciliation completed: restored={} removed={} elapsed={}s",
            processed,
            removed,
            rebuild_started_at.elapsed().as_secs()
        );
        Ok(())
    }

    pub async fn reload_lexicon(&self) -> anyhow::Result<usize> {
        let Some(pool) = &self.pool else {
            return Ok(0);
        };

        let rows: Vec<LexiconRow> =
            sqlx::query_as("SELECT term, freq, tag FROM search_lexicon WHERE enabled = 1 ORDER BY id")
                .fetch_all(pool)
                .await?;

        let entries: Vec<chinese_tokenizer::LexiconEntry> = rows
            .into_iter()
            .map(|row| chinese_tokenizer::LexiconEntry {
                term: row.term,
                freq: row.freq.and_then(|value| usize::try_from(value).ok()).filter(|value| *value > 0),
                tag: row.tag,
            })
            .collect();

        chinese_tokenizer::reload_custom_words(&entries)
    }

    pub async fn rebuild_tantivy_indexes<F, Fut>(&self, job_tag: &str, mut on_progress: F) -> anyhow::Result<()>
    where
        F: FnMut(RebuildProgress) -> Fut+Send,
        Fut: std::future::Future<Output=()>+Send, {
        let Some(pool) = &self.pool else {
            return Err(anyhow!("search engine db pool not set"));
        };
        let _rebuild_guard = self.rebuild_lock.lock().await;

        let total_slices: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM slices").fetch_one(pool).await?;
        let total_full_docs: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE status = 1").fetch_one(pool).await?;
        let total_docs = total_slices + total_full_docs;
        let mut processed_docs = 0_i64;
        on_progress(RebuildProgress { phase: "prepare".to_string(), total_docs, processed_docs }).await;

        let cfg = config::get();
        let rebuild_batch_size = i64::try_from(cfg.search.tantivy_rebuild_batch_size)
            .ok()
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_REBUILD_BATCH_SIZE);
        let tag = sanitize_job_tag(job_tag);
        let slice_live_path = cfg.search.tantivy_index_path.clone();
        let full_live_path = cfg.search.tantivy_full_index_path.clone();
        let slice_temp_path = format!("{}.rebuild.{}", slice_live_path, tag);
        let full_temp_path = format!("{}.rebuild.{}", full_live_path, tag);
        let slice_backup_path = format!("{}.backup.{}", slice_live_path, tag);
        let full_backup_path = format!("{}.backup.{}", full_live_path, tag);

        cleanup_dir_if_exists(&slice_temp_path)?;
        cleanup_dir_if_exists(&full_temp_path)?;
        cleanup_dir_if_exists(&slice_backup_path)?;
        cleanup_dir_if_exists(&full_backup_path)?;

        let rebuild_result: anyhow::Result<()> = async {
            let (slice_schema, slice_temp_index) = tantivy_engine::init_with_path(&slice_temp_path)
                .with_context(|| format!("init temp slice index failed: {}", slice_temp_path))?;
            let (full_schema, full_temp_index) = tantivy_engine::init_with_path(&full_temp_path)
                .with_context(|| format!("init temp full index failed: {}", full_temp_path))?;
            let mut slice_writer = tantivy_engine::create_rebuild_writer(&slice_temp_index, "rebuild_slice")
                .await
                .context("create temp slice writer failed")?;
            let mut full_writer = tantivy_engine::create_rebuild_writer(&full_temp_index, "rebuild_full")
                .await
                .context("create temp full writer failed")?;
            let mut total_slice_docs = 0_usize;
            let mut total_full_docs = 0_usize;

            on_progress(RebuildProgress { phase: "build_slice".to_string(), total_docs, processed_docs }).await;
            let mut last_slice_id = 0_i64;
            loop {
                let rows: Vec<RebuildSliceRow> = sqlx::query_as(
                    "SELECT s.id, s.file_id, f.kb_id, s.content \
                     FROM slices s \
                     JOIN files f ON f.id = s.file_id \
                     WHERE s.id > ? \
                     ORDER BY s.id ASC \
                     LIMIT ?",
                )
                .bind(last_slice_id)
                .bind(rebuild_batch_size)
                .fetch_all(pool)
                .await?;
                if rows.is_empty() {
                    break;
                }
                last_slice_id = rows.last().map(|row| row.id).unwrap_or(last_slice_id);
                let batch_size = rows.len() as i64;
                let docs: Vec<tantivy_engine::Document> = rows
                    .into_iter()
                    .map(|row| tantivy_engine::Document::new(row.id, row.file_id, row.kb_id, row.content))
                    .collect();
                total_slice_docs += tantivy_engine::add_documents(&mut slice_writer, &slice_schema, docs)?;
                processed_docs += batch_size;
                on_progress(RebuildProgress { phase: "build_slice".to_string(), total_docs, processed_docs }).await;
            }
            tantivy_engine::commit_writer(&mut slice_writer, "rebuild_slice", total_slice_docs)
                .context("commit temp slice writer failed")?;

            on_progress(RebuildProgress { phase: "build_full".to_string(), total_docs, processed_docs }).await;
            let mut last_file_id = 0_i64;
            loop {
                let meta_rows: Vec<RebuildFullMetaRow> = sqlx::query_as(
                    "SELECT id, kb_id, filename \
                     FROM files \
                     WHERE status = 1 AND id > ? \
                     ORDER BY id ASC \
                     LIMIT ?",
                )
                .bind(last_file_id)
                .bind(rebuild_batch_size)
                .fetch_all(pool)
                .await?;
                if meta_rows.is_empty() {
                    break;
                }
                last_file_id = meta_rows.last().map(|row| row.id).unwrap_or(last_file_id);
                let batch_size = meta_rows.len() as i64;

                let ids: Vec<i64> = meta_rows.iter().map(|row| row.id).collect();
                let content_by_id = fetch_file_contents_by_ids(pool, &ids).await?;

                let docs: Vec<tantivy_engine::Document> = meta_rows
                    .into_iter()
                    .map(|row| {
                        let full_content = content_by_id.get(&row.id).cloned().unwrap_or_default();
                        let index_content = if full_content.trim().is_empty() {
                            row.filename
                        } else {
                            format!("{}\n\n{}", row.filename, full_content)
                        };
                        tantivy_engine::Document::new(row.id, row.id, row.kb_id, index_content)
                    })
                    .collect();
                total_full_docs += tantivy_engine::add_documents(&mut full_writer, &full_schema, docs)?;
                processed_docs += batch_size;
                on_progress(RebuildProgress { phase: "build_full".to_string(), total_docs, processed_docs }).await;
            }
            tantivy_engine::commit_writer(&mut full_writer, "rebuild_full", total_full_docs)
                .context("commit temp full writer failed")?;

            drop(slice_writer);
            drop(full_writer);

            drop(slice_temp_index);
            drop(full_temp_index);

            let _slice_write_guard = self.index_write_lock.lock().await;
            let _full_write_guard = self.full_index_write_lock.lock().await;
            on_progress(RebuildProgress { phase: "swap".to_string(), total_docs, processed_docs }).await;

            if let Err(err) = swap_index_dir(&slice_live_path, &slice_temp_path, &slice_backup_path) {
                return Err(err.context("swap slice index failed"));
            }

            if let Err(err) = swap_index_dir(&full_live_path, &full_temp_path, &full_backup_path) {
                if let Err(rb_err) = restore_backup_dir(&slice_live_path, &slice_backup_path) {
                    warn!("rollback slice index failed after full swap failure: {}", rb_err);
                }
                return Err(err.context("swap full index failed"));
            }

            if let Err(err) = reload_reader(&self.index_reader, "index") {
                let _ = restore_backup_dir(&full_live_path, &full_backup_path);
                let _ = restore_backup_dir(&slice_live_path, &slice_backup_path);
                return Err(err).context("reload slice reader after swap failed");
            }
            if let Err(err) = reload_reader(&self.full_index_reader, "full_index") {
                let _ = restore_backup_dir(&full_live_path, &full_backup_path);
                let _ = restore_backup_dir(&slice_live_path, &slice_backup_path);
                return Err(err).context("reload full reader after swap failed");
            }

            cleanup_dir_if_exists(&slice_backup_path)?;
            cleanup_dir_if_exists(&full_backup_path)?;
            processed_docs = total_docs;
            on_progress(RebuildProgress { phase: "completed".to_string(), total_docs, processed_docs }).await;
            Ok(())
        }
        .await;

        if rebuild_result.is_err() {
            // 清理临时重建目录（重建过程中的中间产物）
            let _ = cleanup_dir_if_exists(&slice_temp_path);
            let _ = cleanup_dir_if_exists(&full_temp_path);
            // 保留 backup 目录不删除——如果 swap 后 reload 失败且 restore 也失败，
            // backup 是恢复到上一次可用索引的唯一手段。
            // 这些 backup 会在下次成功重建后被覆盖，或通过手动清理。
        }

        rebuild_result
    }

    pub async fn write(
        &self, doc: tantivy_engine::Document, image_embedding: Option<Arc<Vec<f32>>>,
    ) -> anyhow::Result<()> {
        {
            let _guard = self.index_write_lock.lock().await;
            self.index_writer.write_batch(vec![doc.clone()]).await?;
            reload_reader(&self.index_reader, "index")?;
        }

        let mut lancedb_doc = lancedb::Document::new(doc.id, doc.file_id, doc.kb_id, doc.content);
        if let Some(image_embedding) = image_embedding {
            lancedb_doc = lancedb_doc.with_image_embedding(image_embedding);
        }
        lancedb::write_documents(lancedb_doc).await?;
        Ok(())
    }

    /// 批量写入切片到默认索引与 LanceDB。
    ///
    /// 注意：写入后不会自动 reload reader，调用方需在完成全部写入后调用 [`reload_readers`]，
    /// 避免批量处理时每次写入都重建 reader。
    pub async fn write_batch(
        &self, docs: Vec<tantivy_engine::Document>, image_embeddings: Vec<Option<Arc<Vec<f32>>>>,
    ) -> anyhow::Result<()> {
        if docs.is_empty() {
            return Ok(());
        }

        {
            let _guard = self.index_write_lock.lock().await;
            self.index_writer.write_batch(docs.clone()).await?;
        }

        let lancedb_docs: Vec<lancedb::Document> = docs
            .iter()
            .zip(image_embeddings.iter())
            .map(|(doc, image_embedding)| {
                let mut lancedb_doc = lancedb::Document::new(doc.id, doc.file_id, doc.kb_id, doc.content.clone());
                if let Some(embedding) = image_embedding {
                    lancedb_doc = lancedb_doc.with_image_embedding(embedding.clone());
                }
                lancedb_doc
            })
            .collect();

        lancedb::write_documents_batch(lancedb_docs).await?;
        Ok(())
    }

    /// 写入全文索引。注意：写入后不会自动 reload reader，调用方需在完成全部写入后调用 [`reload_readers`]。
    pub async fn write_full(&self, doc: tantivy_engine::Document) -> anyhow::Result<()> {
        {
            let _guard = self.full_index_write_lock.lock().await;
            self.full_index_writer.write_batch(vec![doc]).await?;
        }
        Ok(())
    }

    /// 更新指定切片在默认索引与 LanceDB 中的内容。
    ///
    /// 内部会先删除旧 slice 文档/向量，再写入新内容，并 reload 默认索引 reader。
    /// LanceDB 采用软删除，旧向量记录会被标记为 `is_deleted=true`，查询时不可见。
    pub async fn update_slices(
        &self, file_id: i64, kb_id: Option<i64>, updates: Vec<(i64, String)>,
    ) -> anyhow::Result<()> {
        if updates.is_empty() {
            return Ok(());
        }

        let slice_ids: Vec<i64> = updates.iter().map(|(id, _)| *id).collect();
        let docs: Vec<tantivy_engine::Document> = updates
            .into_iter()
            .map(|(id, content)| tantivy_engine::Document::new(id, file_id, kb_id, content))
            .collect();

        // 1. 默认索引：删除旧 slice 文档并写入新文档，随后 reload reader
        {
            let _guard = self.index_write_lock.lock().await;
            self.index_writer.delete_by_field("id", &slice_ids).await?;
            self.index_writer.write_batch(docs.clone()).await?;
            reload_reader(&self.index_reader, "index")?;
        }

        // 2. LanceDB：软删除旧向量并写入新向量（LanceDB 会自动为 content 生成 embedding）
        lancedb::delete_by_slices(&slice_ids).await?;
        let lancedb_docs: Vec<lancedb::Document> =
            docs.into_iter().map(|doc| lancedb::Document::new(doc.id, doc.file_id, doc.kb_id, doc.content)).collect();
        lancedb::write_documents_batch(lancedb_docs).await?;

        Ok(())
    }

    /// 更新指定文件在全文索引中的内容。
    ///
    /// 会先删除该 file_id 对应的旧全文文档，再写入 `filename\n\nfull_content`。
    pub async fn update_full_index_for_file(
        &self, file_id: i64, kb_id: Option<i64>, filename: String, full_content: String,
    ) -> anyhow::Result<()> {
        let index_content =
            if full_content.trim().is_empty() { filename } else { format!("{}\n\n{}", filename, full_content) };

        {
            let _guard = self.full_index_write_lock.lock().await;
            self.full_index_writer.delete_by_field("file_id", &[file_id]).await?;
            self.full_index_writer
                .write_batch(vec![tantivy_engine::Document::new(file_id, file_id, kb_id, index_content)])
                .await?;
            reload_reader(&self.full_index_reader, "full_index")?;
        }
        Ok(())
    }

    pub fn reload_readers(&self) -> anyhow::Result<()> {
        reload_reader(&self.index_reader, "index")?;
        reload_reader(&self.full_index_reader, "full_index")?;
        Ok(())
    }

    pub async fn delete(&self, file_id: Option<i64>, kb_id: Option<i64>) -> anyhow::Result<()> {
        let file_buf = file_id.map(|id| [id]);
        let kb_buf = kb_id.map(|id| [id]);
        self.delete_batch(file_buf.as_ref().map(|ids| &ids[..]), kb_buf.as_ref().map(|ids| &ids[..])).await
    }

    pub async fn delete_batch(&self, file_ids: Option<&[i64]>, kb_ids: Option<&[i64]>) -> anyhow::Result<()> {
        let overall_start = Instant::now();
        if let Some(file_ids) = file_ids.filter(|ids| !ids.is_empty()) {
            let tantivy_delete = async {
                let lock_wait_start = Instant::now();
                {
                    let _guard = self.index_write_lock.lock().await;
                    let locked_at = Instant::now();
                    debug!(
                        "search_delete file_count={} tantivy_lock_wait_ms={}",
                        file_ids.len(),
                        lock_wait_start.elapsed().as_millis()
                    );
                    self.index_writer.delete_by_field("file_id", file_ids).await?;
                    debug!(
                        "search_delete file_count={} tantivy_inner_ms={}",
                        file_ids.len(),
                        locked_at.elapsed().as_millis()
                    );
                }
                reload_reader(&self.index_reader, "index")?;
                debug!(
                    "search_delete file_count={} tantivy {}ms",
                    file_ids.len(),
                    lock_wait_start.elapsed().as_millis()
                );
                anyhow::Ok(())
            };

            let lancedb_delete = async {
                let step_start = Instant::now();
                if file_ids.len() == 1 {
                    lancedb::delete_by_file(file_ids[0]).await?;
                } else {
                    lancedb::delete_by_files(file_ids).await?;
                }
                debug!("search_delete file_count={} lancedb {}ms", file_ids.len(), step_start.elapsed().as_millis());
                anyhow::Ok(())
            };

            let tantivy_full_delete = async {
                let lock_wait_start = Instant::now();
                {
                    let _guard = self.full_index_write_lock.lock().await;
                    let locked_at = Instant::now();
                    debug!(
                        "search_delete file_count={} tantivy_full_lock_wait_ms={}",
                        file_ids.len(),
                        lock_wait_start.elapsed().as_millis()
                    );
                    self.full_index_writer.delete_by_field("file_id", file_ids).await?;
                    debug!(
                        "search_delete file_count={} tantivy_full_inner_ms={}",
                        file_ids.len(),
                        locked_at.elapsed().as_millis()
                    );
                }
                reload_reader(&self.full_index_reader, "full_index")?;
                debug!(
                    "search_delete file_count={} tantivy_full {}ms",
                    file_ids.len(),
                    lock_wait_start.elapsed().as_millis()
                );
                anyhow::Ok(())
            };

            let (tantivy_result, lancedb_result, tantivy_full_result) =
                tokio::join!(tantivy_delete, lancedb_delete, tantivy_full_delete);
            tantivy_result?;
            lancedb_result?;
            tantivy_full_result?;
        }
        if let Some(kb_ids) = kb_ids.filter(|ids| !ids.is_empty()) {
            let tantivy_delete = async {
                let lock_wait_start = Instant::now();
                {
                    let _guard = self.index_write_lock.lock().await;
                    let locked_at = Instant::now();
                    debug!(
                        "search_delete kb_count={} tantivy_lock_wait_ms={}",
                        kb_ids.len(),
                        lock_wait_start.elapsed().as_millis()
                    );
                    self.index_writer.delete_by_field("kb_id", kb_ids).await?;
                    debug!(
                        "search_delete kb_count={} tantivy_inner_ms={}",
                        kb_ids.len(),
                        locked_at.elapsed().as_millis()
                    );
                }
                reload_reader(&self.index_reader, "index")?;
                debug!("search_delete kb_count={} tantivy {}ms", kb_ids.len(), lock_wait_start.elapsed().as_millis());
                anyhow::Ok(())
            };

            let lancedb_delete = async {
                let step_start = Instant::now();
                if kb_ids.len() == 1 {
                    lancedb::delete_by_kb(kb_ids[0]).await?;
                } else {
                    lancedb::delete_by_kbs(kb_ids).await?;
                }
                debug!("search_delete kb_count={} lancedb {}ms", kb_ids.len(), step_start.elapsed().as_millis());
                anyhow::Ok(())
            };

            let tantivy_full_delete = async {
                let lock_wait_start = Instant::now();
                {
                    let _guard = self.full_index_write_lock.lock().await;
                    let locked_at = Instant::now();
                    debug!(
                        "search_delete kb_count={} tantivy_full_lock_wait_ms={}",
                        kb_ids.len(),
                        lock_wait_start.elapsed().as_millis()
                    );
                    self.full_index_writer.delete_by_field("kb_id", kb_ids).await?;
                    debug!(
                        "search_delete kb_count={} tantivy_full_inner_ms={}",
                        kb_ids.len(),
                        locked_at.elapsed().as_millis()
                    );
                }
                reload_reader(&self.full_index_reader, "full_index")?;
                debug!(
                    "search_delete kb_count={} tantivy_full {}ms",
                    kb_ids.len(),
                    lock_wait_start.elapsed().as_millis()
                );
                anyhow::Ok(())
            };

            let (tantivy_result, lancedb_result, tantivy_full_result) =
                tokio::join!(tantivy_delete, lancedb_delete, tantivy_full_delete);
            tantivy_result?;
            lancedb_result?;
            tantivy_full_result?;
        }
        debug!(
            "search_delete total {}ms file_count={:?} kb_count={:?}",
            overall_start.elapsed().as_millis(),
            file_ids.map(|ids| ids.len()),
            kb_ids.map(|ids| ids.len())
        );
        Ok(())
    }

    pub async fn search(
        &self, query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
    ) -> anyhow::Result<Vec<SearchResultItem>> {
        let total_start = Instant::now();
        debug!("Searching for query: {}", query);

        let synonym_start = Instant::now();
        let synonym_map = match self.load_query_synonyms(query).await {
            Ok(map) => map,
            Err(e) => {
                warn!("Failed to load query synonyms for '{}': {}", query, e);
                HashMap::new()
            }
        };
        debug!(
            "Search synonym lookup {}ms count={}",
            synonym_start.elapsed().as_millis(),
            synonym_map.values().map(Vec::len).sum::<usize>()
        );

        let index_reader = self.index_reader.clone();
        let schema = self.schema.clone();
        let tantivy_query = query.to_string();
        let tantivy_file_ids = file_ids.cloned();
        let tantivy_kb_ids = kb_ids.cloned();
        let tantivy_synonym_map = synonym_map.clone();
        let tantivy_started = Instant::now();
        let tantivy_task = tokio::task::spawn_blocking(move || {
            let synonym_ref = if tantivy_synonym_map.is_empty() { None } else { Some(&tantivy_synonym_map) };
            let results = tantivy_engine::search_sync(
                &index_reader,
                &schema,
                &tantivy_query,
                tantivy_file_ids.as_ref(),
                tantivy_kb_ids.as_ref(),
                synonym_ref,
            )?;
            debug!("Tantivy branch total {}ms", tantivy_started.elapsed().as_millis());
            anyhow::Ok(results)
        });

        let lancedb_started = Instant::now();
        let lancedb_result = lancedb::search(query, file_ids, kb_ids).await;
        debug!("LanceDB branch total {}ms", lancedb_started.elapsed().as_millis());
        let tantivy_result = tantivy_task.await.map_err(|err| anyhow!("Tantivy search task failed: {}", err))?;

        // 使用 tantivy 搜索
        let tantivy_results = tantivy_result?;
        debug!("Tantivy results count: {}", tantivy_results.len());

        // 使用 lancedb 搜索
        let lancedb_results = match lancedb_result {
            Ok(results) => {
                debug!("LanceDB results count: {}", results.len());
                results
            }
            Err(err) => {
                warn!("Vector search failed for query {:?}, falling back to Tantivy-only results: {}", query, err);
                Vec::new()
            }
        };

        // 合并结果：使用 HashMap 按 id 去重，保留最高分数，同时去除内容为空的结果
        let mut merged_map: HashMap<i64, SearchResultItem> = HashMap::new();

        for result in tantivy_results {
            // 跳过空内容（包括仅有空白的情况）
            if result.content.trim().is_empty() {
                continue;
            }
            merged_map.insert(result.id, result);
        }

        for result in lancedb_results {
            // 跳过空内容（包括仅有空白的情况）
            if result.content.trim().is_empty() {
                continue;
            }

            merged_map
                .entry(result.id)
                .and_modify(|e| {
                    // 如果已存在，取两者中分数较高的
                    if result.score > e.score {
                        *e = result.clone();
                    }
                })
                .or_insert(result);
        }

        // 转换为 Vec 并按分数降序排序
        let mut merged_results: Vec<SearchResultItem> = merged_map.into_values().collect();
        merged_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        info!("Merged results count: {}", merged_results.len());

        // 如果结果为空，直接返回
        if merged_results.is_empty() {
            return Ok(merged_results);
        }

        // 使用 BGE-Rerank 重排序（失败时内部回退为原结果，无需预先 clone 整个结果集）
        let mut final_results = self.rerank(query, merged_results).await;
        let limit = config::get().search.limit.max(1);
        if final_results.len() > limit {
            final_results.truncate(limit);
        }

        debug!("Search total {}ms", total_start.elapsed().as_millis());
        Ok(final_results)
    }

    pub async fn search_full(
        &self, query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
    ) -> anyhow::Result<Vec<FullSearchResultItem>> {
        let synonym_map = match self.load_query_synonyms(query).await {
            Ok(map) => map,
            Err(e) => {
                warn!("Failed to load query synonyms for '{}': {}", query, e);
                HashMap::new()
            }
        };
        let synonym_ref = if synonym_map.is_empty() { None } else { Some(&synonym_map) };
        tantivy_engine::search_with_snippet(
            &self.full_index_reader,
            &self.full_schema,
            query,
            file_ids,
            kb_ids,
            FULL_SNIPPET_MAX_CHARS,
            synonym_ref,
        )
        .await
    }

    pub async fn search_image(
        &self, image_embedding: Vec<f32>, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
    ) -> anyhow::Result<Vec<SearchResultItem>> {
        lancedb::search_image(image_embedding, file_ids, kb_ids).await
    }

    /// 计算每个结果（按输入顺序对齐）的 rerank 分数。
    /// 仅借用 results，失败时不消耗它，使调用方可零拷贝回退。
    async fn compute_rerank_scores(
        &self, query: &str, results: &[SearchResultItem],
    ) -> anyhow::Result<Vec<Option<f32>>> {
        let cfg = config::get();

        // 提取所有文档内容用于重排序，并做去重（按内容借用，去重映射只需单次 clone 进 documents）
        let mut documents: Vec<String> = Vec::new();
        let mut document_index_map: Vec<usize> = Vec::with_capacity(results.len());
        let mut document_index_by_content: HashMap<&str, usize> = HashMap::new();
        for result in results.iter() {
            if let Some(&idx) = document_index_by_content.get(result.content.as_str()) {
                document_index_map.push(idx);
                continue;
            }
            let idx = documents.len();
            documents.push(result.content.clone());
            document_index_by_content.insert(result.content.as_str(), idx);
            document_index_map.push(idx);
        }

        // 根据 URL 后缀判断使用哪种 rerank 接口格式
        let use_v1_format = cfg.services.rerank_url.ends_with("/v1/rerank");

        // 调用 BGE-Rerank API
        let rerank_http_start = Instant::now();
        let response = if use_v1_format {
            let rerank_request = RerankRequest {
                model: cfg.ai.rerank_model.clone(),
                query: query.to_string(),
                documents: documents.clone(),
            };
            RERANK_HTTP_CLIENT
                .post(&cfg.services.rerank_url)
                .timeout(Duration::from_secs(cfg.search.rerank_timeout_secs))
                .json(&rerank_request)
                .send()
                .await?
        } else {
            let rerank_request = SimpleRerankRequest { query: query.to_string(), texts: documents.clone() };
            RERANK_HTTP_CLIENT
                .post(&cfg.services.rerank_url)
                .timeout(Duration::from_secs(cfg.search.rerank_timeout_secs))
                .json(&rerank_request)
                .send()
                .await?
        };
        debug!("Rerank HTTP request {}ms", rerank_http_start.elapsed().as_millis());

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Rerank API failed with status {}: {}; documents={}", status, error_text, documents.len());
        }

        // 先获取响应文本用于调试
        let rerank_read_start = Instant::now();
        let response_text = response.text().await?;
        debug!("Rerank response read {}ms", rerank_read_start.elapsed().as_millis());

        // 解析 JSON 响应
        let rerank_parse_start = Instant::now();
        let mut rerank_scores: Vec<Option<f32>> = vec![None; documents.len()];
        if use_v1_format {
            let rerank_response: RerankResponse = serde_json::from_str(&response_text)?;
            debug!("Rerank response parse {}ms", rerank_parse_start.elapsed().as_millis());
            if rerank_response.results.len() != documents.len() {
                anyhow::bail!(
                    "Rerank results count mismatch: expected {}, got {}",
                    documents.len(),
                    rerank_response.results.len()
                );
            }
            for rerank_result in &rerank_response.results {
                if let Some(score) = rerank_scores.get_mut(rerank_result.index) {
                    *score = Some(rerank_result.relevance_score);
                }
            }
        } else {
            let simple_results: Vec<SimpleRerankResult> = serde_json::from_str(&response_text)?;
            debug!("Rerank response parse {}ms", rerank_parse_start.elapsed().as_millis());
            if simple_results.len() != documents.len() {
                anyhow::bail!(
                    "Rerank results count mismatch: expected {}, got {}",
                    documents.len(),
                    simple_results.len()
                );
            }
            for result in &simple_results {
                if let Some(score) = rerank_scores.get_mut(result.index) {
                    *score = Some(result.score);
                }
            }
        }

        // 将去重后的分数映射回每个结果（按输入顺序）
        let per_result: Vec<Option<f32>> =
            document_index_map.iter().map(|&doc_idx| rerank_scores.get(doc_idx).copied().flatten()).collect();
        Ok(per_result)
    }

    async fn rerank(&self, query: &str, results: Vec<SearchResultItem>) -> Vec<SearchResultItem> {
        let rerank_total_start = Instant::now();
        let cfg = config::get();

        // 计算分数；失败则原样返回（无需 clone 回退）
        let scores = match self.compute_rerank_scores(query, &results).await {
            Ok(scores) => scores,
            Err(err) => {
                warn!("Rerank failed for query {:?}, returning merged search results without rerank: {}", query, err);
                return results;
            }
        };

        // 使用重排序分数更新结果
        let mut reranked_results: Vec<SearchResultItem> = results
            .into_iter()
            .enumerate()
            .map(|(i, mut result)| {
                if let Some(Some(score)) = scores.get(i) {
                    result.score = *score;
                }
                result
            })
            .collect();

        // 按新分数降序排序
        reranked_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        let threshold = cfg.ai.rerank_threshold;
        let filter_results: Vec<SearchResultItem> =
            reranked_results.into_iter().filter(|f| f.score >= threshold).collect();
        info!("Reranked results count: {}", filter_results.len());
        debug!("Rerank total {}ms", rerank_total_start.elapsed().as_millis());
        filter_results
    }

    /// 使用知识图谱扩展查询
    /// 从查询中识别实体，并查找相关实体来扩展查询
    pub async fn expand_query_with_graph(&self, query: &str, kb_ids: Option<&Vec<i64>>) -> anyhow::Result<Vec<String>> {
        let pool = match &self.pool {
            Some(p) => p,
            None => return Ok(vec![query.to_string()]), // 如果没有数据库连接，直接返回原查询
        };

        let mut expanded_queries = vec![query.to_string()];

        // 1. 在知识图谱中搜索匹配的实体
        let mut qb = QueryBuilder::new("SELECT DISTINCT name, entity_type FROM graph_nodes WHERE name LIKE ");
        qb.push_bind(format!("%{}%", query));

        if let Some(ids) = kb_ids
            && !ids.is_empty()
        {
            qb.push(" AND kb_id IN (");
            let mut separated = qb.separated(", ");
            for id in ids {
                separated.push_bind(id);
            }
            qb.push(")");
        }
        qb.push(" LIMIT 10");
        let entities: Vec<(String, String)> = qb.build_query_as().fetch_all(pool).await?;

        // 2. 对于每个匹配的实体，查找相关实体
        for (entity_name, _) in entities.iter().take(3) {
            // 限制为前3个实体
            // 查找与该实体相关的其他实体（通过边连接）
            let related_sql = r#"
                SELECT DISTINCT n.name
                FROM graph_nodes n
                JOIN graph_edges e ON (n.id = e.target_node_id OR n.id = e.source_node_id)
                JOIN graph_nodes source ON (source.id = e.source_node_id OR source.id = e.target_node_id)
                WHERE source.name = ?
                AND n.name != ?
                LIMIT 5
            "#;

            let related_entities: Vec<(String,)> =
                sqlx::query_as(related_sql).bind(entity_name).bind(entity_name).fetch_all(pool).await?;

            // 添加相关实体到扩展查询
            for (related_name,) in related_entities {
                if !expanded_queries.contains(&related_name) {
                    expanded_queries.push(related_name);
                }
            }
        }

        info!("Query expansion: '{}' -> {:?}", query, expanded_queries);
        Ok(expanded_queries)
    }

    /// 清理 LanceDB 已删除的记录，释放空间
    pub async fn compact_lancedb(&self) -> anyhow::Result<lancedb::CompactStats> {
        lancedb::compact().await
    }

    /// 强制合并 Tantivy segment，减少碎片与已删除文档 tombstone。
    pub async fn force_merge_tantivy_indexes(
        &self,
    ) -> anyhow::Result<(tantivy_engine::ForceMergeStats, tantivy_engine::ForceMergeStats)> {
        let _rebuild_guard = self.rebuild_lock.lock().await;
        let (slice_stats, full_stats) = {
            let _slice_write_guard = self.index_write_lock.lock().await;
            let _full_write_guard = self.full_index_write_lock.lock().await;
            let slice_stats = self.index_writer.force_merge().await?;
            let full_stats = self.full_index_writer.force_merge().await?;
            (slice_stats, full_stats)
        };

        reload_reader(&self.index_reader, "index")?;
        reload_reader(&self.full_index_reader, "full_index")?;

        Ok((slice_stats, full_stats))
    }

    /// 确保同义词缓存新鲜（TTL 内复用，过期则重载全部 enabled 行）。
    async fn ensure_synonym_cache(&self, pool: &SqlitePool) -> anyhow::Result<()> {
        {
            let guard = self.synonym_cache.read().await;
            if let Some(cache) = guard.as_ref()
                && cache.loaded_at.elapsed() < SYNONYM_CACHE_TTL
            {
                return Ok(());
            }
        }
        let mut guard = self.synonym_cache.write().await;
        // 双检：可能已有其他任务在等待写锁期间刷新过。
        if let Some(cache) = guard.as_ref()
            && cache.loaded_at.elapsed() < SYNONYM_CACHE_TTL
        {
            return Ok(());
        }
        let rows: Vec<SynonymRow> =
            sqlx::query_as("SELECT term, synonym, weight, bidirectional FROM search_synonyms WHERE enabled = 1")
                .fetch_all(pool)
                .await?;
        *guard = Some(SynonymCache::build(rows));
        Ok(())
    }

    /// 主动失效同义词缓存（同义词增删改后调用，使变更立即生效）。
    pub async fn invalidate_synonym_cache(&self) {
        *self.synonym_cache.write().await = None;
    }

    async fn load_query_synonyms(&self, query: &str) -> anyhow::Result<tantivy_engine::SynonymMap> {
        let cfg = config::get();
        if !cfg.search.synonym_enabled {
            return Ok(HashMap::new());
        }
        let Some(pool) = &self.pool else {
            return Ok(HashMap::new());
        };

        let terms = extract_query_terms(query);
        if terms.is_empty() {
            return Ok(HashMap::new());
        }

        self.ensure_synonym_cache(pool).await?;
        let cache_guard = self.synonym_cache.read().await;
        let Some(cache) = cache_guard.as_ref() else {
            return Ok(HashMap::new());
        };
        if cache.rows.is_empty() {
            return Ok(HashMap::new());
        }

        // 收集与查询词相关的候选行下标（命中 term 或 synonym 列）。
        let mut candidate_idx: HashSet<usize> = HashSet::new();
        for term in &terms {
            if let Some(idxs) = cache.by_term.get(term) {
                candidate_idx.extend(idxs.iter().copied());
            }
            if let Some(idxs) = cache.by_synonym.get(term) {
                candidate_idx.extend(idxs.iter().copied());
            }
        }
        if candidate_idx.is_empty() {
            return Ok(HashMap::new());
        }

        let input_terms: HashSet<&str> = terms.iter().map(String::as_str).collect();
        let mut synonym_map: tantivy_engine::SynonymMap = HashMap::new();
        let max_per_term = cfg.search.max_synonyms_per_term.max(1);
        let max_total = cfg.search.max_total_synonyms.max(1);
        let boost_factor = cfg.search.synonym_boost.max(0.0);
        let mut total_inserted = 0usize;

        for idx in candidate_idx {
            let row = &cache.rows[idx];
            let boost = row.weight.max(0.0) * boost_factor;
            if boost <= 0.0 {
                continue;
            }

            if input_terms.contains(row.term.as_str())
                && insert_synonym(&mut synonym_map, row.term.as_str(), row.synonym.as_str(), boost, max_per_term)
            {
                total_inserted += 1;
                if total_inserted >= max_total {
                    break;
                }
            }

            if row.bidirectional != 0
                && input_terms.contains(row.synonym.as_str())
                && insert_synonym(&mut synonym_map, row.synonym.as_str(), row.term.as_str(), boost, max_per_term)
            {
                total_inserted += 1;
                if total_inserted >= max_total {
                    break;
                }
            }
        }

        Ok(synonym_map)
    }

    /// 使用图谱增强的搜索
    /// 先扩展查询，然后对每个扩展查询进行搜索，最后合并去重结果
    pub async fn search_with_graph_expansion(
        &self, query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
    ) -> anyhow::Result<Vec<SearchResultItem>> {
        // 1. 扩展查询
        let expanded_queries = self.expand_query_with_graph(query, kb_ids).await?;

        if expanded_queries.len() == 1 {
            // 没有扩展，直接使用原查询
            return self.search(query, file_ids, kb_ids).await;
        }

        // 2. 对每个扩展查询并发搜索（彼此独立，无需串行）
        let mut all_results: HashMap<i64, SearchResultItem> = HashMap::new();

        let search_futures = expanded_queries.iter().enumerate().map(|(idx, expanded_query)| {
            // 原始查询的结果权重更高
            let weight = if idx == 0 { 1.0 } else { 0.7 };
            async move {
                let results = self.search(expanded_query, file_ids, kb_ids).await;
                (weight, results)
            }
        });
        let per_query = futures::future::join_all(search_futures).await;

        for (weight, results) in per_query {
            for mut result in results? {
                result.score *= weight;

                all_results
                    .entry(result.id)
                    .and_modify(|e| {
                        // 如果已存在，取两者中分数较高的
                        if result.score > e.score {
                            *e = result.clone();
                        }
                    })
                    .or_insert(result);
            }
        }

        // 3. 转换为Vec并按分数排序
        let mut merged_results: Vec<SearchResultItem> = all_results.into_values().collect();
        merged_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        info!("Graph-expanded search returned {} results", merged_results.len());
        Ok(merged_results)
    }
}

async fn fetch_file_contents_by_ids(pool: &SqlitePool, ids: &[i64]) -> anyhow::Result<HashMap<i64, String>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut query_builder: QueryBuilder<'_, Sqlite> = QueryBuilder::new("SELECT id, content FROM files WHERE id IN (");
    let mut separated = query_builder.separated(", ");
    for id in ids {
        separated.push_bind(id);
    }
    separated.push_unseparated(")");

    let rows: Vec<(i64, Option<String>)> = query_builder.build_query_as().fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id, content)| (id, content.unwrap_or_default())).collect())
}

fn sanitize_job_tag(input: &str) -> String {
    let trimmed = input.trim();
    if !trimmed.is_empty() {
        let sanitized: String = trimmed
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '_' })
            .collect();
        if !sanitized.is_empty() {
            return sanitized;
        }
    }
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or_default();
    now_ms.to_string()
}

fn cleanup_dir_if_exists(path: &str) -> anyhow::Result<()> {
    let path_ref = Path::new(path);
    if !path_ref.exists() {
        return Ok(());
    }
    fs::remove_dir_all(path_ref).with_context(|| format!("remove dir failed: {}", path))?;
    Ok(())
}

fn swap_index_dir(active_path: &str, staged_path: &str, backup_path: &str) -> anyhow::Result<()> {
    let active = Path::new(active_path);
    let staged = Path::new(staged_path);
    let backup = Path::new(backup_path);
    if !staged.exists() {
        return Err(anyhow!("staged index path not found: {}", staged_path));
    }

    if let Some(parent) = active.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create parent dir failed: {}", parent.display()))?;
    }

    if backup.exists() {
        fs::remove_dir_all(backup).with_context(|| format!("remove backup dir failed: {}", backup.display()))?;
    }

    let moved_old = if active.exists() {
        fs::rename(active, backup)
            .with_context(|| format!("rename active->backup failed: {} -> {}", active_path, backup_path))?;
        true
    } else {
        false
    };

    if let Err(err) = fs::rename(staged, active) {
        if moved_old && backup.exists() {
            let _ = fs::rename(backup, active);
        }
        return Err(err).with_context(|| format!("rename staged->active failed: {} -> {}", staged_path, active_path));
    }

    Ok(())
}

fn restore_backup_dir(active_path: &str, backup_path: &str) -> anyhow::Result<()> {
    let active = Path::new(active_path);
    let backup = Path::new(backup_path);
    if !backup.exists() {
        return Ok(());
    }
    if active.exists() {
        fs::remove_dir_all(active).with_context(|| format!("remove active dir failed: {}", active_path))?;
    }
    fs::rename(backup, active).with_context(|| format!("restore backup failed: {} -> {}", backup_path, active_path))?;
    Ok(())
}

fn build_reader(index: &Index, label: &str) -> IndexReader {
    let start = Instant::now();
    let reader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .unwrap_or_else(|e| panic!("failed to create tantivy {} reader: {}", label, e));
    debug!("Tantivy {} index.reader init {}ms", label, start.elapsed().as_millis());
    reader
}

fn extract_query_terms(query: &str) -> Vec<String> {
    let mut terms =
        chinese_tokenizer::FastChineseTokenizer::new(chinese_tokenizer::SegmentationMode::Search).segment(query);
    terms.push(query.trim().to_string());
    terms.retain(|t| !t.trim().is_empty());
    terms.sort();
    terms.dedup();
    if terms.len() > MAX_QUERY_TERMS_FOR_SYNONYM_LOOKUP {
        terms.truncate(MAX_QUERY_TERMS_FOR_SYNONYM_LOOKUP);
    }
    terms
}

fn insert_synonym(
    synonym_map: &mut tantivy_engine::SynonymMap, source_term: &str, synonym_term: &str, boost: f32,
    max_per_term: usize,
) -> bool {
    let source = source_term.trim();
    let synonym = synonym_term.trim();
    if source.is_empty() || synonym.is_empty() || source == synonym {
        return false;
    }

    let entry = synonym_map.entry(source.to_string()).or_default();
    if entry.iter().any(|candidate| candidate.term == synonym) {
        return false;
    }
    if entry.len() >= max_per_term {
        return false;
    }

    entry.push(tantivy_engine::SynonymTerm { term: synonym.to_string(), boost });
    true
}

fn reload_reader(reader: &IndexReader, label: &str) -> tantivy::Result<()> {
    let start = Instant::now();
    reader.reload()?;
    debug!("Tantivy {} reader reload {}ms", label, start.elapsed().as_millis());
    Ok(())
}
