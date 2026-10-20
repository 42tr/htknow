//! MCP App UI 的静态托管：把 mcp-app 构建出的单文件 HTML 挂在
//! `/mcp/app/knowledge-space`，供宿主（cm）以 iframe 嵌入。
//!
//! 页面本身不鉴权（纯静态资源）；数据安全由 `/api/v1/knowledge/*` 的
//! auth 中间件保证——UI 里的每次 API 调用都带 x-user-id/x-role 头。
//! 后续作为标准 MCP Apps 的 ui:// 资源暴露时，复用同一份 HTML。

use axum::{
    Router,
    body::Body,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use rmcp::model::{MetaObject, Resource};
use rust_embed::Embed;

/// MCP Apps 资源 URI：resources/list 声明、tool `_meta["ui/resourceUri"]` 引用。
pub const APP_RESOURCE_URI: &str = "ui://htknow/knowledge-space";

#[derive(Embed)]
#[folder = "mcp-app/dist/"]
struct McpAppAssets;

/// 知识空间单文件 HTML（构建产物在编译期内嵌；未构建时返回 None）。
pub fn app_html() -> Option<String> {
    McpAppAssets::get("index.html").map(|content| String::from_utf8_lossy(&content.data).into_owned())
}

/// 挂到 tool `_meta` 上：MCP Apps 宿主据此为工具结果加载 UI。
pub fn tool_meta() -> MetaObject {
    let mut meta = MetaObject::new();
    meta.insert("ui/resourceUri".to_owned(), serde_json::Value::String(APP_RESOURCE_URI.to_owned()));
    meta
}

/// resources/list 中声明的 ui:// 资源（与 HTTP 托管的 /mcp/app/knowledge-space 是同一份 HTML）。
pub fn app_resource() -> Resource {
    Resource::new(APP_RESOURCE_URI, "knowledge-space")
        .with_title("HTKnow 知识空间")
        .with_description("知识库浏览与 Wiki 交互界面（MCP App UI）")
        .with_mime_type("text/html")
}

fn serve_index() -> Response {
    match app_html() {
        Some(html) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            // 单文件应用无外部引用，更新随版本发布，直接禁用缓存最省心。
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(html))
            .unwrap(),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("mcp-app not built: run `npm run build` in mcp-app/"))
            .unwrap(),
    }
}

async fn index_handler() -> impl IntoResponse {
    serve_index()
}

pub fn router() -> Router {
    Router::new().route("/mcp/app/knowledge-space", get(index_handler))
}
