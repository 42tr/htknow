use std::sync::Arc;

use anyhow::Result;
use arrow_array::{
    Array, ArrayRef, BooleanArray, Float32Array, Int64Array, RecordBatch, RecordBatchIterator, StringArray, builder::{FixedSizeListBuilder, Float32Builder}
};
use arrow_schema::{DataType, Field, Schema as ArrowSchema};
use futures::stream::StreamExt;
use lancedb::{
    Connection, connect, query::{ExecutableQuery, QueryBase}, table::NewColumnTransform
};
use once_cell::sync::OnceCell;

use super::{embedding, tantivy_engine::SearchResultItem};
use crate::config;

static LANCEDB: OnceCell<Arc<Connection>> = OnceCell::new();
static TABLE_NAME: &str = "documents";
static IS_DELETED_COLUMN: &str = "is_deleted";

#[derive(Clone)]
pub struct Document {
    pub id: i64,                           // 切片 ID
    pub file_id: i64,                      // 文件 ID
    pub kb_id: Option<i64>,                // 知识库 ID
    pub content: String,                   // 内容
    pub is_image: bool,                    // 是否为图片文件
    pub image_embedding: Option<Vec<f32>>, // 图片 embedding
}

impl Document {
    pub fn new(id: i64, file_id: i64, kb_id: Option<i64>, content: String) -> Self {
        Document { id, file_id, kb_id, content, is_image: false, image_embedding: None }
    }

    pub fn with_image_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.is_image = true;
        self.image_embedding = Some(embedding);
        self
    }
}

pub async fn init() -> Result<()> {
    let cfg = config::get();
    let storage_path = &cfg.storage.lancedb_path;
    std::fs::create_dir_all(storage_path)?;

    let db = connect(storage_path).execute().await?;
    LANCEDB.set(Arc::new(db)).map_err(|_| anyhow::anyhow!("Failed to initialize LanceDB"))?;

    // 创建表的 schema
    let schema = create_schema();

    // 检查表是否存在
    let conn = get_connection()?;
    let table_exists = if let Ok(table_names) = conn.table_names().execute().await {
        table_names.contains(&TABLE_NAME.to_string())
    } else {
        false
    };

    if !table_exists {
        // 表不存在，创建新表
        let empty_batch = create_empty_batch(&schema)?;
        conn.create_table(TABLE_NAME, Box::new(RecordBatchIterator::new(vec![Ok(empty_batch)], schema.clone())))
            .execute()
            .await?;
    } else {
        let table = conn.open_table(TABLE_NAME).execute().await?;
        ensure_is_deleted_column(&table).await?;
    }
    // 如果表已存在，直接使用现有的表

    Ok(())
}

pub async fn write_documents(doc: Document) -> Result<()> {
    let conn = get_connection()?;
    let table = conn.open_table(TABLE_NAME).execute().await?;
    let schema = create_schema();
    let batch = create_record_batch(vec![doc], &schema).await?;
    table.add(Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema))).execute().await?;
    Ok(())
}

pub async fn search(query: &str, file_id: Option<i64>, kb_ids: Option<&Vec<i64>>) -> Result<Vec<SearchResultItem>> {
    let cfg = config::get();
    let conn = get_connection()?;
    let table = conn.open_table(TABLE_NAME).execute().await?;

    // 获取查询文本的 embedding
    let query_vector = embedding::get_embedding(query).await?;

    // 使用向量搜索
    let mut query_builder = table.query().nearest_to(query_vector)?;

    // 应用过滤条件
    let mut filter_conditions = Vec::new();
    filter_conditions.push(format!("({} = false OR {} IS NULL)", IS_DELETED_COLUMN, IS_DELETED_COLUMN));
    filter_conditions.push("is_image = false".to_string());
    if let Some(fid) = file_id {
        filter_conditions.push(format!("file_id = {}", fid));
    }
    if let Some(kids) = kb_ids {
        if !kids.is_empty() {
            let ids_str = kids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ");
            filter_conditions.push(format!("kb_id IN ({})", ids_str));
        }
    }

    if !filter_conditions.is_empty() {
        query_builder = query_builder.only_if(&filter_conditions.join(" AND "));
    }

    let mut result_stream = query_builder.limit(cfg.search.limit).execute().await?;

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

        // 获取距离分数（如果有的话）
        let distance_array =
            batch.column_by_name("_distance").and_then(|col| col.as_any().downcast_ref::<Float32Array>());

        for i in 0..num_rows {
            let id = id_array.value(i);
            let file_id = file_id_array.value(i);
            let kb_id = kb_id_array.and_then(|arr| if arr.is_null(i) { None } else { Some(arr.value(i)) });
            let content = content_array.value(i).to_string();

            // 使用向量距离作为分数（距离越小，分数越高）
            let score = if let Some(dist_arr) = distance_array {
                let distance = dist_arr.value(i);
                // 将距离转换为相似度分数 (0-1)，距离越小分数越高
                (1.0 / (1.0 + distance)).max(0.0).min(1.0)
            } else {
                0.5 // 默认分数
            };

            search_results.push(SearchResultItem { id, file_id, kb_id, content, score });
        }
    }

    // 按分数排序
    search_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    Ok(search_results)
}

pub async fn search_image(
    query_vector: Vec<f32>, file_id: Option<i64>, kb_ids: Option<&Vec<i64>>,
) -> Result<Vec<SearchResultItem>> {
    let cfg = config::get();
    let conn = get_connection()?;
    let table = conn.open_table(TABLE_NAME).execute().await?;

    let image_vector_dim = config::get().ai.image_embedding_dim;
    if query_vector.len() != image_vector_dim as usize {
        anyhow::bail!("Image embedding dimension mismatch: expected {}, got {}", image_vector_dim, query_vector.len());
    }

    let mut query_builder = table.query().nearest_to(query_vector)?.column("image_vector");

    let mut filter_conditions = vec![
        format!("({} = false OR {} IS NULL)", IS_DELETED_COLUMN, IS_DELETED_COLUMN),
        "is_image = true".to_string(),
    ];
    if let Some(fid) = file_id {
        filter_conditions.push(format!("file_id = {}", fid));
    }
    if let Some(kids) = kb_ids {
        if !kids.is_empty() {
            let ids_str = kids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ");
            filter_conditions.push(format!("kb_id IN ({})", ids_str));
        }
    }

    if !filter_conditions.is_empty() {
        query_builder = query_builder.only_if(&filter_conditions.join(" AND "));
    }

    let mut result_stream = query_builder.limit(cfg.search.limit).execute().await?;

    let mut search_results = Vec::new();
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

        let distance_array =
            batch.column_by_name("_distance").and_then(|col| col.as_any().downcast_ref::<Float32Array>());

        for i in 0..num_rows {
            let id = id_array.value(i);
            let file_id = file_id_array.value(i);
            let kb_id = kb_id_array.and_then(|arr| if arr.is_null(i) { None } else { Some(arr.value(i)) });
            let content = content_array.value(i).to_string();

            let score = if let Some(dist_arr) = distance_array {
                let distance = dist_arr.value(i);
                (1.0 / (1.0 + distance)).max(0.0).min(1.0)
            } else {
                0.5
            };

            search_results.push(SearchResultItem { id, file_id, kb_id, content, score });
        }
    }

    search_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    Ok(search_results)
}

pub async fn delete_by_file(file_id: i64) -> Result<()> {
    let conn = get_connection()?;
    let table = conn.open_table(TABLE_NAME).execute().await?;
    let predicate = format!("file_id = {}", file_id);
    table.update().only_if(predicate).column(IS_DELETED_COLUMN, "true").execute().await?;
    Ok(())
}

pub async fn delete_by_kb(kb_id: i64) -> Result<()> {
    let conn = get_connection()?;
    let table = conn.open_table(TABLE_NAME).execute().await?;
    let predicate = format!("kb_id = {}", kb_id);
    table.update().only_if(predicate).column(IS_DELETED_COLUMN, "true").execute().await?;
    Ok(())
}

fn get_connection() -> Result<Arc<Connection>> {
    LANCEDB.get().cloned().ok_or_else(|| anyhow::anyhow!("LanceDB not initialized"))
}

fn create_schema() -> Arc<ArrowSchema> {
    let vector_dim = config::get().ai.embedding_dim;
    let image_vector_dim = config::get().ai.image_embedding_dim;
    Arc::new(ArrowSchema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("file_id", DataType::Int64, false),
        Field::new("kb_id", DataType::Int64, true),
        Field::new("content", DataType::Utf8, false),
        Field::new("is_image", DataType::Boolean, false),
        Field::new(IS_DELETED_COLUMN, DataType::Boolean, true),
        // 向量字段 - 从配置获取维度
        Field::new(
            "vector",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), vector_dim),
            false,
        ),
        Field::new(
            "image_vector",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), image_vector_dim),
            true,
        ),
    ]))
}

fn create_empty_batch(schema: &Arc<ArrowSchema>) -> Result<RecordBatch> {
    let vector_dim = config::get().ai.embedding_dim;
    let image_vector_dim = config::get().ai.image_embedding_dim;
    let id_array: ArrayRef = Arc::new(Int64Array::from(Vec::<i64>::new()));
    let file_id_array: ArrayRef = Arc::new(Int64Array::from(Vec::<i64>::new()));
    let kb_id_array: ArrayRef = Arc::new(Int64Array::from(Vec::<Option<i64>>::new()));
    let content_array: ArrayRef = Arc::new(StringArray::from(Vec::<String>::new()));
    let is_image_array: ArrayRef = Arc::new(BooleanArray::from(Vec::<bool>::new()));
    let is_deleted_array: ArrayRef = Arc::new(BooleanArray::from(Vec::<bool>::new()));

    // 创建空的向量数组
    let value_builder = Float32Builder::new();
    let mut list_builder = FixedSizeListBuilder::new(value_builder, vector_dim);
    let vector_array: ArrayRef = Arc::new(list_builder.finish());

    let image_value_builder = Float32Builder::new();
    let mut image_list_builder = FixedSizeListBuilder::new(image_value_builder, image_vector_dim);
    let image_vector_array: ArrayRef = Arc::new(image_list_builder.finish());

    Ok(RecordBatch::try_new(schema.clone(), vec![
        id_array,
        file_id_array,
        kb_id_array,
        content_array,
        is_image_array,
        is_deleted_array,
        vector_array,
        image_vector_array,
    ])?)
}

async fn create_record_batch(docs: Vec<Document>, schema: &Arc<ArrowSchema>) -> Result<RecordBatch> {
    let vector_dim = config::get().ai.embedding_dim;
    let image_vector_dim = config::get().ai.image_embedding_dim;
    let ids: Vec<i64> = docs.iter().map(|d| d.id).collect();
    let file_ids: Vec<i64> = docs.iter().map(|d| d.file_id).collect();
    let kb_ids: Vec<Option<i64>> = docs.iter().map(|d| d.kb_id).collect();
    let contents: Vec<String> = docs.iter().map(|d| d.content.clone()).collect();
    let is_images: Vec<bool> = docs.iter().map(|d| d.is_image).collect();
    let is_deleted: Vec<bool> = vec![false; docs.len()];

    let id_array: ArrayRef = Arc::new(Int64Array::from(ids));
    let file_id_array: ArrayRef = Arc::new(Int64Array::from(file_ids));
    let kb_id_array: ArrayRef = Arc::new(Int64Array::from(kb_ids));
    let content_array: ArrayRef = Arc::new(StringArray::from(contents.clone()));
    let is_image_array: ArrayRef = Arc::new(BooleanArray::from(is_images));
    let is_deleted_array: ArrayRef = Arc::new(BooleanArray::from(is_deleted));

    // 获取真实的 embedding 向量
    let embeddings = embedding::get_embeddings(&contents).await?;

    let value_builder = Float32Builder::new();
    let mut list_builder = FixedSizeListBuilder::new(value_builder, vector_dim);

    for embedding_vec in embeddings {
        if embedding_vec.len() != vector_dim as usize {
            anyhow::bail!("Embedding dimension mismatch: expected {}, got {}", vector_dim, embedding_vec.len());
        }

        let values_builder = list_builder.values();
        for &value in &embedding_vec {
            values_builder.append_value(value);
        }
        list_builder.append(true);
    }

    let vector_array: ArrayRef = Arc::new(list_builder.finish());

    let image_value_builder = Float32Builder::new();
    let mut image_list_builder = FixedSizeListBuilder::new(image_value_builder, image_vector_dim);
    for doc in &docs {
        let values_builder = image_list_builder.values();
        if let Some(image_embedding) = &doc.image_embedding {
            if image_embedding.len() != image_vector_dim as usize {
                anyhow::bail!(
                    "Image embedding dimension mismatch: expected {}, got {}",
                    image_vector_dim,
                    image_embedding.len()
                );
            }
            for &value in image_embedding {
                values_builder.append_value(value);
            }
            image_list_builder.append(true);
        } else {
            for _ in 0..image_vector_dim {
                values_builder.append_value(0.0);
            }
            image_list_builder.append(false);
        }
    }
    let image_vector_array: ArrayRef = Arc::new(image_list_builder.finish());

    Ok(RecordBatch::try_new(schema.clone(), vec![
        id_array,
        file_id_array,
        kb_id_array,
        content_array,
        is_image_array,
        is_deleted_array,
        vector_array,
        image_vector_array,
    ])?)
}

async fn ensure_is_deleted_column(table: &lancedb::Table) -> Result<()> {
    let schema = table.schema().await?;
    if schema.field_with_name(IS_DELETED_COLUMN).is_err() {
        table
            .add_columns(
                NewColumnTransform::SqlExpressions(vec![(IS_DELETED_COLUMN.to_string(), "false".to_string())]),
                None,
            )
            .await?;
    }
    Ok(())
}
