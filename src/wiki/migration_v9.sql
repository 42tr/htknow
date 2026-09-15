-- 重新解析或移动文档后，旧 Wiki 构建不能使新一轮文件处理看起来已完成。
CREATE TRIGGER IF NOT EXISTS wiki_file_reset_progress
AFTER UPDATE OF status, kb_id ON files
WHEN NEW.kb_id IS NOT OLD.kb_id
  OR (NEW.status IN (0, 2) AND OLD.status NOT IN (0, 2))
BEGIN
    DELETE FROM wiki_tasks WHERE file_id = NEW.id AND task_type = 'wiki:ingest';
    DELETE FROM wiki_builds WHERE file_id = NEW.id;
END;
