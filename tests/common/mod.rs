#![allow(dead_code, unused_imports)]

use std::{
    fs, path::PathBuf, sync::{
        OnceLock, atomic::{AtomicUsize, Ordering}
    }, time::{SystemTime, UNIX_EPOCH}
};

use axum::{Router, extract::DefaultBodyLimit, middleware};
pub use axum::{
    body::Body, http::{Request, Response, StatusCode, header}
};
use htknow::{api, auth, db, search::SearchEngine, slice_content};
pub use http_body_util::BodyExt;
pub use serde_json::Value;
use sqlx::SqlitePool;
use tokio::sync::OnceCell;
pub use tower::ServiceExt;

pub struct TestEnv {
    pub data_dir: PathBuf,
}

static TEST_ENV: OnceLock<TestEnv> = OnceLock::new();
static APP: OnceCell<Router> = OnceCell::const_new();
static TEST_SEQ: AtomicUsize = AtomicUsize::new(1);

pub fn next_seq() -> usize {
    TEST_SEQ.fetch_add(1, Ordering::Relaxed)
}

pub fn setup_env() -> &'static TestEnv {
    TEST_ENV.get_or_init(|| {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let base_dir = std::env::temp_dir().join(format!("htknow-it-{}", nonce));
        std::fs::create_dir_all(&base_dir).expect("create test data dir");

        let db_path = base_dir.join("test.sqlite");
        unsafe {
            std::env::set_var("HTKNOW_DATA_DIR", base_dir.to_string_lossy().as_ref());
            std::env::set_var("DATABASE_URL", format!("sqlite://{}", db_path.display()));
            std::env::set_var("HTKNOW_DB_MAX_CONNECTIONS", "10");
            std::env::set_var("HTKNOW_DB_INIT_DEFAULT_KBS", "false");
            std::env::set_var("HTKNOW_SERVER_UPLOAD_LIMIT_MB", "1");
            std::env::set_var("HTKNOW_SEARCH_LIMIT", "5");
        }

        TestEnv { data_dir: base_dir }
    })
}

pub async fn build_app() -> Router {
    setup_env();

    let pool = db::init().await.expect("init db");
    let search_engine = SearchEngine::init().await.with_pool(pool.clone());
    let upload_limit = htknow::config::get().server.upload_limit_mb * 1024 * 1024;

    Router::new()
        .nest("/api/v1/knowledge/", api::app(pool, search_engine))
        .layer(middleware::from_fn(auth))
        .layer(DefaultBodyLimit::max(upload_limit))
}

pub async fn app() -> Router {
    APP.get_or_init(|| async { build_app().await }).await.clone()
}

pub async fn get_pool() -> SqlitePool {
    let _ = app().await;
    db::init().await.expect("init db")
}

pub struct TestUser {
    pub id: String,
    pub name: String,
    pub role: String,
}

impl TestUser {
    pub fn new(prefix: &str) -> Self {
        Self::with_role(prefix, "admin")
    }

    pub fn with_role(prefix: &str, role: &str) -> Self {
        let seq = next_seq();
        Self { id: format!("{}-{}", prefix, seq), name: format!("{}-name-{}", prefix, seq), role: role.to_string() }
    }
}

pub fn authed_request(
    method: &str, uri: String, user: &TestUser, body: Body, content_type: Option<String>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    builder = builder.header("x-user-id", &user.id).header("x-user-name", &user.name).header("x-role", &user.role);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder.body(body).unwrap()
}

pub fn authed_empty_request(method: &str, uri: impl Into<String>, user: &TestUser) -> Request<Body> {
    authed_request(method, uri.into(), user, Body::empty(), None)
}

pub fn authed_json_request(method: &str, uri: impl Into<String>, user: &TestUser, body: Value) -> Request<Body> {
    authed_request(method, uri.into(), user, Body::from(body.to_string()), Some("application/json".to_string()))
}

pub fn authed_multipart_request(
    method: &str, uri: impl Into<String>, user: &TestUser, boundary: &str, body: Vec<u8>,
) -> Request<Body> {
    let content_type = format!("multipart/form-data; boundary={}", boundary);
    authed_request(method, uri.into(), user, Body::from(body), Some(content_type))
}

pub async fn response_json(res: Response<Body>) -> Value {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    if bytes.is_empty() {
        panic!("response body is empty");
    }
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!("failed to parse JSON response: {}\nbody: {}", e, String::from_utf8_lossy(&bytes));
    })
}

pub async fn insert_kb(
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

pub async fn insert_file(
    pool: &SqlitePool, user: &TestUser, filename: &str, path: &PathBuf, kb_id: Option<i64>, tags: Vec<String>,
    is_public: bool,
) -> i64 {
    let is_public = if is_public { 1 } else { 0 };
    let tags_json = serde_json::to_string(&tags).unwrap();
    let hash = format!("hash-{}", next_seq());
    let size = fs::metadata(path).map(|meta| meta.len() as i64).unwrap_or(0);
    sqlx::query(
        "INSERT INTO files (user_id, user_name, hash, filename, path, size, slice_type, kb_id, is_public, tags, status, log) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&user.id)
    .bind(&user.name)
    .bind(hash)
    .bind(filename)
    .bind(path.to_string_lossy().as_ref())
    .bind(size)
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

pub async fn insert_slice(pool: &SqlitePool, file_id: i64, content: &str) -> i64 {
    let id = sqlx::query("INSERT INTO slices (file_id) VALUES (?)")
        .bind(file_id)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    slice_content::upsert_many(file_id, &[(id, content.to_string())]).await.unwrap();
    id
}

pub async fn insert_slice_position(pool: &SqlitePool, slice_id: i64, page_idx: i32, bbox: [i32; 4]) {
    insert_slice_position_full(pool, slice_id, page_idx, bbox, None, None).await;
}

pub async fn insert_slice_position_full(
    pool: &SqlitePool, slice_id: i64, page_idx: i32, bbox: [i32; 4], sheet_name: Option<&str>, row_num: Option<i32>,
) {
    sqlx::query(
        "INSERT INTO slice_positions (slice_id, page_idx, x1, y1, x2, y2, sheet_name, row_num) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(slice_id)
    .bind(page_idx)
    .bind(bbox[0])
    .bind(bbox[1])
    .bind(bbox[2])
    .bind(bbox[3])
    .bind(sheet_name)
    .bind(row_num)
    .execute(pool)
    .await
    .unwrap();
}

pub async fn insert_graph_node(
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

pub async fn insert_graph_edge(
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

pub async fn insert_entity_mention(pool: &SqlitePool, node_id: i64, slice_id: i64, context: &str) -> i64 {
    sqlx::query("INSERT INTO entity_mentions (node_id, slice_id, context) VALUES (?, ?, ?)")
        .bind(node_id)
        .bind(slice_id)
        .bind(context)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

pub fn multipart_body(boundary: &str, fields: &[(&str, &str)]) -> Vec<u8> {
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

#[allow(clippy::too_many_arguments)]
// Helper: build multipart request that includes an actual file field
pub fn authed_multipart_request_with_file(
    method: &str, uri: impl Into<String>, user: &TestUser, boundary: &str, extra_fields: &[(&str, &str)],
    field_name: &str, file_name: &str, file_content: &[u8],
) -> Request<Body> {
    let mut body = Vec::new();
    for (name, value) in extra_fields {
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{}\"\r\n\r\n", name).as_bytes());
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n", field_name, file_name).as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: text/plain\r\n\r\n");
    body.extend_from_slice(file_content);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

    let content_type = format!("multipart/form-data; boundary={}", boundary);
    authed_request(method, uri.into(), user, Body::from(body), Some(content_type))
}
