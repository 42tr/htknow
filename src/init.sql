CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT, -- 归属用户ID
    hash TEXT NOT NULL, -- 文件HASH
    filename TEXT NOT NULL, -- 文件名称
    path TEXT NOT NULL, -- 文件路径
    content TEXT, -- 内容
    tags TEXT NOT NULL DEFAULT '', -- 标签
    status INTEGER NOT NULL DEFAULT 0, -- 状态: 0-未处理, 1-已处理, -1-处理失败
    log TEXT DEFAULT '', -- 日志
    slice_type TEXT DEFAULT '', -- 切片类型
    kb_id TEXT DEFAULT NULL, -- 知识库ID
    created_at INTEGER DEFAULT (strftime('%s','now')),
    updated_at INTEGER DEFAULT (strftime('%s','now'))
);

CREATE TABLE IF NOT EXISTS knowledge_base (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT, -- 归属用户ID
    name TEXT NOT NULL, -- 知识库名称
    description TEXT DEFAULT '', -- 知识库描述
    created_at INTEGER DEFAULT (strftime('%s','now')),
    updated_at INTEGER DEFAULT (strftime('%s','now'))
);

CREATE TABLE IF NOT EXISTS slices (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id INTEGER NOT NULL, -- 文件ID
    content TEXT NOT NULL, -- 切片内容
    created_at INTEGER DEFAULT (strftime('%s','now')),
    updated_at INTEGER DEFAULT (strftime('%s','now')),
    FOREIGN KEY (file_id) REFERENCES files(id)
);
