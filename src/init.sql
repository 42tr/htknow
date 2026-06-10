CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT, -- 归属用户ID
    user_name TEXT, -- 归属用户名
    hash TEXT NOT NULL, -- 文件HASH
    filename TEXT NOT NULL, -- 文件名称
    path TEXT NOT NULL, -- 文件路径
    size INTEGER NOT NULL DEFAULT 0, -- 文件大小(字节)
    content TEXT, -- 内容
    tags TEXT NOT NULL DEFAULT '', -- 标签
    status INTEGER NOT NULL DEFAULT 0, -- 状态: 0-未处理, 1-已处理, 2-处理中, 3-不解析, -1-处理失败
    log TEXT DEFAULT '', -- 日志
    slice_type TEXT DEFAULT '', -- 切片类型
    kb_id INTEGER DEFAULT NULL, -- 知识库ID
    parse_priority INTEGER NOT NULL DEFAULT 50, -- 解析队列优先级快照，避免领取待解析文件时跨表排序
    is_public INTEGER NOT NULL DEFAULT 0, -- 是否公开: 0-私有, 1-公开
    meta TEXT DEFAULT NULL, -- 元数据，存储任意JSON数据
    created_at INTEGER DEFAULT (strftime('%s','now')),
    updated_at INTEGER DEFAULT (strftime('%s','now'))
);

CREATE TABLE IF NOT EXISTS knowledge_bases (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT, -- 归属用户ID
    user_name TEXT, -- 归属用户名
    name TEXT NOT NULL, -- 知识库名称
    description TEXT DEFAULT '', -- 知识库描述
    kb_type TEXT NOT NULL DEFAULT 'analysis', -- 知识库类型: analysis-分析型, storage-存储型
    parent_id INTEGER, -- 父级知识库ID
    is_public INTEGER NOT NULL DEFAULT 0, -- 是否公开: 0-私有, 1-公开
    parse_priority INTEGER NOT NULL DEFAULT 50, -- 解析优先级: 0-100，越大越优先
    created_at INTEGER DEFAULT (strftime('%s','now')),
    updated_at INTEGER DEFAULT (strftime('%s','now')),
    FOREIGN KEY(parent_id) REFERENCES knowledge_bases(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS slices (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id INTEGER NOT NULL, -- 文件ID
    content TEXT NOT NULL, -- 切片内容
    created_at INTEGER DEFAULT (strftime('%s','now')),
    updated_at INTEGER DEFAULT (strftime('%s','now'))
);

CREATE TABLE IF NOT EXISTS pdf_contents (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id INTEGER NOT NULL, -- 文件ID
    page_idx INTEGER NOT NULL, -- 所在页码
    bbox TEXT DEFAULT NULL, -- 位置坐标 (JSON)
    text TEXT DEFAULT NULL, -- 文本内容
    text_level INTEGER DEFAULT NULL, -- 文本级别
    img_path TEXT DEFAULT NULL, -- 图片路径
    table_body TEXT DEFAULT NULL, -- 表格内容
    created_at INTEGER DEFAULT (strftime('%s','now')),
    updated_at INTEGER DEFAULT (strftime('%s','now'))
);

CREATE TABLE IF NOT EXISTS slice_positions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    slice_id INTEGER NOT NULL, -- 切片ID
    page_idx INTEGER NOT NULL, -- 所在页码（Excel 中表示 sheet 索引）
    x1 INTEGER NOT NULL,
    y1 INTEGER NOT NULL,
    x2 INTEGER NOT NULL,
    y2 INTEGER NOT NULL,
    sheet_name TEXT DEFAULT NULL, -- Excel 中所在 sheet 名称
    row_num INTEGER DEFAULT NULL, -- Excel 中所在行号（1-based）
    created_at INTEGER DEFAULT (strftime('%s','now')),
    FOREIGN KEY (slice_id) REFERENCES slices(id) ON DELETE CASCADE
);

-- 知识图谱相关表

-- 图节点表（实体）
CREATE TABLE IF NOT EXISTS graph_nodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,                    -- 实体名称
    entity_type TEXT NOT NULL,             -- 实体类型
    properties TEXT,                        -- JSON格式的属性
    embedding BLOB,                         -- 实体embedding向量
    file_id INTEGER,                        -- 来源文件ID
    kb_id INTEGER,                          -- 所属知识库ID
    is_public INTEGER NOT NULL DEFAULT 0,   -- 是否公开: 0-私有, 1-公开
    created_at INTEGER DEFAULT (strftime('%s','now')),
    updated_at INTEGER DEFAULT (strftime('%s','now')),
    UNIQUE(name, entity_type, kb_id)       -- 同一知识库内，同类型实体名称唯一
);

-- 图边表（关系）
CREATE TABLE IF NOT EXISTS graph_edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_node_id INTEGER NOT NULL,        -- 源节点ID
    target_node_id INTEGER NOT NULL,        -- 目标节点ID
    relation_type TEXT NOT NULL,            -- 关系类型
    properties TEXT,                        -- JSON格式的属性
    weight REAL DEFAULT 1.0,                -- 关系权重
    file_id INTEGER,                        -- 来源文件ID
    created_at INTEGER DEFAULT (strftime('%s','now')),
    FOREIGN KEY (source_node_id) REFERENCES graph_nodes(id) ON DELETE CASCADE,
    FOREIGN KEY (target_node_id) REFERENCES graph_nodes(id) ON DELETE CASCADE
);

-- 实体提及表（实体在文档中的出现位置）
CREATE TABLE IF NOT EXISTS entity_mentions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id INTEGER NOT NULL,               -- 节点ID
    slice_id INTEGER NOT NULL,              -- 切片ID
    start_offset INTEGER,                   -- 起始位置
    end_offset INTEGER,                     -- 结束位置
    context TEXT,                           -- 上下文片段
    created_at INTEGER DEFAULT (strftime('%s','now')),
    FOREIGN KEY (node_id) REFERENCES graph_nodes(id) ON DELETE CASCADE,
    FOREIGN KEY (slice_id) REFERENCES slices(id) ON DELETE CASCADE
);

-- 图快照表（存储序列化的图结构）
CREATE TABLE IF NOT EXISTS graph_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kb_id INTEGER,                          -- 知识库ID（NULL表示全局）
    graph_data BLOB NOT NULL,               -- 序列化的petgraph图
    node_count INTEGER,                     -- 节点数量
    edge_count INTEGER,                     -- 边数量
    version INTEGER DEFAULT 1,              -- 版本号
    created_at INTEGER DEFAULT (strftime('%s','now')),
    UNIQUE(kb_id, version)
);

-- 索引
CREATE INDEX IF NOT EXISTS idx_knowledge_bases_parent_id ON knowledge_bases(parent_id);
CREATE INDEX IF NOT EXISTS idx_knowledge_bases_user_public_parent ON knowledge_bases(user_id, is_public, parent_id);
CREATE INDEX IF NOT EXISTS idx_files_kb_id ON files(kb_id);
CREATE INDEX IF NOT EXISTS idx_files_kb_id_created_at ON files(kb_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_files_kb_id_filename ON files(kb_id, filename);
CREATE INDEX IF NOT EXISTS idx_files_kb_id_updated_at ON files(kb_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_files_kb_id_status_updated_at ON files(kb_id, status, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_files_status_updated_at ON files(status, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_files_status_id ON files(status, id);
CREATE INDEX IF NOT EXISTS idx_files_id_kb_id ON files(id, kb_id);
CREATE INDEX IF NOT EXISTS idx_files_pending_created_at ON files(created_at) WHERE status = 0;
CREATE INDEX IF NOT EXISTS idx_files_user_id_created_at ON files(user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_files_hash_slice_type_status_updated_at ON files(hash, slice_type, status, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_slices_file_id_id ON slices(file_id, id);
CREATE INDEX IF NOT EXISTS idx_pdf_contents_file_id_id ON pdf_contents(file_id, id);
CREATE INDEX IF NOT EXISTS idx_pdf_contents_file_id_page_id ON pdf_contents(file_id, page_idx, id);
CREATE INDEX IF NOT EXISTS idx_nodes_type ON graph_nodes(entity_type);
CREATE INDEX IF NOT EXISTS idx_nodes_kb ON graph_nodes(kb_id);
CREATE INDEX IF NOT EXISTS idx_nodes_file ON graph_nodes(file_id);
CREATE INDEX IF NOT EXISTS idx_nodes_name ON graph_nodes(name);
CREATE INDEX IF NOT EXISTS idx_edges_source ON graph_edges(source_node_id);
CREATE INDEX IF NOT EXISTS idx_edges_target ON graph_edges(target_node_id);
CREATE INDEX IF NOT EXISTS idx_edges_relation ON graph_edges(relation_type);
CREATE INDEX IF NOT EXISTS idx_mentions_node ON entity_mentions(node_id);
CREATE INDEX IF NOT EXISTS idx_mentions_slice ON entity_mentions(slice_id);
CREATE INDEX IF NOT EXISTS idx_slice_positions_slice ON slice_positions(slice_id);
CREATE INDEX IF NOT EXISTS idx_files_meta ON files(meta);

-- 搜索词表（分词增强）
CREATE TABLE IF NOT EXISTS search_lexicon (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    term TEXT NOT NULL UNIQUE,                -- 词条
    freq INTEGER DEFAULT NULL,                -- 词频
    tag TEXT DEFAULT NULL,                    -- 词性
    enabled INTEGER NOT NULL DEFAULT 1,       -- 是否启用
    created_at INTEGER DEFAULT (strftime('%s','now')),
    updated_at INTEGER DEFAULT (strftime('%s','now'))
);

CREATE INDEX IF NOT EXISTS idx_search_lexicon_enabled ON search_lexicon(enabled);

-- 搜索同义词词典（查询扩展）
CREATE TABLE IF NOT EXISTS search_synonyms (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    term TEXT NOT NULL,                        -- 原词
    synonym TEXT NOT NULL,                     -- 同义词
    weight REAL NOT NULL DEFAULT 1.0,          -- 行级权重
    bidirectional INTEGER NOT NULL DEFAULT 1,  -- 是否双向
    enabled INTEGER NOT NULL DEFAULT 1,        -- 是否启用
    created_at INTEGER DEFAULT (strftime('%s','now')),
    updated_at INTEGER DEFAULT (strftime('%s','now')),
    UNIQUE(term, synonym)
);

CREATE INDEX IF NOT EXISTS idx_search_synonyms_term ON search_synonyms(term);
CREATE INDEX IF NOT EXISTS idx_search_synonyms_synonym ON search_synonyms(synonym);

-- 压缩文件条目表
CREATE TABLE IF NOT EXISTS archive_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id INTEGER NOT NULL,
    entry_path TEXT NOT NULL,
    size INTEGER,
    is_directory INTEGER DEFAULT 0,
    created_at INTEGER DEFAULT (strftime('%s','now')),
    FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_archive_entries_file_id ON archive_entries(file_id);

-- 索引重建任务状态
CREATE TABLE IF NOT EXISTS index_rebuild_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    status TEXT NOT NULL DEFAULT 'idle',      -- idle/running/completed/failed
    phase TEXT NOT NULL DEFAULT '',           -- 当前阶段
    total_docs INTEGER NOT NULL DEFAULT 0,    -- 总文档数（切片 + 全文等）
    processed_docs INTEGER NOT NULL DEFAULT 0,-- 已处理文档数
    started_at INTEGER DEFAULT NULL,          -- 开始时间（秒级时间戳）
    updated_at INTEGER DEFAULT (strftime('%s','now')),
    finished_at INTEGER DEFAULT NULL,         -- 完成时间
    error TEXT DEFAULT NULL,                  -- 错误信息
    meta TEXT DEFAULT NULL                    -- 扩展信息（JSON）
);

CREATE INDEX IF NOT EXISTS idx_index_rebuild_jobs_status ON index_rebuild_jobs(status);
CREATE INDEX IF NOT EXISTS idx_index_rebuild_jobs_updated_at ON index_rebuild_jobs(updated_at);

-- 知识库成员权限表
CREATE TABLE IF NOT EXISTS kb_permissions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kb_id INTEGER NOT NULL,
    user_id TEXT NOT NULL,
    permission TEXT NOT NULL DEFAULT 'viewer', -- 'viewer' | 'editor' | 'admin'
    created_at INTEGER DEFAULT (strftime('%s','now')),
    updated_at INTEGER DEFAULT (strftime('%s','now')),
    FOREIGN KEY(kb_id) REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    UNIQUE(kb_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_kb_permissions_kb_id ON kb_permissions(kb_id);
CREATE INDEX IF NOT EXISTS idx_kb_permissions_user_id ON kb_permissions(user_id);
