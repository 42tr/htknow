use axum::{
    Extension, Router, routing::{get, post}
};
use sqlx::SqlitePool;
use utoipa::OpenApi;

mod error;
mod file;
mod graph;
mod knowledge_base;
mod search;
mod system;
// 重新导出 File 类型供其他模块使用
pub use file::File;
pub(crate) use file::{collect_image_paths_for_files, remove_image_files};

use crate::search::SearchEngine;

/// OpenAPI 文档定义
#[derive(OpenApi)]
#[openapi(
    paths(
        // Knowledge Base
        knowledge_base::list,
        knowledge_base::create,
        knowledge_base::get,
        knowledge_base::update,
        knowledge_base::reparse,
        knowledge_base::delete,
        knowledge_base::tree,
        // File
        file::upload,
        file::list,
        file::get,
        file::update,
        file::delete,
        file::get_slices,
        file::get_image_by_filename,
        file::download,
        file::get_highlighted_pdf,
        // Search
        search::search,
        search::search_full,
        search::search_with_graph,
        search::search_image,
        // Graph
        graph::search_entities,
        graph::get_entity,
        graph::get_graph_stats,
        // System
        system::memory_usage,
        system::heap_profile,
        system::heap_profile_status,
        system::heap_profile_pdf,
    ),
    components(
        schemas(
            knowledge_base::Knowledge,
            knowledge_base::KnowledgeResponse,
            knowledge_base::KnowledgeDetailResponse,
            knowledge_base::KnowledgeTreeNode,
            knowledge_base::KnowledgeCreateReq,
            knowledge_base::KnowledgeUpdateReq,
            knowledge_base::ReparseKnowledgeBaseResponse,
            file::File,
            file::UpdateFileReq,
            file::Slice,
            file::SlicePosition,
            search::SearchResult,
            search::SearchResultItem,
            search::SlicePosition,
            search::FullSearchResult,
            search::FullSearchResultItem,
            search::FileInfo,
            search::KbInfo,
            graph::EntityInfo,
            graph::EntityDetail,
            graph::NeighborInfo,
            graph::MentionInfo,
            graph::GraphStats,
            system::MemoryUsage,
            system::HeapProfileStatus,
        )
    ),
    tags(
        (name = "knowledge_base", description = "知识库管理接口"),
        (name = "file", description = "文件管理接口"),
        (name = "search", description = "搜索接口"),
        (name = "graph", description = "知识图谱接口"),
        (name = "system", description = "系统监控接口")
    ),
    info(
        title = "HTKnow API",
        version = "0.1.0",
        description = "HTKnow 知识库管理系统 API 文档",
    ),
)]
pub struct ApiDoc;

/// 获取 OpenAPI 文档
pub fn openapi() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}

pub fn app(pool: SqlitePool, search_engine: SearchEngine) -> Router {
    let knowledge_router = Router::new()
        .route("/", get(knowledge_base::list).post(knowledge_base::create))
        .route("/reparse", post(knowledge_base::reparse))
        .route("/tree", get(knowledge_base::tree))
        .route("/{id}", get(knowledge_base::get).put(knowledge_base::update).delete(knowledge_base::delete));
    let file_router = Router::new()
        .route("/", get(file::list).post(file::upload))
        .route("/images/{filename}", get(file::get_image_by_filename))
        .route("/{id}", get(file::get).put(file::update).delete(file::delete))
        .route("/{id}/slices", get(file::get_slices))
        .route("/{id}/download", get(file::download))
        .route("/{id}/highlighted-pdf", get(file::get_highlighted_pdf));
    let search_router = Router::new()
        .route("/", get(search::search))
        .route("/full", get(search::search_full))
        .route("/graph", get(search::search_with_graph))
        .route("/image", post(search::search_image));
    let graph_router = Router::new()
        .route("/entities", get(graph::search_entities))
        .route("/entities/{id}", get(graph::get_entity))
        .route("/stats", get(graph::get_graph_stats));
    let system_router = Router::new()
        .route("/memory", get(system::memory_usage))
        .route("/heap", get(system::heap_profile))
        .route("/heap/status", get(system::heap_profile_status))
        .route("/heap/pdf", get(system::heap_profile_pdf));

    Router::new()
        .nest("/knowledge_base/", knowledge_router)
        .nest("/files/", file_router)
        .nest("/search/", search_router)
        .nest("/graph/", graph_router)
        .nest("/system/", system_router)
        .with_state(pool)
        .layer(Extension(search_engine))
}
