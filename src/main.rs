use axum::Router;
use std::net::SocketAddr;
use tokio::net::TcpListener;

mod api;
mod db;
mod log4rs;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    log4rs::init();
    let pool = db::init().await?;

    let listener = TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(
        listener,
        Router::new()
            .nest("/api/v1/knowledge/", api::app(pool))
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
