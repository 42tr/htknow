use std::path::Path;

use log::info;
use serde::Serialize;
use tantivy::{
    Index, Result, TantivyDocument, Term, collector::TopDocs, doc, query::{BooleanQuery, Occur, Query, TermQuery}, schema::{FAST, Field, INDEXED, IndexRecordOption, STORED, Schema, TextFieldIndexing, TextOptions, Value as _}
};

use super::chinese_tokenizer;
use crate::config;

const ALL_TOKENIZER: &str = "all";

#[derive(Clone)]
pub struct Document {
    pub id: i64,            // 切片 ID
    pub file_id: i64,       // 文件 ID
    pub kb_id: Option<i64>, // 知识库 ID
    pub content: String,    // 内容
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

/// create new index
///
/// delete if it exists
pub fn init() -> Result<(Schema, Index)> {
    let cfg = config::get();
    let schema = build_schema();
    let path = Path::new(&cfg.search.tantivy_index_path);
    let index = if path.exists() {
        Index::open_in_dir(path)?
    } else {
        std::fs::create_dir_all(path)?;
        Index::create_in_dir(path, schema.clone())?
    };
    register_tokenizers(&index);
    Ok((schema, index))
}

pub async fn write_documents(index: &Index, schema: &Schema, doc: Document) -> tantivy::Result<()> {
    let writer_memory = config::get().search.tantivy_memory_mb * 1_000_000;
    let mut index_writer = index.writer(writer_memory)?;
    index_writer.add_document(create_document(doc, schema))?;
    index_writer.commit()?;
    Ok(())
}

pub async fn search(
    index: &Index, schema: &Schema, query: &str, file_id: Option<i64>, kb_ids: Option<&Vec<i64>>,
) -> anyhow::Result<Vec<SearchResultItem>> {
    let cfg = config::get();
    let searcher = index.reader()?.searcher();
    let tantivy_query = build_query(query, file_id, kb_ids, schema)?;
    let top_docs = searcher.search(&tantivy_query, &TopDocs::with_limit(cfg.search.limit))?;

    let mut results = vec![];
    for (score, doc_address) in top_docs {
        let retrieved_doc: TantivyDocument = searcher.doc(doc_address)?;
        let id = retrieved_doc.get_first(get_field(schema, "id")).and_then(|v| v.as_i64()).unwrap_or(0);
        let file_id = retrieved_doc.get_first(get_field(schema, "file_id")).and_then(|v| v.as_i64()).unwrap_or(0);
        let kb_id = retrieved_doc.get_first(get_field(schema, "kb_id")).and_then(|v| v.as_i64());
        let content = retrieved_doc
            .get_first(get_field(schema, "content"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();
        results.push(SearchResultItem { id, file_id, kb_id, content, score });
    }

    Ok(results)
}

pub async fn delete_by_file(index: &Index, schema: &Schema, file_id: i64) -> anyhow::Result<()> {
    let writer_memory = config::get().search.tantivy_memory_mb * 1_000_000;
    let mut writer = index.writer::<TantivyDocument>(writer_memory)?;
    let file_id_field = get_field(schema, "file_id");
    let term = Term::from_field_i64(file_id_field, file_id);
    writer.delete_term(term);
    writer.commit()?;
    Ok(())
}

pub async fn delete_by_kb(index: &Index, schema: &Schema, kb_id: i64) -> anyhow::Result<()> {
    let writer_memory = config::get().search.tantivy_memory_mb * 1_000_000;
    let mut writer = index.writer::<TantivyDocument>(writer_memory)?;
    let kb_id_field = get_field(schema, "kb_id");
    let term = Term::from_field_i64(kb_id_field, kb_id);
    writer.delete_term(term);
    writer.commit()?;
    Ok(())
}

fn build_query(
    input: &str, file_id: Option<i64>, kb_ids: Option<&Vec<i64>>, schema: &Schema,
) -> tantivy::Result<Box<dyn Query>> {
    let mut subqueries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
    let segmented_words = perform_segmentation(input, chinese_tokenizer::SegmentationMode::Search);

    // content 至少命中一个
    let mut content_queries = Vec::new();
    for word in segmented_words {
        let tq = TermQuery::new(Term::from_field_text(get_field(schema, "content"), &word), IndexRecordOption::Basic);
        content_queries.push((Occur::Should, Box::new(tq) as Box<dyn Query>));
    }
    let content_bool = BooleanQuery::new(content_queries);
    subqueries.push((Occur::Must, Box::new(content_bool)));

    if let Some(file_id) = file_id {
        let file_id_query =
            TermQuery::new(Term::from_field_i64(get_field(schema, "file_id"), file_id), IndexRecordOption::Basic);
        subqueries.push((Occur::Must, Box::new(file_id_query)));
    }
    
    if let Some(ids) = kb_ids {
        if !ids.is_empty() {
            let mut kb_id_queries = Vec::new();
            let kb_id_field = get_field(schema, "kb_id");
            for kb_id in ids {
                let kb_id_query = TermQuery::new(Term::from_field_i64(kb_id_field, *kb_id), IndexRecordOption::Basic);
                kb_id_queries.push((Occur::Should, Box::new(kb_id_query) as Box<dyn Query>));
            }
            if !kb_id_queries.is_empty() {
                let kb_ids_bool_query = BooleanQuery::new(kb_id_queries);
                subqueries.push((Occur::Must, Box::new(kb_ids_bool_query)));
            }
        }
    }
    Ok(Box::new(BooleanQuery::new(subqueries)))
}

fn create_document(doc: Document, schema: &Schema) -> TantivyDocument {
    let mut document = doc! {
        get_field(schema, "id") => doc.id,
        get_field(schema, "file_id") => doc.file_id,
        get_field(schema, "content") => doc.content,
    };
    if let Some(kb_id) = doc.kb_id {
        document.add_i64(get_field(schema, "kb_id"), kb_id);
    }
    info!("Document: {:?}", document);
    document
}

fn get_field(schema: &Schema, field: &str) -> Field {
    schema.get_field(field).unwrap_or_else(|_| panic!("Field '{}' not found in schema", field))
}

fn perform_segmentation(text: &str, mode: chinese_tokenizer::SegmentationMode) -> Vec<String> {
    chinese_tokenizer::FastChineseTokenizer::new(mode).segment(text)
}

fn register_tokenizers(index: &Index) {
    let all_tokenizer = chinese_tokenizer::FastChineseTokenizer::all();
    index.tokenizers().register(ALL_TOKENIZER, all_tokenizer);
}

fn build_schema() -> Schema {
    let mut schema_builder = Schema::builder();
    schema_builder.add_i64_field("id", INDEXED | STORED | FAST); // 切片 id
    schema_builder.add_i64_field("file_id", INDEXED | STORED | FAST); // 文件 id
    schema_builder.add_i64_field("kb_id", INDEXED | STORED | FAST); // 知识库 id

    let text_options = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(ALL_TOKENIZER)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
        .set_stored();
    schema_builder.add_text_field("content", text_options); // 内容
    schema_builder.build()
}
