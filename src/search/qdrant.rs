use std::sync::Arc;

use once_cell::sync::Lazy;
use qdrant::{
    config::*, content::{
        condition::{Condition, FieldCondition, Match}, filter::Filter, point::{PointIdType, PointStruct}, value::Value
    }, prelude::*, storage::content_manager::toc::TableOfContent
};
use serde::Serialize;
use uuid::Uuid;

static COLLECTION_NAME: &str = "x";

static QDRANT: Lazy<Arc<TableOfContent>> = Lazy::new(|| {
    let storage_path = "data/qdrant_data";

    let storage_config = StorageConfig { storage_path: storage_path.into(), ..Default::default() };

    let config = QdrantConfig { storage: storage_config, ..Default::default() };

    let toc = TableOfContent::new(&config).expect("init qdrant failed");
    Arc::new(toc)
});

pub struct Document {
    id: i64,            // 切片 ID
    file_id: i64,       // 文件 ID
    kb_id: Option<i64>, // 知识库 ID
    content: String,    // 内容
}

/// 搜索结果项
#[derive(Debug, Clone, Serialize)]
pub struct SearchResultItem {
    pub id: i64,            // 切片 ID
    pub file_id: i64,       // 文件 ID
    pub kb_id: Option<i64>, // 知识库 ID
    pub content: String,    // 内容
    pub score: f32,         // 搜索得分
}

impl Document {
    pub fn new(id: i64, file_id: i64, kb_id: Option<i64>, content: String) -> Self {
        Document { id, file_id, kb_id, content }
    }
}

pub async fn init() -> anyhow::Result<()> {
    if QDRANT.has_collection(COLLECTION_NAME).await {
        QDRANT.delete_collection(COLLECTION_NAME).await?;
    }

    let vectors_config = VectorsConfig::Single(VectorParams {
        size: 10,
        distance: Distance::Cosine,
        hnsw_config: None,
        quantization_config: Some(QuantizationConfig::Scalar(ScalarQuantization {
            r#type: ScalarType::Int8,
            quantile: None,
            always_ram: None,
        })),
        on_disk: None,
    });

    QDRANT
        .create_collection(COLLECTION_NAME, CreateCollection { vectors: vectors_config, ..Default::default() })
        .await?;

    Ok(())
}

pub async fn write_documents(doc: Document) -> anyhow::Result<()> {
    let mut payload = qdrant::content::payload::Payload::new();
    payload.insert("id", Value::Integer(doc.id));
    payload.insert("file_id", Value::Integer(doc.file_id));
    payload.insert("content", Value::Text(doc.content));

    if let Some(kb_id) = doc.kb_id {
        payload.insert("kb_id", Value::Integer(kb_id));
    }

    let point =
        PointStruct { id: PointIdType::Uuid(Uuid::new_v4()), vector: vec![12.0; 10].into(), payload: payload.into() };

    QDRANT.upsert_points(COLLECTION_NAME, None, vec![point]).await?;

    Ok(())
}

pub async fn search(query: &str, file_id: Option<i64>, kb_id: Option<i64>) -> anyhow::Result<Vec<SearchResultItem>> {
    let mut conditions = vec![Condition::Field(FieldCondition {
        key: "content".to_string(),
        r#match: Some(Match::Text(query.to_string())),
        ..Default::default()
    })];

    if let Some(file_id) = file_id {
        conditions.push(Condition::Field(FieldCondition {
            key: "file_id".to_string(),
            r#match: Some(Match::Integer(file_id)),
            ..Default::default()
        }));
    }

    if let Some(kb_id) = kb_id {
        conditions.push(Condition::Field(FieldCondition {
            key: "kb_id".to_string(),
            r#match: Some(Match::Integer(kb_id)),
            ..Default::default()
        }));
    }

    let filter = Filter::all(conditions);

    let result = QDRANT.search(COLLECTION_NAME, &[12.0; 10], Some(filter), 10, None).await?;

    let results = result
        .into_iter()
        .map(|scored| {
            let payload = scored.payload;
            let id = payload.get("id").and_then(|v| v.as_integer()).unwrap_or(0);
            let file_id = payload.get("file_id").and_then(|v| v.as_integer()).unwrap_or(0);
            let kb_id = payload.get("kb_id").and_then(|v| v.as_integer());
            let content = payload.get("content").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_default();

            SearchResultItem { id, file_id, kb_id, content, score: scored.score }
        })
        .collect();

    Ok(results)
}
