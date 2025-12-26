use std::time::Duration;

use log::{error, info};
use sqlx::SqlitePool;
use tokio::time;

use crate::api::File;

/// 文件处理器：定时从数据库读取未处理的文件并处理
pub struct FileProcessor {
    pool: SqlitePool,
    interval: Duration,
}

impl FileProcessor {
    /// 创建新的文件处理器
    ///
    /// # Arguments
    /// * `pool` - 数据库连接池
    /// * `interval_secs` - 处理间隔（秒）
    pub fn new(pool: SqlitePool, interval_secs: u64) -> Self {
        Self {
            pool,
            interval: Duration::from_secs(interval_secs),
        }
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
                                info!("No more pending files, waiting for next check");
                                break;
                            }
                            // 有更多文件，继续处理
                            info!("More files pending, continuing processing");
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
        let files = sqlx::query_as::<_, File>(sql)
            .fetch_all(&self.pool)
            .await?;
        Ok(files)
    }

    /// 处理单个文件
    async fn process_file(&self, file: &File) -> anyhow::Result<()> {
        info!("Processing file: {} ({})", file.filename, file.id);

        // TODO: 根据 slice_type 进行不同的处理逻辑
        // 这里是处理逻辑的占位符，你需要根据实际需求实现

        // 示例：读取文件内容
        let content = tokio::fs::read_to_string(file.path.as_str()).await?;

        // 示例：根据 slice_type 进行分片处理
        let slices = self.slice_content(&content, &file.slice_type)?;
        let slice_count = slices.len();

        // 保存分片到数据库
        for slice in slices {
            let sql = "INSERT INTO slices (file_id, content) VALUES (?, ?)";
            sqlx::query(sql)
                .bind(file.id)
                .bind(slice)
                .execute(&self.pool)
                .await?;
        }

        // 更新文件状态为已处理，并保存内容
        let sql = "UPDATE files SET status = 1, content = ?, log = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?";
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
        let sql = "UPDATE files SET status = -1, log = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?";
        sqlx::query(sql)
            .bind(error_msg)
            .bind(file_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// 根据 slice_type 对内容进行分片
    fn slice_content(&self, content: &str, slice_type: &str) -> anyhow::Result<Vec<String>> {
        match slice_type {
            "paragraph" => {
                // 按段落分片（以双换行符分隔）
                Ok(content
                    .split("\n\n")
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect())
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
