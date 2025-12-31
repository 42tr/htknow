use std::collections::HashMap;

use anyhow::Ok;
use log::info;
use tantivy::{Index, schema::Schema};

mod chinese_tokenizer;
mod embedding;
mod lancedb;
pub mod tantivy_engine;

pub use tantivy_engine::SearchResultItem;

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
        Ok(merged_results)
    }
}
