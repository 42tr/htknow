use std::collections::HashMap;

use anyhow::Ok;
use log::{debug, info};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, SqlitePool};
use tantivy::{Index, schema::Schema};

use crate::config;

mod chinese_tokenizer;
mod embedding;
mod lancedb;
pub mod tantivy_engine;

pub use tantivy_engine::SearchResultItem;

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

#[derive(Debug, Clone)]
pub struct SearchEngine {
    index: Index,
    schema: Schema,
    pool: Option<SqlitePool>,
}

impl SearchEngine {
    pub async fn init() -> Self {
        lancedb::init().await.expect("init lancedb failed");
        let (schema, index) = tantivy_engine::init().unwrap();
        Self { index, schema, pool: None }
    }

    pub fn with_pool(mut self, pool: SqlitePool) -> Self {
        self.pool = Some(pool);
        self
    }

    pub async fn write(&self, doc: tantivy_engine::Document) -> anyhow::Result<()> {
        // 写入 tantivy
        tantivy_engine::write_documents(&self.index, &self.schema, doc.clone()).await?;

        // 写入 lancedb
        let lancedb_doc = lancedb::Document::new(doc.id, doc.file_id, doc.kb_id, doc.content);
        lancedb::write_documents(lancedb_doc).await?;

        Ok(())
    }

    pub async fn delete(&self, file_id: Option<i64>, kb_id: Option<i64>) -> anyhow::Result<()> {
        if let Some(file_id) = file_id {
            tantivy_engine::delete_by_file(&self.index, &self.schema, file_id).await?;
            lancedb::delete_by_file(file_id).await?;
        }
        if let Some(kb_id) = kb_id {
            tantivy_engine::delete_by_kb(&self.index, &self.schema, kb_id).await?;
            lancedb::delete_by_kb(kb_id).await?;
        }
        Ok(())
    }

    pub async fn search(
        &self, query: &str, file_id: Option<i64>, kb_ids: Option<&Vec<i64>>,
    ) -> anyhow::Result<Vec<SearchResultItem>> {
        debug!("Searching for query: {}", query);

        // 使用 tantivy 搜索
        let tantivy_results = tantivy_engine::search(&self.index, &self.schema, query, file_id, kb_ids).await?;
        debug!("Tantivy results count: {}", tantivy_results.len());
        debug!("Tantivy results: {:?}", tantivy_results);

        // 使用 lancedb 搜索
        let lancedb_results = lancedb::search(query, file_id, kb_ids).await?;
        debug!("LanceDB results count: {}", lancedb_results.len());
        debug!("LanceDB results: {:?}", lancedb_results);

        // 合并结果：使用 HashMap 按 id 去重，保留最高分数
        let mut merged_map: HashMap<i64, SearchResultItem> = HashMap::new();

        for result in tantivy_results {
            merged_map.insert(result.id, result);
        }

        for result in lancedb_results {
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

    async fn rerank(&self, query: &str, results: Vec<SearchResultItem>) -> anyhow::Result<Vec<SearchResultItem>> {
        let cfg = config::get();

        // 提取所有文档内容用于重排序
        let documents: Vec<String> = results.iter().map(|r| r.content.clone()).collect();

        // 构造请求
        let rerank_request = RerankRequest { model: cfg.ai.rerank_model.clone(), query: query.to_string(), documents };

        // 调用 BGE-Rerank API
        let client = reqwest::Client::new();
        let response = client.post(&cfg.services.rerank_url).json(&rerank_request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Rerank API failed with status {}: {}", status, error_text);
        }

        // 先获取响应文本用于调试
        let response_text = response.text().await?;
        debug!("Rerank API response: {}", response_text);

        // 解析 JSON 响应
        let rerank_response: RerankResponse = serde_json::from_str(&response_text)?;

        // 检查返回的结果数量是否匹配
        if rerank_response.results.len() != results.len() {
            anyhow::bail!(
                "Rerank results count mismatch: expected {}, got {}",
                results.len(),
                rerank_response.results.len()
            );
        }

        // 使用重排序分数更新结果
        let mut reranked_results: Vec<SearchResultItem> = results
            .into_iter()
            .enumerate()
            .map(|(i, mut result)| {
                // 根据 index 找到对应的重排序结果
                if let Some(rerank_result) = rerank_response.results.iter().find(|r| r.index == i) {
                    result.score = rerank_result.relevance_score;
                }
                result
            })
            .collect();

        // 按新分数降序排序
        reranked_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        let threshold = cfg.ai.rerank_threshold;
        let filter_results = reranked_results.into_iter().filter(|f| f.score >= threshold).collect();
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

    /// 使用图谱增强的搜索
    /// 先扩展查询，然后对每个扩展查询进行搜索，最后合并去重结果
    pub async fn search_with_graph_expansion(
        &self, query: &str, file_id: Option<i64>, kb_ids: Option<&Vec<i64>>,
    ) -> anyhow::Result<Vec<SearchResultItem>> {
        // 1. 扩展查询
        let expanded_queries = self.expand_query_with_graph(query, kb_ids).await?;

        if expanded_queries.len() == 1 {
            // 没有扩展，直接使用原查询
            return self.search(query, file_id, kb_ids).await;
        }

        // 2. 对每个扩展查询进行搜索
        let mut all_results: HashMap<i64, SearchResultItem> = HashMap::new();

        for (idx, expanded_query) in expanded_queries.iter().enumerate() {
            let results = self.search(expanded_query, file_id, kb_ids).await?;

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
