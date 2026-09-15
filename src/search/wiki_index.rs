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
    cursors: Mutex<HashMap<i64, Arc<Mutex<i64>>>>,
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
        Ok(Self { schema, reader, writer, cursors: Mutex::new(HashMap::new()) })
    }

    pub async fn rebuild(&self, pool: &SqlitePool) -> Result<()> {
        for cursor in self.cursors.lock().await.values() {
            *cursor.lock().await = 0;
        }
        self.sync_scoped(pool, None).await?;
        Ok(())
    }

    // Only dirty pages in the requested KBs are hydrated. Per-KB cursors serialize
    // reconciliation without holding a global mutex across database reads.
    pub async fn sync_scoped(&self, pool: &SqlitePool, kb_ids: Option<&Vec<i64>>) -> Result<usize> {
        let ids = match kb_ids {
            Some(ids) => ids.clone(),
            None => sqlx::query_scalar("SELECT DISTINCT kb_id FROM wiki_index_changes").fetch_all(pool).await?,
        };
        let mut hydrated = 0;
        for kb_id in ids {
            let lock = self.cursors.lock().await.entry(kb_id).or_default().clone();
            let mut cursor = lock.lock().await;
            loop {
                let changes: Vec<(i64, i64)> = sqlx::query_as(
                    "SELECT seq, page_id FROM wiki_index_changes WHERE kb_id = ? AND seq > ? ORDER BY seq LIMIT 100",
                )
                .bind(kb_id)
                .bind(*cursor)
                .fetch_all(pool)
                .await?;
                if changes.is_empty() {
                    break;
                }
                let page_ids: Vec<_> = changes.iter().map(|(_, id)| *id).collect();
                let pages = load_pages(pool, &page_ids).await?;
                hydrated += pages.len();
                self.writer.delete_by_field("id", &page_ids).await?;
                if !pages.is_empty() {
                    self.writer
                        .write_batch(
                            pages
                                .iter()
                                .map(|p| tantivy_engine::Document::new(p.id, p.id, Some(p.kb_id), p.text()))
                                .collect(),
                        )
                        .await?;
                }
                self.reader.reload()?;
                *cursor = changes.last().unwrap().0;
            }
        }
        Ok(hydrated)
    }

    pub async fn recall(
        &self, pool: &SqlitePool, query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>, limit: usize,
    ) -> Result<Vec<(IndexedPage, f32)>> {
        if query.trim().is_empty() || kb_ids.is_some_and(Vec::is_empty) || file_ids.is_some_and(Vec::is_empty) {
            return Ok(Vec::new());
        }
        self.sync_scoped(pool, kb_ids).await?;
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
        match super::wiki_vector::search(pool, query, page_ids.as_ref(), kb_ids, limit).await {
            Ok(hits) => {
                for (rank, (id, _)) in hits.into_iter().enumerate() {
                    *scores.entry(id).or_default() += 1.0 / (60.0 + rank as f32 + 1.0);
                }
            }
            Err(err) => log::warn!("Wiki vector recall unavailable; using full text: {}", err),
        }
        let pages = load_pages(pool, &scores.keys().copied().collect::<Vec<_>>()).await?;
        let mut results: Vec<_> = pages
            .into_iter()
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

pub(super) async fn load_pages(pool: &SqlitePool, ids: &[i64]) -> Result<Vec<IndexedPage>> {
    let mut pages = Vec::new();
    for ids in ids.chunks(500) {
        let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT p.id, p.kb_id, p.slug, p.title, p.summary, p.content, p.aliases, p.version \
             FROM wiki_pages p WHERE p.status = 'published' AND p.page_type != 'index' AND p.id IN (",
        );
        let mut list = qb.separated(",");
        for id in ids {
            list.push_bind(id);
        }
        list.push_unseparated(")");
        pages.extend(qb.build_query_as::<IndexedPage>().fetch_all(pool).await?);
    }
    Ok(pages)
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
        self.wiki_index.sync_scoped(pool, None).await?;
        super::wiki_vector::sync(pool).await
    }

    pub fn start_wiki_indexer(&self) {
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                if !crate::processor::is_parse_paused() {
                    if let Err(err) = engine.sync_wiki_indexes().await {
                        log::warn!("Wiki index reconciliation will retry: {}", err);
                    }
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wiki_reconciliation_only_hydrates_scoped_changes() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(include_str!("../init.sql")).execute(&pool).await.unwrap();
        crate::graph::graph_manager::migrate(&pool).await.unwrap();
        crate::wiki::migrate(&pool).await.unwrap();
        for id in [1, 2] {
            sqlx::query("INSERT INTO knowledge_bases(id, user_id, name) VALUES(?, 'owner', 'kb')")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO wiki_pages(id, kb_id, slug, content, status) VALUES(?, ?, 'concept/test', 'originaltoken', 'published')")
                .bind(id).bind(id).execute(&pool).await.unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let index = WikiIndex::open(dir.path().to_str().unwrap()).await.unwrap();
        assert_eq!(index.sync_scoped(&pool, Some(&vec![1])).await.unwrap(), 1);
        assert_eq!(index.sync_scoped(&pool, Some(&vec![1])).await.unwrap(), 0);
        assert_eq!(index.sync_scoped(&pool, Some(&vec![2])).await.unwrap(), 1);
        sqlx::query("UPDATE wiki_pages SET content = 'replacementtoken', version = version + 1 WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(index.sync_scoped(&pool, Some(&vec![1])).await.unwrap(), 0);
        // Holding another KB's reconciliation lock must not block an unrelated query.
        let lock = index.cursors.lock().await.get(&2).unwrap().clone();
        let guard = lock.lock().await;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), index.sync_scoped(&pool, Some(&vec![1])))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        drop(guard);
        assert_eq!(index.sync_scoped(&pool, Some(&vec![2])).await.unwrap(), 1);
        assert_eq!(index.recall(&pool, "replacementtoken", None, Some(&vec![2]), 10).await.unwrap().len(), 1);
        sqlx::query("DELETE FROM wiki_pages WHERE id = 2").execute(&pool).await.unwrap();
        assert!(index.recall(&pool, "replacementtoken", None, Some(&vec![2]), 10).await.unwrap().is_empty());
        assert_eq!(index.sync_scoped(&pool, None).await.unwrap(), 0);
    }
}
