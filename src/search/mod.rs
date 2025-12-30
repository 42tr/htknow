use anyhow::Ok;
use log::info;
use tantivy::{Index, schema::Schema};

mod chinese_tokenizer;
pub mod tantivy_engine;

pub use tantivy_engine::SearchResultItem;

#[derive(Debug, Clone)]
pub struct SearchEngine {
    index: Index,
    schema: Schema,
}

impl SearchEngine {
    pub async fn init() -> Self {
        let (schema, index) = tantivy_engine::init().unwrap();
        Self { index, schema }
    }

    pub async fn write(&self, doc: tantivy_engine::Document) -> anyhow::Result<()> {
        tantivy_engine::write_documents(&self.index, &self.schema, doc).await?;
        Ok(())
    }

    pub async fn search(&self, query: &str, file_id: Option<i64>, kb_id: Option<i64>) -> anyhow::Result<Vec<SearchResultItem>> {
        info!("Searching for query: {}", query);
        let results = tantivy_engine::search(&self.index, &self.schema, query, file_id, kb_id).await?;
        Ok(results)
    }
}
