use axum::{
    Extension, Json, extract::{Query, State}, response::Response
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::ToSchema;

use crate::{
    AuthUser, api::{
        common, error::{ApiError, ApiResult}
    }, search::{SearchEngine, tantivy_engine::ForceMergeStats}, settings::{self, SettingItem, UpdateSettingsRequest}
};

#[derive(Debug, Deserialize)]
pub struct SettingsQuery {
    pub group: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SettingsResponse {
    pub settings: std::collections::BTreeMap<String, SettingItem>,
}

#[utoipa::path(
    get,
    path = "/api/v1/knowledge/system/settings",
    params(("group" = Option<String>, Query, description = "配置分组")),
    responses((status = 200, description = "系统配置", body = SettingsResponse)),
    security(("x-user-id" = [], "x-role" = []))
)]
pub async fn get_settings(
    Query(query): Query<SettingsQuery>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<SettingsResponse>> {
    common::ensure_admin(&auth_user)?;
    Ok(Json(SettingsResponse { settings: settings::list(query.group.as_deref()) }))
}

#[utoipa::path(
    put,
    path = "/api/v1/knowledge/system/settings",
    request_body = UpdateSettingsRequest,
    responses((status = 200, description = "配置已更新", body = SettingsResponse)),
    security(("x-user-id" = [], "x-role" = []))
)]
pub async fn update_settings(
    State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>, Json(req): Json<UpdateSettingsRequest>,
) -> ApiResult<Json<SettingsResponse>> {
    common::ensure_admin(&auth_user)?;
    settings::update(&pool, &auth_user.user_id, &req.settings)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(SettingsResponse { settings: settings::list(None) }))
}

/// 进程内存占用
#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryUsage {
    /// 进程 ID
    pub pid: u32,
    /// 常驻内存（RSS），单位：字节
    pub rss_bytes: u64,
    /// 虚拟内存，单位：字节
    pub virtual_bytes: u64,
}

/// 堆分析状态
#[derive(Debug, Serialize, ToSchema)]
pub struct HeapProfileStatus {
    /// jemalloc 版本
    pub jemalloc_version: Option<String>,
    /// 是否启用 heap profiling（opt.prof）
    pub prof_enabled: Option<bool>,
    /// 是否开启采样（prof.active）
    pub prof_active: Option<bool>,
    /// jemalloc 构建时默认配置
    pub malloc_conf: Option<String>,
    /// 运行时环境变量 MALLOC_CONF
    pub env_malloc_conf: Option<String>,
    /// 状态读取失败的提示
    pub warnings: Vec<String>,
}

/// LanceDB compact 统计信息
#[derive(Debug, Serialize, ToSchema)]
pub struct LanceDbCompactStats {
    /// 删除行数（本次 compact 清理）
    pub deleted_rows: u64,
    /// compact 前被标记删除的行数
    pub deleted_rows_before: u64,
    /// compact 后被标记删除的行数
    pub deleted_rows_after: u64,
    /// compact 前总行数
    pub total_rows_before: u64,
    /// compact 后总行数
    pub total_rows_after: u64,
    /// compact 前磁盘大小（字节）
    pub size_before_bytes: u64,
    /// compact 后磁盘大小（字节）
    pub size_after_bytes: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IndexRebuildStatus {
    /// 任务 ID（无任务时为 null）
    pub job_id: Option<i64>,
    /// 任务状态：idle/running/completed/failed
    pub status: String,
    /// 当前阶段描述
    pub phase: String,
    /// 总文档数
    pub total_docs: i64,
    /// 已处理文档数
    pub processed_docs: i64,
    /// 进度百分比（0-100）
    pub progress_pct: f32,
    /// 已运行秒数
    pub elapsed_secs: Option<i64>,
    /// 预计剩余秒数（ETA）
    pub eta_secs: Option<i64>,
    /// 任务开始时间（秒级时间戳）
    pub started_at: Option<i64>,
    /// 最近更新时间（秒级时间戳）
    pub updated_at: Option<i64>,
    /// 任务结束时间（秒级时间戳）
    pub finished_at: Option<i64>,
    /// 失败时错误信息
    pub error: Option<String>,
}

/// 单个 Tantivy 索引 force merge 统计信息
#[derive(Debug, Serialize, ToSchema)]
pub struct TantivyForceMergeIndexStats {
    /// 索引名称：index/full_index
    pub index: String,
    /// merge 前 segment 数
    pub before_segments: usize,
    /// merge 后 segment 数
    pub after_segments: usize,
    /// merge 前存活文档数
    pub before_docs: u64,
    /// merge 后存活文档数
    pub after_docs: u64,
    /// merge 前已删除文档数
    pub before_deleted_docs: u64,
    /// merge 后已删除文档数
    pub after_deleted_docs: u64,
    /// GC 删除的不再被索引引用的文件数
    pub gc_deleted_files: usize,
    /// GC 删除失败的文件数
    pub gc_failed_files: usize,
    /// 是否因为 segment 少于 2 个而跳过
    pub skipped: bool,
    /// 单个索引耗时（毫秒）
    pub duration_ms: u128,
}

/// Tantivy force merge 响应
#[derive(Debug, Serialize, ToSchema)]
pub struct TantivyForceMergeResponse {
    /// 普通切片索引统计
    pub index: TantivyForceMergeIndexStats,
    /// 全文索引统计
    pub full_index: TantivyForceMergeIndexStats,
    /// 总耗时（毫秒）
    pub total_duration_ms: u128,
}

#[derive(Debug, sqlx::FromRow)]
struct IndexRebuildStatusRow {
    id: i64,
    status: String,
    phase: String,
    total_docs: i64,
    processed_docs: i64,
    started_at: Option<i64>,
    updated_at: Option<i64>,
    finished_at: Option<i64>,
    error: Option<String>,
}

/// 生成当前进程堆内存快照（jemalloc heap profile）
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/system/heap",
    operation_id = "system_heap_profile",
    tag = "system",
    responses(
        (status = 200, description = "成功返回堆分析快照", body = Vec<u8>, content_type = "application/octet-stream"),
        (status = 400, description = "未启用堆分析"),
        (status = 500, description = "生成堆分析快照失败")
    )
)]
pub async fn heap_profile() -> ApiResult<Response> {
    heap_profile_impl().await
}

/// 生成堆内存分析 PDF 报告（使用 jeprof）
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/system/heap/pdf",
    operation_id = "system_heap_profile_pdf",
    tag = "system",
    responses(
        (status = 200, description = "成功返回堆分析 PDF 报告", body = Vec<u8>, content_type = "application/pdf"),
        (status = 400, description = "未启用堆分析或 jeprof 不可用"),
        (status = 500, description = "生成堆分析 PDF 失败")
    )
)]
pub async fn heap_profile_pdf() -> ApiResult<Response> {
    heap_profile_pdf_impl().await
}

/// 主动触发 LanceDB compact
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/system/lancedb/compact",
    operation_id = "system_lancedb_compact",
    tag = "system",
    responses(
        (status = 200, description = "LanceDB compact 成功", body = LanceDbCompactStats),
        (status = 500, description = "LanceDB compact 失败")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn lancedb_compact(
    Extension(search_engine): Extension<SearchEngine>,
) -> ApiResult<Json<LanceDbCompactStats>> {
    let stats = search_engine
        .compact_lancedb()
        .await
        .map_err(|e| ApiError::internal(format!("LanceDB compact failed: {}", e)))?;

    Ok(Json(LanceDbCompactStats {
        deleted_rows: stats.deleted_rows,
        deleted_rows_before: stats.deleted_rows_before,
        deleted_rows_after: stats.deleted_rows_after,
        total_rows_before: stats.total_rows_before,
        total_rows_after: stats.total_rows_after,
        size_before_bytes: stats.size_before_bytes,
        size_after_bytes: stats.size_after_bytes,
    }))
}

/// 强制合并 Tantivy segment，减少索引碎片
#[utoipa::path(
    post,
    path = "/api/v1/knowledge/system/index/force-merge",
    operation_id = "system_index_force_merge",
    tag = "system",
    responses(
        (status = 200, description = "Tantivy force merge 成功", body = TantivyForceMergeResponse),
        (status = 400, description = "权限不足"),
        (status = 500, description = "Tantivy force merge 失败")
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn index_force_merge(
    Extension(search_engine): Extension<SearchEngine>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<TantivyForceMergeResponse>> {
    common::ensure_admin(&auth_user)?;

    let start = std::time::Instant::now();
    let (index_stats, full_index_stats) = search_engine
        .force_merge_tantivy_indexes()
        .await
        .map_err(|e| ApiError::internal(format!("Tantivy force merge failed: {}", e)))?;

    Ok(Json(TantivyForceMergeResponse {
        index: force_merge_index_stats("index", index_stats),
        full_index: force_merge_index_stats("full_index", full_index_stats),
        total_duration_ms: start.elapsed().as_millis(),
    }))
}

/// 查询索引重建进度与 ETA
#[utoipa::path(
    get,
    path = "/api/v1/knowledge/system/index/rebuild/status",
    operation_id = "system_index_rebuild_status",
    tag = "system",
    responses(
        (status = 200, description = "成功返回索引重建状态", body = IndexRebuildStatus)
    ),
    security(
        ("x-user-id" = []),
        ("x-role" = [])
    )
)]
pub async fn index_rebuild_status(
    State(pool): State<SqlitePool>, Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<IndexRebuildStatus>> {
    common::ensure_admin(&auth_user)?;

    let row: Option<IndexRebuildStatusRow> = sqlx::query_as(
        "SELECT id, status, phase, total_docs, processed_docs, started_at, updated_at, finished_at, error
         FROM index_rebuild_jobs
         ORDER BY CASE WHEN status = 'running' THEN 0 ELSE 1 END, id DESC
         LIMIT 1",
    )
    .fetch_optional(&pool)
    .await?;

    let Some(row) = row else {
        return Ok(Json(IndexRebuildStatus {
            job_id: None,
            status: "idle".to_string(),
            phase: String::new(),
            total_docs: 0,
            processed_docs: 0,
            progress_pct: 0.0,
            elapsed_secs: None,
            eta_secs: None,
            started_at: None,
            updated_at: None,
            finished_at: None,
            error: None,
        }));
    };

    let total_docs = row.total_docs.max(0);
    let processed_docs = row.processed_docs.clamp(0, total_docs.max(0));
    let progress_pct = if total_docs > 0 {
        ((processed_docs as f64 / total_docs as f64) * 100.0).clamp(0.0, 100.0) as f32
    } else if row.status == "completed" {
        100.0
    } else {
        0.0
    };

    let now = Utc::now().timestamp();
    let elapsed_secs = match row.started_at {
        Some(start) => match row.status.as_str() {
            "running" => Some((now - start).max(0)),
            _ => row.finished_at.map(|finish| (finish - start).max(0)).or(Some((now - start).max(0))),
        },
        None => None,
    };
    let eta_secs = if row.status == "running" && total_docs > processed_docs {
        match elapsed_secs {
            Some(elapsed) if elapsed > 0 && processed_docs > 0 => {
                let speed = processed_docs as f64 / elapsed as f64;
                if speed > 0.0 {
                    let remaining = (total_docs - processed_docs) as f64;
                    Some((remaining / speed).ceil() as i64)
                } else {
                    None
                }
            }
            _ => None,
        }
    } else if row.status == "completed" {
        Some(0)
    } else {
        None
    };

    Ok(Json(IndexRebuildStatus {
        job_id: Some(row.id),
        status: row.status,
        phase: row.phase,
        total_docs,
        processed_docs,
        progress_pct,
        elapsed_secs,
        eta_secs,
        started_at: row.started_at,
        updated_at: row.updated_at,
        finished_at: row.finished_at,
        error: row.error,
    }))
}

fn force_merge_index_stats(index: &str, stats: ForceMergeStats) -> TantivyForceMergeIndexStats {
    TantivyForceMergeIndexStats {
        index: index.to_string(),
        before_segments: stats.before_segments,
        after_segments: stats.after_segments,
        before_docs: stats.before_docs,
        after_docs: stats.after_docs,
        before_deleted_docs: stats.before_deleted_docs,
        after_deleted_docs: stats.after_deleted_docs,
        gc_deleted_files: stats.gc_deleted_files,
        gc_failed_files: stats.gc_failed_files,
        skipped: stats.skipped,
        duration_ms: stats.duration_ms,
    }
}

#[cfg(feature = "profiling")]
async fn heap_profile_impl() -> ApiResult<Response> {
    use std::{
        ffi::CString, io::Seek, time::{SystemTime, UNIX_EPOCH}
    };

    use axum::{
        body::Body, http::{HeaderValue, header}
    };
    use tempfile::NamedTempFile;
    use tikv_jemalloc_ctl::raw;
    use tokio_util::io::ReaderStream;

    let pid = std::process::id();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| ApiError::internal(format!("SystemTime error: {}", e)))?
        .as_secs();
    let filename = format!("htknow.heap.{}.{}.heap", pid, ts);

    // 将堆 dump 写入临时文件后立即删除路径，只保留 fd 用于流式返回。
    let temp = NamedTempFile::new().map_err(|e| ApiError::internal(format!("create temp file failed: {}", e)))?;
    let path = temp.path().to_path_buf();
    let path_str = path.to_string_lossy().into_owned();
    let c_path =
        CString::new(path_str.clone()).map_err(|_| ApiError::internal("Failed to create C string for path"))?;

    let ptr = c_path.as_ptr();
    unsafe {
        raw::write(b"prof.dump\0", ptr).map_err(|e| ApiError::internal(format!("jemalloc heap dump failed: {}", e)))?;
    }

    let mut std_file = temp.into_file();
    std_file
        .seek(std::io::SeekFrom::Start(0))
        .map_err(|e| ApiError::internal(format!("seek temp file failed: {}", e)))?;
    let file = tokio::fs::File::from_std(std_file);
    let len = file.metadata().await.map_err(|e| ApiError::internal(format!("metadata failed: {}", e)))?.len();
    let stream = ReaderStream::new(file);

    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&len.to_string())
            .map_err(|e| ApiError::internal(format!("Invalid header value: {}", e)))?,
    );
    let disposition = format!("attachment; filename=\"{}\"", filename);
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).map_err(|e| ApiError::internal(format!("Invalid header value: {}", e)))?,
    );

    Ok(response)
}

#[cfg(not(feature = "profiling"))]
async fn heap_profile_impl() -> ApiResult<Response> {
    Err(ApiError::BadRequest("Heap profiling is only available with 'profiling' feature enabled.".to_string()))
}

#[cfg(feature = "profiling")]
async fn heap_profile_pdf_impl() -> ApiResult<Response> {
    use std::{
        ffi::CString, process::Stdio, time::{SystemTime, UNIX_EPOCH}
    };

    use axum::{
        body::Body, http::{HeaderValue, header}
    };
    use tempfile::NamedTempFile;
    use tikv_jemalloc_ctl::raw;
    use tokio::{io::AsyncSeekExt, process::Command};
    use tokio_util::io::ReaderStream;

    let pid = std::process::id();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| ApiError::internal(format!("SystemTime error: {}", e)))?
        .as_secs();

    // 生成 heap dump 文件（路径需要保持到 jeprof 执行完毕）
    let heap_temp = NamedTempFile::new().map_err(|e| ApiError::internal(format!("create heap temp failed: {}", e)))?;
    let heap_path = heap_temp.path().to_path_buf();
    let heap_path_str = heap_path.to_string_lossy().into_owned();
    let c_path = CString::new(heap_path_str.clone())
        .map_err(|_| ApiError::internal("Failed to create C string for heap path"))?;

    log::info!("Generating heap dump: {}", heap_path_str);
    let ptr = c_path.as_ptr();
    unsafe {
        raw::write(b"prof.dump\0", ptr).map_err(|e| ApiError::internal(format!("jemalloc heap dump failed: {}", e)))?;
    }

    // 生成 PDF 文件（用 NamedTempFile 承接 jeprof 标准输出）
    let pdf_temp = NamedTempFile::new().map_err(|e| ApiError::internal(format!("create pdf temp failed: {}", e)))?;
    let pdf_std = pdf_temp.into_file();
    let mut pdf_file = tokio::fs::File::from_std(pdf_std);

    // 获取当前二进制文件路径
    let binary_path =
        std::env::current_exe().map_err(|e| ApiError::internal(format!("Failed to get binary path: {}", e)))?;
    let binary_path_str = binary_path.to_string_lossy();

    log::info!("Running jeprof: binary={}, heap={}, output=pdf_stream", binary_path_str, heap_path_str);

    let mut child = Command::new("jeprof")
        .arg("--show_bytes")
        .arg("--pdf")
        .arg(binary_path_str.as_ref())
        .arg(&heap_path_str)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| ApiError::internal(format!("Failed to execute jeprof (make sure jeprof is installed): {}", e)))?;

    let mut stdout = child.stdout.take().ok_or_else(|| ApiError::internal("jeprof stdout unavailable"))?;
    tokio::io::copy(&mut stdout, &mut pdf_file)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to copy jeprof output: {}", e)))?;

    let status = child.wait().await.map_err(|e| ApiError::internal(format!("jeprof process wait failed: {}", e)))?;

    // jeprof 已完成，heap dump 临时文件可以清理
    drop(heap_temp);

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        log::error!("jeprof failed with exit code: {}", code);
        return Err(ApiError::internal(format!("jeprof execution failed with exit code: {}", code)));
    }

    pdf_file
        .seek(std::io::SeekFrom::Start(0))
        .await
        .map_err(|e| ApiError::internal(format!("seek pdf temp failed: {}", e)))?;
    let len = pdf_file.metadata().await.map_err(|e| ApiError::internal(format!("pdf metadata failed: {}", e)))?.len();
    if len == 0 {
        return Err(ApiError::internal("jeprof generated empty PDF"));
    }

    let pdf_filename = format!("htknow.heap.{}.{}.pdf", pid, ts);
    log::info!("Successfully generated heap profile PDF: {} bytes", len);

    let stream = ReaderStream::new(pdf_file);
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&len.to_string())
            .map_err(|e| ApiError::internal(format!("Invalid header value: {}", e)))?,
    );
    let disposition = format!("attachment; filename=\"{}\"", pdf_filename);
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).map_err(|e| ApiError::internal(format!("Invalid header value: {}", e)))?,
    );

    Ok(response)
}

#[cfg(not(feature = "profiling"))]
async fn heap_profile_pdf_impl() -> ApiResult<Response> {
    Err(ApiError::BadRequest("Heap profiling is only available with 'profiling' feature enabled.".to_string()))
}
