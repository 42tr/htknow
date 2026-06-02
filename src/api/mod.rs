use axum::{
    Extension, Router,
    routing::{get, post, put},
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
pub(crate) use file::{
    collect_image_paths_for_files, find_reusable_parsed_file, remove_image_files, resolve_image_storage_path,
};

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
        knowledge_base::reparse_by_id,
        knowledge_base::delete,
        knowledge_base::tree,
        knowledge_base::batch_export_kb,
        // File
        file::upload,
        file::list,
        file::stats,
        file::get,
        file::update,
        file::move_to_kb,
        file::batch_delete,
        file::reparse_failed,
        file::delete,
        file::get_slices,
        file::get_image_by_filename,
        file::download,
        file::get_highlighted_pdf,
        file::excel_data,
        // Search
        search::search,
        search::search_full,
        search::search_with_graph,
        search::search_image,
        search::advanced_search_stream,
        search::list_lexicons,
        search::create_lexicon,
        search::update_lexicon,
        search::delete_lexicon,
        search::toggle_lexicon_enabled,
        search::reload_lexicon,
        search::publish_lexicon,
        search::list_synonyms,
        search::create_synonym,
        search::update_synonym,
        search::delete_synonym,
        search::toggle_synonym_enabled,
        // Graph
        graph::search_entities,
        graph::get_entity,
        graph::get_graph_stats,
        // System
        system::heap_profile,
        system::heap_profile_pdf,
        system::lancedb_compact,
        system::index_force_merge,
        system::index_rebuild_status,
    ),
    components(
        schemas(
            knowledge_base::Knowledge,
            knowledge_base::KnowledgeResponse,
            knowledge_base::KnowledgeDetailResponse,
            knowledge_base::KnowledgeTreeFile,
            knowledge_base::KnowledgeTreeNode,
            knowledge_base::KnowledgeCreateReq,
            knowledge_base::KnowledgeUpdateReq,
            knowledge_base::ReparseKnowledgeBaseResponse,
            knowledge_base::ExportKbResponse,
            knowledge_base::BatchExportKbRequest,
            crate::export::ExportManifest,
            file::File,
            file::UpdateFileReq,
            file::MoveFileReq,
            file::BatchDeleteFilesReq,
            file::BatchDeleteFilesResp,
            file::BatchDeleteSkippedItem,
            file::BatchDeleteCleanupFailedItem,
            file::ReparseFailedFilesReq,
            file::ReparseFailedFilesResp,
            file::Slice,
            file::SlicePosition,
            file::FileStatusBreakdown,
            file::ExcelData,
            file::ExcelSheetData,
            search::SearchResult,
            search::SearchResultItem,
            search::SlicePosition,
            search::FullSearchResult,
            search::FullSearchResultItem,
            search::FileInfo,
            search::KbInfo,
            search::LexiconItem,
            search::LexiconListResponse,
            search::CreateLexiconReq,
            search::UpdateLexiconReq,
            search::ToggleLexiconReq,
            search::DeleteLexiconResponse,
            search::ReloadLexiconResponse,
            search::PublishLexiconResponse,
            search::SynonymItem,
            search::SynonymListResponse,
            search::CreateSynonymReq,
            search::UpdateSynonymReq,
            search::ToggleSynonymReq,
            search::DeleteSynonymResponse,
            graph::EntityInfo,
            graph::EntityDetail,
            graph::NeighborInfo,
            graph::MentionInfo,
            graph::GraphStats,
            system::MemoryUsage,
            system::HeapProfileStatus,
            system::LanceDbCompactStats,
            system::TantivyForceMergeIndexStats,
            system::TantivyForceMergeResponse,
            system::IndexRebuildStatus,
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
        .route("/{id}/reparse", post(knowledge_base::reparse_by_id))
        .route("/export", post(knowledge_base::batch_export_kb))
        .route("/tree", get(knowledge_base::tree))
        .route("/{id}", get(knowledge_base::get).put(knowledge_base::update).delete(knowledge_base::delete));
    let file_router = Router::new()
        .route("/", get(file::list).post(file::upload))
        .route("/batch-delete", post(file::batch_delete))
        .route("/reparse-failed", post(file::reparse_failed))
        .route("/stats", get(file::stats))
        .route("/images/{filename}", get(file::get_image_by_filename))
        .route("/{id}", get(file::get).put(file::update).delete(file::delete))
        .route("/{id}/move", put(file::move_to_kb))
        .route("/{id}/slices", get(file::get_slices))
        .route("/{id}/download", get(file::download))
        .route("/{id}/highlighted-pdf", get(file::get_highlighted_pdf))
        .route("/{id}/excel-data", get(file::excel_data));
    let search_router = Router::new()
        .route("/", get(search::search))
        .route("/full", get(search::search_full))
        .route("/graph", get(search::search_with_graph))
        .route("/image", post(search::search_image))
        .route("/lexicons", get(search::list_lexicons).post(search::create_lexicon))
        .route("/lexicons/reload", post(search::reload_lexicon))
        .route("/lexicons/publish", post(search::publish_lexicon))
        .route("/lexicons/{id}", put(search::update_lexicon).delete(search::delete_lexicon))
        .route("/lexicons/{id}/enabled", put(search::toggle_lexicon_enabled))
        .route("/synonyms", get(search::list_synonyms).post(search::create_synonym))
        .route("/synonyms/{id}", put(search::update_synonym).delete(search::delete_synonym))
        .route("/synonyms/{id}/enabled", put(search::toggle_synonym_enabled))
        .route("/advanced/stream", get(search::advanced_search_stream));
    let graph_router = Router::new()
        .route("/entities", get(graph::search_entities))
        .route("/entities/{id}", get(graph::get_entity))
        .route("/stats", get(graph::get_graph_stats));
    let system_router = Router::new()
        .route("/heap", get(system::heap_profile))
        .route("/heap/pdf", get(system::heap_profile_pdf))
        .route("/lancedb/compact", post(system::lancedb_compact))
        .route("/index/force-merge", post(system::index_force_merge))
        .route("/index/rebuild/status", get(system::index_rebuild_status));

    Router::new()
        .nest("/knowledge_base/", knowledge_router)
        .nest("/files/", file_router)
        .nest("/search/", search_router)
        .nest("/graph/", graph_router)
        .nest("/system/", system_router)
        .with_state(pool)
        .layer(Extension(search_engine))
}
