use anyhow::Result;
use once_cell::sync::Lazy;
use reqwest::{Client, multipart};
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::config;

static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| Client::new());

#[derive(Debug, Serialize)]
struct EmbeddingRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

/// 获取图片的 embedding 向量（从文件路径）
pub async fn get_image_embedding_from_path(path: &str, text: Option<&str>) -> Result<Vec<f32>> {
    let bytes = fs::read(path).await?;
    let file_name = std::path::Path::new(path).file_name().and_then(|name| name.to_str()).unwrap_or("image");
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    get_image_embedding_from_bytes(file_name, Some(mime.essence_str()), bytes, text).await
}

/// 获取图片的 embedding 向量（从文件内容）
pub async fn get_image_embedding_from_bytes(
    file_name: &str, content_type: Option<&str>, bytes: Vec<u8>, text: Option<&str>,
) -> Result<Vec<f32>> {
    let cfg = config::get();
    let mut part = multipart::Part::bytes(bytes).file_name(file_name.to_string());
    if let Some(content_type) = content_type {
        part = part.mime_str(content_type)?;
    }
    let form = multipart::Form::new().part("file", part).text("text", text.unwrap_or(file_name).to_string());

    let response = HTTP_CLIENT.post(&cfg.services.image_embedding_url).multipart(form).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Image embedding API error: {} - {}", status, error_text);
    }

    let embedding_response: EmbeddingResponse = response.json().await?;

    embedding_response
        .data
        .into_iter()
        .next()
        .map(|data| data.embedding)
        .ok_or_else(|| anyhow::anyhow!("No image embedding returned"))
}

/// 获取文本的 embedding 向量
pub async fn get_embedding(text: &str) -> Result<Vec<f32>> {
    let cfg = config::get();
    let request = EmbeddingRequest { model: cfg.ai.embedding_model.clone(), input: vec![text.to_string()] };

    let response = HTTP_CLIENT.post(&cfg.services.embedding_url).json(&request).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Embedding API error: {} - {}", status, error_text);
    }

    let embedding_response: EmbeddingResponse = response.json().await?;

    embedding_response
        .data
        .into_iter()
        .next()
        .map(|data| data.embedding)
        .ok_or_else(|| anyhow::anyhow!("No embedding returned"))
}

/// 批量获取文本的 embedding 向量
pub async fn get_embeddings(texts: &[String]) -> Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let cfg = config::get();
    let request = EmbeddingRequest { model: cfg.ai.embedding_model.clone(), input: texts.to_vec() };

    let response = HTTP_CLIENT.post(&cfg.services.embedding_url).json(&request).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Embedding API error: {} - {}", status, error_text);
    }

    let embedding_response: EmbeddingResponse = response.json().await?;

    Ok(embedding_response.data.into_iter().map(|data| data.embedding).collect())
}
