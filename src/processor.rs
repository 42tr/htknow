use std::{collections::HashMap, time::Duration};

use base64::{Engine, engine::general_purpose::STANDARD};
use log::{debug, error, info};
use reqwest::multipart;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use tokio::{fs, time};

use crate::{
    api::File, search::{self, tantivy_engine}
};

/// MinerU API 返回的结果结构
#[derive(Debug, Deserialize, Serialize)]
struct MinerUResponse {
    #[serde(default)]
    content_list: Vec<ContentItem>,
    #[serde(default)]
    images: HashMap<String, String>,
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
    text_format: Option<i32>, // latex
    #[serde(default)]
    img_path: Option<String>,
    #[serde(default)]
    image_caption: Option<Vec<String>>,
    #[serde(default)]
    table_body: Option<String>,
    #[serde(default)]
    table_caption: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ImageInfo {
    path: String,
    #[serde(default)]
    page_num: Option<i32>,
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

        // 检查文件是否为 PDF
        let is_pdf = file.filename.to_lowercase().ends_with(".pdf");

        if is_pdf {
            // 处理 PDF 文件
            self.process_pdf_file(file).await?;
        } else {
            // 处理普通文本文件
            self.process_text_file(file).await?;
        }

        Ok(())
    }

    /// 处理 PDF 文件，调用 MinerU API
    async fn process_pdf_file(&self, file: &File) -> anyhow::Result<()> {
        info!("Processing PDF file with MinerU: {}", file.filename);

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
        let response = client.post("http://192.168.0.46:10001/file_parse").multipart(form).send().await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!("MinerU API failed: {}", error_text));
        }

        let result: MinerUResponse = response.json().await?;

        // 提取文本内容
        let mut pdf_sql = QueryBuilder::<Sqlite>::new(
            "insert into pdf_contents(file_id, page_idx, text, text_level, img_path, table_body) values",
        );
        pdf_sql.push_values(result.content_list.iter().filter(|item| item.typ != "discarded"), |mut b, item| {
            b.push_bind(file.id)
                .push_bind(item.page_idx)
                .push_bind(&item.text)
                .push_bind(item.text_level)
                .push_bind(&item.img_path)
                .push_bind(&item.table_body);
        });
        pdf_sql.build().execute(&self.pool).await?;

        // 保存图片到本地
        fs::create_dir_all("images").await?;
        for (img_name, img_base64) in result.images {
            // 保存图片
            let b64 = img_base64
                .strip_prefix("data:image/jpeg;base64,")
                .ok_or_else(|| anyhow::anyhow!("invalid base64 image"))?;
            let bytes = STANDARD.decode(b64)?;
            fs::write(format!("images/{}", img_name), bytes).await?;
        }

        // 根据 slice_type 分片并保存
        let mut full_content = String::new();
        for pdf_content in result.content_list {
            if pdf_content.typ == "discarded" {
                continue;
            }
            if pdf_content.typ == "text" {
                if let Some(text) = pdf_content.text {
                    if let Some(lv) = pdf_content.text_level {
                        full_content.push_str("#".repeat(lv as usize).as_str());
                        full_content.push_str(" ");
                    }
                    full_content.push_str(&text);
                }
            } else if pdf_content.typ == "image" {
                if let Some(img_name) = pdf_content.img_path {
                    full_content.push_str(&format!("![{}](images/{})", img_name, img_name,));
                }
                if let Some(captions) = pdf_content.image_caption {
                    for caption in captions {
                        full_content.push_str(&format!("{}", caption));
                    }
                }
            } else if pdf_content.typ == "table" {
                if let Some(table_caption) = pdf_content.table_caption {
                    for caption in table_caption {
                        full_content.push_str(&format!("{}", caption));
                    }
                }
                if let Some(table_body) = pdf_content.table_body {
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
}
