use std::collections::{HashMap, HashSet};

use axum::{
    Extension,
    extract::{Path, Query, State},
    response::Json,
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use utoipa::{IntoParams, ToSchema};

use crate::{AuthUser, api::error::ApiError};

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct EntityInfo {
    pub id: i64,
    pub name: String,
    pub entity_type: String,
    pub properties: HashMap<String, String>,
    pub file_id: Option<i64>,
    pub kb_id: Option<i64>,
    pub created_at: i64,
}
#[derive(FromRow)]
struct EntityRow {
    id: i64,
    name: String,
    entity_type: String,
    properties: Option<String>,
    file_id: Option<i64>,
    kb_id: Option<i64>,
    created_at: i64,
}
impl From<EntityRow> for EntityInfo {
    fn from(row: EntityRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            entity_type: row.entity_type,
            properties: row.properties.and_then(|p| serde_json::from_str(&p).ok()).unwrap_or_default(),
            file_id: row.file_id,
            kb_id: row.kb_id,
            created_at: row.created_at,
        }
    }
}
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct EntitySearchParams {
    pub q: Option<String>,
    pub entity_type: Option<String>,
    pub kb_id: Option<i64>,
    pub file_id: Option<i64>,
    pub limit: Option<i64>,
    pub before_id: Option<i64>,
    pub node_id: Option<i64>,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct EntityDetail {
    pub entity: EntityInfo,
    pub neighbors: Vec<NeighborInfo>,
    pub mentions: Vec<MentionInfo>,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct NeighborInfo {
    pub entity: EntityInfo,
    pub edge_id: i64,
    pub relation_type: String,
    pub direction: String,
}
#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct MentionInfo {
    pub slice_id: i64,
    pub context: String,
    pub file_id: i64,
    pub filename: String,
}
#[derive(Debug, Clone, Serialize, FromRow, ToSchema)]
pub struct GraphEdge {
    pub id: i64,
    pub source_id: i64,
    pub target_id: i64,
    pub relation_type: String,
    pub file_id: Option<i64>,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct Subgraph {
    pub nodes: Vec<EntityInfo>,
    pub edges: Vec<GraphEdge>,
    pub matched_ids: Vec<i64>,
    pub truncated: bool,
}
#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct GraphBuildInfo {
    pub status: String,
    pub model: String,
    pub extractor_version: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GraphStats {
    pub build: Option<GraphBuildInfo>,
    pub node_count: i64,
    pub edge_count: i64,
    pub entity_types: HashMap<String, i64>,
    pub relation_types: HashMap<String, i64>,
}
#[derive(Debug, Deserialize, IntoParams)]
pub struct StatsParams {
    pub kb_id: Option<i64>,
    pub file_id: Option<i64>,
}

// SQL fragments contain only trusted aliases and database/typed integer IDs. User text is bound.
fn ids_sql(ids: &[i64]) -> String {
    if ids.is_empty() { "NULL".into() } else { ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(",") }
}

/// SQL 字符串字面量。
///
/// 只在「未归属文件」作用域子查询里用于内联当前用户 id：SQLite 的字符串字面量里唯一需要
/// 转义的就是单引号（写成两个单引号），没有反斜杠转义；HTTP 头也不可能带 NUL/换行，
/// 因此双写单引号是完备转义。其余用户输入一律走绑定参数。
fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

struct Scope {
    admin: bool,
    user_id: String,
    kb_ids: Vec<i64>,
}
impl Scope {
    async fn load(
        pool: &SqlitePool, user: &AuthUser, kb_id: Option<i64>, file_id: Option<i64>,
    ) -> Result<Self, ApiError> {
        let admin = user.is_admin();
        let kb_ids = if admin {
            vec![]
        } else {
            super::knowledge_base::get_user_viewable_kb_ids(pool, &user.user_id, false).await
        };
        if kb_id.is_some_and(|id| !admin && !kb_ids.contains(&id)) {
            return Err(ApiError::Forbidden("Permission denied".into()));
        }
        if let Some(id) = file_id {
            let row: Option<(Option<i64>,)> =
                sqlx::query_as("SELECT kb_id FROM files WHERE id=?").bind(id).fetch_optional(pool).await?;
            let (file_kb,) = row.ok_or_else(|| ApiError::NotFound("File not found".into()))?;
            // 单行 EXISTS 判定，取代「先把用户所有未归属文件 id 拉进内存再 contains」。
            let visible_loose = if admin || file_kb.is_some() {
                false
            } else {
                sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM files WHERE id=? AND kb_id IS NULL AND (user_id=? OR is_public=1))",
                )
                .bind(id)
                .bind(&user.user_id)
                .fetch_one(pool)
                .await?
            };
            if !admin && !file_kb.is_some_and(|k| kb_ids.contains(&k)) && !visible_loose {
                return Err(ApiError::Forbidden("Permission denied".into()));
            }
            if kb_id.is_some() && kb_id != file_kb {
                return Err(ApiError::BadRequest("file_id does not belong to kb_id".into()));
            }
        }
        Ok(Self { admin, user_id: user.user_id.clone(), kb_ids })
    }

    /// 当前用户可见的「未归属文件」集合。
    ///
    /// 用等价子查询而不是内联 id 列表：这些条件会出现在几乎每条图谱 SQL 里
    /// （`edge_filter` 一次就用到三遍），文件一多就是几十上百 KB 的 SQL 文本，
    /// 每次请求都要重新拼接并让 SQLite 重新解析。子查询不相关，SQLite 只求值一次。
    fn loose_file_ids(&self) -> String {
        format!(
            "SELECT id FROM files WHERE kb_id IS NULL AND (user_id={} OR is_public=1)",
            sql_literal(&self.user_id)
        )
    }

    /// 当前用户可读取的文件集合，与 `common::push_file_access_filter` 完全同口径。
    ///
    /// 图谱的实体名、关系以及 `entity_mentions.context` 都是从文件正文里抽取的，只按知识库放行
    /// 会把「公开库里的私有文件」的内容泄漏给陌生人——检索与下载侧已经收紧到文件级，图谱必须一致。
    ///
    /// 子查询别名固定用 `vf`：`nodes()` 也会被套用到 `files f` 上（实体详情的 mentions 查询），
    /// 换个别名会与外层 `f` 冲突，或被那里的 `f.file_id -> f.id` 文本替换误伤。
    fn visible_file_ids(&self) -> String {
        let user = sql_literal(&self.user_id);
        format!(
            "SELECT vf.id FROM files vf WHERE vf.user_id={user}              OR (vf.kb_id IS NULL AND vf.is_public=1)              OR vf.kb_id IN (SELECT id FROM knowledge_bases WHERE user_id={user})              OR vf.kb_id IN (SELECT kb_id FROM kb_permissions WHERE user_id={user})              OR (vf.is_public=1 AND vf.kb_id IN (SELECT id FROM knowledge_bases WHERE is_public=1))"
        )
    }

    fn nodes(&self, alias: &str) -> String {
        if self.admin {
            return "1=1".into();
        }
        // 必须是单一括号组：调用方会继续用 `AND` 拼接（edge_filter 后面还接了 `n.kb_id IS t.kb_id`），
        // 顶层裸 OR 会让后半段逃出作用域条件。
        format!(
            "(({alias}.kb_id IN ({}) AND ({alias}.file_id IS NULL OR {alias}.file_id IN ({})))              OR ({alias}.kb_id IS NULL AND {alias}.file_id IN ({})))",
            ids_sql(&self.kb_ids),
            self.visible_file_ids(),
            self.loose_file_ids()
        )
    }
}
fn node_filter(scope: &Scope, kb_id: Option<i64>, file_id: Option<i64>) -> String {
    let mut filter = scope.nodes("n");
    if let Some(id) = kb_id {
        filter += &format!(" AND n.kb_id={id}");
    }
    if let Some(id) = file_id {
        filter += &format!(
            " AND (EXISTS(SELECT 1 FROM graph_node_sources s WHERE s.node_id=n.id AND s.file_id={id}) \
            OR (n.file_id={id} AND NOT EXISTS(SELECT 1 FROM graph_node_sources s WHERE s.node_id=n.id)))"
        );
    }
    filter
}
fn edge_filter(scope: &Scope, kb_id: Option<i64>, file_id: Option<i64>) -> String {
    let mut filter = format!("{} AND {} AND n.kb_id IS t.kb_id", scope.nodes("n"), scope.nodes("t"));
    if let Some(id) = kb_id {
        filter += &format!(" AND n.kb_id={id}");
    }
    if let Some(id) = file_id {
        filter += &format!(" AND e.file_id={id}");
    }
    // A file-specific edge must have a source file in the same scope as its endpoints.
    filter += " AND (e.file_id IS NULL OR EXISTS(SELECT 1 FROM files f WHERE f.id=e.file_id AND f.kb_id IS n.kb_id))";
    if !scope.admin {
        filter += &format!(
            " AND (n.kb_id IS NOT NULL OR (n.file_id=t.file_id AND e.file_id IN ({})))",
            scope.loose_file_ids()
        );
    }
    filter
}
const NODE_COLUMNS: &str = "n.id,n.name,n.entity_type,n.properties,n.file_id,n.kb_id,n.created_at";
const EDGE_FROM: &str =
    "FROM graph_edges e JOIN graph_nodes n ON n.id=e.source_node_id JOIN graph_nodes t ON t.id=e.target_node_id";

async fn find_entities(
    pool: &SqlitePool, params: &EntitySearchParams, scope: &Scope,
) -> Result<Vec<EntityInfo>, ApiError> {
    let limit = params.limit.unwrap_or(100);
    if !(1..=200).contains(&limit) {
        return Err(ApiError::BadRequest("limit must be between 1 and 200".into()));
    }
    let mut filter = node_filter(scope, params.kb_id, params.file_id);
    if let Some(id) = params.before_id {
        filter += &format!(" AND n.id<{id}");
    }
    if let Some(id) = params.node_id {
        filter += &format!(" AND n.id={id}");
    }
    let indexed = params.q.as_ref().is_some_and(|q| q.chars().count() >= 3);
    let search = if indexed {
        "n.id IN (SELECT rowid FROM graph_node_names WHERE graph_node_names MATCH ?)"
    } else {
        "(? IS NULL OR instr(lower(n.name),lower(?))>0)"
    };
    let sql = format!(
        "SELECT {NODE_COLUMNS} FROM graph_nodes n WHERE {filter} AND {search} \
        AND (? IS NULL OR n.entity_type=?) ORDER BY n.id DESC LIMIT ?"
    );
    let mut query = sqlx::query_as::<_, EntityRow>(&sql);
    if indexed {
        query = query.bind(format!("\"{}\"", params.q.as_deref().unwrap_or_default().replace('"', "\"\"")));
    } else {
        query = query.bind(&params.q).bind(&params.q);
    }
    let rows = query.bind(&params.entity_type).bind(&params.entity_type).bind(limit).fetch_all(pool).await?;
    Ok(rows.into_iter().map(Into::into).collect())
}
#[utoipa::path(get, path="/api/v1/knowledge/graph/entities", operation_id="graph_search_entities", tag="graph", params(EntitySearchParams), responses((status=200, body=Vec<EntityInfo>)))]
pub async fn search_entities(
    Query(params): Query<EntitySearchParams>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
) -> Result<Json<Vec<EntityInfo>>, ApiError> {
    let scope = Scope::load(&pool, &user, params.kb_id, params.file_id).await?;
    Ok(Json(find_entities(&pool, &params, &scope).await?))
}

#[utoipa::path(get, path="/api/v1/knowledge/graph/subgraph", operation_id="graph_get_subgraph", tag="graph", params(EntitySearchParams), responses((status=200, body=Subgraph)))]
pub async fn get_subgraph(
    Query(params): Query<EntitySearchParams>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
) -> Result<Json<Subgraph>, ApiError> {
    let scope = Scope::load(&pool, &user, params.kb_id, params.file_id).await?;
    let mut nodes = find_entities(&pool, &params, &scope).await?;
    let matched_ids: Vec<i64> = nodes.iter().map(|n| n.id).collect();
    let ids = ids_sql(&matched_ids);
    let filter = edge_filter(&scope, params.kb_id, params.file_id);
    // UNION lets SQLite use both adjacency indexes instead of a full edge-table scan.
    let sql = format!(
        "SELECT e.id,e.source_node_id AS source_id,e.target_node_id AS target_id,e.relation_type,e.file_id \
        {EDGE_FROM} WHERE {filter} AND e.id IN (SELECT id FROM graph_edges WHERE source_node_id IN ({ids}) \
        UNION SELECT id FROM graph_edges WHERE target_node_id IN ({ids})) ORDER BY e.id LIMIT 401"
    );
    let mut edges: Vec<GraphEdge> = sqlx::query_as(&sql).fetch_all(&pool).await?;
    let mut truncated =
        edges.len() > 400 || (params.node_id.is_none() && nodes.len() as i64 == params.limit.unwrap_or(100));
    edges.truncate(400);
    let mut included: HashSet<i64> = matched_ids.iter().copied().collect();
    for edge in &edges {
        for id in [edge.source_id, edge.target_id] {
            if included.len() < 200 {
                included.insert(id);
            }
        }
    }
    edges.retain(|e| {
        let keep = included.contains(&e.source_id) && included.contains(&e.target_id);
        truncated |= !keep;
        keep
    });
    let extra: Vec<i64> = included.into_iter().filter(|id| !matched_ids.contains(id)).collect();
    if !extra.is_empty() {
        let sql = format!(
            "SELECT {NODE_COLUMNS} FROM graph_nodes n WHERE {} AND n.id IN ({}) ORDER BY n.id",
            scope.nodes("n"),
            ids_sql(&extra)
        );
        let rows: Vec<EntityRow> = sqlx::query_as(&sql).fetch_all(&pool).await?;
        nodes.extend(rows.into_iter().map(EntityInfo::from));
    }
    Ok(Json(Subgraph { nodes, edges, matched_ids, truncated }))
}

#[utoipa::path(get, path="/api/v1/knowledge/graph/entities/{id}", operation_id="graph_get_entity", tag="graph", params(("id"=i64, Path)), responses((status=200, body=EntityDetail), (status=404)))]
pub async fn get_entity(
    Path(id): Path<i64>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
) -> Result<Json<EntityDetail>, ApiError> {
    let scope = Scope::load(&pool, &user, None, None).await?;
    let sql = format!("SELECT {NODE_COLUMNS} FROM graph_nodes n WHERE n.id=? AND {}", scope.nodes("n"));
    let entity: EntityInfo = sqlx::query_as::<_, EntityRow>(&sql)
        .bind(id)
        .fetch_optional(&pool)
        .await?
        .ok_or_else(|| ApiError::NotFound("Entity not found".into()))?
        .into();
    let filter = edge_filter(&scope, entity.kb_id, None);
    let sql = format!(
        "SELECT e.id,e.source_node_id AS source_id,e.target_node_id AS target_id,e.relation_type,e.file_id \
        {EDGE_FROM} WHERE {filter} AND e.id IN (SELECT id FROM graph_edges WHERE source_node_id=? UNION SELECT id FROM graph_edges WHERE target_node_id=?) ORDER BY e.id LIMIT 100"
    );
    let edges: Vec<GraphEdge> = sqlx::query_as(&sql).bind(id).bind(id).fetch_all(&pool).await?;
    let neighbor_ids: Vec<i64> =
        edges.iter().map(|e| if e.source_id == id { e.target_id } else { e.source_id }).collect();
    let sql = format!("SELECT {NODE_COLUMNS} FROM graph_nodes n WHERE n.id IN ({})", ids_sql(&neighbor_ids));
    let rows: Vec<EntityRow> = sqlx::query_as(&sql).fetch_all(&pool).await?;
    let nodes: HashMap<i64, EntityInfo> = rows.into_iter().map(|r| (r.id, r.into())).collect();
    let neighbors = edges
        .into_iter()
        .filter_map(|e| {
            let outgoing = e.source_id == id;
            Some(NeighborInfo {
                entity: nodes.get(&if outgoing { e.target_id } else { e.source_id })?.clone(),
                edge_id: e.id,
                relation_type: e.relation_type,
                direction: if outgoing { "outgoing" } else { "incoming" }.into(),
            })
        })
        .collect();
    let file_scope = scope.nodes("f");
    // Source-file ownership is checked even for shared parse artifacts.
    let sql = format!(
        "SELECT s.slice_id,s.context,s.file_id,f.filename FROM graph_node_sources s JOIN files f ON f.id=s.file_id \
        WHERE s.node_id=? AND s.slice_id IS NOT NULL AND f.kb_id IS ? AND {file_scope} \
        UNION SELECT m.slice_id,COALESCE(m.context,''),f.id,f.filename FROM entity_mentions m JOIN slices s ON s.id=m.slice_id JOIN files f ON f.id=s.file_id \
        WHERE m.node_id=? AND f.kb_id IS ? AND {file_scope} ORDER BY file_id,slice_id LIMIT 20"
    );
    // For files the ownership identifier is id rather than file_id.
    let sql = sql.replace("f.file_id", "f.id");
    let mentions =
        sqlx::query_as(&sql).bind(id).bind(entity.kb_id).bind(id).bind(entity.kb_id).fetch_all(&pool).await?;
    Ok(Json(EntityDetail { entity, neighbors, mentions }))
}

#[utoipa::path(get, path="/api/v1/knowledge/graph/stats", operation_id="graph_get_graph_stats", tag="graph", params(StatsParams), responses((status=200, body=GraphStats)))]
pub async fn get_graph_stats(
    Query(params): Query<StatsParams>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
) -> Result<Json<GraphStats>, ApiError> {
    let scope = Scope::load(&pool, &user, params.kb_id, params.file_id).await?;
    let node_sql = format!(
        "SELECT n.entity_type,COUNT(*) FROM graph_nodes n WHERE {} GROUP BY n.entity_type",
        node_filter(&scope, params.kb_id, params.file_id)
    );
    let edge_sql = format!(
        "SELECT e.relation_type,COUNT(*) {EDGE_FROM} WHERE {} GROUP BY e.relation_type",
        edge_filter(&scope, params.kb_id, params.file_id)
    );
    // Counts and distributions come from two grouped queries, in one consistent read snapshot.
    let mut tx = pool.begin().await?;
    let entity_types: HashMap<String, i64> =
        sqlx::query_as::<_, (String, i64)>(&node_sql).fetch_all(&mut *tx).await?.into_iter().collect();
    let relation_types: HashMap<String, i64> =
        sqlx::query_as::<_, (String, i64)>(&edge_sql).fetch_all(&mut *tx).await?.into_iter().collect();
    let build = if let Some(file_id) = params.file_id {
        sqlx::query_as("SELECT status,model,extractor_version,updated_at FROM graph_builds WHERE file_id=?")
            .bind(file_id)
            .fetch_optional(&mut *tx)
            .await?
    } else {
        None
    };
    tx.commit().await?;
    Ok(Json(GraphStats {
        build,
        node_count: entity_types.values().sum(),
        edge_count: relation_types.values().sum(),
        entity_types,
        relation_types,
    }))
}

#[utoipa::path(get, path="/api/v1/knowledge/graph/edges/{id}/evidence", operation_id="graph_get_edge_evidence", tag="graph", params(("id"=i64, Path)), responses((status=200, body=Vec<MentionInfo>), (status=404)))]
pub async fn get_edge_evidence(
    Path(id): Path<i64>, State(pool): State<SqlitePool>, Extension(user): Extension<AuthUser>,
) -> Result<Json<Vec<MentionInfo>>, ApiError> {
    let scope = Scope::load(&pool, &user, None, None).await?;
    let sql = format!("SELECT EXISTS(SELECT 1 {EDGE_FROM} WHERE e.id=? AND {})", edge_filter(&scope, None, None));
    let visible: bool = sqlx::query_scalar(&sql).bind(id).fetch_one(&pool).await?;
    if !visible {
        return Err(ApiError::NotFound("Relation not found".into()));
    }
    let evidence = sqlx::query_as("SELECT s.slice_id,s.context,e.file_id,f.filename FROM graph_edge_sources s JOIN graph_edges e ON e.id=s.edge_id \
        JOIN files f ON f.id=e.file_id WHERE e.id=? ORDER BY s.slice_id,s.context LIMIT 20")
        .bind(id).fetch_all(&pool).await?;
    Ok(Json(evidence))
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn fixture() -> (SqlitePool, AuthUser) {
        let pool = crate::graph::tests::database().await;
        crate::graph::tests::file(&pool, 1, Some(1)).await;
        crate::graph::tests::file(&pool, 2, Some(2)).await;
        sqlx::raw_sql("INSERT INTO knowledge_bases(id,user_id,name) VALUES(1,'owner','Public to owner'),(2,'other','Private'); \
            INSERT INTO graph_nodes(id,name,entity_type,file_id,kb_id) VALUES(1,'张三','人物',1,1),(2,'项目甲','项目',1,1),(3,'保密实体','人物',2,2); \
            INSERT INTO graph_edges(source_node_id,target_node_id,relation_type,file_id) VALUES(1,2,'负责',1),(2,1,'依赖',1),(1,2,'管理',1),(1,3,'错误跨库边',2); \
            INSERT INTO entity_mentions(node_id,slice_id,context) VALUES(1,1,'张三负责项目甲'),(1,2,'不应泄露')")
            .execute(&pool).await.unwrap();
        crate::graph::graph_manager::migrate(&pool).await.unwrap();
        (pool, AuthUser { user_id: "owner".into(), user_name: "Owner".into(), role: "user".into() })
    }
    /// 公开知识库里的**私有文件**不得通过图谱泄漏：实体名与 `entity_mentions.context`
    /// 都来自文件正文，检索/下载侧已按文件级收紧，图谱必须同口径。
    #[tokio::test]
    async fn graph_hides_private_files_inside_a_public_kb() {
        let (pool, owner) = fixture().await;
        sqlx::raw_sql(
            "INSERT INTO knowledge_bases(id,user_id,name,is_public) VALUES(3,'owner','Public KB',1); \
             INSERT INTO files(id,user_id,hash,filename,path,kb_id,status,is_public) \
               VALUES(3,'owner','h3','private.txt','p',3,1,0),(4,'owner','h4','public.txt','p',3,1,1); \
             INSERT INTO slices(id,file_id) VALUES(3,3),(4,4); \
             INSERT INTO graph_nodes(id,name,entity_type,file_id,kb_id) \
               VALUES(4,'私有实体','人物',3,3),(5,'公开实体','人物',4,3); \
             INSERT INTO entity_mentions(node_id,slice_id,context) VALUES(4,3,'PRIVATE_CONTEXT_LEAK')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let stranger = AuthUser { user_id: "stranger".into(), user_name: "Stranger".into(), role: "user".into() };

        let stranger_entities = search_entities(
            Query(EntitySearchParams { limit: Some(50), ..Default::default() }),
            State(pool.clone()),
            Extension(stranger.clone()),
        )
        .await
        .unwrap()
        .0;
        let names: Vec<&str> = stranger_entities.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"公开实体"), "{names:?}");
        assert!(!names.contains(&"私有实体"), "{names:?}");

        // 实体详情整体挡住，避免 mentions 把私有正文带出去。
        let detail = get_entity(Path(4), State(pool.clone()), Extension(stranger)).await;
        assert!(matches!(detail, Err(ApiError::NotFound(_))), "{detail:?}");

        // 属主不受影响：公开库内的私有文件实体照常可见。
        let owner_entities = search_entities(
            Query(EntitySearchParams { kb_id: Some(3), limit: Some(50), ..Default::default() }),
            State(pool),
            Extension(owner),
        )
        .await
        .unwrap()
        .0;
        assert!(owner_entities.iter().any(|e| e.name == "私有实体"), "{owner_entities:?}");
    }

    #[tokio::test]
    async fn graph_scope_applies_before_limit_and_to_all_stats() {
        let (pool, user) = fixture().await;
        let entities = search_entities(
            Query(EntitySearchParams { limit: Some(1), ..Default::default() }),
            State(pool.clone()),
            Extension(user.clone()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].id, 2);
        let stats = get_graph_stats(
            Query(StatsParams { kb_id: None, file_id: None }),
            State(pool.clone()),
            Extension(user.clone()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(stats.node_count, 2);
        assert_eq!(stats.edge_count, 3);
        assert!(matches!(
            get_graph_stats(
                Query(StatsParams { kb_id: None, file_id: Some(2) }),
                State(pool.clone()),
                Extension(user.clone())
            )
            .await,
            Err(ApiError::Forbidden(_))
        ));
        let admin = AuthUser { role: "admin".into(), ..user.clone() };
        assert!(matches!(
            get_graph_stats(
                Query(StatsParams { kb_id: Some(1), file_id: Some(2) }),
                State(pool.clone()),
                Extension(admin)
            )
            .await,
            Err(ApiError::BadRequest(_))
        ));
        assert!(matches!(
            search_entities(
                Query(EntitySearchParams { limit: Some(-1), ..Default::default() }),
                State(pool),
                Extension(user)
            )
            .await,
            Err(ApiError::BadRequest(_))
        ));
    }
    #[tokio::test]
    async fn graph_subgraph_preserves_parallel_edges_direction_and_scope() {
        let (pool, user) = fixture().await;
        let graph = get_subgraph(
            Query(EntitySearchParams { node_id: Some(1), limit: Some(1), ..Default::default() }),
            State(pool.clone()),
            Extension(user.clone()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 3);
        assert!(!graph.truncated);
        assert!(graph.edges.iter().any(|e| e.source_id == 2 && e.target_id == 1));
        let detail = get_entity(Path(1), State(pool.clone()), Extension(user.clone())).await.unwrap().0;
        assert_eq!(detail.neighbors.len(), 3);
        assert_eq!(detail.mentions.len(), 1);
        assert!(matches!(
            get_entity(Path(3), State(pool.clone()), Extension(user.clone())).await,
            Err(ApiError::NotFound(_))
        ));
        assert!(matches!(get_edge_evidence(Path(4), State(pool), Extension(user)).await, Err(ApiError::NotFound(_))));
    }
    #[tokio::test]
    async fn graph_indexed_name_search_and_file_provenance() {
        let (pool, user) = fixture().await;
        let result = search_entities(
            Query(EntitySearchParams { q: Some("项目甲".into()), file_id: Some(1), ..Default::default() }),
            State(pool.clone()),
            Extension(user.clone()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, 2);
        // FTS query syntax must be treated as literal user text.
        let result = search_entities(
            Query(EntitySearchParams { q: Some("\" OR *".into()), ..Default::default() }),
            State(pool.clone()),
            Extension(user.clone()),
        )
        .await
        .unwrap()
        .0;
        assert!(result.is_empty());
        crate::graph::tests::file(&pool, 3, Some(1)).await;
        sqlx::query("INSERT INTO graph_node_sources(node_id,file_id,slice_id,context) VALUES(1,3,3,'张三')")
            .execute(&pool)
            .await
            .unwrap();
        let shared = search_entities(
            Query(EntitySearchParams { file_id: Some(3), ..Default::default() }),
            State(pool.clone()),
            Extension(user.clone()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].id, 1);
        sqlx::query("INSERT INTO graph_edge_sources(edge_id,slice_id,context) VALUES(1,1,'张三负责项目甲')")
            .execute(&pool)
            .await
            .unwrap();
        let evidence = get_edge_evidence(Path(1), State(pool.clone()), Extension(user.clone())).await.unwrap().0;
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].context, "张三负责项目甲");
        let detail = get_entity(Path(999), State(pool), Extension(user)).await;
        assert!(matches!(detail, Err(ApiError::NotFound(_))));
    }
}
