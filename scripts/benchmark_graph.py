#!/usr/bin/env python3
"""Synthetic SQLite graph benchmark. Uses a temporary database; never opens application data."""
import argparse
import json
import math
from pathlib import Path
import sqlite3
import statistics
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--nodes', type=int, default=50000)
parser.add_argument('--repeats', type=int, default=20)
args = parser.parse_args()
if args.nodes < 1000 or args.repeats < 2:
    parser.error('nodes must be >= 1000 and repeats >= 2')
root = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='htknow-graph-bench-') as directory:
    db = sqlite3.connect(str(Path(directory) / 'graph.sqlite'))
    db.execute('PRAGMA foreign_keys=ON')
    db.execute('PRAGMA journal_mode=WAL')
    db.execute('PRAGMA synchronous=NORMAL')
    db.executescript((root / 'src/init.sql').read_text())
    db.executescript((root / 'src/graph/migration.sql').read_text())
    db.execute("INSERT INTO files(id,hash,filename,path,kb_id) VALUES(1,'hash','bench','bench',1)")
    start = time.perf_counter()
    db.executemany('INSERT INTO graph_nodes(id,name,entity_type,file_id,kb_id) VALUES(?,?,?,1,1)',
                   ((i, f'实体{i:08d}', '概念') for i in range(1, args.nodes + 1)))
    db.executemany('INSERT INTO graph_edges(source_node_id,target_node_id,relation_type,file_id) VALUES(?,?,?,1)',
                   ((i, (i + step - 1) % args.nodes + 1, f'关系{step}')
                    for i in range(1, args.nodes + 1) for step in (1, 7, 31, 127)))
    db.commit()
    write_seconds = time.perf_counter() - start
    db.execute('ANALYZE')
    needle = f'实体{args.nodes // 2:08d}'
    seeds = ','.join(str(i) for i in range(args.nodes // 2, args.nodes // 2 + 50))
    queries = {
        'substring_scan': ('SELECT id FROM graph_nodes WHERE kb_id=1 AND name LIKE ? LIMIT 100', (f'%{needle}%',)),
        'substring_fts': ('SELECT id FROM graph_nodes WHERE kb_id=1 AND id IN '
                          '(SELECT rowid FROM graph_node_names WHERE graph_node_names MATCH ?) LIMIT 100', (f'"{needle}"',)),
        'indexed_entity_link': ('SELECT id FROM graph_nodes WHERE name IN (?,?,?) AND kb_id=1 LIMIT 3', (needle, '不存在', '实体')),
        'batch_adjacency_50_seeds': (f'SELECT e.id,e.source_node_id,e.target_node_id FROM graph_edges e '
                                   'JOIN graph_nodes n ON n.id=e.source_node_id JOIN graph_nodes t ON t.id=e.target_node_id '
                                   f'WHERE n.kb_id=1 AND t.kb_id=1 AND e.id IN '
                                   f'(SELECT id FROM graph_edges WHERE source_node_id IN ({seeds}) UNION '
                                   f'SELECT id FROM graph_edges WHERE target_node_id IN ({seeds})) ORDER BY e.id LIMIT 401', ()),
        'grouped_node_stats': ('SELECT entity_type,COUNT(*) FROM graph_nodes WHERE kb_id=1 GROUP BY entity_type', ()),
    }
    results = {}
    for name, (sql, params) in queries.items():
        rows = db.execute(sql, params).fetchall()  # warm up
        samples = []
        for _ in range(args.repeats):
            start = time.perf_counter()
            db.execute(sql, params).fetchall()
            samples.append((time.perf_counter() - start) * 1000)
        results[name] = {
            'median_ms': round(statistics.median(samples), 3),
            'p95_ms': round(sorted(samples)[math.ceil(len(samples) * .95) - 1], 3),
            'rows': len(rows),
            'plan': [row[3] for row in db.execute('EXPLAIN QUERY PLAN ' + sql, params)],
        }
    assert results['substring_scan']['rows'] == results['substring_fts']['rows'] == 1
    print(json.dumps({'sqlite': sqlite3.sqlite_version, 'nodes': args.nodes, 'edges': args.nodes * 4,
                      'repeats': args.repeats, 'initial_write_seconds': round(write_seconds, 3),
                      'warm_query_results': results,
                      'limits': 'Synthetic local SQL measurements; excludes HTTP, permissions lookup, LLM and concurrent writes.'},
                     ensure_ascii=False, indent=2))
    db.close()
