//! 内嵌 MCP server：以 streamable HTTP 挂在主 axum 实例上，
//! 与 REST API 共享 SqlitePool、SearchEngine 和 header 鉴权（x-user-id / x-role）。
//!
//! 鉴权链路：外层 `crate::auth` 中间件校验 header 并把 `AuthUser` 放进请求扩展；
//! rmcp 的 tower service 会把 `http::request::Parts` 注入每条 JSON-RPC 请求的扩展，
//! tool 内再从中取回 `AuthUser` 做数据级权限过滤（与 REST 同一套口径）。

pub mod app_ui;
mod tools;

use std::sync::Arc;

use rmcp::transport::streamable_http_server::{
    StreamableHttpService, StreamableHttpServerConfig, session::local::LocalSessionManager,
};
use sqlx::SqlitePool;

use crate::search::SearchEngine;

pub use tools::HtknowMcp;

pub type McpService = StreamableHttpService<HtknowMcp, LocalSessionManager>;

/// 构造挂在 axum 上的 MCP streamable HTTP service。
pub fn service(pool: SqlitePool, search_engine: SearchEngine) -> McpService {
    let mut config = StreamableHttpServerConfig::default();
    // 简单请求-响应优先返回 application/json，只有 handler 中途发通知/请求时才回退 SSE。
    config.json_response = true;
    config.allowed_hosts = allowed_hosts();
    StreamableHttpService::new(
        move || Ok(HtknowMcp::new(pool.clone(), search_engine.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    )
}

/// rmcp 默认只放行 loopback Host（防 DNS 重绑定）；容器/反向代理部署时 Host 是域名
/// 或内网地址，用 HTKNOW_MCP_ALLOWED_HOSTS（逗号分隔，可带端口）显式声明。
/// 未配置时清空名单（放行全部）：/mcp 在 auth 中间件之后，Host 校验只是纵深防御。
fn allowed_hosts() -> Vec<String> {
    match std::env::var("HTKNOW_MCP_ALLOWED_HOSTS") {
        Ok(value) => value.split(',').map(|host| host.trim().to_owned()).filter(|host| !host.is_empty()).collect(),
        Err(_) => vec![],
    }
}
