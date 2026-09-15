//! Persistent Wiki embeddings. Exact vector search is sufficient for the initial page corpus.
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Context, Result};
use arrow_array::{
    Array, ArrayRef, Int64Array, RecordBatch, StringArray,
    builder::{FixedSizeListBuilder, Float32Builder},
};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::{
    Table,
    query::{ExecutableQuery, QueryBase, Select},
};
use tokio::sync::{Mutex, OnceCell};

use super::{embedding, wiki_index::IndexedPage};

static TABLE: OnceCell<Arc<Table>> = OnceCell::const_new();
static CURSOR: AtomicUsize = AtomicUsize::new(0);
static KNOWN: Mutex<Option<HashMap<i64, i64>>> = Mutex::const_new(None);
static WRITE_LOCK: Mutex<()> = Mutex::const_new(());
const TABLE_NAME: &str = "wiki_pages";

fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("kb_id", DataType::Int64, false),
        Field::new("fingerprint", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                crate::config::get().ai.embedding_dim,
            ),
            false,
        ),
    ]))
}

fn fingerprint(page: &IndexedPage) -> String {
    format!(
        "{}:{}:{}",
        crate::config::get().ai.embedding_model,
        crate::config::get().ai.embedding_dim,
        page.fingerprint()
    )
}

async fn table() -> Result<&'static Arc<Table>> {
    TABLE
        .get_or_try_init(|| async {
            let conn = super::lancedb::get_connection()?;
            let names = conn.table_names().execute().await?;
            let table = if names.iter().any(|n| n == TABLE_NAME) {
                match conn.open_table(TABLE_NAME).execute().await {
                    Ok(t) => t,
                    Err(err) => {
                        log::warn!("Recovering Wiki vector table: {}", err);
                        super::lancedb::recover_named_table(
                            &crate::config::get().storage.lancedb_path,
                            TABLE_NAME,
                            &schema(),
                        )
                        .await?
                    }
                }
            } else {
                super::lancedb::create_empty_named_table(TABLE_NAME, &schema()).await?
            };
            let table = if table.schema().await?.as_ref() != schema().as_ref() {
                log::warn!("Rebuilding Wiki vector cache for changed embedding dimensions");
                super::lancedb::recover_named_table(&crate::config::get().storage.lancedb_path, TABLE_NAME, &schema())
                    .await?
            } else {
                table
            };
            Ok(Arc::new(table))
        })
        .await
}

pub async fn sync(pool: &sqlx::SqlitePool) -> Result<()> {
    let _guard = WRITE_LOCK.lock().await;
    let table = table().await?;
    let mut known_guard = KNOWN.lock().await;
    if known_guard.is_none() {
        // One-time repair for orphan vectors predating the durable change journal.
        // This reads IDs only; normal reconciliation never scans the vector corpus.
        let batches: Vec<RecordBatch> = table
            .query()
            .select(Select::columns(&["id"]))
            .limit(table.count_rows(None).await?.max(1))
            .execute()
            .await?
            .try_collect()
            .await?;
        for batch in batches {
            let ids = batch
                .column_by_name("id")
                .context("missing Wiki id")?
                .as_any()
                .downcast_ref::<Int64Array>()
                .context("invalid Wiki id")?;
            for id in ids.values() {
                let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM wiki_pages WHERE id = ? AND status = 'published' AND page_type != 'index')")
                    .bind(id).fetch_one(pool).await?;
                if !exists {
                    table.delete(&format!("id = {id}")).await?;
                }
            }
        }
    }
    let known = known_guard.get_or_insert_with(HashMap::new);
    let changes: Vec<(i64, i64)> =
        sqlx::query_as("SELECT seq, page_id FROM wiki_index_changes ORDER BY seq").fetch_all(pool).await?;
    let pending: Vec<_> = changes.into_iter().filter(|(seq, id)| known.get(id) != Some(seq)).collect();
    let start = CURSOR.fetch_add(16, Ordering::Relaxed) % pending.len().max(1);
    let mut first_error = None;
    for (seq, id) in pending.iter().cycle().skip(start).take(pending.len().min(16)) {
        let mut pages = super::wiki_index::load_pages(pool, &[*id]).await?;
        let Some(page) = pages.pop() else {
            table.delete(&format!("id = {id}")).await?;
            known.insert(*id, *seq);
            continue;
        };
        // Persisted matching embeddings survive process restarts without another model call.
        let matching = format!("id = {} AND fingerprint = '{}'", id, fingerprint(&page).replace('\'', "''"));
        if table.count_rows(Some(matching)).await? > 0 {
            known.insert(*id, *seq);
            continue;
        }
        let result: Result<()> = async {
            let vector = embedding::get_embedding(&page.text().chars().take(12000).collect::<String>()).await?;
            anyhow::ensure!(
                vector.len() == crate::config::get().ai.embedding_dim as usize,
                "Wiki embedding dimension mismatch"
            );
            let mut builder = FixedSizeListBuilder::new(Float32Builder::new(), vector.len() as i32);
            for value in vector {
                builder.values().append_value(value);
            }
            builder.append(true);
            let batch = RecordBatch::try_new(
                schema(),
                vec![
                    Arc::new(Int64Array::from(vec![page.id])) as ArrayRef,
                    Arc::new(Int64Array::from(vec![page.kb_id])),
                    Arc::new(StringArray::from(vec![fingerprint(&page)])),
                    Arc::new(builder.finish()),
                ],
            )?;
            // Generate before deleting the previous embedding. Query hydration rejects stale fingerprints.
            table.delete(&format!("id = {}", page.id)).await?;
            let reader = arrow_array::RecordBatchIterator::new(vec![Ok(batch)], schema());
            table.add(Box::new(reader) as Box<dyn arrow_array::RecordBatchReader + Send>).execute().await?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                known.insert(*id, *seq);
            }
            Err(err) => {
                first_error = Some(err);
            }
        }
    }
    if let Some(err) = first_error {
        return Err(err);
    }
    Ok(())
}

pub async fn search(
    pool: &sqlx::SqlitePool, query: &str, page_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>, limit: usize,
) -> Result<Vec<(i64, String)>> {
    if limit == 0 || kb_ids.is_some_and(Vec::is_empty) || page_ids.is_some_and(Vec::is_empty) {
        return Ok(Vec::new());
    }
    let Some(table) = TABLE.get() else {
        return Ok(Vec::new());
    };
    let total = table.count_rows(None).await?;
    if total == 0 {
        return Ok(Vec::new());
    }
    let vector = embedding::get_embedding(query).await?;
    let mut filters = Vec::new();
    if let Some(ids) = kb_ids {
        filters.push(format!("kb_id IN ({})", ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")));
    }
    if let Some(ids) = page_ids {
        filters.push(format!("id IN ({})", ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")));
    }
    let prefix = format!("{}:{}:", crate::config::get().ai.embedding_model, crate::config::get().ai.embedding_dim);
    let mut requested = limit.min(total);
    loop {
        let mut search =
            table.query().nearest_to(vector.clone())?.column("vector").select(Select::columns(&["id", "fingerprint"]));
        if !filters.is_empty() {
            search = search.only_if(filters.join(" AND "));
        }
        let batches: Vec<RecordBatch> = search.limit(requested).execute().await?.try_collect().await?;
        let fetched: usize = batches.iter().map(RecordBatch::num_rows).sum();
        let mut candidates = Vec::new();
        for batch in batches {
            let ids = batch
                .column_by_name("id")
                .context("missing Wiki id")?
                .as_any()
                .downcast_ref::<Int64Array>()
                .context("invalid Wiki id")?;
            let fingerprints = batch
                .column_by_name("fingerprint")
                .context("missing Wiki fingerprint")?
                .as_any()
                .downcast_ref::<StringArray>()
                .context("invalid Wiki fingerprint")?;
            for i in 0..batch.num_rows() {
                if let Some(value) = fingerprints.value(i).strip_prefix(&prefix) {
                    candidates.push((ids.value(i), value.to_string()));
                }
            }
        }
        let ids: Vec<_> = candidates.iter().map(|(id, _)| *id).collect();
        let current: HashMap<_, _> = super::wiki_index::load_pages(pool, &ids)
            .await?
            .into_iter()
            .filter(|p| kb_ids.is_none_or(|ids| ids.contains(&p.kb_id)))
            .map(|p| (p.id, p.fingerprint()))
            .collect();
        let mut seen = std::collections::HashSet::new();
        let hits: Vec<_> = candidates
            .into_iter()
            .filter(|(id, fp)| current.get(id) == Some(fp) && seen.insert(*id))
            .take(limit)
            .collect();
        if hits.len() >= limit || fetched < requested || requested >= total {
            return Ok(hits);
        }
        // Refill after filtering stale rows; embed the query only once, with no fixed overfetch cap.
        requested = requested.saturating_mul(2).min(total);
    }
}
