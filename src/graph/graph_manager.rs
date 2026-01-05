use std::collections::HashMap;

use anyhow::Result;
use petgraph::graph::{DiGraph, NodeIndex};
use sqlx::SqlitePool;

use super::{Edge, Entity, EntityType, Node, Relation, RelationType};

/// 知识图谱管理器
pub struct KnowledgeGraph {
    graph: DiGraph<Node, Edge>,
    node_index: HashMap<String, NodeIndex>,
    node_id_to_index: HashMap<i64, NodeIndex>,
    pool: SqlitePool,
    kb_id: Option<i64>,
}

impl KnowledgeGraph {
    /// 从数据库加载图
    pub async fn load_from_db(pool: SqlitePool, kb_id: Option<i64>) -> Result<Self> {
        let mut graph = DiGraph::new();
        let mut node_index = HashMap::new();
        let mut node_id_to_index = HashMap::new();

        let where_clause = if let Some(kb_id) = kb_id { format!("WHERE kb_id = {}", kb_id) } else { "".to_string() };

        // 加载节点
        let nodes_sql = format!("SELECT id, name, entity_type, properties FROM graph_nodes {}", where_clause);
        let nodes: Vec<(i64, String, String, Option<String>)> = sqlx::query_as(&nodes_sql).fetch_all(&pool).await?;

        for (id, name, entity_type_str, properties_json) in nodes {
            let entity_type = EntityType::Custom(entity_type_str);
            let properties: HashMap<String, String> = if let Some(json) = properties_json {
                serde_json::from_str(&json).unwrap_or_default()
            } else {
                HashMap::new()
            };

            let node = Node { id, name: name.clone(), entity_type, properties };

            let idx = graph.add_node(node);
            node_index.insert(name, idx);
            node_id_to_index.insert(id, idx);
        }

        // 加载边
        let edges_sql = format!(
            "SELECT e.id, e.source_node_id, e.target_node_id, e.relation_type, e.properties, e.weight \
             FROM graph_edges e \
             INNER JOIN graph_nodes n1 ON e.source_node_id = n1.id \
             INNER JOIN graph_nodes n2 ON e.target_node_id = n2.id \
             {}",
            where_clause
        );
        let edges: Vec<(i64, i64, i64, String, Option<String>, f32)> =
            sqlx::query_as(&edges_sql).fetch_all(&pool).await?;

        for (id, source_id, target_id, relation_type_str, properties_json, weight) in edges {
            if let (Some(&source_idx), Some(&target_idx)) =
                (node_id_to_index.get(&source_id), node_id_to_index.get(&target_id))
            {
                let relation_type = RelationType::Custom(relation_type_str);
                let properties: HashMap<String, String> = if let Some(json) = properties_json {
                    serde_json::from_str(&json).unwrap_or_default()
                } else {
                    HashMap::new()
                };

                let edge = Edge { id, relation_type, weight, properties };
                graph.add_edge(source_idx, target_idx, edge);
            }
        }

        Ok(Self { graph, node_index, node_id_to_index, pool, kb_id })
    }

    /// 添加节点
    pub async fn add_node(&mut self, entity: &Entity) -> Result<NodeIndex> {
        if let Some(&idx) = self.node_index.get(&entity.name) {
            return Ok(idx);
        }

        let entity_type_str = entity.entity_type.as_str();
        let properties_json = serde_json::to_string(&entity.properties)?;

        let sql = "INSERT INTO graph_nodes (name, entity_type, properties, file_id, kb_id) \
                   VALUES (?, ?, ?, ?, ?) \
                   ON CONFLICT(name, entity_type, kb_id) DO UPDATE SET \
                   properties = excluded.properties, \
                   updated_at = strftime('%s','now') \
                   RETURNING id";

        let id: (i64,) = sqlx::query_as(sql)
            .bind(&entity.name)
            .bind(entity_type_str)
            .bind(&properties_json)
            .bind(entity.file_id)
            .bind(entity.kb_id.or(self.kb_id))
            .fetch_one(&self.pool)
            .await?;

        let node = Node::from_entity(entity, id.0);
        let idx = self.graph.add_node(node);
        self.node_index.insert(entity.name.clone(), idx);
        self.node_id_to_index.insert(id.0, idx);

        Ok(idx)
    }

    /// 添加边
    pub async fn add_edge(
        &mut self, source_name: &str, target_name: &str, relation: &Relation,
    ) -> Result<Option<petgraph::graph::EdgeIndex>> {
        let source_idx = match self.node_index.get(source_name) {
            Some(&idx) => idx,
            None => return Ok(None),
        };

        let target_idx = match self.node_index.get(target_name) {
            Some(&idx) => idx,
            None => return Ok(None),
        };

        let source_id = self.graph[source_idx].id;
        let target_id = self.graph[target_idx].id;

        let relation_type_str = relation.relation_type.as_str();
        let properties_json = serde_json::to_string(&relation.properties)?;

        let sql = "INSERT INTO graph_edges (source_node_id, target_node_id, relation_type, properties, weight, file_id) \
                   VALUES (?, ?, ?, ?, ?, ?) \
                   RETURNING id";

        let id: (i64,) = sqlx::query_as(sql)
            .bind(source_id)
            .bind(target_id)
            .bind(relation_type_str)
            .bind(&properties_json)
            .bind(relation.weight)
            .bind(relation.file_id)
            .fetch_one(&self.pool)
            .await?;

        let edge = Edge::from_relation(relation, id.0);
        let edge_idx = self.graph.add_edge(source_idx, target_idx, edge);

        Ok(Some(edge_idx))
    }

    /// 增量更新
    pub async fn incremental_update(&mut self, entities: Vec<Entity>, relations: Vec<Relation>) -> Result<()> {
        for entity in &entities {
            self.add_node(entity).await?;
        }

        for relation in &relations {
            self.add_edge(&relation.source_name, &relation.target_name, relation).await?;
        }

        Ok(())
    }

    /// 保存图快照
    pub async fn save_snapshot(&self) -> Result<()> {
        let node_count = self.graph.node_count() as i64;
        let edge_count = self.graph.edge_count() as i64;

        let delete_sql = "DELETE FROM graph_snapshots WHERE kb_id IS ?";
        sqlx::query(delete_sql).bind(self.kb_id).execute(&self.pool).await?;

        let insert_sql = "INSERT INTO graph_snapshots (kb_id, graph_data, node_count, edge_count) \
                          VALUES (?, ?, ?, ?)";
        sqlx::query(insert_sql)
            .bind(self.kb_id)
            .bind(Vec::<u8>::new())
            .bind(node_count)
            .bind(edge_count)
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}
