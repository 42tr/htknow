CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT, -- 归属用户ID
    hash TEXT NOT NULL, -- 文件HASH
    filename TEXT NOT NULL, -- 文件名称
    path TEXT NOT NULL, -- 文件路径
    content TEXT, -- 内容
    tags TEXT NOT NULL DEFAULT '', -- 标签
    status INTEGER NOT NULL DEFAULT 0, -- 状态: 0-未处理, 1-已处理, 2-处理中, -1-处理失败
    log TEXT DEFAULT '', -- 日志
    slice_type TEXT DEFAULT '', -- 切片类型
    kb_id INTEGER DEFAULT NULL, -- 知识库ID
    is_public INTEGER NOT NULL DEFAULT 0, -- 是否公开: 0-私有, 1-公开
    created_at INTEGER DEFAULT (strftime('%s','now')),
    updated_at INTEGER DEFAULT (strftime('%s','now'))
);

CREATE TABLE IF NOT EXISTS knowledge_bases (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT, -- 归属用户ID
    name TEXT NOT NULL, -- 知识库名称
    description TEXT DEFAULT '', -- 知识库描述
    parent_id INTEGER, -- 父级知识库ID
    is_public INTEGER NOT NULL DEFAULT 0, -- 是否公开: 0-私有, 1-公开
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
    text TEXT DEFAULT NULL, -- 文本内容
    text_level INTEGER DEFAULT NULL, -- 文本级别
    img_path TEXT DEFAULT NULL, -- 图片路径
    table_body TEXT DEFAULT NULL, -- 表格内容
    created_at INTEGER DEFAULT (strftime('%s','now')),
    updated_at INTEGER DEFAULT (strftime('%s','now'))
);

-- CREATE TABLE IF NOT EXISTS pdf_images (
--     id INTEGER PRIMARY KEY AUTOINCREMENT,
--     file_id INTEGER NOT NULL, -- 文件ID
--     filename TEXT NOT NULL, -- 图片文件名
--     path TEXT NOT NULL, -- 图片路径
--     page_num INTEGER, -- 所在页码
--     created_at INTEGER DEFAULT (strftime('%s','now')),
--     updated_at INTEGER DEFAULT (strftime('%s','now'))
-- );

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
CREATE INDEX IF NOT EXISTS idx_nodes_type ON graph_nodes(entity_type);
CREATE INDEX IF NOT EXISTS idx_nodes_kb ON graph_nodes(kb_id);
CREATE INDEX IF NOT EXISTS idx_nodes_file ON graph_nodes(file_id);
CREATE INDEX IF NOT EXISTS idx_nodes_name ON graph_nodes(name);
CREATE INDEX IF NOT EXISTS idx_edges_source ON graph_edges(source_node_id);
CREATE INDEX IF NOT EXISTS idx_edges_target ON graph_edges(target_node_id);
CREATE INDEX IF NOT EXISTS idx_edges_relation ON graph_edges(relation_type);
CREATE INDEX IF NOT EXISTS idx_mentions_node ON entity_mentions(node_id);
CREATE INDEX IF NOT EXISTS idx_mentions_slice ON entity_mentions(slice_id);
