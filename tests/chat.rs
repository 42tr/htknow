mod common;
use axum::{Json, Router, middleware, response::IntoResponse, routing::post};
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

/// 上游消息正文既可能是纯字符串，也可能是 OpenAI 的 content parts 数组（g SDK 用后者）。
fn message_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    content
        .as_array()
        .map(|parts| parts.iter().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join(""))
        .unwrap_or_default()
}

#[tokio::test]
async fn chat_stream_uses_scoped_evidence_and_reports_upstream_failures() {
    setup_env();
    let received = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = received.clone();
    let mode = Arc::new(AtomicUsize::new(0));
    let mock_mode = mode.clone();
    let malformed_verdict = Arc::new(AtomicBool::new(false));
    let mock_malformed_verdict = malformed_verdict.clone();
    let disconnected = Arc::new(AtomicBool::new(false));
    let disconnected_mock = disconnected.clone();
    let mock = Router::new()
        .route("/embeddings", post(|Json(body): Json<Value>| async move {
            Json(json!({"data": body["input"].as_array().unwrap().iter().enumerate().map(|(index, _)| json!({"index":index,"embedding":[1.0,0.0]})).collect::<Vec<_>>()}))
        }))
        .route("/rerank", post(|Json(body): Json<Value>| async move {
            Json(json!(body["texts"].as_array().unwrap().iter().enumerate().map(|(index, _)| json!({"index":index,"score":0.9})).collect::<Vec<_>>()))
        }))
        .route("/chat/completions", post(move |Json(body): Json<Value>| {
            captured.lock().unwrap().push(body.clone());
            let mode = mock_mode.load(Ordering::SeqCst);
            let malformed_verdict = mock_malformed_verdict.clone();
            let disconnected = disconnected_mock.clone();
            // chat 是 agent 架构：模型必须先回 OpenAI 风格的 tool_calls，knowledge_search
            // 才会执行并发出 sources 事件；看到工具结果（role = tool）后再回正文。
            let wants_tool = [0, 4, 5, 6, 7, 8, 9, 10, 11].contains(&mode)
                && !body["messages"].as_array().is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool"));
            async move {
                // Non-streaming calls are either the independent verifier or the single correction.
                let input = body["messages"].as_array().unwrap().iter().rev().find_map(|m| {
                    serde_json::from_str::<Value>(&message_text(&m["content"])).ok()
                }).unwrap_or(json!({}));
                let visible = input.get("sources").or_else(|| input.pointer("/initial_search/sources"))
                    .and_then(Value::as_array).cloned().unwrap_or_default();
                let evidence = visible.first().map(|s| json!({"source_id":s["id"],"passage_ids":[1]}));
                let mut grounded = if let Some(evidence) = evidence {
                    json!({"claims":[{"text":if mode == 5 { "虚构事实999万元。" } else { "螺旋桨有两类。" },
                        "evidence":[evidence]}],"insufficient":false})
                } else { json!({"claims":[],"insufficient":true}) };
                if mode == 11 { grounded["insufficient"] = json!(true); }
                if body["stream"] == false {
                    let verifying = message_text(&body["messages"][0]["content"]).contains("证据核验器");
                    if verifying && mode == 8 && !malformed_verdict.swap(true, Ordering::SeqCst) {
                        return Json(json!({"choices":[{"message":{"role":"assistant","content":"not JSON"},"finish_reason":"stop"}]})).into_response();
                    }
                    let content = if verifying {
                        json!({"claims":input["answer"]["claims"].as_array().unwrap().iter().enumerate().map(|(index,c)| {
                            let supported = !c["text"].as_str().unwrap().contains("虚构");
                            json!({"index":index,"supported":supported,"atomic":true,
                                "answers_question":!c["text"].as_str().unwrap().contains("资料仅包含"),
                                "evidence":c["evidence"].as_array().unwrap().iter().enumerate().map(|(i,e)|
                                    json!({"source_id":e["source_id"],"contributes":mode != 9 || i == 0,
                                        "reason":if mode == 9 && i > 0 {"主题相关但不支持这条事实"} else {"supports the requested fact"}})
                                ).collect::<Vec<_>>(),
                                "reason":if supported {"supported"} else {"引用有原句，但不支持999万元"}})
                        }).collect::<Vec<_>>(),"insufficient_justified":true,"question_complete":mode != 7 || input["answer"]["claims"].as_array().unwrap().len() == 2,
                            "missing_aspects":if mode == 7 { json!(["FPP与CPP各自的桨距行为"]) } else { json!([]) }})
                    } else if mode == 10 {
                        json!({"claims":[],"insufficient":true})
                    } else if mode == 7 {
                        let source = visible.iter().find(|s| s["content"].as_str().unwrap_or("").contains("fixed pitch")).unwrap();
                        let evidence = json!([{"source_id":source["id"],"passage_ids":[1]}]);
                        json!({"claims":[{"text":"FPP的桨距固定。","evidence":evidence},
                            {"text":"CPP的桨距可调整。","evidence":evidence}],"insufficient":false})
                    } else { grounded };
                    return Json(json!({"choices":[{"message":{"role":"assistant","content":content.to_string()},"finish_reason":"stop"}]})).into_response();
                }
                if wants_tool {
                    let payload = json!({
                        "choices": [{
                            "index": 0,
                            "delta": {
                                "role": "assistant",
                                "content": "检索前的未核验草稿",
                                "tool_calls": [{
                                    "index": 0,
                                    "id": "call_1",
                                    "type": "function",
                                    "function": {
                                        "name": "knowledge_search",
                                        "arguments": json!({"query": "propeller types"}).to_string()
                                    }
                                }]
                            },
                            "finish_reason": "tool_calls"
                        }]
                    });
                    // 同样按任意字节边界分片，覆盖 SSE 跨包解析。
                    let data = format!(": comment\r\ndata: {payload}\r\n\r\ndata: [DONE]\n\n");
                    let chunks: Vec<_> =
                        data.as_bytes().chunks(9).map(|v| Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(v))).collect();
                    return Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(Body::from_stream(futures::stream::iter(chunks)))
                        .unwrap();
                }
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
                let mut grounded = grounded;
                if mode == 4 { grounded["claims"][0]["evidence"] = json!([]); }
                if mode == 6 { grounded["claims"][0]["evidence"][0]["source_id"] = json!(99); }
                if mode == 9 {
                    let other = visible.get(1).expect("two current-turn sources");
                    grounded["claims"][0]["evidence"].as_array_mut().unwrap().push(json!({"source_id":other["id"],"passage_ids":[1]}));
                }
                if [10, 11].contains(&mode) {
                    let explanation = json!({"text":"资料仅包含螺旋桨类型，未包含私有预算。", "evidence":grounded["claims"][0]["evidence"]});
                    if mode == 10 { grounded["claims"] = json!([explanation]); }
                    else { grounded["claims"].as_array_mut().unwrap().push(explanation); }
                    grounded["insufficient"] = json!(true);
                }
                let payload = json!({"choices":[{"delta":{"content":grounded.to_string()}}]});
                let mut data = format!(": comment\r\ndata: {payload}\r\n\r\n").into_bytes();
                if [0, 4, 5, 6, 7, 8, 9, 10, 11].contains(&mode) { data.extend_from_slice(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"); }
                // 任意字节边界分片，包括中文 UTF-8 和 SSE CRLF。
                let mut chunks: Vec<Result<bytes::Bytes, std::io::Error>> =
                    data.chunks(7).map(|v| Ok(bytes::Bytes::copy_from_slice(v))).collect();
                if mode == 2 {
                    // 模拟连接中断：body 直接出错，SSE 既没有 finish_reason 也没有 [DONE]。
                    // 单纯「干净 EOF 但缺 [DONE]」会被 SDK 当作正常收尾，测不出上游流未结束。
                    chunks.push(Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "connection reset")));
                }
                Response::builder().header("content-type", "text/event-stream")
                    .body(Body::from_stream(futures::stream::iter(chunks))).unwrap()
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    unsafe {
        // 用文档口径的完整端点：聊天走 SDK（base + /chat/completions），图谱/Wiki 直接 POST。
        std::env::set_var("LLM_API_URL", format!("http://{address}/chat/completions"));
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
    assert_eq!(upstream["messages"][1]["role"], "user");
    assert_eq!(message_text(&upstream["messages"][1]["content"]), "Tell me about propeller");
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
        // agent 架构下工具仍会执行，但无可见来源时 sources 必须为空，不得回灌其他库的证据。
        for (event, payload) in parse_events(&text) {
            if event == "sources" {
                assert!(payload["sources"].as_array().unwrap().is_empty(), "{payload}");
            }
        }
    }
    assert!(received.lock().unwrap().len() > before, "每次对话都应调用上游 LLM");
    // Invalid drafts must never become visible, including a real quote attached to a false claim.
    for current_mode in [4, 5, 6] {
        mode.store(current_mode, Ordering::SeqCst);
        let response = app
            .clone()
            .oneshot(authed_json_request("POST", "/api/v1/knowledge/chat", &owner, request.clone()))
            .await
            .unwrap();
        let text = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
        let events = parse_events(&text);
        let answer: String =
            events.iter().filter(|(e, _)| e == "delta").map(|(_, v)| v["text"].as_str().unwrap()).collect();
        assert!(!answer.contains("虚构") && !answer.contains("[99]"), "{text}");
        if current_mode == 5 {
            assert!(answer.is_empty(), "unverified draft leaked: {text}");
            assert_eq!(events.last().unwrap().0, "error");
            assert!(!events.iter().any(|(e, _)| e == "done"));
        } else {
            assert_eq!(answer, "螺旋桨有两类。[1]");
            assert_eq!(events.last().unwrap().0, "done");
        }
    }
    mode.store(7, Ordering::SeqCst);
    let response = app
        .clone()
        .oneshot(authed_json_request(
            "POST",
            "/api/v1/knowledge/chat",
            &owner,
            json!({"question":"FPP和CPP的桨距有何不同？", "kb_id":kb}),
        ))
        .await
        .unwrap();
    let text = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    let events = parse_events(&text);
    let answer: String =
        events.iter().filter(|(e, _)| e == "delta").map(|(_, v)| v["text"].as_str().unwrap()).collect();
    assert!(answer.contains("FPP的桨距固定") && answer.contains("CPP的桨距可调整"), "{text}");
    assert!(!answer.contains("螺旋桨有两类"), "incomplete draft leaked: {text}");
    assert!(events.iter().any(|(e, v)| e == "status" && v["stage"] == "correcting"));
    assert_eq!(events.last().unwrap().0, "done");
    // A malformed verifier response gets a bounded schema repair, without rewriting the answer.
    mode.store(8, Ordering::SeqCst);
    let response = app
        .clone()
        .oneshot(authed_json_request("POST", "/api/v1/knowledge/chat", &owner, request.clone()))
        .await
        .unwrap();
    let text = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    let events = parse_events(&text);
    assert!(malformed_verdict.load(Ordering::SeqCst));
    assert_eq!(events.last().unwrap().0, "done", "{text}");
    assert!(
        !events.iter().any(|(e, v)| e == "status" && v["stage"] == "correcting"),
        "answer was needlessly rewritten: {text}"
    );
    assert!(events.iter().any(|(e, v)| e == "delta" && v["text"] == "螺旋桨有两类。[1]"));
    // Reject a redundant citation even when the overall claim is supported.
    // Refusal explanations are removed without discarding a supported partial answer.
    for (current_mode, question) in
        [(9, "propeller types"), (10, "私有预算是多少？"), (11, "螺旋桨有几类，私有预算是多少？")]
    {
        mode.store(current_mode, Ordering::SeqCst);
        let response = app
            .clone()
            .oneshot(authed_json_request(
                "POST",
                "/api/v1/knowledge/chat",
                &owner,
                json!({"question":question,"kb_id":kb}),
            ))
            .await
            .unwrap();
        let text = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
        let events = parse_events(&text);
        let answer: String =
            events.iter().filter(|(e, _)| e == "delta").map(|(_, v)| v["text"].as_str().unwrap()).collect();
        assert_eq!(events.last().unwrap().0, "done", "{text}");
        assert!(events.iter().any(|(e, v)| e == "status" && v["stage"] == "correcting"), "{text}");
        assert!(!answer.contains("资料仅包含") && !answer.contains("[2]"), "{text}");
        match current_mode {
            9 => assert_eq!(answer, "螺旋桨有两类。[1]"),
            10 => {
                assert!(answer.contains("无法确认该问题的答案"), "{text}");
                assert!(!answer.contains('['), "{text}");
            }
            11 => {
                assert!(answer.starts_with("螺旋桨有两类。[1]"), "{text}");
                assert!(answer.contains("其余部分"), "{text}");
            }
            _ => unreachable!(),
        }
    }
    mode.store(3, Ordering::SeqCst);
    let before_disconnect = received.lock().unwrap().len();
    let response = app
        .clone()
        .oneshot(authed_json_request("POST", "/api/v1/knowledge/chat", &owner, request.clone()))
        .await
        .unwrap();
    let body = response.into_body();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while received.lock().unwrap().len() == before_disconnect {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    })
    .await
    .expect("upstream must start while answer text remains buffered");
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
