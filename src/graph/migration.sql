CREATE TABLE IF NOT EXISTS graph_node_sources (
    id INTEGER PRIMARY KEY,
    node_id INTEGER NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    slice_id INTEGER REFERENCES slices(id) ON DELETE CASCADE,
    start_offset INTEGER,
    end_offset INTEGER,
    context TEXT NOT NULL DEFAULT '',
    properties TEXT NOT NULL DEFAULT '{}'
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_graph_node_source_identity
ON graph_node_sources(node_id, file_id, COALESCE(slice_id, -1), COALESCE(start_offset, -1));
CREATE INDEX IF NOT EXISTS idx_graph_sources_file ON graph_node_sources(file_id, node_id);
CREATE INDEX IF NOT EXISTS idx_graph_sources_slice ON graph_node_sources(slice_id);
CREATE TABLE IF NOT EXISTS graph_edge_sources (
    edge_id INTEGER NOT NULL REFERENCES graph_edges(id) ON DELETE CASCADE,
    slice_id INTEGER NOT NULL REFERENCES slices(id) ON DELETE CASCADE,
    context TEXT NOT NULL,
    PRIMARY KEY(edge_id, slice_id, context)
);
CREATE INDEX IF NOT EXISTS idx_graph_edge_sources_slice ON graph_edge_sources(slice_id);
CREATE TABLE IF NOT EXISTS graph_builds (
    file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    run_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    extractor_version INTEGER NOT NULL DEFAULT 1,
    error TEXT,
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
-- Preserve known legacy provenance; no original quotation is invented.
INSERT OR IGNORE INTO graph_node_sources(node_id, file_id, properties)
SELECT n.id, n.file_id, COALESCE(n.properties, '{}') FROM graph_nodes n JOIN files f ON f.id=n.file_id AND f.kb_id IS n.kb_id;
INSERT OR IGNORE INTO graph_node_sources(node_id, file_id, properties)
SELECT n.id, e.file_id, '{}' FROM graph_edges e JOIN graph_nodes n
ON n.id=e.source_node_id OR n.id=e.target_node_id JOIN files f ON f.id=e.file_id AND f.kb_id IS n.kb_id
AND (n.kb_id IS NOT NULL OR n.file_id=f.id);
-- NULL KBs are isolated by file. Consolidate repeated builds within the same file only.
CREATE TEMP TABLE graph_node_merge(old_id INTEGER PRIMARY KEY, new_id INTEGER NOT NULL);
INSERT INTO graph_node_merge
SELECT n.id, MIN(c.id) FROM graph_nodes n JOIN graph_nodes c
ON c.name=n.name AND c.entity_type=n.entity_type AND c.file_id=n.file_id AND c.kb_id IS NULL
WHERE n.kb_id IS NULL AND n.file_id IS NOT NULL GROUP BY n.id HAVING n.id<>MIN(c.id);
UPDATE graph_edges SET source_node_id=(SELECT new_id FROM graph_node_merge WHERE old_id=source_node_id)
WHERE source_node_id IN (SELECT old_id FROM graph_node_merge);
UPDATE graph_edges SET target_node_id=(SELECT new_id FROM graph_node_merge WHERE old_id=target_node_id)
WHERE target_node_id IN (SELECT old_id FROM graph_node_merge);
UPDATE OR IGNORE graph_node_sources SET node_id=(SELECT new_id FROM graph_node_merge WHERE old_id=node_id)
WHERE node_id IN (SELECT old_id FROM graph_node_merge);
UPDATE entity_mentions SET node_id=(SELECT new_id FROM graph_node_merge WHERE old_id=node_id)
WHERE node_id IN (SELECT old_id FROM graph_node_merge);
DELETE FROM graph_nodes WHERE id IN (SELECT old_id FROM graph_node_merge);
DROP TABLE graph_node_merge;
CREATE UNIQUE INDEX IF NOT EXISTS idx_graph_unassigned_identity ON graph_nodes(name,entity_type,file_id)
WHERE kb_id IS NULL AND file_id IS NOT NULL;
-- Consolidate duplicate legacy edges before installing the uniqueness constraint.
DELETE FROM graph_edges WHERE id NOT IN (
    SELECT MIN(id) FROM graph_edges GROUP BY source_node_id, target_node_id, relation_type, file_id
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_graph_edge_identity
ON graph_edges(source_node_id, target_node_id, relation_type, COALESCE(file_id, -1));
CREATE INDEX IF NOT EXISTS idx_graph_edges_file ON graph_edges(file_id, source_node_id, target_node_id);
CREATE INDEX IF NOT EXISTS idx_graph_nodes_kb_created ON graph_nodes(kb_id, created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_graph_nodes_kb_type ON graph_nodes(kb_id, entity_type, id);
-- File removal must also clean graph data on legacy tables without file foreign keys.
CREATE TRIGGER IF NOT EXISTS graph_file_delete BEFORE DELETE ON files BEGIN
    DELETE FROM graph_edges WHERE file_id=OLD.id;
    DELETE FROM graph_node_sources WHERE file_id=OLD.id;
    DELETE FROM graph_nodes WHERE file_id=OLD.id
        AND NOT EXISTS(SELECT 1 FROM graph_node_sources s WHERE s.node_id=graph_nodes.id)
        AND NOT EXISTS(SELECT 1 FROM graph_edges e WHERE e.source_node_id=graph_nodes.id OR e.target_node_id=graph_nodes.id);
    UPDATE graph_nodes SET file_id=(SELECT MIN(s.file_id) FROM graph_node_sources s WHERE s.node_id=graph_nodes.id),
        properties=COALESCE((SELECT s.properties FROM graph_node_sources s WHERE s.node_id=graph_nodes.id ORDER BY s.file_id, s.id LIMIT 1), '{}')
        WHERE file_id=OLD.id;
END;
-- Trigram lookup preserves substring search for names of three or more characters.
CREATE VIRTUAL TABLE IF NOT EXISTS graph_node_names USING fts5(name, content='graph_nodes', content_rowid='id', tokenize='trigram');
INSERT INTO graph_node_names(graph_node_names) VALUES('rebuild');
CREATE TRIGGER IF NOT EXISTS graph_names_insert AFTER INSERT ON graph_nodes BEGIN
    INSERT INTO graph_node_names(rowid,name) VALUES(NEW.id,NEW.name);
END;
CREATE TRIGGER IF NOT EXISTS graph_names_delete AFTER DELETE ON graph_nodes BEGIN
    INSERT INTO graph_node_names(graph_node_names,rowid,name) VALUES('delete',OLD.id,OLD.name);
END;
CREATE TRIGGER IF NOT EXISTS graph_names_update AFTER UPDATE OF name ON graph_nodes BEGIN
    INSERT INTO graph_node_names(graph_node_names,rowid,name) VALUES('delete',OLD.id,OLD.name);
    INSERT INTO graph_node_names(rowid,name) VALUES(NEW.id,NEW.name);
END;
-- Slice replacement can invalidate evidence belonging to files sharing a parse artifact.
CREATE TRIGGER IF NOT EXISTS graph_slice_delete BEFORE DELETE ON slices BEGIN
    UPDATE graph_builds SET status='failed',error='Source slice removed; rebuild required',updated_at=strftime('%s','now')
        WHERE file_id IN (SELECT file_id FROM graph_node_sources WHERE slice_id=OLD.id);
    DELETE FROM graph_edges WHERE id IN (SELECT edge_id FROM graph_edge_sources WHERE slice_id=OLD.id)
        AND NOT EXISTS(SELECT 1 FROM graph_edge_sources s WHERE s.edge_id=graph_edges.id AND s.slice_id<>OLD.id);
    DELETE FROM graph_nodes WHERE EXISTS(SELECT 1 FROM graph_node_sources s WHERE s.node_id=graph_nodes.id AND s.slice_id=OLD.id)
        AND NOT EXISTS(SELECT 1 FROM graph_node_sources s WHERE s.node_id=graph_nodes.id AND s.slice_id IS NOT OLD.id)
        AND NOT EXISTS(SELECT 1 FROM graph_edges e WHERE e.source_node_id=graph_nodes.id OR e.target_node_id=graph_nodes.id);
    UPDATE graph_nodes SET file_id=(SELECT MIN(s.file_id) FROM graph_node_sources s WHERE s.node_id=graph_nodes.id AND s.slice_id IS NOT OLD.id),
        properties=COALESCE((SELECT s.properties FROM graph_node_sources s WHERE s.node_id=graph_nodes.id AND s.slice_id IS NOT OLD.id ORDER BY s.file_id,s.id LIMIT 1),'{}')
        WHERE EXISTS(SELECT 1 FROM graph_node_sources s WHERE s.node_id=graph_nodes.id AND s.slice_id=OLD.id);
END;
