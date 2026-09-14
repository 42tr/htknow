-- 版本快照：保存**被覆盖前**的整份页面内容。
-- 当前版本始终在 wiki_pages 里，历史版本在这里；(page_id, version) 唯一 +
-- INSERT OR IGNORE 让「先快照再更新」在重试下幂等。
CREATE TABLE IF NOT EXISTS wiki_page_revisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    page_id INTEGER NOT NULL REFERENCES wiki_pages(id) ON DELETE CASCADE,
    kb_id INTEGER NOT NULL,
    version INTEGER NOT NULL,
    slug TEXT NOT NULL DEFAULT '',
    title TEXT NOT NULL DEFAULT '',
    page_type TEXT NOT NULL DEFAULT '',
    summary TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL DEFAULT '',
    aliases TEXT NOT NULL DEFAULT '[]',
    edit_source TEXT NOT NULL DEFAULT 'pipeline',  -- 该版本的作者类型
    editor_id TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_wiki_revisions_page_version ON wiki_page_revisions(page_id, version);
CREATE INDEX IF NOT EXISTS idx_wiki_revisions_kb_created ON wiki_page_revisions(kb_id, created_at DESC);
