use std::{path::Path, sync::Arc};

use anyhow::Context;
use arrow_array::{
    ArrayRef, BooleanArray, Int64Array, RecordBatch, StringArray,
    builder::{FixedSizeListBuilder, Float32Builder},
};
use arrow_schema::{DataType, Schema as ArrowSchema};
use futures::stream::StreamExt;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool, sqlite::SqlitePoolOptions};
use tantivy::{
    TantivyDocument, Term,
    collector::TopDocs,
    doc,
    query::{BooleanQuery, Occur, Query, TermQuery},
    schema::{IndexRecordOption, Value as _},
};
use utoipa::ToSchema;

use crate::{config, search::tantivy_engine};

const EXPORT_MANIFEST_FILENAME: &str = "manifest.json";
const EXPORT_DB_FILENAME: &str = "app.sqlite";

/// Export manifest metadata
#[derive(Serialize, Deserialize, Clone, Debug, ToSchema)]
pub struct ExportManifest {
    pub version: String,
    pub export_type: String,
    pub kb_ids: Vec<i64>,
    pub kb_names: Vec<String>,
    pub exported_at: String,
    pub file_count: usize,
    pub slice_count: usize,
    pub node_count: usize,
    pub edge_count: usize,
    pub mention_count: usize,
    pub snapshot_count: usize,
    pub tantivy_doc_count: usize,
    pub tantivy_full_doc_count: usize,
    pub lancedb_row_count: usize,
}

impl Default for ExportManifest {
    fn default() -> Self {
        Self {
            version: "1.0".to_string(),
            export_type: "knowledge_base".to_string(),
            kb_ids: Vec::new(),
            kb_names: Vec::new(),
            exported_at: String::new(),
            file_count: 0,
            slice_count: 0,
            node_count: 0,
            edge_count: 0,
            mention_count: 0,
            snapshot_count: 0,
            tantivy_doc_count: 0,
            tantivy_full_doc_count: 0,
            lancedb_row_count: 0,
        }
    }
}

/// Export multiple knowledge bases (and optionally their children) to a self-contained directory.
///
/// The exported directory can be used as a standalone `HTKNOW_DATA_DIR`:
/// ```bash
/// HTKNOW_DATA_DIR=/path/to/export_dir cargo run
/// ```
pub async fn export_knowledge_bases(
    pool: &SqlitePool, src_kb_ids: &[i64], include_children: bool,
) -> anyhow::Result<String> {
    let total_start = std::time::Instant::now();
    let cfg = config::get();

    if src_kb_ids.is_empty() {
        anyhow::bail!("No knowledge base IDs provided for export");
    }

    // 1. Collect all KB IDs (including children if requested)
    let step_start = std::time::Instant::now();
    let target_kb_ids = collect_kb_ids(pool, src_kb_ids, include_children).await?;
    if target_kb_ids.is_empty() {
        anyhow::bail!("No knowledge bases found for export");
    }
    info!("Exporting knowledge bases: {:?}", target_kb_ids);

    // Collect ancestor KB IDs to preserve hierarchy
    let ancestor_kb_ids = collect_ancestor_kb_ids(pool, &target_kb_ids).await?;
    if !ancestor_kb_ids.is_empty() {
        info!("Including ancestor knowledge bases: {:?}", ancestor_kb_ids);
    }

    // Merge target and ancestor IDs for knowledge_bases table export
    let mut all_kb_ids = target_kb_ids.clone();
    all_kb_ids.extend(&ancestor_kb_ids);
    all_kb_ids.sort_unstable();
    all_kb_ids.dedup();

    // Get KB names for manifest (only target KBs)
    let kb_names = fetch_kb_names(pool, &target_kb_ids).await?;
    info!("[step] Collect KB IDs and names: {}ms", step_start.elapsed().as_millis());

    // 2. Create export directory
    let step_start = std::time::Instant::now();
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let dir_name = if src_kb_ids.len() == 1 {
        format!("kb_{}_{}", src_kb_ids[0], timestamp)
    } else {
        let ids_str = src_kb_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join("_");
        format!("kb_batch_{}_{}", ids_str, timestamp)
    };
    let export_dir =
        Path::new(&cfg.storage.files_path).parent().unwrap_or(Path::new("data")).join("exports").join(dir_name);
    tokio::fs::create_dir_all(&export_dir).await?;
    info!("Export directory: {}", export_dir.display());

    let export_dir_str = export_dir.to_string_lossy().to_string();

    // Create subdirectories
    let files_dir = export_dir.join("files");
    let pdfs_dir = export_dir.join("pdfs");
    let images_dir = export_dir.join("images");
    let tantivy_dir = export_dir.join("tantivy_index");
    let tantivy_full_dir = export_dir.join("tantivy_full_index");
    let lancedb_dir = export_dir.join("lancedb_data");

    tokio::fs::create_dir_all(&files_dir).await?;
    tokio::fs::create_dir_all(&pdfs_dir).await?;
    tokio::fs::create_dir_all(&images_dir).await?;
    tokio::fs::create_dir_all(&tantivy_dir).await?;
    tokio::fs::create_dir_all(&tantivy_full_dir).await?;
    tokio::fs::create_dir_all(&lancedb_dir).await?;
    info!("[step] Create directories: {}ms", step_start.elapsed().as_millis());

    // 3. Export SQLite data
    let step_start = std::time::Instant::now();
    let db_path = export_dir.join(EXPORT_DB_FILENAME);
    let export_pool = create_export_db_pool(&db_path).await?;
    init_export_schema(&export_pool).await?;

    // Disable foreign keys for faster parallel inserts (will re-enable before close)
    sqlx::query("PRAGMA foreign_keys = OFF").execute(&export_pool).await?;

    let file_ids = export_sqlite_data(pool, &export_pool, &target_kb_ids, &all_kb_ids).await?;
    info!("Exported {} files to SQLite in {}ms", file_ids.len(), step_start.elapsed().as_millis());

    // Re-enable foreign keys and close export pool before writing manifest
    sqlx::query("PRAGMA foreign_keys = ON").execute(&export_pool).await?;
    export_pool.close().await;

    // 4. Parallel: copy files, export Tantivy, export LanceDB, and count stats
    let step_start = std::time::Instant::now();

    let file_ids_for_copy = file_ids.clone();
    let file_ids_for_count = file_ids.clone();
    let target_kb_ids_for_tantivy = target_kb_ids.clone();
    let target_kb_ids_for_tantivy_full = target_kb_ids.clone();
    let target_kb_ids_for_lancedb = target_kb_ids.clone();
    let target_kb_ids_for_stats = target_kb_ids.clone();
    let pool_clone = pool.clone();

    let copy_files_future = async move {
        let s = std::time::Instant::now();
        let result = copy_files(&file_ids_for_copy, &files_dir, &pdfs_dir, &images_dir, &pool_clone).await;
        info!("[step] Copy files: {}ms", s.elapsed().as_millis());
        result
    };

    let tantivy_future = async move {
        let s = std::time::Instant::now();
        let count = export_tantivy_index(
            &cfg.search.tantivy_index_path,
            &tantivy_dir.to_string_lossy(),
            &target_kb_ids_for_tantivy,
        )
        .await
        .unwrap_or_else(|e| {
            warn!("Failed to export Tantivy slice index: {}", e);
            0
        });
        info!("[step] Export Tantivy slice index: {}ms", s.elapsed().as_millis());
        count
    };

    let tantivy_full_future = async move {
        let s = std::time::Instant::now();
        let count = export_tantivy_index(
            &cfg.search.tantivy_full_index_path,
            &tantivy_full_dir.to_string_lossy(),
            &target_kb_ids_for_tantivy_full,
        )
        .await
        .unwrap_or_else(|e| {
            warn!("Failed to export Tantivy full index: {}", e);
            0
        });
        info!("[step] Export Tantivy full index: {}ms", s.elapsed().as_millis());
        count
    };

    let lancedb_future = async move {
        let s = std::time::Instant::now();
        let count =
            export_lancedb(&cfg.storage.lancedb_path, &lancedb_dir.to_string_lossy(), &target_kb_ids_for_lancedb)
                .await
                .unwrap_or_else(|e| {
                    warn!("Failed to export LanceDB: {}", e);
                    0
                });
        info!("[step] Export LanceDB: {}ms", s.elapsed().as_millis());
        count
    };

    let stats_future = async move {
        let s = std::time::Instant::now();
        let slice_count = count_slices(pool, &file_ids_for_count).await.unwrap_or(0);
        let (node_count, edge_count, mention_count, snapshot_count) =
            count_graph_data(pool, &target_kb_ids_for_stats).await.unwrap_or((0, 0, 0, 0));
        info!("[step] Count stats: {}ms", s.elapsed().as_millis());
        (slice_count, node_count, edge_count, mention_count, snapshot_count)
    };

    let (copy_result, tantivy_doc_count, tantivy_full_doc_count, lancedb_row_count, stats) = tokio::join!(
        copy_files_future,
        tantivy_future,
        tantivy_full_future,
        lancedb_future,
        stats_future,
    );

    copy_result?;
    let (slice_count, node_count, edge_count, mention_count, snapshot_count) = stats;
    info!(
        "[step] Parallel export (files + tantivy + lancedb + stats) total: {}ms",
        step_start.elapsed().as_millis()
    );

    // 5. Write manifest
    let step_start = std::time::Instant::now();
    let file_count = file_ids.len();

    let manifest = ExportManifest {
        version: "1.0".to_string(),
        export_type: "knowledge_base".to_string(),
        kb_ids: target_kb_ids.clone(),
        kb_names,
        exported_at: chrono::Utc::now().to_rfc3339(),
        file_count,
        slice_count,
        node_count,
        edge_count,
        mention_count,
        snapshot_count,
        tantivy_doc_count,
        tantivy_full_doc_count,
        lancedb_row_count,
    };

    let manifest_path = export_dir.join(EXPORT_MANIFEST_FILENAME);
    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    tokio::fs::write(&manifest_path, manifest_json).await?;
    info!("[step] Write manifest: {}ms", step_start.elapsed().as_millis());

    info!(
        "Export completed in {}ms: {} files, {} slices, {} tantivy docs, {} lancedb rows to {}",
        total_start.elapsed().as_millis(),
        file_count,
        slice_count,
        tantivy_doc_count,
        lancedb_row_count,
        export_dir.display()
    );

    Ok(export_dir_str)
}

async fn collect_kb_ids(pool: &SqlitePool, root_kb_ids: &[i64], include_children: bool) -> anyhow::Result<Vec<i64>> {
    if root_kb_ids.is_empty() {
        return Ok(Vec::new());
    }

    if include_children {
        let mut all_ids: Vec<i64> = Vec::new();
        for root_id in root_kb_ids {
            let ids: Vec<i64> = sqlx::query_scalar(
                r#"
                WITH RECURSIVE descendants AS (
                    SELECT id FROM knowledge_bases WHERE id = ?
                    UNION ALL
                    SELECT kb.id FROM knowledge_bases kb
                    INNER JOIN descendants d ON kb.parent_id = d.id
                )
                SELECT id FROM descendants ORDER BY id
                "#,
            )
            .bind(root_id)
            .fetch_all(pool)
            .await?;
            all_ids.extend(ids);
        }
        all_ids.sort_unstable();
        all_ids.dedup();
        Ok(all_ids)
    } else {
        let mut ids = root_kb_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }
}

/// Collect all ancestor KB IDs for the given KB IDs to preserve hierarchy.
async fn collect_ancestor_kb_ids(pool: &SqlitePool, kb_ids: &[i64]) -> anyhow::Result<Vec<i64>> {
    if kb_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut ancestors = Vec::new();
    let mut seen: std::collections::HashSet<i64> = kb_ids.iter().copied().collect();
    let mut current_batch: Vec<i64> = kb_ids.to_vec();

    while !current_batch.is_empty() {
        let mut next_batch = Vec::new();
        for chunk in current_batch.chunks(SQLITE_BATCH_SIZE) {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "SELECT DISTINCT parent_id FROM knowledge_bases WHERE parent_id IS NOT NULL AND id IN ("
            );
            let mut separated = qb.separated(", ");
            for id in chunk {
                separated.push_bind(id);
            }
            qb.push(")");
            let parent_ids: Vec<Option<i64>> = qb.build_query_scalar().fetch_all(pool).await?;
            for pid in parent_ids.into_iter().flatten() {
                if seen.insert(pid) {
                    ancestors.push(pid);
                    next_batch.push(pid);
                }
            }
        }
        current_batch = next_batch;
    }

    ancestors.sort_unstable();
    Ok(ancestors)
}

async fn fetch_kb_names(pool: &SqlitePool, kb_ids: &[i64]) -> anyhow::Result<Vec<String>> {
    if kb_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for chunk in kb_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT name FROM knowledge_bases WHERE id IN (");
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push(") ORDER BY id");
        let chunk_names: Vec<String> = qb.build_query_scalar().fetch_all(pool).await?;
        names.extend(chunk_names);
    }
    Ok(names)
}

async fn create_export_db_pool(db_path: &Path) -> anyhow::Result<SqlitePool> {
    let db_url = format!("sqlite://{}", db_path.display());
    let connect_options = db_url
        .parse::<sqlx::sqlite::SqliteConnectOptions>()?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);
    // Single connection: SQLite write is serial anyway; multiple connections just create lock contention.
    let pool = SqlitePoolOptions::new().max_connections(1).connect_with(connect_options).await?;
    Ok(pool)
}

async fn init_export_schema(pool: &SqlitePool) -> anyhow::Result<()> {
    let init_sql = include_str!("init.sql");
    for (idx, sql) in init_sql.split(';').enumerate() {
        let sql = sql.trim();
        if sql.is_empty() {
            continue;
        }
        sqlx::query(sql).execute(pool).await.with_context(|| format!("init.sql statement {}", idx + 1))?;
    }
    Ok(())
}

/// Export SQLite data. Returns the list of exported file IDs.
/// `target_kb_ids` are used for data export; `all_kb_ids` includes ancestors for hierarchy preservation.
async fn export_sqlite_data(
    src_pool: &SqlitePool, dst_pool: &SqlitePool, target_kb_ids: &[i64], all_kb_ids: &[i64],
) -> anyhow::Result<Vec<i64>> {
    let step_start = std::time::Instant::now();

    // Export knowledge_bases (including ancestors to preserve hierarchy)
    do_export_knowledge_bases(src_pool, dst_pool, all_kb_ids).await?;
    info!("  [sqlite] knowledge_bases: {}ms", step_start.elapsed().as_millis());

    // Export files (with path rewritten to relative)
    let s = std::time::Instant::now();
    let file_ids = export_files(src_pool, dst_pool, target_kb_ids).await?;
    info!("  [sqlite] files ({} ids): {}ms", file_ids.len(), s.elapsed().as_millis());

    if file_ids.is_empty() {
        // No files, export graph data by kb_id only
        let s = std::time::Instant::now();
        export_graph_nodes_by_kb_id(src_pool, dst_pool, target_kb_ids).await?;
        info!("  [sqlite] graph_nodes (by kb_id only): {}ms", s.elapsed().as_millis());

        let s = std::time::Instant::now();
        export_graph_edges(src_pool, dst_pool).await?;
        info!("  [sqlite] graph_edges: {}ms", s.elapsed().as_millis());

        let s = std::time::Instant::now();
        export_entity_mentions(src_pool, dst_pool).await?;
        info!("  [sqlite] entity_mentions: {}ms", s.elapsed().as_millis());

        let s = std::time::Instant::now();
        export_graph_snapshots(src_pool, dst_pool, target_kb_ids).await?;
        info!("  [sqlite] graph_snapshots: {}ms", s.elapsed().as_millis());

        return Ok(file_ids);
    }

    // Parallel export of independent tables that depend on file_ids
    let file_ids_clone1 = file_ids.clone();
    let file_ids_clone2 = file_ids.clone();
    let file_ids_clone3 = file_ids.clone();
    let target_kb_ids_clone = target_kb_ids.to_vec();

    let slices_future = async {
        let s = std::time::Instant::now();
        export_slices(src_pool, dst_pool, &file_ids).await?;
        info!("  [sqlite] slices: {}ms", s.elapsed().as_millis());
        anyhow::Ok(())
    };

    let slice_positions_future = async {
        let s = std::time::Instant::now();
        export_slice_positions(src_pool, dst_pool, &file_ids_clone1).await?;
        info!("  [sqlite] slice_positions: {}ms", s.elapsed().as_millis());
        anyhow::Ok(())
    };

    let pdf_contents_future = async {
        let s = std::time::Instant::now();
        export_pdf_contents(src_pool, dst_pool, &file_ids_clone2).await?;
        info!("  [sqlite] pdf_contents: {}ms", s.elapsed().as_millis());
        anyhow::Ok(())
    };

    let graph_future = async {
        let s = std::time::Instant::now();
        export_graph_nodes(src_pool, dst_pool, &target_kb_ids_clone, &file_ids_clone3).await?;
        info!("  [sqlite] graph_nodes: {}ms", s.elapsed().as_millis());

        let s = std::time::Instant::now();
        export_graph_edges(src_pool, dst_pool).await?;
        info!("  [sqlite] graph_edges: {}ms", s.elapsed().as_millis());

        let s = std::time::Instant::now();
        export_entity_mentions(src_pool, dst_pool).await?;
        info!("  [sqlite] entity_mentions: {}ms", s.elapsed().as_millis());

        let s = std::time::Instant::now();
        export_graph_snapshots(src_pool, dst_pool, &target_kb_ids_clone).await?;
        info!("  [sqlite] graph_snapshots: {}ms", s.elapsed().as_millis());

        anyhow::Ok(())
    };

    let (r1, r2, r3, r4) = tokio::join!(slices_future, slice_positions_future, pdf_contents_future, graph_future);
    r1?;
    r2?;
    r3?;
    r4?;

    info!("  [sqlite] all tables total: {}ms", step_start.elapsed().as_millis());
    Ok(file_ids)
}

async fn do_export_knowledge_bases(
    src_pool: &SqlitePool, dst_pool: &SqlitePool, kb_ids: &[i64],
) -> anyhow::Result<()> {
    if kb_ids.is_empty() {
        return Ok(());
    }
    for chunk in kb_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT id, user_id, user_name, name, description, kb_type, parent_id, is_public, parse_priority, created_at, updated_at \
             FROM knowledge_bases WHERE id IN (",
        );
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push(")");
        let rows = qb.build().fetch_all(src_pool).await?;
        if rows.is_empty() {
            continue;
        }

        let values: Vec<(i64, String, String, String, String, String, Option<i64>, i32, i32, i64, i64)> =
            rows.iter()
                .map(|row| {
                    (
                        row.get::<i64, _>("id"),
                        row.get::<String, _>("user_id"),
                        row.get::<String, _>("user_name"),
                        row.get::<String, _>("name"),
                        row.get::<String, _>("description"),
                        row.get::<String, _>("kb_type"),
                        row.get::<Option<i64>, _>("parent_id"),
                        row.get::<i32, _>("is_public"),
                        row.get::<i32, _>("parse_priority"),
                        row.get::<i64, _>("created_at"),
                        row.get::<i64, _>("updated_at"),
                    )
                })
                .collect();

        for values_chunk in values.chunks(INSERT_BATCH_SIZE) {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "INSERT INTO knowledge_bases \
                 (id, user_id, user_name, name, description, kb_type, parent_id, is_public, parse_priority, created_at, updated_at) ",
            );
            qb.push_values(values_chunk, |mut b, (id, uid, uname, name, desc, kb_type, pid, is_pub, prio, cat, uat)| {
                b.push_bind(id)
                    .push_bind(uid)
                    .push_bind(uname)
                    .push_bind(name)
                    .push_bind(desc)
                    .push_bind(kb_type)
                    .push_bind(pid)
                    .push_bind(is_pub)
                    .push_bind(prio)
                    .push_bind(cat)
                    .push_bind(uat);
            });
            qb.build().execute(dst_pool).await?;
        }
    }
    Ok(())
}

async fn export_files(src_pool: &SqlitePool, dst_pool: &SqlitePool, kb_ids: &[i64]) -> anyhow::Result<Vec<i64>> {
    if kb_ids.is_empty() {
        return Ok(Vec::new());
    }
    let cfg = config::get();
    let files_path_prefix = format!("{}/", cfg.storage.files_path);

    let mut file_ids = Vec::new();

    for chunk in kb_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT * FROM files WHERE kb_id IN (");
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push(")");

        let rows = qb.build().fetch_all(src_pool).await?;
        if rows.is_empty() {
            continue;
        }

        let values: Vec<(i64, String, String, String, String, String, i64, Option<String>, String, i32, String, String, Option<i64>, i32, i32, Option<String>, i64, i64)> =
            rows.iter()
                .map(|row| {
                    let id: i64 = row.get("id");
                    let path: String = row.get("path");
                    let relative_path = if path.starts_with(&files_path_prefix) {
                        format!("data/files/{}", &path[files_path_prefix.len()..])
                    } else if let Some(filename) = Path::new(&path).file_name() {
                        format!("data/files/{}", filename.to_string_lossy())
                    } else {
                        path.clone()
                    };
                    file_ids.push(id);
                    (
                        id,
                        row.get::<String, _>("user_id"),
                        row.get::<String, _>("user_name"),
                        row.get::<String, _>("hash"),
                        row.get::<String, _>("filename"),
                        relative_path,
                        row.get::<i64, _>("size"),
                        row.get::<Option<String>, _>("content"),
                        row.get::<String, _>("tags"),
                        row.get::<i32, _>("status"),
                        row.get::<String, _>("log"),
                        row.get::<String, _>("slice_type"),
                        row.get::<Option<i64>, _>("kb_id"),
                        row.get::<i32, _>("parse_priority"),
                        row.get::<i32, _>("is_public"),
                        row.get::<Option<String>, _>("meta"),
                        row.get::<i64, _>("created_at"),
                        row.get::<i64, _>("updated_at"),
                    )
                })
                .collect();

        for values_chunk in values.chunks(INSERT_BATCH_SIZE) {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "INSERT INTO files \
                 (id, user_id, user_name, hash, filename, path, size, content, tags, status, log, slice_type, \
                  kb_id, parse_priority, is_public, meta, created_at, updated_at) ",
            );
            qb.push_values(
                values_chunk,
                |mut b, (id, uid, uname, hash, filename, path, size, content, tags, status, log, stype, kb_id, prio, is_pub, meta, cat, uat)| {
                    b.push_bind(id)
                        .push_bind(uid)
                        .push_bind(uname)
                        .push_bind(hash)
                        .push_bind(filename)
                        .push_bind(path)
                        .push_bind(size)
                        .push_bind(content)
                        .push_bind(tags)
                        .push_bind(status)
                        .push_bind(log)
                        .push_bind(stype)
                        .push_bind(kb_id)
                        .push_bind(prio)
                        .push_bind(is_pub)
                        .push_bind(meta)
                        .push_bind(cat)
                        .push_bind(uat);
                },
            );
            qb.build().execute(dst_pool).await?;
        }
    }

    Ok(file_ids)
}

const SQLITE_BATCH_SIZE: usize = 900;
/// Max rows per INSERT to stay under SQLite's 999 variable limit.
/// files table has 18 columns => 999/18 = 55; use 50 to be safe.
const INSERT_BATCH_SIZE: usize = 50;

async fn export_slices(src_pool: &SqlitePool, dst_pool: &SqlitePool, file_ids: &[i64]) -> anyhow::Result<()> {
    if file_ids.is_empty() {
        return Ok(());
    }
    for chunk in file_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT id, file_id, content, created_at, updated_at FROM slices WHERE file_id IN (",
        );
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push(")");
        let rows = qb.build().fetch_all(src_pool).await?;
        if rows.is_empty() {
            continue;
        }

        let values: Vec<(i64, i64, String, i64, i64)> = rows
            .iter()
            .map(|row| {
                (
                    row.get::<i64, _>("id"),
                    row.get::<i64, _>("file_id"),
                    row.get::<String, _>("content"),
                    row.get::<i64, _>("created_at"),
                    row.get::<i64, _>("updated_at"),
                )
            })
            .collect();

        for values_chunk in values.chunks(INSERT_BATCH_SIZE) {
            let mut qb = QueryBuilder::<Sqlite>::new("INSERT INTO slices (id, file_id, content, created_at, updated_at) ");
            qb.push_values(values_chunk, |mut b, (id, file_id, content, created_at, updated_at)| {
                b.push_bind(id).push_bind(file_id).push_bind(content).push_bind(created_at).push_bind(updated_at);
            });
            qb.build().execute(dst_pool).await?;
        }
    }
    Ok(())
}

async fn export_slice_positions(
    src_pool: &SqlitePool, dst_pool: &SqlitePool, file_ids: &[i64],
) -> anyhow::Result<()> {
    if file_ids.is_empty() {
        return Ok(());
    }
    for chunk in file_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT sp.id, sp.slice_id, sp.page_idx, sp.x1, sp.y1, sp.x2, sp.y2, sp.sheet_name, sp.row_num, sp.created_at \
             FROM slice_positions sp \
             JOIN slices s ON s.id = sp.slice_id \
             WHERE s.file_id IN (",
        );
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push(")");
        let rows = qb.build().fetch_all(src_pool).await?;
        if rows.is_empty() {
            continue;
        }

        let values: Vec<(i64, i64, i32, i32, i32, i32, i32, Option<String>, Option<i32>, i64)> = rows
            .iter()
            .map(|row| {
                (
                    row.get::<i64, _>("id"),
                    row.get::<i64, _>("slice_id"),
                    row.get::<i32, _>("page_idx"),
                    row.get::<i32, _>("x1"),
                    row.get::<i32, _>("y1"),
                    row.get::<i32, _>("x2"),
                    row.get::<i32, _>("y2"),
                    row.get::<Option<String>, _>("sheet_name"),
                    row.get::<Option<i32>, _>("row_num"),
                    row.get::<i64, _>("created_at"),
                )
            })
            .collect();

        for values_chunk in values.chunks(INSERT_BATCH_SIZE) {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "INSERT INTO slice_positions (id, slice_id, page_idx, x1, y1, x2, y2, sheet_name, row_num, created_at) ",
            );
            qb.push_values(
                values_chunk,
                |mut b, (id, slice_id, page_idx, x1, y1, x2, y2, sheet_name, row_num, created_at)| {
                    b.push_bind(id)
                        .push_bind(slice_id)
                        .push_bind(page_idx)
                        .push_bind(x1)
                        .push_bind(y1)
                        .push_bind(x2)
                        .push_bind(y2)
                        .push_bind(sheet_name)
                        .push_bind(row_num)
                        .push_bind(created_at);
                },
            );
            qb.build().execute(dst_pool).await?;
        }
    }
    Ok(())
}

async fn export_pdf_contents(
    src_pool: &SqlitePool, dst_pool: &SqlitePool, file_ids: &[i64],
) -> anyhow::Result<()> {
    if file_ids.is_empty() {
        return Ok(());
    }
    let cfg = config::get();
    let images_path_prefix = format!("{}/", cfg.storage.images_path);

    for chunk in file_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT * FROM pdf_contents WHERE file_id IN (");
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push(")");

        let rows = qb.build().fetch_all(src_pool).await?;
        if rows.is_empty() {
            continue;
        }

        let values: Vec<(i64, i64, i32, Option<String>, Option<String>, Option<i32>, Option<String>, Option<String>, i64, i64)> =
            rows.iter()
                .map(|row| {
                    let img_path: Option<String> = row.get("img_path");
                    let relative_img_path = img_path.map(|p| {
                        if p.starts_with(&images_path_prefix) { p[images_path_prefix.len()..].to_string() } else { p }
                    });
                    (
                        row.get::<i64, _>("id"),
                        row.get::<i64, _>("file_id"),
                        row.get::<i32, _>("page_idx"),
                        row.get::<Option<String>, _>("bbox"),
                        row.get::<Option<String>, _>("text"),
                        row.get::<Option<i32>, _>("text_level"),
                        relative_img_path,
                        row.get::<Option<String>, _>("table_body"),
                        row.get::<i64, _>("created_at"),
                        row.get::<i64, _>("updated_at"),
                    )
                })
                .collect();

        for values_chunk in values.chunks(INSERT_BATCH_SIZE) {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "INSERT INTO pdf_contents \
                 (id, file_id, page_idx, bbox, text, text_level, img_path, table_body, created_at, updated_at) ",
            );
            qb.push_values(
                values_chunk,
                |mut b, (id, file_id, page_idx, bbox, text, text_level, img_path, table_body, created_at, updated_at)| {
                    b.push_bind(id)
                        .push_bind(file_id)
                        .push_bind(page_idx)
                        .push_bind(bbox)
                        .push_bind(text)
                        .push_bind(text_level)
                        .push_bind(img_path)
                        .push_bind(table_body)
                        .push_bind(created_at)
                        .push_bind(updated_at);
                },
            );
            qb.build().execute(dst_pool).await?;
        }
    }

    Ok(())
}

async fn export_graph_nodes(
    src_pool: &SqlitePool, dst_pool: &SqlitePool, kb_ids: &[i64], file_ids: &[i64],
) -> anyhow::Result<()> {
    // First export by kb_id (this is the primary source)
    export_graph_nodes_by_kb_id(src_pool, dst_pool, kb_ids).await?;

    // Then export by file_id with INSERT OR IGNORE to avoid duplicates
    if !file_ids.is_empty() {
        for chunk in file_ids.chunks(SQLITE_BATCH_SIZE) {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "SELECT id, name, entity_type, properties, embedding, file_id, kb_id, is_public, created_at, updated_at \
                 FROM graph_nodes WHERE file_id IN (",
            );
            let mut separated = qb.separated(", ");
            for id in chunk {
                separated.push_bind(id);
            }
            qb.push(")");
            let rows = qb.build().fetch_all(src_pool).await?;
            if rows.is_empty() {
                continue;
            }

            let values: Vec<(i64, String, String, Option<String>, Option<Vec<u8>>, Option<i64>, Option<i64>, i32, i64, i64)> =
                rows.iter()
                    .map(|row| {
                        (
                            row.get::<i64, _>("id"),
                            row.get::<String, _>("name"),
                            row.get::<String, _>("entity_type"),
                            row.get::<Option<String>, _>("properties"),
                            row.get::<Option<Vec<u8>>, _>("embedding"),
                            row.get::<Option<i64>, _>("file_id"),
                            row.get::<Option<i64>, _>("kb_id"),
                            row.get::<i32, _>("is_public"),
                            row.get::<i64, _>("created_at"),
                            row.get::<i64, _>("updated_at"),
                        )
                    })
                    .collect();

            for values_chunk in values.chunks(INSERT_BATCH_SIZE) {
                let mut qb = QueryBuilder::<Sqlite>::new(
                    "INSERT OR IGNORE INTO graph_nodes \
                     (id, name, entity_type, properties, embedding, file_id, kb_id, is_public, created_at, updated_at) ",
                );
                qb.push_values(
                    values_chunk,
                    |mut b, (id, name, entity_type, properties, embedding, file_id, kb_id, is_public, created_at, updated_at)| {
                        b.push_bind(id)
                            .push_bind(name)
                            .push_bind(entity_type)
                            .push_bind(properties)
                            .push_bind(embedding)
                            .push_bind(file_id)
                            .push_bind(kb_id)
                            .push_bind(is_public)
                            .push_bind(created_at)
                            .push_bind(updated_at);
                    },
                );
                qb.build().execute(dst_pool).await?;
            }
        }
    }
    Ok(())
}

async fn export_graph_nodes_by_kb_id(
    src_pool: &SqlitePool, dst_pool: &SqlitePool, kb_ids: &[i64],
) -> anyhow::Result<()> {
    if kb_ids.is_empty() {
        return Ok(());
    }
    for chunk in kb_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT id, name, entity_type, properties, embedding, file_id, kb_id, is_public, created_at, updated_at \
             FROM graph_nodes WHERE kb_id IN (",
        );
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push(")");
        let rows = qb.build().fetch_all(src_pool).await?;
        if rows.is_empty() {
            continue;
        }

        let values: Vec<(i64, String, String, Option<String>, Option<Vec<u8>>, Option<i64>, Option<i64>, i32, i64, i64)> =
            rows.iter()
                .map(|row| {
                    (
                        row.get::<i64, _>("id"),
                        row.get::<String, _>("name"),
                        row.get::<String, _>("entity_type"),
                        row.get::<Option<String>, _>("properties"),
                        row.get::<Option<Vec<u8>>, _>("embedding"),
                        row.get::<Option<i64>, _>("file_id"),
                        row.get::<Option<i64>, _>("kb_id"),
                        row.get::<i32, _>("is_public"),
                        row.get::<i64, _>("created_at"),
                        row.get::<i64, _>("updated_at"),
                    )
                })
                .collect();

        for values_chunk in values.chunks(INSERT_BATCH_SIZE) {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "INSERT INTO graph_nodes \
                 (id, name, entity_type, properties, embedding, file_id, kb_id, is_public, created_at, updated_at) ",
            );
            qb.push_values(
                values_chunk,
                |mut b, (id, name, entity_type, properties, embedding, file_id, kb_id, is_public, created_at, updated_at)| {
                    b.push_bind(id)
                        .push_bind(name)
                        .push_bind(entity_type)
                        .push_bind(properties)
                        .push_bind(embedding)
                        .push_bind(file_id)
                        .push_bind(kb_id)
                        .push_bind(is_public)
                        .push_bind(created_at)
                        .push_bind(updated_at);
                },
            );
            qb.build().execute(dst_pool).await?;
        }
    }
    Ok(())
}

async fn export_graph_edges(src_pool: &SqlitePool, dst_pool: &SqlitePool) -> anyhow::Result<()> {
    // Get exported node IDs from dst_pool
    let node_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM graph_nodes").fetch_all(dst_pool).await?;
    if node_ids.is_empty() {
        return Ok(());
    }
    let node_set: std::collections::HashSet<i64> = node_ids.iter().copied().collect();

    for chunk in node_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT id, source_node_id, target_node_id, relation_type, properties, weight, file_id, created_at \
             FROM graph_edges WHERE source_node_id IN (",
        );
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push(")");
        let rows = qb.build().fetch_all(src_pool).await?;
        if rows.is_empty() {
            continue;
        }

        let values: Vec<(i64, i64, i64, String, Option<String>, Option<f64>, Option<i64>, i64)> = rows
            .iter()
            .filter_map(|row| {
                let target_id: i64 = row.get("target_node_id");
                if node_set.contains(&target_id) {
                    Some((
                        row.get::<i64, _>("id"),
                        row.get::<i64, _>("source_node_id"),
                        target_id,
                        row.get::<String, _>("relation_type"),
                        row.get::<Option<String>, _>("properties"),
                        row.get::<Option<f64>, _>("weight"),
                        row.get::<Option<i64>, _>("file_id"),
                        row.get::<i64, _>("created_at"),
                    ))
                } else {
                    None
                }
            })
            .collect();

        if values.is_empty() {
            continue;
        }

        for values_chunk in values.chunks(INSERT_BATCH_SIZE) {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "INSERT INTO graph_edges \
                 (id, source_node_id, target_node_id, relation_type, properties, weight, file_id, created_at) ",
            );
            qb.push_values(
                values_chunk,
                |mut b, (id, source, target, relation_type, properties, weight, file_id, created_at)| {
                    b.push_bind(id)
                        .push_bind(source)
                        .push_bind(target)
                        .push_bind(relation_type)
                        .push_bind(properties)
                        .push_bind(weight)
                        .push_bind(file_id)
                        .push_bind(created_at);
                },
            );
            qb.build().execute(dst_pool).await?;
        }
    }
    Ok(())
}

async fn export_entity_mentions(src_pool: &SqlitePool, dst_pool: &SqlitePool) -> anyhow::Result<()> {
    // Get exported node IDs and slice IDs from dst_pool
    let node_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM graph_nodes").fetch_all(dst_pool).await?;
    let slice_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM slices").fetch_all(dst_pool).await?;
    if node_ids.is_empty() || slice_ids.is_empty() {
        return Ok(());
    }
    let slice_set: std::collections::HashSet<i64> = slice_ids.iter().copied().collect();

    for chunk in node_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT id, node_id, slice_id, start_offset, end_offset, context, created_at \
             FROM entity_mentions WHERE node_id IN (",
        );
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push(")");
        let rows = qb.build().fetch_all(src_pool).await?;
        if rows.is_empty() {
            continue;
        }

        let values: Vec<(i64, i64, i64, Option<i64>, Option<i64>, Option<String>, i64)> = rows
            .iter()
            .filter_map(|row| {
                let slice_id: i64 = row.get("slice_id");
                if slice_set.contains(&slice_id) {
                    Some((
                        row.get::<i64, _>("id"),
                        row.get::<i64, _>("node_id"),
                        slice_id,
                        row.get::<Option<i64>, _>("start_offset"),
                        row.get::<Option<i64>, _>("end_offset"),
                        row.get::<Option<String>, _>("context"),
                        row.get::<i64, _>("created_at"),
                    ))
                } else {
                    None
                }
            })
            .collect();

        if values.is_empty() {
            continue;
        }

        for values_chunk in values.chunks(INSERT_BATCH_SIZE) {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "INSERT INTO entity_mentions \
                 (id, node_id, slice_id, start_offset, end_offset, context, created_at) ",
            );
            qb.push_values(
                values_chunk,
                |mut b, (id, node_id, slice_id, start_offset, end_offset, context, created_at)| {
                    b.push_bind(id)
                        .push_bind(node_id)
                        .push_bind(slice_id)
                        .push_bind(start_offset)
                        .push_bind(end_offset)
                        .push_bind(context)
                        .push_bind(created_at);
                },
            );
            qb.build().execute(dst_pool).await?;
        }
    }
    Ok(())
}

async fn export_graph_snapshots(
    src_pool: &SqlitePool, dst_pool: &SqlitePool, kb_ids: &[i64],
) -> anyhow::Result<()> {
    if kb_ids.is_empty() {
        return Ok(());
    }
    for chunk in kb_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT id, kb_id, graph_data, node_count, edge_count, version, created_at \
             FROM graph_snapshots WHERE kb_id IN (",
        );
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push(")");
        let rows = qb.build().fetch_all(src_pool).await?;
        if rows.is_empty() {
            continue;
        }

        let values: Vec<(i64, Option<i64>, Vec<u8>, Option<i32>, Option<i32>, Option<i32>, i64)> = rows
            .iter()
            .map(|row| {
                (
                    row.get::<i64, _>("id"),
                    row.get::<Option<i64>, _>("kb_id"),
                    row.get::<Vec<u8>, _>("graph_data"),
                    row.get::<Option<i32>, _>("node_count"),
                    row.get::<Option<i32>, _>("edge_count"),
                    row.get::<Option<i32>, _>("version"),
                    row.get::<i64, _>("created_at"),
                )
            })
            .collect();

        for values_chunk in values.chunks(INSERT_BATCH_SIZE) {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "INSERT INTO graph_snapshots (id, kb_id, graph_data, node_count, edge_count, version, created_at) ",
            );
            qb.push_values(
                values_chunk,
                |mut b, (id, kb_id, graph_data, node_count, edge_count, version, created_at)| {
                    b.push_bind(id)
                        .push_bind(kb_id)
                        .push_bind(graph_data)
                        .push_bind(node_count)
                        .push_bind(edge_count)
                        .push_bind(version)
                        .push_bind(created_at);
                },
            );
            qb.build().execute(dst_pool).await?;
        }
    }
    Ok(())
}

async fn copy_files(
    file_ids: &[i64], files_dir: &Path, pdfs_dir: &Path, images_dir: &Path, pool: &SqlitePool,
) -> anyhow::Result<()> {
    let cfg = config::get();

    // Get file paths from database
    let mut file_paths = Vec::new();

    for chunk in file_ids.chunks(1000) {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT id, path FROM files WHERE id IN (");
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push(")");
        let rows = qb.build().fetch_all(pool).await?;
        for row in rows {
            let id: i64 = row.get("id");
            let path: String = row.get("path");
            file_paths.push((id, path));
        }
    }

    let total_files = file_paths.len();
    let copied_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let skipped_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    // Copy original files concurrently via spawn_blocking
    let file_handles: Vec<_> = file_paths
        .iter()
        .cloned()
        .map(|(_file_id, src_path)| {
            let files_dir = files_dir.to_path_buf();
            let copied = copied_count.clone();
            let skipped = skipped_count.clone();
            tokio::task::spawn_blocking(move || {
                let src = Path::new(&src_path);
                if !src.exists() {
                    warn!("Source file not found: {}", src_path);
                    skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
                let filename = src.file_name().unwrap_or(std::ffi::OsStr::new("unknown"));
                let dst = files_dir.join(filename);
                if let Err(e) = std::fs::copy(src, &dst) {
                    warn!("Failed to copy file {} to {}: {}", src_path, dst.display(), e);
                    skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                } else {
                    debug!("Copied file {} to {}", src_path, dst.display());
                    copied.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            })
        })
        .collect();
    futures::future::join_all(file_handles).await;

    // Copy PDFs concurrently via spawn_blocking
    let pdf_handles: Vec<_> = file_paths
        .iter()
        .cloned()
        .map(|(file_id, _src_path)| {
            let pdfs_dir = pdfs_dir.to_path_buf();
            let cfg_pdf_path = cfg.storage.pdf_path.clone();
            tokio::task::spawn_blocking(move || {
                let src_pdf = Path::new(&cfg_pdf_path).join(format!("{}.pdf", file_id));
                if src_pdf.exists() {
                    let dst_pdf = pdfs_dir.join(format!("{}.pdf", file_id));
                    if let Err(e) = std::fs::copy(&src_pdf, &dst_pdf) {
                        warn!("Failed to copy PDF {} to {}: {}", src_pdf.display(), dst_pdf.display(), e);
                    }
                }
            })
        })
        .collect();
    futures::future::join_all(pdf_handles).await;

    let copied = copied_count.load(std::sync::atomic::Ordering::Relaxed);
    let skipped = skipped_count.load(std::sync::atomic::Ordering::Relaxed);
    info!("Copied {}/{} original files, skipped {}", copied, total_files, skipped);

    // Copy images referenced by pdf_contents
    let s = std::time::Instant::now();
    copy_images(pool, images_dir, file_ids).await?;
    info!("  [files] Copy images: {}ms", s.elapsed().as_millis());

    Ok(())
}

async fn copy_images(pool: &SqlitePool, images_dir: &Path, file_ids: &[i64]) -> anyhow::Result<()> {
    if file_ids.is_empty() {
        return Ok(());
    }

    let cfg = config::get();
    let data_root = Path::new(&cfg.storage.images_path).parent();

    // 1. Collect image paths from pdf_contents (MinerU flow)
    let mut all_img_paths = Vec::new();
    for chunk in file_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT DISTINCT img_path FROM pdf_contents WHERE img_path IS NOT NULL AND img_path != '' AND file_id IN ("
        );
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push(")");
        let rows: Vec<(Option<String>,)> = qb.build_query_as().fetch_all(pool).await?;

        for (img_path_opt,) in rows {
            if let Some(img_path) = img_path_opt {
                let trimmed = img_path.trim();
                if !trimmed.is_empty() {
                    all_img_paths.push(trimmed.to_string());
                }
            }
        }
    }

    // 2. Extract image paths from slices of files without pdf_contents (custom parser flow)
    let custom_img_paths = extract_custom_image_paths(pool, file_ids).await?;
    all_img_paths.extend(custom_img_paths);

    if all_img_paths.is_empty() {
        return Ok(());
    }

    // Deduplicate
    let mut seen = std::collections::HashSet::new();
    all_img_paths.retain(|p| seen.insert(p.clone()));

    // 3. Copy all images concurrently via spawn_blocking
    let img_handles: Vec<_> = all_img_paths
        .into_iter()
        .map(|trimmed| {
            let images_dir = images_dir.to_path_buf();
            let cfg_images_path = cfg.storage.images_path.clone();
            let data_root = data_root.map(|p| p.to_path_buf());

            tokio::task::spawn_blocking(move || {
                let use_data_root = trimmed.contains('/') || trimmed.contains('\\');
                let src = if use_data_root {
                    let via_images_path = Path::new(&cfg_images_path).join(&trimmed);
                    if via_images_path.exists() {
                        via_images_path
                    } else if let Some(root) = data_root {
                        root.join(&trimmed)
                    } else {
                        via_images_path
                    }
                } else {
                    Path::new(&cfg_images_path).join(&trimmed)
                };

                if !src.exists() {
                    warn!("Source image not found: {}", src.display());
                    return false;
                }

                let dst = if use_data_root {
                    images_dir.parent().unwrap_or(&images_dir).join(&trimmed)
                } else {
                    images_dir.join(&trimmed)
                };

                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent).ok();
                }

                if let Err(e) = std::fs::copy(&src, &dst) {
                    warn!("Failed to copy image {} to {}: {}", src.display(), dst.display(), e);
                    false
                } else {
                    debug!("Copied image {} to {}", src.display(), dst.display());
                    true
                }
            })
        })
        .collect();

    let results = futures::future::join_all(img_handles).await;
    let copied = results.iter().filter(|r| r.as_ref().map_or(false, |v| *v)).count();
    let failed = results.len() - copied;
    info!("Copied {} images, {} failed", copied, failed);

    Ok(())
}

/// Extract image paths from slices of files that have no pdf_contents records.
/// Custom parser images are not recorded in pdf_contents; their references may be
/// embedded in slice.content (e.g. markdown `![alt](/api/v1/knowledge/files/xxx.jpg)`).
async fn extract_custom_image_paths(pool: &SqlitePool, file_ids: &[i64]) -> anyhow::Result<Vec<String>> {
    let mut custom_file_ids = Vec::new();

    // Find files without pdf_contents records, in batches
    for chunk in file_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT DISTINCT id FROM files WHERE id IN ("
        );
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push(") AND id NOT IN (SELECT DISTINCT file_id FROM pdf_contents WHERE file_id IN (");
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push("))");
        let ids: Vec<i64> = qb.build_query_scalar().fetch_all(pool).await?;
        custom_file_ids.extend(ids);
    }

    if custom_file_ids.is_empty() {
        return Ok(Vec::new());
    }

    // Extract image references from slice content, in batches
    let re = regex::Regex::new(r"/api/v1/knowledge/files/([^\s'\)>]+)")?;
    let mut img_paths = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for chunk in custom_file_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT content FROM slices WHERE file_id IN ("
        );
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push(")");
        let rows: Vec<(String,)> = qb.build_query_as().fetch_all(pool).await?;

        for (content,) in rows {
            for cap in re.captures_iter(&content) {
                if let Some(m) = cap.get(1) {
                    let img_name = m.as_str().trim();
                    if img_name.is_empty() || !seen.insert(img_name.to_string()) {
                        continue;
                    }
                    img_paths.push(img_name.to_string());
                }
            }
        }
    }

    Ok(img_paths)
}

async fn export_tantivy_index(src_path: &str, dst_path: &str, kb_ids: &[i64]) -> anyhow::Result<usize> {
    if kb_ids.is_empty() {
        return Ok(0);
    }

    let src_path = src_path.to_string();
    let dst_path = dst_path.to_string();
    let kb_ids = kb_ids.to_vec();

    tokio::task::spawn_blocking(move || {
        let step_start = std::time::Instant::now();
        let src_index = tantivy::Index::open_in_dir(&src_path)
            .with_context(|| format!("Failed to open source Tantivy index: {}", src_path))?;
        let src_schema = src_index.schema();
        let kb_id_field = src_schema.get_field("kb_id")?;

        let reader = src_index.reader()?;
        let searcher = reader.searcher();

        // Build a BooleanQuery with TermQuery for each kb_id — directly hits the inverted index
        let kb_queries: Vec<(Occur, Box<dyn Query>)> = kb_ids
            .iter()
            .map(|kb_id| {
                (
                    Occur::Should,
                    Box::new(TermQuery::new(
                        Term::from_field_i64(kb_id_field, *kb_id),
                        IndexRecordOption::Basic,
                    )) as Box<dyn Query>,
                )
            })
            .collect();
        let kb_query = BooleanQuery::new(kb_queries);
        let doc_limit = 10_000_000;
        let top_docs = searcher.search(&kb_query, &TopDocs::with_limit(doc_limit))?;

        if top_docs.len() == doc_limit {
            warn!("Tantivy export hit document limit of {}", doc_limit);
        }
        let scan_ms = step_start.elapsed().as_millis();
        info!("  [tantivy] Matched {} docs (scan {}ms)", top_docs.len(), scan_ms);

        // Create destination index
        let s = std::time::Instant::now();
        let (dst_schema, dst_index) = tantivy_engine::init_with_path(&dst_path)?;
        let writer_memory = config::get().search.tantivy_memory_mb * 1_000_000;
        let mut writer = dst_index.writer(writer_memory)?;
        writer.set_merge_policy(Box::new(tantivy::merge_policy::LogMergePolicy::default()));

        let id_field = src_schema.get_field("id")?;
        let file_id_field = src_schema.get_field("file_id")?;
        let content_field = src_schema.get_field("content")?;

        let dst_id_field = dst_schema.get_field("id")?;
        let dst_file_id_field = dst_schema.get_field("file_id")?;
        let dst_content_field = dst_schema.get_field("content")?;
        let dst_kb_id_field = dst_schema.get_field("kb_id")?;

        let mut count = 0;
        for (_, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)?;
            let doc_kb_id = doc.get_first(kb_id_field).and_then(|v| v.as_i64());

            let id = doc.get_first(id_field).and_then(|v| v.as_i64()).unwrap_or(0);
            let file_id = doc.get_first(file_id_field).and_then(|v| v.as_i64()).unwrap_or(0);
            let content = doc.get_first(content_field).and_then(|v| v.as_str()).unwrap_or("");

            let mut new_doc = doc! {
                dst_id_field => id,
                dst_file_id_field => file_id,
                dst_content_field => content,
            };
            if let Some(kb_id) = doc_kb_id {
                new_doc.add_i64(dst_kb_id_field, kb_id);
            }
            writer.add_document(new_doc)?;
            count += 1;
        }

        writer.commit()?;
        info!(
            "Exported {} Tantivy documents to {} (scan {}ms, write {}ms, total {}ms)",
            count,
            dst_path,
            scan_ms,
            s.elapsed().as_millis(),
            step_start.elapsed().as_millis()
        );
        anyhow::Ok(count)
    })
    .await
    .map_err(|e| anyhow::anyhow!("Tantivy export task panicked: {}", e))?
}

async fn export_lancedb(src_path: &str, dst_path: &str, kb_ids: &[i64]) -> anyhow::Result<usize> {
    use lancedb::query::{ExecutableQuery, QueryBase};

    if kb_ids.is_empty() {
        return Ok(0);
    }

    let step_start = std::time::Instant::now();

    let src_conn = lancedb::connect(src_path).execute().await?;
    let src_table = src_conn.open_table("documents").execute().await?;
    let schema = src_table.schema().await?;

    let dst_conn = lancedb::connect(dst_path).execute().await?;

    let mut all_batches = Vec::new();
    let mut total_count = 0;

    for chunk in kb_ids.chunks(SQLITE_BATCH_SIZE) {
        let predicate = if chunk.len() == 1 {
            format!("kb_id = {}", chunk[0])
        } else {
            format!("kb_id IN ({})", chunk.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", "))
        };

        let s = std::time::Instant::now();
        let mut stream = src_table.query().only_if(&predicate).limit(1_000_000).execute().await?;

        let mut chunk_count = 0;
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            chunk_count += batch.num_rows();
            total_count += batch.num_rows();
            all_batches.push(batch);
        }
        info!("  [lancedb] Query chunk ({} kb_ids, {} rows): {}ms", chunk.len(), chunk_count, s.elapsed().as_millis());
    }

    let s = std::time::Instant::now();
    if all_batches.is_empty() {
        // Create empty table with correct schema
        let empty_batch = create_empty_batch_for_schema(&schema)?;
        dst_conn.create_table("documents", empty_batch).execute().await?;
    } else {
        dst_conn.create_table("documents", all_batches.remove(0)).execute().await?;
        for batch in all_batches {
            let dst_table = dst_conn.open_table("documents").execute().await?;
            dst_table.add(batch).execute().await?;
        }
    }
    info!("  [lancedb] Write to destination: {}ms", s.elapsed().as_millis());

    info!("Exported {} LanceDB rows to {} (total {}ms)", total_count, dst_path, step_start.elapsed().as_millis());
    Ok(total_count)
}

fn create_empty_batch_for_schema(schema: &Arc<ArrowSchema>) -> anyhow::Result<RecordBatch> {
    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(schema.fields().len());

    for field in schema.fields() {
        let array: ArrayRef = match field.data_type() {
            DataType::Int64 => Arc::new(Int64Array::from(Vec::<i64>::new())),
            DataType::Utf8 => Arc::new(StringArray::from(Vec::<String>::new())),
            DataType::Boolean => Arc::new(BooleanArray::from(Vec::<bool>::new())),
            DataType::FixedSizeList(inner_field, dim) => match inner_field.data_type() {
                DataType::Float32 => {
                    let value_builder = Float32Builder::new();
                    let mut list_builder = FixedSizeListBuilder::new(value_builder, *dim);
                    Arc::new(list_builder.finish())
                }
                _ => anyhow::bail!("Unsupported inner type in FixedSizeList: {:?}", inner_field.data_type()),
            },
            _ => anyhow::bail!("Unsupported data type for empty batch: {:?}", field.data_type()),
        };
        arrays.push(array);
    }

    Ok(RecordBatch::try_new(schema.clone(), arrays)?)
}

async fn count_slices(pool: &SqlitePool, file_ids: &[i64]) -> anyhow::Result<usize> {
    if file_ids.is_empty() {
        return Ok(0);
    }
    let mut total = 0_i64;
    for chunk in file_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM slices WHERE file_id IN (");
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(id);
        }
        qb.push(")");
        let count: i64 = qb.build_query_scalar().fetch_one(pool).await?;
        total += count;
    }
    Ok(total as usize)
}

async fn count_graph_data(pool: &SqlitePool, kb_ids: &[i64]) -> anyhow::Result<(usize, usize, usize, usize)> {
    if kb_ids.is_empty() {
        return Ok((0, 0, 0, 0));
    }

    let mut node_count = 0_i64;
    for chunk in kb_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM graph_nodes WHERE kb_id IN (");
        let mut separated = qb.separated(", ");
        for id in chunk { separated.push_bind(id); }
        qb.push(")");
        let cnt: i64 = qb.build_query_scalar().fetch_one(pool).await?;
        node_count += cnt;
    }

    let mut edge_count = 0_i64;
    for chunk in kb_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT COUNT(*) FROM graph_edges e WHERE EXISTS (SELECT 1 FROM graph_nodes n WHERE n.id = e.source_node_id AND n.kb_id IN ("
        );
        let mut separated = qb.separated(", ");
        for id in chunk { separated.push_bind(id); }
        qb.push("))");
        let cnt: i64 = qb.build_query_scalar().fetch_one(pool).await?;
        edge_count += cnt;
    }

    let mut mention_count = 0_i64;
    for chunk in kb_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT COUNT(*) FROM entity_mentions m WHERE EXISTS (SELECT 1 FROM graph_nodes n WHERE n.id = m.node_id AND n.kb_id IN ("
        );
        let mut separated = qb.separated(", ");
        for id in chunk { separated.push_bind(id); }
        qb.push("))");
        let cnt: i64 = qb.build_query_scalar().fetch_one(pool).await?;
        mention_count += cnt;
    }

    let mut snapshot_count = 0_i64;
    for chunk in kb_ids.chunks(SQLITE_BATCH_SIZE) {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM graph_snapshots WHERE kb_id IN (");
        let mut separated = qb.separated(", ");
        for id in chunk { separated.push_bind(id); }
        qb.push(")");
        let cnt: i64 = qb.build_query_scalar().fetch_one(pool).await?;
        snapshot_count += cnt;
    }

    Ok((node_count as usize, edge_count as usize, mention_count as usize, snapshot_count as usize))
}
