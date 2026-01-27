use std::{
    collections::HashSet, fs::OpenOptions, path::{Path, PathBuf}
};

use anyhow::Context;
use log::info;
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};

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

    info!("Connecting to SQLite database...");

    let pool = SqlitePoolOptions::new().max_connections(cfg.database.max_connections).connect(database_url).await?;

    info!("SQLite database connected successfully");

    // 可选：设置一些 PRAGMA，以优化并保证行为
    // 开启 WAL 模式与外键支持，设置 busy_timeout（毫秒）
    // 忽略这些 PRAGMA 的错误以兼容不同环境（例如某些内存 DB URL）
    sqlx::query("PRAGMA journal_mode = WAL;").execute(&pool).await.ok();
    sqlx::query("PRAGMA foreign_keys = ON;").execute(&pool).await.ok();
    sqlx::query(&format!("PRAGMA busy_timeout = {};", cfg.database.busy_timeout_ms)).execute(&pool).await.ok();

    // 自动创建表
    create_tables(&pool).await?;
    ensure_kb_type_column(&pool).await?;
    ensure_user_name_columns(&pool).await?;
    ensure_file_size_column(&pool).await?;
    ensure_pdf_contents_bbox_column(&pool).await?;
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
    for sql in init_sql.split(";") {
        if sql.is_empty() {
            continue;
        }
        sqlx::query(sql).execute(pool).await.expect("Failed to execute SQL");
    }

    info!("Tables created successfully");

    Ok(())
}

async fn ensure_default_knowledge_bases(pool: &SqlitePool) -> anyhow::Result<()> {
    let existing_ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM knowledge_bases WHERE id IN (1, 2, 3, 4, 5)").fetch_all(pool).await?;
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

    // 常见内存标识：包含 "memory"
    if s.contains("memory") {
        return None;
    }

    // sqlite://path
    if let Some(rest) = s.strip_prefix("sqlite://") {
        // 有可能是 sqlite:///absolute/path（多一个斜杠），也可能是相对路径 sqlite://./db.sqlite
        // 对于前者，rest 以 / 开头，PathBuf 能正确处理
        let path = rest.to_string();
        if path.is_empty() {
            return None;
        }
        return Some(PathBuf::from(path));
    }

    // sqlite:... 例如 sqlite:./db.sqlite 或 sqlite::memory:
    if let Some(rest) = s.strip_prefix("sqlite:") {
        // 如果是 ::memory: 或 :memory: 等，上面已通过 contains("memory") 处理
        let path = rest.trim_start_matches("//").to_string();
        if path.is_empty() {
            return None;
        }
        return Some(PathBuf::from(path));
    }

    // file: URI
    if let Some(rest) = s.strip_prefix("file:") {
        if rest.contains("memory") {
            return None;
        }
        // file: would be followed by a path
        let path = rest.trim_start_matches("//").to_string();
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

/// 确保数据库文件以及其父目录存在。如果父目录不存在会创建，数据库文件不存在会创建空文件。
fn ensure_db_file(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
            info!("Created parent directory for sqlite DB: {}", parent.display());
        }
    }

    // 创建空文件（若已存在则不修改）
    if !path.exists() {
        OpenOptions::new()
            .create(true)
            .write(true)
            .open(path)
            .with_context(|| format!("failed to create sqlite db file {}", path.display()))?;
        info!("Created sqlite DB file: {}", path.display());
    } else {
        info!("Sqlite DB file already exists: {}", path.display());
    }

    Ok(())
}
