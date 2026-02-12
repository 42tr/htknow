# HTKnow

HTKnow 知识库管理系统，提供文档上传、检索与知识图谱能力，内置前端界面与 OpenAPI 文档。

## 功能概览
- 知识库管理、文件上传与解析
- 全文/向量/图谱增强搜索
- 知识图谱查询与可视化
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
| `HTKNOW_PARSE_ENABLED` | `true` | 是否启动后台文件解析（false 时仅即时解析生效） |
| `HTKNOW_LANCEDB_COMPACT_CRON` | `0 0 3 * * *` | LanceDB 自动压缩 cron 表达式（本地时区，off/disabled/0 禁用） |

### 外部服务
默认值为示例地址，请按实际部署环境调整。
| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `HTKNOW_MINERU_URL` | `http://192.168.0.46:10001/file_parse` | MinerU PDF 解析 |
| `HTKNOW_MINERU_MAX_PAGES` | `0` | MinerU 单次解析 PDF 最大页数（0 表示不限制） |
| `HTKNOW_CUSTOM_PARSE_URL` | 空 | 自定义解析服务地址（配置后仅 Word/PDF 解析走该服务，需返回已切片数据） |
| `HTKNOW_CUSTOM_PARSE_REUSE_URL` | 空 | 自定义解析复用服务地址（仅输入 pdf_contents，不包含图片） |
| `HTKNOW_EMBEDDING_URL` | `http://222.190.139.186:59700/v1/embeddings` | 文本向量服务 |
| `HTKNOW_IMAGE_EMBEDDING_URL` | `http://192.168.0.46:59802/v1/embeddings/file` | 图片向量服务 |
| `HTKNOW_RERANK_URL` | `http://222.190.139.186:59600/v1/rerank` | Rerank 服务 |

### AI 模型
| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `HTKNOW_EMBEDDING_MODEL` | `bge-m3` | Embedding 模型 |
| `HTKNOW_EMBEDDING_DIM` | `1024` | Embedding 维度 |
| `HTKNOW_IMAGE_EMBEDDING_DIM` | `2048` | 图片 Embedding 维度 |
| `HTKNOW_RERANK_MODEL` | `bge-rerank` | Rerank 模型 |
| `HTKNOW_RERANK_THRESHOLD` | `0.1` | Rerank 阈值 |

### 数据库
| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `DATABASE_URL` | `sqlite://data/app.sqlite` | 数据库连接 |
| `HTKNOW_DB_MAX_CONNECTIONS` | `10` | 最大连接数 |
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

### 搜索
| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `HTKNOW_SEARCH_LIMIT` | `10` | 搜索结果限制 |
| `HTKNOW_TANTIVY_INDEX_PATH` | `data/tantivy_index` | Tantivy 索引路径 |
| `HTKNOW_TANTIVY_FULL_INDEX_PATH` | `data/tantivy_full_index` | Tantivy 全文索引 |
| `HTKNOW_TANTIVY_MEMORY_MB` | `50` | Tantivy 内存（MB） |

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

## 启用 etcd 配置
`cargo build --features etcd`

## 日志级别（RUST_LOG）
默认日志级别为 `info`，可通过环境变量覆盖：
```shell
RUST_LOG=debug ./htknow
RUST_LOG=warn,htknow::search=debug ./htknow
```
