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
    let shape_omissions = Arc::new(AtomicUsize::new(0));
    let mock_shape_omissions = shape_omissions.clone();
    let correction_count = Arc::new(AtomicUsize::new(0));
    let mock_correction_count = correction_count.clone();
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
            let shape_omissions = mock_shape_omissions.clone();
            let correction_count = mock_correction_count.clone();
            // chat 是 agent 架构：模型必须先回 OpenAI 风格的 tool_calls，knowledge_search
            // 才会执行并发出 sources 事件；看到工具结果（role = tool）后再回正文。
            let wants_tool = (mode == 0 || (4..=30).contains(&mode))
                && ([21,22].contains(&mode) || !body["messages"].as_array().is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool")));
            async move {
                // Non-streaming calls are either the independent verifier or the single correction.
                let input = body["messages"].as_array().unwrap().iter().rev().find_map(|m| {
                    serde_json::from_str::<Value>(&message_text(&m["content"])).ok().filter(|v| v.get("answer").is_some() || v.get("sources").is_some() || v.get("initial_search").is_some())
                }).unwrap_or(json!({}));
                let visible = input.get("sources").or_else(|| input.pointer("/initial_search/sources"))
                    .and_then(Value::as_array).cloned().unwrap_or_default();
                let evidence = visible.first().map(|s| json!({"source_id":s["id"],"passage_ids":[1]}));
                let mut grounded = if let Some(evidence) = evidence {
                    json!({"claims":[{"text":if [5,22].contains(&mode) { "虚构的桨叶采用金属X。" } else { "螺旋桨有两类。" },
                        "evidence":[evidence]}],"insufficient":false})
                } else { json!({"claims":[],"insufficient":true}) };
                if mode == 11 { grounded["insufficient"] = json!(true); }
                if [28,29].contains(&mode) {
                    grounded["claims"][0]["text"] = json!(if mode == 28 {"FPP是桨距固定的螺旋桨类型。"} else {"虚构的桨叶采用金属X。"});
                }
                if [18,19].contains(&mode) && !grounded["claims"].as_array().unwrap().is_empty() { grounded["claims"][0]["text"] = json!("FPP的桨距固定，CPP的桨距可调整。"); }
                if mode == 20 && !grounded["claims"].as_array().unwrap().is_empty() {
                    let evidence = visible.iter().take(2).map(|s| json!({"source_id":s["id"],"passage_ids":[1]})).collect::<Vec<_>>();
                    grounded["claims"] = json!((0..3).map(|_| json!({"text":"FPP的桨距为固定，CPP的桨距为可调整。","evidence":evidence})).collect::<Vec<_>>());
                }
                if mode == 26 && body["stream"] == true && !grounded["claims"].as_array().unwrap().is_empty() {
                    let c = grounded["claims"][0].clone();
                    grounded["claims"] = json!(vec![c;32]);
                    grounded["claims"][0]["text"] = json!("FPP的桨距固定，CPP的桨距可调整。");
                }
                if body["stream"] == false {
                    if message_text(&body["messages"][0]["content"]).starts_with("你是原文证据选择器") {
                        let evidence = if [28,29].contains(&mode) {
                            json!(visible.iter().take(2).map(|s| json!({"source_id":s["id"],"passage_ids":[1]})).collect::<Vec<_>>())
                        } else { input["claim"]["evidence"].clone() };
                        return Json(json!({"choices":[{"message":{"role":"assistant","content":json!({"evidence":evidence}).to_string()},"finish_reason":"stop"}]})).into_response();
                    }
                    if message_text(&body["messages"][0]["content"]).starts_with("你是事实拆分器") {
                        let evidence = grounded["claims"][0]["evidence"].clone();
                        let split = json!({"claims":[{"text":"FPP的桨距固定。","evidence":evidence},
                            {"text":if mode == 19 {"虚构的桨叶采用金属X。"} else {"CPP的桨距可调整。"},"evidence":evidence}],"insufficient":false});
                        return Json(json!({"choices":[{"message":{"role":"assistant","content":split.to_string()},"finish_reason":"stop"}]})).into_response();
                    }
                    if message_text(&body["messages"][0]["content"]).starts_with("你是事实原子性复核器") {
                        let input: Value = serde_json::from_str(&message_text(&body["messages"].as_array().unwrap().last().unwrap()["content"])).unwrap();
                        let compound = [18,19,26].contains(&mode) && input["text"].as_str().unwrap().contains('，');
                        let verdict = json!({"atomic":!compound,"reason":if compound {"两个独立属性"} else {"本句只有一个属性，来源的其他属性不属于本句"}});
                        return Json(json!({"choices":[{"message":{"role":"assistant","content":verdict.to_string()},"finish_reason":"stop"}]})).into_response();
                    }
                    let verifying = message_text(&body["messages"][0]["content"]).contains("证据核验器");
                    let covering = message_text(&body["messages"][0]["content"]).starts_with("你是知识库答案完整性核验器");
                    if ((verifying && mode == 23) || (covering && mode == 24)) && shape_omissions.fetch_or(1 << mode, Ordering::SeqCst) & (1 << mode) == 0 {
                        return Json(json!({"choices":[{"message":{"role":"assistant","content":"{\"claims\":["},"finish_reason":"length"}]})).into_response();
                    }
                    if covering {
                        if mode == 16 && body["response_format"]["type"] == "json_schema" {
                            return Response::builder().status(400).body(Body::from("response_format json_schema is not supported")).unwrap();
                        }
                        let coverage = json!({"claim_relevance":input["answer"]["claims"].as_array().unwrap().iter().enumerate().map(|(index,c)|
                            json!({"index":index,"relevant":!c["text"].as_str().unwrap().contains("资料仅包含"),"reason":"question relevance"})).collect::<Vec<_>>(),
                            "insufficient_justified":true,
                            "question_complete":mode != 7 || input["answer"]["claims"].as_array().unwrap().len() == 2,
                            "missing_aspects":if mode == 7 { json!(["FPP与CPP各自的桨距行为"]) } else { json!([]) }});
                        return Json(json!({"choices":[{"message":{"role":"assistant","content":coverage.to_string()},"finish_reason":"stop"}]})).into_response();
                    }
                    if verifying && mode == 14 {
                        return Response::builder().status(503).body(Body::from("private verifier failure")).unwrap();
                    }
                    if verifying && mode == 16 && body["response_format"]["type"] == "json_schema" {
                        return Response::builder().status(400).body(Body::from("response_format json_schema is not supported")).unwrap();
                    }
                    if verifying && mode == 8 && !malformed_verdict.swap(true, Ordering::SeqCst) {
                        return Json(json!({"choices":[{"message":{"role":"assistant","content":"not JSON"},"finish_reason":"stop"}]})).into_response();
                    }
                    let mut content = if verifying {
                        json!({"claims":input["answer"]["claims"].as_array().unwrap().iter().enumerate().map(|(index,c)| {
                            let supported = !c["text"].as_str().unwrap().contains("虚构") && !([25,28].contains(&mode) && c["evidence"].as_array().unwrap().len() == 1);
                            json!({"index":index,"supported":supported,"atomic":![17,18,19,26].contains(&mode),
                                "answers_question":!c["text"].as_str().unwrap().contains("资料仅包含"),
                                "evidence":c["evidence"].as_array().unwrap().iter().enumerate().map(|(i,e)|
                                    json!({"source_id":e["source_id"],"contributes":![9,25].contains(&mode) || i == 0,
                                        "reason":if mode == 9 && i > 0 {"主题相关但不支持这条事实"} else {"supports the requested fact"}})
                                ).collect::<Vec<_>>(),
                                "reason":if supported {"supported"} else {"引用有原句，但不支持金属X"}})
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
                    if !verifying && mode == 27 && correction_count.fetch_add(1, Ordering::SeqCst) < 2 {
                        content["claims"][0]["text"] = json!("虚构的桨叶采用金属X。");
                    }
                    if !verifying && mode == 30 {
                        let id = content["claims"][0]["evidence"][0].as_object_mut().unwrap().remove("source_id").unwrap();
                        content["claims"][0]["evidence"][0]["source_id_ref"] = id;
                    }
                    if verifying && [12, 13, 15].contains(&mode) {
                        let first = shape_omissions.fetch_or(1 << mode, Ordering::SeqCst) & (1 << mode) == 0;
                        if first || mode == 15 {
                            if mode == 12 { content["claims"] = json!([]); }
                            else { content["claims"][0]["evidence"] = json!([]); }
                        }
                    }
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
                if [27,30].contains(&mode) {
                    let id = grounded["claims"][0]["evidence"][0].as_object_mut().unwrap().remove("source_id").unwrap();
                    grounded["claims"][0]["evidence"][0]["source_id_ref"] = id;
                }
                if mode == 4 { grounded["claims"][0]["evidence"] = json!([]); }
                if mode == 6 { grounded["claims"][0]["evidence"][0]["source_id"] = json!(99); }
                if [9,25].contains(&mode) {
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
                if mode == 0 || (4..=30).contains(&mode) { data.extend_from_slice(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"); }
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

    // Support checks cannot see uncited sources or another claim. Coverage has
    // a separate full-evidence request so missing known facts still trigger repair.
    {
        let requests = received.lock().unwrap();
        let support =
            requests.iter().find(|r| message_text(&r["messages"][0]["content"]).contains("证据核验器")).unwrap();
        let input: Value =
            serde_json::from_str(&message_text(&support["messages"].as_array().unwrap().last().unwrap()["content"]))
                .unwrap();
        assert_eq!(input["answer"]["claims"].as_array().unwrap().len(), 1);
        assert!(input.get("question").is_none() && input.get("question_context").is_none());
        assert!(input["evidence_scope"].is_object());
        assert_eq!(input["sources"].as_array().unwrap().len(), 1);
        let original = &sources.as_array().unwrap().iter().find(|s| s["id"] == input["sources"][0]["id"]).unwrap()["result"]
            ["content"];
        assert!(original.as_str().unwrap().contains(input["sources"][0]["content"].as_str().unwrap()));
        let coverage = requests
            .iter()
            .find(|r| message_text(&r["messages"][0]["content"]).starts_with("你是知识库答案完整性核验器"))
            .unwrap();
        let input: Value =
            serde_json::from_str(&message_text(&coverage["messages"].as_array().unwrap().last().unwrap()["content"]))
                .unwrap();
        assert!(input["sources"].as_array().unwrap().len() >= 2);
    }
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
        let before = received.lock().unwrap().len();
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
            assert_eq!(
                received.lock().unwrap()[before..]
                    .iter()
                    .filter(
                        |r| r["stream"] == false && message_text(&r["messages"][0]["content"]).contains("证据核验器")
                    )
                    .count(),
                3,
                "semantic rejection must stop after two answer corrections"
            );
        } else {
            assert_eq!(answer, "螺旋桨有两类。[1]");
            assert_eq!(events.last().unwrap().0, "done");
        }
    }
    mode.store(7, Ordering::SeqCst);
    let before_completion = received.lock().unwrap().len();
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
    {
        let requests = received.lock().unwrap();
        let correction = requests[before_completion..]
            .iter()
            .find(|r| {
                r["stream"] == false && message_text(&r["messages"][0]["content"]).starts_with("你是知识库问答助手")
            })
            .unwrap();
        let claims = &correction["response_format"]["json_schema"]["schema"]["properties"]["claims"];
        assert_eq!(claims["maxItems"], 32);
        assert!(claims.get("prefixItems").is_none());
        assert!(claims["items"]["properties"].get("depends_on").is_none());
        assert!(claims["items"]["properties"]["text"].get("pattern").is_none());
        let expression = regex::Regex::new(
            claims["items"]["properties"]["calculation"]["anyOf"][1]["properties"]["expression"]["pattern"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert!(expression.is_match("40*3*2-5"));
        assert!(!expression.is_match("$0*2"));
    }
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
    // Parsed JSON with missing verdict rows/citations is also a verifier format
    // fault. Repair it without consuming the answer correction budget. Persistent
    // shape/HTTP errors stop with no unverified text and no answer rewrite.
    for current_mode in [12, 13, 14, 15, 16] {
        mode.store(current_mode, Ordering::SeqCst);
        let before = received.lock().unwrap().len();
        let response = app
            .clone()
            .oneshot(authed_json_request("POST", "/api/v1/knowledge/chat", &owner, request.clone()))
            .await
            .unwrap();
        let text = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
        let events = parse_events(&text);
        let success = ![14, 15].contains(&current_mode);
        assert_eq!(events.last().unwrap().0, if success { "done" } else { "error" }, "mode={current_mode}: {text}");
        assert!(!events.iter().any(|(e, v)| e == "status" && v["stage"] == "correcting"), "{text}");
        assert_eq!(events.iter().filter(|(e, _)| e == "delta").count(), usize::from(success));
        assert!(!text.contains("private verifier failure"));
        let requests = received.lock().unwrap();
        let verification = requests[before..]
            .iter()
            .filter(|r| r["stream"] == false && message_text(&r["messages"][0]["content"]).contains("证据核验器"))
            .collect::<Vec<_>>();
        assert_eq!(verification.len(), if current_mode == 14 { 1 } else { 2 });
        assert_eq!(verification[0]["response_format"]["type"], "json_schema");
        assert_eq!(verification[0]["response_format"]["json_schema"]["schema"]["properties"]["claims"]["minItems"], 1);
        assert_eq!(
            verification[0]["response_format"]["json_schema"]["schema"]["properties"]["claims"]["prefixItems"][0]["properties"]
                ["index"]["const"],
            0
        );
        if current_mode == 16 {
            assert_eq!(verification[1]["response_format"]["type"], "json_object");
        }
    }
    // Atomicity rechecks see only the current sentence, never the multi-field
    // evidence paragraph. They still reject a genuinely compound sentence.
    for current_mode in [17, 18, 19] {
        mode.store(current_mode, Ordering::SeqCst);
        let before = received.lock().unwrap().len();
        let response = app
            .clone()
            .oneshot(authed_json_request("POST", "/api/v1/knowledge/chat", &owner, request.clone()))
            .await
            .unwrap();
        let text = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
        let events = parse_events(&text);
        assert_eq!(events.last().unwrap().0, if current_mode != 19 { "done" } else { "error" }, "{text}");
        assert_eq!(events.iter().filter(|(e, _)| e == "delta").count(), usize::from(current_mode != 19));
        let requests = received.lock().unwrap();
        let rechecks = requests[before..]
            .iter()
            .filter(|r| message_text(&r["messages"][0]["content"]).starts_with("你是事实原子性复核器"))
            .collect::<Vec<_>>();
        assert!(rechecks.len() >= if current_mode == 17 { 1 } else { 3 });
        for body in rechecks {
            let input: Value =
                serde_json::from_str(&message_text(&body["messages"].as_array().unwrap().last().unwrap()["content"]))
                    .unwrap();
            assert_eq!(input.as_object().unwrap().keys().cloned().collect::<Vec<_>>(), vec!["text".to_string()]);
            assert!(!body.to_string().contains("fixed pitch"));
        }
    }
    // Several compound sentences are repaired together, without spending one
    // answer correction on each sentence. Every resulting fact is verified.
    mode.store(20, Ordering::SeqCst);
    let before = received.lock().unwrap().len();
    let response = app
        .clone()
        .oneshot(authed_json_request("POST", "/api/v1/knowledge/chat", &owner, request.clone()))
        .await
        .unwrap();
    let text = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    let events = parse_events(&text);
    assert_eq!(events.last().unwrap().0, "done", "{text}");
    assert_eq!(events.iter().filter(|(e, v)| e == "status" && v["stage"] == "correcting").count(), 1);
    {
        let requests = received.lock().unwrap();
        let new = &requests[before..];
        let splits = new
            .iter()
            .filter(|r| message_text(&r["messages"][0]["content"]).starts_with("你是事实拆分器"))
            .collect::<Vec<_>>();
        assert_eq!(splits.len(), 3);
        assert!(splits.iter().all(|r| {
            r["response_format"]["json_schema"]["schema"]["properties"]["claims"]["items"]["properties"]["text"]
                .get("pattern")
                .is_none()
        }));
        assert_eq!(new.iter().filter(|r| message_text(&r["messages"][0]["content"]).contains("证据核验器")).count(), 6);
    }
    // Repeated tool calls fall back to one tool-free draft, which still needs
    // full verification. A fabricated fallback must never become visible.
    for current_mode in [21, 22, 23, 24, 25, 26] {
        mode.store(current_mode, Ordering::SeqCst);
        let before = received.lock().unwrap().len();
        let response = app
            .clone()
            .oneshot(authed_json_request("POST", "/api/v1/knowledge/chat", &owner, request.clone()))
            .await
            .unwrap();
        let text = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
        let events = parse_events(&text);
        assert_eq!(
            events.last().unwrap().0,
            if [22, 25].contains(&current_mode) { "error" } else { "done" },
            "mode={current_mode}: {text}"
        );
        assert_eq!(events.iter().filter(|(e, _)| e == "delta").count(), usize::from(![22, 25].contains(&current_mode)));
        if [23, 24].contains(&current_mode) {
            assert!(!events.iter().any(|(e, v)| e == "status" && v["stage"] == "correcting"));
        }
        let requests = received.lock().unwrap();
        let new = &requests[before..];
        if current_mode <= 22 {
            assert_eq!(new.iter().filter(|r| r["stream"] == true).count(), 4);
            assert!(new.iter().any(|r| r["stream"] == false
                && message_text(&r["messages"][0]["content"]).starts_with("你是知识库问答助手")));
        } else if [23, 24].contains(&current_mode) {
            let marker = if current_mode == 23 {
                "你是知识库答案的证据核验器"
            } else {
                "你是知识库答案完整性核验器"
            };
            assert_eq!(
                new.iter().filter(|r| message_text(&r["messages"][0]["content"]).starts_with(marker)).count(),
                2
            );
        } else if current_mode == 26 {
            assert_eq!(
                new.iter()
                    .filter(|r| message_text(&r["messages"][0]["content"]).starts_with("你是知识库问答助手")
                        && r["stream"] == false)
                    .count(),
                1
            );
        }
        assert!(!text.contains("虚构") && !text.contains("检索前的未核验草稿"));
    }
    // A generation schema fault has its own one-shot budget. It cannot consume
    // either substantive correction or permit persistently malformed output.
    // Passage reselection is independently reverified and never edits the fact.
    for current_mode in [27, 28, 29, 30] {
        mode.store(current_mode, Ordering::SeqCst);
        correction_count.store(0, Ordering::SeqCst);
        let before = received.lock().unwrap().len();
        let response = app
            .clone()
            .oneshot(authed_json_request("POST", "/api/v1/knowledge/chat", &owner, request.clone()))
            .await
            .unwrap();
        let text = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
        let events = parse_events(&text);
        let success = [27, 28].contains(&current_mode);
        assert_eq!(events.last().unwrap().0, if success { "done" } else { "error" }, "mode={current_mode}: {text}");
        assert_eq!(events.iter().filter(|(e, _)| e == "delta").count(), usize::from(success));
        assert!(!text.contains("虚构") && !text.contains("source_id_ref"));
        let requests = received.lock().unwrap();
        let new = &requests[before..];
        let rewrites = new
            .iter()
            .filter(|r| {
                r["stream"] == false && message_text(&r["messages"][0]["content"]).starts_with("你是知识库问答助手")
            })
            .count();
        assert_eq!(
            rewrites,
            match current_mode {
                27 => 3,
                28 => 0,
                29 => 2,
                30 => 1,
                _ => unreachable!(),
            }
        );
        if current_mode == 28 {
            assert!(events.iter().any(|(e, v)| e == "delta" && v["text"] == "FPP是桨距固定的螺旋桨类型。[1][2]"));
            let checks = new
                .iter()
                .filter(|r| message_text(&r["messages"][0]["content"]).contains("证据核验器"))
                .collect::<Vec<_>>();
            assert_eq!(checks.len(), 2);
            let input = |r: &Value| serde_json::from_str::<Value>(&message_text(&r["messages"][1]["content"])).unwrap();
            let (first, second) = (input(checks[0]), input(checks[1]));
            assert_eq!(first["answer"]["claims"][0]["text"], second["answer"]["claims"][0]["text"]);
            assert_eq!(first["sources"].as_array().unwrap().len(), 1);
            assert_eq!(second["sources"].as_array().unwrap().len(), 2);
        }
    }
    // Explicit Wiki scope applies to tool calls, generation, correction and
    // verification, even when retrieval also returns an original with a new value.
    mode.store(0, Ordering::SeqCst);
    let before = received.lock().unwrap().len();
    let response = app
        .clone()
        .oneshot(authed_json_request(
            "POST",
            "/api/v1/knowledge/chat",
            &owner,
            json!({"question":"根据Wiki记录回答propeller types", "kb_id":kb}),
        ))
        .await
        .unwrap();
    let text = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert_eq!(parse_events(&text).last().unwrap().0, "done", "{text}");
    for (event, payload) in parse_events(&text) {
        if event == "sources" {
            assert!(!payload["sources"].as_array().unwrap().is_empty());
            assert!(payload["sources"].as_array().unwrap().iter().all(|s| s["result"]["wiki"].is_object()));
        }
    }
    for body in &received.lock().unwrap()[before..] {
        assert!(!body.to_string().contains("fixed pitch"), "out-of-scope original leaked into model input");
    }
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
        assert_eq!(
            events.iter().any(|(e, v)| e == "status" && v["stage"] == "correcting"),
            current_mode != 9,
            "{text}"
        );
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
