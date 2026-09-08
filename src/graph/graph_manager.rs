use std::collections::HashMap;

use anyhow::{Result, ensure};
use sqlx::{SqliteConnection, SqlitePool};

use super::{Entity, Relation};

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    let mut tx = pool.begin().await?;
    let claimed = sqlx::query("INSERT OR IGNORE INTO schema_migrations(version, name) VALUES (5, 'graph_provenance')")
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if claimed != 0 {
        sqlx::raw_sql(include_str!("migration.sql")).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Offsets are UTF-8 byte offsets in the original slice, never in concatenated documents.
pub struct ExtractedChunk {
    pub slice_id: i64,
    pub offset: usize,
    pub text: String,
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
}

/// Split even a single oversized slice. Overlap helps preserve relationships across boundaries.
pub fn extraction_chunks(text: &str) -> Vec<(usize, String)> {
    let boundaries: Vec<usize> = text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len())).collect();
    let mut chunks = Vec::new();
    let mut start = 0;
    while start + 1 < boundaries.len() {
        let end = (start + 2000).min(boundaries.len() - 1);
        let content = &text[boundaries[start]..boundaries[end]];
        if !content.trim().is_empty() {
            chunks.push((boundaries[start], content.to_owned()));
        }
        if end == boundaries.len() - 1 {
            break;
        }
        start = end - 100;
    }
    chunks
}

/// Revoke one document's contributions; shared entities are retained.
pub async fn clear_file(conn: &mut SqliteConnection, file_id: i64) -> Result<(), sqlx::Error> {
    clear_files(conn, &[file_id]).await
}

pub async fn clear_files(conn: &mut SqliteConnection, file_ids: &[i64]) -> Result<(), sqlx::Error> {
    for chunk in file_ids.chunks(500) {
        // IDs are typed integers, never user-supplied SQL text.
        let ids = chunk.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
        sqlx::query(&format!(
            "DELETE FROM entity_mentions WHERE slice_id IN (SELECT id FROM slices WHERE file_id IN ({ids}))"
        ))
        .execute(&mut *conn)
        .await?;
        for table in ["graph_edges", "graph_node_sources", "graph_builds"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE file_id IN ({ids})")).execute(&mut *conn).await?;
        }
        prune(conn, &ids).await?;
    }
    Ok(())
}

async fn prune(conn: &mut SqliteConnection, file_ids_sql: &str) -> Result<(), sqlx::Error> {
    sqlx::query(&format!(
        "DELETE FROM graph_nodes WHERE file_id IN ({file_ids_sql}) AND NOT EXISTS \
        (SELECT 1 FROM graph_node_sources s WHERE s.node_id=graph_nodes.id) AND NOT EXISTS \
        (SELECT 1 FROM graph_edges e WHERE e.source_node_id=graph_nodes.id OR e.target_node_id=graph_nodes.id)"
    ))
    .execute(&mut *conn)
    .await?;
    sqlx::query(&format!("UPDATE graph_nodes SET file_id=(SELECT MIN(s.file_id) FROM graph_node_sources s WHERE s.node_id=graph_nodes.id), \
        properties=COALESCE((SELECT s.properties FROM graph_node_sources s WHERE s.node_id=graph_nodes.id ORDER BY s.file_id,s.id LIMIT 1), '{{}}') \
        WHERE file_id IN ({file_ids_sql})"))
        .execute(&mut *conn).await?;
    Ok(())
}

pub struct KnowledgeGraph;
impl KnowledgeGraph {
    /// All LLM work finishes before acquiring the writer lock. A failed/stale build preserves the old graph.
    pub async fn replace_file(
        pool: &SqlitePool, file_id: i64, kb_id: Option<i64>, run_id: &str, chunks: Vec<ExtractedChunk>,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let claimed = sqlx::query(
            "UPDATE graph_builds SET status='completed', error=NULL, updated_at=strftime('%s','now') \
            WHERE file_id=? AND run_id=? AND status='running' AND EXISTS \
            (SELECT 1 FROM files f WHERE f.id=? AND f.kb_id IS ? AND f.status=1)",
        )
        .bind(file_id)
        .bind(run_id)
        .bind(file_id)
        .bind(kb_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        ensure!(claimed == 1, "graph build superseded or file changed");
        sqlx::query("DELETE FROM entity_mentions WHERE slice_id IN (SELECT id FROM slices WHERE file_id=?)")
            .bind(file_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM graph_edges WHERE file_id=?").bind(file_id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM graph_node_sources WHERE file_id=?").bind(file_id).execute(&mut *tx).await?;
        for chunk in chunks {
            // A reparse or artifact replacement must not publish references to obsolete slices.
            let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM slices s JOIN files f ON f.id=? \
                LEFT JOIN parse_artifacts a ON a.id=f.artifact_id WHERE s.id=? AND s.file_id=COALESCE(a.source_file_id,f.id))")
                .bind(file_id).bind(chunk.slice_id).fetch_one(&mut *tx).await?;
            ensure!(valid, "source slice changed during graph extraction");
            let mut names: HashMap<String, Vec<(String, i64)>> = HashMap::new();
            for entity in chunk.entities {
                let name = entity.name.trim();
                let kind = entity.entity_type.as_str();
                if name.is_empty() || kind.trim().is_empty() {
                    continue;
                }
                // Ground entities in actual source text, not an unsupported LLM description.
                let Some(position) = chunk.text.find(name) else {
                    continue;
                };
                let properties = serde_json::to_string(&entity.properties)?;
                // IS handles nullable KBs; the transaction serializes this check and insert.
                let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM graph_nodes WHERE name=? AND entity_type=? AND kb_id IS ? AND (? IS NOT NULL OR file_id=?) ORDER BY id LIMIT 1")
                    .bind(name).bind(kind.trim()).bind(kb_id).bind(kb_id).bind(file_id).fetch_optional(&mut *tx).await?;
                let id = if let Some(id) = existing {
                    id
                } else {
                    sqlx::query_scalar("INSERT INTO graph_nodes(name,entity_type,properties,file_id,kb_id) VALUES(?,?,?,?,?) RETURNING id")
                        .bind(name).bind(kind.trim()).bind(&properties).bind(file_id).bind(kb_id).fetch_one(&mut *tx).await?
                };
                names.entry(name.to_owned()).or_default().push((kind.trim().to_owned(), id));
                let context_start = chunk.text[..position].char_indices().rev().nth(100).map(|(i, _)| i).unwrap_or(0);
                let context_end = chunk.text[position + name.len()..]
                    .char_indices()
                    .nth(100)
                    .map(|(i, _)| position + name.len() + i)
                    .unwrap_or(chunk.text.len());
                let context = &chunk.text[context_start..context_end];
                sqlx::query("INSERT OR IGNORE INTO graph_node_sources(node_id,file_id,slice_id,start_offset,end_offset,context,properties) VALUES(?,?,?,?,?,?,?)")
                    .bind(id).bind(file_id).bind(chunk.slice_id).bind((chunk.offset+position) as i64)
                    .bind((chunk.offset+position+name.len()) as i64).bind(context).bind(&properties).execute(&mut *tx).await?;
            }
            for relation in chunk.relations {
                let resolve = |name: &str, kind: &Option<String>| -> Option<i64> {
                    let mut ids: Vec<i64> = names
                        .get(name.trim())?
                        .iter()
                        .filter(|(t, _)| kind.as_ref().is_none_or(|k| k.trim() == t))
                        .map(|(_, id)| *id)
                        .collect();
                    ids.sort_unstable();
                    ids.dedup();
                    if ids.len() == 1 { Some(ids[0]) } else { None }
                };
                let (Some(source), Some(target)) = (
                    resolve(&relation.source_name, &relation.source_type),
                    resolve(&relation.target_name, &relation.target_type),
                ) else {
                    log::warn!("Skipping unresolved/ambiguous graph relation in file {}", file_id);
                    continue;
                };
                let kind = relation.relation_type.as_str();
                if kind.trim().is_empty() {
                    continue;
                }
                // Only quotations validated against source text can serve as relation evidence.
                let Some(evidence) =
                    relation.evidence.as_deref().filter(|s| !s.trim().is_empty() && chunk.text.contains(*s))
                else {
                    continue;
                };
                let id: i64 = sqlx::query_scalar("INSERT INTO graph_edges(source_node_id,target_node_id,relation_type,properties,weight,file_id) VALUES(?,?,?,?,?,?) \
                    ON CONFLICT DO UPDATE SET weight=excluded.weight RETURNING id")
                    .bind(source).bind(target).bind(kind.trim()).bind(serde_json::to_string(&relation.properties)?)
                    .bind(relation.weight).bind(file_id).fetch_one(&mut *tx).await?;
                sqlx::query("INSERT OR IGNORE INTO graph_edge_sources(edge_id,slice_id,context) VALUES(?,?,?)")
                    .bind(id)
                    .bind(chunk.slice_id)
                    .bind(evidence)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        prune(&mut tx, &file_id.to_string()).await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_unicode_slice_keeps_tail_and_offsets() {
        let text = format!("{}末尾实体", "中文".repeat(6000));
        let chunks = extraction_chunks(&text);
        assert!(chunks.len() > 1);
        assert!(chunks.last().unwrap().1.ends_with("末尾实体"));
        for (offset, chunk) in chunks {
            assert_eq!(&text[offset..offset + chunk.len()], chunk);
        }
        assert!(extraction_chunks("").is_empty());
    }
}
