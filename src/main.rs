#[cfg(debug_assertions)]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(debug_assertions)]
#[unsafe(export_name = "_rjem_malloc_conf")]
// lg_prof_sample:0 表示每次分配都采样(最详细,但性能开销大)
// lg_prof_sample:10 表示每 1KB 采样一次(推荐)
// lg_prof_sample:19 表示每 512KB 采样一次(默认值)
pub static MALLOC_CONF: &[u8] = b"prof:true,prof_active:true,lg_prof_sample:10\0";

use std::{net::SocketAddr, sync::Arc};

use axum::{Router, extract::DefaultBodyLimit, middleware, response::Html, routing::get};
use chrono::Local;
use htknow::{api, auth, config, db, frontend, log4rs, processor, search};
use tokio::net::TcpListener;
use tokio_cron_scheduler::{Job, JobScheduler};
use utoipa_swagger_ui::SwaggerUi;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    log4rs::init();

    // 加载配置
    let cfg = config::get();
    log::info!("Configuration loaded: server={}:{}", cfg.server.host, cfg.server.port);

    let pool = db::init().await?;
    let search_engine = search::SearchEngine::init().await.with_pool(pool.clone());
    match search_engine.reload_lexicon().await {
        Ok(loaded) => log::info!("Search lexicon loaded: {} words", loaded),
        Err(e) => log::warn!("Failed to load search lexicon at startup: {}", e),
    }

    let processor =
        processor::FileProcessor::new(pool.clone(), search_engine.clone(), cfg.server.process_interval_secs);
    if cfg.server.parse_enabled {
        processor.start();
    } else {
        log::warn!("HTKNOW_PARSE_ENABLED=false，后台文件解析已禁用，仅支持即时解析");
    }
    let cron = cfg.server.lancedb_compact_cron.trim();
    let _lancedb_compact_scheduler = if !cron.is_empty()
        && !cron.eq_ignore_ascii_case("off")
        && !cron.eq_ignore_ascii_case("disabled")
        && cron != "0"
    {
        let compact_lock = Arc::new(tokio::sync::Mutex::new(()));
        let search_engine = search_engine.clone();
        let sched = JobScheduler::new().await?;
        let job_lock = compact_lock.clone();

        let job = Job::new_async_tz(cron, Local, move |_uuid, _lock| {
            let search_engine = search_engine.clone();
            let job_lock = job_lock.clone();
            Box::pin(async move {
                let _guard = job_lock.lock().await;
                match search_engine.compact_lancedb().await {
                    Ok(stats) => {
                        log::info!(
                            "LanceDB auto-compact done: deleted_rows={}, total_rows_before={}, total_rows_after={}, size_before_bytes={}, size_after_bytes={}",
                            stats.deleted_rows,
                            stats.total_rows_before,
                            stats.total_rows_after,
                            stats.size_before_bytes,
                            stats.size_after_bytes,
                        );
                    }
                    Err(e) => {
                        log::error!("LanceDB auto-compact failed: {}", e);
                    }
                }
            })
        })?;

        sched.add(job).await?;
        sched.start().await?;
        log::warn!("LanceDB auto-compact enabled; avoid concurrent writes during compaction");
        log::info!("LanceDB auto-compact cron: {}", cron);
        Some(sched)
    } else {
        None
    };

    // API 路由需要认证
    let upload_limit = cfg.server.upload_limit_mb * 1024 * 1024;
    let api_router = Router::new()
        .nest("/api/v1/knowledge/", api::app(pool, search_engine))
        .layer(middleware::from_fn(auth))
        .layer(DefaultBodyLimit::max(upload_limit));

    // Swagger 路由（不需要认证）
    let swagger = Router::new()
        .route("/docs", get(swagger_ui_handler))
        .merge(SwaggerUi::new("/swagger-ui-assets").url("/api-docs/openapi.json", api::openapi()));

    // 合并路由：前端和 Swagger 不需要认证，API 需要认证
    let app = Router::new().merge(swagger).merge(api_router).merge(frontend::router());

    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let listener = TcpListener::bind(&addr).await?;
    log::info!("Server listening on {}", addr);
    log::info!("Swagger UI available at http://{}/docs", addr);
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}

/// Swagger UI HTML handler
async fn swagger_ui_handler() -> Html<&'static str> {
    Html(
        r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>HTKnow API Documentation</title>
    <link rel="stylesheet" href="/swagger-ui-assets/swagger-ui.css">
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="/swagger-ui-assets/swagger-ui-bundle.js"></script>
    <script src="/swagger-ui-assets/swagger-ui-standalone-preset.js"></script>
    <script>
        window.onload = function() {
            SwaggerUIBundle({
                url: '/api-docs/openapi.json',
                dom_id: '#swagger-ui',
                presets: [
                    SwaggerUIBundle.presets.apis,
                    SwaggerUIStandalonePreset
                ],
                layout: "StandaloneLayout",
                requestInterceptor: (req) => {
                    req.headers['x-user-id'] = localStorage.getItem('swagger-x-user-id') || '1';
                    req.headers['x-user-name'] = localStorage.getItem('swagger-x-user-name') || 'testuser';
                    req.headers['x-role'] = localStorage.getItem('swagger-x-role') || 'admin';
                    return req;
                }
            });
        };
    </script>
    <div id="swagger-auth-config" style="position:fixed;top:10px;right:80px;z-index:1000;background:#fff;padding:10px;border:1px solid #ddd;border-radius:4px;box-shadow:0 2px 5px rgba(0,0,0,0.1);">
        <input type="text" id="auth-user-id" placeholder="x-user-id" style="width:80px;padding:4px;margin-right:5px;border:1px solid #ccc;border-radius:3px;">
        <input type="text" id="auth-user-name" placeholder="x-user-name" style="width:100px;padding:4px;margin-right:5px;border:1px solid #ccc;border-radius:3px;">
        <input type="text" id="auth-role" placeholder="x-role" style="width:60px;padding:4px;margin-right:5px;border:1px solid #ccc;border-radius:3px;">
        <button onclick="saveAuthConfig()" style="padding:4px 10px;background:#007bff;color:#fff;border:none;border-radius:3px;cursor:pointer;">保存认证</button>
    </div>
    <script>
        document.getElementById('auth-user-id').value = localStorage.getItem('swagger-x-user-id') || '1';
        document.getElementById('auth-user-name').value = localStorage.getItem('swagger-x-user-name') || 'testuser';
        document.getElementById('auth-role').value = localStorage.getItem('swagger-x-role') || 'admin';

        function saveAuthConfig() {
            localStorage.setItem('swagger-x-user-id', document.getElementById('auth-user-id').value);
            localStorage.setItem('swagger-x-user-name', document.getElementById('auth-user-name').value);
            localStorage.setItem('swagger-x-role', document.getElementById('auth-role').value);
            location.reload();
        }
    </script>
</body>
</html>
    "#,
    )
}
