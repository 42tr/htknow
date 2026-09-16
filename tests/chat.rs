mod common;
use axum::{Json, Router, middleware, routing::post};
use common::*;
use htknow::{
    api, auth, db,
    search::SearchEngine,
    wiki::page::{self, PageDraft},
};
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn parse_events(body: &str) -> Vec<(String, Value)> {
    body.split("\n\n")
        .filter_map(|block| {
            let event = block.lines().find_map(|line| line.strip_prefix("event: "))?;
            let data = block.lines().find_map(|line| line.strip_prefix("data: "))?;
            Some((event.into(), serde_json::from_str(data).unwrap()))
        })
        .collect()
}

#[tokio::test]
async fn chat_stream_uses_scoped_evidence_and_reports_upstream_failures() {
    setup_env();
    let received = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = received.clone();
    let mode = Arc::new(AtomicUsize::new(0));
    let mock_mode = mode.clone();
    let disconnected = Arc::new(AtomicBool::new(false));
    let disconnected_mock = disconnected.clone();
    let mock = Router::new()
        .route("/embeddings", post(|Json(body): Json<Value>| async move {
            Json(json!({"data": body["input"].as_array().unwrap().iter().enumerate().map(|(index, _)| json!({"index":index,"embedding":[1.0,0.0]})).collect::<Vec<_>>()}))
        }))
        .route("/rerank", post(|Json(body): Json<Value>| async move {
            Json(json!(body["texts"].as_array().unwrap().iter().enumerate().map(|(index, _)| json!({"index":index,"score":0.9})).collect::<Vec<_>>()))
        }))
        .route("/chat", post(move |Json(body): Json<Value>| {
            captured.lock().unwrap().push(body);
            let mode = mock_mode.load(Ordering::SeqCst);
            let disconnected = disconnected_mock.clone();
            async move {
                if mode == 1 {
                    return Response::builder().status(503).body(Body::from("private upstream error")).unwrap();
                }
                if mode == 3 {
                    struct Guard(Arc<AtomicBool>);
                    impl Drop for Guard { fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); } }
                    let stream = futures::stream::unfold((0, Guard(disconnected)), |(index, guard)| async move {
                        if index > 0 { tokio::time::sleep(std::time::Duration::from_millis(100)).await; }
                        Some((Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n")), (index + 1, guard)))
                    });
                    return Response::builder().header("content-type", "text/event-stream").body(Body::from_stream(stream)).unwrap();
                }
                let mut data = ": comment\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"螺旋桨\"}}]}\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"有两类。[1]\"}}]}\n\n".as_bytes().to_vec();
                if mode == 0 { data.extend_from_slice(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"); }
                // 任意字节边界分片，包括中文 UTF-8 和 SSE CRLF。
                let chunks: Vec<_> = data.chunks(7).map(|v| Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(v))).collect();
                Response::builder().header("content-type", "text/event-stream")
                    .body(Body::from_stream(futures::stream::iter(chunks))).unwrap()
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    unsafe {
        std::env::set_var("LLM_API_URL", format!("http://{address}/chat"));
        std::env::set_var("LLM_MODEL", "test-chat");
        std::env::set_var("HTKNOW_EMBEDDING_URL", format!("http://{address}/embeddings"));
        std::env::set_var("HTKNOW_EMBEDDING_DIM", "2");
        std::env::set_var("HTKNOW_RERANK_URL", format!("http://{address}/rerank"));
    }
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let pool = db::init().await.unwrap();
    let engine = SearchEngine::init().await.with_pool(pool.clone());
    let app = Router::new()
        .nest("/api/v1/knowledge/", api::app(pool.clone(), engine.clone()))
        .layer(middleware::from_fn(auth));
    let owner = TestUser::with_role("chat-owner", "user");
    let stranger = TestUser::with_role("chat-stranger", "user");
    let kb = insert_kb(&pool, &owner, "Propellers", "analysis", None, false).await;
    let secret_kb = insert_kb(&pool, &stranger, "Secret", "analysis", None, false).await;
    let empty_kb = insert_kb(&pool, &owner, "Empty", "analysis", None, false).await;
    let path = setup_env().data_dir.join("propellers.txt");
    std::fs::write(&path, "propeller evidence").unwrap();
    let file = insert_file(&pool, &owner, "propellers.txt", &path, Some(kb), vec![], false).await;
    sqlx::query("UPDATE files SET status=1 WHERE id=?").bind(file).execute(&pool).await.unwrap();
    let slice = insert_slice(&pool, file, "propeller FPP has fixed pitch; CPP can adjust pitch").await;
    engine
        .write(
            htknow::search::tantivy_engine::Document::new(
                slice,
                file,
                Some(kb),
                "propeller FPP has fixed pitch; CPP can adjust pitch".into(),
            ),
            None,
        )
        .await
        .unwrap();
    engine.reload_readers().unwrap();
    for (kb_id, content) in [(kb, "propeller has FPP and CPP types"), (secret_kb, "propeller TOP_SECRET_EVIDENCE")] {
        let source_files = if kb_id == kb { vec![file] } else { vec![] };
        let source_slices = if kb_id == kb { vec![slice] } else { vec![] };
        page::upsert(
            &pool,
            &PageDraft {
                kb_id,
                slug: "concept/propeller".into(),
                title: "propeller".into(),
                page_type: "concept".into(),
                content: content.into(),
                summary: content.into(),
                aliases: vec![],
                edit_source: "pipeline".into(),
                editor_id: String::new(),
            },
            &source_files,
            &source_slices,
        )
        .await
        .unwrap();
    }
    let request = json!({"question":"propeller types", "kb_id":kb, "messages":[
        {"role":"user","content":"Tell me about propeller"}, {"role":"assistant","content":"Previous answer [99]"}
    ]});
    let response = app
        .clone()
        .oneshot(authed_json_request("POST", "/api/v1/knowledge/chat", &owner, request.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    assert!(response.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    let events = parse_events(&text);
    assert!(events.iter().all(|(event, _)| event != "error"), "{text}");
    let sources = &events.iter().find(|(event, _)| event == "sources").unwrap().1["sources"];
    assert!(sources.as_array().unwrap().iter().any(|s| s["result"]["file_id"] == file && s["result"]["id"] == slice));
    assert!(sources.as_array().unwrap().iter().any(|s| s["result"]["wiki"].is_object()));
    assert!(!text.contains("TOP_SECRET_EVIDENCE"));
    assert_eq!(events.last().unwrap().0, "done");
    let answer: String = events
        .iter()
        .filter(|(event, _)| event == "delta")
        .map(|(_, payload)| payload["text"].as_str().unwrap())
        .collect();
    assert_eq!(answer, "螺旋桨有两类。[1]");
    let upstream = received.lock().unwrap()[0].clone();
    assert_eq!(upstream["stream"], true);
    assert_eq!(upstream["model"], "test-chat");
    assert_eq!(upstream["messages"][1]["content"], "Tell me about propeller");
    assert!(!upstream.to_string().contains("TOP_SECRET_EVIDENCE"));

    let before = received.lock().unwrap().len();
    for (user, scope) in [(&owner, empty_kb), (&stranger, kb)] {
        let response = app
            .clone()
            .oneshot(authed_json_request(
                "POST",
                "/api/v1/knowledge/chat",
                user,
                json!({"question":"propeller", "kb_id":scope}),
            ))
            .await
            .unwrap();
        let text = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
        assert!(!text.contains("FPP"));
        assert!(!text.contains("TOP_SECRET_EVIDENCE"));
    }
    assert_eq!(received.lock().unwrap().len(), before, "no accessible sources must not invoke LLM");
    mode.store(3, Ordering::SeqCst);
    let response = app
        .clone()
        .oneshot(authed_json_request("POST", "/api/v1/knowledge/chat", &owner, request.clone()))
        .await
        .unwrap();
    let mut body = response.into_body();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(frame) = body.frame().await {
            if let Ok(bytes) = frame.unwrap().into_data() {
                if String::from_utf8_lossy(&bytes).contains("event: delta") {
                    return;
                }
            }
        }
        panic!("stream closed before the first delta");
    })
    .await
    .expect("must receive deltas before upstream finishes");
    drop(body);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !disconnected.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("disconnect must cancel the upstream stream");

    for upstream_mode in [1, 2] {
        mode.store(upstream_mode, Ordering::SeqCst);
        let response = app
            .clone()
            .oneshot(authed_json_request("POST", "/api/v1/knowledge/chat", &owner, request.clone()))
            .await
            .unwrap();
        let text = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
        let events = parse_events(&text);
        assert_eq!(events.last().unwrap().0, "error", "{text}");
        assert!(!text.contains("private upstream error"));
        assert!(events.iter().all(|(event, _)| event != "done"));
    }
    let invalid = app
        .clone()
        .oneshot(authed_json_request("POST", "/api/v1/knowledge/chat", &owner, json!({"question":" "})))
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    server.abort();
}
