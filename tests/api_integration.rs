use std::{
    path::PathBuf, sync::OnceLock, time::{SystemTime, UNIX_EPOCH}
};

use axum::{
    Router, body::Body, extract::DefaultBodyLimit, http::{Request, StatusCode, header}, middleware
};
use htknow::{api, auth, db, search::SearchEngine};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

struct TestEnv {
    _data_dir: PathBuf,
}

static TEST_ENV: OnceLock<TestEnv> = OnceLock::new();

fn setup_env() -> &'static TestEnv {
    TEST_ENV.get_or_init(|| {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let base_dir = std::env::temp_dir().join(format!("htknow-it-{}", nonce));
        std::fs::create_dir_all(&base_dir).expect("create test data dir");

        let db_path = base_dir.join("test.sqlite");
        // Tests run in a single process; setting env vars here is scoped and deterministic.
        unsafe {
            std::env::set_var("HTKNOW_DATA_DIR", base_dir.to_string_lossy().as_ref());
            std::env::set_var("DATABASE_URL", format!("sqlite://{}", db_path.display()));
            std::env::set_var("HTKNOW_DB_MAX_CONNECTIONS", "1");
            std::env::set_var("HTKNOW_DB_INIT_DEFAULT_KBS", "false");
            std::env::set_var("HTKNOW_SERVER_UPLOAD_LIMIT_MB", "1");
            std::env::set_var("HTKNOW_SEARCH_LIMIT", "5");
        }

        TestEnv { _data_dir: base_dir }
    })
}

async fn build_app() -> Router {
    setup_env();

    let pool = db::init().await.expect("init db");
    let search_engine = SearchEngine::init().await.with_pool(pool.clone());
    let upload_limit = htknow::config::get().server.upload_limit_mb * 1024 * 1024;

    Router::new()
        .nest("/api/v1/knowledge/", api::app(pool, search_engine))
        .layer(middleware::from_fn(auth))
        .layer(DefaultBodyLimit::max(upload_limit))
}

#[tokio::test]
async fn knowledge_base_flow() {
    let app = build_app().await;

    let unauth_req =
        Request::builder().method("GET").uri("/api/v1/knowledge/knowledge_base/").body(Body::empty()).unwrap();
    let unauth_res = app.clone().oneshot(unauth_req).await.unwrap();
    assert_eq!(unauth_res.status(), StatusCode::UNAUTHORIZED);

    let create_body = serde_json::json!({
        "name": "Integration KB",
        "description": "kb for integration tests",
        "kb_type": "analysis",
        "parent_id": null,
        "is_public": false
    });
    let create_req = Request::builder()
        .method("POST")
        .uri("/api/v1/knowledge/knowledge_base/")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-user-id", "user-1")
        .header("x-user-name", "tester")
        .header("x-role", "admin")
        .body(Body::from(create_body.to_string()))
        .unwrap();
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_res.status(), StatusCode::OK);
    let create_bytes = create_res.into_body().collect().await.unwrap().to_bytes();
    let created: Value = serde_json::from_slice(&create_bytes).unwrap();
    let kb_id = created["id"].as_i64().expect("created kb id");

    let list_req = Request::builder()
        .method("GET")
        .uri("/api/v1/knowledge/knowledge_base/")
        .header("x-user-id", "user-1")
        .header("x-user-name", "tester")
        .header("x-role", "admin")
        .body(Body::empty())
        .unwrap();
    let list_res = app.clone().oneshot(list_req).await.unwrap();
    assert_eq!(list_res.status(), StatusCode::OK);
    let list_bytes = list_res.into_body().collect().await.unwrap().to_bytes();
    let list: Vec<Value> = serde_json::from_slice(&list_bytes).unwrap();
    assert!(list.iter().any(|kb| kb["id"] == kb_id && kb["name"] == "Integration KB"));

    let update_body = serde_json::json!({
        "parent_id": kb_id
    });
    let update_req = Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/knowledge/knowledge_base/{}", kb_id))
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-user-id", "user-1")
        .header("x-user-name", "tester")
        .header("x-role", "admin")
        .body(Body::from(update_body.to_string()))
        .unwrap();
    let update_res = app.clone().oneshot(update_req).await.unwrap();
    assert_eq!(update_res.status(), StatusCode::BAD_REQUEST);
}
