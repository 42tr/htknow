use std::collections::HashSet;

use sqlx::{QueryBuilder, Sqlite, SqlitePool};

/// 参与子串枚举的查询前缀长度（字符）。
const MAX_QUERY_CHARS: usize = 32;
/// 候选子串的最小长度。单字候选几乎只会命中噪声节点，完整查询本身已单独保留。
const MIN_TERM_CHARS: usize = 2;
/// 候选子串的最大长度，与实体名的实际长度上限一致。
const MAX_TERM_CHARS: usize = 32;

/// Generate bounded exact-name candidates from a question, so the name index can be used.
/// Full query plus Unicode substrings cover embedded Chinese names without scanning every node.
///
/// 候选数量必须是常量级：这些字符串会逐个变成 `name IN (...)` 的绑定参数，
/// 而 `expand` 在每次搜索里都会跑。原先取查询前 128 个字符的所有 1..=32 长子串，
/// 最坏情况一次生成 4096 个参数，SQLite 光是解析和探测索引就要花掉数毫秒。
/// 收窄到前 32 字符、2..=32 长度后上界是 496 个（常见短查询只有几十个），召回口径不变：
/// 实体名超出前 32 字符的查询极少，且完整查询串始终作为候选保留。
fn candidates(query: &str) -> Vec<String> {
    let trimmed = query.trim();
    let chars: Vec<char> = trimmed.chars().take(MAX_QUERY_CHARS).collect();
    let mut result = HashSet::from([trimmed.to_owned()]);
    for len in MIN_TERM_CHARS..=MAX_TERM_CHARS {
        for start in 0..chars.len().saturating_sub(len - 1) {
            let end = start + len;
            if end > chars.len() {
                break;
            }
            let term: String = chars[start..end].iter().collect();
            if !term.trim().is_empty() {
                result.insert(term);
            }
        }
    }
    let mut result: Vec<_> = result.into_iter().collect();
    result.sort();
    result
}
fn restrict(q: &mut QueryBuilder<'_, Sqlite>, column: &str, ids: Option<&[i64]>) {
    if let Some(ids) = ids {
        q.push(format!(" AND {column} IN ("));
        if ids.is_empty() {
            q.push("NULL");
        } else {
            let mut separated = q.separated(",");
            for &id in ids {
                separated.push_bind(id);
            }
        }
        q.push(")");
    }
}

pub async fn expand(
    pool: &SqlitePool, query: &str, kb_ids: Option<&[i64]>, file_ids: Option<&[i64]>,
) -> anyhow::Result<Vec<String>> {
    let mut result = vec![query.to_owned()];
    if query.trim().is_empty() || kb_ids.is_some_and(|ids| ids.is_empty()) || file_ids.is_some_and(|ids| ids.is_empty())
    {
        return Ok(result);
    }
    let mut qb = QueryBuilder::<Sqlite>::new("SELECT n.id,n.name,n.kb_id FROM graph_nodes n WHERE n.name IN (");
    let mut separated = qb.separated(",");
    for candidate in candidates(query) {
        separated.push_bind(candidate);
    }
    qb.push(")");
    restrict(&mut qb, "n.kb_id", kb_ids);
    if file_ids.is_some() {
        qb.push(" AND EXISTS(SELECT 1 FROM graph_node_sources s WHERE s.node_id=n.id");
        restrict(&mut qb, "s.file_id", file_ids);
        qb.push(")");
    }
    qb.push(" ORDER BY length(n.name) DESC,n.id LIMIT 3");
    let seeds: Vec<(i64, String, Option<i64>)> = qb.build_query_as().fetch_all(pool).await?;
    for (id, name, kb_id) in seeds {
        if !result.contains(&name) {
            result.push(name);
        }
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT DISTINCT n.name FROM graph_edges e JOIN graph_nodes n ON n.id=CASE WHEN e.source_node_id=",
        );
        qb.push_bind(id)
            .push(" THEN e.target_node_id ELSE e.source_node_id END WHERE (e.source_node_id=")
            .push_bind(id)
            .push(" OR e.target_node_id=")
            .push_bind(id)
            .push(") AND n.kb_id IS ")
            .push_bind(kb_id);
        restrict(&mut qb, "n.kb_id", kb_ids);
        restrict(&mut qb, "e.file_id", file_ids);
        qb.push(
            " AND (e.file_id IS NULL OR EXISTS(SELECT 1 FROM files f WHERE f.id=e.file_id AND f.kb_id IS n.kb_id))",
        );
        qb.push(" AND n.id != ").push_bind(id).push(" ORDER BY n.name LIMIT 5");
        let names: Vec<String> = qb.build_query_scalar().fetch_all(pool).await?;
        for name in names {
            if !result.contains(&name) {
                result.push(name);
            }
        }
    }
    Ok(result)
}

#[derive(sqlx::FromRow)]
pub struct Evidence {
    pub slice_id: i64,
    pub file_id: i64,
    pub kb_id: Option<i64>,
    pub context: String,
}

pub async fn evidence(
    pool: &SqlitePool, names: &[String], kb_ids: Option<&[i64]>, file_ids: Option<&[i64]>,
) -> anyhow::Result<Vec<Evidence>> {
    if names.is_empty() || kb_ids.is_some_and(|ids| ids.is_empty()) || file_ids.is_some_and(|ids| ids.is_empty()) {
        return Ok(vec![]);
    }
    let mut qb = QueryBuilder::<Sqlite>::new(
        "SELECT s.slice_id,s.file_id,f.kb_id,s.context FROM graph_node_sources s \
        JOIN graph_nodes n ON n.id=s.node_id JOIN files f ON f.id=s.file_id \
        WHERE s.slice_id IS NOT NULL AND s.context<>'' AND f.kb_id IS n.kb_id AND n.name IN (",
    );
    let mut separated = qb.separated(",");
    for name in names {
        separated.push_bind(name.clone());
    }
    qb.push(")");
    restrict(&mut qb, "f.kb_id", kb_ids);
    restrict(&mut qb, "s.file_id", file_ids);
    qb.push(" ORDER BY s.file_id,s.slice_id,s.id LIMIT 40");
    Ok(qb.build_query_as().fetch_all(pool).await?)
}
