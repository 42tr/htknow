use std::net::SocketAddr;

use axum::{
    Router, body::Body, http::{Request, StatusCode}, middleware::{self, Next}, response::{IntoResponse, Response}, extract::DefaultBodyLimit
};
use tokio::net::TcpListener;

mod api;
mod db;
mod frontend;
mod log4rs;
mod processor;
mod search;

/// User authentication info extracted from request headers
#[derive(Clone, Debug)]
pub struct AuthUser {
    pub user_id: String,
    pub role: String,
}

/// Auth middleware: extract `x-user-id` and `x-role` from headers and put them
/// into request extensions as `AuthUser` so handlers can extract `Extension<AuthUser>`.
/// Returns 401 if required headers are missing or invalid.
async fn auth<B>(mut req: Request<Body>, next: Next) -> Response {
    // Extract header values into owned Strings
    let user_id_opt = req.headers().get("x-user-id").and_then(|v| v.to_str().ok()).map(|s| s.to_owned());

    let role_opt = req.headers().get("x-role").and_then(|v| v.to_str().ok()).map(|s| s.to_owned());

    match (user_id_opt, role_opt) {
        (Some(user_id), Some(role)) => {
            let auth_user = AuthUser { user_id, role };
            req.extensions_mut().insert(auth_user);
            next.run(req).await
        }
        (None, _) => (StatusCode::UNAUTHORIZED, "Missing x-user-id header").into_response(),
        (_, None) => (StatusCode::UNAUTHORIZED, "Missing x-role header").into_response(),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    log4rs::init();
    let pool = db::init().await?;
    let search_engine = search::SearchEngine::init().await;

    // 启动文件处理后台任务（每30秒检查一次）
    let processor = processor::FileProcessor::new(pool.clone(), search_engine.clone(), 30);
    processor.start();

    // API 路由需要认证
    let api_router = Router::new()
        .nest("/api/v1/knowledge/", api::app(pool, search_engine))
        .layer(middleware::from_fn(auth::<Body>))
        .layer(DefaultBodyLimit::max(500 * 1024 * 1024)); // 500MB 上传限制

    // 合并路由：前端不需要认证，API需要认证
    let app = Router::new()
        .merge(api_router)
        .merge(frontend::router());

    let listener = TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}
