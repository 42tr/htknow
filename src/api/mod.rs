use axum::{Router, response::IntoResponse, routing::get};

pub fn app() -> Router {
    Router::new().route("/", get(root))
}

async fn root() -> impl IntoResponse {
    "Hello from axum!"
}
