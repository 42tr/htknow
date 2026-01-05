use axum::{Extension, Router, routing::get};
use sqlx::SqlitePool;

mod error;
mod file;
mod graph;
mod knowledge_base;
mod search;
// 重新导出 File 类型供其他模块使用
pub use file::File;

use crate::search::SearchEngine;

pub fn app(pool: SqlitePool, search_engine: SearchEngine) -> Router {
    let knowledge_router = Router::new()
        .route("/", get(knowledge_base::list).post(knowledge_base::create))
        .route("/{id}", get(knowledge_base::get).put(knowledge_base::update).delete(knowledge_base::delete));
    let file_router = Router::new()
        .route("/", get(file::list).post(file::upload))
        .route("/{id}", get(file::get).put(file::update).delete(file::delete))
        .route("/{id}/slices", get(file::get_slices))
        .route("/{id}/images", get(file::get_images))
        .route("/{file_id}/images/{image_id}", get(file::get_image));
    let search_router = Router::new().route("/", get(search::search)).route("/graph", get(search::search_with_graph));
    let graph_router = Router::new()
        .route("/entities", get(graph::search_entities))
        .route("/entities/{id}", get(graph::get_entity))
        .route("/stats", get(graph::get_graph_stats));

    Router::new()
        .nest("/knowledge_base/", knowledge_router)
        .nest("/files/", file_router)
        .nest("/search/", search_router)
        .nest("/graph/", graph_router)
        .with_state(pool)
        .layer(Extension(search_engine))
}
