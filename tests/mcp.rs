mod common;
use axum::Router;
use common::*;

/// MCP streamable HTTP 的集成测试：initialize → tools/list → tools/call，
/// 验证鉴权复用、工具清单和参数校验。
///
/// 服务默认 legacy_session_mode（兼容 2026-07-28 之前的协议版本）：
/// initialize 响应带 mcp-session-id，后续请求必须回传；POST 响应是 SSE 帧。

fn mcp_request(user: &TestUser, body: Value, session_id: Option<&str>) -> Request<Body> {
    let mut req = authed_json_request("POST", "/mcp", user, body);
    let headers = req.headers_mut();
    headers.insert(header::ACCEPT, "application/json, text/event-stream".parse().unwrap());
    // rmcp 会校验 Host（防 DNS 重绑定），即使放行名单为空也要求该头存在。
    headers.insert(header::HOST, "localhost".parse().unwrap());
    if let Some(session_id) = session_id {
        headers.insert("mcp-session-id", session_id.parse().unwrap());
    }
    req
}

/// 响应可能是 application/json 或 text/event-stream（事件流里找 data 帧）。
async fn mcp_response_json(res: Response<Body>) -> Value {
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    if content_type.starts_with("application/json") {
        return serde_json::from_str(&text).unwrap_or_else(|e| panic!("bad json: {e}\nbody: {text}"));
    }
    assert!(content_type.starts_with("text/event-stream"), "unexpected content-type {content_type}: {text}");
    let data = text
        .split("\n\n")
        .filter_map(|block| block.lines().find_map(|line| line.strip_prefix("data: ")))
        .last()
        .unwrap_or_else(|| panic!("no data frame in SSE body: {text}"));
    serde_json::from_str(data).unwrap_or_else(|e| panic!("bad SSE data: {e}\nframe: {data}"))
}

async fn mcp_call(app: &Router, user: &TestUser, body: Value, session_id: Option<&str>) -> (Value, Option<String>) {
    let res = app.clone().oneshot(mcp_request(user, body, session_id)).await.unwrap();
    let session = res
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let status = res.status();
    let json = mcp_response_json(res).await;
    assert_eq!(status, StatusCode::OK, "MCP request failed: {json}");
    (json, session)
}

#[tokio::test]
async fn mcp_initialize_list_and_call_tools() {
    let app = app().await;
    let user = TestUser::new("mcp");

    // 1. initialize
    let (init, session) = mcp_call(
        &app,
        &user,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "it-test", "version": "0.1.0"}
            }
        }),
        None,
    )
    .await;
    assert_eq!(init["result"]["serverInfo"]["name"], "htknow");
    let session = session.expect("legacy session mode should issue a session id");

    // 2. initialized notification
    let res = app
        .clone()
        .oneshot(mcp_request(
            &user,
            serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            Some(&session),
        ))
        .await
        .unwrap();
    assert!(res.status().is_success() || res.status() == StatusCode::ACCEPTED);

    // 3. tools/list：六个只读工具全部在册
    let (list, _) = mcp_call(
        &app,
        &user,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        Some(&session),
    )
    .await;
    let tools: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in ["search_knowledge", "list_knowledge_bases", "get_file_detail", "wiki_search", "wiki_get_page", "graph_query"] {
        assert!(tools.contains(&expected), "missing tool {expected} in {tools:?}");
    }

    // 4. tools/call list_knowledge_bases：structuredContent 与文本内容都要有
    let (call, _) = mcp_call(
        &app,
        &user,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "list_knowledge_bases", "arguments": {}}
        }),
        Some(&session),
    )
    .await;
    let result = &call["result"];
    assert_eq!(result["isError"], serde_json::json!(false));
    assert!(result["structuredContent"]["knowledge_bases"].is_array(), "missing structured content: {result}");
    assert!(result["content"][0]["text"].as_str().unwrap().contains("可见知识库"));

    // 5. 参数校验：空 query 应返回 invalid_params
    let (bad, _) = mcp_call(
        &app,
        &user,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {"name": "search_knowledge", "arguments": {"query": "  "}}
        }),
        Some(&session),
    )
    .await;
    assert!(bad.get("error").is_some() || bad["result"]["isError"] == true, "empty query should fail: {bad}");
}

/// MCP Apps：ui:// 资源的 list/read，以及 tool `_meta` 的 ui/resourceUri 关联。
#[tokio::test]
async fn mcp_list_and_read_ui_resource() {
    let app = app().await;
    let user = TestUser::new("mcp-res");

    let (init, session) = mcp_call(
        &app,
        &user,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "it-test", "version": "0.1.0"}
            }
        }),
        None,
    )
    .await;
    assert!(init["result"]["capabilities"]["resources"].is_object(), "resources capability: {init}");
    let session = session.expect("legacy session mode should issue a session id");

    let res = app
        .clone()
        .oneshot(mcp_request(
            &user,
            serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            Some(&session),
        ))
        .await
        .unwrap();
    assert!(res.status().is_success() || res.status() == StatusCode::ACCEPTED);

    // resources/list：知识空间 ui:// 资源在册
    let (list, _) = mcp_call(
        &app,
        &user,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "resources/list"}),
        Some(&session),
    )
    .await;
    let resources = list["result"]["resources"].as_array().unwrap();
    let resource = resources
        .iter()
        .find(|r| r["uri"] == "ui://htknow/knowledge-space")
        .unwrap_or_else(|| panic!("ui resource missing: {resources:?}"));
    assert_eq!(resource["mimeType"], "text/html");

    // tools/list 里 search_knowledge 应带 ui/resourceUri 指向该资源
    let (tools, _) = mcp_call(
        &app,
        &user,
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}),
        Some(&session),
    )
    .await;
    let search = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "search_knowledge")
        .unwrap();
    assert_eq!(search["_meta"]["ui/resourceUri"], "ui://htknow/knowledge-space", "tool meta: {search}");

    // resources/read：返回嵌入的单文件 HTML
    let (read, _) = mcp_call(
        &app,
        &user,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 4, "method": "resources/read",
            "params": {"uri": "ui://htknow/knowledge-space"}
        }),
        Some(&session),
    )
    .await;
    let contents = read["result"]["contents"].as_array().unwrap();
    assert_eq!(contents[0]["mimeType"], "text/html");
    assert!(contents[0]["text"].as_str().unwrap().contains("<html"), "html content: {read}");

    // 未知 uri：resource_not_found 错误
    let (missing, _) = mcp_call(
        &app,
        &user,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 5, "method": "resources/read",
            "params": {"uri": "ui://htnow/nope"}
        }),
        Some(&session),
    )
    .await;
    assert!(missing.get("error").is_some(), "unknown resource should error: {missing}");
}

#[tokio::test]
async fn mcp_requires_auth_headers() {
    let app = app().await;
    // 缺 x-user-id / x-role：应在 auth 中间件被 401，进不了 MCP 层。
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .body(Body::from(
                    serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                        "protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": {"name": "it-test", "version": "0.1.0"}
                    }})
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
