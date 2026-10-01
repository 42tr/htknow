mod common;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use axum::{Json, Router, routing::post};
use common::*;
use htknow::{db, search::SearchEngine};
use serde_json::json;
use sqlx::SqlitePool;

/// 启动一个本地 embedding / rerank mock，避免检索时访问外部服务。
async fn start_mock_services() {
    let mock = Router::new()
        .route(
            "/embeddings",
            post(|Json(body): Json<Value>| async move {
                let data: Vec<_> = body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .map(|(index, _)| json!({"index": index, "embedding": [1.0, 0.0]}))
                    .collect();
                Json(json!({"data": data}))
            }),
        )
        .route(
            "/rerank",
            post(|| async { (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "rerank unavailable"}))) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });
    unsafe {
        std::env::set_var("HTKNOW_EMBEDDING_URL", format!("http://{addr}/embeddings"));
        std::env::set_var("HTKNOW_EMBEDDING_DIM", "2");
        std::env::set_var("HTKNOW_RERANK_URL", format!("http://{addr}/rerank"));
        std::env::set_var("LLM_API_URL", "");
    }
}

/// 构造一个共享解析产物：`source` 持有切片，`reuse` 通过 artifact 复用同一批切片。
async fn insert_shared_artifact(pool: &SqlitePool, token: &str) -> (i64, i64, i64) {
    let owner = TestUser::with_role(&format!("rebuild-{token}"), "user");
    let kb = insert_kb(pool, &owner, &format!("kb-{token}"), "analysis", None, false).await;
    let path = setup_env().data_dir.join(format!("{token}.txt"));
    std::fs::write(&path, token).unwrap();
    let source = insert_file(pool, &owner, "source.txt", &path, Some(kb), vec![], false).await;
    let reuse = insert_file(pool, &owner, "reuse.txt", &path, Some(kb), vec![], false).await;
    let slice = insert_slice(pool, source, &format!("{token} shared slice content")).await;
    let artifact_id = sqlx::query(
        "INSERT INTO parse_artifacts (artifact_key, content_hash, slice_type, parser_version, config_hash, source_file_id) \
         VALUES (?, ?, 'text', 'builtin-v1', 'test', ?)",
    )
    .bind(format!("key-{token}"))
    .bind(format!("hash-{token}"))
    .bind(source)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query("UPDATE files SET status = 1, artifact_id = ? WHERE id IN (?, ?)")
        .bind(artifact_id)
        .bind(source)
        .bind(reuse)
        .execute(pool)
        .await
        .unwrap();
    (source, reuse, slice)
}

async fn hit_file_ids(engine: &SearchEngine, token: &str) -> Vec<i64> {
    let mut ids: Vec<i64> = engine.search(token, None, None).await.unwrap().into_iter().map(|r| r.file_id).collect();
    ids.sort_unstable();
    ids
}

#[tokio::test]
async fn rebuilds_keep_shared_artifact_projections_and_writer_stays_usable() {
    setup_env();
    start_mock_services().await;
    let pool = db::init().await.unwrap();
    let engine = SearchEngine::init().await.with_pool(pool.clone());

    // 1. 启动一致性检查：按投影重建，复用文件也必须能被检索到。
    let (source_a, reuse_a, _) = insert_shared_artifact(&pool, "alphaquark").await;
    engine.maybe_rebuild_tantivy_from_db().await.unwrap();
    assert_eq!(hit_file_ids(&engine, "alphaquark").await, vec![source_a, reuse_a]);

    // 2. 全量重建：重建期间发生的删除必须回放到新索引。
    let (source_b, reuse_b, _) = insert_shared_artifact(&pool, "betaquark").await;
    let deleted_during_rebuild = Arc::new(AtomicBool::new(false));
    let flag = deleted_during_rebuild.clone();
    let engine_in_cb = engine.clone();
    engine
        .rebuild_tantivy_indexes("it", move |progress| {
            let flag = flag.clone();
            let engine = engine_in_cb.clone();
            async move {
                // build_slice 阶段已读取过数据库快照、且未持有写锁。
                if progress.phase == "build_slice" && progress.processed_docs > 0 && !flag.swap(true, Ordering::SeqCst)
                {
                    engine.delete(Some(reuse_b), None).await.unwrap();
                }
            }
        })
        .await
        .unwrap();
    assert!(deleted_during_rebuild.load(Ordering::SeqCst), "rebuild must report build progress");
    assert_eq!(hit_file_ids(&engine, "alphaquark").await, vec![source_a, reuse_a]);
    assert_eq!(hit_file_ids(&engine, "betaquark").await, vec![source_b], "delete during rebuild was lost");

    // 3. 重建换目录后，常驻 writer 必须继续可用，且不能把旧 segment 写回新目录。
    engine.delete(Some(reuse_a), None).await.unwrap();
    assert_eq!(hit_file_ids(&engine, "alphaquark").await, vec![source_a]);
    assert_eq!(hit_file_ids(&engine, "betaquark").await, vec![source_b]);

    // 4. 从磁盘重新打开索引（模拟重启）：文档数一致，说明 meta.json 没有被旧 writer 覆盖。
    //    LanceDB 是进程级单例，不能在同一进程里再 init 一次 SearchEngine，这里直接打开 Tantivy 目录。
    let path = htknow::config::get().search.tantivy_index_path.clone();
    let index = htknow::search::tantivy_engine::open_existing(&path).unwrap();
    let on_disk_docs = index.reader().unwrap().searcher().num_docs();
    assert_eq!(on_disk_docs, 2, "on-disk index should hold exactly source_a and source_b projections");
}
