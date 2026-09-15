mod common;

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use axum::{Json, Router, middleware, routing::post};
use common::*;
use htknow::{api, auth, db, search::SearchEngine};
use serde_json::json;

#[tokio::test]
async fn wiki_search_lifecycle_permissions_and_vectors() {
    setup_env();
    let requests = Arc::new(AtomicUsize::new(0));
    let fail_embedding = Arc::new(AtomicBool::new(false));
    let counter = requests.clone();
    let failure = fail_embedding.clone();
    let mock = Router::new().route(
        "/embeddings",
        post(move |Json(body): Json<Value>| {
            let counter = counter.clone();
            let failure = failure.clone();
            async move {
                counter.fetch_add(1, Ordering::Relaxed);
                if failure.load(Ordering::Relaxed) {
                    return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":"temporarily unavailable"})));
                }
                let data: Vec<_> = body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .map(|(index, input)| {
                        let text = input.as_str().unwrap();
                        let vector = if text.contains("orbital") || text.contains("cosmic") {
                            vec![1.0, 0.0]
                        } else {
                            vec![0.0, 1.0]
                        };
                        json!({"index":index,"embedding":vector})
                    })
                    .collect();
                (StatusCode::OK, Json(json!({"data":data})))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/embeddings", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });
    unsafe {
        std::env::set_var("HTKNOW_EMBEDDING_URL", endpoint);
        std::env::set_var("HTKNOW_EMBEDDING_DIM", "2");
        std::env::set_var("HTKNOW_RERANK_URL", "http://127.0.0.1:9/rerank");
        std::env::set_var("LLM_API_URL", "");
    }
    let pool = db::init().await.unwrap();
    let engine = SearchEngine::init().await.with_pool(pool.clone());
    let app = Router::new()
        .nest("/api/v1/knowledge/", api::app(pool.clone(), engine.clone()))
        .layer(middleware::from_fn(auth));
    let owner = TestUser::with_role("wiki-search-owner", "user");
    let outsider = TestUser::with_role("wiki-search-outsider", "user");
    let kb = insert_kb(&pool, &owner, "Wiki private search", "analysis", None, false).await;
    let public_kb = insert_kb(&pool, &owner, "Wiki public search", "analysis", None, true).await;
    let create = |kb_id, slug: &str, content: &str| {
        authed_json_request(
            "POST",
            "/api/v1/knowledge/wiki/page",
            &owner,
            json!({"kb_id":kb_id,"title":"Notes","slug":slug,"summary":"Summary","content":content}),
        )
    };
    let created = app.clone().oneshot(create(kb, "orbit", "orbital propulsion bodyonlytoken")).await.unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let page_id = response_json(created).await["page"]["id"].as_i64().unwrap();
    let uri = format!("/api/v1/knowledge/wiki/search?kb_id={kb}&q=bodyonlytoken");
    let result = response_json(app.clone().oneshot(authed_empty_request("GET", &uri, &owner)).await.unwrap()).await;
    assert_eq!(result["items"][0]["id"], page_id, "body text must be searchable before vector indexing: {result}");
    assert_eq!(requests.load(Ordering::Relaxed), 0, "lexical search must not need embedding initialization");
    let res = app.clone().oneshot(authed_empty_request("GET", &uri, &outsider)).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let source_path = setup_env().data_dir.join("wiki-source.txt");
    std::fs::write(&source_path, "orbital source").unwrap();
    let source_id = insert_file(&pool, &owner, "source.txt", &source_path, Some(kb), vec![], false).await;
    let slice_id = insert_slice(&pool, source_id, "orbital evidence").await;
    htknow::wiki::page::add_sources(&pool, page_id, &[source_id]).await.unwrap();
    htknow::wiki::page::add_slice_refs(&pool, page_id, &[slice_id]).await.unwrap();
    assert_eq!(
        engine.search_wiki_scoped("orbital", Some(&vec![source_id]), Some(&vec![kb])).await.unwrap()[0].0.id,
        page_id
    );
    assert!(
        engine.search_wiki_scoped("orbital", Some(&vec![source_id + 100]), Some(&vec![kb])).await.unwrap().is_empty()
    );
    assert!(engine.search_wiki_scoped("orbital", Some(&vec![]), Some(&vec![kb])).await.unwrap().is_empty());

    engine.sync_wiki_indexes().await.unwrap();
    let before = requests.load(Ordering::Relaxed);
    engine.sync_wiki_indexes().await.unwrap();
    assert_eq!(requests.load(Ordering::Relaxed), before, "unchanged pages reuse vectors");
    let semantic = engine.search_wiki("cosmic", Some(&vec![kb])).await.unwrap();
    assert_eq!(semantic[0].0.id, page_id, "vector-only recall should find the page");
    assert!(engine.search_wiki("orbital", Some(&vec![])).await.unwrap().is_empty());

    engine
        .write(
            htknow::search::tantivy_engine::Document::new(slice_id, source_id, Some(kb), "orbital evidence".into()),
            None,
        )
        .await
        .unwrap();
    for path in ["search/", "search/graph", "search/?advanced=true&"] {
        let uri = if path.ends_with('&') {
            format!("/api/v1/knowledge/{path}query=orbital")
        } else {
            format!("/api/v1/knowledge/{path}?query=orbital")
        };
        let res = app.clone().oneshot(authed_empty_request("GET", &uri, &owner)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{uri}");
        let body = response_json(res).await;
        assert_eq!(body["results"][0]["wiki"]["page"]["id"], page_id, "{uri}: {body}");
        assert!(body["results"][0]["file"].is_null());
        assert!(
            body["results"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["file_id"] == source_id && item.get("wiki").is_none()),
            "slice results must coexist with Wiki even when IDs overlap: {body}"
        );
        assert_eq!(body["results"][0]["wiki"]["slices"][0]["slice_id"], slice_id);
        assert_eq!(body["results"][0]["wiki"]["slices"][0]["file_id"], source_id);
        let res = app.clone().oneshot(authed_empty_request("GET", &uri, &outsider)).await.unwrap();
        assert!(response_json(res).await["results"].as_array().unwrap().is_empty());
    }
    let sse = app
        .clone()
        .oneshot(authed_empty_request("GET", "/api/v1/knowledge/search/advanced/stream?query=orbital", &owner))
        .await
        .unwrap();
    assert_eq!(sse.status(), StatusCode::OK);
    let sse = String::from_utf8(sse.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(sse.contains("event: result") && sse.contains("\"wiki\""), "{sse}");

    let update = |payload| authed_json_request("PUT", "/api/v1/knowledge/wiki/page", &owner, payload);
    let res = app
        .clone()
        .oneshot(update(json!({"kb_id":kb,"slug":"concept/orbit","content":"replacementonlytoken"})))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let stale = engine.search_wiki("cosmic", Some(&vec![kb])).await.unwrap();
    assert!(stale.is_empty(), "old vector must be rejected immediately after an edit");
    fail_embedding.store(true, Ordering::Relaxed);
    assert_eq!(engine.search_wiki("replacementonlytoken", Some(&vec![kb])).await.unwrap()[0].0.id, page_id);
    assert!(engine.sync_wiki_indexes().await.is_err(), "failed vector writes remain retryable");
    fail_embedding.store(false, Ordering::Relaxed);
    engine.sync_wiki_indexes().await.unwrap();
    for status in ["archived", "draft", "published"] {
        let res =
            app.clone().oneshot(update(json!({"kb_id":kb,"slug":"concept/orbit","status":status}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let hits = engine.search_wiki("replacementonlytoken", Some(&vec![kb])).await.unwrap();
        assert_eq!(hits.len(), usize::from(status == "published"));
        engine.sync_wiki_indexes().await.unwrap();
    }
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "DELETE",
            "/api/v1/knowledge/wiki/page",
            &owner,
            json!({"kb_id": kb, "slug": "concept/orbit"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(engine.search_wiki("replacementonlytoken", Some(&vec![kb])).await.unwrap().is_empty());
    engine.sync_wiki_indexes().await.unwrap();
    let res = app.clone().oneshot(create(public_kb, "public", "orbital public information")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = app
        .clone()
        .oneshot(authed_empty_request("GET", "/api/v1/knowledge/search/?query=orbital", &outsider))
        .await
        .unwrap();
    let body = response_json(res).await;
    assert_eq!(body["results"][0]["wiki"]["page"]["kb_id"], public_kb);
    for i in 0..6 {
        let res = app.clone().oneshot(create(kb, &format!("limit-{i}"), "limitonlytoken")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }
    let res = app
        .clone()
        .oneshot(authed_empty_request(
            "GET",
            format!("/api/v1/knowledge/wiki/search?kb_id={kb}&q=limitonlytoken&limit=6"),
            &owner,
        ))
        .await
        .unwrap();
    let body = response_json(res).await;
    assert_eq!(
        body["items"].as_array().unwrap().len(),
        6,
        "Wiki endpoint limit must override global search limit: {body}"
    );
    server.abort();
}
