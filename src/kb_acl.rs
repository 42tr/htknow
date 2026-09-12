//! Shared resource authorization, independent of search and file processing.
use crate::{
    AuthUser,
    api::error::{ApiError, ApiResult},
};
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use sqlx::SqlitePool;

/// Get the highest permission level a user has on a knowledge base.
/// Priority: global admin > owner > explicit permission > is_public.
/// Returns None if the user has no access at all.
pub async fn get_kb_permission(pool: &SqlitePool, kb_id: i64, user_id: &str, is_admin: bool) -> Option<String> {
    if is_admin {
        return Some("admin".to_string());
    }

    // 1. Check if owner
    let owner: Option<String> = sqlx::query_scalar("SELECT user_id FROM knowledge_bases WHERE id = ?")
        .bind(kb_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    if owner.as_deref() == Some(user_id) {
        return Some("admin".to_string());
    }

    // 2. Check explicit permission
    let explicit: Option<String> =
        sqlx::query_scalar("SELECT permission FROM kb_permissions WHERE kb_id = ? AND user_id = ?")
            .bind(kb_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    if let Some(perm) = explicit {
        return Some(perm);
    }

    // 3. Check if public
    let is_public: Option<i64> = sqlx::query_scalar("SELECT is_public FROM knowledge_bases WHERE id = ?")
        .bind(kb_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    if is_public == Some(1) {
        return Some("viewer".to_string());
    }

    None
}

pub async fn ensure_file_readable(
    pool: &SqlitePool, kb_id: Option<i64>, owner: &str, is_public: bool, user: &AuthUser,
) -> ApiResult<()> {
    let allowed = if let Some(kb_id) = kb_id {
        get_kb_permission(pool, kb_id, &user.user_id, user.is_admin()).await.is_some()
    } else {
        user.is_admin() || is_public || owner == user.user_id
    };
    if allowed { Ok(()) } else { Err(ApiError::NotFound("File not found or permission denied".into())) }
}

#[derive(serde::Deserialize)]
pub struct CoassistAclSnapshot {
    pub version: i64,
    pub permissions: Vec<CoassistAclEntry>,
}
#[derive(serde::Deserialize)]
pub struct CoassistAclEntry {
    pub user_id: String,
    pub permission: String,
}

/// Atomically replace the complete ACL, including revocations and empty snapshots.
/// Only the service principal may manage its own KBs; it has no global admin role.
pub async fn replace_coassist_permissions(
    Path(id): Path<i64>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
    Json(snapshot): Json<CoassistAclSnapshot>,
) -> ApiResult<Json<serde_json::Value>> {
    if user.role != "service" || user.user_id != "service:coassist" {
        return Err(ApiError::Forbidden("CoAssist service identity required".into()));
    }
    if snapshot.version <= 0
        || snapshot.permissions.iter().any(|p| {
            p.user_id.is_empty()
                || p.user_id.starts_with("service:")
                || !matches!(p.permission.as_str(), "viewer" | "editor" | "admin")
        })
    {
        return Err(ApiError::BadRequest("Invalid ACL snapshot".into()));
    }
    let mut tx = pool.begin().await?;
    // First statement acquires the SQLite write lock before reading ownership/version.
    sqlx::query("INSERT OR IGNORE INTO coassist_acl_versions(kb_id,version) VALUES (?,0)")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let owner: Option<String> =
        sqlx::query_scalar("SELECT user_id FROM knowledge_bases WHERE id=?").bind(id).fetch_optional(&mut *tx).await?;
    if owner.as_deref() != Some("service:coassist") {
        return Err(ApiError::Forbidden("Knowledge base is not managed by CoAssist".into()));
    }
    let updated = sqlx::query("UPDATE coassist_acl_versions SET version=? WHERE kb_id=? AND version<?")
        .bind(snapshot.version)
        .bind(id)
        .bind(snapshot.version)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if updated > 0 {
        let ids: Vec<i64> = sqlx::query_scalar("WITH RECURSIVE children(id) AS (SELECT id FROM knowledge_bases WHERE id=? UNION SELECT kb.id FROM knowledge_bases kb JOIN children c ON kb.parent_id=c.id) SELECT kb.id FROM knowledge_bases kb JOIN children c ON kb.id=c.id WHERE kb.user_id='service:coassist'")
            .bind(id).fetch_all(&mut *tx).await?;
        for target in ids {
            sqlx::query("DELETE FROM kb_permissions WHERE kb_id=?").bind(target).execute(&mut *tx).await?;
            for entry in &snapshot.permissions {
                sqlx::query("INSERT INTO kb_permissions(kb_id,user_id,permission,created_at,updated_at) VALUES (?,?,?,strftime('%s','now'),strftime('%s','now')) ON CONFLICT(kb_id,user_id) DO UPDATE SET permission=excluded.permission")
                    .bind(target).bind(&entry.user_id).bind(&entry.permission).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE knowledge_bases SET is_public=0 WHERE id=?").bind(target).execute(&mut *tx).await?;
            sqlx::query("UPDATE files SET is_public=0 WHERE kb_id=?").bind(target).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    Ok(Json(serde_json::json!({"applied": updated > 0})))
}

#[cfg(test)]
mod oidc_acl_tests {
    use super::*;
    #[tokio::test]
    async fn service_acl_is_scoped_atomic_and_revocable() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE knowledge_bases(id INTEGER PRIMARY KEY,user_id TEXT,is_public INTEGER,parent_id INTEGER); CREATE TABLE kb_permissions(kb_id INTEGER,user_id TEXT,permission TEXT,created_at INTEGER,updated_at INTEGER,UNIQUE(kb_id,user_id)); CREATE TABLE files(kb_id INTEGER,is_public INTEGER); CREATE TABLE coassist_acl_versions(kb_id INTEGER PRIMARY KEY,version INTEGER); INSERT INTO knowledge_bases VALUES(1,'service:coassist',0,NULL),(2,'other',0,NULL);")
            .execute(&pool).await.unwrap();
        let service =
            AuthUser { user_id: "service:coassist".into(), user_name: "service".into(), role: "service".into() };
        let snapshot = |version| CoassistAclSnapshot {
            version,
            permissions: vec![CoassistAclEntry { user_id: "7".into(), permission: "admin".into() }],
        };
        assert!(
            replace_coassist_permissions(Path(2), State(pool.clone()), Extension(service.clone()), Json(snapshot(1)))
                .await
                .is_err()
        );
        replace_coassist_permissions(Path(1), State(pool.clone()), Extension(service.clone()), Json(snapshot(1)))
            .await
            .unwrap();
        assert_eq!(get_kb_permission(&pool, 1, "7", false).await.as_deref(), Some("admin"));
        assert!(get_kb_permission(&pool, 2, "7", false).await.is_none());
        replace_coassist_permissions(
            Path(1),
            State(pool.clone()),
            Extension(service.clone()),
            Json(CoassistAclSnapshot { version: 2, permissions: vec![] }),
        )
        .await
        .unwrap();
        assert!(get_kb_permission(&pool, 1, "7", false).await.is_none());
        replace_coassist_permissions(Path(1), State(pool.clone()), Extension(service), Json(snapshot(1)))
            .await
            .unwrap();
        assert!(get_kb_permission(&pool, 1, "7", false).await.is_none());
        let user = AuthUser { user_id: "7".into(), user_name: "user".into(), role: "user".into() };
        assert!(replace_coassist_permissions(Path(1), State(pool), Extension(user), Json(snapshot(3))).await.is_err());
    }
}

#[cfg(test)]
mod oidc_file_tests {
    use super::*;
    #[tokio::test]
    async fn removed_member_cannot_read_even_their_own_channel_file() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE knowledge_bases(id INTEGER PRIMARY KEY,user_id TEXT,is_public INTEGER); CREATE TABLE kb_permissions(kb_id INTEGER,user_id TEXT,permission TEXT); INSERT INTO knowledge_bases VALUES(1,'service:coassist',0); INSERT INTO kb_permissions VALUES(1,'7','viewer');").execute(&pool).await.unwrap();
        let user = AuthUser { user_id: "7".into(), user_name: "test".into(), role: "user".into() };
        assert!(ensure_file_readable(&pool, Some(1), "7", true, &user).await.is_ok());
        sqlx::query("DELETE FROM kb_permissions").execute(&pool).await.unwrap();
        assert!(ensure_file_readable(&pool, Some(1), "7", true, &user).await.is_err());
        assert!(ensure_file_readable(&pool, None, "7", false, &user).await.is_ok());
    }
}
