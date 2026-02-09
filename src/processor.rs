use std::{
    collections::{HashMap, HashSet}, sync::Arc, time::Duration
};

use base64::{Engine, engine::general_purpose::STANDARD};
use futures::stream::{self, StreamExt};
use log::{debug, error, info, warn};
use lopdf::Document;
use reqwest::multipart;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use tokio::{fs, time};

use crate::{
    api::{
        File, collect_image_paths_for_files, find_reusable_parsed_file, remove_image_files, resolve_image_storage_path
    }, config, graph::{graph_manager::KnowledgeGraph, llm_extractor::LLMGraphExtractor}, search::{self, SearchEngine, tantivy_engine}
};

#[derive(Debug, Deserialize, Serialize)]
struct Result {
    #[serde(default)]
    content_list: String,
    #[serde(default)]
    images: HashMap<String, String>,
}

/// MinerU API 返回的结果结构
#[derive(Debug, Deserialize)]
struct MinerUResponse {
    results: MinerUResults,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MinerUResults {
    Map(HashMap<String, Result>),
    List(Vec<MinerUResultItem>),
}

#[derive(Debug, Deserialize)]
struct MinerUResultItem {
    #[serde(default)]
    status: String,
    #[serde(default)]
    error: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    content_list: String,
    #[serde(default)]
    images: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct AnalyzePdfResponse {
    code: i32,
    #[serde(default)]
    message: String,
    data: Option<AnalyzePdfData>,
}

#[derive(Debug, Deserialize)]
struct AnalyzePdfData {
    #[serde(default)]
    content_list: Vec<ContentItem>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AudioTranscriptionResponse {
    #[serde(default)]
    text: String,
    #[serde(default)]
    language: String,
}

#[derive(Debug, Deserialize)]
struct CustomParseResponse {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    message: String,
    data: Option<CustomParseData>,
}

#[derive(Debug, Deserialize)]
struct CustomParseData {
    #[serde(default)]
    slices: Vec<CustomSlice>,
    #[serde(default)]
    full_content: Option<String>,
    #[serde(default)]
    images: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct CustomSlice {
    content: String,
    #[serde(default)]
    positions: Vec<SlicePosition>,
}
/*
"type": "text",
"text": "维修服务信息",
"text_level": 1,
"bbox": [95, 198, 635, 265],
"page_idx": 0

"type": "image",
"img_path": "images/a1d809e1c746e15f0ea9510353cdf241cd32a9d00ae7eba19d6f2bc943230297.jpg",
"image_caption": [],
"image_footnote": [],
"bbox": [146, 558, 852, 892],
"page_idx": 0

"type": "table",
"img_path": "images/5471f25a6a8b460b6cfc320a3a1c925fd0b44d9ea3a66011a5521928996f1e39.jpg",
"table_caption": [],
"table_footnote": [],
"table_body": "<table><tr><td rowspan=1 colspan=4>油耗信息显示内容的多少与发动机有关</td></tr><tr><td rowspan=1 colspan=1>序号</td><td rowspan=1 colspan=1>内容</td><td rowspan=1 colspan=1>单位</td><td rowspan=1 colspan=1>描述</td></tr><tr><td rowspan=1 colspan=1>1</td><td rowspan=1 colspan=1>瞬时油耗</td><td rowspan=1 colspan=1>L/H</td><td rowspan=1 colspan=1>以当前的喷油量计算出1小时所消耗的油量</td></tr><tr><td rowspan=1 colspan=1>2</td><td rowspan=1 colspan=1>瞬时百公里油耗</td><td rowspan=1 colspan=1>L</td><td rowspan=1 colspan=1>在行驶时显示，以当前的喷油量计算出百公里所消耗的油量</td></tr><tr><td rowspan=1 colspan=1>3</td><td rowspan=1 colspan=1>百公里平均油耗</td><td rowspan=1 colspan=1>L</td><td rowspan=1 colspan=1>以本次运行过程中的平均燃油消耗量，计算出行驶100公里所消耗的油量</td></tr><tr><td rowspan=1 colspan=1>4</td><td rowspan=1 colspan=1>短途平均油耗</td><td rowspan=1 colspan=1>L</td><td rowspan=1 colspan=1>里程大于10Km/h时才会显示</td></tr><tr><td rowspan=1 colspan=1>5</td><td rowspan=1 colspan=1>小计油耗</td><td rowspan=1 colspan=1>L</td><td rowspan=1 colspan=1>本次发动机运行油耗显示（从启动到熄火)</td></tr><tr><td rowspan=1 colspan=1>6</td><td rowspan=1 colspan=1>总油耗</td><td rowspan=1 colspan=1>L</td><td rowspan=1 colspan=1>从发动机第一次运行开始到现在总计消耗的油量</td></tr><tr><td rowspan=1 colspan=1>7</td><td rowspan=1 colspan=1>发动机工作时间</td><td rowspan=1 colspan=1>H</td><td rowspan=1 colspan=1>从发动机第一次运行开始到现在的工作时间</td></tr><tr><td rowspan=1 colspan=4></td></tr><tr><td></td><td></td><td></td><td></td></tr></table>",
"bbox": [70, 138, 925, 916],
"page_idx": 12

"type": "equation",
"img_path": "images/544f359685e12c8500550eb3e209d1cdc4baa41933d8616b32d33a1df34d4a4e.jpg",
"text": "$$\\nf _ { 1 } ( \\\\mathit { t } ) * f _ { 2 } ( \\\\mathit { t } ) = f _ { 2 } ( \\\\mathit { t } ) * f _ { 1 } ( \\\\mathit { t } )\\n$$",
"text_format": "latex",
"bbox": [333, 755, 634, 777],
"page_idx": 89
*/
#[derive(Debug, Clone, Deserialize, Serialize)]
struct ContentItem {
    #[serde(default, rename = "type")]
    typ: String,
    #[serde(default)]
    bbox: Vec<i32>,
    #[serde(default)]
    page_idx: i32,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    text_level: Option<i32>,
    #[serde(default)]
    text_format: Option<String>, // latex
    #[serde(default)]
    img_path: Option<String>,
    #[serde(default)]
    image_caption: Option<Vec<String>>,
    #[serde(default)]
    table_body: Option<String>,
    #[serde(default)]
    table_caption: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
struct SlicePosition {
    page_idx: i32,
    bbox: [i32; 4],
}

#[derive(Debug, Clone)]
struct SliceWithPositions {
    content: String,
    positions: Vec<SlicePosition>,
}

#[derive(Debug, Clone)]
struct Segment {
    start: usize,
    end: usize,
    positions: Vec<SlicePosition>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct PdfContentRow {
    page_idx: i32,
    bbox: Option<String>,
    text: Option<String>,
    text_level: Option<i32>,
    img_path: Option<String>,
    table_body: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct SliceRow {
    id: i64,
    content: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct SlicePositionRecord {
    slice_id: i64,
    page_idx: i32,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
}

#[derive(Debug, Clone)]
struct ClonedSlice {
    old_id: i64,
    new_id: i64,
    content: String,
}

/// 文件处理器：定时从数据库读取未处理的文件并处理
pub struct FileProcessor {
    pool: SqlitePool,
    search_engine: search::SearchEngine,
    interval: Duration,
}

impl FileProcessor {
    /// 创建新的文件处理器
    ///
    /// # Arguments
    /// * `pool` - 数据库连接池
    /// * `search_engine` - 搜索引擎实例
    /// * `interval_secs` - 处理间隔（秒）
    pub fn new(pool: SqlitePool, search_engine: search::SearchEngine, interval_secs: u64) -> Self {
        Self { pool, search_engine, interval: Duration::from_secs(interval_secs) }
    }

    /// 启动后台处理任务
    pub fn start(self) {
        let processor = Arc::new(self);
        tokio::spawn(async move {
            info!("File processor started with interval: {:?}", processor.interval);

            if let Err(e) = processor.reset_processing_files().await {
                error!("Failed to reset in-progress files: {}", e);
            }

            loop {
                // 持续处理直到没有待处理的文件
                loop {
                    match processor.process_pending_files().await {
                        Ok(has_more) => {
                            if !has_more {
                                // 没有更多文件需要处理，退出内循环
                                debug!("No more pending files, waiting for next check");
                                break;
                            }
                            // 有更多文件，继续处理
                            debug!("More files pending, continuing processing");
                        }
                        Err(e) => {
                            error!("Error processing files: {}", e);
                            break;
                        }
                    }
                }

                // 等待指定时间后再次检查
                time::sleep(processor.interval).await;
            }
        });
    }

    /// 重置异常退出时处于“处理中”的文件状态
    async fn reset_processing_files(&self) -> anyhow::Result<()> {
        let file_ids: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM files WHERE status = 2").fetch_all(&self.pool).await?;

        if file_ids.is_empty() {
            return Ok(());
        }

        info!("Found {} in-progress files from previous run, resetting", file_ids.len());

        for file_id in file_ids {
            if let Err(e) = self.reset_processing_file_data(file_id).await {
                error!("Failed to reset processing file {}: {}", file_id, e);
            }
        }

        Ok(())
    }

    async fn reset_processing_file_data(&self, file_id: i64) -> anyhow::Result<()> {
        let image_paths = collect_image_paths_for_files(&self.pool, std::slice::from_ref(&file_id)).await?;
        let mut tx = self.pool.begin().await?;

        self.delete_processing_file_data(&mut tx, file_id).await?;
        sqlx::query("UPDATE files SET status = 0, log = '', updated_at = strftime('%s','now') WHERE id = ?")
            .bind(file_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        remove_image_files(image_paths).await;

        if let Err(e) = self.search_engine.delete(Some(file_id), None).await {
            warn!("Failed to delete search index for file {}: {}", file_id, e);
        }

        Ok(())
    }

    async fn cleanup_processing_file_data(&self, file_id: i64) -> anyhow::Result<()> {
        let image_paths = collect_image_paths_for_files(&self.pool, std::slice::from_ref(&file_id)).await?;
        let mut tx = self.pool.begin().await?;
        self.delete_processing_file_data(&mut tx, file_id).await?;
        tx.commit().await?;

        remove_image_files(image_paths).await;

        if let Err(e) = self.search_engine.delete(Some(file_id), None).await {
            warn!("Failed to delete search index for file {}: {}", file_id, e);
        }

        Ok(())
    }

    async fn delete_processing_file_data(
        &self, tx: &mut sqlx::Transaction<'_, Sqlite>, file_id: i64,
    ) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM entity_mentions WHERE slice_id IN (SELECT id FROM slices WHERE file_id = ?)")
            .bind(file_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM slice_positions WHERE slice_id IN (SELECT id FROM slices WHERE file_id = ?)")
            .bind(file_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM slices WHERE file_id = ?").bind(file_id).execute(&mut **tx).await?;
        sqlx::query("DELETE FROM pdf_contents WHERE file_id = ?").bind(file_id).execute(&mut **tx).await?;
        Ok(())
    }

    /// 处理所有待处理的文件
    /// 返回是否还有更多文件需要处理
    async fn process_pending_files(self: &Arc<Self>) -> anyhow::Result<bool> {
        let files = self.fetch_pending_files().await?;

        if files.is_empty() {
            return Ok(false);
        }

        let cfg = config::get();
        let concurrency = cfg.server.process_concurrency.max(1);
        info!("Found {} pending files to process (concurrency: {})", files.len(), concurrency);

        let results: Vec<_> = stream::iter(files)
            .map(|file| {
                let processor = Arc::clone(self);
                async move {
                    let result = processor.process_file(&file).await;
                    (file, result)
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

        for (file, result) in results {
            if let Err(e) = result {
                error!("Failed to process file {}: {}", file.id, e);
                if let Err(cleanup_err) = self.cleanup_processing_file_data(file.id).await {
                    error!("Failed to cleanup processing data for file {}: {}", file.id, cleanup_err);
                }
                self.mark_file_failed(file.id, &e.to_string()).await?;
            } else {
                info!("Successfully processed file {}", file.id);
            }
        }

        // 返回 true 表示可能还有更多文件（因为我们限制了每次查询的数量）
        Ok(true)
    }

    /// 从数据库获取未处理的文件
    async fn fetch_pending_files(&self) -> anyhow::Result<Vec<File>> {
        let sql = "SELECT * FROM files WHERE status = 0 ORDER BY created_at ASC LIMIT 10";
        let files = sqlx::query_as::<_, File>(sql).fetch_all(&self.pool).await?;
        Ok(files)
    }

    async fn try_reuse_existing_data(&self, file: &File) -> anyhow::Result<bool> {
        if !config::get().server.reuse_duplicate_files {
            return Ok(false);
        }
        let Some(source_file) =
            find_reusable_parsed_file(&self.pool, &file.hash, &file.slice_type, Some(file.id)).await?
        else {
            return Ok(false);
        };

        info!("Found reusable parsed file {} for new file {} (hash={})", source_file.id, file.id, file.hash);

        match self.clone_file_data(&source_file, file).await {
            Ok(_) => Ok(true),
            Err(err) => {
                error!("Failed to reuse parsed data from file {} for file {}: {}", source_file.id, file.id, err);
                if let Err(clean_err) = self.cleanup_processing_file_data(file.id).await {
                    warn!("Failed to cleanup file {} after reuse error: {}", file.id, clean_err);
                }
                sqlx::query("UPDATE files SET status = 0, log = ?, updated_at = strftime('%s','now') WHERE id = ?")
                    .bind(format!("Reuse failed: {}", err))
                    .bind(file.id)
                    .execute(&self.pool)
                    .await?;
                Ok(false)
            }
        }
    }

    /// 处理单个文件
    async fn process_file(&self, file: &File) -> anyhow::Result<()> {
        info!("Processing file: {} ({})", file.filename, file.id);
        if !self.ensure_file_exists(file.id, "process start").await? {
            return Ok(());
        }

        let current_status: Option<i32> = sqlx::query_scalar("SELECT status FROM files WHERE id = ?")
            .bind(file.id)
            .fetch_optional(&self.pool)
            .await?;
        if let Some(status) = current_status {
            if status != 0 {
                info!("File {} status changed to {}, skipping processing", file.id, status);
                return Ok(());
            }
        }

        if self.is_storage_kb(file.kb_id).await? {
            info!("Skipping parsing for storage knowledge base file {}", file.id);
            self.mark_file_storage_skipped(file.id).await?;
            return Ok(());
        }

        if self.try_reuse_existing_data(file).await? {
            info!("File {} reused existing parsed data, skipping processing pipeline", file.id);
            return Ok(());
        }

        let sql = "UPDATE files SET status = 2, updated_at = strftime('%s','now') WHERE id = ?";
        sqlx::query(sql).bind(file.id).execute(&self.pool).await?;

        // 检查文件是否为 PDF 或图片
        let filename_lower = file.filename.to_lowercase();
        let is_pdf = filename_lower.ends_with(".pdf");
        let is_image = Self::is_image_file(&filename_lower);
        let is_audio = Self::is_audio_file(&filename_lower);

        // 检查文件是否为 Word 或 Excel 文档
        let is_word = filename_lower.ends_with(".doc") || filename_lower.ends_with(".docx");
        let is_excel = filename_lower.ends_with(".xls") || filename_lower.ends_with(".xlsx");

        let cfg = config::get();
        let custom_url = cfg.services.custom_parse_url.as_deref();

        if is_word || is_excel {
            if !self.ensure_file_exists(file.id, "before office conversion").await? {
                return Ok(());
            }
            // Word/Excel 文档：先转换为 PDF，再按需要处理
            let doc_kind = if is_word { "Word" } else { "Excel" };
            info!("Detected {} document, converting to PDF: {}", doc_kind, file.filename);

            if let Some(custom_url) = custom_url {
                let stored_pdf_path = self.convert_office_to_pdf(file).await?;
                if is_word {
                    info!("Custom parse enabled, routing file {} to {}", file.id, custom_url);
                    self.process_file_with_custom_parser(file, custom_url).await?;
                    return Ok(());
                }

                let mut temp_file = file.clone();
                temp_file.path = stored_pdf_path.to_string_lossy().to_string();
                temp_file.filename = format!("{}.pdf", file.id);
                self.process_pdf_file(&temp_file, None, false, Some(file.filename.as_str())).await?;
                return Ok(());
            }

            self.convert_office_to_pdf_and_process(file).await?;
            return Ok(());
        }

        if let Some(custom_url) = custom_url {
            if is_pdf {
                info!("Custom parse enabled, routing file {} to {}", file.id, custom_url);
                self.process_file_with_custom_parser(file, custom_url).await?;
                return Ok(());
            }
        }

        if is_pdf {
            if !self.ensure_file_exists(file.id, "before pdf processing").await? {
                return Ok(());
            }
            // 处理 PDF 或图片文件
            self.process_pdf_file(file, None, false, None).await?;
        } else if is_image {
            if !self.ensure_file_exists(file.id, "before image embedding").await? {
                return Ok(());
            }
            let image_embedding =
                search::embedding::get_image_embedding_from_path(&file.path, Some(&file.filename)).await?;
            self.process_pdf_file(file, Some(image_embedding), true, None).await?;
        } else if is_audio {
            if !self.ensure_file_exists(file.id, "before audio processing").await? {
                return Ok(());
            }
            self.process_audio_file(file).await?;
        } else {
            if !self.ensure_file_exists(file.id, "before text processing").await? {
                return Ok(());
            }
            // 处理普通文本文件
            self.process_text_file(file).await?;
        }

        Ok(())
    }

    async fn process_file_with_custom_parser(&self, file: &File, custom_url: &str) -> anyhow::Result<()> {
        if !self.ensure_file_exists(file.id, "custom parse start").await? {
            return Ok(());
        }

        let parse_data = self.call_custom_parse_api(file, custom_url).await?;

        if let Some(images) = parse_data.images.as_ref() {
            if !images.is_empty() {
                self.save_custom_images(images).await?;
            }
        }

        self.save_custom_slices(file, &parse_data.slices, parse_data.full_content.as_deref()).await?;

        Ok(())
    }

    async fn call_custom_parse_api(&self, file: &File, custom_url: &str) -> anyhow::Result<CustomParseData> {
        let file_bytes = tokio::fs::read(&file.path).await?;
        let mime_type = mime_guess::from_path(&file.filename).first_or_octet_stream().essence_str().to_string();
        let form = multipart::Form::new()
            .part("file", multipart::Part::bytes(file_bytes).file_name(file.filename.clone()).mime_str(&mime_type)?);

        let client = reqwest::Client::new();
        let response = client.post(custom_url).multipart(form).send().await?;
        let status = response.status();
        let body_bytes = response.bytes().await?;
        if !status.is_success() {
            let error_text = String::from_utf8_lossy(&body_bytes);
            return Err(anyhow::anyhow!("Custom parse API failed: {}", error_text));
        }

        let parsed: CustomParseResponse = serde_json::from_slice(&body_bytes).map_err(|e| {
            let body_text = String::from_utf8_lossy(&body_bytes);
            anyhow::anyhow!("Custom parse API response decode failed: {} - {}", e, body_text)
        })?;

        if parsed.code != 200 {
            let msg =
                if parsed.message.is_empty() { "Custom parse API returned error".to_string() } else { parsed.message };
            return Err(anyhow::anyhow!("{}", msg));
        }

        let data = parsed.data.ok_or_else(|| anyhow::anyhow!("Custom parse API returned empty data"))?;
        if data.slices.is_empty() {
            return Err(anyhow::anyhow!("Custom parse API returned empty slices"));
        }

        Ok(data)
    }

    async fn save_custom_slices(
        &self, file: &File, slices: &[CustomSlice], full_content: Option<&str>,
    ) -> anyhow::Result<()> {
        if !self.ensure_file_exists(file.id, "before writing custom slices").await? {
            return Ok(());
        }

        let derived_full_content = match full_content {
            Some(content) if !content.trim().is_empty() => content.to_string(),
            _ => {
                let mut combined = String::new();
                for (idx, slice) in slices.iter().enumerate() {
                    if idx > 0 {
                        combined.push_str("\n\n");
                    }
                    combined.push_str(&slice.content);
                }
                combined
            }
        };

        let mut position_rows: Vec<(i64, SlicePosition)> = Vec::new();
        let mut search_docs = Vec::new();

        for slice in slices {
            let sql = "INSERT INTO slices (file_id, content) VALUES (?, ?)";
            let id = sqlx::query(sql).bind(file.id).bind(&slice.content).execute(&self.pool).await?.last_insert_rowid();
            for position in &slice.positions {
                position_rows.push((id, position.clone()));
            }
            search_docs.push(tantivy_engine::Document::new(id, file.id, file.kb_id, slice.content.clone()));
        }

        if !search_docs.is_empty() {
            let embeddings = vec![None; search_docs.len()];
            self.search_engine.write_batch(search_docs, embeddings).await?;
        }

        if !position_rows.is_empty() {
            let binds_per_row = 6_usize;
            let max_vars = 999_usize;
            let batch_size = std::cmp::max(1, max_vars / binds_per_row);
            for chunk in position_rows.chunks(batch_size) {
                let mut pos_sql =
                    QueryBuilder::<Sqlite>::new("insert into slice_positions(slice_id, page_idx, x1, y1, x2, y2) ");
                pos_sql.push_values(chunk.iter(), |mut b, (slice_id, position)| {
                    b.push_bind(slice_id)
                        .push_bind(position.page_idx)
                        .push_bind(position.bbox[0])
                        .push_bind(position.bbox[1])
                        .push_bind(position.bbox[2])
                        .push_bind(position.bbox[3]);
                });
                pos_sql.build().execute(&self.pool).await?;
            }
        }

        if !self.ensure_file_exists(file.id, "before writing custom full index").await? {
            return Ok(());
        }
        let index_full_content = format!("{}\n\n{}", file.filename, derived_full_content);
        self.search_engine
            .write_full(tantivy_engine::Document::new(file.id, file.id, file.kb_id, index_full_content))
            .await?;

        if !self.ensure_file_exists(file.id, "before updating custom status").await? {
            return Ok(());
        }
        let sql = "UPDATE files SET status = 1, content = ?, log = ?, updated_at = strftime('%s','now') WHERE id = ?";
        sqlx::query(sql)
            .bind(&derived_full_content)
            .bind("Custom parse processed successfully")
            .bind(file.id)
            .execute(&self.pool)
            .await?;

        info!("File {} processed successfully with {} custom slices", file.id, slices.len());

        if let Err(e) = self.build_knowledge_graph(file).await {
            error!("Failed to build knowledge graph for file {}: {}", file.id, e);
        }

        Ok(())
    }

    async fn save_custom_images(&self, images: &HashMap<String, String>) -> anyhow::Result<()> {
        let cfg = config::get();
        fs::create_dir_all(&cfg.storage.images_path).await?;
        info!("custom image count: {}", images.len());
        for (img_name, img_base64) in images {
            let payload = match img_base64.find("base64,") {
                Some(idx) => &img_base64[idx + "base64,".len()..],
                None => img_base64.as_str(),
            };
            let bytes = STANDARD.decode(payload)?;
            fs::write(format!("{}/{}", cfg.storage.images_path, img_name), bytes).await?;
        }
        Ok(())
    }

    /// 将 Word/Excel 文档转换为 PDF
    async fn convert_office_to_pdf(&self, file: &File) -> anyhow::Result<std::path::PathBuf> {
        // 创建临时目录用于存放转换后的 PDF
        let cfg = config::get();
        let temp_dir = std::path::Path::new(&cfg.storage.temp_path);
        fs::create_dir_all(temp_dir).await?;
        let pdf_dir = std::path::Path::new(&cfg.storage.pdf_path);
        fs::create_dir_all(pdf_dir).await?;

        // 生成临时 PDF 文件路径
        let pdf_filename = format!("{}.pdf", file.id);

        // 使用 LibreOffice 将 Word/Excel 转换为 PDF
        // 注意：这需要系统中安装了 LibreOffice
        let mut converted_ok = false;
        let mut last_error: Option<String> = None;
        let convert_timeout = Duration::from_secs(120);
        for attempt in 0..2 {
            let mut command = tokio::process::Command::new("soffice");
            command
                .args(&["--headless", "--convert-to", "pdf", "--outdir", temp_dir.to_str().unwrap(), &file.path])
                .kill_on_drop(true);

            let output = match time::timeout(convert_timeout, command.output()).await {
                Ok(result) => result?,
                Err(_) => {
                    last_error = Some(format!("LibreOffice timed out after {}s", convert_timeout.as_secs()));
                    if attempt == 0 {
                        warn!(
                            "LibreOffice convert timed out, retrying in 3s: {}",
                            last_error.as_deref().unwrap_or("unknown error")
                        );
                        time::sleep(Duration::from_secs(3)).await;
                    }
                    continue;
                }
            };

            if output.status.success() {
                converted_ok = true;
                break;
            }

            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let error_msg =
                if stderr.is_empty() { format!("LibreOffice exited with {}", output.status) } else { stderr };
            last_error = Some(error_msg);

            if attempt == 0 {
                warn!(
                    "LibreOffice convert failed, retrying in 3s: {}",
                    last_error.as_deref().unwrap_or("unknown error")
                );
                time::sleep(Duration::from_secs(3)).await;
            }
        }

        if !converted_ok {
            let error_msg = last_error.unwrap_or_else(|| "unknown error".to_string());
            return Err(anyhow::anyhow!("Failed to convert office document to PDF: {}", error_msg));
        }

        // 查找生成的 PDF 文件
        // LibreOffice 会使用原文件名（不含扩展名）+ .pdf
        let converted_pdf_path = temp_dir.join(format!(
            "{}.pdf",
            std::path::Path::new(&file.path)
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| anyhow::anyhow!("Invalid file path: {}", file.path))?
        ));
        let stored_pdf_path = pdf_dir.join(&pdf_filename);

        // 检查转换后的 PDF 是否存在
        if !tokio::fs::try_exists(&converted_pdf_path).await? {
            return Err(anyhow::anyhow!("Converted PDF file not found: {:?}", converted_pdf_path));
        }

        // 保存转换后的 PDF 以便溯源高亮
        fs::copy(&converted_pdf_path, &stored_pdf_path).await?;

        // 清理临时 PDF 文件
        let _ = fs::remove_file(&converted_pdf_path).await;

        Ok(stored_pdf_path)
    }

    /// 将 Word/Excel 文档转换为 PDF 并处理
    async fn convert_office_to_pdf_and_process(&self, file: &File) -> anyhow::Result<()> {
        let stored_pdf_path = self.convert_office_to_pdf(file).await?;

        // 创建临时 File 结构用于处理 PDF
        let mut temp_file = file.clone();
        temp_file.path = stored_pdf_path.to_string_lossy().to_string();
        temp_file.filename = format!("{}.pdf", file.id);

        // 使用 process_pdf_file 处理转换后的 PDF
        self.process_pdf_file(&temp_file, None, false, Some(file.filename.as_str())).await
    }

    /// 处理 PDF 文件，调用 MinerU API
    async fn process_pdf_file(
        &self, file: &File, image_embedding: Option<Vec<f32>>, is_image: bool, index_filename: Option<&str>,
    ) -> anyhow::Result<()> {
        if !self.ensure_file_exists(file.id, "pdf processing start").await? {
            return Ok(());
        }
        info!("Processing PDF file: {}", file.filename);

        // 先检查 pdf_contents 表中是否已有该文件的数据
        let check_sql = "SELECT COUNT(*) as count FROM pdf_contents WHERE file_id = ?";
        let count: (i64,) = sqlx::query_as(check_sql).bind(file.id).fetch_one(&self.pool).await?;
        let bbox_sql =
            "SELECT COUNT(*) as count FROM pdf_contents WHERE file_id = ? AND bbox IS NOT NULL AND bbox != ''";
        let bbox_count: (i64,) = sqlx::query_as(bbox_sql).bind(file.id).fetch_one(&self.pool).await?;

        let mut content_list: Vec<ContentItem>;

        if count.0 > 0 && bbox_count.0 > 0 {
            // 已有数据，直接从数据库读取
            info!("Found existing PDF contents in database for file {}, skipping MinerU API call", file.id);

            let fetch_sql = "SELECT page_idx, bbox, text, text_level, img_path, table_body FROM pdf_contents WHERE file_id = ? ORDER BY page_idx, id";
            let rows: Vec<(i32, Option<String>, Option<String>, Option<i32>, Option<String>, Option<String>)> =
                sqlx::query_as(fetch_sql).bind(file.id).fetch_all(&self.pool).await?;

            // 将数据库记录转换为 ContentItem
            content_list = rows
                .iter()
                .map(|row| {
                    let typ = if row.2.is_some() {
                        "text".to_string()
                    } else if row.4.is_some() {
                        "image".to_string()
                    } else if row.5.is_some() {
                        "table".to_string()
                    } else {
                        "unknown".to_string()
                    };

                    let bbox =
                        row.1.as_ref().and_then(|bbox| serde_json::from_str::<Vec<i32>>(bbox).ok()).unwrap_or_default();

                    ContentItem {
                        typ,
                        bbox,
                        page_idx: row.0,
                        text: row.2.clone(),
                        text_level: row.3,
                        text_format: None,
                        img_path: row.4.clone(),
                        image_caption: None,
                        table_body: row.5.clone(),
                        table_caption: None,
                    }
                })
                .collect();
        } else {
            // 没有数据，调用 MinerU API
            let mut mineru_result = self.call_mineru_api(file, is_image).await?;
            content_list = serde_json::from_str(&mineru_result.content_list)?;
            Self::prefix_images_for_file(file.id, &mut content_list, &mut mineru_result.images);

            if !self.ensure_file_exists(file.id, "before writing pdf contents").await? {
                return Ok(());
            }

            // 提取文本内容并过滤掉 discarded 项
            let valid_content_items: Vec<ContentItem> =
                content_list.iter().filter(|item| item.typ != "discarded").cloned().collect();

            // 只有在有有效内容时才插入数据库
            if !valid_content_items.is_empty() {
                if count.0 > 0 {
                    sqlx::query("DELETE FROM pdf_contents WHERE file_id = ?").bind(file.id).execute(&self.pool).await?;
                }
                let binds_per_row = 7_usize;
                let max_vars = 999_usize;
                let batch_size = std::cmp::max(1, max_vars / binds_per_row);
                for chunk in valid_content_items.chunks(batch_size) {
                    let mut pdf_sql = QueryBuilder::<Sqlite>::new(
                        "insert into pdf_contents(file_id, page_idx, bbox, text, text_level, img_path, table_body) ",
                    );
                    pdf_sql.push_values(chunk.iter(), |mut b, item| {
                        let bbox = if item.bbox.is_empty() {
                            None
                        } else {
                            Some(serde_json::to_string(&item.bbox).unwrap_or_default())
                        };
                        b.push_bind(file.id)
                            .push_bind(item.page_idx)
                            .push_bind(bbox)
                            .push_bind(&item.text)
                            .push_bind(item.text_level)
                            .push_bind(&item.img_path)
                            .push_bind(&item.table_body);
                    });
                    pdf_sql.build().execute(&self.pool).await?;
                }
            }

            // 保存图片到本地
            let cfg = config::get();
            fs::create_dir_all(&cfg.storage.images_path).await?;
            info!("image count: {}", mineru_result.images.len());
            info!("images: {:?}", mineru_result.images.keys());
            for (img_name, img_base64) in &mineru_result.images {
                // 保存图片，如果以 data:image/jpeg;base64, 开头就去掉，没有也不报错
                let b64 = if let Some(stripped) = img_base64.strip_prefix("data:image/jpeg;base64,") {
                    stripped
                } else {
                    img_base64.as_str()
                };
                let bytes = STANDARD.decode(b64)?;
                fs::write(format!("{}/{}", cfg.storage.images_path, img_name), bytes).await?;
            }
        }

        let (full_content, full_segments) = self.build_full_content_and_segments(&content_list);

        // 根据 slice_type 决定切片方式
        let slices = if file.slice_type == "smart" || file.slice_type.is_empty() {
            // 智能切片：使用 content_list
            self.smart_slice_content_with_positions(&content_list)?
        } else if file.slice_type == "fixed" {
            self.fixed_slice_content_with_positions(&full_content, &full_segments)?
        } else {
            self.slice_content(&full_content, &file.slice_type)?
                .into_iter()
                .map(|content| SliceWithPositions { content, positions: vec![] })
                .collect()
        };

        if !self.ensure_file_exists(file.id, "before writing slices").await? {
            return Ok(());
        }

        let slice_count = slices.len();
        let mut position_rows: Vec<(i64, SlicePosition)> = Vec::new();
        let mut search_docs = Vec::new();
        let mut search_embeddings = Vec::new();

        for slice in slices {
            let sql = "INSERT INTO slices (file_id, content) VALUES (?, ?)";
            let id = sqlx::query(sql).bind(file.id).bind(&slice.content).execute(&self.pool).await?.last_insert_rowid();
            if !slice.positions.is_empty() {
                for position in slice.positions {
                    position_rows.push((id, position));
                }
            }
            // 收集文档，稍后批量写入
            search_docs.push(tantivy_engine::Document::new(id, file.id, file.kb_id, slice.content));
            search_embeddings.push(image_embedding.clone());
        }

        // 批量写入搜索引擎
        if !search_docs.is_empty() {
            self.search_engine.write_batch(search_docs, search_embeddings).await?;
        }

        if !position_rows.is_empty() {
            let binds_per_row = 6_usize;
            let max_vars = 999_usize;
            let batch_size = std::cmp::max(1, max_vars / binds_per_row);
            for chunk in position_rows.chunks(batch_size) {
                let mut pos_sql =
                    QueryBuilder::<Sqlite>::new("insert into slice_positions(slice_id, page_idx, x1, y1, x2, y2) ");
                pos_sql.push_values(chunk.iter(), |mut b, (slice_id, position)| {
                    b.push_bind(slice_id)
                        .push_bind(position.page_idx)
                        .push_bind(position.bbox[0])
                        .push_bind(position.bbox[1])
                        .push_bind(position.bbox[2])
                        .push_bind(position.bbox[3]);
                });
                pos_sql.build().execute(&self.pool).await?;
            }
        }

        // 构建知识图谱
        if let Err(e) = self.build_knowledge_graph(file).await {
            error!("Failed to build knowledge graph for file {}: {}", file.id, e);
            // 不影响主流程，仅记录错误
        }

        if !self.ensure_file_exists(file.id, "before writing full index").await? {
            return Ok(());
        }

        let index_filename = index_filename.unwrap_or(file.filename.as_str());
        let index_full_content = format!("{}\n\n{}", index_filename, full_content);
        debug!("write full content: {}", index_full_content);
        self.search_engine
            .write_full(tantivy_engine::Document::new(file.id, file.id, file.kb_id, index_full_content))
            .await?;

        // 更新文件状态
        if !self.ensure_file_exists(file.id, "before updating status").await? {
            return Ok(());
        }
        let sql = "UPDATE files SET status = 1, content = ?, log = ?, updated_at = strftime('%s','now') WHERE id = ?";
        sqlx::query(sql)
            .bind(&full_content)
            .bind("PDF processed successfully")
            .bind(file.id)
            .execute(&self.pool)
            .await?;

        info!("PDF file {} processed successfully with {} slices", file.id, slice_count);

        Ok(())
    }

    async fn call_mineru_api(&self, file: &File, is_image: bool) -> anyhow::Result<Result> {
        let cfg = config::get();

        if !is_image && cfg.services.mineru_max_pages > 0 {
            match Document::load(&file.path) {
                Ok(doc) => {
                    let total_pages = doc.get_pages().len();
                    if total_pages > cfg.services.mineru_max_pages {
                        return self.call_mineru_api_in_ranges(file, total_pages, cfg.services.mineru_max_pages).await;
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to read PDF page count for {}: {}, falling back to single MinerU request",
                        file.filename, e
                    );
                }
            }
        }

        self.call_mineru_api_with_path(&file.path, &file.filename, is_image, None, None).await
    }

    async fn call_mineru_api_in_ranges(
        &self, file: &File, total_pages: usize, max_pages: usize,
    ) -> anyhow::Result<Result> {
        let total_parts = (total_pages + max_pages - 1) / max_pages;
        info!(
            "PDF {} has {} pages, calling MinerU in {} ranges (max {} pages per range)",
            file.filename, total_pages, total_parts, max_pages
        );

        let mut merged_items: Vec<ContentItem> = Vec::new();
        let mut merged_images: HashMap<String, String> = HashMap::new();

        for (part_index, range_start) in (0..total_pages).step_by(max_pages).enumerate() {
            let range_end = std::cmp::min(range_start + max_pages, total_pages) - 1;
            let chunk_filename = format!("{}_part_{}.pdf", file.filename, part_index + 1);

            let chunk_result = self
                .call_mineru_api_with_path(&file.path, &chunk_filename, false, Some(range_start), Some(range_end))
                .await?;
            let mut chunk_items: Vec<ContentItem> = serde_json::from_str(&chunk_result.content_list)?;

            let image_prefix = format!("part{}_", part_index + 1);
            let min_page_idx = chunk_items.iter().map(|item| item.page_idx).min();
            let needs_offset = min_page_idx.map_or(false, |min| min < range_start as i32);
            for item in &mut chunk_items {
                if needs_offset {
                    item.page_idx += range_start as i32;
                }
                if let Some(img_path) = item.img_path.as_deref() {
                    item.img_path = Some(Self::prefix_image_path(img_path, &image_prefix));
                }
            }

            for (img_name, img_base64) in chunk_result.images {
                let prefixed = Self::prefix_image_path(&img_name, &image_prefix);
                merged_images.insert(prefixed, img_base64);
            }

            merged_items.extend(chunk_items);
        }

        let content_list = serde_json::to_string(&merged_items)?;
        Ok(Result { content_list, images: merged_images })
    }

    fn prefix_image_path(img_path: &str, prefix: &str) -> String {
        match img_path.rsplit_once('/') {
            Some((dir, name)) => format!("{}/{}{}", dir, prefix, name),
            None => format!("{}{}", prefix, img_path),
        }
    }

    fn prefix_images_for_file(file_id: i64, content_list: &mut [ContentItem], images: &mut HashMap<String, String>) {
        if images.is_empty() {
            return;
        }
        let prefix = format!("f{}_", file_id);
        let mut renamed: HashMap<String, String> = HashMap::new();
        for item in content_list.iter_mut() {
            let Some(img_path) = item.img_path.as_deref() else { continue };
            let new_name =
                renamed.entry(img_path.to_string()).or_insert_with(|| Self::prefix_image_path(img_path, &prefix));
            item.img_path = Some(new_name.clone());
        }

        let mut new_images = HashMap::new();
        for (img_name, img_base64) in images.drain() {
            let new_name =
                renamed.get(&img_name).cloned().unwrap_or_else(|| Self::prefix_image_path(&img_name, &prefix));
            new_images.insert(new_name, img_base64);
        }
        *images = new_images;
    }

    async fn call_mineru_api_with_path(
        &self, file_path: &str, filename: &str, is_image: bool, start_page: Option<usize>, end_page: Option<usize>,
    ) -> anyhow::Result<Result> {
        let file_bytes = tokio::fs::read(file_path).await?;
        let cfg = config::get();
        let mineru_url = cfg.services.mineru_url.trim_end_matches('/');

        let client = reqwest::Client::new();
        if mineru_url.ends_with("/file_parse") {
            // 构建 multipart form
            let mime_type = if is_image {
                mime_guess::from_path(filename).first_or_octet_stream().essence_str().to_string()
            } else {
                "application/pdf".to_string()
            };

            let mut form = multipart::Form::new()
                .text("return_middle_json", "false")
                .text("return_model_output", "false")
                .text("return_md", "false")
                .text("return_images", "true")
                .text("parse_method", "auto")
                .text("lang_list", "ch")
                .text("output_dir", "")
                .text("server_url", "string")
                .text("return_content_list", "true")
                .text("backend", "pipeline")
                .text("table_enable", "true")
                .text("response_format_zip", "false")
                .text("formula_enable", "true")
                .part(
                    "files",
                    multipart::Part::bytes(file_bytes).file_name(filename.to_string()).mime_str(&mime_type)?,
                );
            let start_page_id = start_page.unwrap_or(0);
            let end_page_id = end_page.map(|v| v.to_string()).unwrap_or_else(|| "99999".to_string());
            form = form.text("start_page_id", start_page_id.to_string()).text("end_page_id", end_page_id);

            // 调用 MinerU API
            let response = client.post(mineru_url).multipart(form).send().await?;
            let status = response.status();
            let body_bytes = response.bytes().await?;
            if !status.is_success() {
                let error_text = String::from_utf8_lossy(&body_bytes);
                return Err(anyhow::anyhow!("MinerU API failed: {}", error_text));
            }

            let mineru_response: MinerUResponse = serde_json::from_slice(&body_bytes).map_err(|e| {
                let body_text = String::from_utf8_lossy(&body_bytes);
                anyhow::anyhow!("MinerU API response decode failed: {} - {}", e, body_text)
            })?;
            let mineru_result = match mineru_response.results {
                MinerUResults::Map(results) => {
                    results.into_values().next().ok_or_else(|| anyhow::anyhow!("MinerU API returned empty results"))?
                }
                MinerUResults::List(results) => {
                    if let Some(item) = results.iter().find(|item| item.status == "error") {
                        let mut error_msg = item.error.clone();
                        if error_msg.is_empty() {
                            error_msg = "MinerU API returned error result".to_string();
                        }
                        if !item.filename.is_empty() {
                            error_msg = format!("{} (file: {})", error_msg, item.filename);
                        }
                        return Err(anyhow::anyhow!("MinerU API failed: {}", error_msg));
                    }
                    let first = results
                        .into_iter()
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("MinerU API returned empty results"))?;
                    Result { content_list: first.content_list, images: first.images }
                }
            };
            return Ok(mineru_result);
        }

        let mut url = reqwest::Url::parse(mineru_url)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("backend", "pipeline");
            query.append_pair("method", "auto");
            query.append_pair("lang", "ch");
            query.append_pair("start_page", &start_page.unwrap_or(0).to_string());
            if let Some(end_page) = end_page {
                query.append_pair("end_page", &end_page.to_string());
            }
            query.append_pair("formula_enable", "true");
            query.append_pair("table_enable", "true");
        }

        let mime_type = mime_guess::from_path(filename).first_or_octet_stream().essence_str().to_string();
        let form = multipart::Form::new()
            .part("file", multipart::Part::bytes(file_bytes).file_name(filename.to_string()).mime_str(&mime_type)?);

        let response = client.post(url.clone()).multipart(form).send().await?;
        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!("MinerU API failed: {}", error_text));
        }

        let analyze_response: AnalyzePdfResponse = response.json().await?;
        if analyze_response.code != 200 {
            return Err(anyhow::anyhow!("MinerU API failed: {}", analyze_response.message));
        }

        let Some(data) = analyze_response.data else {
            return Err(anyhow::anyhow!("MinerU API returned empty data"));
        };

        let host = url.host_str().ok_or_else(|| anyhow::anyhow!("MinerU API URL missing host"))?;
        let origin = if let Some(port) = url.port() {
            format!("{}://{}:{}", url.scheme(), host, port)
        } else {
            format!("{}://{}", url.scheme(), host)
        };

        let mut content_list = data.content_list;
        let mut images = HashMap::new();
        for item in &mut content_list {
            let Some(img_path) = item.img_path.as_deref() else { continue };
            let filename = std::path::Path::new(img_path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(img_path)
                .to_string();

            if !images.contains_key(&filename) {
                let image_url = format!("{}/output/{}", origin, img_path.trim_start_matches('/'));
                let image_response = client.get(&image_url).send().await?;
                if !image_response.status().is_success() {
                    let error_text = image_response.text().await?;
                    return Err(anyhow::anyhow!("MinerU image download failed: {}", error_text));
                }
                let image_bytes = image_response.bytes().await?;
                let image_mime = mime_guess::from_path(&filename).first_or_octet_stream().essence_str().to_string();
                let image_base64 = STANDARD.encode(image_bytes);
                images.insert(filename.clone(), format!("data:{};base64,{}", image_mime, image_base64));
            }

            item.img_path = Some(filename);
        }

        let content_list_json = serde_json::to_string(&content_list)?;
        Ok(Result { content_list: content_list_json, images })
    }

    /// 处理普通文本文件
    async fn process_text_file(&self, file: &File) -> anyhow::Result<()> {
        if !self.ensure_file_exists(file.id, "before reading text").await? {
            return Ok(());
        }
        // 示例：读取文件内容
        let content = tokio::fs::read_to_string(file.path.as_str()).await?;
        self.process_plain_text_content(file, &content, "Processing completed successfully").await
    }

    async fn process_audio_file(&self, file: &File) -> anyhow::Result<()> {
        info!("Processing audio file: {}", file.filename);

        if !self.ensure_file_exists(file.id, "before reading audio").await? {
            return Ok(());
        }
        let file_bytes = tokio::fs::read(&file.path).await?;
        let mime_type = mime_guess::from_path(&file.filename).first_or_octet_stream().essence_str().to_string();

        let form = multipart::Form::new()
            .part("file", multipart::Part::bytes(file_bytes).file_name(file.filename.clone()).mime_str(&mime_type)?);

        let client = reqwest::Client::new();
        let cfg = config::get();
        let mut req_builder = client.post(&cfg.services.audio_transcription_url).multipart(form);
        if let Some(key) = &cfg.services.audio_transcription_key {
            if !key.is_empty() {
                req_builder = req_builder.header("Authorization", format!("Bearer {}", key));
            }
        }

        let response = req_builder.send().await?;
        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!("Audio transcription API failed: {}", error_text));
        }

        let AudioTranscriptionResponse { text, language } = response.json().await?;
        let log_message = if language.is_empty() {
            "Audio processed successfully".to_string()
        } else {
            format!("Audio processed successfully (language: {})", language)
        };

        self.process_plain_text_content(file, &text, &log_message).await
    }

    async fn process_plain_text_content(&self, file: &File, content: &str, log_message: &str) -> anyhow::Result<()> {
        if !self.ensure_file_exists(file.id, "before writing slices").await? {
            return Ok(());
        }
        // 示例：根据 slice_type 进行分片处理
        let slices = self.slice_content(content, &file.slice_type)?;
        let slice_count = slices.len();

        // 保存分片到数据库并收集文档
        let mut search_docs = Vec::new();
        for slice in slices {
            let sql = "INSERT INTO slices (file_id, content) VALUES (?, ?)";
            let id = sqlx::query(sql).bind(file.id).bind(&slice).execute(&self.pool).await?.last_insert_rowid();
            search_docs.push(tantivy_engine::Document::new(id, file.id, file.kb_id, slice));
        }

        // 批量写入搜索引擎
        if !search_docs.is_empty() {
            let embeddings = vec![None; search_docs.len()];
            self.search_engine.write_batch(search_docs, embeddings).await?;
        }

        if !self.ensure_file_exists(file.id, "before writing full index").await? {
            return Ok(());
        }
        let index_full_content = format!("{}\n\n{}", file.filename, content);
        self.search_engine
            .write_full(tantivy_engine::Document::new(file.id, file.id, file.kb_id, index_full_content))
            .await?;

        // 更新文件状态为已处理，并保存内容
        if !self.ensure_file_exists(file.id, "before updating status").await? {
            return Ok(());
        }
        let sql = "UPDATE files SET status = 1, content = ?, log = ?, updated_at = strftime('%s','now') WHERE id = ?";
        sqlx::query(sql).bind(content).bind(log_message).bind(file.id).execute(&self.pool).await?;

        info!("File {} processed successfully with {} slices", file.id, slice_count);

        // 构建知识图谱
        if let Err(e) = self.build_knowledge_graph(file).await {
            error!("Failed to build knowledge graph for file {}: {}", file.id, e);
            // 不影响主流程，仅记录错误
        }

        Ok(())
    }

    /// 标记文件处理失败
    async fn mark_file_failed(&self, file_id: i64, error_msg: &str) -> anyhow::Result<()> {
        let sql = "UPDATE files SET status = -1, log = ?, updated_at = strftime('%s','now') WHERE id = ?";
        sqlx::query(sql).bind(error_msg).bind(file_id).execute(&self.pool).await?;
        Ok(())
    }

    async fn mark_file_storage_skipped(&self, file_id: i64) -> anyhow::Result<()> {
        let sql = "UPDATE files SET status = 3, log = ?, updated_at = strftime('%s','now') WHERE id = ?";
        sqlx::query(sql).bind("Storage mode: not parsed").bind(file_id).execute(&self.pool).await?;
        Ok(())
    }

    async fn ensure_file_exists(&self, file_id: i64, stage: &str) -> anyhow::Result<bool> {
        let exists = sqlx::query_scalar::<_, i64>("SELECT 1 FROM files WHERE id = ? LIMIT 1")
            .bind(file_id)
            .fetch_optional(&self.pool)
            .await?
            .is_some();
        if !exists {
            info!("File {} no longer exists during {}, skipping further processing", file_id, stage);
        }
        Ok(exists)
    }

    async fn is_storage_kb(&self, kb_id: Option<i64>) -> anyhow::Result<bool> {
        let Some(kb_id) = kb_id else { return Ok(false) };
        let kb_type: Option<String> = sqlx::query_scalar("SELECT kb_type FROM knowledge_bases WHERE id = ?")
            .bind(kb_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(matches!(kb_type.as_deref(), Some("storage")))
    }

    fn is_image_file(filename_lower: &str) -> bool {
        filename_lower.ends_with(".jpg")
            || filename_lower.ends_with(".jpeg")
            || filename_lower.ends_with(".png")
            || filename_lower.ends_with(".gif")
            || filename_lower.ends_with(".bmp")
            || filename_lower.ends_with(".webp")
            || filename_lower.ends_with(".tiff")
            || filename_lower.ends_with(".tif")
            || filename_lower.ends_with(".svg")
            || filename_lower.ends_with(".ico")
            || filename_lower.ends_with(".heic")
            || filename_lower.ends_with(".heif")
    }

    fn is_audio_file(filename_lower: &str) -> bool {
        filename_lower.ends_with(".wav")
            || filename_lower.ends_with(".mp3")
            || filename_lower.ends_with(".m4a")
            || filename_lower.ends_with(".aac")
            || filename_lower.ends_with(".flac")
            || filename_lower.ends_with(".ogg")
            || filename_lower.ends_with(".opus")
            || filename_lower.ends_with(".wma")
            || filename_lower.ends_with(".amr")
            || filename_lower.ends_with(".aiff")
            || filename_lower.ends_with(".aif")
            || filename_lower.ends_with(".alac")
            || filename_lower.ends_with(".webm")
    }

    /// 根据 slice_type 对内容进行分片
    fn slice_content(&self, content: &str, slice_type: &str) -> anyhow::Result<Vec<String>> {
        match slice_type {
            "paragraph" => {
                // 按段落分片（以双换行符分隔）
                Ok(content.split("\n\n").map(|s| s.trim()).filter(|s| !s.is_empty()).map(|s| s.to_string()).collect())
            }
            "fixed" => {
                // 固定长度分片（每8000字符，重叠100字符）
                let cfg = config::get();
                let chunk_size = cfg.slice.smart_slice_max_chars;
                let overlap = cfg.slice.fixed_slice_overlap_chars;

                let chars: Vec<char> = content.chars().collect();
                let mut slices = Vec::new();
                let mut start = 0;

                while start < chars.len() {
                    let end = std::cmp::min(start + chunk_size, chars.len());
                    let slice: String = chars[start..end].iter().collect();
                    slices.push(slice);

                    if end >= chars.len() {
                        break;
                    }

                    // 下一个切片从 end - overlap 开始，以实现重叠
                    start = end.saturating_sub(overlap);
                    if start >= end {
                        break;
                    }
                }

                Ok(slices)
            }
            "sentence" => {
                // 按句子分片（简单实现：以句号、问号、感叹号分隔）
                Ok(content
                    .split(|c| c == '。' || c == '.' || c == '?' || c == '!' || c == '？' || c == '！')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect())
            }
            _ => {
                // 默认：整个文件作为一个分片
                Ok(vec![content.to_string()])
            }
        }
    }

    fn build_full_content_and_segments(&self, content_list: &[ContentItem]) -> (String, Vec<Segment>) {
        let mut full_content = String::new();
        let mut segments = Vec::new();
        let mut current_len = 0usize;

        for pdf_content in content_list {
            if pdf_content.typ == "discarded" {
                continue;
            }
            let mut item_content = String::new();
            if pdf_content.typ == "text" {
                if let Some(text) = &pdf_content.text {
                    if let Some(lv) = pdf_content.text_level {
                        item_content.push_str("#".repeat(lv as usize).as_str());
                        item_content.push_str(" ");
                    }
                    item_content.push_str(text);
                }
            } else if pdf_content.typ == "image" {
                if let Some(img_name) = &pdf_content.img_path {
                    item_content.push_str(&format!("![{}](/api/v1/knowledge/files/{})", img_name, img_name));
                }
                if let Some(captions) = &pdf_content.image_caption {
                    for caption in captions {
                        item_content.push_str(&format!("{}", caption));
                    }
                }
            } else if pdf_content.typ == "table" {
                if let Some(table_caption) = &pdf_content.table_caption {
                    for caption in table_caption {
                        item_content.push_str(&format!("{}", caption));
                    }
                }
                if let Some(table_body) = &pdf_content.table_body {
                    item_content.push_str(table_body);
                }
            }

            if item_content.is_empty() {
                continue;
            }

            let start = current_len;
            full_content.push_str(&item_content);
            current_len += item_content.chars().count();
            let end = current_len;

            let positions = Self::positions_from_item(pdf_content);
            if !positions.is_empty() {
                segments.push(Segment { start, end, positions });
            }

            full_content.push_str("\n\n");
            current_len += 2;
        }

        (full_content, segments)
    }

    fn fixed_slice_content_with_positions(
        &self, content: &str, segments: &[Segment],
    ) -> anyhow::Result<Vec<SliceWithPositions>> {
        let cfg = config::get();
        let chunk_size = cfg.slice.smart_slice_max_chars;
        let overlap = cfg.slice.fixed_slice_overlap_chars;

        let chars: Vec<char> = content.chars().collect();
        let mut slices = Vec::new();
        let mut start = 0;

        while start < chars.len() {
            let end = std::cmp::min(start + chunk_size, chars.len());
            let slice: String = chars[start..end].iter().collect();
            let positions = Self::positions_for_range(segments, start, end);
            slices.push(SliceWithPositions { content: slice, positions });

            if end >= chars.len() {
                break;
            }

            start = end.saturating_sub(overlap);
            if start >= end {
                break;
            }
        }

        Ok(slices)
    }

    fn smart_slice_content_with_positions(
        &self, content_list: &[ContentItem],
    ) -> anyhow::Result<Vec<SliceWithPositions>> {
        let cfg = config::get();
        let max_chars = cfg.slice.smart_slice_max_chars;

        let mut slices = Vec::new();
        let mut current_slice = String::new();
        let mut current_segments: Vec<Segment> = Vec::new();
        let mut current_len = 0usize;
        let mut current_header = String::new(); // 当前所在的标题
        let mut current_header_positions: Vec<SlicePosition> = Vec::new();

        for item in content_list {
            if item.typ == "discarded" {
                continue;
            }

            let mut item_content = String::new();
            let item_positions = Self::positions_from_item(item);

            // 处理文本内容
            if item.typ == "text" {
                if let Some(text) = &item.text {
                    // 如果是标题，更新当前标题
                    if let Some(level) = item.text_level {
                        // 这是一个标题
                        let header_text = format!("{} {}", "#".repeat(level as usize), text);

                        // 如果当前有累积的内容，先保存
                        if !current_slice.trim().is_empty() {
                            self.flush_slice_with_positions(
                                &mut slices,
                                &mut current_slice,
                                &mut current_segments,
                                max_chars,
                            );
                            current_len = 0;
                        }

                        // 更新当前标题
                        current_header = header_text;
                        current_header_positions = item_positions.clone();
                        // 开始新的切片，包含标题
                        current_slice.clear();
                        current_segments.clear();
                        Self::append_segment(
                            &mut current_slice,
                            &mut current_len,
                            &mut current_segments,
                            &current_header,
                            current_header_positions.clone(),
                        );
                        Self::append_separator(&mut current_slice, &mut current_len);
                        continue;
                    } else {
                        // 普通文本
                        item_content.push_str(text);
                    }
                }
            } else if item.typ == "image" {
                if let Some(img_name) = &item.img_path {
                    item_content.push_str(&format!("![{}](/api/v1/knowledge/files/{})", img_name, img_name));
                }
                if let Some(captions) = &item.image_caption {
                    for caption in captions {
                        item_content.push_str(&format!("{}", caption));
                    }
                }
            } else if item.typ == "table" {
                if let Some(table_caption) = &item.table_caption {
                    for caption in table_caption {
                        item_content.push_str(&format!("{}", caption));
                    }
                }
                if let Some(table_body) = &item.table_body {
                    item_content.push_str(
                        &table_body
                            .replace(" colspan=\"1\"", "")
                            .replace(" rowspan=\"1\"", "")
                            .replace(" colspan='1'", "")
                            .replace(" rowspan='1'", "")
                            .replace(" colspan=1", "")
                            .replace(" rowspan=1", ""),
                    );
                }
            }

            if item_content.is_empty() {
                continue;
            }

            // 检查加入这个内容后是否超过字数限制
            let test_len = if current_slice.is_empty() {
                // 如果当前切片为空，但有标题，先加上标题
                if !current_header.is_empty() {
                    current_header.chars().count() + 2 + item_content.chars().count()
                } else {
                    item_content.chars().count()
                }
            } else {
                current_len + item_content.chars().count() + 2
            };

            if test_len > max_chars {
                // 超过限制，保存当前切片
                if !current_slice.trim().is_empty() {
                    self.flush_slice_with_positions(&mut slices, &mut current_slice, &mut current_segments, max_chars);
                    current_len = 0;
                }

                // 开始新的切片
                current_slice.clear();
                current_segments.clear();
                if !current_header.is_empty() {
                    Self::append_segment(
                        &mut current_slice,
                        &mut current_len,
                        &mut current_segments,
                        &current_header,
                        current_header_positions.clone(),
                    );
                    Self::append_separator(&mut current_slice, &mut current_len);
                }
                Self::append_segment(
                    &mut current_slice,
                    &mut current_len,
                    &mut current_segments,
                    &item_content,
                    item_positions.clone(),
                );
                Self::append_separator(&mut current_slice, &mut current_len);
            } else {
                // 没有超过限制，继续累积
                if current_slice.is_empty() {
                    current_slice.clear();
                    current_segments.clear();
                    current_len = 0;
                    if !current_header.is_empty() {
                        Self::append_segment(
                            &mut current_slice,
                            &mut current_len,
                            &mut current_segments,
                            &current_header,
                            current_header_positions.clone(),
                        );
                        Self::append_separator(&mut current_slice, &mut current_len);
                    }
                    Self::append_segment(
                        &mut current_slice,
                        &mut current_len,
                        &mut current_segments,
                        &item_content,
                        item_positions.clone(),
                    );
                } else {
                    Self::append_segment(
                        &mut current_slice,
                        &mut current_len,
                        &mut current_segments,
                        &item_content,
                        item_positions.clone(),
                    );
                    Self::append_separator(&mut current_slice, &mut current_len);
                }
            }
        }

        // 保存最后的切片
        if !current_slice.trim().is_empty() {
            self.flush_slice_with_positions(&mut slices, &mut current_slice, &mut current_segments, max_chars);
        }

        Ok(slices)
    }

    fn flush_slice_with_positions(
        &self, slices: &mut Vec<SliceWithPositions>, current_slice: &mut String, segments: &mut Vec<Segment>,
        max_chars: usize,
    ) {
        if current_slice.trim().is_empty() {
            return;
        }
        let content = std::mem::take(current_slice);
        let segment_data = std::mem::take(segments);
        let mut new_slices = self.split_slice_with_positions(&content, &segment_data, max_chars);
        slices.append(&mut new_slices);
    }

    fn split_slice_with_positions(
        &self, content: &str, segments: &[Segment], max_chars: usize,
    ) -> Vec<SliceWithPositions> {
        let char_count = content.chars().count();
        if char_count == 0 {
            return Vec::new();
        }

        let ranges = if char_count <= max_chars {
            vec![(0, char_count)]
        } else {
            let sentence_ranges = Self::sentence_ranges(content);
            if sentence_ranges.is_empty() {
                vec![(0, char_count)]
            } else {
                let mut ranges = Vec::new();
                let mut slice_start = sentence_ranges[0].0;
                let mut slice_end = sentence_ranges[0].1;
                for (start, end) in sentence_ranges.iter().skip(1) {
                    if end - slice_start > max_chars && slice_end > slice_start {
                        ranges.push((slice_start, slice_end));
                        slice_start = *start;
                    }
                    slice_end = *end;
                }
                ranges.push((slice_start, slice_end));
                ranges
            }
        };

        let chars: Vec<char> = content.chars().collect();
        ranges
            .into_iter()
            .filter_map(|(start, end)| {
                if start >= end || end > chars.len() {
                    return None;
                }
                let slice: String = chars[start..end].iter().collect();
                let positions = Self::positions_for_range(segments, start, end);
                Some(SliceWithPositions { content: slice, positions })
            })
            .collect()
    }

    fn sentence_ranges(content: &str) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();
        let mut start = 0usize;
        for (idx, ch) in content.chars().enumerate() {
            if matches!(ch, '。' | '.' | '?' | '!' | '？' | '！') {
                let end = idx + 1;
                if end > start {
                    ranges.push((start, end));
                }
                start = end;
            }
        }
        let total = content.chars().count();
        if start < total {
            ranges.push((start, total));
        }
        ranges
    }

    fn append_segment(
        current_slice: &mut String, current_len: &mut usize, segments: &mut Vec<Segment>, text: &str,
        positions: Vec<SlicePosition>,
    ) {
        if text.is_empty() {
            return;
        }
        let start = *current_len;
        current_slice.push_str(text);
        *current_len += text.chars().count();
        let end = *current_len;
        if !positions.is_empty() {
            segments.push(Segment { start, end, positions });
        }
    }

    fn append_separator(current_slice: &mut String, current_len: &mut usize) {
        current_slice.push_str("\n\n");
        *current_len += 2;
    }

    fn positions_from_item(item: &ContentItem) -> Vec<SlicePosition> {
        if item.bbox.len() == 4 {
            let bbox = [item.bbox[0], item.bbox[1], item.bbox[2], item.bbox[3]];
            vec![SlicePosition { page_idx: item.page_idx, bbox }]
        } else {
            Vec::new()
        }
    }

    fn positions_for_range(segments: &[Segment], start: usize, end: usize) -> Vec<SlicePosition> {
        let mut set: HashSet<SlicePosition> = HashSet::new();
        for segment in segments {
            if segment.end > start && segment.start < end {
                for position in &segment.positions {
                    set.insert(position.clone());
                }
            }
        }
        set.into_iter().collect()
    }

    async fn clone_file_data(&self, source: &File, target: &File) -> anyhow::Result<()> {
        if source.id == target.id {
            anyhow::bail!("Source and target file ids are identical");
        }
        if source.status != 1 {
            anyhow::bail!("Source file {} is not processed", source.id);
        }

        let reuse_log = format!("Reusing parsed data from file {}", source.id);
        sqlx::query("UPDATE files SET status = 2, log = ?, updated_at = strftime('%s','now') WHERE id = ?")
            .bind(&reuse_log)
            .bind(target.id)
            .execute(&self.pool)
            .await?;

        self.cleanup_processing_file_data(target.id).await?;

        let pdf_rows = self.fetch_pdf_content_rows(source.id).await?;
        let slice_rows = self.fetch_slice_rows(source.id).await?;
        let slice_ids: Vec<i64> = slice_rows.iter().map(|row| row.id).collect();
        let slice_positions = self.fetch_slice_position_rows(&slice_ids).await?;
        let (image_jobs, image_mapping) = self.prepare_image_jobs(&pdf_rows, source.id, target.id);

        let mut tx = self.pool.begin().await?;
        self.insert_pdf_rows(&mut tx, target.id, &pdf_rows, &image_mapping).await?;
        let cloned_slices = self.insert_slice_rows(&mut tx, target.id, &slice_rows).await?;
        self.insert_slice_positions(&mut tx, &cloned_slices, &slice_positions).await?;
        tx.commit().await?;

        self.copy_image_files(&image_jobs).await?;
        self.copy_converted_pdf(source.id, target.id).await?;

        let full_content = self.reindex_cloned_slices(target, &cloned_slices, source).await?;

        let final_log = format!("Reused parsed data from file {}", source.id);
        sqlx::query(
            "UPDATE files SET status = 1, content = ?, log = ?, updated_at = strftime('%s','now') WHERE id = ?",
        )
        .bind(&full_content)
        .bind(&final_log)
        .bind(target.id)
        .execute(&self.pool)
        .await?;

        let mut updated_file = target.clone();
        updated_file.content = Some(full_content.clone());
        if let Err(e) = self.build_knowledge_graph(&updated_file).await {
            error!("Failed to build knowledge graph for reused file {}: {}", target.id, e);
        }

        Ok(())
    }

    async fn fetch_pdf_content_rows(&self, file_id: i64) -> anyhow::Result<Vec<PdfContentRow>> {
        let sql = "SELECT page_idx, bbox, text, text_level, img_path, table_body FROM pdf_contents WHERE file_id = ? ORDER BY id";
        let rows = sqlx::query_as::<_, PdfContentRow>(sql).bind(file_id).fetch_all(&self.pool).await?;
        Ok(rows)
    }

    async fn fetch_slice_rows(&self, file_id: i64) -> anyhow::Result<Vec<SliceRow>> {
        let sql = "SELECT id, content FROM slices WHERE file_id = ? ORDER BY id";
        let rows = sqlx::query_as::<_, SliceRow>(sql).bind(file_id).fetch_all(&self.pool).await?;
        Ok(rows)
    }

    async fn fetch_slice_position_rows(&self, slice_ids: &[i64]) -> anyhow::Result<Vec<SlicePositionRecord>> {
        if slice_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut all_rows = Vec::new();
        let chunk_size = 400;
        for chunk in slice_ids.chunks(chunk_size) {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "SELECT slice_id, page_idx, x1, y1, x2, y2 FROM slice_positions WHERE slice_id IN (",
            );
            let mut separated = qb.separated(", ");
            for slice_id in chunk {
                separated.push_bind(slice_id);
            }
            qb.push(") ORDER BY slice_id, id");
            let rows = qb.build_query_as::<SlicePositionRecord>().fetch_all(&self.pool).await?;
            all_rows.extend(rows);
        }
        Ok(all_rows)
    }

    fn prepare_image_jobs(
        &self, rows: &[PdfContentRow], source_id: i64, target_id: i64,
    ) -> (Vec<(String, String)>, HashMap<String, String>) {
        let mut jobs = Vec::new();
        let mut mapping = HashMap::new();
        for row in rows {
            if let Some(path) = &row.img_path {
                if mapping.contains_key(path) {
                    continue;
                }
                let new_path = Self::remap_image_name(path, source_id, target_id);
                jobs.push((path.clone(), new_path.clone()));
                mapping.insert(path.clone(), new_path);
            }
        }
        (jobs, mapping)
    }

    async fn insert_pdf_rows(
        &self, tx: &mut sqlx::Transaction<'_, Sqlite>, target_file_id: i64, rows: &[PdfContentRow],
        image_mapping: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let binds_per_row = 7;
        let batch_size = std::cmp::max(1, 999 / binds_per_row);
        for chunk in rows.chunks(batch_size) {
            let mut qb = QueryBuilder::<Sqlite>::new(
                "INSERT INTO pdf_contents(file_id, page_idx, bbox, text, text_level, img_path, table_body) ",
            );
            qb.push_values(chunk.iter(), |mut b, row| {
                let new_img_path = row
                    .img_path
                    .as_ref()
                    .and_then(|path| image_mapping.get(path))
                    .cloned()
                    .or_else(|| row.img_path.clone());
                b.push_bind(target_file_id)
                    .push_bind(row.page_idx)
                    .push_bind(row.bbox.clone())
                    .push_bind(row.text.clone())
                    .push_bind(row.text_level)
                    .push_bind(new_img_path)
                    .push_bind(row.table_body.clone());
            });
            qb.build().execute(&mut **tx).await?;
        }
        Ok(())
    }

    async fn insert_slice_rows(
        &self, tx: &mut sqlx::Transaction<'_, Sqlite>, target_file_id: i64, rows: &[SliceRow],
    ) -> anyhow::Result<Vec<ClonedSlice>> {
        let mut cloned = Vec::new();
        for row in rows {
            let new_id = sqlx::query("INSERT INTO slices (file_id, content) VALUES (?, ?)")
                .bind(target_file_id)
                .bind(&row.content)
                .execute(&mut **tx)
                .await?
                .last_insert_rowid();
            cloned.push(ClonedSlice { old_id: row.id, new_id, content: row.content.clone() });
        }
        Ok(cloned)
    }

    async fn insert_slice_positions(
        &self, tx: &mut sqlx::Transaction<'_, Sqlite>, cloned_slices: &[ClonedSlice], positions: &[SlicePositionRecord],
    ) -> anyhow::Result<()> {
        if positions.is_empty() || cloned_slices.is_empty() {
            return Ok(());
        }
        let id_map: HashMap<i64, i64> = cloned_slices.iter().map(|slice| (slice.old_id, slice.new_id)).collect();
        let filtered: Vec<&SlicePositionRecord> =
            positions.iter().filter(|row| id_map.contains_key(&row.slice_id)).collect();
        if filtered.is_empty() {
            return Ok(());
        }
        let binds_per_row = 6;
        let batch_size = std::cmp::max(1, 999 / binds_per_row);
        for chunk in filtered.chunks(batch_size) {
            let mut qb =
                QueryBuilder::<Sqlite>::new("INSERT INTO slice_positions(slice_id, page_idx, x1, y1, x2, y2) ");
            qb.push_values(chunk.iter(), |mut b, row| {
                let new_id = id_map[&row.slice_id];
                b.push_bind(new_id)
                    .push_bind(row.page_idx)
                    .push_bind(row.x1)
                    .push_bind(row.y1)
                    .push_bind(row.x2)
                    .push_bind(row.y2);
            });
            qb.build().execute(&mut **tx).await?;
        }
        Ok(())
    }

    async fn copy_image_files(&self, jobs: &[(String, String)]) -> anyhow::Result<()> {
        for (old_path, new_path) in jobs {
            let Some(src_abs) = resolve_image_storage_path(old_path) else {
                warn!("Unable to resolve source image path {}, skipping reuse copy", old_path);
                continue;
            };
            let Some(dst_abs) = resolve_image_storage_path(new_path) else {
                warn!("Unable to resolve destination image path {}, skipping reuse copy", new_path);
                continue;
            };
            if let Some(parent) = std::path::Path::new(&dst_abs).parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::copy(&src_abs, &dst_abs).await?;
        }
        Ok(())
    }

    async fn copy_converted_pdf(&self, source_id: i64, target_id: i64) -> anyhow::Result<()> {
        let cfg = config::get();
        let pdf_dir = std::path::Path::new(&cfg.storage.pdf_path);
        let src_pdf = pdf_dir.join(format!("{}.pdf", source_id));
        match fs::try_exists(&src_pdf).await {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(err) => {
                warn!("Failed to check converted PDF existence for file {}: {}, skipping copy", source_id, err);
                return Ok(());
            }
        }
        fs::create_dir_all(pdf_dir).await?;
        let dst_pdf = pdf_dir.join(format!("{}.pdf", target_id));
        fs::copy(&src_pdf, &dst_pdf).await?;
        Ok(())
    }

    async fn reindex_cloned_slices(
        &self, target: &File, cloned_slices: &[ClonedSlice], source: &File,
    ) -> anyhow::Result<String> {
        let mut search_docs = Vec::new();
        for slice in cloned_slices {
            search_docs.push(tantivy_engine::Document::new(
                slice.new_id,
                target.id,
                target.kb_id,
                slice.content.clone(),
            ));
        }

        if !search_docs.is_empty() {
            let filename_lower = target.filename.to_lowercase();
            let is_image = Self::is_image_file(&filename_lower);
            let embeddings = if is_image {
                let embedding =
                    search::embedding::get_image_embedding_from_path(&target.path, Some(&target.filename)).await?;
                (0..search_docs.len()).map(|_| Some(embedding.clone())).collect()
            } else {
                vec![None; search_docs.len()]
            };
            self.search_engine.write_batch(search_docs, embeddings).await?;
        }

        let full_content = if let Some(content) = source.content.clone() {
            content
        } else if cloned_slices.is_empty() {
            String::new()
        } else {
            cloned_slices
                .iter()
                .map(|slice| slice.content.as_str())
                .filter(|content| !content.trim().is_empty())
                .map(|content| content.to_string())
                .collect::<Vec<_>>()
                .join("\n\n")
        };

        let index_full_content = if full_content.is_empty() {
            target.filename.clone()
        } else {
            format!("{}\n\n{}", target.filename, full_content)
        };
        self.search_engine
            .write_full(tantivy_engine::Document::new(target.id, target.id, target.kb_id, index_full_content))
            .await?;

        Ok(full_content)
    }

    fn remap_image_name(original: &str, source_id: i64, target_id: i64) -> String {
        let path = std::path::Path::new(original);
        let filename = path.file_name().and_then(|name| name.to_str()).unwrap_or(original);
        let prefix = format!("f{}_", source_id);
        let replacement = format!("f{}_", target_id);
        let stripped = if filename.starts_with(&prefix) {
            filename.trim_start_matches(&prefix).to_string()
        } else {
            filename.to_string()
        };
        let new_name = format!("{}{}", replacement, stripped);
        if let Some(parent) = path.parent() { parent.join(new_name).to_string_lossy().to_string() } else { new_name }
    }

    /// 构建知识图谱（完全由LLM生成）
    async fn build_knowledge_graph(&self, file: &File) -> anyhow::Result<()> {
        info!("Building knowledge graph for file {}", file.id);

        // 1. 初始化LLM图谱提取器
        let llm_extractor = LLMGraphExtractor::from_env();

        if !llm_extractor.is_enabled() {
            warn!("LLM not enabled, skipping knowledge graph building for file {}", file.id);
            return Ok(());
        }

        info!("Using LLM to generate knowledge graph for file {}", file.id);

        // 2. 获取文件的所有切片
        let slices_sql = "SELECT id, content FROM slices WHERE file_id = ? ORDER BY id";
        let slices: Vec<(i64, String)> = sqlx::query_as(slices_sql).bind(file.id).fetch_all(&self.pool).await?;

        if slices.is_empty() {
            debug!("No slices found for file {}, skipping graph building", file.id);
            return Ok(());
        }

        // 3. 合并所有切片内容（限制长度避免超出LLM上下文）
        let mut combined_content = String::new();
        let max_content_length = 8000; // 限制总长度

        for (_, content) in &slices {
            if combined_content.len() + content.len() > max_content_length {
                break;
            }
            combined_content.push_str(content);
            combined_content.push_str("\n\n");
        }

        if combined_content.trim().is_empty() {
            debug!("No content to process for file {}", file.id);
            return Ok(());
        }

        // 4. 调用LLM提取知识图谱
        let context = format!("文件名: {}", file.filename);

        let (mut entities, mut relations) =
            match llm_extractor.extract_knowledge_graph(&combined_content, &context).await {
                Ok(result) => result,
                Err(e) => {
                    error!("LLM knowledge graph extraction failed for file {}: {}", file.id, e);
                    return Err(e);
                }
            };

        info!("LLM extracted {} entities and {} relations from file {}", entities.len(), relations.len(), file.id);

        // 5. 为实体和关系添加文件信息
        for entity in &mut entities {
            entity.file_id = Some(file.id);
            entity.kb_id = file.kb_id;
        }

        for relation in &mut relations {
            relation.file_id = Some(file.id);
        }

        // 6. 更新知识图谱
        let mut graph = KnowledgeGraph::load_from_db(self.pool.clone(), file.kb_id).await?;

        // 添加实体和关系
        graph.incremental_update(entities, relations).await?;

        // 7. 保存图快照
        graph.save_snapshot().await?;

        info!("Knowledge graph updated successfully for file {} (LLM-generated)", file.id);

        Ok(())
    }
}

pub async fn process_file_immediate(pool: SqlitePool, search_engine: SearchEngine, file_id: i64) -> anyhow::Result<()> {
    let sql = "SELECT * FROM files WHERE id = ?";
    let file: File = sqlx::query_as(sql).bind(file_id).fetch_one(&pool).await?;

    let processor = FileProcessor::new(pool, search_engine, 0);
    processor.process_file(&file).await
}

pub async fn try_reuse_file_with_file(
    pool: SqlitePool, search_engine: SearchEngine, file: File,
) -> anyhow::Result<bool> {
    let processor = FileProcessor::new(pool, search_engine, 0);
    processor.try_reuse_existing_data(&file).await
}
