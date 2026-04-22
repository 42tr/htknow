use std::{
    collections::{HashMap, HashSet}, fs, path::Path, sync::Arc, time::{Duration, Instant, SystemTime, UNIX_EPOCH}
};

use anyhow::{Context, anyhow};
use log::{debug, info, warn};
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, SqlitePool};
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
const REBUILD_BATCH_SIZE: i64 = 500;
static RERANK_HTTP_CLIENT: Lazy<Client> = Lazy::new(Client::new);

#[derive(Debug, Serialize)]
struct RerankRequest {
    model: String,
    query: String,
    documents: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RerankResponse {
    results: Vec<RerankResult>,
}

#[derive(Debug, Deserialize)]
struct RerankResult {
    index: usize,
    relevance_score: f32,
}

#[derive(Debug, sqlx::FromRow)]
struct SynonymRow {
    term: String,
    synonym: String,
    weight: f32,
    bidirectional: i64,
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
struct RebuildFullRow {
    id: i64,
    kb_id: Option<i64>,
    filename: String,
    content: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RebuildProgress {
    pub phase: String,
    pub total_docs: i64,
    pub processed_docs: i64,
}

#[derive(Clone)]
pub struct SearchEngine {
    index: Index,
    schema: Schema,
    index_reader: IndexReader,
    index_write_lock: Arc<Mutex<()>>,
    full_index: Index,
    full_schema: Schema,
    full_index_reader: IndexReader,
    full_index_write_lock: Arc<Mutex<()>>,
    rebuild_lock: Arc<Mutex<()>>,
    pool: Option<SqlitePool>,
}

impl SearchEngine {
    pub async fn init() -> Self {
        lancedb::init().await.expect("init lancedb failed");
        let (schema, index) = tantivy_engine::init().unwrap();
        let (full_schema, full_index) = tantivy_engine::init_full().unwrap();
        let index_reader = build_reader(&index, "index");
        let full_index_reader = build_reader(&full_index, "full_index");
        Self {
            index,
            schema,
            index_reader,
            index_write_lock: Arc::new(Mutex::new(())),
            full_index,
            full_schema,
            full_index_reader,
            full_index_write_lock: Arc::new(Mutex::new(())),
            rebuild_lock: Arc::new(Mutex::new(())),
            pool: None,
        }
    }

    pub fn with_pool(mut self, pool: SqlitePool) -> Self {
        self.pool = Some(pool);
        self
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
                .bind(REBUILD_BATCH_SIZE)
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
                tantivy_engine::write_documents_batch(&slice_temp_index, &slice_schema, docs).await?;
                processed_docs += batch_size;
                on_progress(RebuildProgress { phase: "build_slice".to_string(), total_docs, processed_docs }).await;
            }

            on_progress(RebuildProgress { phase: "build_full".to_string(), total_docs, processed_docs }).await;
            let mut last_file_id = 0_i64;
            loop {
                let rows: Vec<RebuildFullRow> = sqlx::query_as(
                    "SELECT id, kb_id, filename, content \
                     FROM files \
                     WHERE status = 1 AND id > ? \
                     ORDER BY id ASC \
                     LIMIT ?",
                )
                .bind(last_file_id)
                .bind(REBUILD_BATCH_SIZE)
                .fetch_all(pool)
                .await?;
                if rows.is_empty() {
                    break;
                }
                last_file_id = rows.last().map(|row| row.id).unwrap_or(last_file_id);
                let batch_size = rows.len() as i64;
                let docs: Vec<tantivy_engine::Document> = rows
                    .into_iter()
                    .map(|row| {
                        let full_content = row.content.unwrap_or_default();
                        let index_content = if full_content.trim().is_empty() {
                            row.filename
                        } else {
                            format!("{}\n\n{}", row.filename, full_content)
                        };
                        tantivy_engine::Document::new(row.id, row.id, row.kb_id, index_content)
                    })
                    .collect();
                tantivy_engine::write_documents_batch(&full_temp_index, &full_schema, docs).await?;
                processed_docs += batch_size;
                on_progress(RebuildProgress { phase: "build_full".to_string(), total_docs, processed_docs }).await;
            }

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
            let _ = cleanup_dir_if_exists(&slice_temp_path);
            let _ = cleanup_dir_if_exists(&full_temp_path);
            let _ = cleanup_dir_if_exists(&slice_backup_path);
            let _ = cleanup_dir_if_exists(&full_backup_path);
        }

        rebuild_result
    }

    pub async fn write(&self, doc: tantivy_engine::Document, image_embedding: Option<Vec<f32>>) -> anyhow::Result<()> {
        {
            let _guard = self.index_write_lock.lock().await;
            tantivy_engine::write_documents(&self.index, &self.schema, doc.clone()).await?;
        }

        let mut lancedb_doc = lancedb::Document::new(doc.id, doc.file_id, doc.kb_id, doc.content);
        if let Some(image_embedding) = image_embedding {
            lancedb_doc = lancedb_doc.with_image_embedding(image_embedding);
        }
        lancedb::write_documents(lancedb_doc).await?;
        Ok(())
    }

    pub async fn write_batch(
        &self, docs: Vec<tantivy_engine::Document>, image_embeddings: Vec<Option<Vec<f32>>>,
    ) -> anyhow::Result<()> {
        if docs.is_empty() {
            return Ok(());
        }

        {
            let _guard = self.index_write_lock.lock().await;
            tantivy_engine::write_documents_batch(&self.index, &self.schema, docs.clone()).await?;
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

    pub async fn write_full(&self, doc: tantivy_engine::Document) -> anyhow::Result<()> {
        {
            let _guard = self.full_index_write_lock.lock().await;
            tantivy_engine::write_documents(&self.full_index, &self.full_schema, doc).await?;
        }
        Ok(())
    }

    pub fn reload_readers(&self) -> anyhow::Result<()> {
        reload_reader(&self.index_reader, "index")?;
        reload_reader(&self.full_index_reader, "full_index")?;
        Ok(())
    }

    pub async fn delete(&self, file_id: Option<i64>, kb_id: Option<i64>) -> anyhow::Result<()> {
        let overall_start = Instant::now();
        if let Some(file_id) = file_id {
            let step_start = Instant::now();
            {
                let _guard = self.index_write_lock.lock().await;
                tantivy_engine::delete_by_file(&self.index, &self.schema, file_id).await?;
            }
            reload_reader(&self.index_reader, "index")?;
            debug!("search_delete file_id={} tantivy {}ms", file_id, step_start.elapsed().as_millis());

            let step_start = Instant::now();
            lancedb::delete_by_file(file_id).await?;
            debug!("search_delete file_id={} lancedb {}ms", file_id, step_start.elapsed().as_millis());

            let step_start = Instant::now();
            {
                let _guard = self.full_index_write_lock.lock().await;
                tantivy_engine::delete_by_file(&self.full_index, &self.full_schema, file_id).await?;
            }
            reload_reader(&self.full_index_reader, "full_index")?;
            debug!("search_delete file_id={} tantivy_full {}ms", file_id, step_start.elapsed().as_millis());
        }
        if let Some(kb_id) = kb_id {
            let step_start = Instant::now();
            {
                let _guard = self.index_write_lock.lock().await;
                tantivy_engine::delete_by_kb(&self.index, &self.schema, kb_id).await?;
            }
            reload_reader(&self.index_reader, "index")?;
            debug!("search_delete kb_id={} tantivy {}ms", kb_id, step_start.elapsed().as_millis());

            let step_start = Instant::now();
            lancedb::delete_by_kb(kb_id).await?;
            debug!("search_delete kb_id={} lancedb {}ms", kb_id, step_start.elapsed().as_millis());

            let step_start = Instant::now();
            {
                let _guard = self.full_index_write_lock.lock().await;
                tantivy_engine::delete_by_kb(&self.full_index, &self.full_schema, kb_id).await?;
            }
            reload_reader(&self.full_index_reader, "full_index")?;
            debug!("search_delete kb_id={} tantivy_full {}ms", kb_id, step_start.elapsed().as_millis());
        }
        debug!("search_delete total {}ms file_id={:?} kb_id={:?}", overall_start.elapsed().as_millis(), file_id, kb_id);
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
        let tantivy_result =
            tantivy_task.await.map_err(|err| anyhow!("Tantivy search task failed: {}", err))?;

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

        // 使用 BGE-Rerank 重排序
        let fallback_results = merged_results.clone();
        let final_results = match self.rerank(query, merged_results).await {
            Ok(reranked_results) => {
                info!("Reranked results count: {}", reranked_results.len());
                reranked_results
            }
            Err(err) => {
                warn!("Rerank failed for query {:?}, returning merged search results without rerank: {}", query, err);
                fallback_results
            }
        };

        let mut final_results = final_results;
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

    async fn rerank(&self, query: &str, results: Vec<SearchResultItem>) -> anyhow::Result<Vec<SearchResultItem>> {
        let cfg = config::get();
        let rerank_total_start = Instant::now();

        // 提取所有文档内容用于重排序，并做去重
        let mut documents: Vec<String> = Vec::new();
        let mut document_index_map: Vec<usize> = Vec::with_capacity(results.len());
        let mut document_index_by_content: HashMap<String, usize> = HashMap::new();
        for result in results.iter() {
            if let Some(&idx) = document_index_by_content.get(&result.content) {
                document_index_map.push(idx);
                continue;
            }
            let idx = documents.len();
            documents.push(result.content.clone());
            document_index_by_content.insert(result.content.clone(), idx);
            document_index_map.push(idx);
        }

        // 构造请求
        let rerank_request = RerankRequest { model: cfg.ai.rerank_model.clone(), query: query.to_string(), documents };

        // 调用 BGE-Rerank API
        let rerank_http_start = Instant::now();
        let response = RERANK_HTTP_CLIENT
            .post(&cfg.services.rerank_url)
            .timeout(Duration::from_secs(cfg.search.rerank_timeout_secs))
            .json(&rerank_request)
            .send()
            .await?;
        debug!("Rerank HTTP request {}ms", rerank_http_start.elapsed().as_millis());

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Rerank API failed with status {}: {}; documents={}",
                status,
                error_text,
                rerank_request.documents.len()
            );
        }

        // 先获取响应文本用于调试
        let rerank_read_start = Instant::now();
        let response_text = response.text().await?;
        debug!("Rerank response read {}ms", rerank_read_start.elapsed().as_millis());

        // 解析 JSON 响应
        let rerank_parse_start = Instant::now();
        let rerank_response: RerankResponse = serde_json::from_str(&response_text)?;
        debug!("Rerank response parse {}ms", rerank_parse_start.elapsed().as_millis());

        // 检查返回的结果数量是否匹配
        if rerank_response.results.len() != rerank_request.documents.len() {
            anyhow::bail!(
                "Rerank results count mismatch: expected {}, got {}",
                rerank_request.documents.len(),
                rerank_response.results.len()
            );
        }

        let mut rerank_scores: Vec<Option<f32>> = vec![None; rerank_request.documents.len()];
        for rerank_result in &rerank_response.results {
            if let Some(score) = rerank_scores.get_mut(rerank_result.index) {
                *score = Some(rerank_result.relevance_score);
            }
        }

        // 使用重排序分数更新结果
        let mut reranked_results: Vec<SearchResultItem> = results
            .into_iter()
            .enumerate()
            .map(|(i, mut result)| {
                // 根据去重后的 index 找到对应的重排序结果
                if let Some(doc_index) = document_index_map.get(i) {
                    if let Some(score) = rerank_scores.get(*doc_index).and_then(|s| *s) {
                        result.score = score;
                    }
                }
                result
            })
            .collect();

        // 按新分数降序排序
        reranked_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        let threshold = cfg.ai.rerank_threshold;
        let filter_results = reranked_results.into_iter().filter(|f| f.score >= threshold).collect();
        debug!("Rerank total {}ms", rerank_total_start.elapsed().as_millis());
        Ok(filter_results)
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

        if let Some(ids) = kb_ids {
            if !ids.is_empty() {
                qb.push(" AND kb_id IN (");
                let mut separated = qb.separated(", ");
                for id in ids {
                    separated.push_bind(id);
                }
                qb.push(")");
            }
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

        let mut qb = QueryBuilder::new(
            "SELECT term, synonym, weight, bidirectional FROM search_synonyms \
            WHERE enabled = 1 AND (term IN (",
        );
        {
            let mut separated = qb.separated(", ");
            for term in &terms {
                separated.push_bind(term);
            }
        }
        qb.push(") OR synonym IN (");
        {
            let mut separated = qb.separated(", ");
            for term in &terms {
                separated.push_bind(term);
            }
        }
        qb.push("))");

        let rows: Vec<SynonymRow> = qb.build_query_as().fetch_all(pool).await?;
        if rows.is_empty() {
            return Ok(HashMap::new());
        }

        let input_terms: HashSet<&str> = terms.iter().map(String::as_str).collect();
        let mut synonym_map: tantivy_engine::SynonymMap = HashMap::new();
        let max_per_term = cfg.search.max_synonyms_per_term.max(1);
        let max_total = cfg.search.max_total_synonyms.max(1);
        let boost_factor = cfg.search.synonym_boost.max(0.0);
        let mut total_inserted = 0usize;

        for row in rows {
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

        // 2. 对每个扩展查询进行搜索
        let mut all_results: HashMap<i64, SearchResultItem> = HashMap::new();

        for (idx, expanded_query) in expanded_queries.iter().enumerate() {
            let results = self.search(expanded_query, file_ids, kb_ids).await?;

            // 原始查询的结果权重更高
            let weight = if idx == 0 { 1.0 } else { 0.7 };

            for mut result in results {
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
