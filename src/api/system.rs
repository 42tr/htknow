use axum::response::Response;
use serde::Serialize;
use utoipa::ToSchema;

use crate::api::error::{ApiError, ApiResult};

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

#[cfg(debug_assertions)]
async fn heap_profile_impl() -> ApiResult<Response> {
    use std::{
        ffi::CString, time::{SystemTime, UNIX_EPOCH}
    };

    use axum::{
        body::Body, http::{HeaderValue, header}
    };
    use tikv_jemalloc_ctl::raw;
    use tokio::fs;

    let pid = std::process::id();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| ApiError::internal(format!("SystemTime error: {}", e)))?
        .as_secs();
    let filename = format!("htknow.heap.{}.{}.heap", pid, ts);

    let mut path = std::env::temp_dir();
    path.push(&filename);
    let path_str = path.to_string_lossy().into_owned();
    let c_path =
        CString::new(path_str.clone()).map_err(|_| ApiError::internal("Failed to create C string for path"))?;

    // 使用 raw::write，传递 C 字符串指针
    let ptr = c_path.as_ptr();
    unsafe {
        raw::write(b"prof.dump\0", ptr).map_err(|e| ApiError::internal(format!("jemalloc heap dump failed: {}", e)))?;
    }

    let bytes = fs::read(&path).await?;
    let _ = fs::remove_file(&path).await;

    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    let disposition = format!("attachment; filename=\"{}\"", filename);
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).map_err(|e| ApiError::internal(format!("Invalid header value: {}", e)))?,
    );

    Ok(response)
}

#[cfg(not(debug_assertions))]
async fn heap_profile_impl() -> ApiResult<Response> {
    Err(ApiError::BadRequest("Heap profiling is only available in debug builds.".to_string()))
}

#[cfg(debug_assertions)]
async fn heap_profile_pdf_impl() -> ApiResult<Response> {
    use std::{
        ffi::CString, time::{SystemTime, UNIX_EPOCH}
    };

    use axum::{
        body::Body, http::{HeaderValue, header}
    };
    use tikv_jemalloc_ctl::raw;
    use tokio::{fs, process::Command};

    let pid = std::process::id();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| ApiError::internal(format!("SystemTime error: {}", e)))?
        .as_secs();

    // 生成 heap dump 文件
    let heap_filename = format!("htknow.heap.{}.{}.heap", pid, ts);
    let mut heap_path = std::env::temp_dir();
    heap_path.push(&heap_filename);
    let heap_path_str = heap_path.to_string_lossy().into_owned();
    let c_path = CString::new(heap_path_str.clone())
        .map_err(|_| ApiError::internal("Failed to create C string for heap path"))?;

    // 导出 heap dump
    log::info!("Generating heap dump: {}", heap_path_str);
    let ptr = c_path.as_ptr();
    unsafe {
        raw::write(b"prof.dump\0", ptr).map_err(|e| ApiError::internal(format!("jemalloc heap dump failed: {}", e)))?;
    }

    // 生成 PDF 文件
    let pdf_filename = format!("htknow.heap.{}.{}.pdf", pid, ts);
    let mut pdf_path = std::env::temp_dir();
    pdf_path.push(&pdf_filename);
    let pdf_path_str = pdf_path.to_string_lossy().into_owned();

    // 获取当前二进制文件路径
    let binary_path =
        std::env::current_exe().map_err(|e| ApiError::internal(format!("Failed to get binary path: {}", e)))?;
    let binary_path_str = binary_path.to_string_lossy();

    log::info!("Running jeprof: binary={}, heap={}, output={}", binary_path_str, heap_path_str, pdf_path_str);

    // 执行 jeprof 命令
    let output = Command::new("jeprof")
        .arg("--show_bytes")
        .arg("--pdf")
        .arg(binary_path_str.as_ref())
        .arg(&heap_path_str)
        .output()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to execute jeprof (make sure jeprof is installed): {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!("jeprof failed: {}", stderr);
        let _ = fs::remove_file(&heap_path).await;
        return Err(ApiError::internal(format!("jeprof execution failed: {}", stderr)));
    }

    // 读取生成的 PDF
    let pdf_bytes = output.stdout;
    if pdf_bytes.is_empty() {
        let _ = fs::remove_file(&heap_path).await;
        return Err(ApiError::internal("jeprof generated empty PDF"));
    }

    // 清理临时文件
    let _ = fs::remove_file(&heap_path).await;

    log::info!("Successfully generated heap profile PDF: {} bytes", pdf_bytes.len());

    // 返回 PDF 响应
    let mut response = Response::new(Body::from(pdf_bytes));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    let disposition = format!("attachment; filename=\"{}\"", pdf_filename);
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).map_err(|e| ApiError::internal(format!("Invalid header value: {}", e)))?,
    );

    Ok(response)
}

#[cfg(not(debug_assertions))]
async fn heap_profile_pdf_impl() -> ApiResult<Response> {
    Err(ApiError::BadRequest("Heap profiling is only available in debug builds.".to_string()))
}
