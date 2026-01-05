use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State}, response::Json
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::api::error::ApiError;

/// 实体信息
#[derive(Debug, Serialize)]
pub struct EntityInfo {
    pub id: i64,
    pub name: String,
    pub entity_type: String,
    pub properties: HashMap<String, String>,
    pub file_id: Option<i64>,
    pub kb_id: Option<i64>,
    pub created_at: i64,
}

/// 实体搜索参数
#[derive(Debug, Deserialize)]
pub struct EntitySearchParams {
    pub q: Option<String>,           // 搜索关键词
    pub entity_type: Option<String>, // 实体类型筛选
    pub kb_id: Option<i64>,          // 知识库ID筛选
    pub file_id: Option<i64>,        // 文件ID筛选
    pub limit: Option<i64>,          // 返回数量限制
}

/// 实体查询
pub async fn search_entities(
    Query(params): Query<EntitySearchParams>, State(pool): State<SqlitePool>,
) -> Result<Json<Vec<EntityInfo>>, ApiError> {
    let mut sql =
        "SELECT id, name, entity_type, properties, file_id, kb_id, created_at FROM graph_nodes WHERE 1=1".to_string();

    // 构建查询条件
    if params.q.is_some() {
        sql.push_str(" AND name LIKE '%' || ? || '%'");
    }
    if params.entity_type.is_some() {
        sql.push_str(" AND entity_type = ?");
    }
    if params.kb_id.is_some() {
        sql.push_str(" AND kb_id = ?");
    }
    if params.file_id.is_some() {
        sql.push_str(" AND file_id = ?");
    }

    sql.push_str(" ORDER BY created_at DESC");

    if let Some(limit) = params.limit {
        sql.push_str(&format!(" LIMIT {}", limit));
    } else {
        sql.push_str(" LIMIT 100");
    }

    // 执行查询
    let mut query = sqlx::query_as::<_, (i64, String, String, Option<String>, Option<i64>, Option<i64>, i64)>(&sql);

    if let Some(ref q) = params.q {
        query = query.bind(q);
    }
    if let Some(ref entity_type) = params.entity_type {
        query = query.bind(entity_type);
    }
    if let Some(kb_id) = params.kb_id {
        query = query.bind(kb_id);
    }
    if let Some(file_id) = params.file_id {
        query = query.bind(file_id);
    }

    let rows = query.fetch_all(&pool).await?;

    let entities: Vec<EntityInfo> = rows
        .into_iter()
        .map(|(id, name, entity_type, properties_json, file_id, kb_id, created_at)| {
            let properties: HashMap<String, String> = if let Some(ref json) = properties_json {
                serde_json::from_str(json).unwrap_or_default()
            } else {
                HashMap::new()
            };

            EntityInfo { id, name, entity_type, properties, file_id, kb_id, created_at }
        })
        .collect();

    Ok(Json(entities))
}

/// 实体详情（包含邻居和提及）
#[derive(Debug, Serialize)]
pub struct EntityDetail {
    pub entity: EntityInfo,
    pub neighbors: Vec<NeighborInfo>,
    pub mentions: Vec<MentionInfo>,
}

#[derive(Debug, Serialize)]
pub struct NeighborInfo {
    pub entity: EntityInfo,
    pub relation_type: String,
    pub direction: String, // "outgoing" or "incoming"
}

#[derive(Debug, Serialize)]
pub struct MentionInfo {
    pub slice_id: i64,
    pub context: String,
    pub file_id: i64,
    pub filename: String,
}

/// 获取实体详情
pub async fn get_entity(Path(id): Path<i64>, State(pool): State<SqlitePool>) -> Result<Json<EntityDetail>, ApiError> {
    // 查询实体基本信息
    let entity_sql =
        "SELECT id, name, entity_type, properties, file_id, kb_id, created_at FROM graph_nodes WHERE id = ?";
    let entity_row: Option<(i64, String, String, Option<String>, Option<i64>, Option<i64>, i64)> =
        sqlx::query_as(entity_sql).bind(id).fetch_optional(&pool).await?;

    let entity_row = entity_row.ok_or_else(|| ApiError::Internal("Entity not found".to_string()))?;

    let properties: HashMap<String, String> =
        if let Some(ref json) = entity_row.3 { serde_json::from_str(json).unwrap_or_default() } else { HashMap::new() };

    let entity = EntityInfo {
        id: entity_row.0,
        name: entity_row.1,
        entity_type: entity_row.2,
        properties,
        file_id: entity_row.4,
        kb_id: entity_row.5,
        created_at: entity_row.6,
    };

    // 查询邻居（出边）
    let outgoing_sql = "SELECT n.id, n.name, n.entity_type, n.properties, n.file_id, n.kb_id, n.created_at, e.relation_type \
                        FROM graph_edges e \
                        INNER JOIN graph_nodes n ON e.target_node_id = n.id \
                        WHERE e.source_node_id = ? LIMIT 50";
    let outgoing_rows: Vec<(i64, String, String, Option<String>, Option<i64>, Option<i64>, i64, String)> =
        sqlx::query_as(outgoing_sql).bind(id).fetch_all(&pool).await?;

    // 查询邻居（入边）
    let incoming_sql = "SELECT n.id, n.name, n.entity_type, n.properties, n.file_id, n.kb_id, n.created_at, e.relation_type \
                        FROM graph_edges e \
                        INNER JOIN graph_nodes n ON e.source_node_id = n.id \
                        WHERE e.target_node_id = ? LIMIT 50";
    let incoming_rows: Vec<(i64, String, String, Option<String>, Option<i64>, Option<i64>, i64, String)> =
        sqlx::query_as(incoming_sql).bind(id).fetch_all(&pool).await?;

    let mut neighbors = Vec::new();

    for row in outgoing_rows {
        let properties: HashMap<String, String> =
            if let Some(ref json) = row.3 { serde_json::from_str(json).unwrap_or_default() } else { HashMap::new() };

        neighbors.push(NeighborInfo {
            entity: EntityInfo {
                id: row.0,
                name: row.1,
                entity_type: row.2,
                properties,
                file_id: row.4,
                kb_id: row.5,
                created_at: row.6,
            },
            relation_type: row.7,
            direction: "outgoing".to_string(),
        });
    }

    for row in incoming_rows {
        let properties: HashMap<String, String> =
            if let Some(ref json) = row.3 { serde_json::from_str(json).unwrap_or_default() } else { HashMap::new() };

        neighbors.push(NeighborInfo {
            entity: EntityInfo {
                id: row.0,
                name: row.1,
                entity_type: row.2,
                properties,
                file_id: row.4,
                kb_id: row.5,
                created_at: row.6,
            },
            relation_type: row.7,
            direction: "incoming".to_string(),
        });
    }

    // 查询提及
    let mentions_sql = "SELECT m.slice_id, m.context, s.file_id, f.filename \
                        FROM entity_mentions m \
                        INNER JOIN slices s ON m.slice_id = s.id \
                        INNER JOIN files f ON s.file_id = f.id \
                        WHERE m.node_id = ? LIMIT 20";
    let mentions_rows: Vec<(i64, String, i64, String)> = sqlx::query_as(mentions_sql).bind(id).fetch_all(&pool).await?;

    let mentions: Vec<MentionInfo> = mentions_rows
        .into_iter()
        .map(|(slice_id, context, file_id, filename)| MentionInfo { slice_id, context, file_id, filename })
        .collect();

    Ok(Json(EntityDetail { entity, neighbors, mentions }))
}

/// 图统计信息
#[derive(Debug, Serialize)]
pub struct GraphStats {
    pub node_count: i64,
    pub edge_count: i64,
    pub entity_types: HashMap<String, i64>,
    pub relation_types: HashMap<String, i64>,
}

#[derive(Debug, Deserialize)]
pub struct StatsParams {
    pub kb_id: Option<i64>,
    pub file_id: Option<i64>,
}

/// 获取图统计信息
pub async fn get_graph_stats(
    Query(params): Query<StatsParams>, State(pool): State<SqlitePool>,
) -> Result<Json<GraphStats>, ApiError> {
    // 构建 WHERE 子句
    let where_clause = if let Some(file_id) = params.file_id {
        format!("WHERE file_id = {}", file_id)
    } else if let Some(kb_id) = params.kb_id {
        format!("WHERE kb_id = {}", kb_id)
    } else {
        "".to_string()
    };

    // 节点数量
    let node_count_sql = format!("SELECT COUNT(*) FROM graph_nodes {}", where_clause);
    let node_count: (i64,) = sqlx::query_as(&node_count_sql).fetch_one(&pool).await?;

    // 边数量
    let edge_count_sql = if let Some(file_id) = params.file_id {
        format!(
            "SELECT COUNT(*) FROM graph_edges WHERE file_id = {}",
            file_id
        )
    } else if let Some(kb_id) = params.kb_id {
        format!(
            "SELECT COUNT(*) FROM graph_edges e \
             INNER JOIN graph_nodes n ON e.source_node_id = n.id \
             WHERE n.kb_id = {}",
            kb_id
        )
    } else {
        "SELECT COUNT(*) FROM graph_edges".to_string()
    };

    let edge_count: (i64,) = sqlx::query_as(&edge_count_sql).fetch_one(&pool).await?;

    // 实体类型分布
    let entity_types_sql =
        format!("SELECT entity_type, COUNT(*) FROM graph_nodes {} GROUP BY entity_type", where_clause);
    let entity_types_rows: Vec<(String, i64)> = sqlx::query_as(&entity_types_sql).fetch_all(&pool).await?;
    let entity_types: HashMap<String, i64> = entity_types_rows.into_iter().collect();

    // 关系类型分布
    let relation_types_sql = if let Some(file_id) = params.file_id {
        format!(
            "SELECT relation_type, COUNT(*) FROM graph_edges \
             WHERE file_id = {} GROUP BY relation_type",
            file_id
        )
    } else if let Some(kb_id) = params.kb_id {
        format!(
            "SELECT e.relation_type, COUNT(*) FROM graph_edges e \
             INNER JOIN graph_nodes n ON e.source_node_id = n.id \
             WHERE n.kb_id = {} GROUP BY e.relation_type",
            kb_id
        )
    } else {
        "SELECT relation_type, COUNT(*) FROM graph_edges GROUP BY relation_type".to_string()
    };

    let relation_types_rows: Vec<(String, i64)> = sqlx::query_as(&relation_types_sql).fetch_all(&pool).await?;
    let relation_types: HashMap<String, i64> = relation_types_rows.into_iter().collect();

    Ok(Json(GraphStats { node_count: node_count.0, edge_count: edge_count.0, entity_types, relation_types }))
}
