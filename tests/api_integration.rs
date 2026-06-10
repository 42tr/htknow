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
    let _ = app().await;
    db::init().await.expect("init db")
}

struct TestUser {
    id: String,
    name: String,
    role: String,
}

impl TestUser {
    fn new(prefix: &str) -> Self {
        Self::with_role(prefix, "admin")
    }

    fn with_role(prefix: &str, role: &str) -> Self {
        let seq = next_seq();
        Self { id: format!("{}-{}", prefix, seq), name: format!("{}-name-{}", prefix, seq), role: role.to_string() }
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
    if bytes.is_empty() {
        panic!("response body is empty");
    }
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!("failed to parse JSON response: {}\nbody: {}", e, String::from_utf8_lossy(&bytes));
    })
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
        format!("/api/v1/knowledge/knowledge_base/{}", kb_id),
        &user,
        serde_json::json!({ "is_public": true }),
    );
    let public_res = app.clone().oneshot(public_req).await.unwrap();
    assert_eq!(public_res.status(), StatusCode::OK);
    let public_json = response_json(public_res).await;
    assert_eq!(public_json["is_public"].as_bool(), Some(true));

    let reparse_req = authed_empty_request("POST", "/api/v1/knowledge/knowledge_base/reparse", &user);
    let reparse_res = app.clone().oneshot(reparse_req).await.unwrap();
    assert_eq!(reparse_res.status(), StatusCode::OK);
    let reparse_json = response_json(reparse_res).await;
    assert_eq!(reparse_json["kb_count"].as_i64(), Some(1));
    assert_eq!(reparse_json["file_count"].as_i64(), Some(0));
}

#[tokio::test]
async fn knowledge_base_tree_and_detail_flow() {
    let app = app().await;
    let pool = get_pool().await;
    let env = setup_env();
    let user = TestUser::new("kb-tree");

    let root_kb_id = insert_kb(&pool, &user, "Root KB", "analysis", None, false).await;
    let child_kb_id = insert_kb(&pool, &user, "Child KB", "analysis", Some(root_kb_id), false).await;

    let file_dir = env.data_dir.join("files");
    fs::create_dir_all(&file_dir).unwrap();
    let root_file_path = file_dir.join(format!("root-file-{}.txt", next_seq()));
    let child_file_path = file_dir.join(format!("child-file-{}.txt", next_seq()));
    fs::write(&root_file_path, b"root file").unwrap();
    fs::write(&child_file_path, b"child file").unwrap();

    let root_file_id =
        insert_file(&pool, &user, "root-file.txt", &root_file_path, Some(root_kb_id), vec!["root".to_string()], false)
            .await;
    let child_file_id = insert_file(
        &pool,
        &user,
        "child-file.txt",
        &child_file_path,
        Some(child_kb_id),
        vec!["child".to_string()],
        false,
    )
    .await;

    let tree_req =
        authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/tree?kb_id={}", root_kb_id), &user);
    let tree_res = app.clone().oneshot(tree_req).await.unwrap();
    assert_eq!(tree_res.status(), StatusCode::OK);
    let tree_json = response_json(tree_res).await;
    let tree_nodes = tree_json.as_array().expect("tree nodes");
    assert_eq!(tree_nodes.len(), 1);
    let root_node = &tree_nodes[0];
    assert_eq!(root_node["id"].as_i64(), Some(root_kb_id));
    assert!(root_node["files"].as_array().unwrap().iter().any(|file| file["id"].as_i64() == Some(root_file_id)));
    assert!(root_node["children"].as_array().unwrap().iter().any(|child| {
        child["id"].as_i64() == Some(child_kb_id)
            && child["files"].as_array().unwrap().iter().any(|file| file["id"].as_i64() == Some(child_file_id))
    }));

    let detail_req = authed_empty_request(
        "GET",
        format!("/api/v1/knowledge/knowledge_base/{}?filename=root-file", root_kb_id),
        &user,
    );
    let detail_res = app.clone().oneshot(detail_req).await.unwrap();
    assert_eq!(detail_res.status(), StatusCode::OK);
    let detail_json = response_json(detail_res).await;
    assert_eq!(detail_json["id"].as_i64(), Some(root_kb_id));
    let detail_files = detail_json["files"].as_array().expect("detail files");
    assert!(detail_files.iter().any(|file| file["id"].as_i64() == Some(root_file_id)));
}

#[tokio::test]
async fn file_endpoints_flow() {
    let app = app().await;
    let pool = get_pool().await;
    let env = setup_env();
    let user = TestUser::new("file");

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

    let delete_req = authed_empty_request("DELETE", format!("/api/v1/knowledge/files/{}", file_id), &user);
    let delete_res = app.clone().oneshot(delete_req).await.unwrap();
    assert_eq!(delete_res.status(), StatusCode::OK);
    assert!(!file_path.exists());
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

#[tokio::test]
async fn kb_permission_viewer_can_read_but_not_modify() {
    let app = app().await;
    let owner = TestUser::with_role("kb-perm-owner", "user");
    let viewer = TestUser::with_role("kb-perm-viewer", "user");

    // Owner creates a private KB
    let create_body = serde_json::json!({
        "name": "Permission Test KB",
        "description": "kb for permission tests",
        "kb_type": "analysis",
        "parent_id": null,
        "is_public": false
    });
    let create_req = authed_json_request("POST", "/api/v1/knowledge/knowledge_base/", &owner, create_body);
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_res.status(), StatusCode::OK);
    let created = response_json(create_res).await;
    let kb_id = created["id"].as_i64().expect("created kb id");
    assert_eq!(created["current_user_permission"].as_str(), Some("admin"));

    // Owner grants viewer permission
    let grant_req = authed_json_request(
        "POST",
        format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id),
        &owner,
        serde_json::json!({ "user_id": viewer.id, "permission": "viewer" }),
    );
    let grant_res = app.clone().oneshot(grant_req).await.unwrap();
    assert_eq!(grant_res.status(), StatusCode::OK);

    // Viewer can list the KB
    let list_req = authed_empty_request("GET", "/api/v1/knowledge/knowledge_base/", &viewer);
    let list_res = app.clone().oneshot(list_req).await.unwrap();
    assert_eq!(list_res.status(), StatusCode::OK);
    let list_bytes = list_res.into_body().collect().await.unwrap().to_bytes();
    let list: Vec<Value> = serde_json::from_slice(&list_bytes).unwrap();
    let kb = list.iter().find(|k| k["id"].as_i64() == Some(kb_id)).expect("viewer sees the kb");
    assert_eq!(kb["current_user_permission"].as_str(), Some("viewer"));

    // Viewer can get detail
    let get_req = authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}", kb_id), &viewer);
    let get_res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let get_json = response_json(get_res).await;
    assert_eq!(get_json["current_user_permission"].as_str(), Some("viewer"));

    // Viewer cannot update the KB
    let update_req = authed_json_request(
        "PUT",
        format!("/api/v1/knowledge/knowledge_base/{}", kb_id),
        &viewer,
        serde_json::json!({ "name": "Hacked" }),
    );
    let update_res = app.clone().oneshot(update_req).await.unwrap();
    assert_eq!(update_res.status(), StatusCode::FORBIDDEN);

    // Viewer cannot reparse
    let reparse_req = authed_empty_request("POST", format!("/api/v1/knowledge/knowledge_base/{}/reparse", kb_id), &viewer);
    let reparse_res = app.clone().oneshot(reparse_req).await.unwrap();
    assert_eq!(reparse_res.status(), StatusCode::FORBIDDEN);

    // Viewer cannot delete
    let delete_req = authed_empty_request("DELETE", format!("/api/v1/knowledge/knowledge_base/{}", kb_id), &viewer);
    let delete_res = app.clone().oneshot(delete_req).await.unwrap();
    assert_eq!(delete_res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn kb_permission_editor_can_upload_but_not_delete() {
    let app = app().await;
    let env = setup_env();
    let owner = TestUser::with_role("kb-editor-owner", "user");
    let editor = TestUser::with_role("kb-editor-editor", "user");

    // Owner creates a private storage KB (so we can upload without parse complications)
    let create_body = serde_json::json!({
        "name": "Editor Test KB",
        "description": "kb for editor permission tests",
        "kb_type": "storage",
        "parent_id": null,
        "is_public": false
    });
    let create_req = authed_json_request("POST", "/api/v1/knowledge/knowledge_base/", &owner, create_body);
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_res.status(), StatusCode::OK);
    let kb_id = response_json(create_res).await["id"].as_i64().unwrap();

    // Owner grants editor permission
    let grant_req = authed_json_request(
        "POST",
        format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id),
        &owner,
        serde_json::json!({ "user_id": editor.id, "permission": "editor" }),
    );
    let grant_res = app.clone().oneshot(grant_req).await.unwrap();
    assert_eq!(grant_res.status(), StatusCode::OK);

    // Editor can update the KB name/description
    let update_req = authed_json_request(
        "PUT",
        format!("/api/v1/knowledge/knowledge_base/{}", kb_id),
        &editor,
        serde_json::json!({ "name": "Renamed by Editor", "description": "updated" }),
    );
    let update_res = app.clone().oneshot(update_req).await.unwrap();
    assert_eq!(update_res.status(), StatusCode::OK);

    // Editor cannot change visibility
    let vis_req = authed_json_request(
        "PUT",
        format!("/api/v1/knowledge/knowledge_base/{}", kb_id),
        &editor,
        serde_json::json!({ "is_public": true }),
    );
    let vis_res = app.clone().oneshot(vis_req).await.unwrap();
    assert_eq!(vis_res.status(), StatusCode::FORBIDDEN);

    // Editor can upload a file
    let file_dir = env.data_dir.join("files");
    fs::create_dir_all(&file_dir).unwrap();
    let test_file = file_dir.join(format!("editor-upload-{}.txt", next_seq()));
    fs::write(&test_file, b"editor upload test").unwrap();

    let boundary = format!("boundary-{}", next_seq());
    let upload_req = authed_multipart_request_with_file(
        "POST",
        "/api/v1/knowledge/files/",
        &editor,
        &boundary,
        &[
            ("kb_id", &kb_id.to_string()),
            ("slice_type", "text"),
        ],
        "file",
        "test.txt",
        b"editor upload test",
    );
    let upload_res = app.clone().oneshot(upload_req).await.unwrap();
    assert_eq!(upload_res.status(), StatusCode::OK);

    // Editor cannot delete the KB
    let delete_req = authed_empty_request("DELETE", format!("/api/v1/knowledge/knowledge_base/{}", kb_id), &editor);
    let delete_res = app.clone().oneshot(delete_req).await.unwrap();
    assert_eq!(delete_res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn kb_permission_admin_can_manage_permissions() {
    let app = app().await;
    let owner = TestUser::with_role("kb-admin-owner", "user");
    let viewer = TestUser::with_role("kb-admin-viewer", "user");

    // Owner creates KB
    let create_body = serde_json::json!({
        "name": "Admin Perm Test KB",
        "description": "test",
        "kb_type": "analysis",
        "is_public": false
    });
    let create_req = authed_json_request("POST", "/api/v1/knowledge/knowledge_base/", &owner, create_body);
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    let kb_id = response_json(create_res).await["id"].as_i64().unwrap();

    // Owner lists permissions (should be empty initially except no explicit rows)
    let list_req = authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id), &owner);
    let list_res = app.clone().oneshot(list_req).await.unwrap();
    assert_eq!(list_res.status(), StatusCode::OK);

    // Owner grants viewer permission
    let grant_req = authed_json_request(
        "POST",
        format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id),
        &owner,
        serde_json::json!({ "user_id": viewer.id, "permission": "viewer" }),
    );
    let grant_res = app.clone().oneshot(grant_req).await.unwrap();
    assert_eq!(grant_res.status(), StatusCode::OK);
    let grant_json = response_json(grant_res).await;
    assert_eq!(grant_json["user_id"].as_str(), Some(viewer.id.as_str()));
    assert_eq!(grant_json["permission"].as_str(), Some("viewer"));

    // Viewer cannot call permission APIs
    let viewer_list_req = authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id), &viewer);
    let viewer_list_res = app.clone().oneshot(viewer_list_req).await.unwrap();
    assert_eq!(viewer_list_res.status(), StatusCode::FORBIDDEN);

    // Owner upgrades viewer to admin
    let upgrade_req = authed_json_request(
        "POST",
        format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id),
        &owner,
        serde_json::json!({ "user_id": viewer.id, "permission": "admin" }),
    );
    let upgrade_res = app.clone().oneshot(upgrade_req).await.unwrap();
    assert_eq!(upgrade_res.status(), StatusCode::OK);

    // Now viewer (as KB admin) can list permissions
    let list2_req = authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id), &viewer);
    let list2_res = app.clone().oneshot(list2_req).await.unwrap();
    assert_eq!(list2_res.status(), StatusCode::OK);

    // Owner removes viewer permission
    let remove_req = authed_empty_request(
        "DELETE",
        format!("/api/v1/knowledge/knowledge_base/{}/permissions/{}", kb_id, viewer.id),
        &owner,
    );
    let remove_res = app.clone().oneshot(remove_req).await.unwrap();
    assert_eq!(remove_res.status(), StatusCode::OK);

    // After removal, viewer can no longer access the KB
    let get_req = authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}", kb_id), &viewer);
    let get_res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(get_res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn kb_permission_search_filters_unauthorized_kb() {
    let app = app().await;
    let pool = get_pool().await;
    let owner = TestUser::with_role("kb-search-owner", "user");
    let other = TestUser::with_role("kb-search-other", "user");

    // Owner creates a private KB with a file
    let kb_id = insert_kb(&pool, &owner, "Search Permission KB", "analysis", None, false).await;
    let file_dir = setup_env().data_dir.join("files");
    fs::create_dir_all(&file_dir).unwrap();
    let test_file = file_dir.join(format!("search-perm-{}.txt", next_seq()));
    fs::write(&test_file, b"secret content about dragons").unwrap();
    let file_id = insert_file(&pool, &owner, "secret-perm.txt", &test_file, Some(kb_id), vec![], false).await;

    // Set file as completed so it appears in full search
    sqlx::query("UPDATE files SET status = 1 WHERE id = ?")
        .bind(file_id)
        .execute(&pool)
        .await
        .unwrap();

    // Owner can use full-search filename filter and find the file
    let owner_search_req = authed_empty_request(
        "GET",
        "/api/v1/knowledge/search/full?filename=secret-perm.txt",
        &owner,
    );
    let owner_search_res = app.clone().oneshot(owner_search_req).await.unwrap();
    assert_eq!(owner_search_res.status(), StatusCode::OK);
    let owner_json = response_json(owner_search_res).await;
    let owner_results = owner_json["results"].as_array().expect("results");
    assert!(
        owner_results.iter().any(|r| r["file"]["id"].as_i64() == Some(file_id)),
        "owner should find their own file"
    );

    // Other user without permission cannot see the result
    let other_search_req = authed_empty_request(
        "GET",
        "/api/v1/knowledge/search/full?filename=secret-perm.txt",
        &other,
    );
    let other_search_res = app.clone().oneshot(other_search_req).await.unwrap();
    assert_eq!(other_search_res.status(), StatusCode::OK);
    let other_json = response_json(other_search_res).await;
    let other_results = other_json["results"].as_array().expect("results");
    assert!(
        !other_results.iter().any(|r| r["file"]["id"].as_i64() == Some(file_id)),
        "other user should not see unauthorized file"
    );

    // Grant viewer permission and verify search now returns result
    let grant_req = authed_json_request(
        "POST",
        format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id),
        &owner,
        serde_json::json!({ "user_id": other.id, "permission": "viewer" }),
    );
    let grant_res = app.clone().oneshot(grant_req).await.unwrap();
    assert_eq!(grant_res.status(), StatusCode::OK);

    // Verify other can now access the KB detail
    let get_req = authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}", kb_id), &other);
    let get_res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let get_json = response_json(get_res).await;
    assert_eq!(get_json["current_user_permission"].as_str(), Some("viewer"));


    let granted_search_req = authed_empty_request(
        "GET",
        "/api/v1/knowledge/search/full?filename=secret-perm.txt",
        &other,
    );
    let granted_search_res = app.clone().oneshot(granted_search_req).await.unwrap();
    assert_eq!(granted_search_res.status(), StatusCode::OK);
    let granted_json = response_json(granted_search_res).await;
    let granted_results = granted_json["results"].as_array().expect("results");
    assert!(
        granted_results.iter().any(|r| r["file"]["id"].as_i64() == Some(file_id)),
        "viewer should now find the file"
    );
}

// Helper: build multipart request that includes an actual file field
fn authed_multipart_request_with_file(
    method: &str,
    uri: impl Into<String>,
    user: &TestUser,
    boundary: &str,
    extra_fields: &[(&str, &str)],
    field_name: &str,
    file_name: &str,
    file_content: &[u8],
) -> Request<Body> {
    let mut body = Vec::new();
    for (name, value) in extra_fields {
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{}\"\r\n\r\n", name).as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n",
            field_name, file_name
        )
        .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: text/plain\r\n\r\n");
    body.extend_from_slice(file_content);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

    let content_type = format!("multipart/form-data; boundary={}", boundary);
    authed_request(method, uri.into(), user, Body::from(body), Some(content_type))
}
