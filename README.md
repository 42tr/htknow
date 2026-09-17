# HTKnow

HTKnow 知识库管理系统，提供文档上传、检索与知识图谱能力，内置前端界面与 OpenAPI 文档。

## 功能概览
- 知识库管理、文件上传与解析
- 段落混合搜索（关键词、向量与 Wiki）/图谱增强搜索
- 知识图谱查询与可视化
- 知识库 Wiki：文档解析后自动生成互相链接的条目页（摘要/实体/概念/索引），支持人工编辑、版本回滚、归档与体检
- 内置前端界面与 Swagger API 文档

## 快速开始

### 本地运行
1. 准备外部服务：MinerU、Embedding、图片 Embedding、Rerank（见“配置”）。
2. 启动服务：
```shell
cargo run
# 或
cargo build --release
./target/release/htknow
```

### Docker Compose
```shell
docker compose up -d
```
`docker-compose.yml` 默认将 `8080 -> 3000`。

### 构建 Docker 镜像
```shell
./build-docker.sh
```
脚本会先编译二进制，再构建镜像并给出运行示例。

### GitHub Actions 发布缓存

推送 `v*` tag 会构建两个架构的二进制、发布 Docker 镜像和 GitHub Release。
`master` 上的 Cargo 依赖、Rust 工具链或发布工作流配置变化时，会用相同环境预热两个架构的 Rust 依赖缓存，但不执行发布。
tag 构建只读取缓存；缓存写在默认分支，供不同版本 tag 复用，避免每次发布重新编译全部依赖。

首次启用或依赖变化后，先等待 `master` 的 Release 工作流完成，再推送发布 tag。
缓存被淘汰时，可在 Actions 的 Release 工作流中选择 `master` 手动运行以重新预热。
无缓存时仍会正常完整编译；应用本身和嵌入的前端会在每次构建时重新编译。

### 访问入口
- 前端界面: `http://localhost:3000/`
- API 文档: `http://localhost:3000/docs`
- OpenAPI JSON: `http://localhost:3000/api-docs/openapi.json`

> 使用 docker-compose 时，请将端口替换为 `8080`。

## API 认证
`/api/v1/knowledge/*` 需要请求头：
- `x-user-id`（必填）
- `x-role`（必填）
- `x-user-name`（可选）

示例：
```shell
curl -H 'x-user-id: 1' -H 'x-role: admin' -H 'x-user-name: testuser' \
  http://localhost:3000/api/v1/knowledge/knowledge_base/
```

## 启动 mineru
```shell
docker run -d --name mineru-api --restart unless-stopped --ipc host -p 10001:10001 -e MINERU_MODEL_SOURCE=local --ulimit memlock=-1 --ulimit stack=67108864 --gpus all alexsuntop/mineru:latest mineru-api --host 0.0.0.0 --port 10001
```

## 配置
支持通过环境变量覆盖配置，未设置时使用默认值。

### 服务器
| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `HTKNOW_SERVER_HOST` | `0.0.0.0` | 监听地址 |
| `HTKNOW_SERVER_PORT` | `3000` | 监听端口 |
| `HTKNOW_SERVER_UPLOAD_LIMIT_MB` | `500` | 上传大小限制（MB） |
| `HTKNOW_SERVER_PROCESS_INTERVAL_SECS` | `10` | 文件处理间隔（秒） |
| `HTKNOW_SERVER_PROCESS_CONCURRENCY` | `1` | 后台文件处理并发数 |
| `HTKNOW_PARSE_ENABLED` | `true` | 是否启动后台文件解析（false 时仅即时解析生效） |
| `HTKNOW_REUSE_DUPLICATE_FILES` | `true` | 是否复用重复文件的已解析结果 |
| `HTKNOW_BUILD_KNOWLEDGE_GRAPH` | `false` | 文件解析完成后是否构建知识图谱（依赖 LLM 配置） |
| `HTKNOW_LANCEDB_COMPACT_CRON` | `0 0 3 * * *` | LanceDB 自动压缩 cron 表达式（本地时区，off/disabled/0 禁用） |

### 外部服务
默认值为示例地址，请按实际部署环境调整。

配置在启动时从环境变量读取，未设置时使用默认值；修改环境变量后需重启服务。旧版页面保存的数据库配置不再读取。

图片处理模式通过 `HTKNOW_IMAGE_PARSE_MODE` 指定：`ocr` 调用 OCR 接口，`custom` 调用自定义 JSON 接口，`none` 不生成图片文本描述。未指定模式时，优先使用已配置的自定义图片接口，其次使用 OCR，均未配置则禁用。OCR 接口请求为 `{ "figure_base64": "..." }`，响应为 `{ "data": "ocr结果" }`。

| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `HTKNOW_MINERU_URL` | `http://192.168.0.46:10001/file_parse` | MinerU PDF 解析 |
| `HTKNOW_REQUEST_TIMEOUT_SECS` | `600` | 外部接口请求超时（秒），适用于文件解析相关接口 |
| `HTKNOW_MINERU_MAX_PAGES` | `50` | MinerU 单次解析 PDF 最大页数（0 表示不限制） |
| `HTKNOW_OFFICE_CONVERT_URL` | `http://192.168.0.46:8003/convert` | Office 文档转 PDF 服务，使用 multipart `file` 字段并自动追加 `target_format=pdf` |
| `HTKNOW_CUSTOM_PARSE_URL` | 空 | 自定义解析服务地址（配置后仅 Word/PPT/PDF 解析走该服务，需返回已切片数据） |
| `HTKNOW_CUSTOM_PARSE_REUSE_URL` | 空 | 自定义解析复用服务地址（仅输入 pdf_contents，不包含图片） |
| `HTKNOW_AUDIO_TRANSCRIPTION_URL` | `http://192.168.0.46:59805/api/v1/audio/transcriptions` | 音频转写服务 |
| `HTKNOW_AUDIO_TRANSCRIPTION_KEY` | 空 | 音频转写服务 API Key |
| `HTKNOW_EMBEDDING_URL` | `http://222.190.139.186:59700/v1/embeddings` | 文本向量服务 |
| `HTKNOW_EMBEDDING_KEY` | 空 | 文本向量服务 API Key，配置后以 `Authorization: Bearer <key>` 发送 |
| `HTKNOW_IMAGE_EMBEDDING_URL` | 空 | 图片向量服务，未配置时禁用 |
| `HTKNOW_IMAGE_EMBEDDING_KEY` | 空 | 图片向量服务 API Key，配置后以 `Authorization: Bearer <key>` 发送 |
| `HTKNOW_RERANK_URL` | `http://222.190.139.186:59600/v1/rerank` | Rerank 服务 |
| `HTKNOW_RERANK_KEY` | 空 | Rerank 服务 API Key，配置后以 `Authorization: Bearer <key>` 发送 |
| `HTKNOW_IMAGE_PARSE_MODE` | 按接口配置自动选择 | `none` / `ocr` / `custom` |
| `HTKNOW_IMAGE_PARSE_URL` | 空 | 自定义图片文本化接口 |
| `HTKNOW_IMAGE_OCR_URL` | 空 | OCR 图片文本接口 |
| `HTKNOW_IMAGE_PARSE_TIMEOUT_SECS` | `120` | 图片文本化及 OCR 请求超时（秒） |
| `HTKNOW_IMAGE_PARSE_CONCURRENCY` | `5` | 图片文本化并发数 |

### AI 模型
| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `HTKNOW_EMBEDDING_MODEL` | `bge-m3` | Embedding 模型 |
| `HTKNOW_EMBEDDING_DIM` | `1024` | Embedding 维度 |
| `HTKNOW_IMAGE_EMBEDDING_DIM` | `2048` | 图片 Embedding 维度 |
| `HTKNOW_EMBEDDING_BATCH_SIZE` | `8` | 文本 embedding 每批最多条数（最小 1） |
| `HTKNOW_EMBEDDING_BATCH_MAX_CHARS` | `16000` | 每批文本总字符预算；超长单条独立请求，不截断 |
| `HTKNOW_EMBEDDING_BATCH_TIMEOUT_SECS` | `120` | 文件索引批量 embedding 请求超时（秒），不影响搜索超时 |
| `HTKNOW_RERANK_MODEL` | `bge-rerank` | Rerank 模型 |
| `HTKNOW_RERANK_THRESHOLD` | `0.1` | Rerank 阈值 |

### 数据库
| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `DATABASE_URL` | `sqlite://data/app.sqlite` | 数据库连接 |
| `HTKNOW_DB_MAX_CONNECTIONS` | `16` | 最大连接数 |
| `HTKNOW_DB_MIN_CONNECTIONS` | `2` | 最小空闲连接数 |
| `HTKNOW_DB_BUSY_TIMEOUT_MS` | `5000` | busy_timeout（毫秒） |
| `HTKNOW_DB_INIT_DEFAULT_KBS` | `true` | 是否初始化默认知识库 |

### 存储路径
| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `HTKNOW_DATA_DIR` | `data` | 数据目录 |
| `HTKNOW_LANCEDB_PATH` | `data/lancedb_data` | LanceDB 路径 |
| `HTKNOW_TEMP_PATH` | `data/temp` | 临时目录 |
| `HTKNOW_IMAGES_PATH` | `data/images` | 图片目录 |
| `HTKNOW_PDF_PATH` | `data/pdfs` | PDF 目录 |
| `HTKNOW_FILES_PATH` | `data/files` | 文件目录 |
| `HTKNOW_ARCHIVES_PATH` | `data/archives` | 压缩文件解压目录 |
| `HTKNOW_CONTENTS_PATH` | `data/contents` | 文件解析后完整文本目录 |

### 搜索
| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `HTKNOW_SEARCH_LIMIT` | `10` | 搜索结果限制 |
| `HTKNOW_TANTIVY_INDEX_PATH` | `data/tantivy_index` | Tantivy 索引路径 |
| `HTKNOW_TANTIVY_MEMORY_MB` | `50` | Tantivy 内存（MB） |
| `HTKNOW_SEARCH_TANTIVY_REBUILD_BATCH_SIZE` | `100` | Tantivy 索引重建批次大小 |
| `HTKNOW_SEARCH_LANCEDB_REBUILD_BATCH_SIZE` | `100` | LanceDB 从 SQLite 重建批次大小 |
| `HTKNOW_SEARCH_EMBEDDING_TIMEOUT_SECS` | `30` | 搜索单条文本 / 图片 embedding 请求超时（秒） |
| `HTKNOW_SEARCH_RERANK_TIMEOUT_SECS` | `20` | rerank 请求超时（秒） |
| `HTKNOW_SEARCH_SYNONYM_ENABLED` | `true` | 是否启用同义词查询扩展 |
| `HTKNOW_SEARCH_SYNONYM_BOOST` | `0.7` | 同义词权重因子（与行权重相乘） |
| `HTKNOW_SEARCH_MAX_SYNONYMS_PER_TERM` | `5` | 每个词最多扩展同义词数 |
| `HTKNOW_SEARCH_MAX_TOTAL_SYNONYMS` | `30` | 单次查询最多扩展同义词总数 |
| `HTKNOW_HIGHLIGHT_PAGE_MIN_POSITIONS` | `20` | 高亮页码选择阈值：首选页位置数少于该值时优先使用第二页 |

### 切片
| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `HTKNOW_SMART_SLICE_MAX_CHARS` | `8000` | 智能切片最大字数 |
| `HTKNOW_FIXED_SLICE_OVERLAP_CHARS` | `100` | 固定切片重叠字数 |

### LLM（可选）
| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `LLM_API_URL` | 空 | LLM API 地址 |
| `LLM_API_KEY` | 空 | LLM API Key |
| `LLM_MODEL` | `gpt-3.5-turbo` | LLM 模型 |

### 知识库 Wiki（可选）
开关是**知识库级**的：`HTKNOW_BUILD_WIKI` 只是知识库未显式配置时的默认值，可以在前端 Wiki 设置面板
（`PUT /api/v1/knowledge/wiki/config`）里逐库打开或关闭。Wiki worker 始终运行，空闲时只是一次 SQLite 轮询。
生成依赖 LLM 地址，未单独配置 `WIKI_LLM_*` 时复用上面的 `LLM_*`。设计细节见 `docs/wiki.md`。

人工编辑过的页面不会被后续自动生成覆盖（`last_edit_source` 为 `user`/`revert` 时管道只并入证据、不改正文），
需要新稿就显式回滚到某个自动生成的版本；交叉链接维护是机械改写，对人工页面照常执行。
不想让某个条目出现在目录里又不想丢内容，用「归档」而不是删除。

| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `HTKNOW_BUILD_WIKI` | `false` | 知识库 Wiki 的默认开关，知识库可单独覆盖 |
| `HTKNOW_WIKI_WORKER_INTERVAL_SECS` | `10` | Wiki worker 基础轮询间隔（秒），空闲时自动翻倍 |
| `HTKNOW_WIKI_BATCH_SIZE` | `5` | 单轮认领的任务数，同时是任务并发上限 |
| `HTKNOW_WIKI_REDUCE_PARALLEL` | `4` | 单文档内条目页的并发写入数 |
| `HTKNOW_WIKI_CITATION_PARALLEL` | `4` | 引用归类的批次并发数（仅图谱关闭时的模式 B） |
| `HTKNOW_WIKI_MAX_INFLIGHT_PER_KB` | `2` | 每个知识库同时在途的任务数上限 |
| `HTKNOW_WIKI_LLM_MAX_TOKENS` | `8192` | 单次生成调用的 max_tokens |
| `HTKNOW_WIKI_FINALIZE_DELAY_SECS` | `20` | 索引/交叉链接收敛任务的防抖窗口（秒） |
| `HTKNOW_WIKI_MAX_FAIL_RETRIES` | `5` | 任务失败重试上限，超过后丢弃并保留 `last_error` |
| `HTKNOW_WIKI_CLAIM_STALE_SECS` | `5400` | 认领超时（秒），超时任务复位为待处理 |
| `HTKNOW_WIKI_MAX_PAGES_PER_INGEST` | `0` | 单文档最多生成的条目页数，0 表示不限制 |
| `HTKNOW_WIKI_MAX_SOURCE_CHARS` | `12000` | 送入模型的原文证据字符预算 |
| `HTKNOW_WIKI_REVISION_SOFT_LIMIT` | `50` | 每页保留的**自动生成**版本数上限，0 表示不裁剪 |
| `HTKNOW_WIKI_REVISION_HARD_LIMIT` | `200` | 每页保留的历史版本总数上限，0 表示不限制 |
| `HTKNOW_WIKI_GRANULARITY` | `standard` | 默认抽取粒度：`focused` / `standard` / `exhaustive` |
| `HTKNOW_WIKI_LANGUAGE` | `中文` | 默认生成语言 |
| `WIKI_LLM_API_URL` | 空 | Wiki 专用 LLM API 地址 |
| `WIKI_LLM_API_KEY` | 空 | Wiki 专用 LLM API Key |
| `WIKI_LLM_MODEL` | 空 | Wiki 专用模型名 |

## 启用 etcd 配置
`cargo build --features etcd`

## 日志级别（RUST_LOG）
默认日志级别为 `info`，可通过环境变量覆盖：
```shell
RUST_LOG=debug ./htknow
RUST_LOG=warn,htknow::search=debug ./htknow
```

## 问题处理
1. Tantivy 切片索引异常
可能是异常停止导致的，报错
thread 'main' (1) panicked at src/search/mod.rs:903:29:failed to create tantivy index reader: Failed to open file for read: 'FileDoestiotExist("/app/data/tantivy index/eafseaef4f2340...
run with 'RuST_BAcKTRAcE=l environment variable to display a backtrace

**方案一**：在 `meta.json` 中去掉报错的索引，注意 `meta.json` 中记录的名称有 `-`

**方案二**：

先去掉原本的索引使应用正常启动
```shell
mv tantivy_index tantivy_index_bak0423
docker start htknow
```
然后在词典管理中重建搜索索引

### 文件解析时 embedding 请求失败

批量文本向量请求同时受条数和字符预算约束，建立连接最多等待 5 秒，推理超时独立配置。超时或 HTTP 413 会拆小多条批次；单条超时、连接故障、HTTP 408/429/5xx 最多退避重试两次。认证错误及其他不可重试的错误直接报告。响应条数和 `index` 必须与输入对应，避免向量与切片错配。

长文档可使用 `HTKNOW_EMBEDDING_BATCH_TIMEOUT_SECS=120`、`HTKNOW_EMBEDDING_BATCH_MAX_CHARS=16000`（默认值）；旧版本尚未支持这两个变量时，可临时设置 `HTKNOW_EMBEDDING_BATCH_SIZE=2`、`HTKNOW_SEARCH_EMBEDDING_TIMEOUT_SECS=120`，后者也会延长搜索请求超时。环境变量修改后需重启服务，再重试失败文件。单条文本超过模型上下文限制时仍需调整切片大小；字符预算不会截断原文，也不等同于模型 token 上限。

需要鉴权的服务通过 `HTKNOW_EMBEDDING_KEY`、`HTKNOW_IMAGE_EMBEDDING_KEY`、`HTKNOW_RERANK_KEY` 配置 Key，请求以 `Authorization: Bearer <key>` 发送；留空则不附加鉴权头，认证失败按不可重试错误直接报告。

### 对话与搜索

界面默认使用流式对话：输入问题并回车，系统先混合检索原文切片与 Wiki，再生成带引用的回答。「仅搜索」按钮继续返回原搜索结果；粘贴图片仍使用图片搜索。独立的文档全文搜索入口及 `/search/full` API 已移除，不再创建、写入、重建或导出文档全文索引，原 `HTKNOW_TANTIVY_FULL_INDEX_PATH` 配置不再使用。旧版本留下的索引目录不会自动删除，可在确认升级后自行清理。原文存储、文件预览、切片关键词检索及 `/search/summary` 摘要接口继续使用。

对话使用 `LLM_API_URL`、`LLM_API_KEY`、`LLM_MODEL`，其中 URL 必须是支持 `stream: true` 的完整 OpenAI 兼容 Chat Completions 地址，例如 `https://your-provider.example/v1/chat/completions`。Wiki 专用的 `WIKI_LLM_*` 配置不用于聊天。

- 点击回答中的 `[1]` 等编号可查看本轮证据：原文引用打开对应切片，Wiki 引用打开 Wiki 页面及其来源，不把页面级来源伪装成逐句原文证据。
- 支持追问、停止生成、重新生成和新对话。历史仅保存在前端内存，刷新即清空；切换知识库后开始新对话，不建立后端会话表。
- 每轮最多使用最近 6 轮完整历史（共 16000 字符），检索证据最多 8 条、24000 字符。没有可访问的相关资料时明确说明，不调用 LLM 编造回答。
- 新接口：`POST /api/v1/knowledge/chat`，沿用现有认证头。请求示例：

```json
{"question":"螺旋桨有哪些类型？","kb_id":1,"messages":[]}
```

`messages` 为按 `user` / `assistant` 成对排列的历史消息；`kb_id` 可省略或为 `null`，表示当前用户可访问的知识库。返回 `text/event-stream`，事件依次为 `status`（检索/生成阶段）、`sources`（编号和搜索结果）、`delta`（新增文本），最后 `done`；失败时返回 `error`。断开连接会取消本轮处理，流中断不会标记为正常完成。响应禁用代理缓冲，并每 10 秒发送保活注释。
