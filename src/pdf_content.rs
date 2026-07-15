use std::path::PathBuf;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::config;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PdfContent {
    pub page_idx: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_level: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub img_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_body: Option<String>,
}

pub fn path(file_id: i64) -> PathBuf {
    PathBuf::from(&config::get().storage.contents_path).join(format!("{}.json", file_id))
}

pub async fn read(file_id: i64) -> anyhow::Result<Vec<PdfContent>> {
    let path = path(file_id);
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("failed to read PDF content file {}", path.display())),
    };
    serde_json::from_slice(&bytes).with_context(|| format!("failed to parse PDF content file {}", path.display()))
}

pub async fn write(file_id: i64, contents: &[PdfContent]) -> anyhow::Result<()> {
    if contents.is_empty() {
        return delete(file_id).await;
    }

    let path = path(file_id);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create contents directory {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec(contents)?;
    let tmp_path = path.with_extension("json.tmp");
    tokio::fs::write(&tmp_path, bytes)
        .await
        .with_context(|| format!("failed to write tmp PDF content file {}", tmp_path.display()))?;
    tokio::fs::rename(&tmp_path, &path)
        .await
        .with_context(|| format!("failed to rename PDF content file to {}", path.display()))?;
    Ok(())
}

pub async fn delete(file_id: i64) -> anyhow::Result<()> {
    let path = path(file_id);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("failed to delete PDF content file {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip() {
        let rows = vec![PdfContent {
            page_idx: 1,
            bbox: Some("[1,2,3,4]".to_string()),
            text: Some("text".to_string()),
            text_level: Some(2),
            img_path: None,
            table_body: None,
        }];
        let json = serde_json::to_string(&rows).unwrap();
        assert_eq!(serde_json::from_str::<Vec<PdfContent>>(&json).unwrap(), rows);
    }
}
