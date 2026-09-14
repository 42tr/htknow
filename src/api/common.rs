use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::{
    AuthUser, api::{
        error::{ApiError, ApiResult}, knowledge_base
    }
};

/// 要求当前用户为 admin，否则返回 BadRequest。
pub fn ensure_admin(auth_user: &AuthUser) -> ApiResult<()> {
    if auth_user.is_admin() { Ok(()) } else { Err(ApiError::BadRequest("admin role required".to_string())) }
}

/// 校验用户能否访问指定知识库（owner / public / 显式授权）。不存在或无权限返回 NotFound。
pub async fn ensure_kb_accessible(pool: &SqlitePool, kb_id: i64, user_id: &str, is_admin: bool) -> ApiResult<()> {
    let perm = knowledge_base::get_kb_permission(pool, kb_id, user_id, is_admin).await;
    if perm.is_none() {
        return Err(ApiError::NotFound("Knowledge base not found or permission denied".to_string()));
    }
    Ok(())
}

/// 校验用户是否为指定知识库的 editor / admin。无权限返回 Forbidden。
pub async fn ensure_kb_editor_or_admin(pool: &SqlitePool, kb_id: i64, auth_user: &AuthUser) -> ApiResult<()> {
    let perm = knowledge_base::get_kb_permission(pool, kb_id, &auth_user.user_id, auth_user.is_admin()).await;
    if !knowledge_base::meets_requirement(perm.as_deref(), "editor") {
        return Err(ApiError::Forbidden("Permission denied. Requires editor or admin.".to_string()));
    }
    Ok(())
}

/// 追加知识库访问过滤条件（不含别名）。
/// 生成 `AND (user_id = ? OR is_public = 1 OR id IN (SELECT kb_id FROM kb_permissions WHERE user_id = ?))`
pub fn push_kb_access_filter<'a>(qb: &mut QueryBuilder<'a, Sqlite>, user_id: &'a str) {
    qb.push(" AND (user_id = ");
    qb.push_bind(user_id);
    qb.push(" OR is_public = 1 OR id IN (SELECT kb_id FROM kb_permissions WHERE user_id = ");
    qb.push_bind(user_id);
    qb.push(")");
    qb.push(")");
}

/// 追加知识库访问过滤条件（带 `kb.` 别名）。
/// 生成 `WHERE (kb.user_id = ? OR kb.is_public = 1 OR kb.id IN (SELECT kb_id FROM kb_permissions WHERE user_id = ?))`
pub fn push_kb_access_filter_where<'a>(qb: &mut QueryBuilder<'a, Sqlite>, user_id: &'a str) {
    qb.push(" WHERE (kb.user_id = ");
    qb.push_bind(user_id);
    qb.push(" OR kb.is_public = 1 OR kb.id IN (SELECT kb_id FROM kb_permissions WHERE user_id = ");
    qb.push_bind(user_id);
    qb.push(")");
    qb.push(")");
}

/// 追加文件访问过滤条件。
/// 生成 `(f.user_id = ? OR f.is_public = 1)`；调用方需自行在前面加 `AND `。
pub fn push_file_access_filter<'a>(qb: &mut QueryBuilder<'a, Sqlite>, user_id: &'a str, alias: Option<&'a str>) {
    let col_prefix = alias.map(|a| format!("{}.", a)).unwrap_or_default();
    qb.push("(");
    qb.push(format!("{}user_id", col_prefix));
    qb.push(" = ");
    qb.push_bind(user_id);
    qb.push(format!(" OR {}is_public = 1", col_prefix));
    qb.push(")");
}

/// 收集某个知识库的所有后代知识库 ID（包含自身）。
/// 若 `exclude_storage` 为 true，则结果中过滤掉 `kb_type = 'storage'` 的节点。
pub async fn collect_kb_descendant_ids(
    pool: &SqlitePool, root_kb_id: i64, exclude_storage: bool,
) -> Result<Vec<i64>, sqlx::Error> {
    let rows: Vec<(i64,)> = if exclude_storage {
        sqlx::query_as(
            r#"
            WITH RECURSIVE descendants AS (
                SELECT id, kb_type FROM knowledge_bases WHERE id = ?
                UNION ALL
                SELECT kb.id, kb.kb_type
                FROM knowledge_bases kb
                INNER JOIN descendants d ON kb.parent_id = d.id
            )
            SELECT id FROM descendants WHERE kb_type != ?;
            "#,
        )
        .bind(root_kb_id)
        .bind(knowledge_base::KB_TYPE_STORAGE)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as(
            r#"
            WITH RECURSIVE descendants AS (
                SELECT id FROM knowledge_bases WHERE id = ?
                UNION ALL
                SELECT kb.id
                FROM knowledge_bases kb
                INNER JOIN descendants d ON kb.parent_id = d.id
            )
            SELECT id FROM descendants;
            "#,
        )
        .bind(root_kb_id)
        .fetch_all(pool)
        .await?
    };
    Ok(rows.into_iter().map(|(id,)| id).collect())
}
