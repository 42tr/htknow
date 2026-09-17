use std::time::Duration;

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::with_api_key;
use crate::config;

/// 判断图片 embedding 服务是否已配置。
pub fn image_embedding_enabled() -> bool {
    config::get().services.image_embedding_url.is_some()
}

static HTTP_CLIENT: Lazy<Client> = Lazy::new(Client::new);
// A longer inference timeout must not also turn an unreachable host into a long wait.
static BATCH_HTTP_CLIENT: Lazy<Client> =
    Lazy::new(|| Client::builder().connect_timeout(Duration::from_secs(5)).build().expect("embedding HTTP client"));

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
    #[serde(default)]
    index: Option<usize>,
    embedding: Vec<f32>,
}

/// 获取图片的 embedding 向量（从文件路径）
pub async fn get_image_embedding_from_path(path: &str, text: Option<&str>) -> Result<Vec<f32>> {
    let file_name = std::path::Path::new(path).file_name().and_then(|name| name.to_str()).unwrap_or("image");
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cfg = config::get();
    let url = cfg
        .services
        .image_embedding_url
        .clone()
        .ok_or_else(|| anyhow::anyhow!("image embedding URL is not configured"))?;

    let part = reqwest::multipart::Part::file(path)
        .await
        .with_context(|| format!("failed to open image file for embedding: {}", path))?
        .file_name(file_name.to_string())
        .mime_str(mime.essence_str())?;
    let form = reqwest::multipart::Form::new().part("file", part).text("text", text.unwrap_or(file_name).to_string());

    let response = with_api_key(HTTP_CLIENT.post(&url), cfg.services.image_embedding_key.as_deref())
        .timeout(Duration::from_secs(cfg.search.embedding_timeout_secs))
        .multipart(form)
        .send()
        .await
        .with_context(|| {
            format!(
                "image embedding request failed: url={}, file={}, timeout={}s",
                url, file_name, cfg.search.embedding_timeout_secs
            )
        })?;

    handle_image_embedding_response(response).await
}

/// 获取图片的 embedding 向量（从文件内容）
pub async fn get_image_embedding_from_bytes(
    file_name: &str, content_type: Option<&str>, bytes: Vec<u8>, text: Option<&str>,
) -> Result<Vec<f32>> {
    let cfg = config::get();
    let url = cfg
        .services
        .image_embedding_url
        .clone()
        .ok_or_else(|| anyhow::anyhow!("image embedding URL is not configured"))?;
    let mut part = reqwest::multipart::Part::bytes(bytes).file_name(file_name.to_string());
    if let Some(content_type) = content_type {
        part = part.mime_str(content_type)?;
    }
    let form = reqwest::multipart::Form::new().part("file", part).text("text", text.unwrap_or(file_name).to_string());

    let response = with_api_key(HTTP_CLIENT.post(&url), cfg.services.image_embedding_key.as_deref())
        .timeout(Duration::from_secs(cfg.search.embedding_timeout_secs))
        .multipart(form)
        .send()
        .await
        .with_context(|| {
            format!(
                "image embedding request failed: url={}, file={}, timeout={}s",
                url, file_name, cfg.search.embedding_timeout_secs
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

    let cfg = config::get();
    let embedding_url = Some(cfg.services.embedding_url.clone())
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("services.embedding_url is not configured"))?;
    let response = with_api_key(HTTP_CLIENT.post(&embedding_url), cfg.services.embedding_key.as_deref())
        .timeout(Duration::from_secs(cfg.search.embedding_timeout_secs))
        .json(&request)
        .send()
        .await
        .with_context(|| {
            format!(
                "embedding request failed: url={}, input_chars={}, timeout={}s",
                embedding_url,
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

/// 批量获取文本向量；同时限制条数和字符数，保持结果与原文一一对应。
pub async fn get_embeddings(texts: &[String]) -> Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let cfg = config::get();
    anyhow::ensure!(!cfg.services.embedding_url.trim().is_empty(), "services.embedding_url is not configured");
    let options = BatchOptions {
        url: &cfg.services.embedding_url,
        model: &cfg.ai.embedding_model,
        api_key: cfg.services.embedding_key.as_deref(),
        max_items: cfg.ai.embedding_batch_size,
        max_chars: cfg.ai.embedding_batch_max_chars,
        timeout: Duration::from_secs(cfg.ai.embedding_batch_timeout_secs),
    };
    fetch_batches(texts, &options).await
}

struct BatchOptions<'a> {
    url: &'a str,
    model: &'a str,
    /// 服务鉴权 Key，未配置时不附加 Authorization 头
    api_key: Option<&'a str>,
    max_items: usize,
    max_chars: usize,
    timeout: Duration,
}

struct BatchFailure {
    error: anyhow::Error,
    split: bool,
    retry: bool,
}

impl BatchFailure {
    fn transport(error: reqwest::Error, context: &str) -> Self {
        let split = error.is_timeout();
        let retry = error.is_timeout() || error.is_connect() || error.is_request() || error.is_body();
        let cause = anyhow::Error::new(error);
        Self {
            split,
            retry,
            // The file processing log prints Display, so preserve the cause in that message.
            error: anyhow::anyhow!("{context}: {cause:#}"),
        }
    }
}

fn batches(texts: &[String], max_items: usize, max_chars: usize) -> Vec<&[String]> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut chars = 0usize;
    for (i, text) in texts.iter().enumerate() {
        let count = text.chars().count();
        if i > start && (i - start >= max_items.max(1) || chars.saturating_add(count) > max_chars.max(1)) {
            result.push(&texts[start..i]);
            start = i;
            chars = 0;
        }
        chars = chars.saturating_add(count);
    }
    if start < texts.len() {
        result.push(&texts[start..]);
    }
    result
}

async fn fetch_batches(texts: &[String], options: &BatchOptions<'_>) -> Result<Vec<Vec<f32>>> {
    // Stack pushes right before left, preserving input order even after adaptive splitting.
    let mut pending = batches(texts, options.max_items, options.max_chars);
    pending.reverse();
    let mut all = Vec::with_capacity(texts.len());
    while let Some(batch) = pending.pop() {
        for attempt in 0..3 {
            match request_batch(batch, options).await {
                Ok(vectors) => {
                    all.extend(vectors);
                    break;
                }
                Err(failure) if failure.split && batch.len() > 1 => {
                    log::warn!("{}; splitting batch for retry", failure.error);
                    let (left, right) = batch.split_at(batch.len() / 2);
                    pending.push(right);
                    pending.push(left);
                    break;
                }
                Err(failure) if failure.retry && attempt < 2 => {
                    log::warn!("{}; retry {}/2", failure.error, attempt + 1);
                    tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
                }
                Err(failure) => return Err(failure.error),
            }
        }
    }
    Ok(all)
}

async fn request_batch(
    texts: &[String], options: &BatchOptions<'_>,
) -> std::result::Result<Vec<Vec<f32>>, BatchFailure> {
    let context = format!(
        "batch embedding request failed: url={}, batch_size={}, total_chars={}, timeout={}s",
        options.url,
        texts.len(),
        texts.iter().map(|t| t.chars().count()).sum::<usize>(),
        options.timeout.as_secs_f64()
    );
    let request = EmbeddingRequest { model: options.model.to_string(), input: texts.to_vec() };
    let response = with_api_key(BATCH_HTTP_CLIENT.post(options.url), options.api_key)
        .timeout(options.timeout)
        .json(&request)
        .send()
        .await
        .map_err(|err| BatchFailure::transport(err, &context))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(BatchFailure {
            error: anyhow::anyhow!("{context}: HTTP {status}: {}", body.chars().take(1000).collect::<String>()),
            split: status == reqwest::StatusCode::PAYLOAD_TOO_LARGE,
            retry: status == reqwest::StatusCode::REQUEST_TIMEOUT
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error(),
        });
    }
    let response: EmbeddingResponse = response.json().await.map_err(|err| BatchFailure::transport(err, &context))?;
    ordered_vectors(response, texts.len()).map_err(|err| BatchFailure {
        error: anyhow::anyhow!("{context}: invalid response: {err:#}"),
        split: false,
        retry: false,
    })
}

fn ordered_vectors(response: EmbeddingResponse, count: usize) -> Result<Vec<Vec<f32>>> {
    anyhow::ensure!(response.data.len() == count, "expected {count} vectors, got {}", response.data.len());
    let indexed = response.data.iter().any(|item| item.index.is_some());
    let mut ordered = vec![None; count];
    for (position, item) in response.data.into_iter().enumerate() {
        let index = if indexed { item.index.context("mixed indexed and unindexed vectors")? } else { position };
        anyhow::ensure!(index < count, "vector index {index} is out of range");
        anyhow::ensure!(ordered[index].is_none(), "duplicate vector index {index}");
        anyhow::ensure!(!item.embedding.is_empty(), "empty vector at index {index}");
        ordered[index] = Some(item.embedding);
    }
    ordered.into_iter().map(|item| item.context("missing vector")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::Path, http::{HeaderMap, StatusCode}, routing::post};
    use serde_json::{Value, json};
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    #[test]
    fn embedding_batches_bound_characters_without_losing_text() {
        let mut texts = vec!["文".repeat(7635); 8];
        texts[7].push('字');
        assert_eq!(texts.iter().map(|s| s.chars().count()).sum::<usize>(), 61081);
        let chunks = batches(&texts, 8, 16000);
        assert_eq!(chunks.iter().map(|c| c.len()).collect::<Vec<_>>(), vec![2, 2, 2, 2]);
        assert_eq!(chunks.into_iter().flatten().collect::<Vec<_>>(), texts.iter().collect::<Vec<_>>());
        let oversized = vec!["x".repeat(20000), "y".into()];
        assert_eq!(batches(&oversized, 8, 16000)[0][0].len(), 20000);
        assert_eq!(batches(&texts, 0, 0).len(), 8);
        assert!(batches(&[], 8, 16000).is_empty());
    }

    #[tokio::test]
    async fn embedding_http_batches_split_retry_and_preserve_alignment() {
        let calls = Arc::new(Mutex::new(HashMap::<String, usize>::new()));
        let state = calls.clone();
        let app = Router::new().route(
            "/{mode}",
            post(move |Path(mode): Path<String>, Json(body): Json<Value>| {
                let calls = state.clone();
                async move {
                    let attempt = {
                        let mut counts = calls.lock().unwrap();
                        let count = counts.entry(mode.clone()).or_default();
                        *count += 1;
                        *count
                    };
                    let inputs = body["input"].as_array().unwrap();
                    if mode == "large" && inputs.len() > 1 {
                        return (StatusCode::PAYLOAD_TOO_LARGE, Json(json!({"error":"too large"})));
                    }
                    if mode == "timeout" && inputs.len() > 1 || mode == "single-timeout" {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    if mode == "unauthorized" {
                        return (StatusCode::UNAUTHORIZED, Json(json!({"error":"bad credentials"})));
                    }
                    if mode == "outage" || mode == "transient" && attempt == 1 {
                        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":"busy"})));
                    }
                    let mut data: Vec<_> = inputs
                        .iter()
                        .enumerate()
                        .map(|(index, input)| {
                            let value: f32 = input.as_str().unwrap().parse().unwrap();
                            json!({"index":index,"embedding":[value, 1.0]})
                        })
                        .collect();
                    data.reverse();
                    if mode == "bad" {
                        data.pop();
                    }
                    (StatusCode::OK, Json(json!({"data":data})))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let texts: Vec<_> = (0..4).map(|i| i.to_string()).collect();
        for mode in ["large", "timeout", "transient"] {
            let url = format!("{root}/{mode}");
            let options = BatchOptions {
                url: &url,
                model: "test",
                api_key: None,
                max_items: 8,
                max_chars: 16000,
                timeout: Duration::from_millis(100),
            };
            let vectors = fetch_batches(&texts, &options).await.unwrap();
            assert_eq!(vectors, (0..4).map(|i| vec![i as f32, 1.0]).collect::<Vec<_>>(), "{mode}");
        }
        assert_eq!(calls.lock().unwrap()["large"], 7);
        assert_eq!(calls.lock().unwrap()["timeout"], 7);
        assert_eq!(calls.lock().unwrap()["transient"], 2);
        for (mode, expected, message) in [
            ("unauthorized", 1, "401"),
            ("outage", 3, "503"),
            ("bad", 1, "expected 4 vectors"),
            ("single-timeout", 3, "timed out"),
        ] {
            let url = format!("{root}/{mode}");
            let options = BatchOptions {
                url: &url,
                model: "test",
                api_key: None,
                max_items: 8,
                max_chars: 16000,
                timeout: Duration::from_millis(100),
            };
            let input = if mode == "single-timeout" { &texts[..1] } else { &texts[..] };
            let error = fetch_batches(input, &options).await.unwrap_err().to_string();
            assert!(error.contains(message), "{mode}: {error}");
            assert!(error.contains("batch_size=") && error.contains("total_chars="), "{error}");
            assert_eq!(calls.lock().unwrap()[mode], expected, "{mode}");
        }
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/closed", closed.local_addr().unwrap());
        drop(closed);
        let options = BatchOptions {
            url: &url,
            model: "test",
            api_key: None,
            max_items: 8,
            max_chars: 16000,
            timeout: Duration::from_secs(1),
        };
        let error = fetch_batches(&texts[..1], &options).await.unwrap_err().to_string();
        assert!(error.to_lowercase().contains("connect"), "transport cause must be visible: {error}");
        server.abort();
    }

    #[tokio::test]
    async fn embedding_requests_send_configured_api_key() {
        let seen = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
        let state = seen.clone();
        let app = Router::new().route(
            "/",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let seen = state.clone();
                async move {
                    let auth = headers.get("authorization").and_then(|value| value.to_str().ok()).map(String::from);
                    seen.lock().unwrap().push(auth);
                    let data: Vec<_> = body["input"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .enumerate()
                        .map(|(index, _)| json!({"index":index,"embedding":[1.0]}))
                        .collect();
                    (StatusCode::OK, Json(json!({"data":data})))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let options = BatchOptions {
            url: &url,
            model: "test",
            api_key: Some("  secret-key  "),
            max_items: 8,
            max_chars: 16000,
            timeout: Duration::from_secs(5),
        };
        fetch_batches(&["1".to_string()], &options).await.unwrap();
        let options = BatchOptions { api_key: None, ..options };
        fetch_batches(&["1".to_string()], &options).await.unwrap();
        server.abort();
        assert_eq!(*seen.lock().unwrap(), vec![Some("Bearer secret-key".to_string()), None]);
    }

    #[test]
    fn embedding_response_rejects_ambiguous_indices() {
        for data in [
            json!([{"index":0,"embedding":[1.0]}, {"index":0,"embedding":[2.0]}]),
            json!([{"index":2,"embedding":[1.0]}, {"index":1,"embedding":[2.0]}]),
            json!([{"index":0,"embedding":[1.0]}, {"embedding":[2.0]}]),
            json!([{"index":0,"embedding":[]}, {"index":1,"embedding":[2.0]}]),
        ] {
            let response = serde_json::from_value(json!({"data":data})).unwrap();
            assert!(ordered_vectors(response, 2).is_err());
        }
        let response = serde_json::from_value(json!({"data":[{"embedding":[1.0]},{"embedding":[2.0]}]})).unwrap();
        assert_eq!(ordered_vectors(response, 2).unwrap(), vec![vec![1.0], vec![2.0]]);
    }
}
