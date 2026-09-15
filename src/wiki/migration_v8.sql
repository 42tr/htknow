-- Compact, durable change journal; deletion tombstones survive page/KB cascades.
CREATE TABLE wiki_index_changes (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    page_id INTEGER NOT NULL UNIQUE,
    kb_id INTEGER NOT NULL
);
CREATE INDEX idx_wiki_index_changes_kb_seq ON wiki_index_changes(kb_id, seq);
INSERT INTO wiki_index_changes(page_id, kb_id) SELECT id, kb_id FROM wiki_pages;
CREATE TRIGGER wiki_index_insert AFTER INSERT ON wiki_pages BEGIN
    INSERT OR REPLACE INTO wiki_index_changes(page_id, kb_id) VALUES(NEW.id, NEW.kb_id);
END;
CREATE TRIGGER wiki_index_update AFTER UPDATE OF title, summary, content, aliases, slug, status, version ON wiki_pages
WHEN OLD.title IS NOT NEW.title OR OLD.summary IS NOT NEW.summary OR OLD.content IS NOT NEW.content
  OR OLD.aliases IS NOT NEW.aliases OR OLD.slug IS NOT NEW.slug OR OLD.status IS NOT NEW.status
  OR OLD.version IS NOT NEW.version BEGIN
    INSERT OR REPLACE INTO wiki_index_changes(page_id, kb_id) VALUES(NEW.id, NEW.kb_id);
END;
CREATE TRIGGER wiki_index_delete AFTER DELETE ON wiki_pages BEGIN
    INSERT OR REPLACE INTO wiki_index_changes(page_id, kb_id) VALUES(OLD.id, OLD.kb_id);
END;

-- Withdraw derived text immediately, including manual text and its history. Keep the
-- original rows for recovery, under an internal slug inaccessible to page APIs.
-- A fresh ingest can reuse the original slug without merging withdrawn evidence.
CREATE TRIGGER wiki_file_move AFTER UPDATE OF kb_id ON files
WHEN OLD.kb_id IS NOT NEW.kb_id BEGIN
    INSERT INTO wiki_tasks(kb_id, task_type, file_id)
    SELECT DISTINCT f.kb_id, 'wiki:ingest', f.id FROM files f
      JOIN wiki_page_sources s ON s.file_id = f.id
     WHERE f.id != NEW.id AND f.kb_id = OLD.kb_id AND f.status = 1
       AND s.page_id IN (SELECT page_id FROM wiki_page_sources WHERE file_id = NEW.id);
    DELETE FROM wiki_builds WHERE file_id = NEW.id OR file_id IN (
        SELECT s.file_id FROM wiki_page_sources s WHERE s.page_id IN (
            SELECT page_id FROM wiki_page_sources WHERE file_id = NEW.id));
    UPDATE wiki_pages SET status = 'withdrawn', slug = '__withdrawn/' || id || '/' || slug,
                          version = version + 1, updated_at = strftime('%s','now')
     WHERE status != 'withdrawn' AND (
        id IN (SELECT page_id FROM wiki_page_sources WHERE file_id = NEW.id)
        OR (kb_id = OLD.kb_id AND page_type = 'index'));
    INSERT INTO wiki_tasks(kb_id, task_type) SELECT OLD.kb_id, 'wiki:finalize' WHERE OLD.kb_id IS NOT NULL;
END;

-- Repair pages left behind by moves performed before this migration existed.
CREATE TEMP TABLE wiki_legacy_moved_pages AS
    SELECT DISTINCT p.id, p.kb_id FROM wiki_pages p
      JOIN wiki_page_sources s ON s.page_id = p.id JOIN files f ON f.id = s.file_id
     WHERE f.kb_id IS NOT p.kb_id AND p.status != 'withdrawn';
INSERT INTO wiki_tasks(kb_id, task_type, file_id)
    SELECT DISTINCT f.kb_id, 'wiki:ingest', f.id FROM files f
      JOIN wiki_page_sources s ON s.file_id = f.id
      JOIN wiki_legacy_moved_pages p ON p.id = s.page_id
     WHERE f.kb_id = p.kb_id AND f.status = 1;
DELETE FROM wiki_builds WHERE file_id IN (
    SELECT s.file_id FROM wiki_page_sources s JOIN wiki_legacy_moved_pages p ON p.id = s.page_id);
UPDATE wiki_pages SET status = 'withdrawn', slug = '__withdrawn/' || id || '/' || slug,
                      version = version + 1, updated_at = strftime('%s','now')
 WHERE status != 'withdrawn' AND (id IN (SELECT id FROM wiki_legacy_moved_pages)
    OR (page_type = 'index' AND kb_id IN (SELECT kb_id FROM wiki_legacy_moved_pages)));
INSERT INTO wiki_tasks(kb_id, task_type)
    SELECT DISTINCT kb_id, 'wiki:finalize' FROM wiki_legacy_moved_pages;
DROP TABLE wiki_legacy_moved_pages;
