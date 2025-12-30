use std::path::Path;

use log::info;
use serde::Serialize;
use tantivy::{
    Index, Result, TantivyDocument, Term, collector::TopDocs, doc, query::{BooleanQuery, Occur, Query, TermQuery}, schema::{FAST, Field, INDEXED, IndexRecordOption, STORED, Schema, TextFieldIndexing, TextOptions, Value as _}
};

use super::chinese_tokenizer;

const INDEX_PATH: &str = "data/tantivy_index";
const ALL_TOKENIZER: &str = "all";
const INDEX_WRITER_MEMORY: usize = 50_000_000;
const SEARCH_LIMIT: usize = 10;

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

/// create new index
///
/// delete if it exists
pub fn init() -> Result<(Schema, Index)> {
    let schema = build_schema();
    let path = Path::new(INDEX_PATH);
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
    let mut index_writer = index.writer(INDEX_WRITER_MEMORY)?;
    index_writer.add_document(create_document(doc, schema))?;
    index_writer.commit()?;
    Ok(())
}

pub async fn search(
    index: &Index, schema: &Schema, query: &str, file_id: Option<i64>, kb_id: Option<i64>,
) -> anyhow::Result<Vec<SearchResultItem>> {
    let searcher = index.reader()?.searcher();
    let tantivy_query = build_query(query, file_id, kb_id, schema)?;
    let top_docs = searcher.search(&tantivy_query, &TopDocs::with_limit(SEARCH_LIMIT))?;

    let mut results = vec![];
    for (score, doc_address) in top_docs {
        let retrieved_doc: TantivyDocument = searcher.doc(doc_address)?;

        let id = retrieved_doc
            .get_first(get_field(schema, "id"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        let file_id = retrieved_doc
            .get_first(get_field(schema, "file_id"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        let kb_id = retrieved_doc
            .get_first(get_field(schema, "kb_id"))
            .and_then(|v| v.as_i64());

        let content = retrieved_doc
            .get_first(get_field(schema, "content"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();

        results.push(SearchResultItem {
            id,
            file_id,
            kb_id,
            content,
            score,
        });
    }

    Ok(results)
}

fn build_query(
    input: &str, file_id: Option<i64>, kb_id: Option<i64>, schema: &Schema,
) -> tantivy::Result<Box<dyn Query>> {
    let mut subqueries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
    let segmented_words = perform_segmentation(input, chinese_tokenizer::SegmentationMode::Search);
    for word in segmented_words {
        let term_query =
            TermQuery::new(Term::from_field_text(get_field(schema, "content"), &word), IndexRecordOption::Basic);
        subqueries.push((Occur::Should, Box::new(term_query)));
    }
    if let Some(file_id) = file_id {
        let file_id_query =
            TermQuery::new(Term::from_field_i64(get_field(schema, "file_id"), file_id), IndexRecordOption::Basic);
        subqueries.push((Occur::Must, Box::new(file_id_query)));
    }
    if let Some(kb_id) = kb_id {
        let kb_id_query =
            TermQuery::new(Term::from_field_i64(get_field(schema, "kb_id"), kb_id), IndexRecordOption::Basic);
        subqueries.push((Occur::Must, Box::new(kb_id_query)));
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
