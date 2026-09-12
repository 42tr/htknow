//! Keycloak resource authentication and browser authorization-code sessions.
use crate::AuthUser;
use axum::{
    Json, Router,
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use std::{
    collections::HashMap,
    env,
    time::{SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, (StatusCode, &'static str)>;
fn denied() -> (StatusCode, &'static str) {
    (StatusCode::UNAUTHORIZED, "Authentication required")
}
fn unavailable() -> (StatusCode, &'static str) {
    (StatusCode::SERVICE_UNAVAILABLE, "Identity provider unavailable")
}
pub fn enabled() -> bool {
    env::var("HTKNOW_AUTH_MODE").unwrap_or_else(|_| "oidc".into()) == "oidc"
}
fn setting(key: &str) -> String {
    env::var(key).unwrap_or_default()
}
fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}
fn random() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}
fn digest(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}
fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get("cookie")?.to_str().ok()?.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then(|| value.to_owned())
    })
}
fn client() -> reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("HTTP client")
    });
    CLIENT.clone()
}

fn endpoint(name: &str) -> String {
    format!("{}/protocol/openid-connect/{}", setting("HTKNOW_OIDC_INTERNAL_URL").trim_end_matches('/'), name)
}
async fn token_request(mut data: Vec<(&str, String)>) -> Result<Value> {
    data.push(("client_id", setting("HTKNOW_OIDC_CLIENT_ID")));
    data.push(("client_secret", setting("HTKNOW_OIDC_CLIENT_SECRET")));
    let response = client().post(endpoint("token")).form(&data).send().await.map_err(|_| unavailable())?;
    if !response.status().is_success() {
        return Err(denied());
    }
    response.json().await.map_err(|_| unavailable())
}
async fn introspect(token: &str) -> Result<Value> {
    let response = client()
        .post(endpoint("token/introspect"))
        .form(&[
            ("token", token.to_owned()),
            ("client_id", "htknow-api".to_owned()),
            ("client_secret", setting("HTKNOW_OIDC_API_SECRET")),
        ])
        .send()
        .await
        .map_err(|_| unavailable())?;
    if !response.status().is_success() {
        return Err(unavailable());
    }
    let claims: Value = response.json().await.map_err(|_| unavailable())?;
    validate_claims(&claims, &setting("HTKNOW_OIDC_ISSUER"))?;
    Ok(claims)
}
fn validate_claims(c: &Value, issuer: &str) -> Result<()> {
    let audience = c["aud"].as_str() == Some("htknow-api")
        || c["aud"].as_array().is_some_and(|a| a.iter().any(|v| v.as_str() == Some("htknow-api")));
    if c["active"] != true
        || c["iss"].as_str() != Some(issuer)
        || !audience
        || c["exp"].as_i64().unwrap_or(0) <= now()
        || c["sub"].as_str().unwrap_or("").is_empty()
        || c["nbf"].as_i64().unwrap_or(0) > now()
    {
        return Err(denied());
    }
    Ok(())
}
async fn identity(pool: &SqlitePool, claims: &Value) -> Result<AuthUser> {
    let roles = claims["resource_access"]["htknow-api"]["roles"].as_array();
    let has_role = |role: &str| roles.is_some_and(|r| r.iter().any(|v| v.as_str() == Some(role)));
    if claims["client_id"].as_str().or(claims["azp"].as_str()) == Some("coassist-worker") && has_role("kb-sync") {
        return Ok(AuthUser {
            user_id: "service:coassist".into(),
            user_name: "CoAssist service".into(),
            role: "service".into(),
        });
    }
    let id: Option<String> = sqlx::query_scalar("SELECT user_id FROM oidc_identities WHERE issuer=? AND subject=?")
        .bind(claims["iss"].as_str().unwrap_or(""))
        .bind(claims["sub"].as_str().unwrap_or(""))
        .fetch_optional(pool)
        .await
        .map_err(|_| unavailable())?;
    Ok(AuthUser {
        user_id: id.ok_or((StatusCode::FORBIDDEN, "Account has not been provisioned"))?,
        user_name: claims["preferred_username"].as_str().unwrap_or("").into(),
        role: if has_role("htknow-admin") { "admin" } else { "user" }.into(),
    })
}
pub async fn init(pool: &SqlitePool) -> anyhow::Result<()> {
    for sql in [
        "CREATE TABLE IF NOT EXISTS oidc_identities (issuer TEXT NOT NULL, subject TEXT NOT NULL, user_id TEXT NOT NULL, PRIMARY KEY(issuer,subject))",
        "CREATE TABLE IF NOT EXISTS oidc_flows (state TEXT PRIMARY KEY, browser TEXT NOT NULL, verifier TEXT NOT NULL, nonce TEXT NOT NULL, expires INTEGER NOT NULL)",
        "CREATE TABLE IF NOT EXISTS oidc_sessions (id TEXT PRIMARY KEY, token TEXT NOT NULL, subject TEXT NOT NULL, expires INTEGER NOT NULL)",
        "CREATE TABLE IF NOT EXISTS coassist_acl_versions (kb_id INTEGER PRIMARY KEY, version INTEGER NOT NULL)",
    ] {
        sqlx::query(sql).execute(pool).await?;
    }
    if enabled() {
        for key in [
            "HTKNOW_OIDC_ISSUER",
            "HTKNOW_OIDC_INTERNAL_URL",
            "HTKNOW_OIDC_CLIENT_ID",
            "HTKNOW_OIDC_CLIENT_SECRET",
            "HTKNOW_OIDC_API_SECRET",
            "HTKNOW_OIDC_REDIRECT_URI",
            "HTKNOW_PUBLIC_URL",
        ] {
            anyhow::ensure!(!setting(key).is_empty(), "{key} must be configured in OIDC mode");
        }
    } else {
        anyhow::ensure!(setting("HTKNOW_AUTH_MODE") == "trusted_headers", "Unknown HTKNOW_AUTH_MODE");
        log::warn!("Legacy trusted_headers authentication enabled; isolate this listener from untrusted callers");
    }
    Ok(())
}
pub async fn middleware(State(pool): State<SqlitePool>, mut req: Request<Body>, next: Next) -> Response {
    if !enabled() {
        return crate::auth(req, next).await;
    }
    let headers = req.headers().clone();
    let method = req.method().clone();
    let result = async {
        let token = if let Some(header) = headers.get("authorization") {
            // Invalid Bearer never falls back to cookies or identity headers.
            let header = header.to_str().map_err(|_| denied())?;
            let (scheme, token) = header.split_once(' ').ok_or_else(denied)?;
            if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
                return Err(denied());
            }
            token.to_owned()
        } else {
            let session = cookie(&headers, "htknow_session").ok_or_else(denied)?;
            if !matches!(*&method, axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS)
                && headers.get("origin").and_then(|v| v.to_str().ok())
                    != Some(setting("HTKNOW_PUBLIC_URL").trim_end_matches('/'))
            {
                return Err((StatusCode::FORBIDDEN, "Invalid origin"));
            }
            sqlx::query_scalar::<_, String>("SELECT token FROM oidc_sessions WHERE id=? AND expires>?")
                .bind(digest(&session))
                .bind(now())
                .fetch_optional(&pool)
                .await
                .map_err(|_| unavailable())?
                .ok_or_else(denied)?
        };
        let claims = introspect(&token).await?;
        identity(&pool, &claims).await
    }
    .await;
    match result {
        Ok(user) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Err(error) => error.into_response(),
    }
}
pub fn router(pool: SqlitePool) -> Router {
    Router::new()
        .route("/api/auth/login", get(login))
        .route("/api/auth/callback", get(callback))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/config", get(config))
        .with_state(pool)
}
async fn config() -> Json<Value> {
    Json(json!({"oidc": enabled(), "login_url": "/api/auth/login"}))
}
async fn login(State(pool): State<SqlitePool>) -> Result<Response> {
    if !enabled() {
        return Err((StatusCode::NOT_FOUND, "OIDC disabled"));
    }
    let (state, browser, verifier, nonce) = (random(), random(), random(), random());
    sqlx::query("DELETE FROM oidc_flows WHERE expires<=?")
        .bind(now())
        .execute(&pool)
        .await
        .map_err(|_| unavailable())?;
    sqlx::query("DELETE FROM oidc_sessions WHERE expires<=?")
        .bind(now())
        .execute(&pool)
        .await
        .map_err(|_| unavailable())?;
    sqlx::query("INSERT INTO oidc_flows VALUES (?,?,?,?,?)")
        .bind(digest(&state))
        .bind(digest(&browser))
        .bind(&verifier)
        .bind(&nonce)
        .bind(now() + 300)
        .execute(&pool)
        .await
        .map_err(|_| unavailable())?;
    let mut url = reqwest::Url::parse(&format!("{}/protocol/openid-connect/auth", setting("HTKNOW_OIDC_ISSUER")))
        .map_err(|_| unavailable())?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", setting("HTKNOW_OIDC_CLIENT_ID")),
        ("response_type", "code".into()),
        ("scope", "openid profile".into()),
        ("redirect_uri", setting("HTKNOW_OIDC_REDIRECT_URI")),
        ("state", state),
        ("nonce", nonce),
        ("code_challenge_method", "S256".into()),
        ("code_challenge", URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))),
    ]);
    let mut response = Redirect::to(url.as_str()).into_response();
    set_cookie(&mut response, "htknow_flow", &browser, 300)?;
    Ok(response)
}
fn set_cookie(response: &mut Response, name: &str, value: &str, age: i64) -> Result<()> {
    let secure = if setting("HTKNOW_PUBLIC_URL").starts_with("https://") { "; Secure" } else { "" };
    response.headers_mut().append(
        "set-cookie",
        format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={age}{secure}")
            .parse()
            .map_err(|_| unavailable())?,
    );
    response.headers_mut().insert("cache-control", "no-store".parse().unwrap());
    response.headers_mut().insert("referrer-policy", "no-referrer".parse().unwrap());
    Ok(())
}
async fn callback(
    State(pool): State<SqlitePool>, headers: HeaderMap, Query(params): Query<HashMap<String, String>>,
) -> Result<Response> {
    let state = params.get("state").ok_or_else(denied)?;
    let code = params.get("code").ok_or_else(denied)?;
    // Atomic consume also verifies browser binding. SQLite RETURNING requires 3.35+.
    let row = sqlx::query("DELETE FROM oidc_flows WHERE state=? AND browser=? AND expires>? RETURNING verifier,nonce")
        .bind(digest(state))
        .bind(digest(&cookie(&headers, "htknow_flow").unwrap_or_default()))
        .bind(now())
        .fetch_optional(&pool)
        .await
        .map_err(|_| unavailable())?
        .ok_or_else(denied)?;
    let tokens = token_request(vec![
        ("grant_type", "authorization_code".into()),
        ("code", code.clone()),
        ("redirect_uri", setting("HTKNOW_OIDC_REDIRECT_URI")),
        ("code_verifier", row.get("verifier")),
    ])
    .await?;
    let id_token = tokens["id_token"].as_str().ok_or_else(denied)?;
    let access = tokens["access_token"].as_str().ok_or_else(denied)?;
    let keys: jsonwebtoken::jwk::JwkSet = client()
        .get(endpoint("certs"))
        .send()
        .await
        .map_err(|_| unavailable())?
        .error_for_status()
        .map_err(|_| unavailable())?
        .json()
        .await
        .map_err(|_| unavailable())?;
    let header = jsonwebtoken::decode_header(id_token).map_err(|_| denied())?;
    let key = keys.find(header.kid.as_deref().ok_or_else(denied)?).ok_or_else(denied)?;
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
    validation.set_issuer(&[setting("HTKNOW_OIDC_ISSUER")]);
    validation.set_audience(&[setting("HTKNOW_OIDC_CLIENT_ID")]);
    validation.set_required_spec_claims(&["exp", "iat", "sub", "iss", "aud"]);
    validation.validate_nbf = true;
    let claims = jsonwebtoken::decode::<Value>(
        id_token,
        &jsonwebtoken::DecodingKey::from_jwk(key).map_err(|_| denied())?,
        &validation,
    )
    .map_err(|_| denied())?
    .claims;
    if claims["nonce"].as_str() != Some(row.get::<String, _>("nonce").as_str()) {
        return Err(denied());
    }
    if let Some(azp) = claims["azp"].as_str() {
        if azp != setting("HTKNOW_OIDC_CLIENT_ID") {
            return Err(denied());
        }
    }
    if let Some(at_hash) = claims["at_hash"].as_str() {
        if at_hash != URL_SAFE_NO_PAD.encode(&Sha256::digest(access.as_bytes())[..16]) {
            return Err(denied());
        }
    }
    let payload = introspect(access).await?;
    if payload["sub"] != claims["sub"] {
        return Err(denied());
    }
    identity(&pool, &payload).await?;
    let session = random();
    // No refresh token retained here: expiry re-enters SSO without another password prompt.
    let expires = payload["exp"].as_i64().ok_or_else(denied)?;
    sqlx::query("INSERT INTO oidc_sessions VALUES (?,?,?,?)")
        .bind(digest(&session))
        .bind(access)
        .bind(claims["sub"].as_str().ok_or_else(denied)?)
        .bind(expires)
        .execute(&pool)
        .await
        .map_err(|_| unavailable())?;
    let mut response = Redirect::to("/").into_response();
    set_cookie(&mut response, "htknow_session", &session, expires - now())?;
    set_cookie(&mut response, "htknow_flow", "", 0)?;
    Ok(response)
}
async fn logout(State(pool): State<SqlitePool>, headers: HeaderMap) -> Result<Response> {
    if headers.get("origin").and_then(|v| v.to_str().ok()) != Some(setting("HTKNOW_PUBLIC_URL").trim_end_matches('/')) {
        return Err((StatusCode::FORBIDDEN, "Invalid origin"));
    }
    if let Some(session) = cookie(&headers, "htknow_session") {
        sqlx::query("DELETE FROM oidc_sessions WHERE id=?")
            .bind(digest(&session))
            .execute(&pool)
            .await
            .map_err(|_| unavailable())?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    set_cookie(&mut response, "htknow_session", "", 0)?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_inactive_wrong_issuer_audience_and_expiry() {
        let valid = json!({"active":true,"iss":"test","sub":"u","aud":["htknow-api"],"exp":now()+60});
        assert!(validate_claims(&valid, "test").is_ok());
        for (key, value) in [
            ("active", json!(false)),
            ("iss", json!("other")),
            ("aud", json!("coassist-api")),
            ("exp", json!(0)),
            ("sub", json!("")),
        ] {
            let mut claims = valid.clone();
            claims[key] = value;
            assert!(validate_claims(&claims, "test").is_err());
        }
    }
}

pub async fn me(axum::Extension(user): axum::Extension<AuthUser>) -> Json<Value> {
    Json(json!({"user_id":user.user_id,"user_name":user.user_name,"is_admin":user.is_admin()}))
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    #[tokio::test]
    async fn identity_binding_and_service_role_cannot_grant_global_admin() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE oidc_identities(issuer TEXT,subject TEXT,user_id TEXT); INSERT INTO oidc_identities VALUES('issuer','subject','7');").execute(&pool).await.unwrap();
        let user = identity(&pool, &json!({"iss":"issuer","sub":"subject","role":"admin"})).await.unwrap();
        assert_eq!(user.user_id, "7");
        assert!(!user.is_admin());
        assert!(identity(&pool, &json!({"iss":"other","sub":"subject"})).await.is_err());
        let worker = identity(
            &pool,
            &json!({"client_id":"coassist-worker","resource_access":{"htknow-api":{"roles":["kb-sync"]}}}),
        )
        .await
        .unwrap();
        assert_eq!(worker.user_id, "service:coassist");
        assert!(!worker.is_admin());
        assert!(
            identity(&pool, &json!({"client_id":"untrusted","resource_access":{"htknow-api":{"roles":["kb-sync"]}}}))
                .await
                .is_err()
        );
    }
}

#[cfg(test)]
mod middleware_tests {
    use super::*;
    use tower::ServiceExt;
    #[tokio::test]
    async fn spoofed_headers_and_invalid_bearer_never_authenticate() {
        assert!(enabled(), "Run auth tests with HTKNOW_AUTH_MODE=oidc");
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        let app = Router::new()
            .route("/", get(|| async { "authenticated" }))
            .layer(axum::middleware::from_fn_with_state(pool, middleware));
        for authorization in [None, Some("Basic invalid"), Some("Bearer ")] {
            let mut request = Request::builder().uri("/").header("x-user-id", "1").header("x-role", "admin");
            if let Some(value) = authorization {
                request = request.header("authorization", value).header("cookie", "htknow_session=forged");
            }
            let response = app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }
}
