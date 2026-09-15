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

pub async fn sync(pages: &[IndexedPage]) -> Result<()> {
    let _guard = WRITE_LOCK.lock().await;
    let table = table().await?;
    let batches: Vec<RecordBatch> = table
        .query()
        .select(Select::columns(&["id", "fingerprint"]))
        .limit(table.count_rows(None).await?.max(1))
        .execute()
        .await?
        .try_collect()
        .await?;
    let mut existing = HashMap::new();
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
            existing.insert(ids.value(i), fingerprints.value(i).to_string());
        }
    }
    let expected: HashMap<_, _> = pages.iter().map(|p| (p.id, fingerprint(p))).collect();
    for id in existing.keys().filter(|id| !expected.contains_key(id)) {
        table.delete(&format!("id = {id}")).await?;
    }
    // Bound each cycle; failures are retried from SQLite on the next cycle or restart.
    let mut first_error = None;
    let pending: Vec<_> = pages.iter().filter(|p| existing.get(&p.id) != expected.get(&p.id)).collect();
    let start = CURSOR.fetch_add(16, Ordering::Relaxed) % pending.len().max(1);
    for page in pending.iter().cycle().skip(start).take(pending.len().min(16)) {
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
                    Arc::new(StringArray::from(vec![fingerprint(page)])),
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
        if let Err(err) = result {
            first_error = Some(err);
        }
    }
    if let Some(err) = first_error {
        return Err(err);
    }
    Ok(())
}

pub async fn search(
    query: &str, page_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>, limit: usize,
) -> Result<Vec<(i64, String)>> {
    if kb_ids.is_some_and(Vec::is_empty) || page_ids.is_some_and(Vec::is_empty) {
        return Ok(Vec::new());
    }
    let Some(table) = TABLE.get() else {
        return Ok(Vec::new());
    };
    if table.count_rows(None).await? == 0 {
        return Ok(Vec::new());
    }
    let vector = embedding::get_embedding(query).await?;
    let mut query = table.query().nearest_to(vector)?.column("vector").select(Select::columns(&["id", "fingerprint"]));
    let mut filters = Vec::new();
    if let Some(ids) = kb_ids {
        filters.push(format!("kb_id IN ({})", ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")));
    }
    if let Some(ids) = page_ids {
        filters.push(format!("id IN ({})", ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")));
    }
    if !filters.is_empty() {
        query = query.only_if(filters.join(" AND "));
    }
    let batches: Vec<RecordBatch> = query.limit(limit.max(1)).execute().await?.try_collect().await?;
    let mut hits = Vec::new();
    let prefix = format!("{}:{}:", crate::config::get().ai.embedding_model, crate::config::get().ai.embedding_dim);
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
                hits.push((ids.value(i), value.to_string()));
            }
        }
    }
    Ok(hits)
}
