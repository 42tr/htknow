use std::{path::Path, sync::Arc};

use anyhow::Context;
use arrow_array::{ArrayRef, BooleanArray, Int64Array, RecordBatch, StringArray, builder::{FixedSizeListBuilder, Float32Builder}};
use arrow_schema::{DataType, Schema as ArrowSchema};
use futures::stream::StreamExt;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool, sqlite::SqlitePoolOptions};
use tantivy::{TantivyDocument, collector::TopDocs, doc, query::AllQuery, schema::Value as _};
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
    let start = std::time::Instant::now();
    let cfg = config::get();

    if src_kb_ids.is_empty() {
        anyhow::bail!("No knowledge base IDs provided for export");
    }

    // 1. Collect all KB IDs (including children if requested)
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

    // 2. Create export directory
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let dir_name = if src_kb_ids.len() == 1 {
        format!("kb_{}_{}", src_kb_ids[0], timestamp)
    } else {
        let ids_str = src_kb_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join("_");
        format!("kb_batch_{}_{}", ids_str, timestamp)
    };
    let export_dir = Path::new(&cfg.storage.files_path)
        .parent()
        .unwrap_or(Path::new("data"))
        .join("exports")
        .join(dir_name);
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

    // 3. Export SQLite data
    let db_path = export_dir.join(EXPORT_DB_FILENAME);
    let export_pool = create_export_db_pool(&db_path).await?;
    init_export_schema(&export_pool).await?;

    let file_ids = export_sqlite_data(pool, &export_pool, &target_kb_ids, &all_kb_ids).await?;
    info!("Exported {} files to SQLite", file_ids.len());

    // 4. Copy files (original files, PDFs, images)
    copy_files(&file_ids, &files_dir, &pdfs_dir, &images_dir, pool).await?;

    // 5. Export Tantivy indexes
    let tantivy_doc_count = export_tantivy_index(
        &cfg.search.tantivy_index_path,
        &tantivy_dir.to_string_lossy(),
        &target_kb_ids,
    )
    .await
    .unwrap_or_else(|e| {
        warn!("Failed to export Tantivy slice index: {}", e);
        0
    });

    let tantivy_full_doc_count = export_tantivy_index(
        &cfg.search.tantivy_full_index_path,
        &tantivy_full_dir.to_string_lossy(),
        &target_kb_ids,
    )
    .await
    .unwrap_or_else(|e| {
        warn!("Failed to export Tantivy full index: {}", e);
        0
    });

    // 6. Export LanceDB
    let lancedb_row_count = export_lancedb(&cfg.storage.lancedb_path, &lancedb_dir.to_string_lossy(), &target_kb_ids)
        .await
        .unwrap_or_else(|e| {
            warn!("Failed to export LanceDB: {}", e);
            0
        });

    // Close export pool before writing manifest
    export_pool.close().await;

    // 7. Write manifest
    let file_count = file_ids.len();
    let slice_count = count_slices(pool, &file_ids).await?;
    let (node_count, edge_count, mention_count, snapshot_count) = count_graph_data(pool, &target_kb_ids).await?;

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

    info!(
        "Export completed in {}ms: {} files, {} slices, {} tantivy docs, {} lancedb rows to {}",
        start.elapsed().as_millis(),
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
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new().max_connections(5).connect_with(connect_options).await?;
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
    // Export knowledge_bases (including ancestors to preserve hierarchy)
    do_export_knowledge_bases(src_pool, dst_pool, all_kb_ids).await?;

    // Export files (with path rewritten to relative)
    let file_ids = export_files(src_pool, dst_pool, target_kb_ids).await?;

    // Export slices
    export_slices(src_pool, dst_pool, &file_ids).await?;

    // Export slice_positions
    export_slice_positions(src_pool, dst_pool, &file_ids).await?;

    // Export pdf_contents
    export_pdf_contents(src_pool, dst_pool, &file_ids).await?;

    // Export graph data
    export_graph_nodes(src_pool, dst_pool, target_kb_ids, &file_ids).await?;
    export_graph_edges(src_pool, dst_pool).await?;
    export_entity_mentions(src_pool, dst_pool).await?;
    export_graph_snapshots(src_pool, dst_pool, target_kb_ids).await?;

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
        for row in rows {
            sqlx::query(
                "INSERT INTO knowledge_bases \
                 (id, user_id, user_name, name, description, kb_type, parent_id, is_public, parse_priority, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(row.get::<i64, _>("id"))
            .bind(row.get::<String, _>("user_id"))
            .bind(row.get::<String, _>("user_name"))
            .bind(row.get::<String, _>("name"))
            .bind(row.get::<String, _>("description"))
            .bind(row.get::<String, _>("kb_type"))
            .bind(row.get::<Option<i64>, _>("parent_id"))
            .bind(row.get::<i32, _>("is_public"))
            .bind(row.get::<i32, _>("parse_priority"))
            .bind(row.get::<i64, _>("created_at"))
            .bind(row.get::<i64, _>("updated_at"))
            .execute(dst_pool)
            .await?;
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

        for row in rows {
            let id: i64 = row.get("id");
            let path: String = row.get("path");
            let relative_path = if path.starts_with(&files_path_prefix) {
                format!("files/{}", &path[files_path_prefix.len()..])
            } else if let Some(filename) = Path::new(&path).file_name() {
                format!("files/{}", filename.to_string_lossy())
            } else {
                path.clone()
            };

            sqlx::query(
                "INSERT INTO files \
                 (id, user_id, user_name, hash, filename, path, size, content, tags, status, log, slice_type, \
                  kb_id, parse_priority, is_public, meta, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(row.get::<String, _>("user_id"))
            .bind(row.get::<String, _>("user_name"))
            .bind(row.get::<String, _>("hash"))
            .bind(row.get::<String, _>("filename"))
            .bind(relative_path)
            .bind(row.get::<i64, _>("size"))
            .bind(row.get::<Option<String>, _>("content"))
            .bind(row.get::<String, _>("tags"))
            .bind(row.get::<i32, _>("status"))
            .bind(row.get::<String, _>("log"))
            .bind(row.get::<String, _>("slice_type"))
            .bind(row.get::<Option<i64>, _>("kb_id"))
            .bind(row.get::<i32, _>("parse_priority"))
            .bind(row.get::<i32, _>("is_public"))
            .bind(row.get::<Option<String>, _>("meta"))
            .bind(row.get::<i64, _>("created_at"))
            .bind(row.get::<i64, _>("updated_at"))
            .execute(dst_pool)
            .await?;

            file_ids.push(id);
        }
    }

    Ok(file_ids)
}

const SQLITE_BATCH_SIZE: usize = 900;

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
        for row in rows {
            sqlx::query("INSERT INTO slices (id, file_id, content, created_at, updated_at) VALUES (?, ?, ?, ?, ?)")
                .bind(row.get::<i64, _>("id"))
                .bind(row.get::<i64, _>("file_id"))
                .bind(row.get::<String, _>("content"))
                .bind(row.get::<i64, _>("created_at"))
                .bind(row.get::<i64, _>("updated_at"))
                .execute(dst_pool)
                .await?;
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
        for row in rows {
            sqlx::query(
                "INSERT INTO slice_positions (id, slice_id, page_idx, x1, y1, x2, y2, sheet_name, row_num, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(row.get::<i64, _>("id"))
            .bind(row.get::<i64, _>("slice_id"))
            .bind(row.get::<i32, _>("page_idx"))
            .bind(row.get::<i32, _>("x1"))
            .bind(row.get::<i32, _>("y1"))
            .bind(row.get::<i32, _>("x2"))
            .bind(row.get::<i32, _>("y2"))
            .bind(row.get::<Option<String>, _>("sheet_name"))
            .bind(row.get::<Option<i32>, _>("row_num"))
            .bind(row.get::<i64, _>("created_at"))
            .execute(dst_pool)
            .await?;
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

        for row in rows {
            let img_path: Option<String> = row.get("img_path");
            let relative_img_path = img_path.map(|p| {
                if p.starts_with(&images_path_prefix) {
                    p[images_path_prefix.len()..].to_string()
                } else {
                    p
                }
            });

            sqlx::query(
                "INSERT INTO pdf_contents \
                 (id, file_id, page_idx, bbox, text, text_level, img_path, table_body, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(row.get::<i64, _>("id"))
            .bind(row.get::<i64, _>("file_id"))
            .bind(row.get::<i32, _>("page_idx"))
            .bind(row.get::<Option<String>, _>("bbox"))
            .bind(row.get::<Option<String>, _>("text"))
            .bind(row.get::<Option<i32>, _>("text_level"))
            .bind(relative_img_path)
            .bind(row.get::<Option<String>, _>("table_body"))
            .bind(row.get::<i64, _>("created_at"))
            .bind(row.get::<i64, _>("updated_at"))
            .execute(dst_pool)
            .await?;
        }
    }

    Ok(())
}

async fn export_graph_nodes(
    src_pool: &SqlitePool, dst_pool: &SqlitePool, kb_ids: &[i64], file_ids: &[i64],
) -> anyhow::Result<()> {
    if kb_ids.is_empty() && file_ids.is_empty() {
        return Ok(());
    }

    // Export by kb_id batches
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
        for row in rows {
            sqlx::query(
                "INSERT INTO graph_nodes \
                 (id, name, entity_type, properties, embedding, file_id, kb_id, is_public, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(row.get::<i64, _>("id"))
            .bind(row.get::<String, _>("name"))
            .bind(row.get::<String, _>("entity_type"))
            .bind(row.get::<Option<String>, _>("properties"))
            .bind(row.get::<Option<Vec<u8>>, _>("embedding"))
            .bind(row.get::<Option<i64>, _>("file_id"))
            .bind(row.get::<Option<i64>, _>("kb_id"))
            .bind(row.get::<i32, _>("is_public"))
            .bind(row.get::<i64, _>("created_at"))
            .bind(row.get::<i64, _>("updated_at"))
            .execute(dst_pool)
            .await?;
        }
    }

    // Export by file_id batches
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
        for row in rows {
            sqlx::query(
                "INSERT OR IGNORE INTO graph_nodes \
                 (id, name, entity_type, properties, embedding, file_id, kb_id, is_public, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(row.get::<i64, _>("id"))
            .bind(row.get::<String, _>("name"))
            .bind(row.get::<String, _>("entity_type"))
            .bind(row.get::<Option<String>, _>("properties"))
            .bind(row.get::<Option<Vec<u8>>, _>("embedding"))
            .bind(row.get::<Option<i64>, _>("file_id"))
            .bind(row.get::<Option<i64>, _>("kb_id"))
            .bind(row.get::<i32, _>("is_public"))
            .bind(row.get::<i64, _>("created_at"))
            .bind(row.get::<i64, _>("updated_at"))
            .execute(dst_pool)
            .await?;
        }
    }
    Ok(())
}

async fn export_graph_edges(src_pool: &SqlitePool, dst_pool: &SqlitePool) -> anyhow::Result<()> {
    // Get exported node IDs from dst_pool
    let node_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM graph_nodes")
        .fetch_all(dst_pool)
        .await?;
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
        for row in rows {
            let target_id: i64 = row.get("target_node_id");
            if node_set.contains(&target_id) {
                sqlx::query(
                    "INSERT INTO graph_edges \
                     (id, source_node_id, target_node_id, relation_type, properties, weight, file_id, created_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(row.get::<i64, _>("id"))
                .bind(row.get::<i64, _>("source_node_id"))
                .bind(target_id)
                .bind(row.get::<String, _>("relation_type"))
                .bind(row.get::<Option<String>, _>("properties"))
                .bind(row.get::<Option<f64>, _>("weight"))
                .bind(row.get::<Option<i64>, _>("file_id"))
                .bind(row.get::<i64, _>("created_at"))
                .execute(dst_pool)
                .await?;
            }
        }
    }
    Ok(())
}

async fn export_entity_mentions(src_pool: &SqlitePool, dst_pool: &SqlitePool) -> anyhow::Result<()> {
    // Get exported node IDs and slice IDs from dst_pool
    let node_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM graph_nodes")
        .fetch_all(dst_pool)
        .await?;
    let slice_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM slices")
        .fetch_all(dst_pool)
        .await?;
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
        for row in rows {
            let slice_id: i64 = row.get("slice_id");
            if slice_set.contains(&slice_id) {
                sqlx::query(
                    "INSERT INTO entity_mentions \
                     (id, node_id, slice_id, start_offset, end_offset, context, created_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(row.get::<i64, _>("id"))
                .bind(row.get::<i64, _>("node_id"))
                .bind(slice_id)
                .bind(row.get::<Option<i64>, _>("start_offset"))
                .bind(row.get::<Option<i64>, _>("end_offset"))
                .bind(row.get::<Option<String>, _>("context"))
                .bind(row.get::<i64, _>("created_at"))
                .execute(dst_pool)
                .await?;
            }
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
        for row in rows {
            sqlx::query(
                "INSERT INTO graph_snapshots (id, kb_id, graph_data, node_count, edge_count, version, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(row.get::<i64, _>("id"))
            .bind(row.get::<Option<i64>, _>("kb_id"))
            .bind(row.get::<Vec<u8>, _>("graph_data"))
            .bind(row.get::<Option<i32>, _>("node_count"))
            .bind(row.get::<Option<i32>, _>("edge_count"))
            .bind(row.get::<Option<i32>, _>("version"))
            .bind(row.get::<i64, _>("created_at"))
            .execute(dst_pool)
            .await?;
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
    let mut pdf_file_ids = Vec::new();

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
            pdf_file_ids.push(id);
        }
    }

    // Copy original files
    for (_file_id, src_path) in file_paths {
        let src = Path::new(&src_path);
        if !src.exists() {
            warn!("Source file not found: {}", src_path);
            continue;
        }
        let filename = src.file_name().unwrap_or(std::ffi::OsStr::new("unknown"));
        let dst = files_dir.join(filename);
        if let Err(e) = tokio::fs::copy(src, &dst).await {
            warn!("Failed to copy file {} to {}: {}", src_path, dst.display(), e);
        } else {
            debug!("Copied file {} to {}", src_path, dst.display());
        }
    }

    // Copy PDFs
    for file_id in pdf_file_ids {
        let src_pdf = Path::new(&cfg.storage.pdf_path).join(format!("{}.pdf", file_id));
        if src_pdf.exists() {
            let dst_pdf = pdfs_dir.join(format!("{}.pdf", file_id));
            if let Err(e) = tokio::fs::copy(&src_pdf, &dst_pdf).await {
                warn!("Failed to copy PDF {} to {}: {}", src_pdf.display(), dst_pdf.display(), e);
            }
        }
    }

    // Copy images referenced by pdf_contents
    copy_images(pool, images_dir, file_ids).await?;

    Ok(())
}

async fn copy_images(pool: &SqlitePool, images_dir: &Path, file_ids: &[i64]) -> anyhow::Result<()> {
    if file_ids.is_empty() {
        return Ok(());
    }

    let cfg = config::get();
    let data_root = Path::new(&cfg.storage.images_path).parent();

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
            let Some(img_path) = img_path_opt else { continue };
            let trimmed = img_path.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Resolve to absolute path (same logic as resolve_image_storage_path)
            let use_data_root = trimmed.contains('/') || trimmed.contains('\\');
            let src = if use_data_root {
                if let Some(root) = data_root {
                    root.join(trimmed)
                } else {
                    Path::new(&cfg.storage.images_path).join(trimmed)
                }
            } else {
                Path::new(&cfg.storage.images_path).join(trimmed)
            };

            if !src.exists() {
                warn!("Source image not found: {}", src.display());
                continue;
            }

            // Compute destination path
            let dst = if use_data_root {
                images_dir.parent().unwrap_or(images_dir).join(trimmed)
            } else {
                images_dir.join(trimmed)
            };

            if let Some(parent) = dst.parent() {
                tokio::fs::create_dir_all(parent).await.ok();
            }

            if let Err(e) = tokio::fs::copy(&src, &dst).await {
                warn!("Failed to copy image {} to {}: {}", src.display(), dst.display(), e);
            } else {
                debug!("Copied image {} to {}", src.display(), dst.display());
            }
        }
    }

    Ok(())
}

async fn export_tantivy_index(src_path: &str, dst_path: &str, kb_ids: &[i64]) -> anyhow::Result<usize> {
    if kb_ids.is_empty() {
        return Ok(0);
    }

    let src_index = tantivy::Index::open_in_dir(src_path)
        .with_context(|| format!("Failed to open source Tantivy index: {}", src_path))?;
    let src_schema = src_index.schema();
    let kb_id_field = src_schema.get_field("kb_id")?;

    let reader = src_index.reader()?;
    let searcher = reader.searcher();

    // Collect all matching documents
    let all_query = AllQuery;
    let doc_limit = 10_000_000;
    let top_docs = searcher.search(&all_query, &TopDocs::with_limit(doc_limit))?;

    if top_docs.len() == doc_limit {
        warn!("Tantivy export hit document limit of {}", doc_limit);
    }

    // Create destination index
    let (dst_schema, dst_index) = tantivy_engine::init_with_path(dst_path)?;
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

        if doc_kb_id.map_or(false, |id| kb_ids.contains(&id)) {
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
    }

    writer.commit()?;
    info!("Exported {} Tantivy documents to {}", count, dst_path);
    Ok(count)
}

async fn export_lancedb(src_path: &str, dst_path: &str, kb_ids: &[i64]) -> anyhow::Result<usize> {
    use lancedb::query::{ExecutableQuery, QueryBase};

    if kb_ids.is_empty() {
        return Ok(0);
    }

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

        let mut stream = src_table.query().only_if(&predicate).limit(1_000_000).execute().await?;

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            total_count += batch.num_rows();
            all_batches.push(batch);
        }
    }

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

    info!("Exported {} LanceDB rows to {}", total_count, dst_path);
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
