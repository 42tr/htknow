# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

HTKnow is a Chinese knowledge base management system. The backend is Rust (axum + SQLite + Tantivy + LanceDB) and the frontend is a Vue 3 SPA built with Vite and Tailwind CSS 4. The built frontend is embedded into the Rust binary via `rust-embed`.

## Common Commands

### Backend

```bash
# Run locally (requires external services; see below)
cargo run

# Release build
cargo build --release

# Run all tests
cargo test

# Run a specific integration test
cargo test knowledge_base_flow

# Run only integration tests
cargo test --test api_integration

# Format code (uses .rustfmt.toml config)
cargo fmt

# Build with etcd config center support
cargo build --features etcd

# Build with jemalloc heap profiling (debug only)
cargo build --features profiling
```

Rust edition is 2024 and the project uses the nightly toolchain. Formatting is configured in `.rustfmt.toml` (120 char width, 4-space indentation, `StdExternalCrate` import grouping).

### Frontend

```bash
cd frontend
npm ci
npm run dev        # Dev server proxies /api to localhost:3000
npm run build      # Outputs to frontend/dist/ (embedded in binary)
```

### Docker

```bash
# Run with docker-compose (port 8080 -> 3000)
docker compose up -d

# Build Docker image (builds frontend + Rust binary)
./build-docker.sh

# Debug build with profiling tools
./build-docker.sh debug
```

## Architecture

### Entry Point and Routing (`src/main.rs`, `src/lib.rs`, `src/api/mod.rs`)

The axum server is started in `main.rs`. Routes are defined in `src/api/mod.rs` and organized into nested routers:

- `/api/v1/knowledge/knowledge_base/` — KB CRUD, tree, reparse
- `/api/v1/knowledge/files/` — upload, list, download, slices, highlighted PDF
- `/api/v1/knowledge/search/` — full-text, vector, image, graph-enhanced, advanced stream, lexicon/synonym management
- `/api/v1/knowledge/graph/` — entity search, entity detail, graph stats
- `/api/v1/knowledge/system/` — heap profiling, LanceDB compact, index force merge/rebuild

All `/api/v1/knowledge/*` routes require the `auth` middleware (`src/lib.rs`), which extracts `x-user-id` and `x-role` from headers into an `AuthUser` extension. `x-user-name` is optional and base64-decoded if valid UTF-8.

Swagger UI is at `/docs` and the OpenAPI JSON is at `/api-docs/openapi.json`.

### Database (`src/db.rs`, `src/init.sql`)

SQLite with WAL mode enabled. The schema is auto-initialized on startup from `src/init.sql`. Key tables include `knowledge_base`, `file`, `file_slice`, `entity`, `relation`, `mention`, `lexicon`, and `synonym`. The pool is created via `sqlx` and passed as axum state to handlers.

### Search (`src/search/`)

`SearchEngine` (`src/search/mod.rs`) orchestrates three search backends:

- **Tantivy** (`src/search/tantivy_engine.rs`) — full-text search with a custom Chinese tokenizer based on `jieba-rs`. Maintains two indexes: a default index and a "full" index. Supports lexicon-based synonym expansion.
- **LanceDB** (`src/search/lancedb.rs`) — vector search. Stores text and image embeddings in separate tables.
- **Embedding client** (`src/search/embedding.rs`) — calls external embedding services for text and image vectors.

The search module also handles reranking via an external service and provides an advanced streaming search endpoint (`src/search/advanced.rs`).

### Background File Processing (`src/processor.rs`)

`FileProcessor` runs as a background task polling the database for pending files. It:

1. Converts Office docs to PDF via an external service.
2. Parses PDFs via MinerU (or a custom parse service if configured).
3. Transcribes audio files via an external service.
4. Splits content into slices (smart or fixed overlap).
5. Stores slices in SQLite, indexes them in Tantivy, and generates embeddings for LanceDB.
6. Optionally builds a knowledge graph via LLM extraction (if `HTKNOW_BUILD_KNOWLEDGE_GRAPH=true`).

File statuses: `0=pending`, `1=completed`, `2=processing`, `3=skipped`, `-1=failed`.

### Knowledge Graph (`src/graph/`)

- `graph_manager.rs` — manages entities, relations, and mentions in SQLite.
- `llm_extractor.rs` — uses an LLM API to extract entities and relations from file content.

Graph data is stored relationally in SQLite and queried through the graph API endpoints.

### Configuration (`src/config.rs`)

All configuration is environment-variable based with sensible defaults. No config files. Key prefixes:

- `HTKNOW_SERVER_*` — server settings
- `HTKNOW_MINERU_URL`, `HTKNOW_OFFICE_CONVERT_URL`, `HTKNOW_EMBEDDING_URL`, etc. — external services
- `DATABASE_URL` — SQLite connection string
- `HTKNOW_DATA_DIR`, `HTKNOW_LANCEDB_PATH`, `HTKNOW_TANTIVY_INDEX_PATH` — storage paths
- `LLM_API_URL`, `LLM_API_KEY`, `LLM_MODEL` — optional LLM for knowledge graph

See `README.md` for the full environment variable table.

### Frontend (`frontend/`)

Vue 3 SPA using Vite and Tailwind CSS 4. The `vite.config.js` proxies `/api` to `http://localhost:3000` in dev mode. Built assets in `frontend/dist/` are served by the Rust binary via `rust-embed` (`src/frontend.rs`).

## External Service Dependencies

For full functionality, these external services must be running:

- **MinerU** — PDF parsing (`HTKNOW_MINERU_URL`)
- **Office converter** — Office to PDF (`HTKNOW_OFFICE_CONVERT_URL`)
- **Embedding service** — text embeddings (`HTKNOW_EMBEDDING_URL`)
- **Image embedding service** — image embeddings (`HTKNOW_IMAGE_EMBEDDING_URL`)
- **Rerank service** — result reranking (`HTKNOW_RERANK_URL`)
- **Audio transcription** — audio to text (`HTKNOW_AUDIO_TRANSCRIPTION_URL`)

Default values in `docker-compose.yml` and `src/config.rs` point to internal network IPs.

## Testing

Integration tests live in `tests/api_integration.rs`. They set up an isolated test environment with a temp directory and a fresh SQLite database, then exercise the full axum router.

The `APP` router is initialized once via `tokio::sync::OnceCell` and reused across tests. Environment variables are set via `unsafe { std::env::set_var(...) }` during test setup.

## Memory Profiling (Debug Builds)

Debug builds use jemalloc as the global allocator. With the `profiling` feature, heap profiling is enabled and accessible via:

- `GET /api/v1/knowledge/system/heap` — returns a heap profile
- `GET /api/v1/knowledge/system/heap/pdf` — returns a PDF visualization

## Notes

- LanceDB auto-compact runs on a cron schedule (`HTKNOW_LANCEDB_COMPACT_CRON`, default `0 0 3 * * *`). Compaction holds a lock to prevent concurrent writes.
- If Tantivy indexes become corrupted (e.g., from an unclean shutdown), the app will panic on startup. Recovery: back up and remove the `tantivy_index` and `tantivy_full_index` directories, then trigger a rebuild via the system endpoints.
- The `cargo build --features etcd` flag enables etcd as a config center, but the etcd integration is optional and not enabled by default.
