use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;
use tokio::net::TcpListener;

mod api;
mod db;
mod log4rs;

/// Auth middleware: extract `x-user-id` from headers and put the raw `String`
/// into request extensions so handlers can extract `Extension<String>`.
/// Returns 401 if the header is missing or invalid.
async fn auth<B>(mut req: Request<Body>, next: Next) -> Response {
    // Extract header value into an owned String first (no references into `req` are kept).
    let user_id_opt = req
        .headers()
        .get("x-user-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned());

    match user_id_opt {
        Some(uid) => {
            req.extensions_mut().insert(uid);
            next.run(req).await
        }
        None => (StatusCode::UNAUTHORIZED, "Missing x-user-id header").into_response(),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    log4rs::init();
    let pool = db::init().await?;

    let app = Router::new()
        .nest("/api/v1/knowledge/", api::app(pool))
        .layer(middleware::from_fn(auth::<Body>));

    let listener = TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
