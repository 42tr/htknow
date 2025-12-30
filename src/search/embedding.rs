use anyhow::Result;
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};

static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| Client::new());

// 从环境变量读取，或使用默认值
const EMBEDDING_URL: &str = "http://192.168.0.46:9700/v1/embeddings";
const EMBEDDING_MODEL: &str = "bge-m3";

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
    let request = EmbeddingRequest { model: EMBEDDING_MODEL.to_string(), input: vec![text.to_string()] };

    let response = HTTP_CLIENT.post(EMBEDDING_URL).json(&request).send().await?;

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

    let request = EmbeddingRequest { model: EMBEDDING_MODEL.to_string(), input: texts.to_vec() };

    let response = HTTP_CLIENT.post(EMBEDDING_URL).json(&request).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Embedding API error: {} - {}", status, error_text);
    }

    let embedding_response: EmbeddingResponse = response.json().await?;

    Ok(embedding_response.data.into_iter().map(|data| data.embedding).collect())
}
