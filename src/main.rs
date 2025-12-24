use std::net::SocketAddr;
use tokio::net::TcpListener;

mod api;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(
        listener,
        api::app().into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
