CREATE TABLE IF NOT EXISTS file (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT, -- 归属用户ID
    hash TEXT, -- 文件HASH
    filename TEXT NOT NULL, -- 文件名称
    path TEXT, -- 文件路径
    content TEXT, -- 内容
    tags TEXT NOT NULL DEFAULT '[]', -- 标签
    status INTEGER NOT NULL DEFAULT 0, -- 状态: 0-未处理, 1-已处理, -1-处理失败
    log TEXT DEFAULT '', -- 日志
    slices TEXT DEFAULT '[]', -- 切片信息
    slice_type TEXT, -- 切片类型
    kb_id TEXT DEFAULT NULL, -- 知识库ID
    created_at INTEGER,
    updated_at INTEGER
);

CREATE TABLE IF NOT EXISTS knowledge_base (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT, -- 归属用户ID
    name TEXT NOT NULL, -- 知识库名称
    description TEXT DEFAULT '', -- 知识库描述
    created_at INTEGER,
    updated_at INTEGER
);
