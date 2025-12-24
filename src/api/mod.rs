use axum::{Router, routing::get};
use sqlx::SqlitePool;

mod error;
mod knowledge;

pub fn app(pool: SqlitePool) -> Router {
    let knowledge_router = Router::new()
        .route("/", get(knowledge::list))
        .with_state(pool);
    Router::new().nest("/knowledge/", knowledge_router)
}
