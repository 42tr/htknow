# Wiki（知识库百科）调研与引入设计

本文分两部分：先拆解 Tencent/WeKnora 的 Wiki 是怎么实现的，再评估 HTKnow 能否引入、以什么形态引入。

## 一、WeKnora 的 Wiki 实现

### 形态与定位

Wiki 是知识库级别的可选能力（`knowledge_bases.indexing_strategy.wiki_enabled` 开关 + `wiki_config` JSONB 调参）。文档入库后异步生成一组互相链接的 Markdown 页面，把散落的资料整理成可按主题浏览的百科：

- `summary/<knowledge_id>`：每篇源文档一页结构化摘要
- `entity/<slug>`：实体页（人物、组织、产品、地点）
- `concept/<slug>`：概念页（方法论、理论、主题）
- `index`：索引页，目录 + LLM 撰写的 intro
- `synthesis` / `comparison`：只由 Agent 通过工具创建，生成管道不产出

页面带 `status`（draft/published/archived）、`version`、文档级来源 `source_refs`、切片级证据 `chunk_refs`、互链 `in_links`/`out_links`、别名 `aliases`。目录树是独立的 `wiki_folders` 表（邻接表 + 物化 `path`），页面的 `folder_id` 是唯一真源，`category_path`/`wiki_path`/`depth` 是写时同步的反范式缓存，列表查询无需 join。

### 数据模型

| 表 | 作用 |
| --- | --- |
| `wiki_pages` | 页面主表，`(kb_id, slug)` 在未删除行上唯一，软删 |
| `wiki_folders` | 目录节点，允许空目录（先搭骨架后归档） |
| `wiki_page_revisions` | 被覆盖版本的整份快照，`(page_id, version)` 唯一 + `ON CONFLICT DO NOTHING`，让「先快照再更新」在重试下幂等 |
| `wiki_page_issues` | Agent/用户标记的页面问题，状态 pending/ignored/resolved |
| `task_pending_ops` | 通用持久化待办队列：`(task_type, scope, scope_id, dedup_key, payload, fail_count, claimed_at)` |
| `task_dead_letters` | 重试耗尽的死信归档，保留原始 payload 以便人工重放 |

版本溯源分两层：`wiki_pages.last_edit_source` 记录**当前版本**作者类型（`pipeline`/`agent`/`user`/`revert`），历史版本各自在快照里保留 `edit_source`。历史保留分级清理：软上限 50 版只裁 pipeline 写的快照，硬上限 200 版不分作者一律裁剪，优先保住人工编辑记录。

### 生成流水线：map → reduce → finalize

**触发**：文档后处理完成后 `EnqueueWikiIngest` 写一行 `task_pending_ops`，并投递一个延时 30s 的 asynq trigger（`wiki:ingest`，独立 wiki 队列）。删除/移动文档走 `EnqueueWikiRetract`，同一套持久化模型。

**批次调度**：`ProcessWikiIngest` 一次认领 ≤5 篇文档（`IngestBatchSize` 可配）。标准模式（有 Redis）不加 KB 级排他锁，多个批次用 `SELECT ... FOR UPDATE SKIP LOCKED` 认领互斥行，让一个库的积压摊到整个 worker 池；`claimed_at` 超过 90 分钟视为崩溃残留可回收。Lite 模式退化为进程内锁、每 KB 串行。每 KB 在途批次上限默认 4，防止单库大批量导入独占共享池。

**MAP（每篇文档并行，errgroup limit 默认 10）**：

1. 取 chunks 重建全文（含图片描述等富化内容）；真实文本量不足直接跳过，不让模型对着空内容编故事。
2. Pass 0 候选抽取（`WikiCandidateSlugPrompt`）：只要 name/slug/aliases/description/details 骨架，明确告诉模型「后续另有一遍负责挂证据，这里不必写详尽事实」。抽取粒度三档 `focused`/`standard`/`exhaustive`。失败时回退到旧的单发 `WikiKnowledgeExtractPrompt`。
3. 两支并行：
   - 摘要页（`WikiSummaryPrompt`）：首行必须是 `SUMMARY: 一句话`，正文 Markdown，且凡是命中候选表里的名称/别名必须写成 `[[slug|显示名]]`。刻意**不把文件名喂给模型**——扫描件常以扫描仪型号命名，喂进去只会诱导幻觉。
   - Pass 1..N 引用归类（`WikiChunkCitationPrompt`）：按 12000 runes 切批、批内并发 ≤4，让模型判定「哪些 chunk 实质讨论了该候选」（要求至少一条具体事实，排除顺带提及），输出 `{slug: [chunk_id...]}`。
4. 产出 `[]SlugUpdate`（每 slug 的新增内容 + 引用的 chunk），并把「该文档旧页面集合」与「新抽取集合」对账，生成 removals。

这里最核心的设计是**两遍式抽取**：事实以原文 chunk 的形式保留，页面由被引用的原文组织而成，而不是让模型转述。幻觉更低，且天然可溯源。

**REDUCE（按 slug 分组并行，limit 默认 10）**：

- 同 slug 的读-改-写用 Redis 锁 `wiki:slug:<kb>:<slug>`（TTL 5min，最多等 2min）串行化。拿不到锁或写失败时，贡献该 slug 的文档**重新入队**（`fail_count` 上限 5 后进死信），绝不静默丢弃——因为 finalize 只重建索引和链接，不会重跑 reduce。
- 新页面直接用被引用的 chunk 原文组织；已有页面走 `WikiPageModifySystemPrompt` + `WikiPageModifyUserPrompt`，喂 `<existing_page_content>` + `<new_information>`（引用 chunk 原文）+ 需剔除的来源，让模型做增量合并。
- **slug 句柄化**：所有出链 slug 在送入模型前替换成 `ref-N` 句柄，生成后再还原。模型永远不接触真实 slug，杜绝它改写或编造（UUID 型 summary slug 曾被改坏导致 404）。
- 页面写完立即 publish，不等 finalize，用户尽快看到内容。

**FINALIZE（防抖 20s 的 `wiki:finalize`）**：把 N 篇文档触发的 KB 级收敛合并成一次执行，包含：

- **去重**（`WikiDeduplicationPrompt`）：先用 pg_trgm 相似度（topK=5，Jaccard 下限 0.08）为每个新 item 挑出少量候选，再让模型判定是否同一实体。预筛只从 prompt 里**移除**候选，最终写入仍有 validMerge 校验兜底；语料够小时直接绕过预筛。
- **目录规划**（`WikiTaxonomyPlanPrompt`）：一次调用给整批 item 分配 ≤2 级目录路径，强制复用已有目录标签、禁止同义词目录、禁止拿 entity/concept 当目录名、要求同类 item 落在同层。已有目录超过 60 个时先用 embedding 余弦挑 top-3 相关目录再喂 prompt。结果只应用到「尚无分类」的页面，不翻动用户手工归档。
- **索引页 intro**：`WikiIndexIntroPrompt` / `WikiIndexIntroUpdatePrompt`。
- **死链清理 + 交叉链接注入**：`linkifyContent` 扫描正文，把已有页面的标题/别名自动转成 `[[slug|name]]`，并精确跳过代码围栏、行内代码、已有链接、自动链接和引用定义；reduce 失败的 slug 会从本批摘要页里抹掉。
- 空目录 prune。

**回撤**：文档删除时 summary 页直接删，实体/概念页移除该文档的独占贡献。`isKnowledgeGone` + Redis tombstone（1h）处理「删除与排队中 ingest 的竞态」，避免生成指向幽灵文档的页面。

### 消费侧

- **Agent 工具**：`wiki_search`、`wiki_read_page`（带读取预算）、`wiki_read_source_doc`、`wiki_write_page`、`wiki_replace_text`、`wiki_rename_page`、`wiki_delete_page`、`wiki_link_mutation`、`wiki_flag_issue`/`wiki_read_issue`/`wiki_update_issue`；预置 `wiki_researcher`、`wiki_fixer`、`hybrid_rag_wiki_agent` 三种角色。
- **检索加权**：`PluginWikiBoost` 在 CHUNK_RERANK 阶段把 `chunk_type=wiki_page` 的分数 ×1.3 后稳定重排。先做「结果里没有 wiki chunk 就整体跳过」的快路径，避免每轮对话都打 KB 服务。
- **质量运维**：`RunLint` 检查 orphan_page / broken_link / stale_ref / missing_cross_ref / empty_content / duplicate_slug，`AutoFix` 修可自动修复的部分。
- **可观测**：Langfuse span 树 `postprocess.wiki → extract / summary / classify / page[slug]`。
- **前端**：WikiBrowser（目录树 + 正文 + 出处）、Wiki 图谱（overview / ego 两种模式）、RevisionDrawer（版本列表、行级 diff、回滚）。
- **API**：`/api/v1/knowledgebase/:kb_id/wiki/{pages,folders,index,graph,stats,search,revisions,revert,move-page,rebuild-links,lint,auto-fix,issues}`；读 Viewer+，写要求 KB owner/admin。

### 值得直接借鉴的工程决策

1. 两遍式抽取（候选 → chunk 引用），事实留在原文，页面可溯源。
2. slug 句柄化，模型不接触真实 slug，杜绝链接腐烂。
3. map/reduce 按 slug 分组 + 锁，避免多篇文档并发覆盖同一页面。
4. 持久化待办 + claim/stale 回收 + `fail_count` + 死信 + 重启恢复，崩溃和限流都不丢贡献。
5. finalize 防抖，批量导入时 KB 级收敛只跑一次。
6. prompt 块顺序为前缀缓存设计：静态规则和文档内稳定的候选表在前，每批变化的 chunks 在后，同文档后续批次复用长前缀。
7. 版本快照「先写后改」+ 唯一索引 `ON CONFLICT DO NOTHING`，重试幂等。
8. 历史保留分级（软上限只裁机器写的，硬上限全裁），存储有界同时优先保人工编辑。
9. linkify 是 Markdown 感知的，不在代码块和已有链接里插 wiki 链接。
10. 429/配额触发时把后续 trigger 的延时切到更长的退避区间，用调度间隔而非硬失败来降速。

## 二、HTKnow 现状对照

| 能力 | HTKnow 现状 | Wiki 是否可复用 |
| --- | --- | --- |
| KB / 文件 / 切片模型 | `files`、`slices`，切片正文外置为按文件的 JSON（`src/slice_content.rs:7`） | 可直接作为 chunk 引用源 |
| 后台异步处理 | `FileProcessor` 轮询 DB + 自适应间隔 + `status`/`parse_run_id` 令牌（`src/processor.rs:568`） | 同一套模式即可，无需 Redis/asynq |
| LLM 客户端 | `WikiLlm.chat` / `chat_json`（`src/wiki/llm.rs`）、`LLMGraphExtractor`（`src/graph/llm_extractor.rs`） | Wiki 客户端支持 system+user 双消息与重试退避 |
| 实体抽取 | 已有图谱：`graph_nodes`/`graph_edges`/`entity_mentions`/`graph_node_sources`/`graph_edge_sources` + `graph_builds` 指纹复用 | **`entity_mentions` 已经把实体映射到 `slice_id`**，等于免费拿到 WeKnora 花一整遍 LLM 调用才产出的 chunk 级引用 |
| 名称检索 | `graph_node_names` FTS5 trigram 虚表（`src/graph/migration.sql:78`） | 可替代 pg_trgm 做去重预筛 |
| 检索 | Tantivy 双索引（默认 + full）+ LanceDB `documents`/`file_summaries` + rerank + 图谱扩展（`src/search/mod.rs:1746`） | 加一路 wiki 索引即可 |
| 权限 | `kb_permissions`（`src/init.sql:244`）+ `x-user-id`/`x-role` 中间件（`src/lib.rs:48`） | 页面继承 KB 权限 |
| 迁移机制 | `schema_migrations` 版本 1–4（`src/db.rs:104`），图谱占用版本 5（`src/graph/graph_manager.rs:8`） | Wiki 用版本 6–8 |
| 前端 Markdown | 无渲染器，`frontend/package.json` 只有 vue + pdfjs-dist | 缺口，需引入 marked + dompurify |
| 目录树 / 图可视化 | `KnowledgeDirectory.vue`、`GraphVisualization.vue` | 可复用 |
| Agent 框架 | 无 | Agent 维护 Wiki 这部分短期不做 |

**结论：可以引入。** HTKnow 在「实体 → 切片证据」这条链上起点比 WeKnora 更好，图谱已经产出 `entity_mentions`，能整条省掉 chunk-citation pass。主要缺口是 Markdown 渲染、页面版本管理，以及一套 KB 级收敛任务。建议做精简版而非全量对齐：Agent 工具链、issues 闭环、多租户 claim 机制在单实例部署下收益低，先不做。

## 三、引入方案

### 分期

- **P1 可浏览的 Wiki**（已完成）：数据模型 + 任务队列 + 生成管道（summary/entity/concept/index）+ 只读 API + 前端浏览器
- **P2 可维护**（已完成）：人工编辑、版本快照/diff/回滚、归档、lint、链接收敛入口；
  `wiki_folders` 目录树、页面去重合并、Agent 工具链仍延后（见「四、实现状态」）
- **P3 融入检索**（已实现）：Wiki 正文进 Tantivy/LanceDB、按排名融合与加权、普通/图谱增强搜索引用 Wiki 页

### 3.1 数据模型（migration 6，`src/wiki/migration.sql`）

```sql
CREATE TABLE wiki_pages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kb_id INTEGER NOT NULL,
    slug TEXT NOT NULL,                    -- summary/<file_id> | entity/<slug> | concept/<slug> | index
    title TEXT NOT NULL DEFAULT '',
    page_type TEXT NOT NULL DEFAULT 'summary',  -- summary|entity|concept|index
    status TEXT NOT NULL DEFAULT 'draft',       -- draft|published|archived
    summary TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL DEFAULT '',
    aliases TEXT NOT NULL DEFAULT '[]',         -- JSON array
    out_links TEXT NOT NULL DEFAULT '[]',
    in_links TEXT NOT NULL DEFAULT '[]',
    version INTEGER NOT NULL DEFAULT 1,
    last_edit_source TEXT NOT NULL DEFAULT 'pipeline',  -- pipeline|user|revert
    last_editor_id TEXT NOT NULL DEFAULT '',
    content_fingerprint TEXT NOT NULL DEFAULT '',       -- 内容未变则不 +version、不重跑
    created_at INTEGER DEFAULT (strftime('%s','now')),
    updated_at INTEGER DEFAULT (strftime('%s','now'))
);
-- HTKnow 实际使用彻底删除；目录字段仍未实现。
CREATE UNIQUE INDEX idx_wiki_pages_kb_slug ON wiki_pages(kb_id, slug);
CREATE INDEX idx_wiki_pages_kb_type ON wiki_pages(kb_id, page_type, status);

-- 文档级来源（对应 WeKnora 的 source_refs）
CREATE TABLE wiki_page_sources (
    page_id INTEGER NOT NULL REFERENCES wiki_pages(id) ON DELETE CASCADE,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    PRIMARY KEY (page_id, file_id)
);

-- 切片级证据（对应 chunk_refs），展示时按 file_id 批量读 slice_content
CREATE TABLE wiki_page_slice_refs (
    page_id INTEGER NOT NULL REFERENCES wiki_pages(id) ON DELETE CASCADE,
    slice_id INTEGER NOT NULL REFERENCES slices(id) ON DELETE CASCADE,
    PRIMARY KEY (page_id, slice_id)
);

-- 版本快照（migration 7，src/wiki/migration_v7.sql）：存**被覆盖前**的整份内容，
-- 当前版本永远在 wiki_pages 里；(page_id, version) 唯一 + INSERT OR IGNORE 让「先快照再更新」幂等
CREATE TABLE wiki_page_revisions (
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
    edit_source TEXT NOT NULL DEFAULT 'pipeline',  -- 被快照那一版的作者类型
    editor_id TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    UNIQUE (page_id, version)
);

-- 待办队列（对应 task_pending_ops），不依赖 Redis
CREATE TABLE wiki_tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kb_id INTEGER NOT NULL,
    task_type TEXT NOT NULL,               -- wiki:ingest | wiki:finalize | wiki:retract
    op TEXT NOT NULL DEFAULT 'add',        -- add|remove|slug|change
    file_id INTEGER,
    slug TEXT,
    payload TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL DEFAULT 'pending',    -- pending|claimed|done|dead
    fail_count INTEGER NOT NULL DEFAULT 0,
    not_before INTEGER NOT NULL DEFAULT 0,     -- 防抖/退避，代替 asynq.ProcessIn
    run_id TEXT,
    claimed_at INTEGER,
    last_error TEXT NOT NULL DEFAULT '',
    enqueued_at INTEGER DEFAULT (strftime('%s','now')),
    updated_at INTEGER DEFAULT (strftime('%s','now'))
);
CREATE INDEX idx_wiki_tasks_claim ON wiki_tasks(kb_id, task_type, status, not_before, id);

-- 构建指纹（对应 graph_builds），相同内容 + 相同模型/配置直接复用
CREATE TABLE wiki_builds (
    file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending',    -- pending|running|completed|failed
    run_id TEXT,
    fingerprint TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    page_count INTEGER NOT NULL DEFAULT 0,
    error TEXT NOT NULL DEFAULT '',
    updated_at INTEGER DEFAULT (strftime('%s','now'))
);

ALTER TABLE knowledge_bases ADD COLUMN wiki_config TEXT;
-- {"enabled":true,"granularity":"standard","language":"中文","model":null,"max_pages_per_ingest":0}
```

与 WeKnora 的差异及原因：

- SQLite 没有 JSONB/GIN/pg_trgm，`aliases`/`out_links` 用 TEXT 存 JSON；页面全文搜索交给 Tantivy，不用 SQL LIKE。
- 去重预筛复用已有的 FTS5 trigram 能力（`graph_node_names` 同款），或直接对 `wiki_pages.title` 建一张 FTS5 虚表。
- 页面正文留在 SQLite（每页 1–5KB，万页量级约几十 MB），不像切片那样外置——因为页面要被编辑、快照、检索。修订按软 50 / 硬 200 两级裁剪控制体积。
- `content_fingerprint` 是 HTKnow 侧的新增项：内容哈希未变则跳过写入与版本递增，抑制页面抖动。

### 3.2 队列与并发（不引 Redis）

- 新增 `WikiWorker`，照 `FileProcessor::start` 的自适应轮询写：空闲逐步拉长间隔，有任务立即回到基础间隔。
- 原子认领：`UPDATE wiki_tasks SET status='claimed', run_id=?, claimed_at=? WHERE id IN (SELECT id FROM wiki_tasks WHERE kb_id=? AND task_type=? AND status='pending' AND not_before<=? ORDER BY id LIMIT ?) RETURNING id`，与 `claim_pending_file_by_id`（`src/processor.rs:166`）同构。
- 崩溃恢复：启动时把 `status='claimed'` 且 `claimed_at` 超阈值的行复位为 `pending`（对应 WeKnora 的 `recover_pending_wiki_tasks`）。
- 同 slug 串行：进程内 `DashMap<(kb_id, slug), Arc<Mutex<()>>>`。HTKnow 是单实例部署，进程内锁足够；LLM 调用一律放在写事务之外，只在最后一个小事务里落库——与图谱构建既有约定一致。若将来多实例，再补 `claimed_at` 级别的 slug 抢占。
- 失败处理：`fail_count++` 并复位 `pending`，超过 5 次置 `dead` 并记 `last_error`；429/超时按指数退避拉长 `not_before`。
- 防抖：finalize 任务 `not_before = now + 20s`；同 KB 已有未执行的 finalize 时只合并 payload，不新增行。

### 3.3 生成管道

模块划分：

```
src/wiki/
  mod.rs         WikiPage/WikiTask 类型、slug 规范化、页面类型常量
  migration.sql  版本 6 DDL
  prompts.rs     候选抽取 / 摘要 / 页面合并 / 索引 intro / 目录规划 / 去重（中文模板）
  worker.rs      WikiWorker 轮询、认领、分发、恢复
  ingest.rs      map（单文档）+ reduce（按 slug）
  finalize.rs    索引 intro、交叉链接注入、死链清理、目录 prune
  retract.rs     文档删除/移动的回撤
  linkify.rs     Markdown 感知的 [[slug|name]] 注入
  page.rs        CRUD、版本快照、回滚、lint
```

单文档 map 支持两种模式：

**模式 A：图谱复用（推荐默认，`HTKNOW_BUILD_KNOWLEDGE_GRAPH=true` 时）**

1. 从 `graph_node_sources` / `entity_mentions` 取该文件的实体，以及每个实体的 `slice_id` 和原文上下文 → 直接得到候选 slug 与切片级引用，**省掉 Pass 0 候选抽取和整条 citation pass**。
2. 一次 LLM 调用生成文档摘要页（`SUMMARY:` 首行 + Markdown 正文 + `[[slug|显示名]]` 内链）。
3. 每个实体/概念页一次 LLM 调用：新页面用被引用切片的原文组织；已存在则走合并 prompt。
4. 需要给图谱抽取 prompt 补 `aliases` 与一句话 `description` 字段，让实体页有稳定的别名表和索引摘要。

**模式 B：独立抽取（图谱关闭时回退）**

照搬 WeKnora：候选抽取 → 按 12000 字符切批的引用归类（批内并发 ≤4）→ 摘要 → 页面生成。

reduce / finalize 与 WeKnora 一致，但做三处 HTKnow 化简：

- **保留 slug 句柄化**：成本低、收益高，直接照搬。
- **去重预筛换实现**：FTS5 trigram（可选叠加 embedding 余弦）替代 pg_trgm，Jaccard 下限沿用 0.08。
- **回撤不让模型做减法**：页面若只剩被删文档这一个来源 → 直接 archive/删除；若还有其他来源 → 用剩余来源的切片**确定性重建**该页（一次生成调用）。比让模型「剔除某文档的独占贡献」更稳、更好测。

### 3.4 API

挂在现有 `/api/v1/knowledge/wiki/` 下，沿用 `auth` 中间件与 utoipa 注册（`src/api/mod.rs`）。

P1（已实现，`src/api/wiki.rs`）：

- `GET  /wiki/pages?kb_id=&page_type=&status=&limit=&before_id=`：游标分页列表，不含正文
- `GET  /wiki/page?kb_id=&slug=`：正文 + 来源文件 + 切片证据；`slices[].file_id` 已换算成用户可见的文件，
  可直接拼现有的 `/files/{id}/slices/{slice_id}/highlight`
- `GET  /wiki/index?kb_id=`：按类型分组的结构化索引 + 导言（目录由数据库确定性生成，始终最新）
- `GET  /wiki/stats?kb_id=`、`GET  /wiki/search?kb_id=&q=&limit=`
- `GET  /wiki/status?kb_id=`：生成进度（待处理任务数、构建分布、最近错误），供前端「索引中」角标
- `GET  /wiki/graph?kb_id=&center=&depth=&limit=`：页面互链图，overview / ego 两种模式
- `GET|PUT /wiki/config?kb_id=`：读取合并后的生效配置 / 写入知识库级覆盖
- `POST /wiki/rebuild`：整库或单文件重建，只写 `wiki_tasks`

P2（已实现，与 P1 一致用查询参数而不是路径参数）：

- `PUT /wiki/page`：局部编辑（`title`/`summary`/`content`/`aliases`/`status` 任选，省略字段保持原值）
- `POST /wiki/page`：手工建页（`page_type` 只能是 `entity`/`concept`；`slug` 省略时由标题派生）
- `DELETE /wiki/page`：彻底删除（历史版本随外键级联清掉）
- `GET /wiki/revisions?kb_id=&slug=&limit=`：版本元数据列表（不含正文）
- `GET /wiki/revision?kb_id=&slug=&version=`：某个版本的正文 + 当前正文，前端一次请求即可 diff
- `POST /wiki/revert`：回滚到指定版本（作为新版本写入，历史只追加）
- `GET /wiki/lint`：体检报告（断链/空正文/缺摘要/孤儿页/来源失效/标题重复）
- `POST /wiki/rebuild-links`：立即执行一次 KB 级收敛，与后台 `wiki:finalize` 同一份逻辑，返回其报告
- 未实现：`PUT /wiki/pages/{slug}/move`（改 slug 会牵连全部入链，收益低）、`/wiki/folders`、`POST /wiki/auto-fix`

权限：读沿用 KB viewer 判定（公开库的非成员是 viewer，可读版本但改不了），写要求 editor/admin，与 `kb_permissions` 语义一致。

编辑与生成管道的边界（`src/wiki/edit.rs`、`src/wiki/page.rs`）：

- 人工编辑过的页面（`last_edit_source` 为 `user`/`revert`）**不会被自动生成的内容覆盖**：
  `page::upsert` 遇到管道草稿时直接跳过内容写入（`UpsertOutcome.skipped_manual = true`），但仍并入来源与切片证据。
  回滚到管道历史版本只恢复其内容，新版本仍标记为 `revert`，继续受人工内容保护；目前没有恢复自动生成的入口。
- 交叉链接维护走 `page::update_content(..., edit_source = None)`：改正文与出链、写版本快照、递增 version，
  但**保留原有作者归属**，所以人工页面既不会链接腐烂，也不会因此变成「管道页」而被覆盖。
- 状态切换（归档/恢复）不写快照、不递增 version：归档再恢复不该凭空多出一个版本。
- 归档页退出目录、搜索、`list_surfaces`（linkify 候选）与图，指向它的内链由 finalize 当死链清掉；
  正文与历史都还在，`GET /wiki/pages?status=archived` 可以列出来并恢复。
- 文档删除触发 retract 时，人工页面失去全部来源只归档不删除（`RetractReport.pages_archived`）。

### 3.5 检索集成（P3，已实现）

- **Tantivy**：独立索引目录为 `${HTKNOW_TANTIVY_INDEX_PATH}_wiki`。复用现有中文分词和数值过滤 schema，
  `id` 与内部 `file_id` 过滤槽都存页面 ID（不代表源文件），正文索引包括标题、别名、摘要和完整正文。
  启动时重建派生的全文缓存；显式重建搜索索引时也会重建 Wiki，以应用词典变化。
- **LanceDB**：独立 `wiki_pages` 表存页面 ID、KB ID、内容/模型指纹与向量。向量输入是标题、别名、摘要和正文的前 12000 字符，
  复用 `HTKNOW_EMBEDDING_*` 配置；已有指纹相同则不重复调用模型。表损坏或向量维度变化时可从 SQLite 重建。
  当前采用精确向量搜索，后续随页面规模增加再补近似索引和专用 compact。
- **同步与回收**：`SearchEngine::start_wiki_indexer` 每 10 秒对账，单轮最多更新 16 页向量，轮转处理避免失败页阻塞后续页面。
  查询前同步全文缓存，并在返回前核对发布状态、版本、KB 和向量指纹。
  编辑、回滚、归档/恢复、页面删除、整库删除以及生成管道更新都由同一条对账路径覆盖，无需 LLM 生成开关。
  向量写入失败会在后续轮询/重启时重试；向量查询失败仍返回全文结果。
- **融合加权**：先对 Wiki 的全文/向量结果做 RRF（倒数排名融合），再在 API 层与切片结果按排名融合，Wiki 权重为 1.3。
  普通搜索、图谱增强搜索都返回包含 Wiki 页面引用的混合结果，
  存在 Wiki 结果时切片结果也使用可比较的排名分数。无 Wiki 命中时保留原切片分数。
- **权限与范围**：只检索 `published` 的非索引页；API 沿用知识库权限，并在召回后复核。
  文件范围先映射到 `wiki_page_sources` 的页面集合，再进行全文/向量召回，手工无来源页不会混入限定文件的搜索。
- **引用跳转**：混合结果新增可选 `wiki` 对象（页面元数据、正文、来源与切片证据）；此时 `id` 是页面 ID、`file_id` 为 0、`file` 为空。
  前端以 `wiki.page.id` 区分页面和切片，显示 Wiki 类型，点击按 KB + slug 打开 `WikiBrowser`，继续使用原文高亮与来源文件预览。
- **Wiki 内搜索**：`GET /wiki/search` 已从标题/摘要 LIKE 匹配切换为上述正文全文/向量检索。

实现：`src/search/wiki_index.rs`、`src/search/wiki_vector.rs`、`src/api/search.rs`。
目前对账会扫描全部已发布页面；大规模部署下可进一步改为持久化增量变更队列，避免每次查询扫描全量正文。

### 3.6 前端

- 依赖：新增 `marked` + `dompurify`（WeKnora 同款组合），暂不需要 katex / highlight.js。
- 组件：`WikiBrowser.vue`（左侧目录/搜索 + 右侧正文 + 相关链接 + 来源证据 + 设置面板；
  P2 增加编辑表单、新建条目、体检面板、「已归档」列表与页面操作条）。
  `WikiRevisionDrawer.vue`（版本列表 + 行级 diff + 回滚）已落地；独立的 Wiki 图视图仍待做
  （后端 `/wiki/graph` 已就绪，可直接复用 `GraphVisualization.vue`）。
- 入口：`KnowledgeBaseList.vue` 工具栏「知识库 Wiki」按钮、知识库卡片图标、`KnowledgeDirectory.vue` 的目录菜单项（新增 `wiki` 事件）。
- `[[slug|显示名]]` 在渲染前改写为 `[显示名](#wiki/<slug>)` 并跳过代码围栏，点击时拦截走内部跳转而不是外链；
  渲染结果统一过 dompurify。改写逻辑由 `WikiBrowser.test.mjs` 用 `node --test` 覆盖。
- 生成中每 5 秒轮询 `/wiki/status`，收敛后自动刷新目录与当前页。
- 行级 diff 落在 `frontend/src/wikiDiff.js`（参考 WeKnora 的 `wikiLineDiff.ts`）：削公共前后缀 + LCS 动态规划，
  超过 400 万 DP 单元时退化为「整段替换」，`foldDiff` 再把未变化的长段折叠成 `gap` 行。
  由 `wikiDiff.test.mjs` 与 `WikiRevisionDrawer.test.mjs` 用 `node --test` 覆盖（后者用 vm 跑组件里真实的 diff 逻辑）。

### 3.7 配置

```
HTKNOW_BUILD_WIKI=false                 # 全局开关，对应 HTKNOW_BUILD_KNOWLEDGE_GRAPH
HTKNOW_WIKI_WORKER_INTERVAL_SECS=10     # 空闲时自动翻倍，上限 3 倍
HTKNOW_WIKI_BATCH_SIZE=5                # 单轮认领数，同时是任务并发上限
HTKNOW_WIKI_REDUCE_PARALLEL=4           # 单文档内条目页并发
HTKNOW_WIKI_CITATION_PARALLEL=4         # 模式 B 的引用归类批次并发
HTKNOW_WIKI_MAX_INFLIGHT_PER_KB=2
HTKNOW_WIKI_LLM_MAX_TOKENS=8192
HTKNOW_WIKI_FINALIZE_DELAY_SECS=20      # KB 级收敛防抖
HTKNOW_WIKI_MAX_FAIL_RETRIES=5
HTKNOW_WIKI_CLAIM_STALE_SECS=5400
HTKNOW_WIKI_MAX_PAGES_PER_INGEST=0      # 0 表示不限制
HTKNOW_WIKI_MAX_SOURCE_CHARS=12000      # 送入模型的原文证据预算
HTKNOW_WIKI_REVISION_SOFT_LIMIT=50      # 每页保留的**管道生成**版本上限，0 表示不裁剪
HTKNOW_WIKI_REVISION_HARD_LIMIT=200     # 每页保留的历史版本总上限，0 表示不限制
HTKNOW_WIKI_GRANULARITY=standard
HTKNOW_WIKI_LANGUAGE=中文
WIKI_LLM_API_URL / WIKI_LLM_API_KEY / WIKI_LLM_MODEL   # 可选，未设则复用 LLM_*
```

KB 级 `knowledge_bases.wiki_config`（JSON）覆盖全局：`enabled` / `granularity` / `language` / `model` / `max_pages_per_ingest`，
前端设置面板走 `PUT /wiki/config`。版本快照的保留上限是全局的（`HTKNOW_WIKI_REVISION_*`），不做知识库级覆盖：
软限额只裁 `edit_source = pipeline` 的版本，人工编辑与回滚留下的版本只受硬限额约束，因此用户历史不会被自动生成挤掉。

开关是两级的，但**以知识库为准**：`HTKNOW_BUILD_WIKI` 只是 `enabled` 的默认值，知识库可以单独打开或关闭。
worker 始终运行（空闲轮询代价可忽略），因此关掉全局默认也不影响某个库单独启用；
真正的前置条件是配置了 LLM 地址，否则解析后不入队、`POST /wiki/rebuild` 直接返回 400。

### 成本估算

每篇文档的 LLM 调用数（P 为该文档产出的实体/概念页数，通常 3–15）：

| 模式 | 调用数 | 说明 |
| --- | --- | --- |
| A 图谱复用 | `1 + P` | 1 次摘要 + P 次页面生成/合并 |
| B 独立抽取 | `2 + ceil(字符数/12000) + P` | 候选 + 引用批次 + 摘要 + 页面 |
| KB 级摊销 | 每次 finalize | 索引 intro 1 次、目录规划 `ceil(P/60)` 次、去重按候选组数 |

模式 A 相比 WeKnora 少掉「1 次候选 + N 次引用归类」，是最划算的引入路径；代价是实体质量受图谱抽取 prompt 约束，需要先给图谱抽取补 aliases/description。

### 风险与对策

| 风险 | 对策 |
| --- | --- |
| slug 不稳定导致页面重复、链接腐烂 | slug 由名称规范化生成并持久化；送模型前句柄化；`wiki_builds` 指纹复用 |
| 页面反复重写抖动 | `content_fingerprint` 未变则跳过；`version` 只在用户可见字段变化时 +1 |
| SQLite 写并发瓶颈 | LLM 调用在事务外；写用短事务；单实例部署 + `claimed_at` 复位 |
| 中文 linkify 误插链接 | 照搬 WeKnora 的禁止区间（代码围栏/行内代码/已有链接/引用定义）与词边界判定 |
| 生成成本失控 | 全局开关 + KB 级开关 + 每 KB 在途上限 + 三档粒度 + `max_pages_per_ingest` |
| 删除文档后残留幽灵引用 | retract 任务 + `wiki_page_sources`/`wiki_page_slice_refs` 外键级联 + finalize 死链清理 |
| 前端 XSS | dompurify 白名单；wiki 链接只允许内部 slug，不放行任意协议 |
| 与图谱重复建设 | 明确分工：图谱负责「实体—关系—原文证据」，Wiki 负责「可读的成篇叙述」，Wiki 复用图谱产物而非另起一套抽取 |

## 四、实现状态

### P1 已完成

| 部分 | 位置 |
| --- | --- |
| 迁移 6（`wiki_pages` / `wiki_page_sources` / `wiki_page_slice_refs` / `wiki_tasks` / `wiki_builds` + 文件删除触发器） | `src/wiki/migration.sql`、`src/db.rs` |
| 任务队列：原子认领、防抖、退避、重试上限、崩溃复位 | `src/wiki/queue.rs` |
| worker：自适应轮询、每知识库在途上限、暂停让位索引维护 | `src/wiki/worker.rs` |
| 生成管道：map（模式 A 图谱复用 / 模式 B 抽取+引用归类）→ reduce（按 slug 加锁合并） | `src/wiki/ingest.rs` |
| KB 级收敛：索引页目录 + 导言、交叉链接注入、死链清理 | `src/wiki/finalize.rs`、`src/wiki/linkify.rs` |
| 回撤：删除文件后无来源页面直接删除（人工页面改为归档），多来源页面按剩余证据确定性重建 | `src/wiki/retract.rs` |
| 只读 API + 配置 + 重建入口 | `src/api/wiki.rs` |
| 前端浏览器（目录、搜索、Markdown 正文、内链跳转、来源与原文高亮、设置面板） | `frontend/src/components/WikiBrowser.vue` |

入队点：`FileProcessor` 在解析完成（含复用解析产物）后调用 `maybe_enqueue_wiki`（`src/processor.rs`）。
图谱构建是先 `await` 完成的，因此模式 A 一定能读到刚写入的 `graph_node_sources`。

已用 mock LLM 跑通的完整链路：单文档 ingest → 摘要/实体/概念页 → finalize（导言、交叉链接、死链清理、索引目录）
→ 第二篇文档并入同一实体页（来源与切片证据累加）→ 删除首篇文档后回撤（摘要页删除、实体页按剩余证据重建、索引更新）。

### P2 已完成

| 部分 | 位置 |
| --- | --- |
| 迁移 7（`wiki_page_revisions`，页面删除时级联） | `src/wiki/migration_v7.sql`、`src/wiki/mod.rs` |
| 版本快照：`snapshot_current` / `list` / `get` / 两级 `prune`（软限额只裁管道版本） | `src/wiki/revision.rs` |
| 写入路径统一「先快照再更新」：`page::upsert` 与 `page::update_content` | `src/wiki/page.rs` |
| 人工编辑：局部改写、手工建页、回滚、归档/恢复、删除，含输入校验与 slug 锁 | `src/wiki/edit.rs` |
| 人工内容保护：管道草稿不覆盖 `user`/`revert` 页面，链接维护保留作者归属 | `src/wiki/page.rs`、`src/wiki/finalize.rs` |
| 归档语义：退出列表/搜索/linkify 候选，`stats.archived` 单独计数，仍可读可回滚 | `src/wiki/page.rs` |
| 体检：断链、空正文、缺摘要、孤儿页、来源失效、标题重复（只报告不改写） | `src/wiki/lint.rs` |
| 写接口 + 版本接口 + 体检/链接收敛接口 | `src/api/wiki.rs`、`src/api/mod.rs` |
| 前端：编辑表单、新建条目、体检面板、归档列表、历史抽屉（行级 diff + 回滚） | `frontend/src/components/WikiBrowser.vue`、`frontend/src/components/WikiRevisionDrawer.vue`、`frontend/src/wikiDiff.js` |

编辑链路已用集成测试覆盖（`tests/api_integration.rs::wiki_edit_and_revision_flow`）：
建页 → 局部编辑 → 版本列表/详情 → 体检报出断链 → `rebuild-links` 清掉死链且不改作者归属 → 回滚 →
归档（退出目录与搜索、按状态仍可列出）→ 恢复 → 删除（历史级联清空），并校验 404/403/400 分支。

### P3 已实现

默认对话和「仅搜索」均复用切片与 Wiki 混合检索；对话回答使用本轮来源编号，可打开对应 Wiki 页面及来源。独立文档全文搜索入口及 `/search/full` API 已移除；Wiki 与切片自身的 Tantivy 关键词召回保留。

### 文件处理进度

知识库启用 Wiki 后，原文解析完成还不代表整体处理完成。文件列表和详情会显示「等待生成 Wiki」「正在生成 Wiki」「Wiki 生成重试中」或「Wiki 生成失败」；该文档的页面生成成功并确认任务后才计入已完成。正文不足而跳过生成的文件会明确标注「Wiki 无可用正文」。知识库级目录与交叉链接整理继续异步执行。

文件 API 保留 `status` 表示原文解析状态，新增 `processing_status` 表示整体状态（0 待处理、2 处理中、1 已完成、-1 失败、3 不解析），并返回 `wiki_status` 和 `wiki_error`。状态统计使用整体状态；文件列表在存在待处理任务时每 5 秒刷新一次。

「重试失败文件」会对原文解析失败的文件重新解析，对仅 Wiki 失败的文件只重新入队 Wiki。重新解析或移动文件会清除旧 Wiki 构建状态，避免旧结果把新一轮处理误标为完成。关闭知识库 Wiki 后，整体状态重新以原文解析结果为准；启用后尚未构建 Wiki 的历史文件显示等待，可通过 Wiki 重建入口启动生成。

- 独立全文与向量索引、指纹复用、自动同步与删除回收。
- 普通搜索、图谱增强搜索的 Wiki 引用、加权与权限隔离。
- Wiki 正文搜索、结果类型筛选、指定页面跳转和来源预览。
- `tests/wiki_search.rs` 使用本地 mock embedding 服务覆盖正文/纯语义召回、服务失败回退与重试、文件范围、
  私有/公开知识库权限、编辑后的旧向量排除、归档/草稿/恢复/删除和普通/图谱增强搜索入口。

### 实现问题修复

- **跨库移动**：迁移 8 的触发器在文件移动事务内撤下受影响页面和旧目录。旧正文（包括人工编辑）及历史保留为内部 `withdrawn` 页面，原 slug 释放，所有页面/版本/目录/图/体检/搜索接口均不可读取隔离页。旧库剩余来源文件重新入队生成，旧构建指纹失效；升级时也会隔离修复旧版本已经遗留的跨库来源页面。正在生成的内容写入时再次校验来源所属库。隔离内容目前仅可由管理员从数据库恢复，不能通过普通“恢复归档”重新发布。
- **人工编辑保护**：finalize 与编辑共用 slug 锁，取得锁后重读页面；正文更新校验版本和状态，快照与更新处于同一事务。旧快照不覆盖新正文，也不产生错误的历史版本。摘要、刷新路径同样遵守人工保护。
- **部分失败重试**：摘要成功但任一候选条目失败时，构建标记失败并由任务队列重试；只有完整成功的构建才能复用指纹。生成条目上限也进入指纹。
- **混合排序**：Wiki 和切片候选在同一次 rerank 中获得可比分数，再对 Wiki 应用 1.3 权重；服务不可用时按 `1 / (rank + 1)` 交错合并，避免固定的 60 偏移让 Wiki 占满前排。SSE 切片保留自身分数。
- **向量补召回**：检索过滤失效指纹、归档和删除结果后，逐步扩大候选范围，直到得到足量有效页面或耗尽候选；查询只生成一次 embedding。
- **增量同步**：`wiki_index_changes` 为每页保留最新变更序号及删除记录。全文同步按知识库独立推进游标，只加载变化页面；查询不再全库扫描正文。向量后台每轮最多处理 16 条变化，成功后才确认，失败轮转重试；启动时用持久指纹复用已有向量。删除记录不随页面/知识库级联消失，以便重启后清理旧索引。

### 尚未做（P2 余项 / 后续优化）

- `wiki_folders` 目录树与页面移动（改 slug 会牵连全部入链）
- 页面去重（FTS5 trigram 预筛 + Jaccard）与同义词合并；`lint` 目前只报「标题重复」提示，不做自动合并
- lint 的 auto-fix（补写摘要之类要过模型，成本高于收益，暂由用户在编辑表单里手工处理）
- Agent 工具链（把 Wiki 页面作为 Agent 可读写的知识载体）
- 独立的 Wiki 图视图（后端 `/wiki/graph` 已就绪，前端复用 `GraphVisualization.vue` 即可）
- 大规模索引优化：Wiki 向量近似索引、专用 compact 与删除变更记录的安全回收
- 显式恢复人工页面的自动生成（目前回滚仍保留人工保护）

### 验证命令

```sh
cargo test --test wiki_regressions                    # 跨库隔离、历史、旧快照拒绝、候选失败后重试
cargo test --test wiki_search                          # P3 检索与同步、权限、来源、mock 向量服务
cargo test --lib wiki                                  # 迁移、队列、slug、配置合并、模式 A 候选、删除触发器、
                                                       # 版本快照/裁剪、人工编辑保护、回滚、归档、lint
cargo test --test api_integration wiki                 # 只读接口流程 + 编辑/版本/体检/归档/删除流程
node --test frontend/src/components/SearchResults.test.mjs # Wiki 类型筛选、来源预览竞态
node --test frontend/src/components/WikiBrowser.test.mjs   # [[slug|名称]] 改写与代码围栏保护
node --test frontend/src/wikiDiff.test.mjs                 # 行级 diff：增删改、行号、折叠、超大规模退化
node --test frontend/src/components/WikiRevisionDrawer.test.mjs  # 版本抽屉里的 diff 与标签逻辑
npm run build --prefix frontend
cargo check --all-targets
```

手工验证编辑与版本链路（不需要 LLM，任意知识库都能试）：

```sh
curl -H 'x-user-id: user1' -H 'x-role: admin' -X POST -H 'Content-Type: application/json' \
  -d '{"kb_id":1,"title":"检索增强生成","content":"初版正文"}' \
  http://127.0.0.1:3000/api/v1/knowledge/wiki/page
curl -H 'x-user-id: user1' -H 'x-role: admin' -X PUT -H 'Content-Type: application/json' \
  -d '{"kb_id":1,"slug":"concept/检索增强生成","content":"第二版正文"}' \
  http://127.0.0.1:3000/api/v1/knowledge/wiki/page
curl -H 'x-user-id: user1' -H 'x-role: admin' \
  'http://127.0.0.1:3000/api/v1/knowledge/wiki/revisions?kb_id=1&slug=concept/检索增强生成'
curl -H 'x-user-id: user1' -H 'x-role: admin' -X POST -H 'Content-Type: application/json' \
  -d '{"kb_id":1,"slug":"concept/检索增强生成","version":1}' \
  http://127.0.0.1:3000/api/v1/knowledge/wiki/revert
curl -H 'x-user-id: user1' -H 'x-role: admin' 'http://127.0.0.1:3000/api/v1/knowledge/wiki/lint?kb_id=1'
```

手工验证（无需真实 LLM，用任意 OpenAI 兼容端点即可）：

```sh
HTKNOW_BUILD_WIKI=true WIKI_LLM_API_URL=http://127.0.0.1:3998/chat cargo run
curl -H 'x-user-id: user1' -H 'x-role: admin' -X PUT -H 'Content-Type: application/json' \
  -d '{"kb_id":1,"enabled":true}' http://127.0.0.1:3000/api/v1/knowledge/wiki/config
curl -H 'x-user-id: user1' -H 'x-role: admin' -X POST -H 'Content-Type: application/json' \
  -d '{"kb_id":1}' http://127.0.0.1:3000/api/v1/knowledge/wiki/rebuild
curl -H 'x-user-id: user1' -H 'x-role: admin' \
  'http://127.0.0.1:3000/api/v1/knowledge/wiki/status?kb_id=1'
```
