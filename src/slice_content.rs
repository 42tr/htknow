use std::{collections::HashMap, path::PathBuf};

use anyhow::Context;
use once_cell::sync::Lazy;
use tokio::sync::Mutex;

use crate::config;

/// 按 `file_id` 分片的写锁。
///
/// `upsert_many` 是「读整个 JSON → 合并 → 原子替换」的读-改-写：同一文件的两批切片并发写入时，
/// 后完成的一方会用自己读到的旧快照覆盖掉对方刚写的内容，切片正文就此丢失。
/// 用固定分片而不是按 file_id 建锁表，锁数量不随文件数增长。
const WRITE_SHARDS: usize = 64;

static WRITE_LOCKS: Lazy<Vec<Mutex<()>>> =
    Lazy::new(|| (0..WRITE_SHARDS).map(|_| Mutex::new(())).collect());

fn shard(file_id: i64) -> usize {
    (file_id.unsigned_abs() as usize) % WRITE_SHARDS
}

fn content_path(file_id: i64) -> PathBuf {
    PathBuf::from(&config::get().storage.slice_contents_path).join(format!("{}.json", file_id))
}

/// 一个源文件的全部切片正文。正文不再放入 SQLite，以减少 WAL 和主库体积。
pub async fn read_all(file_id: i64) -> anyhow::Result<HashMap<i64, String>> {
    let path = content_path(file_id);
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("invalid slice content file {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(e).with_context(|| format!("failed to read slice content file {}", path.display())),
    }
}

pub async fn upsert_many(file_id: i64, rows: &[(i64, String)]) -> anyhow::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let _guard = WRITE_LOCKS[shard(file_id)].lock().await;
    let mut contents = read_all(file_id).await?;
    for (id, content) in rows {
        contents.insert(*id, content.clone());
    }
    write_all_locked(file_id, &contents).await
}

pub async fn write_all(file_id: i64, contents: &HashMap<i64, String>) -> anyhow::Result<()> {
    let _guard = WRITE_LOCKS[shard(file_id)].lock().await;
    write_all_locked(file_id, contents).await
}

/// 调用方必须已持有该 `file_id` 的分片锁。
async fn write_all_locked(file_id: i64, contents: &HashMap<i64, String>) -> anyhow::Result<()> {
    if contents.is_empty() {
        return delete_locked(file_id).await;
    }
    let path = content_path(file_id);
    let parent = path.parent().expect("slice content path has parent");
    tokio::fs::create_dir_all(parent).await?;
    let bytes = serde_json::to_vec(contents)?;
    // 临时名必须唯一：共用固定的 `.json.tmp` 时，同一文件的两次写入会互相覆盖临时文件，
    // 先 rename 的一方还会把文件搬走，让后一次 rename 直接 ENOENT。
    let tmp = path.with_extension(format!("{}.json.tmp", uuid::Uuid::new_v4().simple()));
    let result = async {
        tokio::fs::write(&tmp, bytes).await.with_context(|| format!("failed to write {}", tmp.display()))?;
        tokio::fs::rename(&tmp, &path).await.with_context(|| format!("failed to rename {}", path.display()))
    }
    .await;
    if result.is_err() {
        // 失败时清掉半成品，避免残留文件在存储目录里越堆越多。
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result
}

pub async fn delete(file_id: i64) -> anyhow::Result<()> {
    let _guard = WRITE_LOCKS[shard(file_id)].lock().await;
    delete_locked(file_id).await
}

/// 调用方必须已持有该 `file_id` 的分片锁。
async fn delete_locked(file_id: i64) -> anyhow::Result<()> {
    let path = content_path(file_id);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("failed to delete slice content file {}", path.display())),
    }
}
