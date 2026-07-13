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

        // 加载节点
        let nodes: Vec<(i64, String, String, Option<String>)> = if let Some(kb_id) = kb_id {
            sqlx::query_as("SELECT id, name, entity_type, properties FROM graph_nodes WHERE kb_id = ?")
                .bind(kb_id)
                .fetch_all(&pool)
                .await?
        } else {
            sqlx::query_as("SELECT id, name, entity_type, properties FROM graph_nodes").fetch_all(&pool).await?
        };

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

        // 加载边（使用 n1.kb_id 避免歧义）
        let edges: Vec<(i64, i64, i64, String, Option<String>, f32)> = if let Some(kb_id) = kb_id {
            sqlx::query_as(
                "SELECT e.id, e.source_node_id, e.target_node_id, e.relation_type, e.properties, e.weight \
                 FROM graph_edges e \
                 INNER JOIN graph_nodes n1 ON e.source_node_id = n1.id \
                 INNER JOIN graph_nodes n2 ON e.target_node_id = n2.id \
                 WHERE n1.kb_id = ?",
            )
            .bind(kb_id)
            .fetch_all(&pool)
            .await?
        } else {
            sqlx::query_as(
                "SELECT e.id, e.source_node_id, e.target_node_id, e.relation_type, e.properties, e.weight \
                 FROM graph_edges e \
                 INNER JOIN graph_nodes n1 ON e.source_node_id = n1.id \
                 INNER JOIN graph_nodes n2 ON e.target_node_id = n2.id",
            )
            .fetch_all(&pool)
            .await?
        };

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

    /// 添加节点（在给定事务连接上执行，调用方负责提交）
    async fn add_node(&mut self, entity: &Entity, conn: &mut sqlx::SqliteConnection) -> Result<NodeIndex> {
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
            .fetch_one(&mut *conn)
            .await?;

        let node = Node::from_entity(entity, id.0);
        let idx = self.graph.add_node(node);
        self.node_index.insert(entity.name.clone(), idx);
        self.node_id_to_index.insert(id.0, idx);

        Ok(idx)
    }

    /// 添加边（在给定事务连接上执行，调用方负责提交）
    async fn add_edge(
        &mut self, source_name: &str, target_name: &str, relation: &Relation, conn: &mut sqlx::SqliteConnection,
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
            .fetch_one(&mut *conn)
            .await?;

        let edge = Edge::from_relation(relation, id.0);
        let edge_idx = self.graph.add_edge(source_idx, target_idx, edge);

        Ok(Some(edge_idx))
    }

    /// 增量更新：所有实体/关系的写入在单个事务内完成，避免逐条提交的往返开销
    pub async fn incremental_update(&mut self, entities: Vec<Entity>, relations: Vec<Relation>) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for entity in &entities {
            self.add_node(entity, &mut tx).await?;
        }

        for relation in &relations {
            self.add_edge(&relation.source_name, &relation.target_name, relation, &mut tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// 不加载整个内存图，直接在数据库中做增量 upsert（用于后台文件处理）。
    pub async fn incremental_update_direct(
        pool: SqlitePool, kb_id: Option<i64>, entities: Vec<Entity>, relations: Vec<Relation>,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let mut name_to_id: HashMap<String, i64> = HashMap::with_capacity(entities.len());

        for entity in entities {
            let entity_type_str = entity.entity_type.as_str();
            let properties_json = serde_json::to_string(&entity.properties)?;
            let id: (i64,) = sqlx::query_as(
                "INSERT INTO graph_nodes (name, entity_type, properties, file_id, kb_id) \
                 VALUES (?, ?, ?, ?, ?) \
                 ON CONFLICT(name, entity_type, kb_id) DO UPDATE SET \
                 properties = excluded.properties, \
                 updated_at = strftime('%s','now') \
                 RETURNING id",
            )
            .bind(&entity.name)
            .bind(entity_type_str)
            .bind(&properties_json)
            .bind(entity.file_id)
            .bind(entity.kb_id.or(kb_id))
            .fetch_one(&mut *tx)
            .await?;
            name_to_id.insert(entity.name, id.0);
        }

        for relation in relations {
            let Some(&source_id) = name_to_id.get(&relation.source_name) else {
                continue;
            };
            let Some(&target_id) = name_to_id.get(&relation.target_name) else {
                continue;
            };
            let relation_type_str = relation.relation_type.as_str();
            let properties_json = serde_json::to_string(&relation.properties)?;
            sqlx::query(
                "INSERT INTO graph_edges (source_node_id, target_node_id, relation_type, properties, weight, file_id) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(source_id)
            .bind(target_id)
            .bind(relation_type_str)
            .bind(&properties_json)
            .bind(relation.weight)
            .bind(relation.file_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
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

    /// 不加载内存图，直接统计数据库中当前 KB 的节点/边数量并保存快照。
    pub async fn save_snapshot_direct(pool: &SqlitePool, kb_id: Option<i64>) -> Result<()> {
        let (node_count,): (i64,) = if let Some(kb_id) = kb_id {
            sqlx::query_as("SELECT COUNT(*) FROM graph_nodes WHERE kb_id = ?").bind(kb_id).fetch_one(pool).await?
        } else {
            sqlx::query_as("SELECT COUNT(*) FROM graph_nodes").fetch_one(pool).await?
        };

        let (edge_count,): (i64,) = if let Some(kb_id) = kb_id {
            sqlx::query_as(
                "SELECT COUNT(*) FROM graph_edges e \
                 INNER JOIN graph_nodes n1 ON e.source_node_id = n1.id \
                 WHERE n1.kb_id = ?",
            )
            .bind(kb_id)
            .fetch_one(pool)
            .await?
        } else {
            sqlx::query_as(
                "SELECT COUNT(*) FROM graph_edges e \
                 INNER JOIN graph_nodes n1 ON e.source_node_id = n1.id",
            )
            .fetch_one(pool)
            .await?
        };

        let mut tx = pool.begin().await?;
        sqlx::query("DELETE FROM graph_snapshots WHERE kb_id IS ?").bind(kb_id).execute(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO graph_snapshots (kb_id, graph_data, node_count, edge_count) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(kb_id)
        .bind(Vec::<u8>::new())
        .bind(node_count)
        .bind(edge_count)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(())
    }
}
