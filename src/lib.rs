use axum::{
    body::Body, http::{Request, StatusCode}, middleware::Next, response::{IntoResponse, Response}
};

pub mod api;
pub mod config;
pub mod db;
pub mod frontend;
pub mod graph;
pub mod log4rs;
pub mod processor;
pub mod search;

/// User authentication info extracted from request headers
#[derive(Clone, Debug)]
pub struct AuthUser {
    pub user_id: String,
    pub user_name: String,
    pub role: String,
}

/// Auth middleware: extract `x-user-id` and `x-role` from headers and put them
/// into request extensions as `AuthUser` so handlers can extract `Extension<AuthUser>`.
/// Returns 401 if required headers are missing or invalid.
pub async fn auth(mut req: Request<Body>, next: Next) -> Response {
    let user_id_opt = req.headers().get("x-user-id").and_then(|v| v.to_str().ok()).map(|s| s.to_owned());
    let user_name_opt = req.headers().get("x-user-name").and_then(|v| v.to_str().ok()).map(|s| s.to_owned());
    let role_opt = req.headers().get("x-role").and_then(|v| v.to_str().ok()).map(|s| s.to_owned());

    match (user_id_opt, user_name_opt, role_opt) {
        (Some(user_id), _, Some(role)) => {
            let user_name = user_name_opt.unwrap_or_default();
            let auth_user = AuthUser { user_id, user_name, role };
            req.extensions_mut().insert(auth_user);
            next.run(req).await
        }
        (None, _, _) => (StatusCode::UNAUTHORIZED, "Missing x-user-id header").into_response(),
        (_, _, None) => (StatusCode::UNAUTHORIZED, "Missing x-role header").into_response(),
    }
}
