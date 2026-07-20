use std::{collections::HashMap, path::PathBuf};

use anyhow::Context;

use crate::config;

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
    let mut contents = read_all(file_id).await?;
    for (id, content) in rows {
        contents.insert(*id, content.clone());
    }
    write_all(file_id, &contents).await
}

pub async fn write_all(file_id: i64, contents: &HashMap<i64, String>) -> anyhow::Result<()> {
    let path = content_path(file_id);
    if contents.is_empty() {
        return delete(file_id).await;
    }
    let parent = path.parent().expect("slice content path has parent");
    tokio::fs::create_dir_all(parent).await?;
    let bytes = serde_json::to_vec(contents)?;
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, bytes).await.with_context(|| format!("failed to write {}", tmp.display()))?;
    tokio::fs::rename(&tmp, &path).await.with_context(|| format!("failed to rename {}", path.display()))
}

pub async fn delete(file_id: i64) -> anyhow::Result<()> {
    let path = content_path(file_id);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("failed to delete slice content file {}", path.display())),
    }
}
