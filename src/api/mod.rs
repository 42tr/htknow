use axum::{Extension, Router, routing::get};
use sqlx::SqlitePool;

mod error;
mod knowledge_base;
use crate::search::SearchEngine;

pub fn app(pool: SqlitePool, search_engine: SearchEngine) -> Router {
    let knowledge_router = Router::new()
        .route(
            "/",
            get(knowledge_base::list).put(knowledge_base::create).post(knowledge_base::update),
        )
        .route(
            "/{id}",
            get(knowledge_base::get).delete(knowledge_base::delete),
        )
        .with_state(pool)
        .layer(Extension(search_engine));
    Router::new().nest("/knowledge/", knowledge_router)
}
