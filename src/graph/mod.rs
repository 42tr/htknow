use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub mod graph_manager;
pub mod llm_extractor;

/// 实体类型枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum EntityType {
    // 领域特定（完全由LLM定义）
    Custom(String),
}

impl EntityType {
    /// 转换为字符串
    pub fn as_str(&self) -> String {
        match self {
            EntityType::Custom(s) => s.clone(),
        }
    }
}

/// 关系类型枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum RelationType {
    // 自定义（完全由LLM定义）
    Custom(String),
}

impl RelationType {
    /// 转换为字符串
    pub fn as_str(&self) -> String {
        match self {
            RelationType::Custom(s) => s.clone(),
        }
    }
}

/// 实体结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub name: String,
    pub entity_type: EntityType,
    pub properties: HashMap<String, String>,
    pub file_id: Option<i64>,
    pub kb_id: Option<i64>,
}

impl Entity {
    pub fn new(name: String, entity_type: EntityType) -> Self {
        Self { name, entity_type, properties: HashMap::new(), file_id: None, kb_id: None }
    }

    pub fn with_property(mut self, key: String, value: String) -> Self {
        self.properties.insert(key, value);
        self
    }
}

/// 关系结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    pub source_name: String,
    pub target_name: String,
    pub relation_type: RelationType,
    pub properties: HashMap<String, String>,
    pub weight: f32,
    pub file_id: Option<i64>,
}

impl Relation {
    pub fn new(source_name: String, target_name: String, relation_type: RelationType) -> Self {
        Self { source_name, target_name, relation_type, properties: HashMap::new(), weight: 1.0, file_id: None }
    }

    pub fn with_property(mut self, key: String, value: String) -> Self {
        self.properties.insert(key, value);
        self
    }
}

/// 图节点（用于petgraph）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: i64,
    pub name: String,
    pub entity_type: EntityType,
    pub properties: HashMap<String, String>,
}

impl Node {
    pub fn from_entity(entity: &Entity, id: i64) -> Self {
        Self {
            id,
            name: entity.name.clone(),
            entity_type: entity.entity_type.clone(),
            properties: entity.properties.clone(),
        }
    }
}

/// 图边（用于petgraph）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: i64,
    pub relation_type: RelationType,
    pub weight: f32,
    pub properties: HashMap<String, String>,
}

impl Edge {
    pub fn from_relation(relation: &Relation, id: i64) -> Self {
        Self {
            id,
            relation_type: relation.relation_type.clone(),
            weight: relation.weight,
            properties: relation.properties.clone(),
        }
    }
}
