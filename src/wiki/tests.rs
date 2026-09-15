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

// ============================================================================
// P2：人工编辑、版本快照、归档、体检
// ============================================================================

use super::{
    EDIT_SOURCE_REVERT, EDIT_SOURCE_USER, INDEX_SLUG, PAGE_TYPE_CONCEPT, PAGE_TYPE_INDEX, STATUS_ARCHIVED,
    STATUS_PUBLISHED,
    edit::{EditError, NewPage, PageEdit},
    page::{self, PageDraft},
    revision,
};

fn draft(kb_id: i64, slug: &str, title: &str, summary: &str, content: &str) -> PageDraft {
    PageDraft {
        kb_id,
        slug: slug.to_string(),
        title: title.to_string(),
        page_type: super::PAGE_TYPE_ENTITY.to_string(),
        summary: summary.to_string(),
        content: content.to_string(),
        aliases: vec![],
        edit_source: super::EDIT_SOURCE_PIPELINE.to_string(),
        editor_id: String::new(),
    }
}

fn new_page(title: &str) -> NewPage {
    NewPage {
        title: title.to_string(),
        page_type: None,
        slug: None,
        summary: "一句话摘要".to_string(),
        content: "手工正文".to_string(),
        aliases: vec![],
        status: None,
        editor_id: "alice".to_string(),
    }
}

fn edit_content(content: &str) -> PageEdit {
    PageEdit { content: Some(content.to_string()), editor_id: "alice".to_string(), ..Default::default() }
}

#[tokio::test]
async fn migrate_v7_creates_revision_table_once() {
    let pool = database().await;
    // 版本号已抢占，重复执行不该再跑 DDL。
    super::migrate(&pool).await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM schema_migrations WHERE version = 7").fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1);
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wiki_page_revisions").fetch_one(&pool).await.unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn upsert_snapshots_previous_version_before_overwriting() {
    let pool = database().await;
    seed_kb(&pool, 1).await;
    let first = page::upsert(&pool, &draft(1, "entity/张三", "张三", "摘要", "第一版"), &[], &[]).await.unwrap();
    assert_eq!(first.version, 1);
    assert!(!first.skipped_manual);
    // 首版没有历史：当前行就是唯一版本，不该产生快照。
    assert_eq!(revision::count(&pool, first.page_id).await.unwrap(), 0);

    let second = page::upsert(&pool, &draft(1, "entity/张三", "张三", "摘要", "第二版"), &[], &[]).await.unwrap();
    assert_eq!(second.version, 2);
    let items = revision::list(&pool, first.page_id, 10).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].version, 1);
    assert_eq!(items[0].edit_source, super::EDIT_SOURCE_PIPELINE);
    assert_eq!(items[0].content_length, 3);
    let snapshot = revision::get(&pool, first.page_id, 1).await.unwrap().unwrap();
    assert_eq!(snapshot.content, "第一版");
    assert_eq!(snapshot.title, "张三");
    assert!(revision::get(&pool, first.page_id, 2).await.unwrap().is_none());

    // 内容未变：不写快照、不递增版本。
    let same = page::upsert(&pool, &draft(1, "entity/张三", "张三", "摘要", "第二版"), &[], &[]).await.unwrap();
    assert!(!same.changed);
    assert_eq!(same.version, 2);
    assert_eq!(revision::count(&pool, first.page_id).await.unwrap(), 1);
}

#[tokio::test]
async fn revision_prune_keeps_user_versions_longer_than_pipeline_ones() {
    let pool = database().await;
    seed_kb(&pool, 1).await;
    let first = page::upsert(&pool, &draft(1, "entity/张三", "张三", "摘要", "v1"), &[], &[]).await.unwrap();
    for index in 2..=5 {
        page::upsert(&pool, &draft(1, "entity/张三", "张三", "摘要", &format!("v{}", index)), &[], &[]).await.unwrap();
    }
    // 第一次人工编辑留下的快照仍归属 pipeline（那是被覆盖版本的作者），第二次才是 user 版本。
    super::edit::apply_user_edit(&pool, 1, "entity/张三", &edit_content("人工一")).await.unwrap();
    super::edit::apply_user_edit(&pool, 1, "entity/张三", &edit_content("人工二")).await.unwrap();
    assert_eq!(revision::count(&pool, first.page_id).await.unwrap(), 6);

    // 软限额只裁管道版本：留最新的 1 条 pipeline + 全部 user 版本。
    let removed = revision::prune(&pool, first.page_id, 1, 0).await.unwrap();
    assert_eq!(removed, 4);
    let kept: Vec<(i64, String)> = revision::list(&pool, first.page_id, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.version, r.edit_source))
        .collect();
    assert_eq!(kept, vec![(6, EDIT_SOURCE_USER.to_string()), (5, super::EDIT_SOURCE_PIPELINE.to_string())]);

    // 硬限额裁所有版本，0 表示不裁剪。
    assert_eq!(revision::prune(&pool, first.page_id, 0, 0).await.unwrap(), 0);
    assert_eq!(revision::prune(&pool, first.page_id, 0, 1).await.unwrap(), 1);
    let kept: Vec<i64> =
        revision::list(&pool, first.page_id, 10).await.unwrap().into_iter().map(|r| r.version).collect();
    assert_eq!(kept, vec![6]);
}

#[tokio::test]
async fn pipeline_does_not_overwrite_manual_edits() {
    let pool = database().await;
    seed_kb(&pool, 1).await;
    seed_file(&pool, 10, Some(1), "a.pdf").await;
    seed_file(&pool, 11, Some(1), "b.pdf").await;
    let first = page::upsert(&pool, &draft(1, "entity/张三", "张三", "摘要", "自动生成"), &[10], &[]).await.unwrap();
    super::edit::apply_user_edit(&pool, 1, "entity/张三", &edit_content("人工修订")).await.unwrap();

    // 管道再写同一 slug：内容被丢弃，证据照旧并入。
    let skipped =
        page::upsert(&pool, &draft(1, "entity/张三", "张三", "摘要", "自动生成的新版"), &[11], &[]).await.unwrap();
    assert!(skipped.skipped_manual);
    assert!(!skipped.changed);
    assert_eq!(skipped.version, 2);
    let current = page::get_by_id(&pool, first.page_id).await.unwrap().unwrap();
    assert_eq!(current.content, "人工修订");
    assert_eq!(current.last_edit_source, EDIT_SOURCE_USER);
    assert_eq!(current.last_editor_id, "alice");
    assert_eq!(page::source_file_ids(&pool, current.id).await.unwrap(), vec![10, 11]);

    // 链接维护是机械改写：照常生效，但不改作者归属，否则人工页会被后续 ingest 覆盖。
    let changed =
        page::update_content(&pool, &current, "人工修订 [[entity/李四|李四]]", &["entity/李四".to_string()], None)
            .await
            .unwrap();
    assert!(changed);
    let linked = page::get_by_id(&pool, current.id).await.unwrap().unwrap();
    assert_eq!(linked.version, 3);
    assert_eq!(linked.last_edit_source, EDIT_SOURCE_USER);
    assert_eq!(linked.out_links, vec!["entity/李四".to_string()]);
    // 内容没变时是空操作，不产生新版本。
    assert!(!page::update_content(&pool, &linked, &linked.content.clone(), &linked.out_links, None).await.unwrap());
    assert_eq!(page::get_by_id(&pool, linked.id).await.unwrap().unwrap().version, 3);
}

#[tokio::test]
async fn revert_reapplies_snapshot_as_new_version() {
    let pool = database().await;
    seed_kb(&pool, 1).await;
    let first = page::upsert(&pool, &draft(1, "entity/张三", "张三", "摘要", "第一版"), &[], &[]).await.unwrap();
    super::edit::apply_user_edit(&pool, 1, "entity/张三", &edit_content("第二版")).await.unwrap();

    let reverted = super::edit::revert(&pool, 1, "entity/张三", 1, "bob").await.unwrap();
    assert_eq!(reverted.content, "第一版");
    assert_eq!(reverted.version, 3);
    assert_eq!(reverted.last_edit_source, EDIT_SOURCE_REVERT);
    assert_eq!(reverted.last_editor_id, "bob");

    // 历史只追加：回滚前也先快照当前版本。
    let versions: Vec<i64> =
        revision::list(&pool, first.page_id, 10).await.unwrap().into_iter().map(|r| r.version).collect();
    assert_eq!(versions, vec![2, 1]);

    assert!(matches!(super::edit::revert(&pool, 1, "entity/张三", 99, "bob").await, Err(EditError::NotFound(_))));
    assert!(matches!(super::edit::revert(&pool, 1, "entity/不存在", 1, "bob").await, Err(EditError::NotFound(_))));
}

#[tokio::test]
async fn create_user_page_derives_slug_and_validates_input() {
    let pool = database().await;
    seed_kb(&pool, 1).await;

    let created = super::edit::create_user_page(
        &pool,
        1,
        &NewPage {
            aliases: vec!["RAG".to_string(), "  ".to_string(), "RAG".to_string()],
            content: "手工正文 [[entity/张三|张三]]".to_string(),
            ..new_page("检索增强生成")
        },
    )
    .await
    .unwrap();
    assert_eq!(created.slug, "concept/检索增强生成");
    assert_eq!(created.page_type, PAGE_TYPE_CONCEPT);
    assert_eq!(created.status, STATUS_PUBLISHED);
    assert_eq!(created.aliases, vec!["RAG".to_string()]);
    assert_eq!(created.out_links, vec!["entity/张三".to_string()]);
    assert_eq!(created.last_edit_source, EDIT_SOURCE_USER);
    assert_eq!(created.version, 1);
    // 建页后入队一次收敛，索引目录与入链才会带上它。
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wiki_tasks WHERE task_type = 'wiki:finalize'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(pending, 1);

    assert!(matches!(
        super::edit::create_user_page(&pool, 1, &new_page("检索增强生成")).await,
        Err(EditError::Conflict(_))
    ));
    // 摘要页绑定文档、索引页由系统维护，都不允许手工创建。
    assert!(matches!(
        super::edit::create_user_page(&pool, 1, &NewPage { page_type: Some("summary".into()), ..new_page("文档") })
            .await,
        Err(EditError::Invalid(_))
    ));
    assert!(matches!(
        super::edit::create_user_page(&pool, 1, &NewPage { title: "   ".into(), ..new_page("x") }).await,
        Err(EditError::Invalid(_))
    ));
    // 自定义 slug 会被规范化并补上类型前缀。
    let custom =
        super::edit::create_user_page(&pool, 1, &NewPage { slug: Some("My Page!".into()), ..new_page("自定义") })
            .await
            .unwrap();
    assert_eq!(custom.slug, "concept/my-page");
    assert!(matches!(
        super::edit::create_user_page(&pool, 1, &NewPage { slug: Some("///".into()), ..new_page("非法") }).await,
        Err(EditError::Invalid(_))
    ));
    // 草稿状态建页：不进目录，等发布。
    let draft_page =
        super::edit::create_user_page(&pool, 1, &NewPage { status: Some("draft".into()), ..new_page("草稿页") })
            .await
            .unwrap();
    assert_eq!(draft_page.status, super::STATUS_DRAFT);
}

#[tokio::test]
async fn edit_validation_rejects_bad_input() {
    let pool = database().await;
    seed_kb(&pool, 1).await;
    page::upsert(&pool, &draft(1, "entity/张三", "张三", "摘要", "内容"), &[], &[]).await.unwrap();
    page::upsert(
        &pool,
        &PageDraft { page_type: PAGE_TYPE_INDEX.to_string(), ..draft(1, INDEX_SLUG, "索引", "导言", "目录") },
        &[],
        &[],
    )
    .await
    .unwrap();

    assert!(matches!(
        super::edit::apply_user_edit(&pool, 1, "entity/张三", &PageEdit::default()).await,
        Err(EditError::Invalid(_))
    ));
    assert!(matches!(
        super::edit::apply_user_edit(&pool, 1, "entity/不存在", &edit_content("x")).await,
        Err(EditError::NotFound(_))
    ));
    assert!(matches!(
        super::edit::apply_user_edit(&pool, 1, INDEX_SLUG, &edit_content("x")).await,
        Err(EditError::Conflict(_))
    ));
    assert!(matches!(
        super::edit::apply_user_edit(
            &pool,
            1,
            "entity/张三",
            &PageEdit { title: Some("  ".into()), ..Default::default() }
        )
        .await,
        Err(EditError::Invalid(_))
    ));
    assert!(matches!(
        super::edit::apply_user_edit(
            &pool,
            1,
            "entity/张三",
            &PageEdit { status: Some("bogus".into()), ..Default::default() }
        )
        .await,
        Err(EditError::Invalid(_))
    ));
    // 局部编辑：只给摘要时正文与标题保持原样。
    let edited = super::edit::apply_user_edit(
        &pool,
        1,
        "entity/张三",
        &PageEdit { summary: Some("新摘要".into()), editor_id: "alice".into(), ..Default::default() },
    )
    .await
    .unwrap();
    assert_eq!(edited.summary, "新摘要");
    assert_eq!(edited.title, "张三");
    assert_eq!(edited.content, "内容");
    assert_eq!(edited.version, 2);
}

#[tokio::test]
async fn archived_pages_leave_list_search_and_links_but_stay_readable() {
    let pool = database().await;
    seed_kb(&pool, 1).await;
    page::upsert(&pool, &draft(1, "entity/张三", "张三", "摘要", "内容"), &[], &[]).await.unwrap();
    page::upsert(&pool, &draft(1, "entity/李四", "李四", "摘要", "内容"), &[], &[]).await.unwrap();

    let archived = super::edit::archive(&pool, 1, "entity/张三", "alice").await.unwrap();
    assert_eq!(archived.status, STATUS_ARCHIVED);
    assert_eq!(archived.content, "内容");

    let visible = page::list(&pool, 1, None, None, 10, None).await.unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].slug, "entity/李四");
    let hidden = page::list(&pool, 1, None, Some(STATUS_ARCHIVED), 10, None).await.unwrap();
    assert_eq!(hidden.len(), 1);
    assert!(page::search(&pool, 1, "张三", 10).await.unwrap().is_empty());
    // 归档页退出 linkify 候选：指向它的内链会被 finalize 当死链清掉。
    assert_eq!(page::list_surfaces(&pool, 1).await.unwrap().len(), 1);
    let stats = page::stats(&pool, 1).await.unwrap();
    assert_eq!((stats.total, stats.published, stats.archived), (2, 1, 1));

    // 归档不是删除：正文与历史都还在，可以随时恢复。
    let restored = super::edit::restore(&pool, 1, "entity/张三", "alice").await.unwrap();
    assert_eq!(restored.status, STATUS_PUBLISHED);
    assert_eq!(restored.version, 1, "状态切换不算内容变更，不该递增版本");

    // 删除才是真删除。
    super::edit::delete_page(&pool, 1, "entity/张三").await.unwrap();
    assert!(page::get_by_slug(&pool, 1, "entity/张三").await.unwrap().is_none());
    assert!(matches!(super::edit::delete_page(&pool, 1, "entity/张三").await, Err(EditError::NotFound(_))));
    assert!(matches!(super::edit::delete_page(&pool, 1, INDEX_SLUG).await, Err(EditError::NotFound(_))));
}

#[tokio::test]
async fn retract_archives_manual_page_and_deletes_generated_one() {
    let pool = database().await;
    seed_kb(&pool, 1).await;
    seed_file(&pool, 10, Some(1), "a.pdf").await;
    seed_file(&pool, 11, Some(1), "b.pdf").await;
    let manual = page::upsert(&pool, &draft(1, "entity/张三", "张三", "摘要", "自动"), &[10], &[]).await.unwrap();
    super::edit::apply_user_edit(&pool, 1, "entity/张三", &edit_content("人工")).await.unwrap();
    let generated = page::upsert(&pool, &draft(1, "entity/李四", "李四", "摘要", "自动"), &[11], &[]).await.unwrap();

    sqlx::query("DELETE FROM files WHERE id IN (10, 11)").execute(&pool).await.unwrap();
    let report = super::retract::retract_pages(&pool, &[manual.page_id, generated.page_id]).await.unwrap();
    assert_eq!((report.pages_archived, report.pages_deleted), (1, 1));

    let kept = page::get_by_id(&pool, manual.page_id).await.unwrap().unwrap();
    assert_eq!(kept.status, STATUS_ARCHIVED);
    assert_eq!(kept.content, "人工", "归档保留用户写的内容");
    assert!(page::get_by_id(&pool, generated.page_id).await.unwrap().is_none());
}

#[tokio::test]
async fn lint_reports_broken_links_orphans_and_stale_sources() {
    let pool = database().await;
    seed_kb(&pool, 1).await;
    seed_file(&pool, 10, Some(1), "a.pdf").await;
    seed_file(&pool, 11, Some(1), "b.pdf").await;
    // 解析未完成的来源：内容无法再被重建。
    sqlx::query("UPDATE files SET status = 0 WHERE id = 11").execute(&pool).await.unwrap();

    let healthy = page::upsert(&pool, &draft(1, "entity/张三", "张三", "摘要", "内容"), &[10], &[]).await.unwrap();
    let broken =
        page::upsert(&pool, &draft(1, "entity/孤儿", "孤儿", "", "[[entity/不存在|x]]"), &[11], &[]).await.unwrap();
    page::upsert(&pool, &draft(1, "entity/重名", "张三", "摘要", "另一个张三"), &[], &[]).await.unwrap();
    page::upsert(
        &pool,
        &PageDraft { page_type: PAGE_TYPE_INDEX.to_string(), ..draft(1, INDEX_SLUG, "索引", "导言", "目录") },
        &[],
        &[],
    )
    .await
    .unwrap();
    // 入链：健康页被索引外的页面链接一次，避免它也被判成孤儿页。
    page::set_out_links(&pool, broken.page_id, &["entity/张三".to_string()]).await.unwrap();
    page::rebuild_in_links(&pool, 1).await.unwrap();

    let report = super::lint::lint_kb(&pool, 1).await.unwrap();
    assert_eq!(report.scanned, 4, "索引页也在扫描范围内，但归档页不在");
    assert!(!report.truncated);
    let mut kinds: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for issue in &report.issues {
        *kinds.entry(issue.kind.as_str()).or_default() += 1;
    }
    assert_eq!(kinds.get(super::lint::ISSUE_BROKEN_LINK), Some(&1));
    assert_eq!(kinds.get(super::lint::ISSUE_MISSING_SUMMARY), Some(&1));
    assert_eq!(kinds.get(super::lint::ISSUE_STALE_SOURCE), Some(&1));
    assert_eq!(kinds.get(super::lint::ISSUE_DUPLICATE_TITLE), Some(&2));
    assert_eq!(kinds.get(super::lint::ISSUE_ORPHAN_PAGE), Some(&2));
    assert!(report.issues.iter().all(|issue| issue.slug != INDEX_SLUG), "索引页不体检自身");

    let broken_link = report.issues.iter().find(|i| i.kind == super::lint::ISSUE_BROKEN_LINK).unwrap();
    assert_eq!(broken_link.page_id, broken.page_id);
    assert_eq!(broken_link.target, "entity/不存在");
    assert_eq!(broken_link.severity, "warning");
    let stale = report.issues.iter().find(|i| i.kind == super::lint::ISSUE_STALE_SOURCE).unwrap();
    assert_eq!(stale.target, "11");
    // 健康页只因为标题撞车被点名，不该有别的问题。
    let healthy_issues: Vec<&str> = report
        .issues
        .iter()
        .filter(|issue| issue.page_id == healthy.page_id)
        .map(|issue| issue.kind.as_str())
        .collect();
    assert_eq!(healthy_issues, vec![super::lint::ISSUE_DUPLICATE_TITLE]);
    let by_kind: Vec<(&str, usize)> = report.by_kind.iter().map(|c| (c.kind.as_str(), c.count)).collect();
    assert_eq!(by_kind.first().unwrap().1, 2, "按数量倒序");
}

#[tokio::test]
async fn finalize_injects_cross_links_without_touching_manual_attribution() {
    let pool = database().await;
    seed_kb(&pool, 1).await;
    let zhang =
        page::upsert(&pool, &draft(1, "entity/张三", "张三", "摘要", "张三是工程师。"), &[], &[]).await.unwrap();
    page::upsert(&pool, &draft(1, "concept/rag", "RAG", "摘要", "RAG 用于问答。"), &[], &[]).await.unwrap();
    // 人工改写其中一页：finalize 仍要给它补内链，但不能把它标成管道页。
    super::edit::apply_user_edit(&pool, 1, "entity/张三", &edit_content("张三 负责 RAG 检索。")).await.unwrap();

    let report = super::finalize::finalize_kb(&pool, 1).await.unwrap();
    assert_eq!(report.pages_scanned, 2);
    assert_eq!(report.pages_changed, 1, "只有被注入链接的那一页算变化");
    assert!(!report.index_updated, "全局开关关闭时不生成索引导言");

    let linked = page::get_by_id(&pool, zhang.page_id).await.unwrap().unwrap();
    assert!(linked.content.contains("[[concept/rag|RAG]]"), "{}", linked.content);
    assert_eq!(linked.out_links, vec!["concept/rag".to_string()]);
    assert_eq!(linked.last_edit_source, EDIT_SOURCE_USER, "链接维护是机械改写，不改作者归属");
    assert_eq!(linked.version, 3);
    // 两次改写（人工编辑 + 链接注入）各留下一条快照。
    assert_eq!(revision::count(&pool, linked.id).await.unwrap(), 2);
    let rag = page::get_by_slug(&pool, 1, "concept/rag").await.unwrap().unwrap();
    assert_eq!(rag.in_links, vec!["entity/张三".to_string()]);

    // 再收敛一次：内容已稳定，不该产生新版本。
    let again = super::finalize::finalize_kb(&pool, 1).await.unwrap();
    assert_eq!(again.pages_changed, 0);
    assert_eq!(page::get_by_id(&pool, zhang.page_id).await.unwrap().unwrap().version, 3);
}

#[tokio::test]
async fn finalize_rebuilds_index_directory_and_skips_llm_when_unchanged() {
    let pool = database().await;
    seed_kb_with_wiki_config(&pool, 1, r#"{"enabled":true}"#).await;
    page::upsert(&pool, &draft(1, "entity/张三", "张三", "检索工程师", "内容"), &[], &[]).await.unwrap();

    let report = super::finalize::finalize_kb(&pool, 1).await.unwrap();
    assert!(report.index_updated);
    let index = page::get_by_slug(&pool, 1, INDEX_SLUG).await.unwrap().unwrap();
    assert_eq!(index.page_type, PAGE_TYPE_INDEX);
    assert!(index.content.contains("<!-- wiki:directory -->"), "{}", index.content);
    assert!(index.content.contains("[[entity/张三|张三]] — 检索工程师"), "{}", index.content);
    // 没有可用的 LLM 时退回静态导言，finalize 不该整体失败。
    assert!(!index.summary.is_empty());

    // 目录未变：第二次收敛不再触发改写，索引页版本保持不动。
    let again = super::finalize::finalize_kb(&pool, 1).await.unwrap();
    assert!(!again.index_updated);
    assert_eq!(page::get_by_slug(&pool, 1, INDEX_SLUG).await.unwrap().unwrap().version, index.version);

    // 归档页退出目录。
    super::edit::archive(&pool, 1, "entity/张三", "alice").await.unwrap();
    super::finalize::finalize_kb(&pool, 1).await.unwrap();
    let index = page::get_by_slug(&pool, 1, INDEX_SLUG).await.unwrap().unwrap();
    assert!(!index.content.contains("entity/张三"), "{}", index.content);
}

#[tokio::test]
async fn migrate_v8_withdraws_legacy_cross_kb_sources() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().in_memory(true).foreign_keys(true))
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../init.sql")).execute(&pool).await.unwrap();
    crate::graph::graph_manager::migrate(&pool).await.unwrap();
    sqlx::raw_sql(include_str!("migration.sql")).execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!("migration_v7.sql")).execute(&pool).await.unwrap();
    sqlx::raw_sql("INSERT INTO schema_migrations(version, name) VALUES(6,'wiki_pages'),(7,'wiki_page_revisions')")
        .execute(&pool)
        .await
        .unwrap();
    seed_kb(&pool, 1).await;
    seed_kb(&pool, 2).await;
    seed_file(&pool, 10, Some(2), "already-moved.txt").await;
    seed_file(&pool, 11, Some(1), "remaining.txt").await;
    sqlx::raw_sql("INSERT INTO wiki_pages(id,kb_id,slug,status,content) VALUES(1,1,'concept/secret','published','secret');
        INSERT INTO wiki_pages(id,kb_id,slug,status,page_type,content) VALUES(2,1,'index','published','index','secret directory');
        INSERT INTO wiki_page_sources(page_id,file_id) VALUES(1,10),(1,11)")
        .execute(&pool).await.unwrap();
    super::migrate(&pool).await.unwrap();
    assert!(super::page::get_by_slug(&pool, 1, "concept/secret").await.unwrap().is_none());
    assert!(super::page::get_by_slug(&pool, 1, "index").await.unwrap().is_none());
    assert_eq!(super::page::stats(&pool, 1).await.unwrap().total, 0);
    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM wiki_tasks WHERE task_type = 'wiki:ingest' AND file_id = 11")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pending, 1);
    let changes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wiki_index_changes").fetch_one(&pool).await.unwrap();
    assert_eq!(changes, 2);
    super::migrate(&pool).await.unwrap();
    let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wiki_tasks").fetch_one(&pool).await.unwrap();
    assert_eq!(queued, 2, "repeated migration must not enqueue another rebuild");
}

#[tokio::test]
async fn concurrent_wiki_upserts_keep_atomic_versions_without_busy_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(dir.path().join("concurrent.db"))
                .create_if_missing(true)
                .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                .busy_timeout(std::time::Duration::from_secs(10))
                .foreign_keys(true),
        )
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../init.sql")).execute(&pool).await.unwrap();
    crate::graph::graph_manager::migrate(&pool).await.unwrap();
    super::migrate(&pool).await.unwrap();
    seed_kb(&pool, 1).await;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(8));
    let mut tasks = Vec::new();
    for i in 0..8 {
        let pool = pool.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            super::page::upsert(
                &pool,
                &super::page::PageDraft {
                    kb_id: 1,
                    slug: "concept/concurrent".into(),
                    title: "Concurrent".into(),
                    page_type: "concept".into(),
                    summary: String::new(),
                    content: format!("Content {i}"),
                    aliases: vec![],
                    edit_source: "pipeline".into(),
                    editor_id: String::new(),
                },
                &[],
                &[],
            )
            .await
            .unwrap()
        }));
    }
    let mut versions = Vec::new();
    for task in tasks {
        versions.push(task.await.unwrap().version);
    }
    versions.sort_unstable();
    assert_eq!(versions, (1..=8).collect::<Vec<_>>());
    let page = super::page::get_by_slug(&pool, 1, "concept/concurrent").await.unwrap().unwrap();
    assert_eq!(page.version, 8);
    assert_eq!(super::revision::count(&pool, page.id).await.unwrap(), 7);
}
