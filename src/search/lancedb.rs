use std::{
    collections::HashSet, path::Path, sync::{
        Arc, atomic::{AtomicBool, AtomicU64, Ordering}
    }, time::{SystemTime, UNIX_EPOCH}
};

use anyhow::{Context, Result};
use arrow_array::{
    Array, ArrayRef, BooleanArray, Float32Array, Int64Array, RecordBatch, StringArray, builder::{FixedSizeListBuilder, Float32Builder}
};
use arrow_schema::{DataType, Field, Schema as ArrowSchema};
use futures::stream::StreamExt;
use lancedb::{
    Connection, Table, connect, index::{
        Index, scalar::{BTreeIndexBuilder, BitmapIndexBuilder}
    }, query::{ExecutableQuery, QueryBase, Select}, table::{CompactionOptions, NewColumnTransform, OptimizeAction, OptimizeOptions}
};
use log::{debug, info, warn};
use once_cell::sync::OnceCell;

use super::{embedding, tantivy_engine::SearchResultItem};
use crate::config;

static LANCEDB: OnceCell<Arc<Connection>> = OnceCell::new();
static LANCEDB_TABLE: OnceCell<Arc<Table>> = OnceCell::new();
static TABLE_NAME: &str = "documents";
static IS_DELETED_COLUMN: &str = "is_deleted";
static SEARCH_SELECT_COLUMNS: &[&str] = &["id", "file_id", "kb_id", "content", "_distance"];
static VECTOR_FAST_SEARCH_ENABLED: AtomicBool = AtomicBool::new(false);
static IMAGE_FAST_SEARCH_ENABLED: AtomicBool = AtomicBool::new(false);

/// 增量索引优化与 compact 之间的互斥锁。
static OPTIMIZE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// 单调递增版本号，用于防抖：只有最新版本才实际执行优化。
static OPTIMIZE_VERSION: AtomicU64 = AtomicU64::new(0);
/// 写操作后触发增量索引优化的防抖间隔。
const OPTIMIZE_DEBOUNCE_MS: u64 = 5000;

#[derive(Debug, Clone)]
pub struct CompactStats {
    pub deleted_rows: u64,
    pub deleted_rows_before: u64,
    pub deleted_rows_after: u64,
    pub total_rows_before: u64,
    pub total_rows_after: u64,
    pub size_before_bytes: u64,
    pub size_after_bytes: u64,
}

#[derive(Clone)]
pub struct Document {
    pub id: i64,                                // 切片 ID
    pub file_id: i64,                           // 文件 ID
    pub kb_id: Option<i64>,                     // 知识库 ID
    pub content: String,                        // 内容
    pub is_image: bool,                         // 是否为图片文件
    pub image_embedding: Option<Arc<Vec<f32>>>, // 图片 embedding
}

impl Document {
    pub fn new(id: i64, file_id: i64, kb_id: Option<i64>, content: String) -> Self {
        Document { id, file_id, kb_id, content, is_image: false, image_embedding: None }
    }

    pub fn with_image_embedding(mut self, embedding: Arc<Vec<f32>>) -> Self {
        self.is_image = true;
        self.image_embedding = Some(embedding);
        self
    }
}

/// 初始化 LanceDB。返回 `true` 表示表是新建或从损坏中恢复的，调用方应考虑从 SQLite 回填数据。
pub async fn init() -> Result<bool> {
    let cfg = config::get();
    let storage_path = &cfg.storage.lancedb_path;
    tokio::fs::create_dir_all(storage_path).await?;

    let db = connect(storage_path).execute().await?;
    LANCEDB.set(Arc::new(db)).map_err(|_| anyhow::anyhow!("Failed to initialize LanceDB"))?;

    // 创建表的 schema
    let schema = create_schema();

    // 检查表是否存在
    let conn = get_connection()?;
    let table_exists = conn
        .table_names()
        .execute()
        .await
        .map(|table_names| table_names.contains(&TABLE_NAME.to_string()))
        .unwrap_or(false);

    let (table, was_recreated) = if table_exists {
        match conn.open_table(TABLE_NAME).execute().await {
            Ok(table) => (table, false),
            Err(err) => {
                warn!(
                    "LanceDB table '{}' exists but cannot be opened (likely corrupted): {}. Attempting recovery...",
                    TABLE_NAME, err
                );
                (recover_table(storage_path, &schema).await?, true)
            }
        }
    } else {
        (create_empty_table(&schema).await?, true)
    };

    ensure_is_deleted_column(&table).await?;

    if let Err(err) = ensure_search_indices(&table).await {
        warn!("LanceDB ensure indices failed: {}", err);
    }
    if let Err(err) = refresh_fast_search_state_for_column(&table, "vector", &VECTOR_FAST_SEARCH_ENABLED).await {
        warn!("LanceDB refresh vector fast-search state failed: {}", err);
        VECTOR_FAST_SEARCH_ENABLED.store(false, Ordering::Relaxed);
    }
    if let Err(err) = refresh_fast_search_state_for_column(&table, "image_vector", &IMAGE_FAST_SEARCH_ENABLED).await {
        warn!("LanceDB refresh image_vector fast-search state failed: {}", err);
        IMAGE_FAST_SEARCH_ENABLED.store(false, Ordering::Relaxed);
    }

    LANCEDB_TABLE.set(Arc::new(table)).map_err(|_| anyhow::anyhow!("Failed to cache LanceDB table"))?;

    Ok(was_recreated)
}

async fn create_empty_table(schema: &Arc<ArrowSchema>) -> Result<Table> {
    let conn = get_connection()?;
    let empty_batch = create_empty_batch(schema)?;
    conn.create_table(TABLE_NAME, empty_batch)
        .execute()
        .await
        .with_context(|| format!("Failed to create LanceDB table '{}'", TABLE_NAME))
}

async fn recover_table(storage_path: &str, schema: &Arc<ArrowSchema>) -> Result<Table> {
    let conn = get_connection()?;

    // 先尝试用 LanceDB API 优雅删除损坏的表
    if let Err(err) = conn.drop_table(TABLE_NAME, &[]).await {
        warn!("LanceDB drop_table failed (will remove filesystem directory instead): {}", err);
    }

    // 清理文件系统残留，并备份到带时间戳的目录
    let table_dir = Path::new(storage_path).join(format!("{}.lance", TABLE_NAME));
    if table_dir.exists() {
        let backup_dir = {
            let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
            table_dir.with_file_name(format!("documents.lance.corrupted.{}", timestamp))
        };
        warn!("Moving corrupted LanceDB table from {} to {}", table_dir.display(), backup_dir.display());
        tokio::fs::rename(&table_dir, &backup_dir)
            .await
            .with_context(|| format!("Failed to move corrupted LanceDB table to {}", backup_dir.display()))?;
    }

    create_empty_table(schema).await
}

pub async fn write_documents(doc: Document) -> Result<()> {
    let table = get_table()?;
    let schema = create_schema();
    let batch = create_record_batch(vec![doc], &schema).await?;
    table.add(batch).execute().await?;
    on_write_may_need_optimize();
    Ok(())
}

/// 批量写入文档，一次性获取 embeddings 并写入
pub async fn write_documents_batch(docs: Vec<Document>) -> Result<()> {
    write_documents_batch_inner(docs, true).await
}

/// 重建期间批量写入，由调用方在全部完成后统一触发索引刷新。
pub async fn write_documents_batch_for_rebuild(docs: Vec<Document>) -> Result<()> {
    write_documents_batch_inner(docs, false).await
}

async fn write_documents_batch_inner(docs: Vec<Document>, schedule_optimize: bool) -> Result<()> {
    if docs.is_empty() {
        return Ok(());
    }
    let table = get_table()?;
    let schema = create_schema();
    let batch = create_record_batch(docs, &schema).await?;
    table.add(batch).execute().await?;
    if schedule_optimize {
        on_write_may_need_optimize();
    } else {
        set_fast_search_enabled(false, &VECTOR_FAST_SEARCH_ENABLED);
        set_fast_search_enabled(false, &IMAGE_FAST_SEARCH_ENABLED);
    }
    Ok(())
}

/// 重建写入完成后触发一次防抖索引刷新。
pub fn schedule_optimize_after_rebuild() {
    on_write_may_need_optimize();
}

pub async fn search(
    query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
) -> Result<Vec<SearchResultItem>> {
    if has_empty_scope(file_ids, kb_ids) {
        return Ok(Vec::new());
    }

    let cfg = config::get();
    let table = get_table()?;

    // 获取查询文本的 embedding
    let embedding_start = std::time::Instant::now();
    let query_vector = embedding::get_embedding(query).await?;
    debug!("LanceDB embedding {}ms", embedding_start.elapsed().as_millis());

    // 使用向量搜索
    let fast_search = vector_fast_search_enabled();
    let mut query_builder =
        table.query().nearest_to(query_vector)?.column("vector").select(Select::columns(SEARCH_SELECT_COLUMNS));
    if fast_search {
        query_builder = query_builder.fast_search();
    }

    // 应用过滤条件
    let filter_conditions = build_filter_conditions(false, file_ids, kb_ids);

    if !filter_conditions.is_empty() {
        query_builder = query_builder.only_if(filter_conditions.join(" AND "));
    }

    let execute_start = std::time::Instant::now();
    let mut result_stream = query_builder.limit(cfg.search.limit).execute().await?;
    debug!("LanceDB execute {}ms fast_search={}", execute_start.elapsed().as_millis(), fast_search);

    let mut search_results = Vec::with_capacity(cfg.search.limit);

    // 从 stream 中读取数据
    let stream_start = std::time::Instant::now();
    let mut first_batch_ms = None;
    let mut batch_count = 0usize;
    let mut row_count = 0usize;
    let mut decode_ms = 0u128;
    while let Some(batch_result) = result_stream.next().await {
        if first_batch_ms.is_none() {
            first_batch_ms = Some(stream_start.elapsed().as_millis());
        }
        let batch = batch_result?;
        row_count += batch.num_rows();
        let decode_start = std::time::Instant::now();
        decode_search_batch(&batch, &mut search_results)?;
        decode_ms += decode_start.elapsed().as_millis();
        batch_count += 1;
    }

    // 按分数排序
    search_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    debug!(
        "LanceDB stream total={}ms first_batch={}ms decode={}ms batches={} rows={}",
        stream_start.elapsed().as_millis(),
        first_batch_ms.unwrap_or(0),
        decode_ms,
        batch_count,
        row_count
    );

    Ok(search_results)
}

pub async fn search_image(
    query_vector: Vec<f32>, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
) -> Result<Vec<SearchResultItem>> {
    if has_empty_scope(file_ids, kb_ids) {
        return Ok(Vec::new());
    }

    let cfg = config::get();
    let table = get_table()?;

    let image_vector_dim = config::get().ai.image_embedding_dim;
    if query_vector.len() != image_vector_dim as usize {
        anyhow::bail!("Image embedding dimension mismatch: expected {}, got {}", image_vector_dim, query_vector.len());
    }

    let mut query_builder =
        table.query().nearest_to(query_vector)?.column("image_vector").select(Select::columns(SEARCH_SELECT_COLUMNS));

    if image_fast_search_enabled() {
        query_builder = query_builder.fast_search();
    }

    let filter_conditions = build_filter_conditions(true, file_ids, kb_ids);

    if !filter_conditions.is_empty() {
        query_builder = query_builder.only_if(filter_conditions.join(" AND "));
    }

    let mut result_stream = query_builder.limit(cfg.search.limit).execute().await?;

    let mut search_results = Vec::with_capacity(cfg.search.limit);
    while let Some(batch_result) = result_stream.next().await {
        let batch = batch_result?;
        decode_search_batch(&batch, &mut search_results)?;
    }

    search_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    Ok(search_results)
}

fn build_in_predicate(column: &str, ids: &[i64]) -> String {
    if ids.len() == 1 {
        format!("{column} = {}", ids[0])
    } else {
        let ids = ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ");
        format!("{column} IN ({ids})")
    }
}

async fn delete_by_predicate(predicate: String) -> Result<()> {
    delete_by_predicate_inner(predicate, true).await
}

async fn delete_by_predicate_inner(predicate: String, schedule_optimize: bool) -> Result<()> {
    let table = get_table()?;
    table.update().only_if(predicate).column(IS_DELETED_COLUMN, "true").execute().await?;
    if schedule_optimize {
        on_write_may_need_optimize();
    } else {
        set_fast_search_enabled(false, &VECTOR_FAST_SEARCH_ENABLED);
        set_fast_search_enabled(false, &IMAGE_FAST_SEARCH_ENABLED);
    }
    Ok(())
}

pub async fn delete_by_file(file_id: i64) -> Result<()> {
    delete_by_files(&[file_id]).await
}

pub async fn delete_by_files(file_ids: &[i64]) -> Result<()> {
    if file_ids.is_empty() {
        return Ok(());
    }
    delete_by_predicate(build_in_predicate("file_id", file_ids)).await
}

pub async fn delete_by_slices(slice_ids: &[i64]) -> Result<()> {
    if slice_ids.is_empty() {
        return Ok(());
    }
    delete_by_predicate(build_in_predicate("id", slice_ids)).await
}

/// 重建期间批量软删除多余切片，由调用方在全部完成后统一刷新索引。
pub async fn delete_by_slices_for_rebuild(slice_ids: &[i64]) -> Result<()> {
    if slice_ids.is_empty() {
        return Ok(());
    }
    delete_by_predicate_inner(build_in_predicate("id", slice_ids), false).await
}

pub async fn delete_by_kb(kb_id: i64) -> Result<()> {
    delete_by_kbs(&[kb_id]).await
}

pub async fn delete_by_kbs(kb_ids: &[i64]) -> Result<()> {
    if kb_ids.is_empty() {
        return Ok(());
    }
    delete_by_predicate(build_in_predicate("kb_id", kb_ids)).await
}

/// 清理已删除的记录，释放磁盘和内存空间
pub async fn compact() -> Result<CompactStats> {
    let _guard = OPTIMIZE_LOCK.lock().await;

    // 先禁用 fast_search，避免在 compact 期间使用未就绪的索引
    VECTOR_FAST_SEARCH_ENABLED.store(false, Ordering::Relaxed);
    IMAGE_FAST_SEARCH_ENABLED.store(false, Ordering::Relaxed);

    let table = get_table()?;
    let storage_path = config::get().storage.lancedb_path.clone();

    let size_before_bytes = {
        let path = storage_path.clone();
        tokio::task::spawn_blocking(move || dir_size_bytes(Path::new(&path))).await??
    };
    let total_rows_before = table.count_rows(None).await? as u64;
    let deleted_rows_before = table.count_rows(Some(format!("{} = true", IS_DELETED_COLUMN))).await? as u64;

    // 删除标记为已删除的记录
    let delete_predicate = format!("{} = true", IS_DELETED_COLUMN);
    table.delete(&delete_predicate).await?;

    // 执行 compact / index / prune，回收空间并强制清理旧版本
    table.optimize(OptimizeAction::Compact { options: CompactionOptions::default(), remap_options: None }).await?;
    table.optimize(OptimizeAction::Index(OptimizeOptions::default())).await?;
    table
        .optimize(OptimizeAction::Prune {
            older_than: Some(lancedb::table::Duration::minutes(10)),
            delete_unverified: Some(false),
            error_if_tagged_old_versions: Some(true),
        })
        .await?;

    refresh_fast_search_state_for_column(&table, "vector", &VECTOR_FAST_SEARCH_ENABLED).await?;
    refresh_fast_search_state_for_column(&table, "image_vector", &IMAGE_FAST_SEARCH_ENABLED).await?;

    let total_rows_after = table.count_rows(None).await? as u64;
    let deleted_rows_after = table.count_rows(Some(format!("{} = true", IS_DELETED_COLUMN))).await? as u64;
    let size_after_bytes = {
        let path = storage_path.clone();
        tokio::task::spawn_blocking(move || dir_size_bytes(Path::new(&path))).await??
    };

    Ok(CompactStats {
        deleted_rows: deleted_rows_before.saturating_sub(deleted_rows_after),
        deleted_rows_before,
        deleted_rows_after,
        total_rows_before,
        total_rows_after,
        size_before_bytes,
        size_after_bytes,
    })
}

fn get_connection() -> Result<Arc<Connection>> {
    LANCEDB.get().cloned().ok_or_else(|| anyhow::anyhow!("LanceDB not initialized"))
}

fn get_table() -> Result<Arc<Table>> {
    LANCEDB_TABLE.get().cloned().ok_or_else(|| anyhow::anyhow!("LanceDB table not initialized"))
}

/// 清空 LanceDB 中所有文档，用于从 SQLite 完全重建。
pub async fn clear_all_documents() -> Result<()> {
    let table = get_table()?;
    table.delete("true").await?;
    on_write_may_need_optimize();
    Ok(())
}

/// 一次性加载 LanceDB 中所有未软删除的文档 id。
pub async fn load_existing_ids(expected_count: u64) -> Result<HashSet<i64>> {
    let table = get_table()?;
    let predicate = format!("({} = false OR {} IS NULL)", IS_DELETED_COLUMN, IS_DELETED_COLUMN);
    let started_at = std::time::Instant::now();

    info!("Loading existing LanceDB document ids: expected={}", expected_count);
    let capacity = usize::try_from(expected_count).unwrap_or(0);
    let mut existing = HashSet::with_capacity(capacity);
    let mut stream = table.query().select(Select::columns(&["id"])).only_if(predicate).execute().await?;
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(10));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;

    loop {
        let next_batch = stream.next();
        tokio::pin!(next_batch);
        let batch_result = loop {
            tokio::select! {
                batch_result = &mut next_batch => break batch_result,
                _ = heartbeat.tick() => {
                    info!(
                        "Loading existing LanceDB document ids: loaded={}/{} elapsed={}s (waiting for next batch)",
                        existing.len(),
                        expected_count,
                        started_at.elapsed().as_secs()
                    );
                }
            }
        };

        let Some(batch_result) = batch_result else {
            break;
        };
        let batch = batch_result?;
        let id_array = batch
            .column_by_name("id")
            .and_then(|col| col.as_any().downcast_ref::<Int64Array>())
            .ok_or_else(|| anyhow::anyhow!("Missing or invalid id column"))?;
        for i in 0..batch.num_rows() {
            existing.insert(id_array.value(i));
        }
        let percent = if expected_count == 0 { 100.0 } else { existing.len() as f64 * 100.0 / expected_count as f64 };
        info!(
            "Loading existing LanceDB document ids: loaded={}/{} ({:.1}%) elapsed={}s",
            existing.len(),
            expected_count,
            percent,
            started_at.elapsed().as_secs()
        );
    }

    info!(
        "Loaded existing LanceDB document ids: count={} elapsed={}ms",
        existing.len(),
        started_at.elapsed().as_millis()
    );
    Ok(existing)
}

fn vector_fast_search_enabled() -> bool {
    VECTOR_FAST_SEARCH_ENABLED.load(Ordering::Relaxed)
}

fn image_fast_search_enabled() -> bool {
    IMAGE_FAST_SEARCH_ENABLED.load(Ordering::Relaxed)
}

fn set_fast_search_enabled(enabled: bool, flag: &AtomicBool) {
    flag.store(enabled, Ordering::Relaxed);
}

/// 写/删操作后立即调用：先禁用 fast_search 防止读到未索引数据，再触发一次防抖增量优化。
fn on_write_may_need_optimize() {
    set_fast_search_enabled(false, &VECTOR_FAST_SEARCH_ENABLED);
    set_fast_search_enabled(false, &IMAGE_FAST_SEARCH_ENABLED);

    let version = OPTIMIZE_VERSION.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(OPTIMIZE_DEBOUNCE_MS)).await;
        run_debounced_optimize(version).await;
    });
}

async fn run_debounced_optimize(version: u64) {
    // 如果版本号已被后续写操作超过，跳过本次优化
    if OPTIMIZE_VERSION.load(Ordering::Relaxed) != version {
        return;
    }

    let _guard = OPTIMIZE_LOCK.lock().await;

    // 再次检查版本，避免在等锁期间又有新写入
    if OPTIMIZE_VERSION.load(Ordering::Relaxed) != version {
        return;
    }

    let table = match get_table() {
        Ok(t) => t,
        Err(e) => {
            warn!("LanceDB debounced optimize failed to get table: {}", e);
            return;
        }
    };

    let optimize_start = std::time::Instant::now();
    match table.optimize(OptimizeAction::Index(OptimizeOptions::default())).await {
        Ok(stats) => {
            debug!("LanceDB debounced optimize_index done in {}ms: {:?}", optimize_start.elapsed().as_millis(), stats);
            if let Err(e) = refresh_fast_search_state_for_column(&table, "vector", &VECTOR_FAST_SEARCH_ENABLED).await {
                warn!("LanceDB refresh vector fast_search after optimize failed: {}", e);
            }
            if let Err(e) =
                refresh_fast_search_state_for_column(&table, "image_vector", &IMAGE_FAST_SEARCH_ENABLED).await
            {
                warn!("LanceDB refresh image_vector fast_search after optimize failed: {}", e);
            }
        }
        Err(e) => {
            warn!("LanceDB debounced optimize_index failed: {} ({}ms)", e, optimize_start.elapsed().as_millis());
        }
    }
}

async fn refresh_fast_search_state_for_column(table: &Table, column: &str, flag: &AtomicBool) -> Result<()> {
    let indices = table.list_indices().await?;
    let Some(index_config) = indices.iter().find(|idx| idx.columns.iter().any(|c| c == column)) else {
        set_fast_search_enabled(false, flag);
        warn!("LanceDB index missing on column={}; fast_search disabled", column);
        return Ok(());
    };

    let Some(stats) = table.index_stats(&index_config.name).await? else {
        set_fast_search_enabled(false, flag);
        warn!("LanceDB index stats missing for index={}; fast_search disabled", index_config.name);
        return Ok(());
    };

    let fast_search = stats.num_unindexed_rows == 0 && stats.num_indexed_rows > 0;
    set_fast_search_enabled(fast_search, flag);
    info!(
        "LanceDB index ready: column={} name={} type={:?} indexed_rows={} unindexed_rows={} fast_search={}",
        column, index_config.name, stats.index_type, stats.num_indexed_rows, stats.num_unindexed_rows, fast_search
    );

    Ok(())
}

fn has_empty_scope(file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>) -> bool {
    matches!(file_ids, Some(ids) if ids.is_empty()) || matches!(kb_ids, Some(ids) if ids.is_empty())
}

fn build_filter_conditions(is_image: bool, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>) -> Vec<String> {
    let mut filter_conditions = vec![
        format!("({} = false OR {} IS NULL)", IS_DELETED_COLUMN, IS_DELETED_COLUMN),
        format!("is_image = {}", is_image),
    ];

    if let Some(fids) = file_ids {
        let ids_str = fids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ");
        filter_conditions.push(format!("file_id IN ({})", ids_str));
    }
    if let Some(kids) = kb_ids {
        let ids_str = kids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ");
        filter_conditions.push(format!("kb_id IN ({})", ids_str));
    }

    filter_conditions
}

fn decode_search_batch(batch: &RecordBatch, search_results: &mut Vec<SearchResultItem>) -> Result<()> {
    let num_rows = batch.num_rows();

    let id_array = batch
        .column_by_name("id")
        .and_then(|col| col.as_any().downcast_ref::<Int64Array>())
        .ok_or_else(|| anyhow::anyhow!("Missing or invalid id column"))?;

    let file_id_array = batch
        .column_by_name("file_id")
        .and_then(|col| col.as_any().downcast_ref::<Int64Array>())
        .ok_or_else(|| anyhow::anyhow!("Missing or invalid file_id column"))?;

    let kb_id_array = batch.column_by_name("kb_id").and_then(|col| col.as_any().downcast_ref::<Int64Array>());

    let content_array = batch
        .column_by_name("content")
        .and_then(|col| col.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow::anyhow!("Missing or invalid content column"))?;

    let distance_array = batch.column_by_name("_distance").and_then(|col| col.as_any().downcast_ref::<Float32Array>());

    for i in 0..num_rows {
        let id = id_array.value(i);
        let file_id = file_id_array.value(i);
        let kb_id = kb_id_array.and_then(|arr| if arr.is_null(i) { None } else { Some(arr.value(i)) });
        let content = content_array.value(i).to_string();
        let score = distance_array.map_or(0.5, |arr| distance_to_score(arr.value(i)));

        search_results.push(SearchResultItem { id, file_id, kb_id, content, score });
    }

    Ok(())
}

fn distance_to_score(distance: f32) -> f32 {
    (1.0 / (1.0 + distance)).clamp(0.0, 1.0)
}

fn dir_size_bytes(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0u64;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total = total.saturating_add(dir_size_bytes(&entry.path())?);
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn create_schema() -> Arc<ArrowSchema> {
    let vector_dim = config::get().ai.embedding_dim;
    let image_vector_dim = config::get().ai.image_embedding_dim;
    Arc::new(ArrowSchema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("file_id", DataType::Int64, false),
        Field::new("kb_id", DataType::Int64, true),
        Field::new("content", DataType::Utf8, false),
        Field::new("is_image", DataType::Boolean, false),
        Field::new(IS_DELETED_COLUMN, DataType::Boolean, true),
        // 向量字段 - 从配置获取维度
        Field::new(
            "vector",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), vector_dim),
            false,
        ),
        Field::new(
            "image_vector",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), image_vector_dim),
            true,
        ),
    ]))
}

fn create_empty_batch(schema: &Arc<ArrowSchema>) -> Result<RecordBatch> {
    let vector_dim = config::get().ai.embedding_dim;
    let image_vector_dim = config::get().ai.image_embedding_dim;
    let id_array: ArrayRef = Arc::new(Int64Array::from(Vec::<i64>::new()));
    let file_id_array: ArrayRef = Arc::new(Int64Array::from(Vec::<i64>::new()));
    let kb_id_array: ArrayRef = Arc::new(Int64Array::from(Vec::<Option<i64>>::new()));
    let content_array: ArrayRef = Arc::new(StringArray::from(Vec::<String>::new()));
    let is_image_array: ArrayRef = Arc::new(BooleanArray::from(Vec::<bool>::new()));
    let is_deleted_array: ArrayRef = Arc::new(BooleanArray::from(Vec::<bool>::new()));

    // 创建空的向量数组
    let value_builder = Float32Builder::new();
    let mut list_builder = FixedSizeListBuilder::new(value_builder, vector_dim);
    let vector_array: ArrayRef = Arc::new(list_builder.finish());

    let image_value_builder = Float32Builder::new();
    let mut image_list_builder = FixedSizeListBuilder::new(image_value_builder, image_vector_dim);
    let image_vector_array: ArrayRef = Arc::new(image_list_builder.finish());

    Ok(RecordBatch::try_new(schema.clone(), vec![
        id_array,
        file_id_array,
        kb_id_array,
        content_array,
        is_image_array,
        is_deleted_array,
        vector_array,
        image_vector_array,
    ])?)
}

async fn create_record_batch(docs: Vec<Document>, schema: &Arc<ArrowSchema>) -> Result<RecordBatch> {
    let vector_dim = config::get().ai.embedding_dim;
    let image_vector_dim = config::get().ai.image_embedding_dim;
    let ids: Vec<i64> = docs.iter().map(|d| d.id).collect();
    let file_ids: Vec<i64> = docs.iter().map(|d| d.file_id).collect();
    let kb_ids: Vec<Option<i64>> = docs.iter().map(|d| d.kb_id).collect();
    let contents: Vec<String> = docs.iter().map(|d| d.content.clone()).collect();
    let is_images: Vec<bool> = docs.iter().map(|d| d.is_image).collect();
    let is_deleted: Vec<bool> = vec![false; docs.len()];

    // 先用 contents 计算 embedding，再把所有权转移给 StringArray，避免多一次 clone
    let embeddings = embedding::get_embeddings(&contents).await?;

    let id_array: ArrayRef = Arc::new(Int64Array::from(ids));
    let file_id_array: ArrayRef = Arc::new(Int64Array::from(file_ids));
    let kb_id_array: ArrayRef = Arc::new(Int64Array::from(kb_ids));
    let content_array: ArrayRef = Arc::new(StringArray::from(contents));
    let is_image_array: ArrayRef = Arc::new(BooleanArray::from(is_images));
    let is_deleted_array: ArrayRef = Arc::new(BooleanArray::from(is_deleted));

    let value_builder = Float32Builder::new();
    let mut list_builder = FixedSizeListBuilder::new(value_builder, vector_dim);

    for embedding_vec in embeddings {
        if embedding_vec.len() != vector_dim as usize {
            anyhow::bail!("Embedding dimension mismatch: expected {}, got {}", vector_dim, embedding_vec.len());
        }

        let values_builder = list_builder.values();
        for &value in &embedding_vec {
            values_builder.append_value(value);
        }
        list_builder.append(true);
    }

    let vector_array: ArrayRef = Arc::new(list_builder.finish());

    let image_value_builder = Float32Builder::new();
    let mut image_list_builder = FixedSizeListBuilder::new(image_value_builder, image_vector_dim);
    for doc in &docs {
        let values_builder = image_list_builder.values();
        if let Some(image_embedding) = &doc.image_embedding {
            if image_embedding.len() != image_vector_dim as usize {
                anyhow::bail!(
                    "Image embedding dimension mismatch: expected {}, got {}",
                    image_vector_dim,
                    image_embedding.len()
                );
            }
            for &value in image_embedding.as_ref() {
                values_builder.append_value(value);
            }
            image_list_builder.append(true);
        } else {
            for _ in 0..image_vector_dim {
                values_builder.append_value(0.0);
            }
            image_list_builder.append(false);
        }
    }
    let image_vector_array: ArrayRef = Arc::new(image_list_builder.finish());

    Ok(RecordBatch::try_new(schema.clone(), vec![
        id_array,
        file_id_array,
        kb_id_array,
        content_array,
        is_image_array,
        is_deleted_array,
        vector_array,
        image_vector_array,
    ])?)
}

async fn ensure_is_deleted_column(table: &lancedb::Table) -> Result<()> {
    let schema = table.schema().await?;
    if schema.field_with_name(IS_DELETED_COLUMN).is_err() {
        table
            .add_columns(
                NewColumnTransform::SqlExpressions(vec![(IS_DELETED_COLUMN.to_string(), "false".to_string())]),
                None,
            )
            .await?;
    }
    Ok(())
}

async fn ensure_search_indices(table: &Table) -> Result<()> {
    let existing_indices = table.list_indices().await?;
    let row_count = table.count_rows(None).await?;

    // 空表时跳过向量索引创建：LanceDB 的 IVF/PQ 索引需要 KMeans 训练，
    // 0 个向量会导致 `cannot train 1 centroids with 0 vectors` 报错。
    // 后续写入数据后，optimize 流程会自动创建向量索引。
    if row_count > 0 {
        if !has_index_on_column(&existing_indices, "vector") {
            info!("Creating LanceDB vector index on column=vector");
            table.create_index(&["vector"], Index::Auto).replace(false).execute().await?;
        }

        if !has_index_on_column(&existing_indices, "image_vector") {
            info!("Creating LanceDB vector index on column=image_vector");
            table.create_index(&["image_vector"], Index::Auto).replace(false).execute().await?;
        }
    } else {
        info!("LanceDB table is empty, deferring vector index creation until data is written");
    }

    let scalar_indices = [
        ("id", Index::BTree(BTreeIndexBuilder::default())),
        ("file_id", Index::BTree(BTreeIndexBuilder::default())),
        ("kb_id", Index::BTree(BTreeIndexBuilder::default())),
        ("is_deleted", Index::Bitmap(BitmapIndexBuilder::default())),
        ("is_image", Index::Bitmap(BitmapIndexBuilder::default())),
    ];
    for (column, index) in scalar_indices {
        if !has_index_on_column(&existing_indices, column) {
            info!("Creating LanceDB scalar index on column={}", column);
            table.create_index(&[column], index).replace(false).execute().await?;
        }
    }

    let refreshed_indices = table.list_indices().await?;
    debug!(
        "LanceDB indices: {}",
        refreshed_indices
            .iter()
            .map(|idx| format!("{}:{:?}:{:?}", idx.name, idx.index_type, idx.columns))
            .collect::<Vec<_>>()
            .join(", ")
    );

    Ok(())
}

fn has_index_on_column(indices: &[lancedb::index::IndexConfig], column: &str) -> bool {
    indices.iter().any(|idx| idx.columns.iter().any(|candidate| candidate == column))
}
