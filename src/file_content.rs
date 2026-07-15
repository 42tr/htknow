use std::path::PathBuf;

use anyhow::Context;

use crate::config;

/// 返回指定 file_id 对应的 content 文件路径。
fn content_path(file_id: i64) -> PathBuf {
    PathBuf::from(&config::get().storage.contents_path).join(format!("{}.txt", file_id))
}

/// 读取文件的完整 content。
///
/// - 文件存在：返回 `Some(内容)`。
/// - 文件不存在：返回 `None`（对应数据库中 `content IS NULL` 的语义）。
/// - 其他 IO 错误：返回 `Err`。
pub async fn read(file_id: i64) -> anyhow::Result<Option<String>> {
    let path = content_path(file_id);
    match tokio::fs::read_to_string(&path).await {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("failed to read content file {}", path.display())),
    }
}

/// 写入文件的完整 content。
///
/// 使用临时文件 + rename 保证原子性，避免崩溃后出现半写文件。
pub async fn write(file_id: i64, content: &str) -> anyhow::Result<()> {
    if content.is_empty() {
        // 空内容与 NULL 语义统一：删除文件，避免留下空文件。
        return delete(file_id).await;
    }

    let path = content_path(file_id);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create contents directory {}", parent.display()))?;
    }

    let tmp_path = path.with_extension("txt.tmp");
    tokio::fs::write(&tmp_path, content.as_bytes())
        .await
        .with_context(|| format!("failed to write tmp content file {}", tmp_path.display()))?;
    tokio::fs::rename(&tmp_path, &path)
        .await
        .with_context(|| format!("failed to rename content file to {}", path.display()))?;
    Ok(())
}

/// 删除文件的 content。
///
/// 文件不存在时视为已成功删除。
pub async fn delete(file_id: i64) -> anyhow::Result<()> {
    let path = content_path(file_id);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("failed to delete content file {}", path.display())),
    }
}
