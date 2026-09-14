//! Wiki 模块的共用测试脚手架：内存库 + 最小可用的知识库/文件/切片数据。

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

/// 建一个干净的内存库，执行 `init.sql`、图谱迁移与 Wiki 迁移。
pub(crate) async fn database() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().in_memory(true).foreign_keys(true))
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../init.sql")).execute(&pool).await.unwrap();
    crate::graph::graph_manager::migrate(&pool).await.unwrap();
    super::migrate(&pool).await.unwrap();
    pool
}

pub(crate) async fn seed_kb(pool: &SqlitePool, kb_id: i64) {
    sqlx::query("INSERT INTO knowledge_bases(id, user_id, name) VALUES(?, 'owner', ?)")
        .bind(kb_id)
        .bind(format!("kb-{}", kb_id))
        .execute(pool)
        .await
        .unwrap();
}

pub(crate) async fn seed_kb_with_wiki_config(pool: &SqlitePool, kb_id: i64, config: &str) {
    seed_kb(pool, kb_id).await;
    sqlx::query("UPDATE knowledge_bases SET wiki_config = ? WHERE id = ?")
        .bind(config)
        .bind(kb_id)
        .execute(pool)
        .await
        .unwrap();
}

pub(crate) async fn seed_file(pool: &SqlitePool, file_id: i64, kb_id: Option<i64>, filename: &str) {
    sqlx::query("INSERT INTO files(id, user_id, hash, filename, path, kb_id, status) VALUES(?,'owner',?,?,'p',?,1)")
        .bind(file_id)
        .bind(format!("hash-{}", file_id))
        .bind(filename)
        .bind(kb_id)
        .execute(pool)
        .await
        .unwrap();
}

pub(crate) async fn seed_slice(pool: &SqlitePool, slice_id: i64, file_id: i64) {
    sqlx::query("INSERT INTO slices(id, file_id) VALUES(?, ?)")
        .bind(slice_id)
        .bind(file_id)
        .execute(pool)
        .await
        .unwrap();
}

/// 写入图谱实体及其切片证据，用于验证「模式 A：复用图谱产物」的候选来源。
pub(crate) async fn seed_graph_entity(
    pool: &SqlitePool, node_id: i64, name: &str, entity_type: &str, kb_id: i64, file_id: i64, slice_id: i64,
    description: &str, context: &str,
) {
    sqlx::query("INSERT INTO graph_nodes(id, name, entity_type, properties, file_id, kb_id) VALUES(?,?,?,?,?,?)")
        .bind(node_id)
        .bind(name)
        .bind(entity_type)
        .bind(serde_json::json!({ "description": description }).to_string())
        .bind(file_id)
        .bind(kb_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO graph_node_sources(id, node_id, file_id, slice_id, start_offset, end_offset, context) \
                 VALUES(?,?,?,?,0,0,?)",
    )
    .bind(node_id)
    .bind(node_id)
    .bind(file_id)
    .bind(slice_id)
    .bind(context)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn migrate_is_idempotent() {
    let pool = database().await;
    // 版本号已被抢占，重复执行不应再次跑 DDL（ALTER TABLE 会因列已存在而失败）。
    super::migrate(&pool).await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM schema_migrations WHERE version = 6").fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn file_delete_trigger_enqueues_retract_with_page_ids() {
    let pool = database().await;
    seed_kb(&pool, 1).await;
    seed_file(&pool, 10, Some(1), "a.pdf").await;
    seed_slice(&pool, 100, 10).await;
    crate::wiki::page::upsert(
        &pool,
        &crate::wiki::page::PageDraft {
            kb_id: 1,
            slug: "entity/张三".to_string(),
            title: "张三".to_string(),
            page_type: crate::wiki::PAGE_TYPE_ENTITY.to_string(),
            summary: "一句话".to_string(),
            content: "正文".to_string(),
            aliases: vec![],
            edit_source: crate::wiki::EDIT_SOURCE_PIPELINE.to_string(),
            editor_id: String::new(),
        },
        &[10],
        &[100],
    )
    .await
    .unwrap();

    sqlx::query("DELETE FROM files WHERE id = 10").execute(&pool).await.unwrap();

    let tasks = sqlx::query_as::<_, (String, Option<i64>, String)>(
        "SELECT task_type, file_id, payload FROM wiki_tasks WHERE task_type = 'wiki:retract'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].1, Some(10));
    let page_ids = crate::wiki::queue::parse_retract_payload(&tasks[0].2).page_ids;
    assert_eq!(page_ids.len(), 1);

    // 切片证据随文件级联清理，页面来源也一并清空。
    let sources: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wiki_page_sources").fetch_one(&pool).await.unwrap();
    assert_eq!(sources, 0);
}

#[test]
fn slugify_keeps_cjk_and_folds_separators() {
    assert_eq!(super::slugify("张三 / Zhang San"), "张三-zhang-san");
    assert_eq!(super::slugify("  Hello,   World! "), "hello-world");
    assert_eq!(super::slugify("RAG（检索增强生成）"), "rag-检索增强生成");
    assert_eq!(super::slugify("///"), "");
}

#[test]
fn make_slug_falls_back_to_hash_for_unsluggable_names() {
    assert_eq!(super::make_slug("entity", "张三"), "entity/张三");
    let slug = super::make_slug("concept", "///");
    assert!(slug.starts_with("concept/"));
    assert_eq!(slug.len(), "concept/".len() + 16);
    // 同名稳定：slug 必须可复现，否则重复入库会产生两个页面。
    assert_eq!(slug, super::make_slug("concept", "///"));
}

#[test]
fn slug_prefix_and_summary_slug() {
    assert_eq!(super::slug_prefix("entity/张三"), "entity");
    assert_eq!(super::slug_prefix("index"), "");
    assert_eq!(super::summary_slug(42), "summary/42");
}

#[tokio::test]
async fn resolve_config_merges_kb_overrides() {
    let pool = database().await;
    seed_kb_with_wiki_config(&pool, 1, r#"{"enabled":true,"granularity":"focused","language":"English"}"#).await;
    seed_kb(&pool, 2).await;

    let resolved = super::resolve_config(&pool, 1).await.unwrap().unwrap();
    assert!(resolved.enabled);
    assert_eq!(resolved.granularity, super::Granularity::Focused);
    assert_eq!(resolved.language, "English");

    // 未配置的知识库回退到全局默认（HTKNOW_BUILD_WIKI 默认关闭）。
    let fallback = super::resolve_config(&pool, 2).await.unwrap().unwrap();
    assert!(!fallback.enabled);
    assert_eq!(fallback.granularity, super::Granularity::Standard);

    assert!(super::resolve_config(&pool, 999).await.unwrap().is_none());
}

#[tokio::test]
async fn resolve_config_tolerates_invalid_json() {
    let pool = database().await;
    seed_kb_with_wiki_config(&pool, 1, "not-json").await;
    let resolved = super::resolve_config(&pool, 1).await.unwrap().unwrap();
    assert_eq!(resolved.granularity, super::Granularity::Standard);
}

fn resolved(granularity: super::Granularity) -> super::ResolvedWikiConfig {
    super::ResolvedWikiConfig {
        enabled: true,
        granularity,
        language: "中文".to_string(),
        model: None,
        max_pages_per_ingest: 0,
    }
}

#[tokio::test]
async fn candidates_from_graph_merges_slugs_and_keeps_only_evidenced_nodes() {
    let pool = database().await;
    seed_kb(&pool, 1).await;
    seed_file(&pool, 10, Some(1), "产品介绍.pdf").await;
    seed_slice(&pool, 100, 10).await;
    seed_slice(&pool, 101, 10).await;

    seed_graph_entity(&pool, 1, "张三", "person", 1, 10, 100, "候选人", "张三负责检索系统").await;
    // graph_nodes 的唯一键是 (name, entity_type, kb_id)，所以「苹果」是两个节点；
    // 但 make_slug 会把它们折叠到同一个 entity/ 页面，必须合并证据而不是互相覆盖。
    seed_graph_entity(&pool, 2, "苹果", "organization", 1, 10, 100, "一家公司", "苹果公司发布了…").await;
    seed_graph_entity(&pool, 3, "苹果", "product", 1, 10, 101, "一款手机", "苹果 15 的售价…").await;
    seed_graph_entity(&pool, 4, "RAG", "concept", 1, 10, 101, "检索增强生成", "用 RAG 做问答").await;
    // 没有切片证据的节点不构成候选：没有原文可引用，写出来只能是编的。
    sqlx::query("INSERT INTO graph_nodes(id, name, entity_type, kb_id, file_id) VALUES(5, '孤儿', 'person', 1, 10)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO graph_node_sources(node_id, file_id, slice_id, context) VALUES(5, 10, NULL, '')")
        .execute(&pool)
        .await
        .unwrap();

    let candidates = super::ingest::candidates_from_graph(&pool, 1, 10, &resolved(super::Granularity::Standard), true)
        .await
        .unwrap()
        .unwrap();

    let slugs: Vec<&str> = candidates.iter().map(|c| c.slug.as_str()).collect();
    assert_eq!(candidates.len(), 3, "{:?}", slugs);
    let by_slug: std::collections::HashMap<&str, &super::ingest::Candidate> =
        candidates.iter().map(|c| (c.slug.as_str(), c)).collect();

    let person = by_slug.get("entity/张三").expect("person 应归类为 entity 页面");
    assert_eq!(person.slice_ids, vec![100]);
    assert_eq!(person.description, "候选人");

    let merged = by_slug.get("entity/苹果").expect("同 slug 的两个节点应合并");
    assert_eq!(merged.slice_ids, vec![100, 101]);
    assert_eq!(merged.description, "一家公司");

    let rag = by_slug.get("concept/rag").expect("concept 类型应归类为 concept 页面");
    assert_eq!(rag.page_type, super::PAGE_TYPE_CONCEPT);
    assert_eq!(rag.slice_ids, vec![101]);

    // 图谱未开启时返回 None，由调用方回退到模式 B。
    assert!(
        super::ingest::candidates_from_graph(&pool, 1, 10, &resolved(super::Granularity::Standard), false)
            .await
            .unwrap()
            .is_none()
    );
}
