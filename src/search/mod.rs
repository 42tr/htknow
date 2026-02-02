use std::{collections::HashMap, sync::Arc, time::Instant};

use log::{debug, info};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, SqlitePool};
use tantivy::{Index, IndexReader, ReloadPolicy, schema::Schema};
use tokio::sync::Mutex;

use crate::config;

mod chinese_tokenizer;
pub mod embedding;
mod lancedb;
pub mod tantivy_engine;

pub use tantivy_engine::{FullSearchResultItem, SearchResultItem};

const FULL_SNIPPET_MAX_CHARS: usize = 400;

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
            pool: None,
        }
    }

    pub fn with_pool(mut self, pool: SqlitePool) -> Self {
        self.pool = Some(pool);
        self
    }

    pub async fn write(&self, doc: tantivy_engine::Document, image_embedding: Option<Vec<f32>>) -> anyhow::Result<()> {
        // 写入 tantivy
        {
            let _guard = self.index_write_lock.lock().await;
            tantivy_engine::write_documents(&self.index, &self.schema, doc.clone()).await?;
        }
        reload_reader(&self.index_reader, "index")?;

        // 写入 lancedb
        let mut lancedb_doc = lancedb::Document::new(doc.id, doc.file_id, doc.kb_id, doc.content);
        if let Some(image_embedding) = image_embedding {
            lancedb_doc = lancedb_doc.with_image_embedding(image_embedding);
        }
        lancedb::write_documents(lancedb_doc).await?;

        Ok(())
    }

    /// 批量写入文档，减少 commit 次数
    pub async fn write_batch(
        &self, docs: Vec<tantivy_engine::Document>, image_embeddings: Vec<Option<Vec<f32>>>,
    ) -> anyhow::Result<()> {
        if docs.is_empty() {
            return Ok(());
        }

        // 批量写入 tantivy
        {
            let _guard = self.index_write_lock.lock().await;
            tantivy_engine::write_documents_batch(&self.index, &self.schema, docs.clone()).await?;
        }
        reload_reader(&self.index_reader, "index")?;

        // 批量写入 lancedb：收集所有 lancedb 文档后一次性写入
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
        debug!("Searching for query: {}", query);

        // 使用 tantivy 搜索
        let tantivy_results = tantivy_engine::search(&self.index_reader, &self.schema, query, file_ids, kb_ids).await?;
        debug!("Tantivy results count: {}", tantivy_results.len());
        debug!("Tantivy results: {:?}", tantivy_results);

        // 使用 lancedb 搜索
        let lancedb_start = Instant::now();
        let lancedb_results = lancedb::search(query, file_ids, kb_ids).await?;
        debug!("LanceDB search {}ms", lancedb_start.elapsed().as_millis());
        debug!("LanceDB results count: {}", lancedb_results.len());
        debug!("LanceDB results: {:?}", lancedb_results);

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
        let reranked_results = self.rerank(query, merged_results).await?;
        info!("Reranked results count: {}", reranked_results.len());

        Ok(reranked_results)
    }

    pub async fn search_full(
        &self, query: &str, file_ids: Option<&Vec<i64>>, kb_ids: Option<&Vec<i64>>,
    ) -> anyhow::Result<Vec<FullSearchResultItem>> {
        tantivy_engine::search_with_snippet(
            &self.full_index_reader,
            &self.full_schema,
            query,
            file_ids,
            kb_ids,
            FULL_SNIPPET_MAX_CHARS,
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
        let client = reqwest::Client::new();
        let rerank_http_start = Instant::now();
        let response = client.post(&cfg.services.rerank_url).json(&rerank_request).send().await?;
        debug!("Rerank HTTP request {}ms", rerank_http_start.elapsed().as_millis());

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Rerank API failed with status {}: {}; rerank_request: {:?}",
                status,
                error_text,
                rerank_request
            );
        }

        // 先获取响应文本用于调试
        let rerank_read_start = Instant::now();
        let response_text = response.text().await?;
        debug!("Rerank response read {}ms", rerank_read_start.elapsed().as_millis());
        debug!("Rerank API response: {}", response_text);

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

fn reload_reader(reader: &IndexReader, label: &str) -> tantivy::Result<()> {
    let start = Instant::now();
    reader.reload()?;
    debug!("Tantivy {} reader reload {}ms", label, start.elapsed().as_millis());
    Ok(())
}
