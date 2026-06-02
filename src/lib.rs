use axum::{
    body::Body, http::{Request, StatusCode}, middleware::Next, response::{IntoResponse, Response}
};
use base64::{Engine, engine::general_purpose::STANDARD};

pub mod api;
pub mod config;
pub mod db;
pub mod export;
pub mod frontend;
pub mod graph;
pub mod log4rs;
pub mod pdf_highlight;
pub mod processor;
pub mod search;

/// User authentication info extracted from request headers
#[derive(Clone, Debug)]
pub struct AuthUser {
    pub user_id: String,
    pub user_name: String,
    pub role: String,
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}

fn decode_base64_if_utf8(value: &str) -> String {
    match STANDARD.decode(value) {
        Ok(bytes) => String::from_utf8(bytes).unwrap_or_else(|_| value.to_owned()),
        Err(_) => value.to_owned(),
    }
}

/// Auth middleware: extract `x-user-id` and `x-role` from headers and put them
/// into request extensions as `AuthUser` so handlers can extract `Extension<AuthUser>`.
/// Returns 401 if required headers are missing or invalid.
pub async fn auth(mut req: Request<Body>, next: Next) -> Response {
    let user_id_opt = req.headers().get("x-user-id").and_then(|v| v.to_str().ok()).map(|s| s.to_owned());
    let user_name_opt = req.headers().get("x-user-name").and_then(|v| v.to_str().ok()).map(decode_base64_if_utf8);
    let role_opt = req.headers().get("x-role").and_then(|v| v.to_str().ok()).map(|s| s.to_owned());

    match (user_id_opt, role_opt) {
        (Some(user_id), Some(role)) => {
            let user_name = user_name_opt.unwrap_or_default();
            let auth_user = AuthUser { user_id, user_name, role };
            req.extensions_mut().insert(auth_user);
            next.run(req).await
        }
        (None, _) => (StatusCode::UNAUTHORIZED, "Missing x-user-id header").into_response(),
        (_, None) => (StatusCode::UNAUTHORIZED, "Missing x-role header").into_response(),
    }
}
