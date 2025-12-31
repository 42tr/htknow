use std::collections::HashMap;

use anyhow::Ok;
use log::info;
use serde::{Deserialize, Serialize};
use tantivy::{Index, schema::Schema};

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
}

impl SearchEngine {
    pub async fn init() -> Self {
        lancedb::init().await.expect("init lancedb failed");
        let (schema, index) = tantivy_engine::init().unwrap();
        Self { index, schema }
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
        &self, query: &str, file_id: Option<i64>, kb_id: Option<i64>,
    ) -> anyhow::Result<Vec<SearchResultItem>> {
        info!("Searching for query: {}", query);

        // 使用 tantivy 搜索
        let tantivy_results = tantivy_engine::search(&self.index, &self.schema, query, file_id, kb_id).await?;
        info!("Tantivy results count: {}", tantivy_results.len());
        info!("Tantivy results: {:?}", tantivy_results);

        // 使用 lancedb 搜索
        let lancedb_results = lancedb::search(query, file_id, kb_id).await?;
        info!("LanceDB results count: {}", lancedb_results.len());
        info!("LanceDB results: {:?}", lancedb_results);

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
        // 提取所有文档内容用于重排序
        let documents: Vec<String> = results.iter().map(|r| r.content.clone()).collect();

        // 构造请求
        let rerank_request = RerankRequest { model: "bge-rerank".to_string(), query: query.to_string(), documents };

        // 调用 BGE-Rerank API
        let client = reqwest::Client::new();
        let response = client.post("http://192.168.0.46:9600/v1/rerank").json(&rerank_request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Rerank API failed with status {}: {}", status, error_text);
        }

        // 先获取响应文本用于调试
        let response_text = response.text().await?;
        info!("Rerank API response: {}", response_text);

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

        Ok(reranked_results)
    }
}
