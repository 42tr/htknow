mod common;
use std::{
    fs,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
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

/// 延迟门禁阈值。
///
/// 这些断言只在「安静机器 + release 构建」下才有意义：debug 构建没有优化，
/// 而且 `cargo test` 会把本文件里的四个压测用例并行跑，互相争抢 CPU 与 SQLite 写锁，
/// 原阈值必然偶发失败。因此 debug 下放宽 5 倍；需要严格把关时（CI 上跑 release）
/// 设 `HTKNOW_STRICT_PERF=1` 用原始阈值。成功率断言不受影响，任何构建下都生效。
fn threshold(ms: f64) -> f64 {
    if std::env::var("HTKNOW_STRICT_PERF").is_ok_and(|value| value == "1") {
        return ms;
    }
    if cfg!(debug_assertions) { ms * 5.0 } else { ms }
}

/// 压测用例之间的串行闸门。
///
/// 这四个用例共享同一个 app 和 SQLite 连接池，而 `cargo test` 默认在同一进程里多线程执行，
/// 于是「用例内部刻意制造的并发」会和「用例之间的互相排队」叠加，p95 完全变成噪声
/// （实测同一个用例能从 500ms 抖到 6s）。闸门只隔离用例之间，不影响单个用例内部的并发测量。
static PERF_GATE: Mutex<()> = Mutex::new(());

fn perf_gate() -> MutexGuard<'static, ()> {
    PERF_GATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 清掉本文件压测造出来的知识库。
///
/// 四个用例共用一个 SQLite：写压测一次留下上百个库，随后的读压测（admin 身份会列出全部库
/// 并逐库算文件数/子库数）就在完全不同的数据量上测量，p95 能翻好几倍。清理放在断言之前，
/// 保证用例失败时也不会把脏数据留给下一个用例。
async fn cleanup_perf_kbs(user_id_prefix: &str) {
    let pool = get_pool().await;
    sqlx::query("DELETE FROM knowledge_bases WHERE user_id LIKE ?")
        .bind(format!("{user_id_prefix}%"))
        .execute(&pool)
        .await
        .unwrap();
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
    let _gate = perf_gate();
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
    let limit = threshold(500.0);
    assert!(metrics.p95_ms < limit, "p95 latency should be below {limit:.0}ms, got {:.1}ms", metrics.p95_ms);
}

#[tokio::test]
async fn concurrent_kb_create_write_perf() {
    let _gate = perf_gate();
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
    cleanup_perf_kbs("perf-kb-create-user-").await;

    assert_eq!(metrics.success, metrics.total, "all KB create requests should succeed");
    let limit = threshold(1000.0);
    assert!(metrics.p95_ms < limit, "p95 latency should be below {limit:.0}ms, got {:.1}ms", metrics.p95_ms);
}

#[tokio::test]
async fn concurrent_file_list_read_perf() {
    let _gate = perf_gate();
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
    let limit = threshold(500.0);
    assert!(metrics.p95_ms < limit, "p95 latency should be below {limit:.0}ms, got {:.1}ms", metrics.p95_ms);
}

#[tokio::test]
async fn concurrent_mixed_read_write_perf() {
    let _gate = perf_gate();
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
    cleanup_perf_kbs("perf-mixed").await;

    assert!(metrics.success >= metrics.total * 99 / 100, "success rate should be >= 99%");
    let limit = threshold(1000.0);
    assert!(metrics.p95_ms < limit, "p95 latency should be below {limit:.0}ms, got {:.1}ms", metrics.p95_ms);
}
