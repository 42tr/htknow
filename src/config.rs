//! 配置管理模块
//!
//! 支持从环境变量读取配置，预留 etcd 支持接口，并支持实时更新。

use std::sync::{Arc, RwLock};

use once_cell::sync::Lazy;

/// 全局配置单例
///
/// 内层存 `Arc<AppConfig>`，`get()` 只克隆 Arc（廉价），避免每次深拷贝整个配置结构体。
/// 保留外层 `RwLock` 以支持未来的配置热更新（替换内层 Arc 即可）。
static CONFIG: Lazy<RwLock<Arc<AppConfig>>> = Lazy::new(|| RwLock::new(Arc::new(AppConfig::from_env())));

/// 获取当前配置的只读快照（克隆 Arc，零深拷贝）
pub fn get() -> Arc<AppConfig> {
    CONFIG.read().unwrap().clone()
}

// 重新加载配置（用于热更新）
// pub fn reload() {
//     let new_config = AppConfig::from_env();
//     let mut config = CONFIG.write().unwrap();
//     *config = new_config;
//     info!("Configuration reloaded");
// }

// 使用新配置更新（用于 etcd watch 等场景）
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
    pub slice: SliceConfig,
    pub llm: LLMConfig,
    pub wiki: WikiConfig,
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
    /// 文件处理并发数
    pub process_concurrency: usize,
    /// LanceDB 自动压缩 cron 表达式（off/disabled/0 表示禁用）
    pub lancedb_compact_cron: String,
    /// 是否启用后台文件解析
    pub parse_enabled: bool,
    /// 是否启用重复文件复用
    pub reuse_duplicate_files: bool,
    /// 文件解析后是否构建知识图谱
    pub build_knowledge_graph: bool,
}

/// 外部服务配置
#[derive(Debug, Clone)]
pub struct ServicesConfig {
    /// MinerU PDF 解析服务地址
    pub mineru_url: String,
    /// 外部接口请求超时（秒）
    pub request_timeout_secs: u64,
    /// MinerU 单次解析 PDF 最大页数（0 表示不限制）
    pub mineru_max_pages: usize,
    /// Office 文档转 PDF 服务地址
    pub office_convert_url: String,
    /// 自定义解析服务地址（配置后仅 Word/PPT/PDF 走该服务）
    pub custom_parse_url: Option<String>,
    /// 自定义解析复用服务地址（仅输入 pdf_contents）
    pub custom_parse_reuse_url: Option<String>,
    /// 音频转写服务地址
    pub audio_transcription_url: String,
    /// 音频转写服务 API Key（可选）
    pub audio_transcription_key: Option<String>,
    /// Embedding 服务地址
    pub embedding_url: String,
    /// Embedding 服务 API Key（可选）
    pub embedding_key: Option<String>,
    /// 图片 Embedding 服务地址（可选，未配置时不进行图片 embedding）
    pub image_embedding_url: Option<String>,
    /// 图片 Embedding 服务 API Key（可选）
    pub image_embedding_key: Option<String>,
    /// Rerank 服务地址
    pub rerank_url: String,
    /// Rerank 服务 API Key（可选）
    pub rerank_key: Option<String>,
    /// 图片处理模式：none、ocr 或 custom
    pub image_parse_mode: String,
    /// 图片文本化服务地址（可选）
    pub image_parse_url: Option<String>,
    /// OCR 图片文本服务地址（可选）
    pub image_ocr_url: Option<String>,
    /// 图片文本化服务超时（秒）
    pub image_parse_timeout_secs: u64,
    /// 图片文本化并发数
    pub image_parse_concurrency: usize,
}

/// AI 模型配置
#[derive(Debug, Clone)]
pub struct AIConfig {
    /// Embedding 模型名称
    pub embedding_model: String,
    /// Embedding 向量维度
    pub embedding_dim: i32,
    /// 图片 Embedding 向量维度
    pub image_embedding_dim: i32,
    /// Rerank 模型名称
    pub rerank_model: String,
    /// Rerank 分数阈值
    pub rerank_threshold: f32,
    /// Embedding 批量请求批次大小
    pub embedding_batch_size: usize,
    /// 文本批次字符预算；超长单条独立请求，不截断内容
    pub embedding_batch_max_chars: usize,
    /// 文件索引批量 embedding 超时，与交互搜索分开
    pub embedding_batch_timeout_secs: u64,
}

/// 数据库配置
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    /// 数据库连接 URL
    pub url: String,
    /// 最大连接数
    pub max_connections: u32,
    /// 最小空闲连接数（保持热连接，避免负载波动时反复建连）
    pub min_connections: u32,
    /// 忙超时（毫秒）
    pub busy_timeout_ms: u64,
    /// 是否初始化默认知识库
    pub init_default_kbs: bool,
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
    /// 转换后的 PDF 保存路径
    pub pdf_path: String,
    /// 上传文件保存路径
    pub files_path: String,
    /// 压缩文件解压后保存路径
    pub archives_path: String,
    /// 文件解析后完整文本的保存路径
    pub contents_path: String,
    /// 文件切片正文（按源文件聚合）的保存路径
    pub slice_contents_path: String,
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
    /// Tantivy 重建批次大小
    pub tantivy_rebuild_batch_size: usize,
    /// LanceDB 从 SQLite 重建时的批次大小
    pub lancedb_rebuild_batch_size: usize,
    /// 文本 embedding 请求超时（秒）
    pub embedding_timeout_secs: u64,
    /// rerank 请求超时（秒）
    pub rerank_timeout_secs: u64,
    /// 是否启用同义词扩展
    pub synonym_enabled: bool,
    /// 同义词默认权重因子
    pub synonym_boost: f32,
    /// 每个词最多扩展的同义词数
    pub max_synonyms_per_term: usize,
    /// 单次查询最多扩展的同义词总数
    pub max_total_synonyms: usize,
    /// 高亮页码选择阈值：首选页的内容字数（按 pdf_content 中文本长度计算）少于该值时，优先使用第二页（如果存在）
    pub highlight_page_min_chars: usize,
}

/// 切片配置
#[derive(Debug, Clone)]
pub struct SliceConfig {
    /// 智能切片每个切片的最大字数
    pub smart_slice_max_chars: usize,
    /// 固定长度切片的重叠字数
    pub fixed_slice_overlap_chars: usize,
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
            slice: SliceConfig::from_env(),
            llm: LLMConfig::from_env(),
            wiki: WikiConfig::from_env(),
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
            process_concurrency: env_or_parse("HTKNOW_SERVER_PROCESS_CONCURRENCY", 1),
            lancedb_compact_cron: env_or("HTKNOW_LANCEDB_COMPACT_CRON", "0 0 3 * * *"),
            parse_enabled: env_or_parse("HTKNOW_PARSE_ENABLED", true),
            reuse_duplicate_files: env_or_parse("HTKNOW_REUSE_DUPLICATE_FILES", true),
            build_knowledge_graph: env_or_parse("HTKNOW_BUILD_KNOWLEDGE_GRAPH", false),
        }
    }
}

impl ServicesConfig {
    fn from_env() -> Self {
        let image_parse_url = env_optional("HTKNOW_IMAGE_PARSE_URL");
        let image_ocr_url = env_optional("HTKNOW_IMAGE_OCR_URL");
        let default_image_mode = if image_parse_url.is_some() {
            "custom"
        } else if image_ocr_url.is_some() {
            "ocr"
        } else {
            "none"
        };
        Self {
            mineru_url: env_or("HTKNOW_MINERU_URL", "http://192.168.0.46:10001/file_parse"),
            request_timeout_secs: env_or_parse("HTKNOW_REQUEST_TIMEOUT_SECS", 600),
            mineru_max_pages: env_or_parse("HTKNOW_MINERU_MAX_PAGES", 50),
            office_convert_url: env_or("HTKNOW_OFFICE_CONVERT_URL", "http://192.168.0.46:8003/convert"),
            custom_parse_url: env_optional("HTKNOW_CUSTOM_PARSE_URL"),
            custom_parse_reuse_url: env_optional("HTKNOW_CUSTOM_PARSE_REUSE_URL"),
            audio_transcription_url: env_or(
                "HTKNOW_AUDIO_TRANSCRIPTION_URL",
                "http://192.168.0.46:59805/api/v1/audio/transcriptions",
            ),
            audio_transcription_key: std::env::var("HTKNOW_AUDIO_TRANSCRIPTION_KEY").ok(),
            embedding_url: env_or("HTKNOW_EMBEDDING_URL", "http://222.190.139.186:59700/v1/embeddings"),
            embedding_key: env_optional("HTKNOW_EMBEDDING_KEY"),
            image_embedding_url: env_optional("HTKNOW_IMAGE_EMBEDDING_URL"),
            image_embedding_key: env_optional("HTKNOW_IMAGE_EMBEDDING_KEY"),
            rerank_url: env_or("HTKNOW_RERANK_URL", "http://222.190.139.186:59600/v1/rerank"),
            rerank_key: env_optional("HTKNOW_RERANK_KEY"),
            image_parse_mode: env_or("HTKNOW_IMAGE_PARSE_MODE", default_image_mode),
            image_parse_url,
            image_ocr_url,
            image_parse_timeout_secs: env_or_parse("HTKNOW_IMAGE_PARSE_TIMEOUT_SECS", 120),
            image_parse_concurrency: env_or_parse("HTKNOW_IMAGE_PARSE_CONCURRENCY", 5),
        }
    }
}

impl AIConfig {
    fn from_env() -> Self {
        let embedding_dim = env_or_parse("HTKNOW_EMBEDDING_DIM", 1024);
        Self {
            embedding_model: env_or("HTKNOW_EMBEDDING_MODEL", "bge-m3"),
            embedding_dim,
            image_embedding_dim: env_or_parse("HTKNOW_IMAGE_EMBEDDING_DIM", 2048),
            rerank_model: env_or("HTKNOW_RERANK_MODEL", "bge-rerank"),
            rerank_threshold: env_or_parse("HTKNOW_RERANK_THRESHOLD", 0.1),
            embedding_batch_size: env_or_parse::<usize>("HTKNOW_EMBEDDING_BATCH_SIZE", 8).max(1),
            embedding_batch_max_chars: env_or_parse::<usize>("HTKNOW_EMBEDDING_BATCH_MAX_CHARS", 16000).max(1),
            embedding_batch_timeout_secs: env_or_parse::<u64>("HTKNOW_EMBEDDING_BATCH_TIMEOUT_SECS", 120).max(1),
        }
    }
}

impl DatabaseConfig {
    fn from_env() -> Self {
        Self {
            url: env_or("DATABASE_URL", "sqlite://data/app.sqlite"),
            // 文件型 SQLite 写操作串行，连接数过高只会加剧锁竞争；WAL 下读可并发，16 足够。
            max_connections: env_or_parse("HTKNOW_DB_MAX_CONNECTIONS", 16),
            min_connections: env_or_parse("HTKNOW_DB_MIN_CONNECTIONS", 2),
            busy_timeout_ms: env_or_parse("HTKNOW_DB_BUSY_TIMEOUT_MS", 5000),
            init_default_kbs: env_or_parse("HTKNOW_DB_INIT_DEFAULT_KBS", true),
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
            pdf_path: env_or("HTKNOW_PDF_PATH", &format!("{}/pdfs", data_dir)),
            files_path: env_or("HTKNOW_FILES_PATH", &format!("{}/files", data_dir)),
            archives_path: env_or("HTKNOW_ARCHIVES_PATH", &format!("{}/archives", data_dir)),
            contents_path: env_or("HTKNOW_CONTENTS_PATH", &format!("{}/contents", data_dir)),
            slice_contents_path: env_or("HTKNOW_SLICE_CONTENTS_PATH", &format!("{}/slice_contents", data_dir)),
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
            tantivy_rebuild_batch_size: env_or_parse("HTKNOW_SEARCH_TANTIVY_REBUILD_BATCH_SIZE", 100),
            lancedb_rebuild_batch_size: env_or_parse("HTKNOW_SEARCH_LANCEDB_REBUILD_BATCH_SIZE", 1000),
            embedding_timeout_secs: env_or_parse("HTKNOW_SEARCH_EMBEDDING_TIMEOUT_SECS", 30),
            rerank_timeout_secs: env_or_parse("HTKNOW_SEARCH_RERANK_TIMEOUT_SECS", 20),
            synonym_enabled: env_or_parse("HTKNOW_SEARCH_SYNONYM_ENABLED", true),
            synonym_boost: env_or_parse("HTKNOW_SEARCH_SYNONYM_BOOST", 0.7),
            max_synonyms_per_term: env_or_parse("HTKNOW_SEARCH_MAX_SYNONYMS_PER_TERM", 5),
            max_total_synonyms: env_or_parse("HTKNOW_SEARCH_MAX_TOTAL_SYNONYMS", 30),
            highlight_page_min_chars: std::env::var("HTKNOW_HIGHLIGHT_PAGE_MIN_CHARS")
                .ok()
                .and_then(|v| v.parse().ok())
                .or_else(|| std::env::var("HTKNOW_HIGHLIGHT_PAGE_MIN_POSITIONS").ok().and_then(|v| v.parse().ok()))
                .unwrap_or(20),
        }
    }
}

impl SliceConfig {
    fn from_env() -> Self {
        Self {
            smart_slice_max_chars: env_or_parse("HTKNOW_SMART_SLICE_MAX_CHARS", 8000),
            fixed_slice_overlap_chars: env_or_parse("HTKNOW_FIXED_SLICE_OVERLAP_CHARS", 100),
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

/// Wiki 生成配置
#[derive(Debug, Clone)]
pub struct WikiConfig {
    /// 是否启用 Wiki 生成（全局默认，知识库可覆盖）
    pub enabled: bool,
    /// 后台 worker 轮询间隔（秒）
    pub worker_interval_secs: u64,
    /// 单次认领的任务数
    pub batch_size: usize,
    /// 单文档内 reduce（按 slug 写页面）的并发数
    pub reduce_parallel: usize,
    /// 引用归类批次的并发数（仅在独立抽取模式下使用）
    pub citation_parallel: usize,
    /// 每个知识库同时在途的批次上限
    pub max_inflight_per_kb: usize,
    /// 单次 LLM 调用的最大输出 token
    pub llm_max_tokens: usize,
    /// finalize 防抖延迟（秒），批量导入时把知识库级收敛合并成一次
    pub finalize_delay_secs: u64,
    /// 单个任务的最大重试次数，超过后丢弃并记录错误
    pub max_fail_retries: u32,
    /// 认领后多久视为失效可回收（秒）
    pub claim_stale_secs: u64,
    /// 单文档最多生成/更新的页面数，0 表示不限制
    pub max_pages_per_ingest: usize,
    /// 送入 LLM 的单页来源正文上限（字符）
    pub max_source_chars: usize,
    /// 每页保留的**管道生成**版本上限（软裁剪，0 表示不裁剪）
    pub revision_soft_limit: usize,
    /// 每页保留的历史版本总上限（硬裁剪，0 表示不限制）
    pub revision_hard_limit: usize,
    /// 默认抽取粒度：focused / standard / exhaustive
    pub granularity: String,
    /// 默认生成语言
    pub default_language: String,
    /// Wiki 专用 LLM 地址，未配置时回退到 LLM_API_URL
    pub api_url: Option<String>,
    /// Wiki 专用 LLM Key，未配置时回退到 LLM_API_KEY
    pub api_key: Option<String>,
    /// Wiki 专用模型，未配置时回退到 LLM_MODEL
    pub model: Option<String>,
}

impl WikiConfig {
    fn from_env() -> Self {
        let llm = LLMConfig::from_env();
        Self {
            enabled: env_or_parse("HTKNOW_BUILD_WIKI", false),
            worker_interval_secs: env_or_parse("HTKNOW_WIKI_WORKER_INTERVAL_SECS", 10),
            batch_size: env_or_parse("HTKNOW_WIKI_BATCH_SIZE", 5),
            reduce_parallel: env_or_parse("HTKNOW_WIKI_REDUCE_PARALLEL", 4),
            citation_parallel: env_or_parse("HTKNOW_WIKI_CITATION_PARALLEL", 4),
            max_inflight_per_kb: env_or_parse("HTKNOW_WIKI_MAX_INFLIGHT_PER_KB", 2),
            llm_max_tokens: env_or_parse("HTKNOW_WIKI_LLM_MAX_TOKENS", 8192),
            finalize_delay_secs: env_or_parse("HTKNOW_WIKI_FINALIZE_DELAY_SECS", 20),
            max_fail_retries: env_or_parse("HTKNOW_WIKI_MAX_FAIL_RETRIES", 5),
            claim_stale_secs: env_or_parse("HTKNOW_WIKI_CLAIM_STALE_SECS", 5400),
            max_pages_per_ingest: env_or_parse("HTKNOW_WIKI_MAX_PAGES_PER_INGEST", 0),
            max_source_chars: env_or_parse("HTKNOW_WIKI_MAX_SOURCE_CHARS", 12000),
            revision_soft_limit: env_or_parse("HTKNOW_WIKI_REVISION_SOFT_LIMIT", 50),
            revision_hard_limit: env_or_parse("HTKNOW_WIKI_REVISION_HARD_LIMIT", 200),
            granularity: env_or("HTKNOW_WIKI_GRANULARITY", "standard"),
            default_language: env_or("HTKNOW_WIKI_LANGUAGE", "中文"),
            api_url: env_optional("WIKI_LLM_API_URL").or(llm.api_url),
            api_key: env_optional("WIKI_LLM_API_KEY").or(llm.api_key),
            model: env_optional("WIKI_LLM_MODEL").or(Some(llm.model)),
        }
    }

    /// 是否具备生成 Wiki 的条件（开关打开且配置了 LLM 地址）
    pub fn is_enabled(&self) -> bool {
        self.enabled && self.api_url.is_some()
    }

    /// 解析默认抽取粒度，非法值回退到 standard
    pub fn default_granularity(&self) -> crate::wiki::Granularity {
        crate::wiki::Granularity::parse(&self.granularity).unwrap_or_default()
    }
}

/// 从环境变量读取字符串，如果不存在则返回默认值
fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// 从环境变量读取字符串，空字符串视为未配置
fn env_optional(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
    })
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
pub trait ConfigLoader: Send + Sync {
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
        F: Fn(AppConfig) + Send + Sync + 'static,
    {
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
    use std::sync::Mutex;

    use once_cell::sync::Lazy;

    use super::*;

    static ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    #[test]
    fn test_default_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("HTKNOW_PARSE_ENABLED");
            std::env::remove_var("HTKNOW_BUILD_KNOWLEDGE_GRAPH");
            std::env::remove_var("HTKNOW_REQUEST_TIMEOUT_SECS");
        }
        let config = AppConfig::from_env();

        // 验证默认值
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.server.upload_limit_mb, 500);
        assert_eq!(config.ai.embedding_model, "bge-m3");
        assert_eq!(config.ai.embedding_dim, 1024);
        assert_eq!(config.ai.image_embedding_dim, 2048);
        assert_eq!(config.services.request_timeout_secs, 600);
        assert_eq!(config.search.embedding_timeout_secs, 30);
        assert_eq!(config.search.rerank_timeout_secs, 20);
        assert!(config.server.parse_enabled);
        assert!(!config.server.build_knowledge_graph);
        assert_eq!(config.search.highlight_page_min_chars, 20);
    }

    #[test]
    fn test_get_config() {
        let config = get();
        assert!(!config.server.host.is_empty());
    }

    #[test]
    fn test_parse_enabled_env_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("HTKNOW_PARSE_ENABLED", "false");
        }
        let config = AppConfig::from_env();
        assert!(!config.server.parse_enabled);
        unsafe {
            std::env::remove_var("HTKNOW_PARSE_ENABLED");
        }
    }

    #[test]
    fn test_build_knowledge_graph_env_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("HTKNOW_BUILD_KNOWLEDGE_GRAPH", "true");
        }
        let config = AppConfig::from_env();
        assert!(config.server.build_knowledge_graph);
        unsafe {
            std::env::remove_var("HTKNOW_BUILD_KNOWLEDGE_GRAPH");
        }
    }

    #[test]
    fn test_image_parse_mode_from_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let keys = ["HTKNOW_IMAGE_PARSE_MODE", "HTKNOW_IMAGE_PARSE_URL", "HTKNOW_IMAGE_OCR_URL"];
        let original = keys.map(|key| std::env::var_os(key));
        let cases = [
            (None, None, None, "none"),
            (None, Some("  "), Some(""), "none"),
            (None, Some("http://parse"), None, "custom"),
            (None, None, Some("http://ocr"), "ocr"),
            (None, Some("http://parse"), Some("http://ocr"), "custom"),
            (Some("ocr"), Some("http://parse"), Some("http://ocr"), "ocr"),
            (Some("none"), Some("http://parse"), Some("http://ocr"), "none"),
        ];
        let results: Vec<_> = cases
            .iter()
            .map(|(mode, parse, ocr, expected)| {
                for (key, value) in keys.iter().zip([mode, parse, ocr]) {
                    unsafe {
                        match value {
                            Some(value) => std::env::set_var(key, value),
                            None => std::env::remove_var(key),
                        }
                    }
                }
                (ServicesConfig::from_env().image_parse_mode, *expected)
            })
            .collect();
        for (key, value) in keys.iter().zip(original) {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        for (actual, expected) in results {
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn test_request_timeout_env_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("HTKNOW_REQUEST_TIMEOUT_SECS", "120");
        }
        let config = AppConfig::from_env();
        assert_eq!(config.services.request_timeout_secs, 120);
        unsafe {
            std::env::remove_var("HTKNOW_REQUEST_TIMEOUT_SECS");
        }
    }

    #[test]
    fn test_service_api_keys_from_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("HTKNOW_EMBEDDING_KEY", "  embed-key  ");
            std::env::set_var("HTKNOW_IMAGE_EMBEDDING_KEY", "   ");
            std::env::set_var("HTKNOW_RERANK_KEY", "rerank-key");
        }
        let services = ServicesConfig::from_env();
        // 空值与纯空白视为未配置，非空值去除首尾空白
        assert_eq!(services.embedding_key.as_deref(), Some("embed-key"));
        assert_eq!(services.image_embedding_key, None);
        assert_eq!(services.rerank_key.as_deref(), Some("rerank-key"));
        unsafe {
            std::env::remove_var("HTKNOW_EMBEDDING_KEY");
            std::env::remove_var("HTKNOW_IMAGE_EMBEDDING_KEY");
            std::env::remove_var("HTKNOW_RERANK_KEY");
        }
    }
}
