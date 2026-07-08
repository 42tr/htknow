mod common;
use std::{
    fs, time::{Duration, Instant}
};

use axum::http::StatusCode;
use common::*;
use futures::future::join_all;

#[derive(Debug, Clone)]
struct RequestResult {
    status: StatusCode,
    duration: Duration,
}

#[derive(Debug)]
struct ConcurrencyMetrics {
    total: usize,
    success: usize,
    failed: usize,
    mean_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    rps: f64,
    total_duration_ms: f64,
}

impl std::fmt::Display for ConcurrencyMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "total={} success={} failed={} rps={:.1} mean={:.1}ms p50={:.1}ms p95={:.1}ms p99={:.1}ms total_time={:.1}ms",
            self.total,
            self.success,
            self.failed,
            self.rps,
            self.mean_ms,
            self.p50_ms,
            self.p95_ms,
            self.p99_ms,
            self.total_duration_ms
        )
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p) as usize;
    sorted[idx]
}

fn compute_metrics(results: Vec<RequestResult>, total_duration: Duration) -> ConcurrencyMetrics {
    let total = results.len();
    let success = results.iter().filter(|r| r.status.is_success()).count();
    let failed = total.saturating_sub(success);

    let mut durations: Vec<f64> = results.iter().map(|r| r.duration.as_secs_f64() * 1000.0).collect();
    durations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let mean_ms = durations.iter().sum::<f64>() / total.max(1) as f64;
    let p50_ms = percentile(&durations, 0.5);
    let p95_ms = percentile(&durations, 0.95);
    let p99_ms = percentile(&durations, 0.99);
    let total_duration_ms = total_duration.as_secs_f64() * 1000.0;
    let rps = total as f64 / total_duration.as_secs_f64().max(f64::EPSILON);

    ConcurrencyMetrics { total, success, failed, mean_ms, p50_ms, p95_ms, p99_ms, rps, total_duration_ms }
}

fn kb_create_body(name: &str) -> Value {
    serde_json::json!({
        "name": name,
        "description": "perf test knowledge base",
        "kb_type": "analysis",
        "parent_id": null,
        "is_public": false
    })
}

#[tokio::test]
async fn concurrent_kb_list_read_perf() {
    let app = app().await;
    let user = TestUser::new("perf-kb-list");

    // Seed a few KBs so the list endpoint has real work to do.
    for _ in 0..5 {
        let req = authed_json_request(
            "POST",
            "/api/v1/knowledge/knowledge_base/",
            &user,
            kb_create_body(&format!("Seed KB {}", next_seq())),
        );
        let _ = app.clone().oneshot(req).await.unwrap();
    }

    const REQUEST_COUNT: usize = 200;
    const CONCURRENCY: usize = 50;

    let start = Instant::now();
    let futures: Vec<_> = (0..REQUEST_COUNT)
        .map(|i| {
            let app = app.clone();
            let user = TestUser::with_role(&format!("perf-kb-list-user-{}", i), "admin");
            async move {
                let req_start = Instant::now();
                let req = authed_empty_request("GET", "/api/v1/knowledge/knowledge_base/", &user);
                let res = app.oneshot(req).await.unwrap();
                RequestResult { status: res.status(), duration: req_start.elapsed() }
            }
        })
        .collect();

    let results = join_all(futures).await;
    let metrics = compute_metrics(results, start.elapsed());
    println!("concurrent_kb_list_read_perf ({} requests, concurrency={}): {}", REQUEST_COUNT, CONCURRENCY, metrics);

    assert_eq!(metrics.success, metrics.total, "all KB list requests should succeed");
    assert!(metrics.p95_ms < 500.0, "p95 latency should be below 500ms, got {:.1}ms", metrics.p95_ms);
}

#[tokio::test]
async fn concurrent_kb_create_write_perf() {
    let app = app().await;

    const REQUEST_COUNT: usize = 100;
    const CONCURRENCY: usize = 20;

    let start = Instant::now();
    let futures: Vec<_> = (0..REQUEST_COUNT)
        .map(|i| {
            let app = app.clone();
            let user = TestUser::with_role(&format!("perf-kb-create-user-{}", i), "admin");
            async move {
                let req_start = Instant::now();
                let body = kb_create_body(&format!("Perf KB {}", next_seq()));
                let req = authed_json_request("POST", "/api/v1/knowledge/knowledge_base/", &user, body);
                let res = app.oneshot(req).await.unwrap();
                RequestResult { status: res.status(), duration: req_start.elapsed() }
            }
        })
        .collect();

    let results = join_all(futures).await;
    let metrics = compute_metrics(results, start.elapsed());
    println!("concurrent_kb_create_write_perf ({} requests, concurrency={}): {}", REQUEST_COUNT, CONCURRENCY, metrics);

    assert_eq!(metrics.success, metrics.total, "all KB create requests should succeed");
    assert!(metrics.p95_ms < 1000.0, "p95 latency should be below 1000ms, got {:.1}ms", metrics.p95_ms);
}

#[tokio::test]
async fn concurrent_file_list_read_perf() {
    let app = app().await;
    let pool = get_pool().await;
    let env = setup_env();
    let user = TestUser::new("perf-file-list");

    let kb_id = insert_kb(&pool, &user, "Perf File KB", "analysis", None, false).await;

    // Seed a few files so the file list endpoint has real work to do.
    let file_dir = env.data_dir.join("files");
    fs::create_dir_all(&file_dir).unwrap();
    for _ in 0..5 {
        let path = file_dir.join(format!("perf-file-{}.txt", next_seq()));
        fs::write(&path, b"perf test content").unwrap();
        insert_file(&pool, &user, "perf.txt", &path, Some(kb_id), vec!["perf".to_string()], false).await;
    }

    const REQUEST_COUNT: usize = 150;
    const CONCURRENCY: usize = 30;

    let start = Instant::now();
    let futures: Vec<_> = (0..REQUEST_COUNT)
        .map(|i| {
            let app = app.clone();
            let user = TestUser::with_role(&format!("perf-file-list-user-{}", i), "admin");
            async move {
                let req_start = Instant::now();
                let req = authed_empty_request("GET", "/api/v1/knowledge/files/", &user);
                let res = app.oneshot(req).await.unwrap();
                RequestResult { status: res.status(), duration: req_start.elapsed() }
            }
        })
        .collect();

    let results = join_all(futures).await;
    let metrics = compute_metrics(results, start.elapsed());
    println!("concurrent_file_list_read_perf ({} requests, concurrency={}): {}", REQUEST_COUNT, CONCURRENCY, metrics);

    assert_eq!(metrics.success, metrics.total, "all file list requests should succeed");
    assert!(metrics.p95_ms < 500.0, "p95 latency should be below 500ms, got {:.1}ms", metrics.p95_ms);
}

#[tokio::test]
async fn concurrent_mixed_read_write_perf() {
    let app = app().await;
    let user = TestUser::new("perf-mixed");

    // Seed a few KBs.
    for _ in 0..3 {
        let req = authed_json_request(
            "POST",
            "/api/v1/knowledge/knowledge_base/",
            &user,
            kb_create_body(&format!("Seed {}", next_seq())),
        );
        let _ = app.clone().oneshot(req).await.unwrap();
    }

    const REQUEST_COUNT: usize = 200;
    const CONCURRENCY: usize = 40;
    // 70% read, 30% write.
    const WRITE_RATIO: usize = 7;

    let start = Instant::now();
    let futures: Vec<_> = (0..REQUEST_COUNT)
        .map(|i| {
            let app = app.clone();
            let user = TestUser::with_role(&format!("perf-mixed-user-{}", i), "admin");
            async move {
                let req_start = Instant::now();
                let is_read = i % 10 < WRITE_RATIO;
                let req = if is_read {
                    authed_empty_request("GET", "/api/v1/knowledge/knowledge_base/", &user)
                } else {
                    let body = kb_create_body(&format!("Mixed Perf KB {}", next_seq()));
                    authed_json_request("POST", "/api/v1/knowledge/knowledge_base/", &user, body)
                };
                let res = app.oneshot(req).await.unwrap();
                RequestResult { status: res.status(), duration: req_start.elapsed() }
            }
        })
        .collect();

    let results = join_all(futures).await;
    let metrics = compute_metrics(results, start.elapsed());
    println!(
        "concurrent_mixed_read_write_perf ({} requests, concurrency={}, 70% read / 30% write): {}",
        REQUEST_COUNT, CONCURRENCY, metrics
    );

    assert!(metrics.success >= metrics.total * 99 / 100, "success rate should be >= 99%");
    assert!(metrics.p95_ms < 1000.0, "p95 latency should be below 1000ms, got {:.1}ms", metrics.p95_ms);
}
