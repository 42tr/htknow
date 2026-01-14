use std::{
    fs::OpenOptions, path::{Path, PathBuf}
};

use anyhow::Context;
use log::info;
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};

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

    info!("Database initialized successfully");

    Ok(pool)
}

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
