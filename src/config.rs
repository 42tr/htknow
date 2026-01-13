//! 配置管理模块
//!
//! 支持从环境变量读取配置，预留 etcd 支持接口，并支持实时更新。

use std::sync::{Arc, RwLock};

use once_cell::sync::Lazy;

/// 全局配置单例
static CONFIG: Lazy<Arc<RwLock<AppConfig>>> = Lazy::new(|| Arc::new(RwLock::new(AppConfig::from_env())));

/// 获取当前配置的只读快照
pub fn get() -> AppConfig {
    CONFIG.read().unwrap().clone()
}

/// 重新加载配置（用于热更新）
// pub fn reload() {
//     let new_config = AppConfig::from_env();
//     let mut config = CONFIG.write().unwrap();
//     *config = new_config;
//     info!("Configuration reloaded");
// }

/// 使用新配置更新（用于 etcd watch 等场景）
// pub fn update(new_config: AppConfig) {
//     let mut config = CONFIG.write().unwrap();
//     *config = new_config;
//     info!("Configuration updated");
// }

/// 应用配置
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub services: ServicesConfig,
    pub ai: AIConfig,
    pub database: DatabaseConfig,
    pub storage: StorageConfig,
    pub search: SearchConfig,
    pub llm: LLMConfig,
}

/// 服务器配置
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// 监听地址
    pub host: String,
    /// 监听端口
    pub port: u16,
    /// 上传文件大小限制（MB）
    pub upload_limit_mb: usize,
    /// 文件处理间隔（秒）
    pub process_interval_secs: u64,
}

/// 外部服务配置
#[derive(Debug, Clone)]
pub struct ServicesConfig {
    /// MinerU PDF 解析服务地址
    pub mineru_url: String,
    /// Embedding 服务地址
    pub embedding_url: String,
    /// Rerank 服务地址
    pub rerank_url: String,
}

/// AI 模型配置
#[derive(Debug, Clone)]
pub struct AIConfig {
    /// Embedding 模型名称
    pub embedding_model: String,
    /// Embedding 向量维度
    pub embedding_dim: i32,
    /// Rerank 模型名称
    pub rerank_model: String,
    /// Rerank 分数阈值
    pub rerank_threshold: f32,
}

/// 数据库配置
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    /// 数据库连接 URL
    pub url: String,
    /// 最大连接数
    pub max_connections: u32,
    /// 忙超时（毫秒）
    pub busy_timeout_ms: u64,
}

/// 存储路径配置
#[derive(Debug, Clone)]
pub struct StorageConfig {
    /// LanceDB 存储路径
    pub lancedb_path: String,
    /// 临时目录路径
    pub temp_path: String,
    /// 图片保存路径
    pub images_path: String,
}

/// 搜索配置
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// 搜索结果限制数
    pub limit: usize,
    /// Tantivy 索引路径
    pub tantivy_index_path: String,
    /// Tantivy 索引内存（MB）
    pub tantivy_memory_mb: usize,
}

/// LLM 配置
#[derive(Debug, Clone)]
pub struct LLMConfig {
    /// LLM API URL
    pub api_url: Option<String>,
    /// LLM API Key
    pub api_key: Option<String>,
    /// LLM 模型名称
    pub model: String,
}

impl AppConfig {
    /// 从环境变量加载配置
    pub fn from_env() -> Self {
        Self {
            server: ServerConfig::from_env(),
            services: ServicesConfig::from_env(),
            ai: AIConfig::from_env(),
            database: DatabaseConfig::from_env(),
            storage: StorageConfig::from_env(),
            search: SearchConfig::from_env(),
            llm: LLMConfig::from_env(),
        }
    }
}

impl ServerConfig {
    fn from_env() -> Self {
        Self {
            host: env_or("HTKNOW_SERVER_HOST", "0.0.0.0"),
            port: env_or_parse("HTKNOW_SERVER_PORT", 3000),
            upload_limit_mb: env_or_parse("HTKNOW_SERVER_UPLOAD_LIMIT_MB", 500),
            process_interval_secs: env_or_parse("HTKNOW_SERVER_PROCESS_INTERVAL_SECS", 10),
        }
    }
}

impl ServicesConfig {
    fn from_env() -> Self {
        Self {
            mineru_url: env_or("HTKNOW_MINERU_URL", "http://192.168.0.46:10001/file_parse"),
            embedding_url: env_or("HTKNOW_EMBEDDING_URL", "http://222.190.139.186:59700/v1/embeddings"),
            rerank_url: env_or("HTKNOW_RERANK_URL", "http://222.190.139.186:59600/v1/rerank"),
        }
    }
}

impl AIConfig {
    fn from_env() -> Self {
        Self {
            embedding_model: env_or("HTKNOW_EMBEDDING_MODEL", "bge-m3"),
            embedding_dim: env_or_parse("HTKNOW_EMBEDDING_DIM", 1024),
            rerank_model: env_or("HTKNOW_RERANK_MODEL", "bge-rerank"),
            rerank_threshold: env_or_parse("HTKNOW_RERANK_THRESHOLD", 0.1),
        }
    }
}

impl DatabaseConfig {
    fn from_env() -> Self {
        Self {
            url: env_or("DATABASE_URL", "sqlite://data/app.sqlite"),
            max_connections: env_or_parse("HTKNOW_DB_MAX_CONNECTIONS", 10),
            busy_timeout_ms: env_or_parse("HTKNOW_DB_BUSY_TIMEOUT_MS", 5000),
        }
    }
}

impl StorageConfig {
    fn from_env() -> Self {
        let data_dir = env_or("HTKNOW_DATA_DIR", "data");
        Self {
            lancedb_path: env_or("HTKNOW_LANCEDB_PATH", &format!("{}/lancedb_data", data_dir)),
            temp_path: env_or("HTKNOW_TEMP_PATH", &format!("{}/temp", data_dir)),
            images_path: env_or("HTKNOW_IMAGES_PATH", &format!("{}/images", data_dir)),
        }
    }
}

impl SearchConfig {
    fn from_env() -> Self {
        let data_dir = env_or("HTKNOW_DATA_DIR", "data");
        Self {
            limit: env_or_parse("HTKNOW_SEARCH_LIMIT", 10),
            tantivy_index_path: env_or("HTKNOW_TANTIVY_INDEX_PATH", &format!("{}/tantivy_index", data_dir)),
            tantivy_memory_mb: env_or_parse("HTKNOW_TANTIVY_MEMORY_MB", 50),
        }
    }
}

impl LLMConfig {
    fn from_env() -> Self {
        Self {
            api_url: std::env::var("LLM_API_URL").ok(),
            api_key: std::env::var("LLM_API_KEY").ok(),
            model: env_or("LLM_MODEL", "gpt-3.5-turbo"),
        }
    }

    /// 检查 LLM 是否已启用
    pub fn is_enabled(&self) -> bool {
        self.api_url.is_some()
    }
}

/// 从环境变量读取字符串，如果不存在则返回默认值
fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// 从环境变量读取并解析值，如果不存在或解析失败则返回默认值
fn env_or_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

// ============================================================================
// etcd 支持（可选功能）
// ============================================================================

/// 配置加载器 trait
#[allow(dead_code)]
pub trait ConfigLoader: Send+Sync {
    /// 加载配置
    fn load(&self) -> anyhow::Result<AppConfig>;
}

/// 环境变量配置加载器
#[allow(dead_code)]
pub struct EnvConfigLoader;

impl ConfigLoader for EnvConfigLoader {
    fn load(&self) -> anyhow::Result<AppConfig> {
        Ok(AppConfig::from_env())
    }
}

/// etcd 配置加载器（预留接口）
#[cfg(feature = "etcd")]
#[allow(dead_code)]
pub struct EtcdConfigLoader {
    pub endpoints: Vec<String>,
    pub prefix: String,
}

#[cfg(feature = "etcd")]
#[allow(dead_code)]
impl EtcdConfigLoader {
    pub fn new(endpoints: Vec<String>, prefix: String) -> Self {
        Self { endpoints, prefix }
    }

    /// 启动 watch 监听配置变化
    pub async fn watch<F>(&self, _callback: F) -> anyhow::Result<()>
    where
        F: Fn(AppConfig)+Send+Sync+'static, {
        // TODO: 实现 etcd watch 逻辑
        // 1. 连接 etcd
        // 2. 监听 prefix 下的 key 变化
        // 3. 当配置变化时调用 callback
        unimplemented!("etcd watch not implemented yet")
    }
}

#[cfg(feature = "etcd")]
impl ConfigLoader for EtcdConfigLoader {
    fn load(&self) -> anyhow::Result<AppConfig> {
        // TODO: 实现从 etcd 读取配置
        // 1. 连接 etcd
        // 2. 读取 prefix 下的所有 key
        // 3. 解析并构建 AppConfig
        // 4. 如果 etcd 中没有配置，回退到环境变量
        unimplemented!("etcd config loading not implemented yet")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AppConfig::from_env();

        // 验证默认值
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.server.upload_limit_mb, 500);
        assert_eq!(config.ai.embedding_model, "bge-m3");
        assert_eq!(config.ai.embedding_dim, 1024);
    }

    #[test]
    fn test_get_config() {
        let config = get();
        assert!(!config.server.host.is_empty());
    }
}
