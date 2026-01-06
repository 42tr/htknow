use std::{collections::HashMap, time::Duration};

use base64::{Engine, engine::general_purpose::STANDARD};
use log::{debug, error, info, warn};
use reqwest::multipart;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use tokio::{fs, time};

use crate::{
    api::File, config, graph::{graph_manager::KnowledgeGraph, llm_extractor::LLMGraphExtractor}, search::{self, tantivy_engine}
};

#[derive(Debug, Deserialize, Serialize)]
struct Result {
    #[serde(default)]
    content_list: String,
    #[serde(default)]
    images: HashMap<String, String>,
}

/// MinerU API 返回的结果结构
#[derive(Debug, Deserialize, Serialize)]
struct MinerUResponse {
    results: HashMap<String, Result>,
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
#[derive(Debug, Deserialize, Serialize)]
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
        tokio::spawn(async move {
            info!("File processor started with interval: {:?}", self.interval);

            loop {
                // 持续处理直到没有待处理的文件
                loop {
                    match self.process_pending_files().await {
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
                time::sleep(self.interval).await;
            }
        });
    }

    /// 处理所有待处理的文件
    /// 返回是否还有更多文件需要处理
    async fn process_pending_files(&self) -> anyhow::Result<bool> {
        let files = self.fetch_pending_files().await?;

        if files.is_empty() {
            return Ok(false);
        }

        info!("Found {} pending files to process", files.len());

        for file in files {
            if let Err(e) = self.process_file(&file).await {
                error!("Failed to process file {}: {}", file.id, e);
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

    /// 处理单个文件
    async fn process_file(&self, file: &File) -> anyhow::Result<()> {
        info!("Processing file: {} ({})", file.filename, file.id);
        let sql = "UPDATE files SET status = 2, updated_at = strftime('%s','now') WHERE id = ?";
        sqlx::query(sql).bind(file.id).execute(&self.pool).await?;

        // 检查文件是否为 PDF 或图片
        let filename_lower = file.filename.to_lowercase();
        let is_pdf_or_img = filename_lower.ends_with(".pdf")
            || filename_lower.ends_with(".jpg")
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
            || filename_lower.ends_with(".heif");

        // 检查文件是否为 Word 文档
        let is_word = filename_lower.ends_with(".doc") || filename_lower.ends_with(".docx");

        if is_word {
            // Word 文档：先转换为 PDF，再使用 process_pdf_file 处理
            info!("Detected Word document, converting to PDF: {}", file.filename);
            self.convert_word_to_pdf_and_process(file).await?;
        } else if is_pdf_or_img {
            // 处理 PDF 或图片文件
            self.process_pdf_file(file).await?;
        } else {
            // 处理普通文本文件
            self.process_text_file(file).await?;
        }

        Ok(())
    }

    /// 将 Word 文档转换为 PDF 并处理
    async fn convert_word_to_pdf_and_process(&self, file: &File) -> anyhow::Result<()> {
        // 创建临时目录用于存放转换后的 PDF
        let cfg = config::get();
        let temp_dir = std::path::Path::new(&cfg.storage.temp_path);
        fs::create_dir_all(temp_dir).await?;

        // 生成临时 PDF 文件路径
        let pdf_filename = format!("{}.pdf", file.id);

        // 使用 LibreOffice 将 Word 转换为 PDF
        // 注意：这需要系统中安装了 LibreOffice
        let output = tokio::process::Command::new("soffice")
            .args(&["--headless", "--convert-to", "pdf", "--outdir", temp_dir.to_str().unwrap(), &file.path])
            .output()
            .await?;

        if !output.status.success() {
            let error_msg = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("Failed to convert Word to PDF: {}", error_msg));
        }

        // 查找生成的 PDF 文件
        // LibreOffice 会使用原文件名（不含扩展名）+ .pdf
        let converted_pdf_path = temp_dir.join(format!("{}.pdf", file.path.split("/").last().unwrap()));

        // 检查转换后的 PDF 是否存在
        if !tokio::fs::try_exists(&converted_pdf_path).await? {
            return Err(anyhow::anyhow!("Converted PDF file not found: {:?}", converted_pdf_path));
        }

        // 创建临时 File 结构用于处理 PDF
        let mut temp_file = file.clone();
        temp_file.path = converted_pdf_path.to_string_lossy().to_string();
        temp_file.filename = pdf_filename;

        // 使用 process_pdf_file 处理转换后的 PDF
        let result = self.process_pdf_file(&temp_file).await;

        // 清理临时 PDF 文件
        let _ = fs::remove_file(&converted_pdf_path).await;

        result
    }

    /// 处理 PDF 文件，调用 MinerU API
    async fn process_pdf_file(&self, file: &File) -> anyhow::Result<()> {
        info!("Processing PDF file: {}", file.filename);

        // 先检查 pdf_contents 表中是否已有该文件的数据
        let check_sql = "SELECT COUNT(*) as count FROM pdf_contents WHERE file_id = ?";
        let count: (i64,) = sqlx::query_as(check_sql).bind(file.id).fetch_one(&self.pool).await?;

        let content_list: Vec<ContentItem>;

        if count.0 > 0 {
            // 已有数据，直接从数据库读取
            info!("Found existing PDF contents in database for file {}, skipping MinerU API call", file.id);

            let fetch_sql = "SELECT page_idx, text, text_level, img_path, table_body FROM pdf_contents WHERE file_id = ? ORDER BY page_idx, id";
            let rows: Vec<(i32, Option<String>, Option<i32>, Option<String>, Option<String>)> =
                sqlx::query_as(fetch_sql).bind(file.id).fetch_all(&self.pool).await?;

            // 将数据库记录转换为 ContentItem
            content_list = rows
                .iter()
                .map(|row| {
                    let typ = if row.1.is_some() {
                        "text".to_string()
                    } else if row.3.is_some() {
                        "image".to_string()
                    } else if row.4.is_some() {
                        "table".to_string()
                    } else {
                        "unknown".to_string()
                    };

                    ContentItem {
                        typ,
                        bbox: vec![],
                        page_idx: row.0,
                        text: row.1.clone(),
                        text_level: row.2,
                        text_format: None,
                        img_path: row.3.clone(),
                        image_caption: None,
                        table_body: row.4.clone(),
                        table_caption: None,
                    }
                })
                .collect();
        } else {
            // 没有数据，调用 MinerU API
            // 读取 PDF 文件
            let file_bytes = tokio::fs::read(&file.path).await?;

            // 构建 multipart form
            let form = multipart::Form::new()
                .text("return_middle_json", "false")
                .text("return_model_output", "false")
                .text("return_md", "false")
                .text("return_images", "true")
                .text("end_page_id", "99999")
                .text("parse_method", "auto")
                .text("start_page_id", "0")
                .text("lang_list", "ch")
                .text("output_dir", "./output")
                .text("server_url", "string")
                .text("return_content_list", "true")
                .text("backend", "pipeline")
                .text("table_enable", "true")
                .text("response_format_zip", "false")
                .text("formula_enable", "true")
                .part(
                    "files",
                    multipart::Part::bytes(file_bytes).file_name(file.filename.clone()).mime_str("application/pdf")?,
                );

            // 调用 MinerU API
            let client = reqwest::Client::new();
            let cfg = config::get();
            let response = client.post(&cfg.services.mineru_url).multipart(form).send().await?;

            if !response.status().is_success() {
                let error_text = response.text().await?;
                return Err(anyhow::anyhow!("MinerU API failed: {}", error_text));
            }

            let result: MinerUResponse = response.json().await?;

            let result = result.results.values().next().unwrap();
            content_list = serde_json::from_str(&result.content_list)?;

            // 提取文本内容并过滤掉 discarded 项
            let valid_content_items: Vec<_> = content_list.iter().filter(|item| item.typ != "discarded").collect();

            // 只有在有有效内容时才插入数据库
            if !valid_content_items.is_empty() {
                let mut pdf_sql = QueryBuilder::<Sqlite>::new(
                    "insert into pdf_contents(file_id, page_idx, text, text_level, img_path, table_body) ",
                );
                pdf_sql.push_values(valid_content_items.iter(), |mut b, item| {
                    b.push_bind(file.id)
                        .push_bind(item.page_idx)
                        .push_bind(&item.text)
                        .push_bind(item.text_level)
                        .push_bind(&item.img_path)
                        .push_bind(&item.table_body);
                });
                pdf_sql.build().execute(&self.pool).await?;
            }

            // 保存图片到本地
            let cfg = config::get();
            fs::create_dir_all(&cfg.storage.images_path).await?;
            for (img_name, img_base64) in &result.images {
                // 保存图片
                let b64 = img_base64
                    .strip_prefix("data:image/jpeg;base64,")
                    .ok_or_else(|| anyhow::anyhow!("invalid base64 image"))?;
                let bytes = STANDARD.decode(b64)?;
                fs::write(format!("{}/{}", cfg.storage.images_path, img_name), bytes).await?;
            }
        }

        // 根据 slice_type 分片并保存
        let mut full_content = String::new();
        for pdf_content in &content_list {
            if pdf_content.typ == "discarded" {
                continue;
            }
            if pdf_content.typ == "text" {
                if let Some(text) = &pdf_content.text {
                    if let Some(lv) = pdf_content.text_level {
                        full_content.push_str("#".repeat(lv as usize).as_str());
                        full_content.push_str(" ");
                    }
                    full_content.push_str(&text);
                }
            } else if pdf_content.typ == "image" {
                if let Some(img_name) = &pdf_content.img_path {
                    full_content.push_str(&format!("![{}](images/{})", img_name, img_name,));
                }
                if let Some(captions) = &pdf_content.image_caption {
                    for caption in captions {
                        full_content.push_str(&format!("{}", caption));
                    }
                }
            } else if pdf_content.typ == "table" {
                if let Some(table_caption) = &pdf_content.table_caption {
                    for caption in table_caption {
                        full_content.push_str(&format!("{}", caption));
                    }
                }
                if let Some(table_body) = &pdf_content.table_body {
                    full_content.push_str(&table_body);
                }
            }
            full_content.push_str("\n\n");
        }
        let slices = self.slice_content(&full_content, &file.slice_type)?;
        let slice_count = slices.len();

        for slice in slices {
            let sql = "INSERT INTO slices (file_id, content) VALUES (?, ?)";
            let id = sqlx::query(sql).bind(file.id).bind(&slice).execute(&self.pool).await?.last_insert_rowid();
            self.search_engine.write(tantivy_engine::Document::new(id, file.id, file.kb_id, slice)).await?;
        }

        // 构建知识图谱
        if let Err(e) = self.build_knowledge_graph(file).await {
            error!("Failed to build knowledge graph for file {}: {}", file.id, e);
            // 不影响主流程，仅记录错误
        }

        // 更新文件状态
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

    /// 处理普通文本文件
    async fn process_text_file(&self, file: &File) -> anyhow::Result<()> {
        // 示例：读取文件内容
        let content = tokio::fs::read_to_string(file.path.as_str()).await?;

        // 示例：根据 slice_type 进行分片处理
        let slices = self.slice_content(&content, &file.slice_type)?;
        let slice_count = slices.len();

        // 保存分片到数据库
        for slice in slices {
            let sql = "INSERT INTO slices (file_id, content) VALUES (?, ?)";
            let id = sqlx::query(sql).bind(file.id).bind(&slice).execute(&self.pool).await?.last_insert_rowid();
            self.search_engine.write(tantivy_engine::Document::new(id, file.id, file.kb_id, slice)).await?;
        }

        // 更新文件状态为已处理，并保存内容
        let sql = "UPDATE files SET status = 1, content = ?, log = ?, updated_at = strftime('%s','now') WHERE id = ?";
        sqlx::query(sql)
            .bind(&content)
            .bind("Processing completed successfully")
            .bind(file.id)
            .execute(&self.pool)
            .await?;

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

    /// 根据 slice_type 对内容进行分片
    fn slice_content(&self, content: &str, slice_type: &str) -> anyhow::Result<Vec<String>> {
        match slice_type {
            "paragraph" => {
                // 按段落分片（以双换行符分隔）
                Ok(content.split("\n\n").map(|s| s.trim()).filter(|s| !s.is_empty()).map(|s| s.to_string()).collect())
            }
            "fixed" => {
                // 固定长度分片（每1000字符）
                let chunk_size = 1000;
                Ok(content
                    .chars()
                    .collect::<Vec<_>>()
                    .chunks(chunk_size)
                    .map(|chunk| chunk.iter().collect::<String>())
                    .collect())
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
