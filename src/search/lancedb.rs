use std::sync::Arc;

use anyhow::Result;
use arrow_array::{
    Array, ArrayRef, Int64Array, RecordBatch, RecordBatchIterator, StringArray, builder::{FixedSizeListBuilder, Float32Builder}
};
use arrow_schema::{DataType, Field, Schema as ArrowSchema};
use futures::stream::StreamExt;
use lancedb::{
    Connection, connect, query::{ExecutableQuery, QueryBase}
};
use once_cell::sync::OnceCell;

use super::tantivy_engine::SearchResultItem;

static LANCEDB: OnceCell<Arc<Connection>> = OnceCell::new();
static TABLE_NAME: &str = "documents";

#[derive(Clone)]
pub struct Document {
    pub id: i64,            // 切片 ID
    pub file_id: i64,       // 文件 ID
    pub kb_id: Option<i64>, // 知识库 ID
    pub content: String,    // 内容
}

impl Document {
    pub fn new(id: i64, file_id: i64, kb_id: Option<i64>, content: String) -> Self {
        Document { id, file_id, kb_id, content }
    }
}

pub async fn init() -> Result<()> {
    let storage_path = "data/lancedb_data";
    std::fs::create_dir_all(storage_path)?;

    let db = connect(storage_path).execute().await?;
    LANCEDB.set(Arc::new(db)).map_err(|_| anyhow::anyhow!("Failed to initialize LanceDB"))?;

    // 创建表的 schema
    let schema = create_schema();

    // 检查表是否存在，如果存在则删除
    let conn = get_connection()?;
    if let Ok(table_names) = conn.table_names().execute().await {
        if table_names.contains(&TABLE_NAME.to_string()) {
            conn.drop_table(TABLE_NAME).await?;
        }
    }

    // 创建空表
    let empty_batch = create_empty_batch(&schema)?;
    conn.create_table(TABLE_NAME, Box::new(RecordBatchIterator::new(vec![Ok(empty_batch)], schema.clone())))
        .execute()
        .await?;

    Ok(())
}

pub async fn write_documents(doc: Document) -> Result<()> {
    let conn = get_connection()?;
    let table = conn.open_table(TABLE_NAME).execute().await?;

    let schema = create_schema();
    let batch = create_record_batch(vec![doc], &schema)?;

    table.add(Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema))).execute().await?;

    Ok(())
}

pub async fn search(query: &str, file_id: Option<i64>, kb_id: Option<i64>) -> Result<Vec<SearchResultItem>> {
    let conn = get_connection()?;
    let table = conn.open_table(TABLE_NAME).execute().await?;

    // LanceDB 主要用于向量搜索，这里我们使用简单的全表扫描过滤
    // 在实际应用中，你需要将 content 转换为向量嵌入后进行向量搜索
    let mut query_builder = table.query();

    // 应用过滤条件
    let mut filter_conditions = Vec::new();
    if let Some(fid) = file_id {
        filter_conditions.push(format!("file_id = {}", fid));
    }
    if let Some(kid) = kb_id {
        filter_conditions.push(format!("kb_id = {}", kid));
    }

    if !filter_conditions.is_empty() {
        query_builder = query_builder.only_if(&filter_conditions.join(" AND "));
    }

    let mut result_stream = query_builder.limit(10).execute().await?;

    let mut search_results = Vec::new();

    // 从 stream 中读取数据
    while let Some(batch_result) = result_stream.next().await {
        let batch = batch_result?;
        let num_rows = batch.num_rows();

        let id_array = batch
            .column_by_name("id")
            .and_then(|col| col.as_any().downcast_ref::<Int64Array>())
            .ok_or_else(|| anyhow::anyhow!("Missing or invalid id column"))?;

        let file_id_array = batch
            .column_by_name("file_id")
            .and_then(|col| col.as_any().downcast_ref::<Int64Array>())
            .ok_or_else(|| anyhow::anyhow!("Missing or invalid file_id column"))?;

        let kb_id_array = batch.column_by_name("kb_id").and_then(|col| col.as_any().downcast_ref::<Int64Array>());

        let content_array = batch
            .column_by_name("content")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("Missing or invalid content column"))?;

        for i in 0..num_rows {
            let id = id_array.value(i);
            let file_id = file_id_array.value(i);
            let kb_id = kb_id_array.and_then(|arr| if arr.is_null(i) { None } else { Some(arr.value(i)) });
            let content = content_array.value(i).to_string();

            // 简单的文本匹配评分（实际应该使用向量相似度）
            let score = if content.contains(query) { 0.8 } else { 0.1 };

            search_results.push(SearchResultItem { id, file_id, kb_id, content, score });
        }
    }

    // 按分数排序
    search_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    Ok(search_results)
}

fn get_connection() -> Result<Arc<Connection>> {
    LANCEDB.get().cloned().ok_or_else(|| anyhow::anyhow!("LanceDB not initialized"))
}

fn create_schema() -> Arc<ArrowSchema> {
    Arc::new(ArrowSchema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("file_id", DataType::Int64, false),
        Field::new("kb_id", DataType::Int64, true),
        Field::new("content", DataType::Utf8, false),
        // 为向量搜索预留字段（维度为 384，常用的嵌入维度）
        Field::new("vector", DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), 384), true),
    ]))
}

fn create_empty_batch(schema: &Arc<ArrowSchema>) -> Result<RecordBatch> {
    let id_array: ArrayRef = Arc::new(Int64Array::from(Vec::<i64>::new()));
    let file_id_array: ArrayRef = Arc::new(Int64Array::from(Vec::<i64>::new()));
    let kb_id_array: ArrayRef = Arc::new(Int64Array::from(Vec::<Option<i64>>::new()));
    let content_array: ArrayRef = Arc::new(StringArray::from(Vec::<String>::new()));

    // 创建空的向量数组
    let value_builder = Float32Builder::new();
    let mut list_builder = FixedSizeListBuilder::new(value_builder, 384);
    let vector_array: ArrayRef = Arc::new(list_builder.finish());

    Ok(RecordBatch::try_new(schema.clone(), vec![id_array, file_id_array, kb_id_array, content_array, vector_array])?)
}

fn create_record_batch(docs: Vec<Document>, schema: &Arc<ArrowSchema>) -> Result<RecordBatch> {
    let ids: Vec<i64> = docs.iter().map(|d| d.id).collect();
    let file_ids: Vec<i64> = docs.iter().map(|d| d.file_id).collect();
    let kb_ids: Vec<Option<i64>> = docs.iter().map(|d| d.kb_id).collect();
    let contents: Vec<String> = docs.iter().map(|d| d.content.clone()).collect();

    let id_array: ArrayRef = Arc::new(Int64Array::from(ids));
    let file_id_array: ArrayRef = Arc::new(Int64Array::from(file_ids));
    let kb_id_array: ArrayRef = Arc::new(Int64Array::from(kb_ids));
    let content_array: ArrayRef = Arc::new(StringArray::from(contents));

    // 创建虚拟向量（实际应该使用嵌入模型生成）
    let value_builder = Float32Builder::new();
    let mut list_builder = FixedSizeListBuilder::new(value_builder, 384);

    for _ in 0..docs.len() {
        let values_builder = list_builder.values();
        for _ in 0..384 {
            values_builder.append_value(0.0);
        }
        list_builder.append(true);
    }

    let vector_array: ArrayRef = Arc::new(list_builder.finish());

    Ok(RecordBatch::try_new(schema.clone(), vec![id_array, file_id_array, kb_id_array, content_array, vector_array])?)
}
