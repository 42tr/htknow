use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub mod entity_extractor;
pub mod graph_manager;
pub mod llm_extractor;
pub mod relation_extractor;

/// 实体类型枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum EntityType {
    // 通用实体
    Person,       // 人物
    Organization, // 组织/公司
    Location,     // 地点
    Date,         // 日期/时间
    Product,      // 产品名称

    // 技术实体
    Technology, // 技术/框架
    Concept,    // 技术概念
    Api,        // API/接口

    // 文档结构
    Document, // 文档
    Chapter,  // 章节
    Table,    // 表格
    Image,    // 图片

    // 领域特定（可扩展）
    Custom(String),
}

impl EntityType {
    /// 从字符串解析实体类型
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "person" => EntityType::Person,
            "organization" => EntityType::Organization,
            "location" => EntityType::Location,
            "date" => EntityType::Date,
            "product" => EntityType::Product,
            "technology" => EntityType::Technology,
            "concept" => EntityType::Concept,
            "api" => EntityType::Api,
            "document" => EntityType::Document,
            "chapter" => EntityType::Chapter,
            "table" => EntityType::Table,
            "image" => EntityType::Image,
            _ => EntityType::Custom(s.to_string()),
        }
    }

    /// 转换为字符串
    pub fn as_str(&self) -> String {
        match self {
            EntityType::Person => "person".to_string(),
            EntityType::Organization => "organization".to_string(),
            EntityType::Location => "location".to_string(),
            EntityType::Date => "date".to_string(),
            EntityType::Product => "product".to_string(),
            EntityType::Technology => "technology".to_string(),
            EntityType::Concept => "concept".to_string(),
            EntityType::Api => "api".to_string(),
            EntityType::Document => "document".to_string(),
            EntityType::Chapter => "chapter".to_string(),
            EntityType::Table => "table".to_string(),
            EntityType::Image => "image".to_string(),
            EntityType::Custom(s) => s.clone(),
        }
    }
}

/// 关系类型枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum RelationType {
    // 语义关系
    IsA,         // 是一种
    PartOf,      // 部分-整体
    HasProperty, // 具有属性
    DependsOn,   // 依赖于
    RelatedTo,   // 相关

    // 时序关系
    Before, // 发生在...之前
    After,  // 发生在...之后
    During, // 在...期间

    // 引用关系
    References, // 引用
    CoOccurs,   // 共现
    DefinedIn,  // 定义于

    // 文档关系
    Contains,    // 包含
    MentionedIn, // 提及于

    // 自定义
    Custom(String),
}

impl RelationType {
    /// 从字符串解析关系类型
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "isa" => RelationType::IsA,
            "partof" => RelationType::PartOf,
            "hasproperty" => RelationType::HasProperty,
            "dependson" => RelationType::DependsOn,
            "relatedto" => RelationType::RelatedTo,
            "before" => RelationType::Before,
            "after" => RelationType::After,
            "during" => RelationType::During,
            "references" => RelationType::References,
            "cooccurs" => RelationType::CoOccurs,
            "definedin" => RelationType::DefinedIn,
            "contains" => RelationType::Contains,
            "mentionedin" => RelationType::MentionedIn,
            _ => RelationType::Custom(s.to_string()),
        }
    }

    /// 转换为字符串
    pub fn as_str(&self) -> String {
        match self {
            RelationType::IsA => "isa".to_string(),
            RelationType::PartOf => "partof".to_string(),
            RelationType::HasProperty => "hasproperty".to_string(),
            RelationType::DependsOn => "dependson".to_string(),
            RelationType::RelatedTo => "relatedto".to_string(),
            RelationType::Before => "before".to_string(),
            RelationType::After => "after".to_string(),
            RelationType::During => "during".to_string(),
            RelationType::References => "references".to_string(),
            RelationType::CoOccurs => "cooccurs".to_string(),
            RelationType::DefinedIn => "definedin".to_string(),
            RelationType::Contains => "contains".to_string(),
            RelationType::MentionedIn => "mentionedin".to_string(),
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
    pub embedding: Option<Vec<f32>>,
}

impl Entity {
    pub fn new(name: String, entity_type: EntityType) -> Self {
        Self { name, entity_type, properties: HashMap::new(), file_id: None, kb_id: None, embedding: None }
    }

    pub fn with_file(mut self, file_id: i64) -> Self {
        self.file_id = Some(file_id);
        self
    }

    pub fn with_kb(mut self, kb_id: i64) -> Self {
        self.kb_id = Some(kb_id);
        self
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

    pub fn with_weight(mut self, weight: f32) -> Self {
        self.weight = weight;
        self
    }

    pub fn with_file(mut self, file_id: i64) -> Self {
        self.file_id = Some(file_id);
        self
    }

    pub fn with_property(mut self, key: String, value: String) -> Self {
        self.properties.insert(key, value);
        self
    }
}

/// 图节点（用于petgraph）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: i64,      // 数据库ID
    pub name: String, // 实体名称
    pub entity_type: EntityType,
    pub properties: HashMap<String, String>,
    pub embedding: Option<Vec<f32>>,
}

impl Node {
    pub fn from_entity(entity: &Entity, id: i64) -> Self {
        Self {
            id,
            name: entity.name.clone(),
            entity_type: entity.entity_type.clone(),
            properties: entity.properties.clone(),
            embedding: entity.embedding.clone(),
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

/// 实体提及（在文档中的出现位置）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityMention {
    pub node_id: i64,
    pub slice_id: i64,
    pub start_offset: Option<usize>,
    pub end_offset: Option<usize>,
    pub context: String,
}

impl EntityMention {
    pub fn new(node_id: i64, slice_id: i64, context: String) -> Self {
        Self { node_id, slice_id, start_offset: None, end_offset: None, context }
    }

    pub fn with_offsets(mut self, start: usize, end: usize) -> Self {
        self.start_offset = Some(start);
        self.end_offset = Some(end);
        self
    }
}
