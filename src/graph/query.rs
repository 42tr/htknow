use std::collections::HashSet;

use sqlx::{QueryBuilder, Sqlite, SqlitePool};

/// Generate bounded exact-name candidates from a question, so the name index can be used.
/// Full query plus Unicode substrings cover embedded Chinese names without scanning every node.
fn candidates(query: &str) -> Vec<String> {
    let chars: Vec<char> = query.trim().chars().take(128).collect();
    let mut result = HashSet::from([query.trim().to_owned()]);
    for start in 0..chars.len() {
        for end in (start + 1)..=(start + 32).min(chars.len()) {
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
