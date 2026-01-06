use anyhow::Result;
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};

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
