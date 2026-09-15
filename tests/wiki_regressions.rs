mod common;

use axum::{Json, Router, middleware, routing::post};
use common::*;
use htknow::{
    api, auth, db,
    search::SearchEngine,
    wiki::{edit, ingest, page, prompts, queue, revision},
};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[tokio::test]
async fn wiki_move_edit_and_partial_generation_regressions() {
    setup_env();
    let fail = Arc::new(AtomicBool::new(true));
    let failure = fail.clone();
    let mock = Router::new().route(
        "/chat",
        post(move |Json(body): Json<Value>| {
            let fail = failure.clone();
            async move {
                let system = body["messages"][0]["content"].as_str().unwrap();
                let content = if system == prompts::CANDIDATE_EXTRACT_SYSTEM {
                    json!({"concepts":[{"name":"Retry topic", "slug":"concept/retry-topic", "description":"retry"}]})
                        .to_string()
                } else if system == prompts::CHUNK_CITATION_SYSTEM {
                    json!({"citations":{}}).to_string()
                } else if system == prompts::PAGE_BODY_SYSTEM {
                    if fail.load(Ordering::Relaxed) {
                        return (StatusCode::BAD_REQUEST, Json(json!({"error":"candidate failure"})));
                    }
                    "Recovered candidate body".to_string()
                } else {
                    "Summary\n\nGenerated document summary".to_string()
                };
                (StatusCode::OK, Json(json!({"choices":[{"message":{"content":content}}]})))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    unsafe {
        std::env::set_var("WIKI_LLM_API_URL", format!("http://{}/chat", listener.local_addr().unwrap()));
        std::env::set_var("HTKNOW_EMBEDDING_URL", "http://127.0.0.1:9/embeddings");
    }
    let server = tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });
    let pool = db::init().await.unwrap();
    let engine = SearchEngine::init().await.with_pool(pool.clone());
    let app = Router::new()
        .nest("/api/v1/knowledge/", api::app(pool.clone(), engine.clone()))
        .layer(middleware::from_fn(auth));
    let owner = TestUser::with_role("wiki-fix-owner", "user");
    let outsider = TestUser::with_role("wiki-fix-outsider", "user");
    let public_kb = insert_kb(&pool, &owner, "public", "analysis", None, true).await;
    let private_kb = insert_kb(&pool, &owner, "private", "analysis", None, false).await;
    let draft = page::PageDraft {
        kb_id: public_kb,
        slug: "concept/race".into(),
        title: "Old title".into(),
        page_type: "concept".into(),
        summary: "summary".into(),
        content: "old text with Target".into(),
        aliases: vec![],
        edit_source: "pipeline".into(),
        editor_id: String::new(),
    };
    let created = page::upsert(&pool, &draft, &[], &[]).await.unwrap();
    let stale = page::get_by_id(&pool, created.page_id).await.unwrap().unwrap();
    let edited = edit::apply_user_edit(
        &pool,
        public_kb,
        &draft.slug,
        &edit::PageEdit {
            content: Some("new user text".into()),
            title: Some("New title".into()),
            editor_id: owner.id.clone(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let revisions = revision::count(&pool, created.page_id).await.unwrap();
    assert!(
        !page::update_content(&pool, &stale, "old [[concept/target|Target]]", &["concept/target".into()], None)
            .await
            .unwrap()
    );
    let actual = page::get_by_id(&pool, created.page_id).await.unwrap().unwrap();
    assert_eq!(actual.content, edited.content);
    assert_eq!(actual.title, edited.title);
    assert_eq!(actual.version, edited.version);
    assert_eq!(revision::count(&pool, created.page_id).await.unwrap(), revisions);
    assert!(
        page::update_content(
            &pool,
            &actual,
            "new user text [[concept/target|Target]]",
            &["concept/target".into()],
            None
        )
        .await
        .unwrap()
    );
    assert_eq!(page::get_by_id(&pool, created.page_id).await.unwrap().unwrap().last_edit_source, "user");

    let path = setup_env().data_dir.join("move.txt");
    std::fs::write(&path, "source information").unwrap();
    let file_id = insert_file(&pool, &owner, "move.txt", &path, Some(public_kb), vec![], false).await;
    let remaining_id = insert_file(&pool, &owner, "remaining.txt", &path, Some(public_kb), vec![], false).await;
    sqlx::query("UPDATE files SET status = 1 WHERE id = ?").bind(remaining_id).execute(&pool).await.unwrap();
    let moved_draft = page::PageDraft {
        slug: format!("summary/{file_id}"),
        page_type: "summary".into(),
        content: "withdrawsecrettoken".into(),
        ..draft.clone()
    };
    let moved_page = page::upsert(&pool, &moved_draft, &[file_id, remaining_id], &[]).await.unwrap();
    let manual = edit::apply_user_edit(
        &pool,
        public_kb,
        &moved_draft.slug,
        &edit::PageEdit {
            content: Some("withdrawsecrettoken manual changes".into()),
            editor_id: owner.id.clone(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let index = page::upsert(
        &pool,
        &page::PageDraft {
            slug: "index".into(),
            page_type: "index".into(),
            content: "withdrawsecrettoken directory".into(),
            ..draft.clone()
        },
        &[],
        &[],
    )
    .await
    .unwrap();
    assert_eq!(engine.search_wiki("withdrawsecrettoken", Some(&vec![public_kb])).await.unwrap().len(), 1);
    let response = app
        .clone()
        .oneshot(authed_json_request(
            "PUT",
            format!("/api/v1/knowledge/files/{file_id}/move"),
            &owner,
            json!({"target_kb_id":private_kb}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    for endpoint in ["page", "revisions", "revision"] {
        for slug in [
            moved_draft.slug.clone(),
            format!("__withdrawn/{}/{}", moved_page.page_id, moved_draft.slug),
            "index".into(),
        ] {
            let response = app
                .clone()
                .oneshot(authed_empty_request(
                    "GET",
                    format!("/api/v1/knowledge/wiki/{endpoint}?kb_id={public_kb}&slug={slug}&version=1"),
                    &outsider,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{endpoint} {slug}");
        }
    }
    for endpoint in ["pages", "index", "graph", "lint"] {
        let response = app
            .clone()
            .oneshot(authed_empty_request(
                "GET",
                format!("/api/v1/knowledge/wiki/{endpoint}?kb_id={public_kb}"),
                &outsider,
            ))
            .await
            .unwrap();
        assert!(!response_json(response).await.to_string().contains("withdrawsecrettoken"), "{endpoint}");
    }
    assert!(engine.search_wiki("withdrawsecrettoken", Some(&vec![public_kb])).await.unwrap().is_empty());
    assert!(page::get_by_id(&pool, moved_page.page_id).await.unwrap().is_none());
    assert!(page::get_by_id(&pool, index.page_id).await.unwrap().is_none());
    let saved: (String, String) = sqlx::query_as("SELECT status, content FROM wiki_pages WHERE id = ?")
        .bind(moved_page.page_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(saved, ("withdrawn".into(), manual.content.clone()));
    assert!(revision::count(&pool, moved_page.page_id).await.unwrap() > 0, "manual history is retained");
    assert!(page::upsert(&pool, &moved_draft, &[file_id], &[]).await.is_err(), "in-flight old-KB generation must fail");
    assert!(!page::update_content(&pool, &manual, "stale generated content", &[], None).await.unwrap());
    let fresh = page::upsert(
        &pool,
        &page::PageDraft { content: "remaining evidence only".into(), ..moved_draft },
        &[remaining_id],
        &[],
    )
    .await
    .unwrap();
    assert_ne!(fresh.page_id, moved_page.page_id);
    assert_eq!(revision::count(&pool, fresh.page_id).await.unwrap(), 0);
    let queued: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM wiki_tasks WHERE task_type = 'wiki:ingest' AND file_id = ?")
            .bind(remaining_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(queued > 0);

    // A successful summary plus a failed candidate must not create a reusable completed build.
    sqlx::query("UPDATE knowledge_bases SET wiki_config = '{\"enabled\":true}' WHERE id = ?")
        .bind(private_kb)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE files SET status = 1 WHERE id = ?").bind(file_id).execute(&pool).await.unwrap();
    insert_slice(&pool, file_id, &"A detailed document about a retry topic and its evidence. ".repeat(10)).await;
    let err = ingest::ingest_file(&pool, private_kb, file_id).await.unwrap_err();
    assert!(err.to_string().contains("candidate pages failed"), "{err:#}");
    let status: String = sqlx::query_scalar("SELECT status FROM wiki_builds WHERE file_id = ?")
        .bind(file_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "failed");
    assert!(page::get_by_slug(&pool, private_kb, &format!("summary/{file_id}")).await.unwrap().is_some());
    queue::enqueue_ingest(&pool, private_kb, file_id).await.unwrap();
    let task = queue::claim_batch(&pool, 100, "retry-test")
        .await
        .unwrap()
        .into_iter()
        .find(|task| task.file_id == Some(file_id))
        .unwrap();
    queue::mark_failed(&pool, &task, &err.to_string()).await.unwrap();
    let retry: (String, i64) = sqlx::query_as("SELECT status, fail_count FROM wiki_tasks WHERE id = ?")
        .bind(task.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(retry, ("pending".into(), 1));
    fail.store(false, Ordering::Relaxed);
    let report = ingest::ingest_file(&pool, private_kb, file_id).await.unwrap();
    assert!(!report.skipped);
    assert_eq!(report.pages_written, 2);
    assert!(page::get_by_slug(&pool, private_kb, "concept/retry-topic").await.unwrap().is_some());
    assert!(ingest::ingest_file(&pool, private_kb, file_id).await.unwrap().skipped);
    server.abort();
}
