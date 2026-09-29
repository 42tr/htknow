use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};

pub mod api;
pub mod archive;
pub mod config;
pub mod db;
pub mod export;
pub mod file_content;
pub mod frontend;
pub mod graph;
pub mod image_description;
pub mod image_ocr;
pub mod image_parse;
pub mod log4rs;
pub mod pdf_content;
pub mod pdf_highlight;
pub mod processor;
pub mod search;
pub mod slice_content;
pub mod wiki;

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

/// 解析 `x-user-name` 请求头。
///
/// HTTP 头只能放 ASCII，因此中文名必须编码后再传。约定：
/// - `b64:<base64>`：显式编码形式（前端使用这种）。
/// - 裸值：仅当它既是合法 base64、解出来又是有效 UTF-8、**且包含非 ASCII 字符**时才按 base64 处理。
///
/// 旧实现是「能解就解」，于是像 `42tr`、`TWFu` 这种恰好构成合法 base64 的纯 ASCII 名字
/// 有约 1/8 的概率被悄悄解码成乱码（能过 UTF-8 校验时），且没有任何日志可循。
fn decode_user_name(value: &str) -> String {
    let (encoded, explicit) = match value.strip_prefix("b64:") {
        Some(encoded) => (encoded.trim(), true),
        None => (value, false),
    };
    let Some(decoded) = STANDARD.decode(encoded).ok().and_then(|bytes| String::from_utf8(bytes).ok()) else {
        // 显式声明了编码却解不出来：用去掉前缀的原文，至少不要把 "b64:" 带进用户名。
        return encoded.to_owned();
    };
    if explicit || decoded.chars().any(|char| !char.is_ascii()) { decoded } else { value.to_owned() }
}

/// Auth middleware: extract `x-user-id` and `x-role` from headers and put them
/// into request extensions as `AuthUser` so handlers can extract `Extension<AuthUser>`.
/// Returns 401 if required headers are missing or invalid.
pub async fn auth(mut req: Request<Body>, next: Next) -> Response {
    let user_id_opt = req.headers().get("x-user-id").and_then(|v| v.to_str().ok()).map(|s| s.to_owned());
    let user_name_opt = req.headers().get("x-user-name").and_then(|v| v.to_str().ok()).map(decode_user_name);
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

#[cfg(test)]
mod tests {
    use super::decode_user_name;
    use base64::{Engine, engine::general_purpose::STANDARD};

    #[test]
    fn user_name_header_keeps_ascii_and_decodes_explicit_or_non_ascii_base64() {
        // 纯 ASCII 名字原样保留，即使它恰好是合法 base64（旧实现在这种情况下会解出乱码）。
        assert_eq!(decode_user_name("42tr"), "42tr");
        assert_eq!(decode_user_name("TWFu"), "TWFu");
        assert_eq!(decode_user_name("user1"), "user1");
        // 显式前缀：任何内容都按 base64 解。
        let encoded = STANDARD.encode("42tr");
        assert_eq!(decode_user_name(&format!("b64:{encoded}")), "42tr");
        // 无前缀但解出非 ASCII（中文名的常规编码方式）：按 base64 解。
        let encoded = STANDARD.encode("张三");
        assert_eq!(decode_user_name(&encoded), "张三");
        // 声明了编码却解不出来时，不要把前缀带进用户名。
        assert_eq!(decode_user_name("b64:!!!"), "!!!");
    }
}
