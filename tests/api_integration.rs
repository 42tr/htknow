use std::{
    fs, path::PathBuf, sync::{
        OnceLock, atomic::{AtomicUsize, Ordering}
    }, time::{SystemTime, UNIX_EPOCH}
};

use axum::{
    Router, body::Body, extract::DefaultBodyLimit, http::{Request, StatusCode, header}, middleware, response::Response
};
use htknow::{api, auth, db, search::SearchEngine};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::SqlitePool;
use tokio::sync::OnceCell;
use tower::ServiceExt;

struct TestEnv {
    data_dir: PathBuf,
}

static TEST_ENV: OnceLock<TestEnv> = OnceLock::new();
static APP: OnceCell<Router> = OnceCell::const_new();
static TEST_SEQ: AtomicUsize = AtomicUsize::new(1);

fn next_seq() -> usize {
    TEST_SEQ.fetch_add(1, Ordering::Relaxed)
}

fn setup_env() -> &'static TestEnv {
    TEST_ENV.get_or_init(|| {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let base_dir = std::env::temp_dir().join(format!("htknow-it-{}", nonce));
        std::fs::create_dir_all(&base_dir).expect("create test data dir");

        let db_path = base_dir.join("test.sqlite");
        unsafe {
            std::env::set_var("HTKNOW_DATA_DIR", base_dir.to_string_lossy().as_ref());
            std::env::set_var("DATABASE_URL", format!("sqlite://{}", db_path.display()));
            std::env::set_var("HTKNOW_DB_MAX_CONNECTIONS", "1");
            std::env::set_var("HTKNOW_DB_INIT_DEFAULT_KBS", "false");
            std::env::set_var("HTKNOW_SERVER_UPLOAD_LIMIT_MB", "1");
            std::env::set_var("HTKNOW_SEARCH_LIMIT", "5");
        }

        TestEnv { data_dir: base_dir }
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

async fn app() -> Router {
    APP.get_or_init(|| async { build_app().await }).await.clone()
}

async fn get_pool() -> SqlitePool {
    app().await;
    db::init().await.expect("init db")
}

struct TestUser {
    id: String,
    name: String,
    role: String,
}

impl TestUser {
    fn new(prefix: &str) -> Self {
        let seq = next_seq();
        Self { id: format!("{}-{}", prefix, seq), name: format!("{}-name-{}", prefix, seq), role: "admin".to_string() }
    }
}

fn authed_request(
    method: &str, uri: String, user: &TestUser, body: Body, content_type: Option<String>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    builder = builder.header("x-user-id", &user.id).header("x-user-name", &user.name).header("x-role", &user.role);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder.body(body).unwrap()
}

fn authed_empty_request(method: &str, uri: impl Into<String>, user: &TestUser) -> Request<Body> {
    authed_request(method, uri.into(), user, Body::empty(), None)
}

fn authed_json_request(method: &str, uri: impl Into<String>, user: &TestUser, body: Value) -> Request<Body> {
    authed_request(method, uri.into(), user, Body::from(body.to_string()), Some("application/json".to_string()))
}

fn authed_multipart_request(
    method: &str, uri: impl Into<String>, user: &TestUser, boundary: &str, body: Vec<u8>,
) -> Request<Body> {
    let content_type = format!("multipart/form-data; boundary={}", boundary);
    authed_request(method, uri.into(), user, Body::from(body), Some(content_type))
}

async fn response_json(res: Response) -> Value {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn insert_kb(
    pool: &SqlitePool, user: &TestUser, name: &str, kb_type: &str, parent_id: Option<i64>, is_public: bool,
) -> i64 {
    let is_public = if is_public { 1 } else { 0 };
    sqlx::query(
        "INSERT INTO knowledge_bases (user_id, user_name, name, description, kb_type, parent_id, is_public) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&user.id)
    .bind(&user.name)
    .bind(name)
    .bind("test knowledge base")
    .bind(kb_type)
    .bind(parent_id)
    .bind(is_public)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

async fn insert_file(
    pool: &SqlitePool, user: &TestUser, filename: &str, path: &PathBuf, kb_id: Option<i64>, tags: Vec<String>,
    is_public: bool,
) -> i64 {
    let is_public = if is_public { 1 } else { 0 };
    let tags_json = serde_json::to_string(&tags).unwrap();
    let hash = format!("hash-{}", next_seq());
    sqlx::query(
        "INSERT INTO files (user_id, user_name, hash, filename, path, slice_type, kb_id, is_public, tags, status, log) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&user.id)
    .bind(&user.name)
    .bind(hash)
    .bind(filename)
    .bind(path.to_string_lossy().as_ref())
    .bind("text")
    .bind(kb_id)
    .bind(is_public)
    .bind(tags_json)
    .bind(0)
    .bind("")
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

async fn insert_slice(pool: &SqlitePool, file_id: i64, content: &str) -> i64 {
    sqlx::query("INSERT INTO slices (file_id, content) VALUES (?, ?)")
        .bind(file_id)
        .bind(content)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

async fn insert_slice_position(pool: &SqlitePool, slice_id: i64, page_idx: i32, bbox: [i32; 4]) {
    sqlx::query("INSERT INTO slice_positions (slice_id, page_idx, x1, y1, x2, y2) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(slice_id)
        .bind(page_idx)
        .bind(bbox[0])
        .bind(bbox[1])
        .bind(bbox[2])
        .bind(bbox[3])
        .execute(pool)
        .await
        .unwrap();
}

async fn ensure_pdf_images_table(pool: &SqlitePool) {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS pdf_images (\
            id INTEGER PRIMARY KEY AUTOINCREMENT,\
            file_id INTEGER NOT NULL,\
            filename TEXT NOT NULL,\
            path TEXT NOT NULL,\
            page_num INTEGER,\
            created_at INTEGER DEFAULT (strftime('%s','now')),\
            updated_at INTEGER DEFAULT (strftime('%s','now'))\
        )",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_pdf_image(
    pool: &SqlitePool, file_id: i64, filename: &str, path: &PathBuf, page_num: Option<i64>,
) -> i64 {
    sqlx::query("INSERT INTO pdf_images (file_id, filename, path, page_num) VALUES (?, ?, ?, ?)")
        .bind(file_id)
        .bind(filename)
        .bind(path.to_string_lossy().as_ref())
        .bind(page_num)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

async fn insert_graph_node(
    pool: &SqlitePool, name: &str, entity_type: &str, properties: Option<Value>, file_id: Option<i64>,
    kb_id: Option<i64>,
) -> i64 {
    let props = properties.map(|value| value.to_string());
    sqlx::query(
        "INSERT INTO graph_nodes (name, entity_type, properties, file_id, kb_id, is_public) \
         VALUES (?, ?, ?, ?, ?, 0)",
    )
    .bind(name)
    .bind(entity_type)
    .bind(props)
    .bind(file_id)
    .bind(kb_id)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

async fn insert_graph_edge(
    pool: &SqlitePool, source_node_id: i64, target_node_id: i64, relation_type: &str, file_id: Option<i64>,
) -> i64 {
    sqlx::query("INSERT INTO graph_edges (source_node_id, target_node_id, relation_type, file_id) VALUES (?, ?, ?, ?)")
        .bind(source_node_id)
        .bind(target_node_id)
        .bind(relation_type)
        .bind(file_id)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

async fn insert_entity_mention(pool: &SqlitePool, node_id: i64, slice_id: i64, context: &str) -> i64 {
    sqlx::query("INSERT INTO entity_mentions (node_id, slice_id, context) VALUES (?, ?, ?)")
        .bind(node_id)
        .bind(slice_id)
        .bind(context)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

fn multipart_body(boundary: &str, fields: &[(&str, &str)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{}\"\r\n\r\n", name).as_bytes());
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());
    body
}

#[tokio::test]
async fn knowledge_base_flow() {
    let app = app().await;
    let user = TestUser::new("kb-flow");

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
    let create_req = authed_json_request("POST", "/api/v1/knowledge/knowledge_base/", &user, create_body);
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_res.status(), StatusCode::OK);
    let created = response_json(create_res).await;
    let kb_id = created["id"].as_i64().expect("created kb id");

    let list_req = authed_empty_request("GET", "/api/v1/knowledge/knowledge_base/", &user);
    let list_res = app.clone().oneshot(list_req).await.unwrap();
    assert_eq!(list_res.status(), StatusCode::OK);
    let list_bytes = list_res.into_body().collect().await.unwrap().to_bytes();
    let list: Vec<Value> = serde_json::from_slice(&list_bytes).unwrap();
    assert!(list.iter().any(|kb| kb["id"].as_i64() == Some(kb_id) && kb["name"].as_str() == Some("Integration KB")));

    let update_body = serde_json::json!({
        "parent_id": kb_id
    });
    let update_req =
        authed_json_request("PUT", format!("/api/v1/knowledge/knowledge_base/{}", kb_id), &user, update_body);
    let update_res = app.clone().oneshot(update_req).await.unwrap();
    assert_eq!(update_res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn knowledge_base_public_and_reparse() {
    let app = app().await;
    let user = TestUser::new("kb-public");

    let create_body = serde_json::json!({
        "name": "Public KB",
        "description": "kb for public tests",
        "kb_type": "analysis",
        "parent_id": null,
        "is_public": false
    });
    let create_req = authed_json_request("POST", "/api/v1/knowledge/knowledge_base/", &user, create_body);
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_res.status(), StatusCode::OK);
    let created = response_json(create_res).await;
    let kb_id = created["id"].as_i64().expect("created kb id");

    let public_req = authed_json_request(
        "PUT",
        format!("/api/v1/knowledge/knowledge_base/{}/public", kb_id),
        &user,
        serde_json::json!({ "is_public": true }),
    );
    let public_res = app.clone().oneshot(public_req).await.unwrap();
    assert_eq!(public_res.status(), StatusCode::OK);
    let public_json = response_json(public_res).await;
    assert_eq!(public_json["is_public"].as_i64(), Some(1));

    let reparse_req = authed_empty_request("POST", "/api/v1/knowledge/knowledge_base/reparse", &user);
    let reparse_res = app.clone().oneshot(reparse_req).await.unwrap();
    assert_eq!(reparse_res.status(), StatusCode::OK);
    let reparse_json = response_json(reparse_res).await;
    assert_eq!(reparse_json["kb_count"].as_i64(), Some(1));
    assert_eq!(reparse_json["file_count"].as_i64(), Some(0));
}

#[tokio::test]
async fn file_endpoints_flow() {
    let app = app().await;
    let pool = get_pool().await;
    let env = setup_env();
    let user = TestUser::new("file");

    ensure_pdf_images_table(&pool).await;

    let kb_id = insert_kb(&pool, &user, "File KB", "analysis", None, false).await;

    let file_suffix = next_seq();
    let file_dir = env.data_dir.join("files");
    fs::create_dir_all(&file_dir).unwrap();
    let file_path = file_dir.join(format!("file-{}.txt", file_suffix));
    let file_contents = b"file contents";
    fs::write(&file_path, file_contents).unwrap();

    let file_id = insert_file(
        &pool,
        &user,
        "file.txt",
        &file_path,
        Some(kb_id),
        vec!["tag1".to_string(), "tag2".to_string()],
        false,
    )
    .await;

    let slice_id = insert_slice(&pool, file_id, "slice content").await;
    insert_slice_position(&pool, slice_id, 1, [1, 2, 3, 4]).await;

    let image_dir = env.data_dir.join("pdf_images");
    fs::create_dir_all(&image_dir).unwrap();
    let image_path = image_dir.join(format!("image-{}.png", file_suffix));
    fs::write(&image_path, b"png").unwrap();
    let image_id = insert_pdf_image(&pool, file_id, "image.png", &image_path, Some(1)).await;

    let list_req = authed_empty_request("GET", "/api/v1/knowledge/files/", &user);
    let list_res = app.clone().oneshot(list_req).await.unwrap();
    assert_eq!(list_res.status(), StatusCode::OK);
    let list_json = response_json(list_res).await;
    let list = list_json.as_array().expect("file list");
    assert!(list.iter().any(|f| f["id"].as_i64() == Some(file_id)));

    let list_tag_req = authed_empty_request("GET", "/api/v1/knowledge/files/?tag=tag1", &user);
    let list_tag_res = app.clone().oneshot(list_tag_req).await.unwrap();
    assert_eq!(list_tag_res.status(), StatusCode::OK);
    let list_tag_json = response_json(list_tag_res).await;
    let list_tag = list_tag_json.as_array().expect("file list");
    assert!(list_tag.iter().any(|f| f["id"].as_i64() == Some(file_id)));

    let list_kb_req = authed_empty_request("GET", format!("/api/v1/knowledge/files/?kb_id={}", kb_id), &user);
    let list_kb_res = app.clone().oneshot(list_kb_req).await.unwrap();
    assert_eq!(list_kb_res.status(), StatusCode::OK);
    let list_kb_json = response_json(list_kb_res).await;
    let list_kb = list_kb_json.as_array().expect("file list");
    assert!(list_kb.iter().any(|f| f["id"].as_i64() == Some(file_id)));

    let get_req = authed_empty_request("GET", format!("/api/v1/knowledge/files/{}", file_id), &user);
    let get_res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let get_json = response_json(get_res).await;
    assert_eq!(get_json["id"].as_i64(), Some(file_id));

    let update_body = serde_json::json!({
        "filename": "renamed.txt",
        "tags": ["tag3"]
    });
    let update_req = authed_json_request("PUT", format!("/api/v1/knowledge/files/{}", file_id), &user, update_body);
    let update_res = app.clone().oneshot(update_req).await.unwrap();
    assert_eq!(update_res.status(), StatusCode::OK);
    let update_json = response_json(update_res).await;
    assert_eq!(update_json["filename"].as_str(), Some("renamed.txt"));
    let updated_tags: Vec<String> = serde_json::from_str(update_json["tags"].as_str().unwrap()).unwrap();
    assert_eq!(updated_tags, vec!["tag3".to_string()]);

    let content_req = authed_empty_request("GET", format!("/api/v1/knowledge/files/{}/content", file_id), &user);
    let content_res = app.clone().oneshot(content_req).await.unwrap();
    assert_eq!(content_res.status(), StatusCode::OK);
    let content_type = content_res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(content_type.contains("text/plain"));
    let content_bytes = content_res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(content_bytes.as_ref(), file_contents);

    let download_req = authed_empty_request("GET", format!("/api/v1/knowledge/files/{}/download", file_id), &user);
    let download_res = app.clone().oneshot(download_req).await.unwrap();
    assert_eq!(download_res.status(), StatusCode::OK);
    let disposition =
        download_res.headers().get(header::CONTENT_DISPOSITION).and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(disposition.contains("renamed.txt"));
    let download_bytes = download_res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(download_bytes.as_ref(), file_contents);

    let slices_req = authed_empty_request("GET", format!("/api/v1/knowledge/files/{}/slices", file_id), &user);
    let slices_res = app.clone().oneshot(slices_req).await.unwrap();
    assert_eq!(slices_res.status(), StatusCode::OK);
    let slices_json = response_json(slices_res).await;
    let slices = slices_json.as_array().expect("slice list");
    let slice = slices.iter().find(|s| s["id"].as_i64() == Some(slice_id)).expect("slice entry");
    let positions = slice["positions"].as_array().expect("slice positions");
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0]["page_idx"].as_i64(), Some(1));
    assert_eq!(positions[0]["bbox"].as_array().unwrap().len(), 4);

    let images_req = authed_empty_request("GET", format!("/api/v1/knowledge/files/{}/images", file_id), &user);
    let images_res = app.clone().oneshot(images_req).await.unwrap();
    assert_eq!(images_res.status(), StatusCode::OK);
    let images_json = response_json(images_res).await;
    let images = images_json.as_array().expect("image list");
    assert!(images.iter().any(|image| image["id"].as_i64() == Some(image_id)));

    let image_req =
        authed_empty_request("GET", format!("/api/v1/knowledge/files/{}/images/{}", file_id, image_id), &user);
    let image_res = app.clone().oneshot(image_req).await.unwrap();
    assert_eq!(image_res.status(), StatusCode::OK);
    let image_type = image_res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(image_type.contains("image/png"));
    let image_bytes = image_res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(image_bytes.as_ref(), b"png");

    let delete_req = authed_empty_request("DELETE", format!("/api/v1/knowledge/files/{}", file_id), &user);
    let delete_res = app.clone().oneshot(delete_req).await.unwrap();
    assert_eq!(delete_res.status(), StatusCode::OK);
    assert!(!file_path.exists());
    assert!(!image_path.exists());
}

#[tokio::test]
async fn graph_endpoints_flow() {
    let app = app().await;
    let pool = get_pool().await;
    let env = setup_env();
    let user = TestUser::new("graph");

    let kb_id = insert_kb(&pool, &user, "Graph KB", "analysis", None, false).await;

    let file_suffix = next_seq();
    let file_dir = env.data_dir.join("files");
    fs::create_dir_all(&file_dir).unwrap();
    let file_path = file_dir.join(format!("graph-{}.txt", file_suffix));
    fs::write(&file_path, b"graph file").unwrap();

    let file_id = insert_file(&pool, &user, "graph.txt", &file_path, Some(kb_id), Vec::new(), false).await;
    let slice_id = insert_slice(&pool, file_id, "mention context").await;

    let node_a_id = insert_graph_node(
        &pool,
        "Alpha",
        "device",
        Some(serde_json::json!({ "origin": "test" })),
        Some(file_id),
        Some(kb_id),
    )
    .await;
    let node_b_id = insert_graph_node(&pool, "Beta", "component", None, Some(file_id), Some(kb_id)).await;
    insert_graph_edge(&pool, node_a_id, node_b_id, "related_to", Some(file_id)).await;
    insert_entity_mention(&pool, node_a_id, slice_id, "Alpha mention").await;

    let search_req =
        authed_empty_request("GET", format!("/api/v1/knowledge/graph/entities?kb_id={}&q=Alpha", kb_id), &user);
    let search_res = app.clone().oneshot(search_req).await.unwrap();
    assert_eq!(search_res.status(), StatusCode::OK);
    let search_json = response_json(search_res).await;
    let search_list = search_json.as_array().expect("entity list");
    assert!(search_list.iter().any(|entity| entity["id"].as_i64() == Some(node_a_id)));

    let entity_req = authed_empty_request("GET", format!("/api/v1/knowledge/graph/entities/{}", node_a_id), &user);
    let entity_res = app.clone().oneshot(entity_req).await.unwrap();
    assert_eq!(entity_res.status(), StatusCode::OK);
    let entity_json = response_json(entity_res).await;
    assert_eq!(entity_json["entity"]["id"].as_i64(), Some(node_a_id));
    let neighbors = entity_json["neighbors"].as_array().expect("neighbors");
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0]["entity"]["id"].as_i64(), Some(node_b_id));
    assert_eq!(neighbors[0]["direction"].as_str(), Some("outgoing"));
    let mentions = entity_json["mentions"].as_array().expect("mentions");
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0]["file_id"].as_i64(), Some(file_id));

    let stats_req = authed_empty_request("GET", format!("/api/v1/knowledge/graph/stats?kb_id={}", kb_id), &user);
    let stats_res = app.clone().oneshot(stats_req).await.unwrap();
    assert_eq!(stats_res.status(), StatusCode::OK);
    let stats_json = response_json(stats_res).await;
    assert_eq!(stats_json["node_count"].as_i64(), Some(2));
    assert_eq!(stats_json["edge_count"].as_i64(), Some(1));
    let entity_types = stats_json["entity_types"].as_object().expect("entity types");
    assert_eq!(entity_types.get("device").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(entity_types.get("component").and_then(|v| v.as_i64()), Some(1));
    let relation_types = stats_json["relation_types"].as_object().expect("relation types");
    assert_eq!(relation_types.get("related_to").and_then(|v| v.as_i64()), Some(1));
}

#[tokio::test]
async fn search_full_empty_and_image_requires_file() {
    let app = app().await;
    let user = TestUser::new("search");

    let full_req = authed_empty_request("GET", "/api/v1/knowledge/search/full?query=missing", &user);
    let full_res = app.clone().oneshot(full_req).await.unwrap();
    assert_eq!(full_res.status(), StatusCode::OK);
    let full_json = response_json(full_res).await;
    assert_eq!(full_json["results"].as_array().map(|v| v.len()), Some(0));

    let boundary = format!("boundary-{}", next_seq());
    let body = multipart_body(&boundary, &[("text", "sample")]);
    let image_req = authed_multipart_request("POST", "/api/v1/knowledge/search/image", &user, &boundary, body);
    let image_res = app.clone().oneshot(image_req).await.unwrap();
    assert_eq!(image_res.status(), StatusCode::BAD_REQUEST);
}
