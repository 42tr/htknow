//! Wiki recall uses a separate namespace: page IDs must never be interpreted as slice IDs.
//! SQLite is authoritative. Reconciliation also repairs interrupted writes and deleted pages.
use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::Result;
use sqlx::SqlitePool;
use tantivy::{IndexReader, schema::Schema};
use tokio::sync::Mutex;

use super::{SearchEngine, tantivy_engine};

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct IndexedPage {
    pub id: i64,
    pub kb_id: i64,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub aliases: String,
    pub version: i64,
}

impl IndexedPage {
    pub fn text(&self) -> String {
        format!("{}\n{}\n{}\n{}", self.title, self.aliases, self.summary, self.content)
    }
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(format!("{}\n{}\n{}", self.kb_id, self.slug, self.text())))
    }
}

pub struct WikiIndex {
    schema: Schema,
    reader: IndexReader,
    writer: Arc<tantivy_engine::IndexWriterHandle>,
    known: Mutex<HashMap<i64, String>>,
}

impl WikiIndex {
    pub async fn open(path: &str) -> Result<Self> {
        let (schema, index) = tantivy_engine::init_with_path(path)?;
        // Rebuild the derived lexical cache on startup, including equal-count edits/deletions.
        // Vectors retain fingerprints on disk and are reused across restarts.
        {
            let mut writer = tantivy_engine::create_writer_with_timing(&index, "wiki reset").await?;
            writer.delete_all_documents()?;
            writer.commit()?;
        }
        let reader = super::build_reader(&index, "wiki_index");
        let writer = tantivy_engine::IndexWriterHandle::open(index, schema.clone(), "wiki_index".into()).await?;
        Ok(Self { schema, reader, writer, known: Mutex::new(HashMap::new()) })
    }

    pub async fn rebuild(&self, pool: &SqlitePool) -> Result<()> {
        for value in self.known.lock().await.values_mut() {
            value.clear();
        }
        self.sync(pool).await?;
        Ok(())
    }

    pub async fn sync(&self, pool: &SqlitePool) -> Result<Vec<IndexedPage>> {
        let mut known = self.known.lock().await;
        let pages = published_pages(pool).await?;
        let expected: HashMap<_, _> = pages.iter().map(|p| (p.id, p.fingerprint())).collect();
        let removed: Vec<_> = known.keys().filter(|id| !expected.contains_key(id)).copied().collect();
        let changed: Vec<_> = pages.iter().filter(|p| known.get(&p.id) != expected.get(&p.id)).collect();
        let mut replace = removed;
        replace.extend(changed.iter().map(|p| p.id));
        if !replace.is_empty() {
            self.writer.delete_by_field("id", &replace).await?;
            for chunk in changed.chunks(100) {
                self.writer
                    .write_batch(
                        chunk
                            .iter()
                            .map(|p| {
                                // Reuse the numeric filter slot for page IDs in this isolated index.
                                tantivy_engine::Document::new(p.id, p.id, Some(p.kb_id), p.text())
                            })
                            .collect(),
                    )
                    .await?;
            }
            self.reader.reload()?;
        }
        *known = expected;
        Ok(pages)
    }

    pub async fn recall(
        &self, pool: &SqlitePool, query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>, limit: usize,
    ) -> Result<Vec<(IndexedPage, f32)>> {
        if query.trim().is_empty() || kb_ids.is_some_and(Vec::is_empty) || file_ids.is_some_and(Vec::is_empty) {
            return Ok(Vec::new());
        }
        let pages = self.sync(pool).await?;
        let page_ids: Option<Vec<i64>> = if let Some(ids) = file_ids {
            let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
                "SELECT DISTINCT page_id FROM wiki_page_sources WHERE file_id IN (",
            );
            let mut separated = qb.separated(",");
            for id in ids {
                separated.push_bind(id);
            }
            separated.push_unseparated(")");
            Some(qb.build_query_scalar().fetch_all(pool).await?)
        } else {
            None
        };
        if page_ids.as_ref().is_some_and(Vec::is_empty) {
            return Ok(Vec::new());
        }
        let by_id: HashMap<_, _> = pages
            .into_iter()
            .filter(|p| {
                page_ids.as_ref().is_none_or(|ids| ids.contains(&p.id))
                    && kb_ids.is_none_or(|ids| ids.contains(&p.kb_id))
            })
            .map(|p| (p.id, p))
            .collect();
        if by_id.is_empty() {
            return Ok(Vec::new());
        }
        let lexical = tantivy_engine::search_sync_with_limit(
            &self.reader,
            &self.schema,
            query,
            page_ids.as_ref(),
            kb_ids,
            None,
            None,
            limit,
        )?;
        let mut scores = HashMap::<i64, f32>::new();
        // Reciprocal-rank fusion avoids comparing BM25 with vector distances.
        for (rank, hit) in lexical.iter().enumerate() {
            *scores.entry(hit.id).or_default() += 1.0 / (60.0 + rank as f32 + 1.0);
        }
        match super::wiki_vector::search(query, page_ids.as_ref(), kb_ids, limit).await {
            Ok(hits) => {
                for (rank, (id, fingerprint)) in hits.into_iter().enumerate() {
                    if by_id.get(&id).is_some_and(|p| p.fingerprint() == fingerprint) {
                        *scores.entry(id).or_default() += 1.0 / (60.0 + rank as f32 + 1.0);
                    }
                }
            }
            Err(err) => log::warn!("Wiki vector recall unavailable; using full text: {}", err),
        }
        let mut results: Vec<_> = by_id
            .into_values()
            .filter_map(|p| {
                if kb_ids.is_some_and(|ids| !ids.contains(&p.kb_id)) {
                    return None;
                }
                scores.get(&p.id).map(|score| (p, *score))
            })
            .collect();
        results.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.id.cmp(&b.0.id)));
        results.truncate(limit);
        Ok(results)
    }
}

pub async fn published_pages(pool: &SqlitePool) -> Result<Vec<IndexedPage>> {
    Ok(sqlx::query_as(
        "SELECT p.id, p.kb_id, p.slug, p.title, p.summary, p.content, p.aliases, p.version \
         FROM wiki_pages p JOIN knowledge_bases kb ON kb.id = p.kb_id \
         WHERE p.status = 'published' AND p.page_type != 'index' ORDER BY p.id",
    )
    .fetch_all(pool)
    .await?)
}

impl SearchEngine {
    pub async fn search_wiki(&self, query: &str, kb_ids: Option<&Vec<i64>>) -> Result<Vec<(IndexedPage, f32)>> {
        let pool = self.pool.as_ref().ok_or_else(|| anyhow::anyhow!("search engine db pool not set"))?;
        self.wiki_index.recall(pool, query, None, kb_ids, crate::config::get().search.limit).await
    }

    pub async fn search_wiki_scoped(
        &self, query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
    ) -> Result<Vec<(IndexedPage, f32)>> {
        let pool = self.pool.as_ref().ok_or_else(|| anyhow::anyhow!("search engine db pool not set"))?;
        self.wiki_index.recall(pool, query, file_ids, kb_ids, crate::config::get().search.limit).await
    }

    pub async fn search_wiki_limited(
        &self, query: &str, kb_ids: Option<&Vec<i64>>, limit: usize,
    ) -> Result<Vec<(IndexedPage, f32)>> {
        let pool = self.pool.as_ref().ok_or_else(|| anyhow::anyhow!("search engine db pool not set"))?;
        self.wiki_index.recall(pool, query, None, kb_ids, limit).await
    }

    pub async fn sync_wiki_indexes(&self) -> Result<()> {
        let pool = self.pool.as_ref().ok_or_else(|| anyhow::anyhow!("search engine db pool not set"))?;
        let pages = self.wiki_index.sync(pool).await?;
        super::wiki_vector::sync(&pages).await
    }

    pub fn start_wiki_indexer(&self) {
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                if !crate::processor::is_parse_paused() {
                    if let Some(pool) = &engine.pool {
                        match engine.wiki_index.sync(pool).await {
                            Ok(pages) => {
                                if let Err(err) = super::wiki_vector::sync(&pages).await {
                                    log::warn!("Wiki vector reconciliation will retry: {}", err);
                                }
                            }
                            Err(err) => log::warn!("Wiki index reconciliation will retry: {}", err),
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });
    }
}
