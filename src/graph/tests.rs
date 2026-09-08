use sqlx::{
    SqlitePool, sqlite::{SqliteConnectOptions, SqlitePoolOptions}
};

use super::{
    Entity, EntityType, Relation, RelationType, graph_manager::{self, ExtractedChunk, KnowledgeGraph}
};

pub(crate) async fn database() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().in_memory(true).foreign_keys(true))
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../init.sql")).execute(&pool).await.unwrap();
    pool
}
pub(crate) async fn file(pool: &SqlitePool, id: i64, kb: Option<i64>) {
    sqlx::query(
        "INSERT INTO files(id,user_id,hash,filename,path,kb_id,status) VALUES(?,'owner','hash','test','test',?,1)",
    )
    .bind(id)
    .bind(kb)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO slices(id,file_id) VALUES(?,?)").bind(id).bind(id).execute(pool).await.unwrap();
}
async fn run(pool: &SqlitePool, file: i64, token: &str) {
    sqlx::query("INSERT INTO graph_builds(file_id,status,run_id) VALUES(?,'running',?) ON CONFLICT(file_id) DO UPDATE SET status='running',run_id=excluded.run_id")
        .bind(file).bind(token).execute(pool).await.unwrap();
}
fn chunk(file: i64, description: &str) -> ExtractedChunk {
    let mut relation = Relation::new("张三".into(), "项目甲".into(), RelationType::Custom("负责".into()));
    relation.evidence = Some("张三负责项目甲".into());
    ExtractedChunk {
        slice_id: file,
        offset: 0,
        text: "张三负责项目甲".into(),
        entities: vec![
            Entity::new("张三".into(), EntityType::Custom("人物".into()))
                .with_property("description".into(), description.into()),
            Entity::new("项目甲".into(), EntityType::Custom("项目".into())),
        ],
        relations: vec![relation],
    }
}
async fn count(pool: &SqlitePool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}")).fetch_one(pool).await.unwrap()
}
#[tokio::test]
async fn graph_replace_is_idempotent_and_shared_sources_survive_delete() {
    let pool = database().await;
    file(&pool, 1, Some(1)).await;
    file(&pool, 2, Some(1)).await;
    graph_manager::migrate(&pool).await.unwrap();
    for (id, desc) in [(1, "A"), (2, "B"), (2, "B")] {
        run(&pool, id, "run").await;
        KnowledgeGraph::replace_file(&pool, id, Some(1), "run", vec![chunk(id, desc), chunk(id, desc)]).await.unwrap();
    }
    assert_eq!(count(&pool, "graph_nodes").await, 2);
    assert_eq!(count(&pool, "graph_edges").await, 2); // one distinct source per document
    assert_eq!(count(&pool, "graph_node_sources").await, 4);
    assert_eq!(count(&pool, "graph_edge_sources").await, 2);
    let provenance: (i64, String) =
        sqlx::query_as("SELECT file_id,properties FROM graph_nodes WHERE name='张三'").fetch_one(&pool).await.unwrap();
    assert_eq!(provenance.0, 1);
    assert!(provenance.1.contains('A'));
    // Exercise the actual file-delete trigger, not just the cleanup helper.
    sqlx::query("DELETE FROM files WHERE id=1").execute(&pool).await.unwrap();
    assert_eq!(count(&pool, "graph_nodes").await, 2);
    assert_eq!(count(&pool, "graph_edges").await, 1);
    let provenance: (i64, String) =
        sqlx::query_as("SELECT file_id,properties FROM graph_nodes WHERE name='张三'").fetch_one(&pool).await.unwrap();
    assert_eq!(provenance.0, 2);
    assert!(provenance.1.contains('B'));
    run(&pool, 2, "empty").await;
    KnowledgeGraph::replace_file(&pool, 2, Some(1), "empty", vec![]).await.unwrap();
    assert_eq!(count(&pool, "graph_nodes").await, 0);
    assert_eq!(count(&pool, "graph_edges").await, 0);
}
#[tokio::test]
async fn graph_stale_build_and_invalid_slice_roll_back() {
    let pool = database().await;
    file(&pool, 1, Some(1)).await;
    graph_manager::migrate(&pool).await.unwrap();
    run(&pool, 1, "first").await;
    KnowledgeGraph::replace_file(&pool, 1, Some(1), "first", vec![chunk(1, "A")]).await.unwrap();
    run(&pool, 1, "new").await;
    assert!(KnowledgeGraph::replace_file(&pool, 1, Some(1), "old", vec![]).await.is_err());
    let mut invalid = chunk(1, "B");
    invalid.slice_id = 999;
    assert!(KnowledgeGraph::replace_file(&pool, 1, Some(1), "new", vec![invalid]).await.is_err());
    assert_eq!(count(&pool, "graph_edges").await, 1);
    let status: String =
        sqlx::query_scalar("SELECT status FROM graph_builds WHERE file_id=1").fetch_one(&pool).await.unwrap();
    assert_eq!(status, "running");
}
#[tokio::test]
async fn graph_ambiguous_endpoints_and_ungrounded_claims_are_rejected() {
    let pool = database().await;
    file(&pool, 1, Some(1)).await;
    graph_manager::migrate(&pool).await.unwrap();
    run(&pool, 1, "test").await;
    let mut c = chunk(1, "A");
    c.entities.push(Entity::new("张三".into(), EntityType::Custom("公司".into())));
    let mut typed = c.relations[0].clone();
    typed.source_type = Some("人物".into());
    let mut unsupported = typed.clone();
    unsupported.evidence = Some("张三拥有项目甲".into());
    c.relations.extend([typed, unsupported]);
    KnowledgeGraph::replace_file(&pool, 1, Some(1), "test", vec![c]).await.unwrap();
    assert_eq!(count(&pool, "graph_edges").await, 1);
    let kind: String =
        sqlx::query_scalar("SELECT n.entity_type FROM graph_edges e JOIN graph_nodes n ON n.id=e.source_node_id")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(kind, "人物");
}
#[tokio::test]
async fn graph_null_kb_entities_are_isolated_by_file() {
    let pool = database().await;
    file(&pool, 1, None).await;
    file(&pool, 2, None).await;
    graph_manager::migrate(&pool).await.unwrap();
    for id in [1, 2, 1] {
        run(&pool, id, "test").await;
        KnowledgeGraph::replace_file(&pool, id, None, "test", vec![chunk(id, "test")]).await.unwrap();
    }
    assert_eq!(count(&pool, "graph_nodes").await, 4);
    assert_eq!(count(&pool, "graph_edges").await, 2);
}
#[tokio::test]
async fn graph_migration_preserves_legacy_provenance_and_deduplicates() {
    let pool = database().await;
    file(&pool, 1, Some(1)).await;
    sqlx::raw_sql("INSERT INTO graph_nodes(id,name,entity_type,file_id,kb_id) VALUES(1,'张三','人物',1,1),(2,'项目甲','项目',1,1); \
        INSERT INTO graph_edges(source_node_id,target_node_id,relation_type,file_id) VALUES(1,2,'负责',1),(1,2,'负责',1)")
        .execute(&pool).await.unwrap();
    graph_manager::migrate(&pool).await.unwrap();
    graph_manager::migrate(&pool).await.unwrap();
    assert_eq!(count(&pool, "graph_nodes").await, 2);
    assert_eq!(count(&pool, "graph_edges").await, 1);
    assert_eq!(count(&pool, "graph_node_sources").await, 2);
    assert_eq!(count(&pool, "graph_edge_sources").await, 0); // do not fabricate quotations
}
#[tokio::test]
async fn graph_question_linking_cannot_expand_other_kbs() {
    let pool = database().await;
    file(&pool, 1, Some(1)).await;
    file(&pool, 2, Some(2)).await;
    graph_manager::migrate(&pool).await.unwrap();
    for id in [1, 2] {
        run(&pool, id, "test").await;
        KnowledgeGraph::replace_file(&pool, id, Some(id), "test", vec![chunk(id, "test")]).await.unwrap();
    }
    sqlx::query("UPDATE graph_nodes SET name='保密项目' WHERE kb_id=2 AND name='项目甲'").execute(&pool).await.unwrap();
    let result = super::query::expand(&pool, "张三负责什么", Some(&[1]), None).await.unwrap();
    assert!(result.contains(&"项目甲".to_owned()));
    assert!(!result.contains(&"保密项目".to_owned()));
    let evidence = super::query::evidence(&pool, &["张三".into()], Some(&[1]), None).await.unwrap();
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].file_id, 1);
    assert!(super::query::evidence(&pool, &["张三".into()], Some(&[1]), Some(&[2])).await.unwrap().is_empty());
    assert_eq!(super::query::expand(&pool, "张三", Some(&[]), None).await.unwrap(), vec!["张三"]);
    assert_eq!(super::query::expand(&pool, "张三", Some(&[1]), Some(&[2])).await.unwrap(), vec!["张三"]);
}

#[tokio::test]
async fn graph_batch_cleanup_keeps_other_documents_and_invalidates_jobs() {
    let pool = database().await;
    for id in [1, 2, 3] {
        file(&pool, id, Some(1)).await;
    }
    graph_manager::migrate(&pool).await.unwrap();
    for id in [1, 2, 3] {
        run(&pool, id, "build").await;
        KnowledgeGraph::replace_file(&pool, id, Some(1), "build", vec![chunk(id, "source")]).await.unwrap();
    }
    let mut tx = pool.begin().await.unwrap();
    graph_manager::clear_files(&mut tx, &[1, 2]).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(count(&pool, "graph_nodes").await, 2);
    assert_eq!(count(&pool, "graph_edges").await, 1);
    assert_eq!(count(&pool, "graph_node_sources").await, 2);
    assert_eq!(count(&pool, "graph_builds").await, 1);
    assert!(KnowledgeGraph::replace_file(&pool, 1, Some(1), "build", vec![chunk(1, "stale")]).await.is_err());
}

#[tokio::test]
async fn graph_slice_removal_revokes_claims_but_keeps_other_evidence() {
    let pool = database().await;
    file(&pool, 1, Some(1)).await;
    file(&pool, 2, Some(1)).await;
    graph_manager::migrate(&pool).await.unwrap();
    for id in [1, 2] {
        run(&pool, id, "test").await;
        KnowledgeGraph::replace_file(&pool, id, Some(1), "test", vec![chunk(id, if id == 1 { "A" } else { "B" })])
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM slices WHERE id=1").execute(&pool).await.unwrap();
    assert_eq!(count(&pool, "graph_nodes").await, 2);
    assert_eq!(count(&pool, "graph_edges").await, 1);
    let row: (i64, String) =
        sqlx::query_as("SELECT file_id,properties FROM graph_nodes WHERE name='张三'").fetch_one(&pool).await.unwrap();
    assert_eq!(row.0, 2);
    assert!(row.1.contains('B'));
    let status: String =
        sqlx::query_scalar("SELECT status FROM graph_builds WHERE file_id=1").fetch_one(&pool).await.unwrap();
    assert_eq!(status, "failed");
    sqlx::query("DELETE FROM slices WHERE id=2").execute(&pool).await.unwrap();
    assert_eq!(count(&pool, "graph_nodes").await, 0);
    assert_eq!(count(&pool, "graph_edges").await, 0);
}

#[tokio::test]
async fn graph_migration_merges_null_kb_duplicates_only_within_a_file() {
    let pool = database().await;
    file(&pool, 1, None).await;
    file(&pool, 2, None).await;
    sqlx::raw_sql("INSERT INTO graph_nodes(id,name,entity_type,file_id) VALUES(1,'同名','人物',1),(2,'同名','人物',1),(3,'同名','人物',2),(4,'项目','项目',1); \
        INSERT INTO graph_edges(source_node_id,target_node_id,relation_type,file_id) VALUES(1,4,'负责',1),(2,4,'负责',1); \
        INSERT INTO entity_mentions(node_id,slice_id,context) VALUES(2,1,'同名')").execute(&pool).await.unwrap();
    graph_manager::migrate(&pool).await.unwrap();
    assert_eq!(count(&pool, "graph_nodes").await, 3);
    assert_eq!(count(&pool, "graph_edges").await, 1);
    let mention: i64 = sqlx::query_scalar("SELECT node_id FROM entity_mentions").fetch_one(&pool).await.unwrap();
    assert_eq!(mention, 1);
    assert!(
        sqlx::query("INSERT INTO graph_nodes(name,entity_type,file_id) VALUES('同名','人物',1)")
            .execute(&pool)
            .await
            .is_err()
    );
}
