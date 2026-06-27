use std::{
    collections::HashSet, fs::OpenOptions, path::{Path, PathBuf}, time::Duration
};

use anyhow::Context;
use log::info;
use sqlx::{
    Row, SqlitePool, sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteSynchronous}
};

use crate::config;

/// 初始化数据库连接池并自动创建表
///
/// 在连接之前，如果 DATABASE_URL 指向一个文件型的 SQLite 数据库（例如 `sqlite://path/to/db.sqlite`
/// 或者直接 `./data/db.sqlite`），会确保父目录存在并且数据库文件被创建（如果不存在）。
pub async fn init() -> anyhow::Result<SqlitePool> {
    // 从配置读取数据库设置
    let cfg = config::get();
    let database_url = &cfg.database.url;

    // 尝试从 URL 中解析出文件路径并提前创建目录/文件
    if let Some(db_path) = sqlite_path_from_url(database_url) {
        ensure_db_file(&db_path).with_context(|| format!("failed to ensure sqlite db file: {}", db_path.display()))?;
    } else {
        info!("Detected non-file SQLite URL or in-memory DB; skipping file creation");
    }

    anyhow::ensure!(
        cfg.database.busy_timeout_ms <= i32::MAX as u64,
        "HTKNOW_DB_BUSY_TIMEOUT_MS is too large for sqlite busy_timeout: {}",
        cfg.database.busy_timeout_ms
    );

    let is_file_db = sqlite_path_from_url(database_url).is_some();
    let mut connect_options = database_url
        .parse::<SqliteConnectOptions>()
        .with_context(|| format!("failed to parse sqlite database url: {}", database_url))?
        .create_if_missing(is_file_db)
        .foreign_keys(true)
        .busy_timeout(Duration::from_millis(cfg.database.busy_timeout_ms));

    // 文件型 SQLite 的写性能调优：WAL 下 NORMAL 同步级别足够安全且明显更快；
    // 增大页缓存与 mmap、临时表落内存，减少磁盘 IO。内存库无需这些设置。
    if is_file_db {
        connect_options = connect_options
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .pragma("cache_size", "-65536") // 64MB 页缓存（负值表示 KB）
            .pragma("mmap_size", "268435456") // 256MB 内存映射
            .pragma("temp_store", "MEMORY");
    }

    info!("Connecting to SQLite database...");

    let pool = SqlitePoolOptions::new()
        .max_connections(cfg.database.max_connections)
        .min_connections(cfg.database.min_connections)
        .connect_with(connect_options)
        .await?;

    info!("SQLite database connected successfully");

    // WAL 是数据库文件级设置；foreign_keys 和 busy_timeout 已通过连接选项应用到每条连接。
    if is_file_db {
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode = WAL;").fetch_one(&pool).await?;
        anyhow::ensure!(
            journal_mode.eq_ignore_ascii_case("wal"),
            "failed to enable sqlite WAL journal mode, actual mode: {}",
            journal_mode
        );
    }

    // 自动创建表
    create_tables(&pool).await?;
    ensure_kb_type_column(&pool).await?;
    ensure_parse_priority_column(&pool).await?;
    ensure_file_parse_priority_column(&pool).await?;
    ensure_user_name_columns(&pool).await?;
    ensure_file_size_column(&pool).await?;
    ensure_pdf_contents_bbox_column(&pool).await?;
    ensure_slice_positions_excel_columns(&pool).await?;
    if cfg.database.init_default_kbs {
        ensure_default_knowledge_bases(&pool).await?;
    } else {
        info!("Skipping default knowledge base initialization");
    }

    info!("Database initialized successfully");

    Ok(pool)
}

const DEFAULT_KNOWLEDGE_BASES: &[(i64, &str, &str, &str, i32)] = &[
    (
        1,
        "个人空间",
        "您的私有工作区。管理个人草稿、收藏的文档以及发布的方案报告。支持跨设备同步与离线访问。",
        "analysis",
        1,
    ),
    (2, "技术资料", "技术手册、船舰资料。", "analysis", 1),
    (3, "法规标准", "舰船修理法规、行业标准。", "analysis", 1),
    (4, "通用知识", "基本原理、结构组成等。", "analysis", 1),
    (5, "故障知识", "典型故障现象、原因分析等。", "analysis", 1),
    (6, "图库", "设备结构与故障样本。", "analysis", 1),
    (7, "VDR数据仓", "全船航行与运行数据回放。", "storage", 1),
];

/// 自动创建所有需要的表
async fn create_tables(pool: &SqlitePool) -> anyhow::Result<()> {
    info!("Creating tables if not exists...");

    let init_sql = include_str!("init.sql");
    for (idx, sql) in init_sql.split(";").enumerate() {
        let sql = sql.trim();
        if sql.is_empty() {
            continue;
        }
        sqlx::query(sql)
            .execute(pool)
            .await
            .with_context(|| format!("failed to execute init.sql statement {}", idx + 1))?;
    }

    info!("Tables created successfully");

    Ok(())
}

async fn ensure_default_knowledge_bases(pool: &SqlitePool) -> anyhow::Result<()> {
    let default_ids: Vec<i64> = DEFAULT_KNOWLEDGE_BASES.iter().map(|(id, ..)| *id).collect();
    let placeholders = vec!["?"; default_ids.len()].join(", ");
    let sql = format!("SELECT id FROM knowledge_bases WHERE id IN ({})", placeholders);
    let mut query = sqlx::query_scalar(&sql);
    for id in &default_ids {
        query = query.bind(id);
    }
    let existing_ids: Vec<i64> = query.fetch_all(pool).await?;
    let existing_set: HashSet<i64> = existing_ids.into_iter().collect();
    let mut inserted = 0;

    for &(id, name, description, kb_type, is_public) in DEFAULT_KNOWLEDGE_BASES {
        if existing_set.contains(&id) {
            continue;
        }
        sqlx::query(
            "INSERT INTO knowledge_bases \
            (id, user_id, user_name, name, description, kb_type, parent_id, is_public) \
            VALUES (?, ?, ?, ?, ?, ?, NULL, ?)",
        )
        .bind(id)
        .bind("")
        .bind("")
        .bind(name)
        .bind(description)
        .bind(kb_type)
        .bind(is_public)
        .execute(pool)
        .await?;
        inserted += 1;
    }

    if inserted > 0 {
        info!("Inserted {} default knowledge bases", inserted);
    }

    Ok(())
}

async fn ensure_kb_type_column(pool: &SqlitePool) -> anyhow::Result<()> {
    let columns = sqlx::query("PRAGMA table_info(knowledge_bases);").fetch_all(pool).await?;
    let has_kb_type = columns.iter().any(|row| row.get::<String, _>("name") == "kb_type");

    if !has_kb_type {
        sqlx::query("ALTER TABLE knowledge_bases ADD COLUMN kb_type TEXT NOT NULL DEFAULT 'analysis'")
            .execute(pool)
            .await?;
    }

    Ok(())
}

async fn ensure_parse_priority_column(pool: &SqlitePool) -> anyhow::Result<()> {
    let columns = sqlx::query("PRAGMA table_info(knowledge_bases);").fetch_all(pool).await?;
    let has_parse_priority = columns.iter().any(|row| row.get::<String, _>("name") == "parse_priority");

    if !has_parse_priority {
        sqlx::query("ALTER TABLE knowledge_bases ADD COLUMN parse_priority INTEGER NOT NULL DEFAULT 50")
            .execute(pool)
            .await?;
    } else {
        sqlx::query("UPDATE knowledge_bases SET parse_priority = 50 WHERE parse_priority IS NULL")
            .execute(pool)
            .await?;
    }

    Ok(())
}

async fn ensure_file_parse_priority_column(pool: &SqlitePool) -> anyhow::Result<()> {
    let columns = sqlx::query("PRAGMA table_info(files);").fetch_all(pool).await?;
    let has_parse_priority = columns.iter().any(|row| row.get::<String, _>("name") == "parse_priority");

    if !has_parse_priority {
        sqlx::query("ALTER TABLE files ADD COLUMN parse_priority INTEGER NOT NULL DEFAULT 50").execute(pool).await?;
    }

    sqlx::query(
        "UPDATE files
         SET parse_priority = COALESCE(
             (SELECT kb.parse_priority FROM knowledge_bases kb WHERE kb.id = files.kb_id),
             50
         )
         WHERE status = 0
           AND parse_priority != COALESCE(
               (SELECT kb.parse_priority FROM knowledge_bases kb WHERE kb.id = files.kb_id),
               50
           )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_files_pending_status_priority_created_at_id
         ON files(status, parse_priority DESC, created_at ASC, id ASC)
         WHERE status = 0",
    )
    .execute(pool)
    .await?;

    ensure_file_parse_priority_triggers(pool).await?;

    Ok(())
}

async fn ensure_file_parse_priority_triggers(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TRIGGER IF NOT EXISTS trg_files_parse_priority_after_insert
         AFTER INSERT ON files
         BEGIN
             UPDATE files
             SET parse_priority = COALESCE(
                 (SELECT kb.parse_priority FROM knowledge_bases kb WHERE kb.id = NEW.kb_id),
                 50
             )
             WHERE id = NEW.id
               AND parse_priority != COALESCE(
                   (SELECT kb.parse_priority FROM knowledge_bases kb WHERE kb.id = NEW.kb_id),
                   50
               );
         END",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TRIGGER IF NOT EXISTS trg_files_parse_priority_after_kb_change
         AFTER UPDATE OF kb_id ON files
         BEGIN
             UPDATE files
             SET parse_priority = COALESCE(
                 (SELECT kb.parse_priority FROM knowledge_bases kb WHERE kb.id = NEW.kb_id),
                 50
             )
             WHERE id = NEW.id
               AND parse_priority != COALESCE(
                   (SELECT kb.parse_priority FROM knowledge_bases kb WHERE kb.id = NEW.kb_id),
                   50
               );
         END",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TRIGGER IF NOT EXISTS trg_files_parse_priority_after_pending
         AFTER UPDATE OF status ON files
         WHEN NEW.status = 0
         BEGIN
             UPDATE files
             SET parse_priority = COALESCE(
                 (SELECT kb.parse_priority FROM knowledge_bases kb WHERE kb.id = NEW.kb_id),
                 50
             )
             WHERE id = NEW.id
               AND parse_priority != COALESCE(
                   (SELECT kb.parse_priority FROM knowledge_bases kb WHERE kb.id = NEW.kb_id),
                   50
               );
         END",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TRIGGER IF NOT EXISTS trg_kbs_parse_priority_update_pending_files
         AFTER UPDATE OF parse_priority ON knowledge_bases
         BEGIN
             UPDATE files
             SET parse_priority = NEW.parse_priority
             WHERE kb_id = NEW.id
               AND status = 0
               AND parse_priority != NEW.parse_priority;
         END",
    )
    .execute(pool)
    .await?;

    Ok(())
}

async fn ensure_user_name_columns(pool: &SqlitePool) -> anyhow::Result<()> {
    ensure_user_name_column(pool, "files").await?;
    ensure_user_name_column(pool, "knowledge_bases").await?;
    Ok(())
}

async fn ensure_file_size_column(pool: &SqlitePool) -> anyhow::Result<()> {
    let columns = sqlx::query("PRAGMA table_info(files);").fetch_all(pool).await?;
    let has_size = columns.iter().any(|row| row.get::<String, _>("name") == "size");

    if !has_size {
        sqlx::query("ALTER TABLE files ADD COLUMN size INTEGER NOT NULL DEFAULT 0").execute(pool).await?;
    }

    Ok(())
}

async fn ensure_user_name_column(pool: &SqlitePool, table: &str) -> anyhow::Result<()> {
    let pragma_sql = format!("PRAGMA table_info({});", table);
    let columns = sqlx::query(&pragma_sql).fetch_all(pool).await?;
    let has_user_name = columns.iter().any(|row| row.get::<String, _>("name") == "user_name");

    if !has_user_name {
        let alter_sql = format!("ALTER TABLE {} ADD COLUMN user_name TEXT NOT NULL DEFAULT ''", table);
        sqlx::query(&alter_sql).execute(pool).await?;
    } else {
        let backfill_sql = format!("UPDATE {} SET user_name = '' WHERE user_name IS NULL", table);
        sqlx::query(&backfill_sql).execute(pool).await?;
    }

    Ok(())
}

async fn ensure_pdf_contents_bbox_column(pool: &SqlitePool) -> anyhow::Result<()> {
    let columns = sqlx::query("PRAGMA table_info(pdf_contents);").fetch_all(pool).await?;
    let has_bbox = columns.iter().any(|row| row.get::<String, _>("name") == "bbox");

    if !has_bbox {
        sqlx::query("ALTER TABLE pdf_contents ADD COLUMN bbox TEXT DEFAULT NULL").execute(pool).await?;
    }

    Ok(())
}

async fn ensure_slice_positions_excel_columns(pool: &SqlitePool) -> anyhow::Result<()> {
    let columns = sqlx::query("PRAGMA table_info(slice_positions);").fetch_all(pool).await?;

    let has_sheet_name = columns.iter().any(|row| row.get::<String, _>("name") == "sheet_name");
    let has_row_num = columns.iter().any(|row| row.get::<String, _>("name") == "row_num");

    if !has_sheet_name {
        sqlx::query("ALTER TABLE slice_positions ADD COLUMN sheet_name TEXT DEFAULT NULL").execute(pool).await?;
    }
    if !has_row_num {
        sqlx::query("ALTER TABLE slice_positions ADD COLUMN row_num INTEGER DEFAULT NULL").execute(pool).await?;
    }

    Ok(())
}

/// 如果给出的 sqlite URL 指向一个文件式数据库，返回对应的文件系统路径。
/// 支持的形式（常见）：
/// - sqlite://path/to/db.sqlite
/// - sqlite:////absolute/path/to/db.sqlite
/// - file:relative/or/absolute/path.db
/// - 直接给文件路径：./data/db.sqlite 或 /abs/path/db.sqlite
///
/// 返回 None 的情况包括内存数据库（包含 "memory" 的 URL）或不能识别为文件路径的 URL。
fn sqlite_path_from_url(url: &str) -> Option<PathBuf> {
    let s = url.trim();

    if is_sqlite_memory_url(s) {
        return None;
    }

    // sqlite://path
    if let Some(rest) = s.strip_prefix("sqlite://") {
        // 有可能是 sqlite:///absolute/path（多一个斜杠），也可能是相对路径 sqlite://./db.sqlite
        // 对于前者，rest 以 / 开头，PathBuf 能正确处理
        let path = strip_uri_suffix(rest).to_string();
        if path.is_empty() {
            return None;
        }
        return Some(PathBuf::from(path));
    }

    // sqlite:... 例如 sqlite:./db.sqlite 或 sqlite::memory:
    if let Some(rest) = s.strip_prefix("sqlite:") {
        // 如果是 ::memory: 或 :memory: 等，上面已通过 contains("memory") 处理
        let path = strip_uri_suffix(rest.trim_start_matches("//")).to_string();
        if path.is_empty() {
            return None;
        }
        return Some(PathBuf::from(path));
    }

    // file: URI
    if let Some(rest) = s.strip_prefix("file:") {
        if is_sqlite_memory_url(rest) {
            return None;
        }
        // file: would be followed by a path
        let path = strip_uri_suffix(rest.trim_start_matches("//")).to_string();
        if path.is_empty() {
            return None;
        }
        return Some(PathBuf::from(path));
    }

    // 如果字符串看起来像普通的文件路径（不含 scheme://），则直接返回 PathBuf
    // 简单判断：如果包含 path separator 或以 '.' 开头，或以 '/' 开头，视为文件路径
    if s.starts_with('.') || s.starts_with('/') || s.contains(std::path::MAIN_SEPARATOR) {
        return Some(PathBuf::from(s));
    }

    // 其它情况（例如带有其他 scheme），不处理
    None
}

fn is_sqlite_memory_url(url: &str) -> bool {
    url == ":memory:" || url.contains(":memory:") || url.contains("mode=memory")
}

fn strip_uri_suffix(value: &str) -> &str {
    let query_idx = value.find('?').unwrap_or(value.len());
    let fragment_idx = value.find('#').unwrap_or(value.len());
    &value[..query_idx.min(fragment_idx)]
}

/// 确保数据库文件以及其父目录存在。如果父目录不存在会创建，数据库文件不存在会创建空文件。
fn ensure_db_file(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
        info!("Created parent directory for sqlite DB: {}", parent.display());
    }

    // 创建空文件（若已存在则不修改）
    if !path.exists() {
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .with_context(|| format!("failed to create sqlite db file {}", path.display()))?;
        info!("Created sqlite DB file: {}", path.display());
    } else {
        info!("Sqlite DB file already exists: {}", path.display());
    }

    Ok(())
}
