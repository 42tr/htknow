use axum::{
    Extension, Router, routing::{get, post}
};
use sqlx::SqlitePool;

mod error;
mod file;
mod knowledge_base;
use crate::search::SearchEngine;

// 重新导出 File 类型供其他模块使用
pub use file::File;

pub fn app(pool: SqlitePool, search_engine: SearchEngine) -> Router {
    let knowledge_router = Router::new()
        .route("/", get(knowledge_base::list).post(knowledge_base::create))
        .route("/{id}", get(knowledge_base::get).put(knowledge_base::update).delete(knowledge_base::delete));
    let file_router = Router::new()
        .route("/", post(file::upload))
        .route("/{id}", get(file::get).put(file::update).delete(file::delete));
    Router::new()
        .nest("/knowledge_base/", knowledge_router)
        .nest("/files/", file_router)
        .with_state(pool)
        .layer(Extension(search_engine))
}
