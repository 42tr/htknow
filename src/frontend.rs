use axum::{
    Router, body::Body, http::{StatusCode, Uri, header}, response::{IntoResponse, Response}, routing::get
};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "frontend/dist/"]
struct Assets;

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    // 如果路径为空，返回 index.html
    let path = if path.is_empty() { "index.html" } else { path };

    serve_file(path)
}

async fn index_handler() -> impl IntoResponse {
    serve_file("index.html")
}

fn serve_file(path: &str) -> Response {
    if path == "favicon.ico" {
        return Response::builder().status(StatusCode::NO_CONTENT).body(Body::empty()).unwrap();
    }

    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream().to_string();

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime)
                .body(Body::from(content.data.into_owned()))
                .unwrap()
        }
        None => Response::builder().status(StatusCode::NOT_FOUND).body(Body::from("Not Found")).unwrap(),
    }
}

pub fn router() -> Router {
    Router::new().route("/", get(index_handler)).route("/{*path}", get(static_handler))
}
