use log::warn;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::{
    AuthUser,
    api::{
        error::{ApiError, ApiResult},
        knowledge_base,
    },
};

/// 构造符合 RFC 6266 的 `Content-Disposition` 值。
///
/// 文件名可能是中文，也可能带引号或反斜杠：直接拼进 `filename="..."` 时，引号会提前结束
/// 参数（后面的内容被当成新的 disposition 参数），非 ASCII 字节在不同浏览器里还会显示成乱码。
/// 这里同时给出 ASCII 回退名和 `filename*=UTF-8''…` 百分号编码名，现代浏览器优先用后者，
/// 且返回值恒为 ASCII，`HeaderValue::from_str` 不会失败。
pub fn content_disposition(disposition_type: &str, filename: &str) -> String {
    let fallback: String = filename
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    let fallback = if fallback.trim_matches('_').is_empty() { "download".to_string() } else { fallback };
    format!("{disposition_type}; filename=\"{fallback}\"; filename*=UTF-8''{}", percent_encode(filename))
}

/// 按 RFC 3986 百分号编码：unreserved 之外的字节全部转义。
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(*byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

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

// 下面三个 helper 一律按值绑定 user_id：这样签名里不需要把 `&str` 的生命周期与
// QueryBuilder 的 `'a` 绑在一起，才能安全地用在 `Fn(&mut QueryBuilder<'_, Sqlite>)`
// 这类高阶闭包（例如分页列表的 push_filters）里。

/// 追加知识库访问过滤条件（不含别名）。
/// 生成 `AND (user_id = ? OR is_public = 1 OR id IN (SELECT kb_id FROM kb_permissions WHERE user_id = ?))`
pub fn push_kb_access_filter(qb: &mut QueryBuilder<'_, Sqlite>, user_id: &str) {
    qb.push(" AND (user_id = ");
    qb.push_bind(user_id.to_string());
    qb.push(" OR is_public = 1 OR id IN (SELECT kb_id FROM kb_permissions WHERE user_id = ");
    qb.push_bind(user_id.to_string());
    qb.push(")");
    qb.push(")");
}

/// 追加知识库访问过滤条件（带 `kb.` 别名）。
/// 生成 `WHERE (kb.user_id = ? OR kb.is_public = 1 OR kb.id IN (SELECT kb_id FROM kb_permissions WHERE user_id = ?))`
pub fn push_kb_access_filter_where(qb: &mut QueryBuilder<'_, Sqlite>, user_id: &str) {
    qb.push(" WHERE (kb.user_id = ");
    qb.push_bind(user_id.to_string());
    qb.push(" OR kb.is_public = 1 OR kb.id IN (SELECT kb_id FROM kb_permissions WHERE user_id = ");
    qb.push_bind(user_id.to_string());
    qb.push(")");
    qb.push(")");
}

/// 追加「当前用户可读取该文件」的过滤条件。
///
/// 口径与 `api::file::ensure_file_readable`、`api::search::has_visibility_permission` 完全一致：
/// - 文件所有者始终可见；
/// - 未归属知识库的散文件看自身 `is_public`；
/// - 知识库成员（属主或有 `kb_permissions` 显式授权）可见库内**全部**文件；
/// - 仅因知识库公开而可访问的陌生用户，只能看到该库中 `is_public = 1` 的文件
///   （前端有逐文件公开开关，公开库不等于库内文件全部公开）。
///
/// 调用方需自行在前面加 `AND `，并在全局 admin 场景跳过整个条件。
pub fn push_file_access_filter(qb: &mut QueryBuilder<'_, Sqlite>, user_id: &str, alias: Option<&str>) {
    let p = alias.map(|a| format!("{}.", a)).unwrap_or_default();
    // 整个条件必须是**单一括号组**：调用方以 `AND <filter>` 拼接，而 SQL 里 AND 优先级高于 OR，
    // 顶层若写成 `(A) OR (B)`，B 会逃出调用方的作用域条件（例如按 kb_id 过滤、按标签分组）。
    // 文件属主始终可见。
    qb.push(format!("({p}user_id = "));
    qb.push_bind(user_id.to_string());
    // 未归属知识库的散文件：自身公开即可见。
    qb.push(format!(" OR ({p}kb_id IS NULL AND {p}is_public = 1)"));
    // 知识库属主：库内文件全部可见。
    qb.push(format!(" OR {p}kb_id IN (SELECT id FROM knowledge_bases WHERE user_id = "));
    qb.push_bind(user_id.to_string());
    // 显式授权成员：库内文件全部可见。
    qb.push(format!(") OR {p}kb_id IN (SELECT kb_id FROM kb_permissions WHERE user_id = "));
    qb.push_bind(user_id.to_string());
    // 仅因知识库公开而来的陌生用户：只见库内公开文件。
    qb.push(format!(") OR ({p}is_public = 1 AND {p}kb_id IN (SELECT id FROM knowledge_bases WHERE is_public = 1)))"));
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
                UNION
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
                UNION
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

/// 知识库层级的最大深度。仅用于防御异常数据（例如历史遗留的父子环）导致的递归膨胀。
pub const MAX_KB_DEPTH: usize = 64;

/// 收集某个知识库的祖先链 ID（不含自身）。
///
/// 逐个父节点向上遍历并记录已访问节点，因此即使历史数据中存在父子环也会安全终止。
pub async fn collect_kb_ancestor_ids(pool: &SqlitePool, kb_id: i64) -> Result<Vec<i64>, sqlx::Error> {
    let mut ancestors: Vec<i64> = Vec::new();
    let mut visited: std::collections::HashSet<i64> = std::collections::HashSet::from([kb_id]);
    let mut current = kb_id;
    for _ in 0..MAX_KB_DEPTH {
        let parent: Option<Option<i64>> =
            sqlx::query_scalar("SELECT parent_id FROM knowledge_bases WHERE id = ?")
                .bind(current)
                .fetch_optional(pool)
                .await?;
        let Some(Some(parent_id)) = parent else { break };
        if !visited.insert(parent_id) {
            warn!("Knowledge base hierarchy cycle detected at kb_id={parent_id}, stopping ancestor walk");
            break;
        }
        ancestors.push(parent_id);
        current = parent_id;
    }
    Ok(ancestors)
}

/// 计算某个知识库在层级中的深度（根节点为 0）。带环保护。
pub async fn kb_depth(pool: &SqlitePool, kb_id: i64) -> Result<usize, sqlx::Error> {
    Ok(collect_kb_ancestor_ids(pool, kb_id).await?.len())
}

/// 判断 `candidate_parent_id` 是否是 `kb_id` 自身或其后代。
///
/// 用于在移动知识库前阻止形成父子环：一旦成环，所有基于递归 CTE 的层级查询都会失效。
pub async fn is_self_or_descendant(
    pool: &SqlitePool, kb_id: i64, candidate_parent_id: i64,
) -> Result<bool, sqlx::Error> {
    if kb_id == candidate_parent_id {
        return Ok(true);
    }
    let descendants = collect_kb_descendant_ids(pool, kb_id, false).await?;
    Ok(descendants.contains(&candidate_parent_id))
}

/// 从给定的知识库 ID 中筛出当前用户满足 `required` 权限的那些（保持输入顺序）。
pub async fn filter_kb_ids_by_permission(
    pool: &SqlitePool, kb_ids: &[i64], auth_user: &AuthUser, required: &str,
) -> Vec<i64> {
    if kb_ids.is_empty() {
        return Vec::new();
    }
    let perms = knowledge_base::get_kb_permissions_batch(pool, kb_ids, &auth_user.user_id, auth_user.is_admin()).await;
    kb_ids
        .iter()
        .copied()
        .filter(|kb_id| knowledge_base::meets_requirement(perms.get(kb_id).map(String::as_str), required))
        .collect()
}

/// 计算以 `root_kb_id` 为根的子树高度（根节点自身高度为 0）。
///
/// 在 Rust 侧基于已去重的后代集合做后序遍历，而不是用带 depth 列的递归 CTE：
/// depth 会让每行都不同，`UNION` 去重随之失效，遇到历史脏数据里的父子环会指数级膨胀。
pub async fn kb_subtree_height(pool: &SqlitePool, root_kb_id: i64) -> Result<usize, sqlx::Error> {
    use std::collections::{HashMap, HashSet};

    let ids = collect_kb_descendant_ids(pool, root_kb_id, false).await?;
    if ids.len() <= 1 {
        return Ok(0);
    }

    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    for chunk in ids.chunks(500) {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT id, parent_id FROM knowledge_bases WHERE id IN (");
        let mut separated = qb.separated(", ");
        for id in chunk {
            separated.push_bind(*id);
        }
        qb.push(")");
        let rows: Vec<(i64, Option<i64>)> = qb.build_query_as().fetch_all(pool).await?;
        for (id, parent) in rows {
            if let Some(parent) = parent.filter(|p| *p != id) {
                children.entry(parent).or_default().push(id);
            }
        }
    }

    let mut height: HashMap<i64, usize> = HashMap::new();
    let mut on_stack: HashSet<i64> = HashSet::new();
    let mut stack: Vec<(i64, bool)> = vec![(root_kb_id, false)];
    while let Some((node, expanded)) = stack.pop() {
        if expanded {
            on_stack.remove(&node);
            let child_max = children
                .get(&node)
                .and_then(|kids| kids.iter().filter_map(|kid| height.get(kid).copied()).max())
                .map(|max| max + 1)
                .unwrap_or(0);
            height.insert(node, child_max);
            continue;
        }
        if height.contains_key(&node) {
            continue;
        }
        if !on_stack.insert(node) {
            warn!("Knowledge base hierarchy cycle detected near kb_id={node}, stopping height walk");
            height.insert(node, 0);
            continue;
        }
        stack.push((node, true));
        if let Some(kids) = children.get(&node) {
            for &kid in kids {
                stack.push((kid, false));
            }
        }
    }
    Ok(height.get(&root_kb_id).copied().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 断言权限片段是「单一括号组」且括号平衡：调用方一律以 `AND <片段>` 拼接，
    /// 顶层若出现裸 `OR`，后半段会逃出调用方的作用域条件（曾导致标签统计跨库泄漏）。
    fn assert_single_group(sql: &str) {
        let trimmed = sql.trim();
        assert!(trimmed.starts_with('(') && trimmed.ends_with(')'), "not wrapped: {sql}");
        let mut depth = 0i32;
        for (index, ch) in trimmed.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    assert!(depth >= 0, "unbalanced at {index}: {sql}");
                    // 只有在最后一个字符处才允许回到 0，否则说明顶层被 OR 拆成了多段。
                    if depth == 0 {
                        assert_eq!(index + ch.len_utf8(), trimmed.len(), "top-level OR escapes group: {sql}");
                    }
                }
                _ => {}
            }
        }
        assert_eq!(depth, 0, "unbalanced: {sql}");
    }

    #[test]
    fn access_filters_stay_inside_a_single_paren_group() {
        let prefix = "SELECT 1 FROM files f WHERE f.kb_id = 1 AND ";
        let mut qb = QueryBuilder::<Sqlite>::new(prefix);
        push_file_access_filter(&mut qb, "u1", Some("f"));
        let sql = qb.into_sql();
        let filter = &sql[prefix.len()..];
        assert_single_group(filter);
        // 属主、散文件公开、库属主、显式授权、公开库公开文件：五个分支一个都不能少。
        assert_eq!(filter.matches(" OR ").count(), 4, "{filter}");
        assert_eq!(filter.matches('?').count(), 3, "{filter}");

        // 这两个 helper 自带连接词（" AND " / " WHERE "），断言时要把连接词一起算进前缀。
        let prefix = "SELECT id FROM knowledge_bases WHERE 1 = 1 AND ";
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT id FROM knowledge_bases WHERE 1 = 1");
        push_kb_access_filter(&mut qb, "u1");
        assert_single_group(&qb.into_sql()[prefix.len()..]);

        let prefix = "SELECT id FROM knowledge_bases kb WHERE ";
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT id FROM knowledge_bases kb");
        push_kb_access_filter_where(&mut qb, "u1");
        assert_single_group(&qb.into_sql()[prefix.len()..]);
    }
}
