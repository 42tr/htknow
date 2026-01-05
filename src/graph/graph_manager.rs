use std::collections::HashMap;

use anyhow::Result;
use bincode;
use petgraph::{
    Direction, algo::dijkstra, graph::{DiGraph, NodeIndex}
};
use sqlx::SqlitePool;

use super::{Edge, Entity, EntityMention, EntityType, Node, Relation, RelationType};

/// 知识图谱管理器
pub struct KnowledgeGraph {
    graph: DiGraph<Node, Edge>,                // petgraph图
    node_index: HashMap<String, NodeIndex>,    // 实体名称 -> 节点索引
    node_id_to_index: HashMap<i64, NodeIndex>, // 数据库ID -> 节点索引
    pool: SqlitePool,                          // 数据库连接
    kb_id: Option<i64>,                        // 知识库ID
}

impl KnowledgeGraph {
    /// 从数据库加载图
    pub async fn load_from_db(pool: SqlitePool, kb_id: Option<i64>) -> Result<Self> {
        let mut graph = DiGraph::new();
        let mut node_index = HashMap::new();
        let mut node_id_to_index = HashMap::new();

        // 构建WHERE子句
        let where_clause = if let Some(kb_id) = kb_id { format!("WHERE kb_id = {}", kb_id) } else { "".to_string() };

        // 加载节点
        let nodes_sql =
            format!("SELECT id, name, entity_type, properties, embedding FROM graph_nodes {}", where_clause);
        let nodes: Vec<(i64, String, String, Option<String>, Option<Vec<u8>>)> =
            sqlx::query_as(&nodes_sql).fetch_all(&pool).await?;

        for (id, name, entity_type_str, properties_json, embedding_bytes) in nodes {
            let entity_type = EntityType::from_str(&entity_type_str);
            let properties: HashMap<String, String> = if let Some(json) = properties_json {
                serde_json::from_str(&json).unwrap_or_default()
            } else {
                HashMap::new()
            };

            let embedding = if let Some(bytes) = embedding_bytes { bincode::deserialize(&bytes).ok() } else { None };

            let node = Node { id, name: name.clone(), entity_type, properties, embedding };

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
                let relation_type = RelationType::from_str(&relation_type_str);
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
        // 检查节点是否已存在
        if let Some(&idx) = self.node_index.get(&entity.name) {
            return Ok(idx);
        }

        // 插入数据库
        let entity_type_str = entity.entity_type.as_str();
        let properties_json = serde_json::to_string(&entity.properties)?;
        let embedding_bytes =
            if let Some(ref embedding) = entity.embedding { Some(bincode::serialize(embedding)?) } else { None };

        let sql = "INSERT INTO graph_nodes (name, entity_type, properties, embedding, file_id, kb_id) \
                   VALUES (?, ?, ?, ?, ?, ?) \
                   ON CONFLICT(name, entity_type, kb_id) DO UPDATE SET \
                   properties = excluded.properties, \
                   embedding = excluded.embedding, \
                   updated_at = strftime('%s','now') \
                   RETURNING id";

        let id: (i64,) = sqlx::query_as(sql)
            .bind(&entity.name)
            .bind(entity_type_str)
            .bind(&properties_json)
            .bind(embedding_bytes)
            .bind(entity.file_id)
            .bind(entity.kb_id.or(self.kb_id))
            .fetch_one(&self.pool)
            .await?;

        // 添加到图
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
        // 获取源节点和目标节点索引
        let source_idx = match self.node_index.get(source_name) {
            Some(&idx) => idx,
            None => return Ok(None),
        };

        let target_idx = match self.node_index.get(target_name) {
            Some(&idx) => idx,
            None => return Ok(None),
        };

        // 获取节点的数据库ID
        let source_id = self.graph[source_idx].id;
        let target_id = self.graph[target_idx].id;

        // 插入数据库
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

        // 添加到图
        let edge = Edge::from_relation(relation, id.0);
        let edge_idx = self.graph.add_edge(source_idx, target_idx, edge);

        Ok(Some(edge_idx))
    }

    /// 增量更新（添加新实体和关系）
    pub async fn incremental_update(&mut self, entities: Vec<Entity>, relations: Vec<Relation>) -> Result<()> {
        // 添加所有实体
        for entity in &entities {
            self.add_node(entity).await?;
        }

        // 添加所有关系
        for relation in &relations {
            self.add_edge(&relation.source_name, &relation.target_name, relation).await?;
        }

        Ok(())
    }

    /// 保存图快照到数据库
    pub async fn save_snapshot(&self) -> Result<()> {
        // TODO: petgraph doesn't implement Serialize by default
        // We can implement a custom serialization or use a different approach
        // For now, we skip this as it's not critical for MVP

        // Just record the statistics
        let node_count = self.graph.node_count() as i64;
        let edge_count = self.graph.edge_count() as i64;

        // 删除旧快照
        let delete_sql = "DELETE FROM graph_snapshots WHERE kb_id IS ?";
        sqlx::query(delete_sql).bind(self.kb_id).execute(&self.pool).await?;

        // 插入新快照
        let insert_sql = "INSERT INTO graph_snapshots (kb_id, graph_data, node_count, edge_count) \
                          VALUES (?, ?, ?, ?)";
        sqlx::query(insert_sql)
            .bind(self.kb_id)
            .bind(Vec::<u8>::new()) // Empty for now
            .bind(node_count)
            .bind(edge_count)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// 查找实体
    pub fn find_entity(&self, name: &str) -> Option<&Node> {
        self.node_index.get(name).map(|&idx| &self.graph[idx])
    }

    /// 获取邻居节点
    pub fn get_neighbors(&self, entity_name: &str, max_depth: usize) -> Vec<&Node> {
        let mut neighbors = Vec::new();

        if let Some(&start_idx) = self.node_index.get(entity_name) {
            let mut visited = std::collections::HashSet::new();
            let mut queue = std::collections::VecDeque::new();

            queue.push_back((start_idx, 0));
            visited.insert(start_idx);

            while let Some((idx, depth)) = queue.pop_front() {
                if depth >= max_depth {
                    continue;
                }

                // 获取所有邻居（入边和出边）
                for neighbor_idx in self.graph.neighbors_undirected(idx) {
                    if !visited.contains(&neighbor_idx) {
                        visited.insert(neighbor_idx);
                        neighbors.push(&self.graph[neighbor_idx]);
                        queue.push_back((neighbor_idx, depth + 1));
                    }
                }
            }
        }

        neighbors
    }

    /// 查找最短路径
    pub fn find_shortest_path(&self, from: &str, to: &str) -> Option<Vec<&Node>> {
        let start_idx = self.node_index.get(from)?;
        let end_idx = self.node_index.get(to)?;

        // 使用Dijkstra算法
        let path_map = dijkstra(&self.graph, *start_idx, Some(*end_idx), |_| 1);

        if !path_map.contains_key(end_idx) {
            return None;
        }

        // 重建路径
        let mut path = Vec::new();
        let mut current = *end_idx;
        path.push(&self.graph[current]);

        while current != *start_idx {
            let mut found = false;
            for predecessor in self.graph.neighbors_directed(current, Direction::Incoming) {
                if path_map.contains_key(&predecessor) && path_map[&predecessor] + 1 == path_map[&current] {
                    current = predecessor;
                    path.push(&self.graph[current]);
                    found = true;
                    break;
                }
            }
            if !found {
                break;
            }
        }

        path.reverse();
        Some(path)
    }

    /// 获取图统计信息
    pub fn get_stats(&self) -> GraphStats {
        GraphStats { node_count: self.graph.node_count(), edge_count: self.graph.edge_count(), kb_id: self.kb_id }
    }

    /// 添加实体提及
    pub async fn add_entity_mention(&self, mention: &EntityMention) -> Result<()> {
        let sql = "INSERT INTO entity_mentions (node_id, slice_id, start_offset, end_offset, context) \
                   VALUES (?, ?, ?, ?, ?)";

        sqlx::query(sql)
            .bind(mention.node_id)
            .bind(mention.slice_id)
            .bind(mention.start_offset.map(|v| v as i64))
            .bind(mention.end_offset.map(|v| v as i64))
            .bind(&mention.context)
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}

/// 图统计信息
#[derive(Debug, Clone)]
pub struct GraphStats {
    pub node_count: usize,
    pub edge_count: usize,
    pub kb_id: Option<i64>,
}
