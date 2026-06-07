use std::{
    collections::{HashMap, HashSet}, future::Future, sync::{
        Arc, atomic::{AtomicBool, AtomicU64, Ordering}
    }, time::{Duration, Instant, SystemTime, UNIX_EPOCH}
};

use base64::{Engine, engine::general_purpose::STANDARD};
use log::{debug, error, info, warn};
use lopdf::Document;
use reqwest::multipart;
use serde::{Deserialize, Deserializer, Serialize};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use tokio::{fs, time};

use crate::{
    api::{
        File, collect_image_paths_for_files, collect_image_raw_paths_for_files, find_reusable_parsed_file, remove_image_files, resolve_image_storage_path, update_file_custom_image_meta
    }, archive, config, graph::{graph_manager::KnowledgeGraph, llm_extractor::LLMGraphExtractor}, search::{self, SearchEngine, tantivy_engine}
};

#[derive(Debug, Deserialize, Serialize)]
struct Result {
    #[serde(default)]
    content_list: String,
    #[serde(default)]
    images: HashMap<String, String>,
}

// Type aliases for complex SQL / return types
#[allow(clippy::type_complexity)]
type PdfContentDbRow = (i32, Option<String>, Option<String>, Option<i32>, Option<String>, Option<String>);
#[allow(clippy::type_complexity)]
type RawImageJobs = (Vec<(String, String)>, HashMap<String, String>, Vec<String>);

/// MinerU API 返回的结果结构
#[derive(Debug, Deserialize)]
struct MinerUResponse {
    results: MinerUResults,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MinerUResults {
    Map(HashMap<String, Result>),
    List(Vec<MinerUResultItem>),
}

#[derive(Debug, Deserialize)]
struct MinerUResultItem {
    #[serde(default)]
    status: String,
    #[serde(default)]
    error: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    content_list: String,
    #[serde(default)]
    images: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct AnalyzePdfResponse {
    code: i32,
    #[serde(default)]
    message: String,
    data: Option<AnalyzePdfData>,
}

#[derive(Debug, Deserialize)]
struct AnalyzePdfData {
    #[serde(default)]
    content_list: Vec<ContentItem>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AudioTranscriptionResponse {
    #[serde(default)]
    text: String,
    #[serde(default)]
    language: String,
}

#[derive(Debug, Deserialize)]
struct CustomParseResponse {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    message: String,
    data: Option<CustomParseData>,
}

#[derive(Debug, Deserialize)]
struct CustomParseData {
    #[serde(default)]
    slices: Vec<CustomSlice>,
    #[serde(default)]
    full_content: Option<String>,
    #[serde(default)]
    images: Option<HashMap<String, String>>,
    #[serde(default)]
    content_list: Option<Vec<ContentItem>>,
}

#[derive(Debug)]
struct NormalizedCustomParseData {
    slices: Vec<CustomSlice>,
    full_content: Option<String>,
    images: HashMap<String, String>,
    content_list: Option<Vec<ContentItem>>,
    image_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CustomParseReuseRequest<'a> {
    pdf_contents: &'a [PdfContentRow],
}

#[derive(Debug, Deserialize)]
struct CustomSlice {
    content: String,
    #[serde(default, deserialize_with = "deserialize_slice_positions")]
    positions: Vec<SlicePosition>,
}
/*
"type": "text",
"text": "维修服务信息",
"text_level": 1,
"bbox": [95, 198, 635, 265],
"page_idx": 0

"type": "image",
"img_path": "images/a1d809e1c746e15f0ea9510353cdf241cd32a9d00ae7eba19d6f2bc943230297.jpg",
"image_caption": [],
"image_footnote": [],
"bbox": [146, 558, 852, 892],
"page_idx": 0

"type": "table",
"img_path": "images/5471f25a6a8b460b6cfc320a3a1c925fd0b44d9ea3a66011a5521928996f1e39.jpg",
"table_caption": [],
"table_footnote": [],
"table_body": "<table><tr><td rowspan=1 colspan=4>油耗信息显示内容的多少与发动机有关</td></tr><tr><td rowspan=1 colspan=1>序号</td><td rowspan=1 colspan=1>内容</td><td rowspan=1 colspan=1>单位</td><td rowspan=1 colspan=1>描述</td></tr><tr><td rowspan=1 colspan=1>1</td><td rowspan=1 colspan=1>瞬时油耗</td><td rowspan=1 colspan=1>L/H</td><td rowspan=1 colspan=1>以当前的喷油量计算出1小时所消耗的油量</td></tr><tr><td rowspan=1 colspan=1>2</td><td rowspan=1 colspan=1>瞬时百公里油耗</td><td rowspan=1 colspan=1>L</td><td rowspan=1 colspan=1>在行驶时显示，以当前的喷油量计算出百公里所消耗的油量</td></tr><tr><td rowspan=1 colspan=1>3</td><td rowspan=1 colspan=1>百公里平均油耗</td><td rowspan=1 colspan=1>L</td><td rowspan=1 colspan=1>以本次运行过程中的平均燃油消耗量，计算出行驶100公里所消耗的油量</td></tr><tr><td rowspan=1 colspan=1>4</td><td rowspan=1 colspan=1>短途平均油耗</td><td rowspan=1 colspan=1>L</td><td rowspan=1 colspan=1>里程大于10Km/h时才会显示</td></tr><tr><td rowspan=1 colspan=1>5</td><td rowspan=1 colspan=1>小计油耗</td><td rowspan=1 colspan=1>L</td><td rowspan=1 colspan=1>本次发动机运行油耗显示（从启动到熄火)</td></tr><tr><td rowspan=1 colspan=1>6</td><td rowspan=1 colspan=1>总油耗</td><td rowspan=1 colspan=1>L</td><td rowspan=1 colspan=1>从发动机第一次运行开始到现在总计消耗的油量</td></tr><tr><td rowspan=1 colspan=1>7</td><td rowspan=1 colspan=1>发动机工作时间</td><td rowspan=1 colspan=1>H</td><td rowspan=1 colspan=1>从发动机第一次运行开始到现在的工作时间</td></tr><tr><td rowspan=1 colspan=4></td></tr><tr><td></td><td></td><td></td><td></td></tr></table>",
"bbox": [70, 138, 925, 916],
"page_idx": 12

"type": "equation",
"img_path": "images/544f359685e12c8500550eb3e209d1cdc4baa41933d8616b32d33a1df34d4a4e.jpg",
"text": "$$\\nf _ { 1 } ( \\\\mathit { t } ) * f _ { 2 } ( \\\\mathit { t } ) = f _ { 2 } ( \\\\mathit { t } ) * f _ { 1 } ( \\\\mathit { t } )\\n$$",
"text_format": "latex",
"bbox": [333, 755, 634, 777],
"page_idx": 89
*/
#[derive(Debug, Clone, Deserialize, Serialize)]
struct ContentItem {
    #[serde(default, rename = "type")]
    typ: String,
    #[serde(default)]
    bbox: Vec<i32>,
    #[serde(default)]
    page_idx: i32,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    text_level: Option<i32>,
    #[serde(default)]
    text_format: Option<String>, // latex
    #[serde(default)]
    img_path: Option<String>,
    #[serde(default)]
    image_caption: Option<Vec<String>>,
    #[serde(default)]
    table_body: Option<String>,
    #[serde(default)]
    table_caption: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
struct SlicePosition {
    page_idx: i32,
    bbox: [i32; 4],
    #[serde(default)]
    sheet_name: Option<String>,
    #[serde(default)]
    row_num: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct RawSlicePosition {
    page_idx: i32,
    #[serde(default)]
    bbox: Vec<i32>,
    #[serde(default)]
    sheet_name: Option<String>,
    #[serde(default)]
    row_num: Option<i32>,
}

fn deserialize_slice_positions<'de, D>(deserializer: D) -> std::result::Result<Vec<SlicePosition>, D::Error>
where
    D: Deserializer<'de>, {
    let raw_positions: Option<Vec<RawSlicePosition>> = Option::deserialize(deserializer)?;
    let mut positions = Vec::new();
    if let Some(raw_positions) = raw_positions {
        for raw in raw_positions {
            if raw.bbox.len() == 4 {
                positions.push(SlicePosition {
                    page_idx: raw.page_idx,
                    bbox: [raw.bbox[0], raw.bbox[1], raw.bbox[2], raw.bbox[3]],
                    sheet_name: raw.sheet_name,
                    row_num: raw.row_num,
                });
            } else {
                debug!("Dropping custom slice position on page {} due to invalid bbox {:?}", raw.page_idx, raw.bbox);
            }
        }
    }
    Ok(positions)
}

#[derive(Debug, Clone)]
struct SliceWithPositions {
    content: String,
    positions: Vec<SlicePosition>,
}

#[derive(Debug, Clone)]
struct Segment {
    start: usize,
    end: usize,
    positions: Vec<SlicePosition>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
struct PdfContentRow {
    page_idx: i32,
    bbox: Option<String>,
    text: Option<String>,
    text_level: Option<i32>,
    img_path: Option<String>,
    table_body: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct SliceRow {
    id: i64,
    content: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct SlicePositionRecord {
    slice_id: i64,
    page_idx: i32,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    sheet_name: Option<String>,
    row_num: Option<i32>,
}

#[derive(Debug, Clone)]
struct ClonedSlice {
    old_id: i64,
    new_id: i64,
    content: String,
}

/// 文件处理器：定时从数据库读取未处理的文件并处理
pub struct FileProcessor {
    pool: SqlitePool,
    search_engine: search::SearchEngine,
    interval: Duration,
}

static PARSE_PAUSED: AtomicBool = AtomicBool::new(false);
static PARSE_TIMING_RUN_SEQ: AtomicU64 = AtomicU64::new(1);

struct ParseTimingCtx {
    file_id: i64,
    filename: String,
    run_id: String,
    pipeline: &'static str,
    run_started: Instant,
    step_seq: u32,
}

impl ParseTimingCtx {
    fn new(file: &File, pipeline: &'static str) -> Self {
        let seq = PARSE_TIMING_RUN_SEQ.fetch_add(1, Ordering::Relaxed);
        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or_default();
        let run_id = format!("f{}-{}-{}", file.id, now_ms, seq);
        let ctx = Self {
            file_id: file.id,
            filename: file.filename.clone(),
            run_id,
            pipeline,
            run_started: Instant::now(),
            step_seq: 0,
        };
        debug!(
            target: "parse_timing",
            "[parse_timing] event=run_start file_id={} filename={:?} run_id={} pipeline={}",
            ctx.file_id,
            ctx.filename,
            ctx.run_id,
            ctx.pipeline
        );
        ctx
    }

    fn set_pipeline(&mut self, pipeline: &'static str) {
        if self.pipeline == pipeline {
            return;
        }
        self.pipeline = pipeline;
        debug!(
            target: "parse_timing",
            "[parse_timing] event=pipeline_switch file_id={} run_id={} pipeline={}",
            self.file_id,
            self.run_id,
            self.pipeline
        );
    }

    fn step_start(&mut self, step: &'static str) -> (u32, Instant) {
        self.step_seq += 1;
        let seq = self.step_seq;
        debug!(
            target: "parse_timing",
            "[parse_timing] event=step_start file_id={} run_id={} pipeline={} seq={} step={}",
            self.file_id,
            self.run_id,
            self.pipeline,
            seq,
            step
        );
        (seq, Instant::now())
    }

    fn step_ok(&self, step: &'static str, seq: u32, started_at: Instant) {
        debug!(
            target: "parse_timing",
            "[parse_timing] event=step_end file_id={} run_id={} pipeline={} seq={} step={} status=ok duration_ms={}",
            self.file_id,
            self.run_id,
            self.pipeline,
            seq,
            step,
            started_at.elapsed().as_millis()
        );
    }

    fn step_err(&self, step: &'static str, seq: u32, started_at: Instant, err: &anyhow::Error) {
        let err_detail = format!("{:#}", err);
        debug!(
            target: "parse_timing",
            "[parse_timing] event=step_end file_id={} run_id={} pipeline={} seq={} step={} status=err duration_ms={} err={:?}",
            self.file_id,
            self.run_id,
            self.pipeline,
            seq,
            step,
            started_at.elapsed().as_millis(),
            err_detail
        );
    }

    fn finish(&self, result: &anyhow::Result<()>) {
        match result {
            Ok(_) => {
                debug!(
                    target: "parse_timing",
                    "[parse_timing] event=run_end file_id={} filename={:?} run_id={} pipeline={} status=ok total_duration_ms={} steps={}",
                    self.file_id,
                    self.filename,
                    self.run_id,
                    self.pipeline,
                    self.run_started.elapsed().as_millis(),
                    self.step_seq
                );
            }
            Err(err) => {
                let err_detail = format!("{:#}", err);
                debug!(
                    target: "parse_timing",
                    "[parse_timing] event=run_end file_id={} filename={:?} run_id={} pipeline={} status=err total_duration_ms={} steps={} err={:?}",
                    self.file_id,
                    self.filename,
                    self.run_id,
                    self.pipeline,
                    self.run_started.elapsed().as_millis(),
                    self.step_seq,
                    err_detail
                );
            }
        }
    }

    async fn step<T, Fut>(&mut self, step: &'static str, fut: Fut) -> anyhow::Result<T>
    where
        Fut: Future<Output=anyhow::Result<T>>, {
        let (seq, started_at) = self.step_start(step);
        match fut.await {
            Ok(value) => {
                self.step_ok(step, seq, started_at);
                Ok(value)
            }
            Err(err) => {
                self.step_err(step, seq, started_at, &err);
                Err(err)
            }
        }
    }
}

async fn timed_step_opt<T, Fut>(
    timing: Option<&mut ParseTimingCtx>, step: &'static str, fut: Fut,
) -> anyhow::Result<T>
where
    Fut: Future<Output=anyhow::Result<T>>, {
    match timing {
        Some(ctx) => ctx.step(step, fut).await,
        None => fut.await,
    }
}

fn summarize_http_body(body_bytes: &[u8]) -> String {
    if body_bytes.is_empty() {
        return "<empty response body>".to_string();
    }

    let raw = String::from_utf8_lossy(body_bytes).trim().to_string();
    if raw.is_empty() {
        return "<blank response body>".to_string();
    }

    const MAX_CHARS: usize = 800;
    if raw.chars().count() > MAX_CHARS {
        let preview: String = raw.chars().take(MAX_CHARS).collect();
        format!("{}...(truncated, {} bytes)", preview, body_bytes.len())
    } else {
        raw
    }
}

pub fn set_parse_paused(paused: bool) {
    PARSE_PAUSED.store(paused, Ordering::SeqCst);
    if paused {
        warn!("File parsing has been paused for index maintenance");
    } else {
        info!("File parsing resumed");
    }
}

pub fn is_parse_paused() -> bool {
    PARSE_PAUSED.load(Ordering::SeqCst)
}

impl FileProcessor {
    /// 创建新的文件处理器
    ///
    /// # Arguments
    /// * `pool` - 数据库连接池
    /// * `search_engine` - 搜索引擎实例
    /// * `interval_secs` - 处理间隔（秒）
    pub fn new(pool: SqlitePool, search_engine: search::SearchEngine, interval_secs: u64) -> Self {
        Self { pool, search_engine, interval: Duration::from_secs(interval_secs) }
    }

    fn services_http_client(&self) -> anyhow::Result<reqwest::Client> {
        let timeout = Duration::from_secs(config::get().services.request_timeout_secs);
        Ok(reqwest::Client::builder().timeout(timeout).build()?)
    }

    /// 启动后台处理任务
    pub fn start(self) {
        let processor = Arc::new(self);
        tokio::spawn(async move {
            info!("File processor started with interval: {:?}", processor.interval);

            if let Err(e) = processor.reset_processing_files().await {
                error!("Failed to reset in-progress files: {}", e);
            }

            loop {
                if is_parse_paused() {
                    debug!("File processor paused, sleeping for {:?}", processor.interval);
                    time::sleep(processor.interval).await;
                    continue;
                }
                // 持续处理直到没有待处理的文件
                loop {
                    if is_parse_paused() {
                        debug!("File processor paused while processing queue");
                        break;
                    }
                    match processor.process_pending_files().await {
                        Ok(has_more) => {
                            if !has_more {
                                // 没有更多文件需要处理，退出内循环
                                debug!("No more pending files, waiting for next check");
                                break;
                            }
                            // 有更多文件，继续处理
                            debug!("More files pending, continuing processing");
                        }
                        Err(e) => {
                            error!("Error processing files: {}", e);
                            break;
                        }
                    }
                }

                // 等待指定时间后再次检查
                time::sleep(processor.interval).await;
            }
        });
    }

    /// 重置异常退出时处于“处理中”的文件状态
    async fn reset_processing_files(&self) -> anyhow::Result<()> {
        let file_ids: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM files WHERE status = 2").fetch_all(&self.pool).await?;

        if file_ids.is_empty() {
            return Ok(());
        }

        info!("Found {} in-progress files from previous run, resetting", file_ids.len());

        for file_id in file_ids {
            if let Err(e) = self.reset_processing_file_data(file_id).await {
                error!("Failed to reset processing file {}: {}", file_id, e);
            }
        }

        Ok(())
    }

    async fn reset_processing_file_data(&self, file_id: i64) -> anyhow::Result<()> {
        let image_paths = collect_image_paths_for_files(&self.pool, std::slice::from_ref(&file_id)).await?;
        let mut tx = self.pool.begin().await?;

        self.delete_processing_file_data(&mut tx, file_id).await?;
        sqlx::query("UPDATE files SET status = 0, log = '', updated_at = strftime('%s','now') WHERE id = ?")
            .bind(file_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        remove_image_files(image_paths).await;

        if let Err(e) = self.search_engine.delete(Some(file_id), None).await {
            warn!("Failed to delete search index for file {}: {}", file_id, e);
        }

        Ok(())
    }

    async fn cleanup_processing_file_data(&self, file_id: i64) -> anyhow::Result<()> {
        let image_paths = collect_image_paths_for_files(&self.pool, std::slice::from_ref(&file_id)).await?;
        let mut tx = self.pool.begin().await?;
        self.delete_processing_file_data(&mut tx, file_id).await?;
        tx.commit().await?;

        remove_image_files(image_paths).await;

        if let Err(e) = self.search_engine.delete(Some(file_id), None).await {
            warn!("Failed to delete search index for file {}: {}", file_id, e);
        }

        Ok(())
    }

    async fn cleanup_processing_file_data_with_retry(&self, file_id: i64, max_attempts: usize) -> anyhow::Result<()> {
        let max_attempts = max_attempts.max(1);
        let mut attempt = 1usize;

        loop {
            match self.cleanup_processing_file_data(file_id).await {
                Ok(()) => return Ok(()),
                Err(err) => {
                    if attempt >= max_attempts || !Self::is_pool_timeout_error(&err) {
                        return Err(err);
                    }

                    let retry_in_ms = (attempt as u64) * 200;
                    warn!(
                        "Cleanup for file {} failed due to DB pool timeout (attempt {}/{}), retrying in {}ms",
                        file_id, attempt, max_attempts, retry_in_ms
                    );
                    time::sleep(Duration::from_millis(retry_in_ms)).await;
                    attempt += 1;
                }
            }
        }
    }

    fn is_pool_timeout_error(err: &anyhow::Error) -> bool {
        err.chain().any(|cause| {
            cause.downcast_ref::<sqlx::Error>().is_some_and(|sqlx_err| matches!(sqlx_err, sqlx::Error::PoolTimedOut))
        })
    }

    async fn delete_processing_file_data(
        &self, tx: &mut sqlx::Transaction<'_, Sqlite>, file_id: i64,
    ) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM entity_mentions WHERE slice_id IN (SELECT id FROM slices WHERE file_id = ?)")
            .bind(file_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM slice_positions WHERE slice_id IN (SELECT id FROM slices WHERE file_id = ?)")
            .bind(file_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM slices WHERE file_id = ?").bind(file_id).execute(&mut **tx).await?;
        sqlx::query("DELETE FROM pdf_contents WHERE file_id = ?").bind(file_id).execute(&mut **tx).await?;
        Ok(())
    }

    /// 处理所有待处理的文件
    /// 返回是否还有更多文件需要处理
    async fn process_pending_files(self: &Arc<Self>) -> anyhow::Result<bool> {
        if is_parse_paused() {
            return Ok(false);
        }

        let cfg = config::get();
        let configured_concurrency = cfg.server.process_concurrency.max(1);
        let db_safe_concurrency = (cfg.database.max_connections as usize).saturating_sub(2).max(1);
        let concurrency = configured_concurrency.min(db_safe_concurrency);
        if concurrency < configured_concurrency {
            warn!(
                "Reducing file processing concurrency from {} to {} to avoid DB pool exhaustion (max_connections={})",
                configured_concurrency, concurrency, cfg.database.max_connections
            );
        }
        info!("Processing pending queue with dynamic claiming (concurrency: {})", concurrency);

        let mut handles = Vec::with_capacity(concurrency);
        for worker_idx in 0..concurrency {
            let processor = Arc::clone(self);
            handles.push(tokio::spawn(async move { processor.run_pending_worker(worker_idx).await }));
        }

        let mut claimed_total = 0usize;
        for handle in handles {
            let worker_claimed = handle.await.map_err(|e| anyhow::anyhow!("pending worker join failed: {}", e))??;
            claimed_total += worker_claimed;
        }

        Ok(claimed_total > 0)
    }

    async fn run_pending_worker(self: Arc<Self>, worker_idx: usize) -> anyhow::Result<usize> {
        let mut claimed = 0usize;

        loop {
            if is_parse_paused() {
                break;
            }

            let Some(file) = self.claim_next_pending_file().await? else {
                break;
            };
            claimed += 1;

            info!("Pending worker {} claimed file {} (kb_id={:?})", worker_idx, file.id, file.kb_id);

            if let Err(e) = self.process_file_claimed(&file).await {
                error!("Failed to process file {}: {}", file.id, e);
                if let Err(cleanup_err) = self.cleanup_processing_file_data_with_retry(file.id, 3).await {
                    error!("Failed to cleanup processing data for file {}: {}", file.id, cleanup_err);
                }
                self.mark_file_failed(file.id, &e.to_string()).await?;
            } else {
                info!("Successfully processed file {}", file.id);
            }
        }

        Ok(claimed)
    }

    /// 原子领取一个待处理文件，按知识库解析优先级排序
    async fn claim_next_pending_file(&self) -> anyhow::Result<Option<File>> {
        let file = sqlx::query_as::<_, File>(
            "UPDATE files
             SET status = 2, updated_at = strftime('%s','now')
             WHERE id = (
                 SELECT id
                 FROM files
                 WHERE status = 0
                 ORDER BY parse_priority DESC, created_at ASC, id ASC
                 LIMIT 1
             )
             AND status = 0
             RETURNING *",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(file)
    }

    async fn try_reuse_existing_data(&self, file: &File) -> anyhow::Result<bool> {
        if !config::get().server.reuse_duplicate_files {
            return Ok(false);
        }
        let Some(source_file) =
            find_reusable_parsed_file(&self.pool, &file.hash, &file.slice_type, Some(file.id)).await?
        else {
            return Ok(false);
        };

        info!("Found reusable parsed file {} for new file {} (hash={})", source_file.id, file.id, file.hash);

        match self.clone_file_data(&source_file, file).await {
            Ok(_) => Ok(true),
            Err(err) => {
                error!("Failed to reuse parsed data from file {} for file {}: {}", source_file.id, file.id, err);
                if let Err(clean_err) = self.cleanup_processing_file_data_with_retry(file.id, 3).await {
                    warn!("Failed to cleanup file {} after reuse error: {}", file.id, clean_err);
                }
                sqlx::query("UPDATE files SET status = 0, log = ?, updated_at = strftime('%s','now') WHERE id = ?")
                    .bind(format!("Reuse failed: {}", err))
                    .bind(file.id)
                    .execute(&self.pool)
                    .await?;
                Ok(false)
            }
        }
    }

    /// 处理单个文件
    async fn process_file(&self, file: &File) -> anyhow::Result<()> {
        self.process_file_inner(file, false, false).await
    }

    async fn process_file_claimed(&self, file: &File) -> anyhow::Result<()> {
        self.process_file_inner(file, true, false).await
    }

    async fn process_file_skip_reuse(&self, file: &File) -> anyhow::Result<()> {
        self.process_file_inner(file, false, true).await
    }

    async fn process_file_inner(&self, file: &File, already_claimed: bool, skip_reuse: bool) -> anyhow::Result<()> {
        let mut timing = ParseTimingCtx::new(file, "dispatch");
        let result = async {
            if is_parse_paused() {
                anyhow::bail!("parse is paused for index maintenance");
            }
            info!("Processing file: {} ({})", file.filename, file.id);
            if !self.ensure_file_exists(file.id, "process start").await? {
                return Ok(());
            }

            let current_status: Option<i32> = timing
                .step("status_check", async {
                    Ok(sqlx::query_scalar("SELECT status FROM files WHERE id = ?")
                        .bind(file.id)
                        .fetch_optional(&self.pool)
                        .await?)
                })
                .await?;
            if let Some(status) = current_status {
                if !already_claimed && status != 0 {
                    info!("File {} status changed to {}, skipping processing", file.id, status);
                    return Ok(());
                }
                if already_claimed && status != 2 {
                    info!("Claimed file {} status changed to {}, skipping processing", file.id, status);
                    return Ok(());
                }
            }

            let is_storage = timing.step("storage_kb_check", self.is_storage_kb(file.kb_id)).await?;
            if is_storage {
                info!("Skipping parsing for storage knowledge base file {}", file.id);
                timing.step("mark_storage_skipped", self.mark_file_storage_skipped(file.id)).await?;
                return Ok(());
            }

            if !skip_reuse
                && timing.step("reuse_check", self.try_reuse_existing_data(file)).await? {
                    timing.set_pipeline("reuse");
                    info!("File {} reused existing parsed data, skipping processing pipeline", file.id);
                    return Ok(());
                }

            timing
                .step("set_processing_status", async {
                    let sql = "UPDATE files SET status = 2, updated_at = strftime('%s','now') WHERE id = ?";
                    sqlx::query(sql).bind(file.id).execute(&self.pool).await?;
                    Ok(())
                })
                .await?;

            // 检查文件是否为 PDF 或图片
            let filename_lower = file.filename.to_lowercase();
            let is_pdf = filename_lower.ends_with(".pdf");
            let is_image = Self::is_image_file(&filename_lower);
            let is_audio = Self::is_audio_file(&filename_lower);

            // 检查文件是否为 Word 或 Excel 文档
            let is_word = filename_lower.ends_with(".doc") || filename_lower.ends_with(".docx");
            let is_excel = filename_lower.ends_with(".xls") || filename_lower.ends_with(".xlsx");

            let cfg = config::get();
            let custom_url = cfg.services.custom_parse_url.as_deref();
            let custom_reuse_url = cfg.services.custom_parse_reuse_url.as_deref();

            if let Some(reuse_url) = custom_reuse_url {
                info!("Custom parse reuse enabled");
                match self.process_file_with_custom_reuse_parser(file, reuse_url, Some(&mut timing)).await {
                    Ok(true) => {
                        timing.set_pipeline("custom_reuse");
                        info!(
                            "Custom parse reuse enabled, reused existing pdf_contents for file {} via {}",
                            file.id, reuse_url
                        );
                        return Ok(());
                    }
                    Ok(false) => {}
                    Err(err) => {
                        error!("Custom parse reuse failed for file {}: {}", file.id, err);
                    }
                }
            }

            if is_excel {
                timing.set_pipeline("excel");
                if !self.ensure_file_exists(file.id, "before excel processing").await? {
                    return Ok(());
                }
                info!("Detected Excel document, parsing directly: {}", file.filename);
                self.process_excel_file(file, Some(&mut timing)).await?;
                return Ok(());
            }

            if is_word {
                timing.set_pipeline("office_pdf");
                if !self.ensure_file_exists(file.id, "before office conversion").await? {
                    return Ok(());
                }
                info!("Detected Word document, converting to PDF: {}", file.filename);

                if let Some(custom_url) = custom_url {
                    let stored_pdf_path =
                        timing.step("convert_office_to_pdf", self.convert_office_to_pdf(file)).await?;
                    let mut temp_file = file.clone();
                    temp_file.path = stored_pdf_path.to_string_lossy().to_string();
                    temp_file.filename = format!("{}.pdf", file.id);

                    timing.set_pipeline("custom_parser");
                    info!("Custom parse enabled, routing converted PDF for file {} to {}", file.id, custom_url);
                    self.process_file_with_custom_parser(&temp_file, custom_url, Some(&mut timing)).await?;
                    return Ok(());
                }

                self.convert_office_to_pdf_and_process(file, Some(&mut timing)).await?;
                return Ok(());
            }

            // 压缩文件：不解析，直接标记为跳过
            if archive::is_archive_file(&filename_lower) {
                timing.set_pipeline("archive");
                info!("Detected archive file, skipping parsing: {}", file.filename);
                timing
                    .step("mark_archive_skipped", async {
                        let sql =
                            "UPDATE files SET status = 3, log = ?, updated_at = strftime('%s','now') WHERE id = ?";
                        sqlx::query(sql).bind("Archive file: not parsed").bind(file.id).execute(&self.pool).await?;
                        Ok(())
                    })
                    .await?;
                return Ok(());
            }

            if let Some(custom_url) = custom_url
                && is_pdf {
                    timing.set_pipeline("custom_parser");
                    info!("Custom parse enabled, routing file {} to {}", file.id, custom_url);
                    self.process_file_with_custom_parser(file, custom_url, Some(&mut timing)).await?;
                    return Ok(());
                }

            if is_pdf {
                timing.set_pipeline("pdf");
                if !self.ensure_file_exists(file.id, "before pdf processing").await? {
                    return Ok(());
                }
                // 处理 PDF 或图片文件
                self.process_pdf_file(file, None, false, None, Some(&mut timing)).await?;
            } else if is_image {
                timing.set_pipeline("image");
                if !self.ensure_file_exists(file.id, "before image embedding").await? {
                    return Ok(());
                }
                let image_embedding = timing
                    .step(
                        "get_image_embedding",
                        search::embedding::get_image_embedding_from_path(&file.path, Some(&file.filename)),
                    )
                    .await?;
                self.process_pdf_file(file, Some(image_embedding), true, None, Some(&mut timing)).await?;
            } else if is_audio {
                timing.set_pipeline("audio");
                if !self.ensure_file_exists(file.id, "before audio processing").await? {
                    return Ok(());
                }
                self.process_audio_file(file, Some(&mut timing)).await?;
            } else {
                timing.set_pipeline("text");
                if !self.ensure_file_exists(file.id, "before text processing").await? {
                    return Ok(());
                }
                // 处理普通文本文件
                self.process_text_file(file, Some(&mut timing)).await?;
            }

            Ok(())
        }
        .await;
        timing.finish(&result);
        result
    }

    async fn process_file_with_custom_parser(
        &self, file: &File, custom_url: &str, mut timing: Option<&mut ParseTimingCtx>,
    ) -> anyhow::Result<()> {
        if !self.ensure_file_exists(file.id, "custom parse start").await? {
            return Ok(());
        }

        let parse_data =
            timed_step_opt(timing.as_deref_mut(), "custom_parse_api", self.call_custom_parse_api(file, custom_url))
                .await?;
        let normalized = Self::normalize_custom_parse_data(file.id, parse_data)?;

        if let Some(content_list) = normalized.content_list.as_ref() {
            self.insert_custom_pdf_contents(file.id, content_list, timing.as_deref_mut()).await?;
        } else {
            timed_step_opt(timing.as_deref_mut(), "custom_update_image_meta", async {
                update_file_custom_image_meta(&self.pool, file.id, &normalized.image_paths, "custom_parse_images")
                    .await?;
                Ok(())
            })
            .await?;
        }

        if !normalized.images.is_empty() {
            self.save_custom_images(&normalized.images, timing.as_deref_mut()).await?;
        }

        self.save_custom_slices(file, &normalized.slices, normalized.full_content.as_deref(), timing)
            .await?;

        Ok(())
    }

    async fn process_file_with_custom_reuse_parser(
        &self, file: &File, custom_reuse_url: &str, mut timing: Option<&mut ParseTimingCtx>,
    ) -> anyhow::Result<bool> {
        let pdf_rows =
            timed_step_opt(timing.as_deref_mut(), "fetch_pdf_content_rows", self.fetch_pdf_content_rows(file.id))
                .await?;
        if pdf_rows.is_empty() {
            debug!("Custom parse reuse skipped for file {} because pdf_contents are empty", file.id);
            return Ok(false);
        }

        let payload = CustomParseReuseRequest { pdf_contents: &pdf_rows };
        let client = self.services_http_client()?;
        let response = timed_step_opt(timing.as_deref_mut(), "custom_parse_reuse_api", async {
            Ok(client.post(custom_reuse_url).json(&payload).send().await?)
        })
        .await?;
        let response_url = response.url().to_string();
        let status = response.status();
        let request_id =
            response.headers().get("x-request-id").and_then(|v| v.to_str().ok()).unwrap_or("-").to_string();
        let body_bytes = response.bytes().await?;
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "Custom parse reuse API failed (status={}, url={}, request_id={}, body_len={}): {}",
                status.as_u16(),
                response_url,
                request_id,
                body_bytes.len(),
                summarize_http_body(&body_bytes)
            ));
        }

        let parsed: CustomParseResponse = serde_json::from_slice(&body_bytes).map_err(|e| {
            anyhow::anyhow!(
                "Custom parse reuse API response decode failed (status={}, url={}, body_len={}): {} - {}",
                status.as_u16(),
                response_url,
                body_bytes.len(),
                e,
                summarize_http_body(&body_bytes)
            )
        })?;

        if parsed.code != 200 {
            let msg = if parsed.message.is_empty() { "<empty message>" } else { parsed.message.as_str() };
            return Err(anyhow::anyhow!(
                "Custom parse reuse API returned error code={} message={} url={}",
                parsed.code,
                msg,
                response_url
            ));
        }

        let data = parsed.data.ok_or_else(|| anyhow::anyhow!("Custom parse reuse API returned empty data"))?;
        if data.slices.is_empty() {
            return Err(anyhow::anyhow!("Custom parse reuse API returned empty slices"));
        }

        self.save_custom_slices(file, &data.slices, data.full_content.as_deref(), timing).await?;

        Ok(true)
    }

    fn normalize_custom_parse_data(file_id: i64, data: CustomParseData) -> anyhow::Result<NormalizedCustomParseData> {
        let mut image_mapping: HashMap<String, String> = HashMap::new();
        let prefix = format!("f{}_", file_id);
        let images = data.images.unwrap_or_default();

        for image_name in images.keys() {
            Self::ensure_safe_image_name(image_name)?;
            image_mapping.entry(image_name.clone()).or_insert_with(|| Self::prefix_image_path(image_name, &prefix));
        }

        if let Some(content_list) = data.content_list.as_ref() {
            for item in content_list {
                if let Some(img_path) = item.img_path.as_deref() {
                    Self::ensure_safe_image_name(img_path)?;
                    let mapped_path = image_mapping.get(img_path).cloned().or_else(|| {
                        Self::basename_for_path(img_path).and_then(|basename| image_mapping.get(&basename).cloned())
                    });
                    let mapped_path = mapped_path.unwrap_or_else(|| Self::prefix_image_path(img_path, &prefix));
                    image_mapping.entry(img_path.to_string()).or_insert(mapped_path);
                }
            }
        }

        let image_mapping = Self::expand_image_mapping_aliases(image_mapping);
        let mut referenced_image_paths = Vec::new();
        for slice in &data.slices {
            for image_path in Self::extract_custom_image_refs(&slice.content) {
                if Self::lookup_image_mapping(&image_mapping, &image_path).is_none()
                    && resolve_image_storage_path(&image_path).is_some()
                    && !referenced_image_paths.iter().any(|existing| existing == &image_path)
                {
                    referenced_image_paths.push(image_path);
                }
            }
        }
        if let Some(full_content) = data.full_content.as_deref() {
            for image_path in Self::extract_custom_image_refs(full_content) {
                if Self::lookup_image_mapping(&image_mapping, &image_path).is_none()
                    && resolve_image_storage_path(&image_path).is_some()
                    && !referenced_image_paths.iter().any(|existing| existing == &image_path)
                {
                    referenced_image_paths.push(image_path);
                }
            }
        }

        let mut normalized_images = HashMap::new();
        for (image_name, image_base64) in images {
            let new_name = image_mapping
                .get(&image_name)
                .cloned()
                .or_else(|| Self::basename_for_path(&image_name).and_then(|name| image_mapping.get(&name).cloned()))
                .unwrap_or_else(|| Self::prefix_image_path(&image_name, &prefix));
            normalized_images.insert(new_name, image_base64);
        }

        let mut content_list = data.content_list;
        if let Some(items) = content_list.as_mut() {
            for item in items {
                if let Some(img_path) = item.img_path.as_deref()
                    && let Some(new_path) = Self::lookup_image_mapping(&image_mapping, img_path) {
                        item.img_path = Some(new_path);
                    }
            }
        }

        let slices = data
            .slices
            .into_iter()
            .map(|mut slice| {
                slice.content = Self::rewrite_custom_image_refs(&slice.content, &image_mapping);
                slice
            })
            .collect();
        let full_content = data.full_content.map(|content| Self::rewrite_custom_image_refs(&content, &image_mapping));

        let mut image_paths = Vec::new();
        let mut seen = HashSet::new();
        for new_path in image_mapping.values() {
            let trimmed = new_path.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                image_paths.push(trimmed.to_string());
            }
        }
        for path in referenced_image_paths {
            let trimmed = path.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                image_paths.push(trimmed.to_string());
            }
        }

        Ok(NormalizedCustomParseData { slices, full_content, images: normalized_images, content_list, image_paths })
    }

    fn ensure_safe_image_name(image_name: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            resolve_image_storage_path(image_name).is_some(),
            "Custom parse returned unsafe image path: {}",
            image_name
        );
        Ok(())
    }

    fn lookup_image_mapping(mapping: &HashMap<String, String>, image_path: &str) -> Option<String> {
        mapping
            .get(image_path)
            .cloned()
            .or_else(|| Self::basename_for_path(image_path).and_then(|basename| mapping.get(&basename).cloned()))
    }

    fn basename_for_path(path: &str) -> Option<String> {
        std::path::Path::new(path).file_name().and_then(|name| name.to_str()).map(|name| name.to_string())
    }

    fn expand_image_mapping_aliases(mut mapping: HashMap<String, String>) -> HashMap<String, String> {
        let aliases: Vec<(String, String)> = mapping
            .iter()
            .filter_map(|(old_path, new_path)| {
                let basename = Self::basename_for_path(old_path)?;
                if basename == *old_path { None } else { Some((basename, new_path.clone())) }
            })
            .collect();
        for (alias, new_path) in aliases {
            mapping.entry(alias).or_insert(new_path);
        }
        mapping
    }

    fn extract_custom_image_refs(content: &str) -> Vec<String> {
        let api_re = regex::Regex::new(r"/api/v1/knowledge/files/([^\s'\)>]+)").expect("valid image reference regex");
        let markdown_re = regex::Regex::new(r"!\[[^\]]*\]\(([^)]+)\)").expect("valid markdown image regex");
        let mut refs = Vec::new();
        for cap in api_re.captures_iter(content) {
            if let Some(path) = cap.get(1).map(|m| m.as_str().trim()).filter(|path| !path.is_empty())
                && !refs.iter().any(|existing| existing == path) {
                    refs.push(path.to_string());
                }
        }
        for cap in markdown_re.captures_iter(content) {
            let Some(path) = cap.get(1).map(|m| m.as_str().trim()).filter(|path| !path.is_empty()) else {
                continue;
            };
            if path.starts_with("http://") || path.starts_with("https://") || path.starts_with("data:") {
                continue;
            }
            let normalized = path.strip_prefix("/api/v1/knowledge/files/").unwrap_or(path).trim();
            if !normalized.is_empty() && !refs.iter().any(|existing| existing == normalized) {
                refs.push(normalized.to_string());
            }
        }
        refs
    }

    fn rewrite_custom_image_refs(content: &str, mapping: &HashMap<String, String>) -> String {
        if mapping.is_empty() || content.is_empty() {
            return content.to_string();
        }

        let mut rewritten = content.to_string();
        let mut pairs: Vec<(&String, &String)> = mapping.iter().collect();
        pairs.sort_by_key(|(right, _)| std::cmp::Reverse(right.len()));
        for (old_path, new_path) in pairs {
            rewritten = rewritten.replace(
                &format!("/api/v1/knowledge/files/{}", old_path),
                &format!("/api/v1/knowledge/files/{}", new_path),
            );
            rewritten = rewritten.replace(&format!("]({})", old_path), &format!("]({})", new_path));
        }
        rewritten
    }

    async fn call_custom_parse_api(&self, file: &File, custom_url: &str) -> anyhow::Result<CustomParseData> {
        let file_bytes = tokio::fs::read(&file.path).await?;
        let mime_type = mime_guess::from_path(&file.filename).first_or_octet_stream().essence_str().to_string();
        let form = multipart::Form::new()
            .part("file", multipart::Part::bytes(file_bytes).file_name(file.filename.clone()).mime_str(&mime_type)?);

        let client = self.services_http_client()?;
        let response = client.post(custom_url).multipart(form).send().await?;
        let response_url = response.url().to_string();
        let status = response.status();
        let request_id =
            response.headers().get("x-request-id").and_then(|v| v.to_str().ok()).unwrap_or("-").to_string();
        let body_bytes = response.bytes().await?;
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "Custom parse API failed (status={}, url={}, request_id={}, body_len={}): {}",
                status.as_u16(),
                response_url,
                request_id,
                body_bytes.len(),
                summarize_http_body(&body_bytes)
            ));
        }

        let parsed: CustomParseResponse = serde_json::from_slice(&body_bytes).map_err(|e| {
            anyhow::anyhow!(
                "Custom parse API response decode failed (status={}, url={}, body_len={}): {} - {}",
                status.as_u16(),
                response_url,
                body_bytes.len(),
                e,
                summarize_http_body(&body_bytes)
            )
        })?;

        if parsed.code != 200 {
            let msg = if parsed.message.is_empty() { "<empty message>" } else { parsed.message.as_str() };
            return Err(anyhow::anyhow!(
                "Custom parse API returned error code={} message={} url={}",
                parsed.code,
                msg,
                response_url
            ));
        }

        let data = parsed.data.ok_or_else(|| anyhow::anyhow!("Custom parse API returned empty data"))?;
        if data.slices.is_empty() {
            return Err(anyhow::anyhow!("Custom parse API returned empty slices"));
        }

        Ok(data)
    }

    async fn save_custom_slices(
        &self, file: &File, slices: &[CustomSlice], full_content: Option<&str>, mut timing: Option<&mut ParseTimingCtx>,
    ) -> anyhow::Result<()> {
        if !self.ensure_file_exists(file.id, "before writing custom slices").await? {
            return Ok(());
        }

        let derived_full_content = match full_content {
            Some(content) if !content.trim().is_empty() => content.to_string(),
            _ => {
                let mut combined = String::new();
                for (idx, slice) in slices.iter().enumerate() {
                    if idx > 0 {
                        combined.push_str("\n\n");
                    }
                    combined.push_str(&slice.content);
                }
                combined
            }
        };

        let search_docs = timed_step_opt(timing.as_deref_mut(), "custom_insert_slices", async {
            let owned_slices: Vec<SliceWithPositions> = slices
                .iter()
                .map(|slice| SliceWithPositions { content: slice.content.clone(), positions: slice.positions.clone() })
                .collect();
            let persisted = self.insert_slices_and_positions(file.id, owned_slices).await?;
            let mut search_docs = Vec::with_capacity(persisted.len());
            for (id, content) in persisted {
                search_docs.push(tantivy_engine::Document::new(id, file.id, file.kb_id, content));
            }
            Ok(search_docs)
        })
        .await?;

        if !search_docs.is_empty() {
            timed_step_opt(timing.as_deref_mut(), "custom_write_search_batch", async {
                let embeddings = vec![None; search_docs.len()];
                self.search_engine.write_batch(search_docs, embeddings).await?;
                Ok(())
            })
            .await?;
        }

        if !self.ensure_file_exists(file.id, "before writing custom full index").await? {
            return Ok(());
        }
        timed_step_opt(timing.as_deref_mut(), "custom_write_full_index", async {
            let index_full_content = format!("{}\n\n{}", file.filename, derived_full_content);
            self.search_engine
                .write_full(tantivy_engine::Document::new(file.id, file.id, file.kb_id, index_full_content))
                .await?;
            Ok(())
        })
        .await?;

        if !self.ensure_file_exists(file.id, "before updating custom status").await? {
            return Ok(());
        }
        timed_step_opt(timing.as_deref_mut(), "custom_finalize_file_status", async {
            let sql =
                "UPDATE files SET status = 1, content = ?, log = ?, updated_at = strftime('%s','now') WHERE id = ?";
            sqlx::query(sql)
                .bind(&derived_full_content)
                .bind("Custom parse processed successfully")
                .bind(file.id)
                .execute(&self.pool)
                .await?;
            Ok(())
        })
        .await?;

        info!("File {} processed successfully with {} custom slices", file.id, slices.len());

        self.search_engine.reload_readers()?;

        timed_step_opt(timing, "build_knowledge_graph", async {
            self.maybe_build_knowledge_graph(file).await;
            Ok(())
        })
        .await?;

        Ok(())
    }

    async fn insert_custom_pdf_contents(
        &self, file_id: i64, content_list: &[ContentItem], timing: Option<&mut ParseTimingCtx>,
    ) -> anyhow::Result<()> {
        let valid_content_items: Vec<ContentItem> =
            content_list.iter().filter(|item| item.typ != "discarded").cloned().collect();
        timed_step_opt(timing, "custom_write_pdf_contents", async {
            sqlx::query("DELETE FROM pdf_contents WHERE file_id = ?").bind(file_id).execute(&self.pool).await?;
            if valid_content_items.is_empty() {
                return Ok(());
            }

            let binds_per_row = 7_usize;
            let max_vars = 999_usize;
            let batch_size = std::cmp::max(1, max_vars / binds_per_row);
            for chunk in valid_content_items.chunks(batch_size) {
                let mut pdf_sql = QueryBuilder::<Sqlite>::new(
                    "insert into pdf_contents(file_id, page_idx, bbox, text, text_level, img_path, table_body) ",
                );
                pdf_sql.push_values(chunk.iter(), |mut b, item| {
                    let bbox = if item.bbox.is_empty() {
                        None
                    } else {
                        Some(serde_json::to_string(&item.bbox).unwrap_or_default())
                    };
                    b.push_bind(file_id)
                        .push_bind(item.page_idx)
                        .push_bind(bbox)
                        .push_bind(&item.text)
                        .push_bind(item.text_level)
                        .push_bind(&item.img_path)
                        .push_bind(&item.table_body);
                });
                pdf_sql.build().execute(&self.pool).await?;
            }
            Ok(())
        })
        .await
    }

    async fn save_custom_images(
        &self, images: &HashMap<String, String>, timing: Option<&mut ParseTimingCtx>,
    ) -> anyhow::Result<()> {
        timed_step_opt(timing, "custom_save_images", async {
            let cfg = config::get();
            fs::create_dir_all(&cfg.storage.images_path).await?;
            info!("custom image count: {}", images.len());
            for (img_name, img_base64) in images {
                let payload = match img_base64.find("base64,") {
                    Some(idx) => &img_base64[idx + "base64,".len()..],
                    None => img_base64.as_str(),
                };
                let preview: String = payload.chars().take(32).collect();
                debug!("Decoding custom image {} (len={}, preview=\"{}\")", img_name, payload.len(), preview);
                let bytes = STANDARD.decode(payload).map_err(|err| {
                    error!(
                        "Failed to decode custom image {} (len={}, preview=\"{}\"): {}",
                        img_name,
                        payload.len(),
                        preview,
                        err
                    );
                    anyhow::anyhow!(err)
                })?;
                let Some(image_path) = resolve_image_storage_path(img_name) else {
                    anyhow::bail!("Custom parse returned unsafe image path: {}", img_name);
                };
                if let Some(parent) = std::path::Path::new(&image_path).parent() {
                    fs::create_dir_all(parent).await?;
                }
                fs::write(image_path, bytes).await?;
            }
            Ok(())
        })
        .await
    }

    /// 调用外部服务将 Word/Excel 文档转换为 PDF
    async fn convert_office_to_pdf(&self, file: &File) -> anyhow::Result<std::path::PathBuf> {
        let cfg = config::get();
        let pdf_dir = std::path::Path::new(&cfg.storage.pdf_path);
        fs::create_dir_all(pdf_dir).await?;

        let pdf_filename = format!("{}.pdf", file.id);
        let stored_pdf_path = pdf_dir.join(&pdf_filename);
        let temp_pdf_path = pdf_dir.join(format!(".{}.pdf.tmp", file.id));
        let file_bytes = tokio::fs::read(&file.path).await?;
        let mime_type = mime_guess::from_path(&file.filename).first_or_octet_stream().essence_str().to_string();
        let mut convert_url = reqwest::Url::parse(&cfg.services.office_convert_url)?;
        if !convert_url.query_pairs().any(|(key, _)| key == "target_format") {
            convert_url.query_pairs_mut().append_pair("target_format", "pdf");
        }

        let mut last_error: Option<String> = None;
        for attempt in 0..2 {
            let form = multipart::Form::new().part(
                "file",
                multipart::Part::bytes(file_bytes.clone()).file_name(file.filename.clone()).mime_str(&mime_type)?,
            );

            let client = self.services_http_client()?;
            let response = match client.post(convert_url.clone()).multipart(form).send().await {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(format!("Office convert API request failed (url={}): {}", convert_url, err));
                    if attempt == 0 {
                        warn!(
                            "Office convert API request failed, retrying in 3s: {}",
                            last_error.as_deref().unwrap_or("unknown error")
                        );
                        time::sleep(Duration::from_secs(3)).await;
                    }
                    continue;
                }
            };

            let response_url = response.url().to_string();
            let status = response.status();
            let body_bytes = response.bytes().await?;
            if !status.is_success() {
                last_error = Some(format!(
                    "Office convert API failed (status={}, url={}, body_len={}): {}",
                    status.as_u16(),
                    response_url,
                    body_bytes.len(),
                    summarize_http_body(&body_bytes)
                ));
            } else if body_bytes.is_empty() {
                last_error = Some(format!("Office convert API returned empty PDF body (url={})", response_url));
            } else if !body_bytes.starts_with(b"%PDF-") {
                last_error = Some(format!(
                    "Office convert API returned non-PDF body (status={}, url={}, body_len={}): {}",
                    status.as_u16(),
                    response_url,
                    body_bytes.len(),
                    summarize_http_body(&body_bytes)
                ));
            } else {
                fs::write(&temp_pdf_path, &body_bytes).await?;
                fs::rename(&temp_pdf_path, &stored_pdf_path).await?;
                return Ok(stored_pdf_path);
            }

            let _ = fs::remove_file(&temp_pdf_path).await;
            if attempt == 0 {
                warn!(
                    "Office convert API failed, retrying in 3s: {}",
                    last_error.as_deref().unwrap_or("unknown error")
                );
                time::sleep(Duration::from_secs(3)).await;
            }
        }

        let error_msg = last_error.unwrap_or_else(|| "unknown error".to_string());
        Err(anyhow::anyhow!("Failed to convert office document to PDF: {}", error_msg))
    }

    /// 将 Word/Excel 文档转换为 PDF 并处理
    async fn convert_office_to_pdf_and_process(
        &self, file: &File, mut timing: Option<&mut ParseTimingCtx>,
    ) -> anyhow::Result<()> {
        let stored_pdf_path =
            timed_step_opt(timing.as_deref_mut(), "convert_office_to_pdf", self.convert_office_to_pdf(file)).await?;

        // 创建临时 File 结构用于处理 PDF
        let mut temp_file = file.clone();
        temp_file.path = stored_pdf_path.to_string_lossy().to_string();
        temp_file.filename = format!("{}.pdf", file.id);

        // 使用 process_pdf_file 处理转换后的 PDF
        self.process_pdf_file(&temp_file, None, false, Some(file.filename.as_str()), timing).await
    }

    /// 处理 Excel 文件，按 sheet+行 生成切片
    async fn process_excel_file(&self, file: &File, mut timing: Option<&mut ParseTimingCtx>) -> anyhow::Result<()> {
        if !self.ensure_file_exists(file.id, "excel processing start").await? {
            return Ok(());
        }
        info!("Processing Excel file: {}", file.filename);

        let slices =
            timed_step_opt(timing.as_deref_mut(), "parse_excel", async { self.parse_excel_to_slices(file).await })
                .await?;

        if slices.is_empty() {
            timed_step_opt(timing.as_deref_mut(), "finalize_empty_excel", async {
                let sql =
                    "UPDATE files SET status = 1, content = ?, log = ?, updated_at = strftime('%s','now') WHERE id = ?";
                sqlx::query(sql)
                    .bind("")
                    .bind("Excel processed successfully (empty)")
                    .bind(file.id)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            })
            .await?;
            return Ok(());
        }

        let slice_count = slices.len();
        let full_content = slices.iter().map(|s| s.content.as_str()).collect::<Vec<_>>().join("\n\n");

        let (search_docs, search_embeddings) = timed_step_opt(timing.as_deref_mut(), "insert_slices", async {
            let persisted = self.insert_slices_and_positions(file.id, slices).await?;
            let mut search_docs = Vec::with_capacity(persisted.len());
            let mut search_embeddings = Vec::with_capacity(persisted.len());
            for (id, content) in persisted {
                search_docs.push(tantivy_engine::Document::new(id, file.id, file.kb_id, content));
                search_embeddings.push(None);
            }
            Ok((search_docs, search_embeddings))
        })
        .await?;

        if !search_docs.is_empty() {
            timed_step_opt(timing.as_deref_mut(), "write_search_batch", async {
                self.search_engine.write_batch(search_docs, search_embeddings).await?;
                Ok(())
            })
            .await?;
        }

        timed_step_opt(timing.as_deref_mut(), "build_knowledge_graph", async {
            self.maybe_build_knowledge_graph(file).await;
            Ok(())
        })
        .await?;

        if !self.ensure_file_exists(file.id, "before writing full index").await? {
            return Ok(());
        }

        let index_full_content = format!("{}\n\n{}", file.filename, full_content);
        timed_step_opt(timing.as_deref_mut(), "write_full_index", async {
            self.search_engine
                .write_full(tantivy_engine::Document::new(file.id, file.id, file.kb_id, index_full_content))
                .await?;
            Ok(())
        })
        .await?;

        if !self.ensure_file_exists(file.id, "before updating status").await? {
            return Ok(());
        }
        timed_step_opt(timing, "finalize_file_status", async {
            let sql =
                "UPDATE files SET status = 1, content = ?, log = ?, updated_at = strftime('%s','now') WHERE id = ?";
            sqlx::query(sql)
                .bind(&full_content)
                .bind("Excel processed successfully")
                .bind(file.id)
                .execute(&self.pool)
                .await?;
            Ok(())
        })
        .await?;

        info!("Excel file {} processed successfully with {} slices", file.id, slice_count);

        self.search_engine.reload_readers()?;

        Ok(())
    }

    /// 解析 Excel 文件，按 sheet+行 生成切片
    async fn parse_excel_to_slices(&self, file: &File) -> anyhow::Result<Vec<SliceWithPositions>> {
        use calamine::{Reader, open_workbook_auto};

        let path = std::path::Path::new(&file.path);
        let mut workbook: calamine::Sheets<std::io::BufReader<std::fs::File>> =
            open_workbook_auto(path).map_err(|e| anyhow::anyhow!("Failed to open Excel file: {}", e))?;

        let mut slices = Vec::new();
        let sheet_names = workbook.sheet_names().to_vec();

        for (sheet_idx, sheet_name) in sheet_names.iter().enumerate() {
            let range = match workbook.worksheet_range(sheet_name) {
                Ok(r) => r,
                Err(e) => {
                    warn!("Failed to read sheet '{}' in file {}: {}", sheet_name, file.id, e);
                    continue;
                }
            };

            let rows: Vec<_> = range.rows().collect();
            if rows.len() < 2 {
                continue;
            }

            let header: Vec<String> = rows[0]
                .iter()
                .enumerate()
                .map(|(col_idx, cell)| {
                    let s = cell.to_string().trim().to_string();
                    if s.is_empty() { format!("列{}", col_idx + 1) } else { s }
                })
                .collect();

            for (row_idx, row) in rows.iter().enumerate().skip(1) {
                let mut lines = Vec::new();
                let mut has_data = false;

                for (col_idx, cell) in row.iter().enumerate() {
                    let value = cell.to_string().trim().to_string();
                    if !value.is_empty() {
                        let header_label = header.get(col_idx).cloned().unwrap_or_else(|| format!("列{}", col_idx + 1));
                        lines.push(format!("{}: {}", header_label, value));
                        has_data = true;
                    }
                }

                if !has_data {
                    continue;
                }

                let mut content = format!("Sheet: {}\n", sheet_name);
                content.push_str(&lines.join("\n"));

                let positions = vec![SlicePosition {
                    page_idx: sheet_idx as i32,
                    bbox: [0, 0, 0, 0],
                    sheet_name: Some(sheet_name.clone()),
                    row_num: Some((row_idx + 1) as i32),
                }];

                slices.push(SliceWithPositions { content, positions });
            }
        }

        Ok(slices)
    }

    /// 处理 PDF 文件，调用 MinerU API
    async fn process_pdf_file(
        &self, file: &File, image_embedding: Option<Vec<f32>>, is_image: bool, index_filename: Option<&str>,
        mut timing: Option<&mut ParseTimingCtx>,
    ) -> anyhow::Result<()> {
        if !self.ensure_file_exists(file.id, "pdf processing start").await? {
            return Ok(());
        }
        info!("Processing PDF file: {}", file.filename);

        // 先检查 pdf_contents 表中是否已有该文件的数据
        let (count, bbox_count): ((i64,), (i64,)) =
            timed_step_opt(timing.as_deref_mut(), "pdf_existing_content_check", async {
                let check_sql = "SELECT COUNT(*) as count FROM pdf_contents WHERE file_id = ?";
                let count: (i64,) = sqlx::query_as(check_sql).bind(file.id).fetch_one(&self.pool).await?;
                let bbox_sql =
                    "SELECT COUNT(*) as count FROM pdf_contents WHERE file_id = ? AND bbox IS NOT NULL AND bbox != ''";
                let bbox_count: (i64,) = sqlx::query_as(bbox_sql).bind(file.id).fetch_one(&self.pool).await?;
                Ok((count, bbox_count))
            })
            .await?;

        let mut content_list: Vec<ContentItem>;

        if count.0 > 0 && bbox_count.0 > 0 {
            // 已有数据，直接从数据库读取
            info!("Found existing PDF contents in database for file {}, skipping MinerU API call", file.id);

            content_list = timed_step_opt(timing.as_deref_mut(), "load_existing_pdf_content", async {
                let fetch_sql = "SELECT page_idx, bbox, text, text_level, img_path, table_body FROM pdf_contents WHERE file_id = ? ORDER BY page_idx, id";
                let rows: Vec<PdfContentDbRow> =
                    sqlx::query_as(fetch_sql).bind(file.id).fetch_all(&self.pool).await?;

                // 将数据库记录转换为 ContentItem
                let content_list = rows
                    .iter()
                    .map(|row| {
                        let typ = if row.2.is_some() {
                            "text".to_string()
                        } else if row.4.is_some() {
                            "image".to_string()
                        } else if row.5.is_some() {
                            "table".to_string()
                        } else {
                            "unknown".to_string()
                        };

                        let bbox = row
                            .1
                            .as_ref()
                            .and_then(|bbox| serde_json::from_str::<Vec<i32>>(bbox).ok())
                            .unwrap_or_default();

                        ContentItem {
                            typ,
                            bbox,
                            page_idx: row.0,
                            text: row.2.clone(),
                            text_level: row.3,
                            text_format: None,
                            img_path: row.4.clone(),
                            image_caption: None,
                            table_body: row.5.clone(),
                            table_caption: None,
                        }
                    })
                    .collect();
                Ok(content_list)
            })
            .await?;
        } else {
            // 没有数据，调用 MinerU API
            let mut mineru_result = self.call_mineru_api(file, is_image, timing.as_deref_mut()).await?;
            content_list = timed_step_opt(timing.as_deref_mut(), "parse_mineru_content_list", async {
                Ok(serde_json::from_str(&mineru_result.content_list)?)
            })
            .await?;
            Self::prefix_images_for_file(file.id, &mut content_list, &mut mineru_result.images);

            if !self.ensure_file_exists(file.id, "before writing pdf contents").await? {
                return Ok(());
            }

            // 提取文本内容并过滤掉 discarded 项
            let valid_content_items: Vec<ContentItem> =
                content_list.iter().filter(|item| item.typ != "discarded").cloned().collect();

            // 只有在有有效内容时才插入数据库
            if !valid_content_items.is_empty() {
                timed_step_opt(timing.as_deref_mut(), "write_pdf_contents", async {
                    if count.0 > 0 {
                        sqlx::query("DELETE FROM pdf_contents WHERE file_id = ?").bind(file.id).execute(&self.pool).await?;
                    }
                    let binds_per_row = 7_usize;
                    let max_vars = 999_usize;
                    let batch_size = std::cmp::max(1, max_vars / binds_per_row);
                    for chunk in valid_content_items.chunks(batch_size) {
                        let mut pdf_sql = QueryBuilder::<Sqlite>::new(
                            "insert into pdf_contents(file_id, page_idx, bbox, text, text_level, img_path, table_body) ",
                        );
                        pdf_sql.push_values(chunk.iter(), |mut b, item| {
                            let bbox = if item.bbox.is_empty() {
                                None
                            } else {
                                Some(serde_json::to_string(&item.bbox).unwrap_or_default())
                            };
                            b.push_bind(file.id)
                                .push_bind(item.page_idx)
                                .push_bind(bbox)
                                .push_bind(&item.text)
                                .push_bind(item.text_level)
                                .push_bind(&item.img_path)
                                .push_bind(&item.table_body);
                        });
                        pdf_sql.build().execute(&self.pool).await?;
                    }
                    Ok(())
                })
                .await?;
            }

            // 保存图片到本地
            timed_step_opt(timing.as_deref_mut(), "save_mineru_images", async {
                let cfg = config::get();
                fs::create_dir_all(&cfg.storage.images_path).await?;
                info!("image count: {}", mineru_result.images.len());
                info!("images: {:?}", mineru_result.images.keys());
                for (img_name, img_base64) in &mineru_result.images {
                    // 保存图片，如果以 data:image/jpeg;base64, 开头就去掉，没有也不报错
                    let base64_marker = "base64,";
                    let (prefix, payload) = match img_base64.find(base64_marker) {
                        Some(idx) => {
                            (&img_base64[..idx + base64_marker.len()], &img_base64[idx + base64_marker.len()..])
                        }
                        None => ("(raw)", img_base64.as_str()),
                    };
                    let preview: String = payload.chars().take(32).collect();
                    debug!(
                        "Decoding mineru image {} for file {} (prefix={}, len={}, preview=\"{}\")",
                        img_name,
                        file.id,
                        prefix,
                        payload.len(),
                        preview
                    );
                    let bytes = STANDARD.decode(payload).map_err(|err| {
                        error!(
                            "Failed to decode mineru image {} for file {} (prefix={}, len={}, preview=\"{}\"): {}",
                            img_name,
                            file.id,
                            prefix,
                            payload.len(),
                            preview,
                            err
                        );
                        anyhow::anyhow!(err)
                    })?;
                    fs::write(format!("{}/{}", cfg.storage.images_path, img_name), bytes).await?;
                }
                Ok(())
            })
            .await?;
        }

        let (full_content, full_segments) = timed_step_opt(timing.as_deref_mut(), "build_full_content", async {
            Ok(self.build_full_content_and_segments(&content_list))
        })
        .await?;

        // 根据 slice_type 决定切片方式
        let slices = timed_step_opt(timing.as_deref_mut(), "slice_build", async {
            let slices = if file.slice_type == "smart" || file.slice_type.is_empty() {
                // 智能切片：使用 content_list
                self.smart_slice_content_with_positions(&content_list)?
            } else if file.slice_type == "fixed" {
                self.fixed_slice_content_with_positions(&full_content, &full_segments)?
            } else {
                self.slice_content(&full_content, &file.slice_type)?
                    .into_iter()
                    .map(|content| SliceWithPositions { content, positions: vec![] })
                    .collect()
            };
            Ok(slices)
        })
        .await?;

        if !self.ensure_file_exists(file.id, "before writing slices").await? {
            return Ok(());
        }

        let slice_count = slices.len();
        let (search_docs, search_embeddings) = timed_step_opt(timing.as_deref_mut(), "insert_slices", async {
            let persisted = self.insert_slices_and_positions(file.id, slices).await?;
            let mut search_docs = Vec::with_capacity(persisted.len());
            let mut search_embeddings = Vec::with_capacity(persisted.len());
            for (id, content) in persisted {
                search_docs.push(tantivy_engine::Document::new(id, file.id, file.kb_id, content));
                search_embeddings.push(image_embedding.clone());
            }
            Ok((search_docs, search_embeddings))
        })
        .await?;

        // 批量写入搜索引擎
        if !search_docs.is_empty() {
            timed_step_opt(timing.as_deref_mut(), "write_search_batch", async {
                self.search_engine.write_batch(search_docs, search_embeddings).await?;
                Ok(())
            })
            .await?;
        }

        // 构建知识图谱
        timed_step_opt(timing.as_deref_mut(), "build_knowledge_graph", async {
            self.maybe_build_knowledge_graph(file).await;
            Ok(())
        })
        .await?;

        if !self.ensure_file_exists(file.id, "before writing full index").await? {
            return Ok(());
        }

        let index_filename = index_filename.unwrap_or(file.filename.as_str());
        let index_full_content = format!("{}\n\n{}", index_filename, full_content);
        debug!("write full content: {}", index_full_content);
        timed_step_opt(timing.as_deref_mut(), "write_full_index", async {
            self.search_engine
                .write_full(tantivy_engine::Document::new(file.id, file.id, file.kb_id, index_full_content))
                .await?;
            Ok(())
        })
        .await?;

        // 更新文件状态
        if !self.ensure_file_exists(file.id, "before updating status").await? {
            return Ok(());
        }
        timed_step_opt(timing, "finalize_file_status", async {
            let sql =
                "UPDATE files SET status = 1, content = ?, log = ?, updated_at = strftime('%s','now') WHERE id = ?";
            sqlx::query(sql)
                .bind(&full_content)
                .bind("PDF processed successfully")
                .bind(file.id)
                .execute(&self.pool)
                .await?;
            Ok(())
        })
        .await?;

        info!("PDF file {} processed successfully with {} slices", file.id, slice_count);

        self.search_engine.reload_readers()?;

        Ok(())
    }

    async fn call_mineru_api(
        &self, file: &File, is_image: bool, mut timing: Option<&mut ParseTimingCtx>,
    ) -> anyhow::Result<Result> {
        let cfg = config::get();

        if !is_image && cfg.services.mineru_max_pages > 0 {
            match Document::load(&file.path) {
                Ok(doc) => {
                    let total_pages = doc.get_pages().len();
                    if total_pages > cfg.services.mineru_max_pages {
                        return timed_step_opt(timing.as_deref_mut(), "mineru_api_in_ranges", async {
                            self.call_mineru_api_in_ranges(file, total_pages, cfg.services.mineru_max_pages).await
                        })
                        .await;
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to read PDF page count for {}: {}, falling back to single MinerU request",
                        file.filename, e
                    );
                }
            }
        }

        timed_step_opt(timing, "mineru_api", async {
            self.call_mineru_api_with_path(&file.path, &file.filename, is_image, None, None).await
        })
        .await
    }

    async fn call_mineru_api_in_ranges(
        &self, file: &File, total_pages: usize, max_pages: usize,
    ) -> anyhow::Result<Result> {
        let total_parts = total_pages.div_ceil(max_pages);
        info!(
            "PDF {} has {} pages, calling MinerU in {} ranges (max {} pages per range)",
            file.filename, total_pages, total_parts, max_pages
        );

        let mut merged_items: Vec<ContentItem> = Vec::new();
        let mut merged_images: HashMap<String, String> = HashMap::new();

        for (part_index, range_start) in (0..total_pages).step_by(max_pages).enumerate() {
            let range_end = std::cmp::min(range_start + max_pages, total_pages) - 1;
            let chunk_filename = format!("{}_part_{}.pdf", file.filename, part_index + 1);

            let chunk_result = self
                .call_mineru_api_with_path(&file.path, &chunk_filename, false, Some(range_start), Some(range_end))
                .await?;
            let mut chunk_items: Vec<ContentItem> = serde_json::from_str(&chunk_result.content_list)?;

            let image_prefix = format!("part{}_", part_index + 1);
            let min_page_idx = chunk_items.iter().map(|item| item.page_idx).min();
            let needs_offset = min_page_idx.is_some_and(|min| min < range_start as i32);
            for item in &mut chunk_items {
                if needs_offset {
                    item.page_idx += range_start as i32;
                }
                if let Some(img_path) = item.img_path.as_deref() {
                    item.img_path = Some(Self::prefix_image_path(img_path, &image_prefix));
                }
            }

            for (img_name, img_base64) in chunk_result.images {
                let prefixed = Self::prefix_image_path(&img_name, &image_prefix);
                merged_images.insert(prefixed, img_base64);
            }

            merged_items.extend(chunk_items);
        }

        let content_list = serde_json::to_string(&merged_items)?;
        Ok(Result { content_list, images: merged_images })
    }

    fn prefix_image_path(img_path: &str, prefix: &str) -> String {
        match img_path.rsplit_once('/') {
            Some((dir, name)) => format!("{}/{}{}", dir, prefix, name),
            None => format!("{}{}", prefix, img_path),
        }
    }

    fn prefix_images_for_file(file_id: i64, content_list: &mut [ContentItem], images: &mut HashMap<String, String>) {
        if images.is_empty() {
            return;
        }
        let prefix = format!("f{}_", file_id);
        let mut renamed: HashMap<String, String> = HashMap::new();
        for item in content_list.iter_mut() {
            let Some(img_path) = item.img_path.as_deref() else { continue };
            let new_name =
                renamed.entry(img_path.to_string()).or_insert_with(|| Self::prefix_image_path(img_path, &prefix));
            item.img_path = Some(new_name.clone());
        }

        let mut new_images = HashMap::new();
        for (img_name, img_base64) in images.drain() {
            let new_name =
                renamed.get(&img_name).cloned().unwrap_or_else(|| Self::prefix_image_path(&img_name, &prefix));
            new_images.insert(new_name, img_base64);
        }
        *images = new_images;
    }

    async fn call_mineru_api_with_path(
        &self, file_path: &str, filename: &str, is_image: bool, start_page: Option<usize>, end_page: Option<usize>,
    ) -> anyhow::Result<Result> {
        let file_bytes = tokio::fs::read(file_path).await?;
        let cfg = config::get();
        let mineru_url = cfg.services.mineru_url.trim_end_matches('/');

        let client = self.services_http_client()?;
        if mineru_url.ends_with("/file_parse") {
            // 构建 multipart form
            let mime_type = if is_image {
                mime_guess::from_path(filename).first_or_octet_stream().essence_str().to_string()
            } else {
                "application/pdf".to_string()
            };

            let mut form = multipart::Form::new()
                .text("return_middle_json", "false")
                .text("return_model_output", "false")
                .text("return_md", "false")
                .text("return_images", "true")
                .text("parse_method", "auto")
                .text("lang_list", "ch")
                .text("output_dir", "")
                .text("server_url", "string")
                .text("return_content_list", "true")
                .text("backend", "pipeline")
                .text("table_enable", "true")
                .text("response_format_zip", "false")
                .text("formula_enable", "true")
                .part(
                    "files",
                    multipart::Part::bytes(file_bytes).file_name(filename.to_string()).mime_str(&mime_type)?,
                );
            let start_page_id = start_page.unwrap_or(0);
            let end_page_id = end_page.map(|v| v.to_string()).unwrap_or_else(|| "99999".to_string());
            form = form.text("start_page_id", start_page_id.to_string()).text("end_page_id", end_page_id);

            // 调用 MinerU API
            let response = client.post(mineru_url).multipart(form).send().await?;
            let status = response.status();
            let body_bytes = response.bytes().await?;
            if !status.is_success() {
                let error_text = String::from_utf8_lossy(&body_bytes);
                return Err(anyhow::anyhow!("MinerU API failed: {}", error_text));
            }

            let mineru_response: MinerUResponse = serde_json::from_slice(&body_bytes).map_err(|e| {
                let body_text = String::from_utf8_lossy(&body_bytes);
                anyhow::anyhow!("MinerU API response decode failed: {} - {}", e, body_text)
            })?;
            let mineru_result = match mineru_response.results {
                MinerUResults::Map(results) => {
                    results.into_values().next().ok_or_else(|| anyhow::anyhow!("MinerU API returned empty results"))?
                }
                MinerUResults::List(results) => {
                    if let Some(item) = results.iter().find(|item| item.status == "error") {
                        let mut error_msg = item.error.clone();
                        if error_msg.is_empty() {
                            error_msg = "MinerU API returned error result".to_string();
                        }
                        if !item.filename.is_empty() {
                            error_msg = format!("{} (file: {})", error_msg, item.filename);
                        }
                        return Err(anyhow::anyhow!("MinerU API failed: {}", error_msg));
                    }
                    let first = results
                        .into_iter()
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("MinerU API returned empty results"))?;
                    Result { content_list: first.content_list, images: first.images }
                }
            };
            return Ok(mineru_result);
        }

        let mut url = reqwest::Url::parse(mineru_url)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("backend", "pipeline");
            query.append_pair("method", "auto");
            query.append_pair("lang", "ch");
            query.append_pair("start_page", &start_page.unwrap_or(0).to_string());
            if let Some(end_page) = end_page {
                query.append_pair("end_page", &end_page.to_string());
            }
            query.append_pair("formula_enable", "true");
            query.append_pair("table_enable", "true");
        }

        let mime_type = mime_guess::from_path(filename).first_or_octet_stream().essence_str().to_string();
        let form = multipart::Form::new()
            .part("file", multipart::Part::bytes(file_bytes).file_name(filename.to_string()).mime_str(&mime_type)?);

        let response = client.post(url.clone()).multipart(form).send().await?;
        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!("MinerU API failed: {}", error_text));
        }

        let analyze_response: AnalyzePdfResponse = response.json().await?;
        if analyze_response.code != 200 {
            return Err(anyhow::anyhow!("MinerU API failed: {}", analyze_response.message));
        }

        let Some(data) = analyze_response.data else {
            return Err(anyhow::anyhow!("MinerU API returned empty data"));
        };

        let host = url.host_str().ok_or_else(|| anyhow::anyhow!("MinerU API URL missing host"))?;
        let origin = if let Some(port) = url.port() {
            format!("{}://{}:{}", url.scheme(), host, port)
        } else {
            format!("{}://{}", url.scheme(), host)
        };

        let mut content_list = data.content_list;
        let mut images = HashMap::new();
        for item in &mut content_list {
            let Some(img_path) = item.img_path.as_deref() else { continue };
            let filename = std::path::Path::new(img_path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(img_path)
                .to_string();

            if !images.contains_key(&filename) {
                let image_url = format!("{}/output/{}", origin, img_path.trim_start_matches('/'));
                let image_response = client.get(&image_url).send().await?;
                if !image_response.status().is_success() {
                    let error_text = image_response.text().await?;
                    return Err(anyhow::anyhow!("MinerU image download failed: {}", error_text));
                }
                let image_bytes = image_response.bytes().await?;
                let image_mime = mime_guess::from_path(&filename).first_or_octet_stream().essence_str().to_string();
                let image_base64 = STANDARD.encode(image_bytes);
                images.insert(filename.clone(), format!("data:{};base64,{}", image_mime, image_base64));
            }

            item.img_path = Some(filename);
        }

        let content_list_json = serde_json::to_string(&content_list)?;
        Ok(Result { content_list: content_list_json, images })
    }

    /// 处理普通文本文件
    async fn process_text_file(&self, file: &File, mut timing: Option<&mut ParseTimingCtx>) -> anyhow::Result<()> {
        if !self.ensure_file_exists(file.id, "before reading text").await? {
            return Ok(());
        }
        // 示例：读取文件内容
        let content = timed_step_opt(timing.as_deref_mut(), "read_text_file", async {
            Ok(tokio::fs::read_to_string(file.path.as_str()).await?)
        })
        .await?;
        self.process_plain_text_content(file, &content, "Processing completed successfully", timing)
            .await
    }

    async fn process_audio_file(&self, file: &File, mut timing: Option<&mut ParseTimingCtx>) -> anyhow::Result<()> {
        info!("Processing audio file: {}", file.filename);

        if !self.ensure_file_exists(file.id, "before reading audio").await? {
            return Ok(());
        }
        let file_bytes =
            timed_step_opt(timing.as_deref_mut(), "read_audio_file", async { Ok(tokio::fs::read(&file.path).await?) })
                .await?;
        let mime_type = mime_guess::from_path(&file.filename).first_or_octet_stream().essence_str().to_string();

        let form = multipart::Form::new()
            .part("file", multipart::Part::bytes(file_bytes).file_name(file.filename.clone()).mime_str(&mime_type)?);

        let client = self.services_http_client()?;
        let cfg = config::get();
        let mut req_builder = client.post(&cfg.services.audio_transcription_url).multipart(form);
        if let Some(key) = &cfg.services.audio_transcription_key
            && !key.is_empty() {
                req_builder = req_builder.header("Authorization", format!("Bearer {}", key));
            }

        let response =
            timed_step_opt(timing.as_deref_mut(), "audio_transcription_api", async { Ok(req_builder.send().await?) })
                .await?;
        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!("Audio transcription API failed: {}", error_text));
        }

        let AudioTranscriptionResponse { text, language } = response.json().await?;
        let log_message = if language.is_empty() {
            "Audio processed successfully".to_string()
        } else {
            format!("Audio processed successfully (language: {})", language)
        };

        self.process_plain_text_content(file, &text, &log_message, timing).await
    }

    async fn process_plain_text_content(
        &self, file: &File, content: &str, log_message: &str, mut timing: Option<&mut ParseTimingCtx>,
    ) -> anyhow::Result<()> {
        if !self.ensure_file_exists(file.id, "before writing slices").await? {
            return Ok(());
        }
        // 示例：根据 slice_type 进行分片处理
        let slices = timed_step_opt(timing.as_deref_mut(), "slice_build", async {
            self.slice_content(content, &file.slice_type)
        })
        .await?;
        let slice_count = slices.len();

        // 保存分片到数据库并收集文档
        let search_docs = timed_step_opt(timing.as_deref_mut(), "insert_slices", async {
            let wrapped: Vec<SliceWithPositions> =
                slices.into_iter().map(|content| SliceWithPositions { content, positions: vec![] }).collect();
            let persisted = self.insert_slices_and_positions(file.id, wrapped).await?;
            let mut search_docs = Vec::with_capacity(persisted.len());
            for (id, content) in persisted {
                search_docs.push(tantivy_engine::Document::new(id, file.id, file.kb_id, content));
            }
            Ok(search_docs)
        })
        .await?;

        // 批量写入搜索引擎
        if !search_docs.is_empty() {
            timed_step_opt(timing.as_deref_mut(), "write_search_batch", async {
                let embeddings = vec![None; search_docs.len()];
                self.search_engine.write_batch(search_docs, embeddings).await?;
                Ok(())
            })
            .await?;
        }

        if !self.ensure_file_exists(file.id, "before writing full index").await? {
            return Ok(());
        }
        let index_full_content = format!("{}\n\n{}", file.filename, content);
        timed_step_opt(timing.as_deref_mut(), "write_full_index", async {
            self.search_engine
                .write_full(tantivy_engine::Document::new(file.id, file.id, file.kb_id, index_full_content))
                .await?;
            Ok(())
        })
        .await?;

        // 更新文件状态为已处理，并保存内容
        if !self.ensure_file_exists(file.id, "before updating status").await? {
            return Ok(());
        }
        timed_step_opt(timing.as_deref_mut(), "finalize_file_status", async {
            let sql =
                "UPDATE files SET status = 1, content = ?, log = ?, updated_at = strftime('%s','now') WHERE id = ?";
            sqlx::query(sql).bind(content).bind(log_message).bind(file.id).execute(&self.pool).await?;
            Ok(())
        })
        .await?;

        info!("File {} processed successfully with {} slices", file.id, slice_count);

        self.search_engine.reload_readers()?;

        // 构建知识图谱
        timed_step_opt(timing, "build_knowledge_graph", async {
            self.maybe_build_knowledge_graph(file).await;
            Ok(())
        })
        .await?;

        Ok(())
    }

    /// 标记文件处理失败
    async fn mark_file_failed(&self, file_id: i64, error_msg: &str) -> anyhow::Result<()> {
        let sql = "UPDATE files SET status = -1, log = ?, updated_at = strftime('%s','now') WHERE id = ?";
        sqlx::query(sql).bind(error_msg).bind(file_id).execute(&self.pool).await?;
        Ok(())
    }

    async fn mark_file_storage_skipped(&self, file_id: i64) -> anyhow::Result<()> {
        let sql = "UPDATE files SET status = 3, log = ?, updated_at = strftime('%s','now') WHERE id = ?";
        sqlx::query(sql).bind("Storage mode: not parsed").bind(file_id).execute(&self.pool).await?;
        Ok(())
    }

    /// 事务化写入 slices 和 slice_positions，减少单条 INSERT 的提交开销。
    /// 返回每个 slice 的 (id, content) 供后续构建搜索文档使用。
    async fn insert_slices_and_positions(
        &self, file_id: i64, slices: Vec<SliceWithPositions>,
    ) -> anyhow::Result<Vec<(i64, String)>> {
        if slices.is_empty() {
            return Ok(Vec::new());
        }
        // 每批最多 500 条 slice 一个事务，避免长时间持有 SQLite 写锁阻塞其他写操作
        const SLICE_TX_BATCH: usize = 500;
        let mut persisted: Vec<(i64, String)> = Vec::with_capacity(slices.len());
        for slice_chunk in slices.chunks(SLICE_TX_BATCH) {
            let mut tx = self.pool.begin().await?;
            let mut chunk_persisted: Vec<(i64, String)> = Vec::with_capacity(slice_chunk.len());
            let mut position_rows: Vec<(i64, SlicePosition)> = Vec::new();
            let binds_per_row = 2_usize;
            let max_vars = 999_usize;
            let batch_size = std::cmp::max(1, max_vars / binds_per_row);
            for insert_chunk in slice_chunk.chunks(batch_size) {
                let mut slice_sql = QueryBuilder::<Sqlite>::new("INSERT INTO slices (file_id, content) ");
                slice_sql.push_values(insert_chunk.iter(), |mut b, slice| {
                    b.push_bind(file_id).push_bind(&slice.content);
                });
                slice_sql.push(" RETURNING id");

                let inserted_ids: Vec<(i64,)> = slice_sql.build_query_as().fetch_all(&mut *tx).await?;
                anyhow::ensure!(
                    inserted_ids.len() == insert_chunk.len(),
                    "inserted slice row count mismatch: expected {}, got {}",
                    insert_chunk.len(),
                    inserted_ids.len()
                );

                for (slice, (id,)) in insert_chunk.iter().zip(inserted_ids) {
                    for position in &slice.positions {
                        position_rows.push((id, position.clone()));
                    }
                    chunk_persisted.push((id, slice.content.clone()));
                }
            }
            if !position_rows.is_empty() {
                let binds_per_row = 8_usize;
                let max_vars = 999_usize;
                let batch_size = std::cmp::max(1, max_vars / binds_per_row);
                for chunk in position_rows.chunks(batch_size) {
                    let mut pos_sql = QueryBuilder::<Sqlite>::new(
                        "insert into slice_positions(slice_id, page_idx, x1, y1, x2, y2, sheet_name, row_num) ",
                    );
                    pos_sql.push_values(chunk.iter(), |mut b, (slice_id, position)| {
                        b.push_bind(slice_id)
                            .push_bind(position.page_idx)
                            .push_bind(position.bbox[0])
                            .push_bind(position.bbox[1])
                            .push_bind(position.bbox[2])
                            .push_bind(position.bbox[3])
                            .push_bind(&position.sheet_name)
                            .push_bind(position.row_num);
                    });
                    pos_sql.build().execute(&mut *tx).await?;
                }
            }
            tx.commit().await?;
            persisted.extend(chunk_persisted);
        }
        Ok(persisted)
    }

    async fn ensure_file_exists(&self, file_id: i64, stage: &str) -> anyhow::Result<bool> {
        let exists = sqlx::query_scalar::<_, i64>("SELECT 1 FROM files WHERE id = ? LIMIT 1")
            .bind(file_id)
            .fetch_optional(&self.pool)
            .await?
            .is_some();
        if !exists {
            info!("File {} no longer exists during {}, skipping further processing", file_id, stage);
        }
        Ok(exists)
    }

    async fn is_storage_kb(&self, kb_id: Option<i64>) -> anyhow::Result<bool> {
        let Some(kb_id) = kb_id else { return Ok(false) };
        let kb_type: Option<String> = sqlx::query_scalar("SELECT kb_type FROM knowledge_bases WHERE id = ?")
            .bind(kb_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(matches!(kb_type.as_deref(), Some("storage")))
    }

    fn is_image_file(filename_lower: &str) -> bool {
        filename_lower.ends_with(".jpg")
            || filename_lower.ends_with(".jpeg")
            || filename_lower.ends_with(".png")
            || filename_lower.ends_with(".gif")
            || filename_lower.ends_with(".bmp")
            || filename_lower.ends_with(".webp")
            || filename_lower.ends_with(".tiff")
            || filename_lower.ends_with(".tif")
            || filename_lower.ends_with(".svg")
            || filename_lower.ends_with(".ico")
            || filename_lower.ends_with(".heic")
            || filename_lower.ends_with(".heif")
    }

    fn is_audio_file(filename_lower: &str) -> bool {
        filename_lower.ends_with(".wav")
            || filename_lower.ends_with(".mp3")
            || filename_lower.ends_with(".m4a")
            || filename_lower.ends_with(".aac")
            || filename_lower.ends_with(".flac")
            || filename_lower.ends_with(".ogg")
            || filename_lower.ends_with(".opus")
            || filename_lower.ends_with(".wma")
            || filename_lower.ends_with(".amr")
            || filename_lower.ends_with(".aiff")
            || filename_lower.ends_with(".aif")
            || filename_lower.ends_with(".alac")
            || filename_lower.ends_with(".webm")
    }

    /// 根据 slice_type 对内容进行分片
    fn slice_content(&self, content: &str, slice_type: &str) -> anyhow::Result<Vec<String>> {
        match slice_type {
            "paragraph" => {
                // 按段落分片（以双换行符分隔）
                Ok(content.split("\n\n").map(|s| s.trim()).filter(|s| !s.is_empty()).map(|s| s.to_string()).collect())
            }
            "sentence" => {
                // 按句子分片（简单实现：以句号、问号、感叹号分隔）
                Ok(content
                    .split(['。', '.', '?', '!', '？', '！'])
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect())
            }
            _ => {
                // 固定长度分片（每8000字符，重叠100字符）
                let cfg = config::get();
                let chunk_size = cfg.slice.smart_slice_max_chars;
                let overlap = cfg.slice.fixed_slice_overlap_chars;

                let chars: Vec<char> = content.chars().collect();
                let mut slices = Vec::new();
                let mut start = 0;

                while start < chars.len() {
                    let end = std::cmp::min(start + chunk_size, chars.len());
                    let slice: String = chars[start..end].iter().collect();
                    slices.push(slice);

                    if end >= chars.len() {
                        break;
                    }

                    // 下一个切片从 end - overlap 开始，以实现重叠
                    start = end.saturating_sub(overlap);
                    if start >= end {
                        break;
                    }
                }

                Ok(slices)
            }
        }
    }

    fn build_full_content_and_segments(&self, content_list: &[ContentItem]) -> (String, Vec<Segment>) {
        let mut full_content = String::new();
        let mut segments = Vec::new();
        let mut current_len = 0usize;

        for pdf_content in content_list {
            if pdf_content.typ == "discarded" {
                continue;
            }
            let mut item_content = String::new();
            if pdf_content.typ == "text" {
                if let Some(text) = &pdf_content.text {
                    if let Some(lv) = pdf_content.text_level {
                        item_content.push_str("#".repeat(lv as usize).as_str());
                        item_content.push(' ');
                    }
                    item_content.push_str(text);
                }
            } else if pdf_content.typ == "image" {
                if let Some(img_name) = &pdf_content.img_path {
                    item_content.push_str(&format!("![{}](/api/v1/knowledge/files/{})", img_name, img_name));
                }
                if let Some(captions) = &pdf_content.image_caption {
                    for caption in captions {
                        item_content.push_str(&caption.to_string());
                    }
                }
            } else if pdf_content.typ == "table" {
                if let Some(table_caption) = &pdf_content.table_caption {
                    for caption in table_caption {
                        item_content.push_str(&caption.to_string());
                    }
                }
                if let Some(table_body) = &pdf_content.table_body {
                    item_content.push_str(table_body);
                }
            }

            if item_content.is_empty() {
                continue;
            }

            let start = current_len;
            full_content.push_str(&item_content);
            current_len += item_content.chars().count();
            let end = current_len;

            let positions = Self::positions_from_item(pdf_content);
            if !positions.is_empty() {
                segments.push(Segment { start, end, positions });
            }

            full_content.push_str("\n\n");
            current_len += 2;
        }

        (full_content, segments)
    }

    fn fixed_slice_content_with_positions(
        &self, content: &str, segments: &[Segment],
    ) -> anyhow::Result<Vec<SliceWithPositions>> {
        let cfg = config::get();
        let chunk_size = cfg.slice.smart_slice_max_chars;
        let overlap = cfg.slice.fixed_slice_overlap_chars;

        let chars: Vec<char> = content.chars().collect();
        let mut slices = Vec::new();
        let mut start = 0;

        while start < chars.len() {
            let end = std::cmp::min(start + chunk_size, chars.len());
            let slice: String = chars[start..end].iter().collect();
            let positions = Self::positions_for_range(segments, start, end);
            slices.push(SliceWithPositions { content: slice, positions });

            if end >= chars.len() {
                break;
            }

            start = end.saturating_sub(overlap);
            if start >= end {
                break;
            }
        }

        Ok(slices)
    }

    fn smart_slice_content_with_positions(
        &self, content_list: &[ContentItem],
    ) -> anyhow::Result<Vec<SliceWithPositions>> {
        let cfg = config::get();
        let max_chars = cfg.slice.smart_slice_max_chars;

        let mut slices = Vec::new();
        let mut current_slice = String::new();
        let mut current_segments: Vec<Segment> = Vec::new();
        let mut current_len = 0usize;
        let mut current_header = String::new(); // 当前所在的标题
        let mut current_header_positions: Vec<SlicePosition> = Vec::new();

        for item in content_list {
            if item.typ == "discarded" {
                continue;
            }

            let mut item_content = String::new();
            let item_positions = Self::positions_from_item(item);

            // 处理文本内容
            if item.typ == "text" {
                if let Some(text) = &item.text {
                    // 如果是标题，更新当前标题
                    if let Some(level) = item.text_level {
                        // 这是一个标题
                        let header_text = format!("{} {}", "#".repeat(level as usize), text);

                        // 如果当前有累积的内容，先保存
                        if !current_slice.trim().is_empty() {
                            self.flush_slice_with_positions(
                                &mut slices,
                                &mut current_slice,
                                &mut current_segments,
                                max_chars,
                            );
                            current_len = 0;
                        }

                        // 更新当前标题
                        current_header = header_text;
                        current_header_positions = item_positions.clone();
                        // 开始新的切片，包含标题
                        current_slice.clear();
                        current_segments.clear();
                        Self::append_segment(
                            &mut current_slice,
                            &mut current_len,
                            &mut current_segments,
                            &current_header,
                            current_header_positions.clone(),
                        );
                        Self::append_separator(&mut current_slice, &mut current_len);
                        continue;
                    } else {
                        // 普通文本
                        item_content.push_str(text);
                    }
                }
            } else if item.typ == "image" {
                if let Some(img_name) = &item.img_path {
                    item_content.push_str(&format!("![{}](/api/v1/knowledge/files/{})", img_name, img_name));
                }
                if let Some(captions) = &item.image_caption {
                    for caption in captions {
                        item_content.push_str(&caption.to_string());
                    }
                }
            } else if item.typ == "table" {
                if let Some(table_caption) = &item.table_caption {
                    for caption in table_caption {
                        item_content.push_str(&caption.to_string());
                    }
                }
                if let Some(table_body) = &item.table_body {
                    item_content.push_str(
                        &table_body
                            .replace(" colspan=\"1\"", "")
                            .replace(" rowspan=\"1\"", "")
                            .replace(" colspan='1'", "")
                            .replace(" rowspan='1'", "")
                            .replace(" colspan=1", "")
                            .replace(" rowspan=1", ""),
                    );
                }
            }

            if item_content.is_empty() {
                continue;
            }

            // 检查加入这个内容后是否超过字数限制
            let test_len = if current_slice.is_empty() {
                // 如果当前切片为空，但有标题，先加上标题
                if !current_header.is_empty() {
                    current_header.chars().count() + 2 + item_content.chars().count()
                } else {
                    item_content.chars().count()
                }
            } else {
                current_len + item_content.chars().count() + 2
            };

            if test_len > max_chars {
                // 超过限制，保存当前切片
                if !current_slice.trim().is_empty() {
                    self.flush_slice_with_positions(&mut slices, &mut current_slice, &mut current_segments, max_chars);
                    current_len = 0;
                }

                // 开始新的切片
                current_slice.clear();
                current_segments.clear();
                if !current_header.is_empty() {
                    Self::append_segment(
                        &mut current_slice,
                        &mut current_len,
                        &mut current_segments,
                        &current_header,
                        current_header_positions.clone(),
                    );
                    Self::append_separator(&mut current_slice, &mut current_len);
                }
                Self::append_segment(
                    &mut current_slice,
                    &mut current_len,
                    &mut current_segments,
                    &item_content,
                    item_positions.clone(),
                );
                Self::append_separator(&mut current_slice, &mut current_len);
            } else {
                // 没有超过限制，继续累积
                if current_slice.is_empty() {
                    current_slice.clear();
                    current_segments.clear();
                    current_len = 0;
                    if !current_header.is_empty() {
                        Self::append_segment(
                            &mut current_slice,
                            &mut current_len,
                            &mut current_segments,
                            &current_header,
                            current_header_positions.clone(),
                        );
                        Self::append_separator(&mut current_slice, &mut current_len);
                    }
                    Self::append_segment(
                        &mut current_slice,
                        &mut current_len,
                        &mut current_segments,
                        &item_content,
                        item_positions.clone(),
                    );
                } else {
                    Self::append_segment(
                        &mut current_slice,
                        &mut current_len,
                        &mut current_segments,
                        &item_content,
                        item_positions.clone(),
                    );
                    Self::append_separator(&mut current_slice, &mut current_len);
                }
            }
        }

        // 保存最后的切片
        if !current_slice.trim().is_empty() {
            self.flush_slice_with_positions(&mut slices, &mut current_slice, &mut current_segments, max_chars);
        }

        Ok(slices)
    }

    fn flush_slice_with_positions(
        &self, slices: &mut Vec<SliceWithPositions>, current_slice: &mut String, segments: &mut Vec<Segment>,
        max_chars: usize,
    ) {
        if current_slice.trim().is_empty() {
            return;
        }
        let content = std::mem::take(current_slice);
        let segment_data = std::mem::take(segments);
        let mut new_slices = self.split_slice_with_positions(&content, &segment_data, max_chars);
        slices.append(&mut new_slices);
    }

    fn split_slice_with_positions(
        &self, content: &str, segments: &[Segment], max_chars: usize,
    ) -> Vec<SliceWithPositions> {
        let char_count = content.chars().count();
        if char_count == 0 {
            return Vec::new();
        }

        let ranges = if char_count <= max_chars {
            vec![(0, char_count)]
        } else {
            let sentence_ranges = Self::sentence_ranges(content);
            if sentence_ranges.is_empty() {
                vec![(0, char_count)]
            } else {
                let mut ranges = Vec::new();
                let mut slice_start = sentence_ranges[0].0;
                let mut slice_end = sentence_ranges[0].1;
                for (start, end) in sentence_ranges.iter().skip(1) {
                    if end - slice_start > max_chars && slice_end > slice_start {
                        ranges.push((slice_start, slice_end));
                        slice_start = *start;
                    }
                    slice_end = *end;
                }
                ranges.push((slice_start, slice_end));
                ranges
            }
        };

        let chars: Vec<char> = content.chars().collect();
        ranges
            .into_iter()
            .filter_map(|(start, end)| {
                if start >= end || end > chars.len() {
                    return None;
                }
                let slice: String = chars[start..end].iter().collect();
                let positions = Self::positions_for_range(segments, start, end);
                Some(SliceWithPositions { content: slice, positions })
            })
            .collect()
    }

    fn sentence_ranges(content: &str) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();
        let mut start = 0usize;
        for (idx, ch) in content.chars().enumerate() {
            if matches!(ch, '。' | '.' | '?' | '!' | '？' | '！') {
                let end = idx + 1;
                if end > start {
                    ranges.push((start, end));
                }
                start = end;
            }
        }
        let total = content.chars().count();
        if start < total {
            ranges.push((start, total));
        }
        ranges
    }

    fn append_segment(
        current_slice: &mut String, current_len: &mut usize, segments: &mut Vec<Segment>, text: &str,
        positions: Vec<SlicePosition>,
    ) {
        if text.is_empty() {
            return;
        }
        let start = *current_len;
        current_slice.push_str(text);
        *current_len += text.chars().count();
        let end = *current_len;
        if !positions.is_empty() {
            segments.push(Segment { start, end, positions });
        }
    }

    fn append_separator(current_slice: &mut String, current_len: &mut usize) {
        current_slice.push_str("\n\n");
        *current_len += 2;
    }

    fn positions_from_item(item: &ContentItem) -> Vec<SlicePosition> {
        if item.bbox.len() == 4 {
            let bbox = [item.bbox[0], item.bbox[1], item.bbox[2], item.bbox[3]];
            vec![SlicePosition { page_idx: item.page_idx, bbox, sheet_name: None, row_num: None }]
        } else {
            Vec::new()
        }
    }

    fn positions_for_range(segments: &[Segment], start: usize, end: usize) -> Vec<SlicePosition> {
        let mut set: HashSet<SlicePosition> = HashSet::new();
        for segment in segments {
            if segment.end > start && segment.start < end {
                for position in &segment.positions {
                    set.insert(position.clone());
                }
            }
        }
        set.into_iter().collect()
    }

    async fn clone_file_data(&self, source: &File, target: &File) -> anyhow::Result<()> {
        if source.id == target.id {
            anyhow::bail!("Source and target file ids are identical");
        }
        if source.status != 1 {
            anyhow::bail!("Source file {} is not processed", source.id);
        }

        let reuse_log = format!("Reusing parsed data from file {}", source.id);
        sqlx::query("UPDATE files SET status = 2, log = ?, updated_at = strftime('%s','now') WHERE id = ?")
            .bind(&reuse_log)
            .bind(target.id)
            .execute(&self.pool)
            .await?;

        self.cleanup_processing_file_data_with_retry(target.id, 3).await?;

        let pdf_rows = self.fetch_pdf_content_rows(source.id).await?;
        let mut slice_rows = self.fetch_slice_rows(source.id).await?;
        let slice_ids: Vec<i64> = slice_rows.iter().map(|row| row.id).collect();
        let slice_positions = self.fetch_slice_position_rows(&slice_ids).await?;
        let (image_jobs, image_mapping) = self.prepare_image_jobs(&pdf_rows, source.id, target.id);
        let meta_image_paths = if pdf_rows.is_empty() {
            collect_image_raw_paths_for_files(&self.pool, &[source.id]).await?
        } else {
            Vec::new()
        };
        let (meta_image_jobs, meta_image_mapping, target_meta_image_paths) =
            Self::prepare_raw_image_jobs(&meta_image_paths, source.id, target.id);
        if !meta_image_mapping.is_empty() {
            for row in &mut slice_rows {
                row.content = Self::rewrite_custom_image_refs(&row.content, &meta_image_mapping);
            }
        }
        let mut source_for_reindex = source.clone();
        if !meta_image_mapping.is_empty()
            && let Some(content) = source_for_reindex.content.clone() {
                source_for_reindex.content = Some(Self::rewrite_custom_image_refs(&content, &meta_image_mapping));
            }

        let mut tx = self.pool.begin().await?;
        self.insert_pdf_rows(&mut tx, target.id, &pdf_rows, &image_mapping).await?;
        let cloned_slices = self.insert_slice_rows(&mut tx, target.id, &slice_rows).await?;
        self.insert_slice_positions(&mut tx, &cloned_slices, &slice_positions).await?;
        tx.commit().await?;

        self.copy_image_files(&image_jobs).await?;
        self.copy_image_files(&meta_image_jobs).await?;
        self.copy_converted_pdf(source.id, target.id).await?;

        if pdf_rows.is_empty() {
            update_file_custom_image_meta(&self.pool, target.id, &target_meta_image_paths, "reuse_custom_images")
                .await?;
        }

        let full_content = self.reindex_cloned_slices(target, &cloned_slices, &source_for_reindex).await?;

        self.search_engine.reload_readers()?;

        let final_log = format!("Reused parsed data from file {}", source.id);
        sqlx::query(
            "UPDATE files SET status = 1, content = ?, log = ?, updated_at = strftime('%s','now') WHERE id = ?",
        )
        .bind(&full_content)
        .bind(&final_log)
        .bind(target.id)
        .execute(&self.pool)
        .await?;

        let mut updated_file = target.clone();
        updated_file.content = Some(full_content.clone());
        self.maybe_build_knowledge_graph(&updated_file).await;

        Ok(())
    }

    async fn fetch_pdf_content_rows(&self, file_id: i64) -> anyhow::Result<Vec<PdfContentRow>> {
        let sql = "SELECT page_idx, bbox, text, text_level, img_path, table_body FROM pdf_contents WHERE file_id = ? ORDER BY id";
        let rows = sqlx::query_as::<_, PdfContentRow>(sql).bind(file_id).fetch_all(&self.pool).await?;
        Ok(rows)
    }

    async fn fetch_slice_rows(&self, file_id: i64) -> anyhow::Result<Vec<SliceRow>> {
        let sql = "SELECT id, content FROM slices WHERE file_id = ? ORDER BY id";
        let rows = sqlx::query_as::<_, SliceRow>(sql).bind(file_id).fetch_all(&self.pool).await?;
        Ok(rows)
    }

    async fn fetch_slice_position_rows(&self, slice_ids: &[i64]) -> anyhow::Result<Vec<SlicePositionRecord>> {
        if slice_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut all_rows = Vec::new();
        let chunk_size = 400;
        for chunk in slice_ids.chunks(chunk_size) {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "SELECT slice_id, page_idx, x1, y1, x2, y2, sheet_name, row_num FROM slice_positions WHERE slice_id IN (",
            );
            let mut separated = qb.separated(", ");
            for slice_id in chunk {
                separated.push_bind(slice_id);
            }
            qb.push(") ORDER BY slice_id, id");
            let rows = qb.build_query_as::<SlicePositionRecord>().fetch_all(&self.pool).await?;
            all_rows.extend(rows);
        }
        Ok(all_rows)
    }

    fn prepare_image_jobs(
        &self, rows: &[PdfContentRow], source_id: i64, target_id: i64,
    ) -> (Vec<(String, String)>, HashMap<String, String>) {
        let mut jobs = Vec::new();
        let mut mapping = HashMap::new();
        for row in rows {
            if let Some(path) = &row.img_path {
                if mapping.contains_key(path) {
                    continue;
                }
                let new_path = Self::remap_image_name(path, source_id, target_id);
                jobs.push((path.clone(), new_path.clone()));
                mapping.insert(path.clone(), new_path);
            }
        }
        (jobs, mapping)
    }

    fn prepare_raw_image_jobs(
        paths: &[String], source_id: i64, target_id: i64,
    ) -> RawImageJobs {
        let mut jobs = Vec::new();
        let mut mapping = HashMap::new();
        let mut target_paths = Vec::new();
        let mut seen = HashSet::new();
        for path in paths {
            let trimmed = path.trim();
            if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
                continue;
            }
            let new_path = Self::remap_image_name(trimmed, source_id, target_id);
            mapping.insert(trimmed.to_string(), new_path.clone());
            jobs.push((trimmed.to_string(), new_path.clone()));
            target_paths.push(new_path);
        }
        (jobs, mapping, target_paths)
    }

    async fn insert_pdf_rows(
        &self, tx: &mut sqlx::Transaction<'_, Sqlite>, target_file_id: i64, rows: &[PdfContentRow],
        image_mapping: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let binds_per_row = 7;
        let batch_size = std::cmp::max(1, 999 / binds_per_row);
        for chunk in rows.chunks(batch_size) {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "INSERT INTO pdf_contents(file_id, page_idx, bbox, text, text_level, img_path, table_body) ",
            );
            qb.push_values(chunk.iter(), |mut b, row| {
                let new_img_path = row
                    .img_path
                    .as_ref()
                    .and_then(|path| image_mapping.get(path))
                    .cloned()
                    .or_else(|| row.img_path.clone());
                b.push_bind(target_file_id)
                    .push_bind(row.page_idx)
                    .push_bind(row.bbox.clone())
                    .push_bind(row.text.clone())
                    .push_bind(row.text_level)
                    .push_bind(new_img_path)
                    .push_bind(row.table_body.clone());
            });
            qb.build().execute(&mut **tx).await?;
        }
        Ok(())
    }

    async fn insert_slice_rows(
        &self, tx: &mut sqlx::Transaction<'_, Sqlite>, target_file_id: i64, rows: &[SliceRow],
    ) -> anyhow::Result<Vec<ClonedSlice>> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }

        let binds_per_row = 2_usize;
        let max_vars = 999_usize;
        let batch_size = std::cmp::max(1, max_vars / binds_per_row);
        let mut cloned = Vec::with_capacity(rows.len());
        for chunk in rows.chunks(batch_size) {
            let mut qb = QueryBuilder::<Sqlite>::new("INSERT INTO slices (file_id, content) ");
            qb.push_values(chunk.iter(), |mut b, row| {
                b.push_bind(target_file_id).push_bind(&row.content);
            });
            qb.push(" RETURNING id");

            let inserted_ids: Vec<(i64,)> = qb.build_query_as().fetch_all(&mut **tx).await?;
            anyhow::ensure!(
                inserted_ids.len() == chunk.len(),
                "inserted cloned slice row count mismatch: expected {}, got {}",
                chunk.len(),
                inserted_ids.len()
            );

            for (row, (new_id,)) in chunk.iter().zip(inserted_ids) {
                cloned.push(ClonedSlice { old_id: row.id, new_id, content: row.content.clone() });
            }
        }

        Ok(cloned)
    }

    async fn insert_slice_positions(
        &self, tx: &mut sqlx::Transaction<'_, Sqlite>, cloned_slices: &[ClonedSlice], positions: &[SlicePositionRecord],
    ) -> anyhow::Result<()> {
        if positions.is_empty() || cloned_slices.is_empty() {
            return Ok(());
        }
        let id_map: HashMap<i64, i64> = cloned_slices.iter().map(|slice| (slice.old_id, slice.new_id)).collect();
        let filtered: Vec<&SlicePositionRecord> =
            positions.iter().filter(|row| id_map.contains_key(&row.slice_id)).collect();
        if filtered.is_empty() {
            return Ok(());
        }
        let binds_per_row = 8;
        let batch_size = std::cmp::max(1, 999 / binds_per_row);
        for chunk in filtered.chunks(batch_size) {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "INSERT INTO slice_positions(slice_id, page_idx, x1, y1, x2, y2, sheet_name, row_num) ",
            );
            qb.push_values(chunk.iter(), |mut b, row| {
                let new_id = id_map[&row.slice_id];
                b.push_bind(new_id)
                    .push_bind(row.page_idx)
                    .push_bind(row.x1)
                    .push_bind(row.y1)
                    .push_bind(row.x2)
                    .push_bind(row.y2)
                    .push_bind(&row.sheet_name)
                    .push_bind(row.row_num);
            });
            qb.build().execute(&mut **tx).await?;
        }
        Ok(())
    }

    async fn copy_image_files(&self, jobs: &[(String, String)]) -> anyhow::Result<()> {
        for (old_path, new_path) in jobs {
            let Some(src_abs) = resolve_image_storage_path(old_path) else {
                warn!("Unable to resolve source image path {}, skipping reuse copy", old_path);
                continue;
            };
            let Some(dst_abs) = resolve_image_storage_path(new_path) else {
                warn!("Unable to resolve destination image path {}, skipping reuse copy", new_path);
                continue;
            };
            if let Some(parent) = std::path::Path::new(&dst_abs).parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::copy(&src_abs, &dst_abs).await?;
        }
        Ok(())
    }

    async fn copy_converted_pdf(&self, source_id: i64, target_id: i64) -> anyhow::Result<()> {
        let cfg = config::get();
        let pdf_dir = std::path::Path::new(&cfg.storage.pdf_path);
        let src_pdf = pdf_dir.join(format!("{}.pdf", source_id));
        match fs::try_exists(&src_pdf).await {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(err) => {
                warn!("Failed to check converted PDF existence for file {}: {}, skipping copy", source_id, err);
                return Ok(());
            }
        }
        fs::create_dir_all(pdf_dir).await?;
        let dst_pdf = pdf_dir.join(format!("{}.pdf", target_id));
        fs::copy(&src_pdf, &dst_pdf).await?;
        Ok(())
    }

    async fn reindex_cloned_slices(
        &self, target: &File, cloned_slices: &[ClonedSlice], source: &File,
    ) -> anyhow::Result<String> {
        let mut search_docs = Vec::new();
        for slice in cloned_slices {
            search_docs.push(tantivy_engine::Document::new(
                slice.new_id,
                target.id,
                target.kb_id,
                slice.content.clone(),
            ));
        }

        if !search_docs.is_empty() {
            let filename_lower = target.filename.to_lowercase();
            let is_image = Self::is_image_file(&filename_lower);
            let embeddings = if is_image {
                let embedding =
                    search::embedding::get_image_embedding_from_path(&target.path, Some(&target.filename)).await?;
                (0..search_docs.len()).map(|_| Some(embedding.clone())).collect()
            } else {
                vec![None; search_docs.len()]
            };
            self.search_engine.write_batch(search_docs, embeddings).await?;
        }

        let full_content = if let Some(content) = source.content.clone() {
            content
        } else if cloned_slices.is_empty() {
            String::new()
        } else {
            cloned_slices
                .iter()
                .map(|slice| slice.content.as_str())
                .filter(|content| !content.trim().is_empty())
                .map(|content| content.to_string())
                .collect::<Vec<_>>()
                .join("\n\n")
        };

        let index_full_content = if full_content.is_empty() {
            target.filename.clone()
        } else {
            format!("{}\n\n{}", target.filename, full_content)
        };
        self.search_engine
            .write_full(tantivy_engine::Document::new(target.id, target.id, target.kb_id, index_full_content))
            .await?;

        Ok(full_content)
    }

    fn remap_image_name(original: &str, source_id: i64, target_id: i64) -> String {
        let path = std::path::Path::new(original);
        let filename = path.file_name().and_then(|name| name.to_str()).unwrap_or(original);
        let prefix = format!("f{}_", source_id);
        let replacement = format!("f{}_", target_id);
        let stripped = if filename.starts_with(&prefix) {
            filename.trim_start_matches(&prefix).to_string()
        } else {
            filename.to_string()
        };
        let new_name = format!("{}{}", replacement, stripped);
        if let Some(parent) = path.parent() { parent.join(new_name).to_string_lossy().to_string() } else { new_name }
    }

    async fn maybe_build_knowledge_graph(&self, file: &File) {
        if !config::get().server.build_knowledge_graph {
            debug!("Knowledge graph building disabled by config, skipping file {}", file.id);
            return;
        }

        if let Err(e) = self.build_knowledge_graph(file).await {
            error!("Failed to build knowledge graph for file {}: {}", file.id, e);
            // 不影响主流程，仅记录错误
        }
    }

    /// 构建知识图谱（完全由LLM生成）
    async fn build_knowledge_graph(&self, file: &File) -> anyhow::Result<()> {
        info!("Building knowledge graph for file {}", file.id);

        // 1. 初始化LLM图谱提取器
        let llm_extractor = LLMGraphExtractor::from_env();

        if !llm_extractor.is_enabled() {
            warn!("LLM not enabled, skipping knowledge graph building for file {}", file.id);
            return Ok(());
        }

        info!("Using LLM to generate knowledge graph for file {}", file.id);

        // 2. 获取文件的所有切片
        let slices_sql = "SELECT id, content FROM slices WHERE file_id = ? ORDER BY id";
        let slices: Vec<(i64, String)> = sqlx::query_as(slices_sql).bind(file.id).fetch_all(&self.pool).await?;

        if slices.is_empty() {
            debug!("No slices found for file {}, skipping graph building", file.id);
            return Ok(());
        }

        // 3. 合并所有切片内容（限制长度避免超出LLM上下文）
        let mut combined_content = String::new();
        let max_content_length = 8000; // 限制总长度

        for (_, content) in &slices {
            if combined_content.len() + content.len() > max_content_length {
                break;
            }
            combined_content.push_str(content);
            combined_content.push_str("\n\n");
        }

        if combined_content.trim().is_empty() {
            debug!("No content to process for file {}", file.id);
            return Ok(());
        }

        // 4. 调用LLM提取知识图谱
        let context = format!("文件名: {}", file.filename);

        let (mut entities, mut relations) =
            match llm_extractor.extract_knowledge_graph(&combined_content, &context).await {
                Ok(result) => result,
                Err(e) => {
                    error!("LLM knowledge graph extraction failed for file {}: {}", file.id, e);
                    return Err(e);
                }
            };

        info!("LLM extracted {} entities and {} relations from file {}", entities.len(), relations.len(), file.id);

        // 5. 为实体和关系添加文件信息
        for entity in &mut entities {
            entity.file_id = Some(file.id);
            entity.kb_id = file.kb_id;
        }

        for relation in &mut relations {
            relation.file_id = Some(file.id);
        }

        // 6. 更新知识图谱
        let mut graph = KnowledgeGraph::load_from_db(self.pool.clone(), file.kb_id).await?;

        // 添加实体和关系
        graph.incremental_update(entities, relations).await?;

        // 7. 保存图快照
        graph.save_snapshot().await?;

        info!("Knowledge graph updated successfully for file {} (LLM-generated)", file.id);

        Ok(())
    }
}

pub async fn process_file_immediate(pool: SqlitePool, search_engine: SearchEngine, file_id: i64) -> anyhow::Result<()> {
    if is_parse_paused() {
        anyhow::bail!("parse is paused for index maintenance");
    }
    let sql = "SELECT * FROM files WHERE id = ?";
    let file: File = sqlx::query_as(sql).bind(file_id).fetch_one(&pool).await?;

    let processor = FileProcessor::new(pool, search_engine, 0);
    processor.process_file(&file).await
}

pub async fn process_file_immediate_skip_reuse(
    pool: SqlitePool, search_engine: SearchEngine, file_id: i64,
) -> anyhow::Result<()> {
    if is_parse_paused() {
        anyhow::bail!("parse is paused for index maintenance");
    }
    let sql = "SELECT * FROM files WHERE id = ?";
    let file: File = sqlx::query_as(sql).bind(file_id).fetch_one(&pool).await?;

    let processor = FileProcessor::new(pool, search_engine, 0);
    processor.process_file_skip_reuse(&file).await
}

pub async fn try_reuse_file_with_file(
    pool: SqlitePool, search_engine: SearchEngine, file: File,
) -> anyhow::Result<bool> {
    if is_parse_paused() {
        return Ok(false);
    }
    let processor = FileProcessor::new(pool, search_engine, 0);
    processor.try_reuse_existing_data(&file).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_content_item(img_path: &str) -> ContentItem {
        ContentItem {
            typ: "image".to_string(),
            bbox: vec![1, 2, 3, 4],
            page_idx: 0,
            text: None,
            text_level: None,
            text_format: None,
            img_path: Some(img_path.to_string()),
            image_caption: None,
            table_body: None,
            table_caption: None,
        }
    }

    #[test]
    fn custom_parse_normalization_rewrites_known_image_refs() -> anyhow::Result<()> {
        let mut images = HashMap::new();
        images.insert("img.png".to_string(), "raw-base64".to_string());
        let data = CustomParseData {
            slices: vec![CustomSlice {
                content: "see ![x](/api/v1/knowledge/files/img.png)".to_string(),
                positions: Vec::new(),
            }],
            full_content: Some("full /api/v1/knowledge/files/img.png".to_string()),
            images: Some(images),
            content_list: Some(vec![image_content_item("images/img.png")]),
        };

        let normalized = FileProcessor::normalize_custom_parse_data(42, data)?;

        assert!(normalized.images.contains_key("f42_img.png"));
        assert_eq!(normalized.content_list.as_ref().unwrap()[0].img_path.as_deref(), Some("f42_img.png"));
        assert!(normalized.slices[0].content.contains("/api/v1/knowledge/files/f42_img.png"));
        assert!(normalized.full_content.as_ref().unwrap().contains("/api/v1/knowledge/files/f42_img.png"));
        assert_eq!(normalized.image_paths, vec!["f42_img.png".to_string()]);
        Ok(())
    }

    #[test]
    fn custom_parse_normalization_preserves_unmapped_slice_refs() -> anyhow::Result<()> {
        let data = CustomParseData {
            slices: vec![CustomSlice {
                content: "legacy ![x](/api/v1/knowledge/files/legacy.png)".to_string(),
                positions: Vec::new(),
            }],
            full_content: None,
            images: None,
            content_list: None,
        };

        let normalized = FileProcessor::normalize_custom_parse_data(7, data)?;

        assert!(normalized.images.is_empty());
        assert!(normalized.slices[0].content.contains("/api/v1/knowledge/files/legacy.png"));
        assert_eq!(normalized.image_paths, vec!["legacy.png".to_string()]);
        Ok(())
    }
}
