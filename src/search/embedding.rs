use std::time::Duration;

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::config;

static HTTP_CLIENT: Lazy<Client> = Lazy::new(Client::new);

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
    let file_name = std::path::Path::new(path).file_name().and_then(|name| name.to_str()).unwrap_or("image");
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cfg = config::get();

    let part = reqwest::multipart::Part::file(path)
        .await
        .with_context(|| format!("failed to open image file for embedding: {}", path))?
        .file_name(file_name.to_string())
        .mime_str(mime.essence_str())?;
    let form = reqwest::multipart::Form::new().part("file", part).text("text", text.unwrap_or(file_name).to_string());

    let response = HTTP_CLIENT
        .post(&cfg.services.image_embedding_url)
        .timeout(Duration::from_secs(cfg.search.embedding_timeout_secs))
        .multipart(form)
        .send()
        .await
        .with_context(|| {
            format!(
                "image embedding request failed: url={}, file={}, timeout={}s",
                cfg.services.image_embedding_url, file_name, cfg.search.embedding_timeout_secs
            )
        })?;

    handle_image_embedding_response(response).await
}

/// 获取图片的 embedding 向量（从文件内容）
pub async fn get_image_embedding_from_bytes(
    file_name: &str, content_type: Option<&str>, bytes: Vec<u8>, text: Option<&str>,
) -> Result<Vec<f32>> {
    let cfg = config::get();
    let mut part = reqwest::multipart::Part::bytes(bytes).file_name(file_name.to_string());
    if let Some(content_type) = content_type {
        part = part.mime_str(content_type)?;
    }
    let form = reqwest::multipart::Form::new().part("file", part).text("text", text.unwrap_or(file_name).to_string());

    let response = HTTP_CLIENT
        .post(&cfg.services.image_embedding_url)
        .timeout(Duration::from_secs(cfg.search.embedding_timeout_secs))
        .multipart(form)
        .send()
        .await
        .with_context(|| {
            format!(
                "image embedding request failed: url={}, file={}, timeout={}s",
                cfg.services.image_embedding_url, file_name, cfg.search.embedding_timeout_secs
            )
        })?;

    handle_image_embedding_response(response).await
}

async fn handle_image_embedding_response(response: reqwest::Response) -> Result<Vec<f32>> {
    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Image embedding API error: {} - {}", status, error_text);
    }

    let embedding_response: EmbeddingResponse =
        response.json().await.context("image embedding response decode failed")?;

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
    let query = text.trim();
    if query.is_empty() {
        anyhow::bail!("Embedding query cannot be empty");
    }
    let request = EmbeddingRequest { model: cfg.ai.embedding_model.clone(), input: vec![query.to_string()] };

    let response = HTTP_CLIENT
        .post(&cfg.services.embedding_url)
        .timeout(Duration::from_secs(cfg.search.embedding_timeout_secs))
        .json(&request)
        .send()
        .await
        .with_context(|| {
            format!(
                "embedding request failed: url={}, input_chars={}, timeout={}s",
                cfg.services.embedding_url,
                text.chars().count(),
                cfg.search.embedding_timeout_secs
            )
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Embedding API error: {} - {}, input_chars={}", status, error_text, text.chars().count());
    }

    let embedding_response: EmbeddingResponse = response.json().await.context("embedding response decode failed")?;

    let embedding = embedding_response
        .data
        .into_iter()
        .next()
        .map(|data| data.embedding)
        .ok_or_else(|| anyhow::anyhow!("No embedding returned"))?;
    Ok(embedding)
}

/// 批量获取文本的 embedding 向量（自动分批）
pub async fn get_embeddings(texts: &[String]) -> Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let batch_size = config::get().ai.embedding_batch_size;
    let mut all_embeddings = Vec::with_capacity(texts.len());
    for chunk in texts.chunks(batch_size) {
        let batch = get_embeddings_single_batch(chunk).await?;
        all_embeddings.extend(batch);
    }
    Ok(all_embeddings)
}

async fn get_embeddings_single_batch(texts: &[String]) -> Result<Vec<Vec<f32>>> {
    let cfg = config::get();
    let request = EmbeddingRequest { model: cfg.ai.embedding_model.clone(), input: texts.to_vec() };

    let response = HTTP_CLIENT
        .post(&cfg.services.embedding_url)
        .timeout(Duration::from_secs(cfg.search.embedding_timeout_secs))
        .json(&request)
        .send()
        .await
        .with_context(|| {
            format!(
                "batch embedding request failed: url={}, batch_size={}, total_chars={}, timeout={}s",
                cfg.services.embedding_url,
                texts.len(),
                texts.iter().map(|t| t.chars().count()).sum::<usize>(),
                cfg.search.embedding_timeout_secs
            )
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "Embedding API error: {} - {}, batch_size={}, total_chars={}",
            status,
            error_text,
            texts.len(),
            texts.iter().map(|text| text.chars().count()).sum::<usize>()
        );
    }

    let embedding_response: EmbeddingResponse =
        response.json().await.context("batch embedding response decode failed")?;

    Ok(embedding_response.data.into_iter().map(|data| data.embedding).collect())
}
