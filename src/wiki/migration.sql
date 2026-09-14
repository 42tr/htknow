CREATE TABLE IF NOT EXISTS wiki_pages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kb_id INTEGER NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    slug TEXT NOT NULL,                        -- summary/<file_id> | entity/<slug> | concept/<slug> | index
    title TEXT NOT NULL DEFAULT '',
    page_type TEXT NOT NULL DEFAULT 'summary', -- summary | entity | concept | index
    status TEXT NOT NULL DEFAULT 'draft',      -- draft | published | archived
    summary TEXT NOT NULL DEFAULT '',          -- 索引列表用的一句话摘要
    content TEXT NOT NULL DEFAULT '',          -- Markdown 正文
    aliases TEXT NOT NULL DEFAULT '[]',        -- JSON 数组
    out_links TEXT NOT NULL DEFAULT '[]',      -- JSON 数组，本页链出的 slug
    in_links TEXT NOT NULL DEFAULT '[]',       -- JSON 数组，链入本页的 slug
    version INTEGER NOT NULL DEFAULT 1,        -- 仅当用户可见字段变化时递增
    last_edit_source TEXT NOT NULL DEFAULT 'pipeline', -- pipeline | user | revert
    last_editor_id TEXT NOT NULL DEFAULT '',
    content_fingerprint TEXT NOT NULL DEFAULT '',      -- 内容未变则跳过写入与版本递增
    created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_wiki_pages_kb_slug ON wiki_pages(kb_id, slug);
CREATE INDEX IF NOT EXISTS idx_wiki_pages_kb_type ON wiki_pages(kb_id, page_type, status, title);
CREATE INDEX IF NOT EXISTS idx_wiki_pages_kb_updated ON wiki_pages(kb_id, updated_at DESC, id DESC);

-- 文档级来源：页面由哪些文件贡献。文件删除时级联，retract 依赖触发器提前捕获。
CREATE TABLE IF NOT EXISTS wiki_page_sources (
    page_id INTEGER NOT NULL REFERENCES wiki_pages(id) ON DELETE CASCADE,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    PRIMARY KEY (page_id, file_id)
);
CREATE INDEX IF NOT EXISTS idx_wiki_page_sources_file ON wiki_page_sources(file_id);

-- 切片级证据：页面内容依据哪些切片写成，前端可跳转到原文高亮。
CREATE TABLE IF NOT EXISTS wiki_page_slice_refs (
    page_id INTEGER NOT NULL REFERENCES wiki_pages(id) ON DELETE CASCADE,
    slice_id INTEGER NOT NULL REFERENCES slices(id) ON DELETE CASCADE,
    PRIMARY KEY (page_id, slice_id)
);
CREATE INDEX IF NOT EXISTS idx_wiki_page_slice_refs_slice ON wiki_page_slice_refs(slice_id);

-- 待办队列：不依赖 Redis，沿用 files.status + parse_run_id 的令牌式认领思路。
CREATE TABLE IF NOT EXISTS wiki_tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kb_id INTEGER NOT NULL DEFAULT 0,
    task_type TEXT NOT NULL,                   -- wiki:ingest | wiki:finalize | wiki:retract | wiki:refresh
    op TEXT NOT NULL DEFAULT 'add',
    file_id INTEGER,
    slug TEXT NOT NULL DEFAULT '',
    payload TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending',    -- pending | claimed
    fail_count INTEGER NOT NULL DEFAULT 0,
    not_before INTEGER NOT NULL DEFAULT 0,     -- 防抖/退避，秒级时间戳
    run_id TEXT NOT NULL DEFAULT '',
    claimed_at INTEGER,
    last_error TEXT NOT NULL DEFAULT '',
    enqueued_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
CREATE INDEX IF NOT EXISTS idx_wiki_tasks_claim ON wiki_tasks(status, not_before, id);
CREATE INDEX IF NOT EXISTS idx_wiki_tasks_kb ON wiki_tasks(kb_id, task_type, status);
CREATE INDEX IF NOT EXISTS idx_wiki_tasks_file ON wiki_tasks(file_id, task_type);

-- 构建指纹：相同内容 + 相同模型/配置的已完成构建直接复用，避免重复生成。
CREATE TABLE IF NOT EXISTS wiki_builds (
    file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending',    -- pending | running | completed | failed
    run_id TEXT NOT NULL DEFAULT '',
    fingerprint TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    page_count INTEGER NOT NULL DEFAULT 0,
    error TEXT NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);

ALTER TABLE knowledge_bases ADD COLUMN wiki_config TEXT;

-- 文件删除时提前捕获受影响页面：wiki_page_sources 会随文件级联删除，
-- 异步 retract 任务运行时已经查不到来源关系。
CREATE TRIGGER IF NOT EXISTS wiki_file_delete BEFORE DELETE ON files BEGIN
    INSERT INTO wiki_tasks(kb_id, task_type, op, file_id, slug, payload)
    SELECT COALESCE(OLD.kb_id, 0), 'wiki:retract', 'remove', OLD.id, '',
           (SELECT group_concat(s.page_id, ',') FROM wiki_page_sources s WHERE s.file_id = OLD.id)
    WHERE EXISTS(SELECT 1 FROM wiki_page_sources s WHERE s.file_id = OLD.id);
END;
