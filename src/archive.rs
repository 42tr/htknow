//! 压缩文件解压模块
//!
//! 支持 ZIP、TAR（及其 GZ/BZ2/XZ 变体）格式的解压。
//! 7Z 和 RAR 格式暂不支持。

use std::io::Read;
use std::path::Path;

use serde::Serialize;
use utoipa::ToSchema;

/// 压缩包内单个文件条目
#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
pub struct ArchiveEntry {
    pub id: Option<i64>,
    pub file_id: i64,
    pub entry_path: String,
    pub size: Option<i64>,
    pub is_directory: bool,
}

/// 解压结果
#[derive(Debug, Serialize, ToSchema)]
pub struct ExtractResult {
    pub entries: Vec<ArchiveEntry>,
    pub needs_password: bool,
}

/// 压缩文件处理错误
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("密码错误或需要密码")]
    PasswordRequired,
    #[error("无效的密码")]
    InvalidPassword,
    #[error("不支持的压缩格式: {0}")]
    UnsupportedFormat(String),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("ZIP 错误: {0}")]
    Zip(String),
    #[error("解压大小超过限制: 最大 {max_mb}MB, 实际约 {actual_mb}MB")]
    SizeLimitExceeded { max_mb: u64, actual_mb: u64 },
    #[error("文件数量超过限制: 最大 {max}, 实际 {actual}")]
    FileCountExceeded { max: usize, actual: usize },
    #[error("{0}")]
    Other(String),
}

/// 解压安全限制
pub struct ExtractLimits {
    /// 最大解压后总大小（字节），默认 1GB
    pub max_total_size: u64,
    /// 最大文件数量，默认 10000
    pub max_file_count: usize,
    /// 单个文件最大大小（字节），默认 100MB
    pub max_file_size: u64,
}

impl Default for ExtractLimits {
    fn default() -> Self {
        Self {
            max_total_size: 1_073_741_824, // 1GB
            max_file_count: 10_000,
            max_file_size: 104_857_600, // 100MB
        }
    }
}

/// 判断文件是否为支持的压缩格式
pub fn is_archive_file(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    lower.ends_with(".zip")
        || lower.ends_with(".7z")
        || lower.ends_with(".tar")
        || lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
        || lower.ends_with(".tar.bz2")
        || lower.ends_with(".tar.xz")
}

/// 判断文件是否为 tar 变体格式
fn is_tar_variant(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    lower.ends_with(".tar")
        || lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
        || lower.ends_with(".tar.bz2")
        || lower.ends_with(".tar.xz")
}

/// 获取文件扩展名对应的压缩格式描述
fn archive_format_desc(filename: &str) -> &'static str {
    let lower = filename.to_lowercase();
    if lower.ends_with(".zip") {
        "ZIP"
    } else if lower.ends_with(".7z") {
        "7Z"
    } else if lower.ends_with(".tar") {
        "TAR"
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        "TAR.GZ"
    } else if lower.ends_with(".tar.bz2") {
        "TAR.BZ2"
    } else if lower.ends_with(".tar.xz") {
        "TAR.XZ"
    } else {
        "未知"
    }
}

/// 将压缩文件解压到指定目录
///
/// 返回解压后的文件列表
pub fn extract_archive(
    src_path: &str,
    dest_dir: &str,
    password: Option<&str>,
    file_id: i64,
) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let limits = ExtractLimits::default();
    extract_archive_with_limits(src_path, dest_dir, password, file_id, &limits)
}

/// 带限制的解压
pub fn extract_archive_with_limits(
    src_path: &str,
    dest_dir: &str,
    password: Option<&str>,
    file_id: i64,
    limits: &ExtractLimits,
) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let path = Path::new(src_path);
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(src_path);

    if !is_archive_file(filename) {
        return Err(ArchiveError::UnsupportedFormat(archive_format_desc(filename).to_string()));
    }

    // 创建目标目录
    std::fs::create_dir_all(dest_dir)?;

    let lower = filename.to_lowercase();
    if lower.ends_with(".zip") {
        extract_zip(src_path, dest_dir, password, file_id, limits)
    } else if lower.ends_with(".7z") {
        Err(ArchiveError::UnsupportedFormat("7Z 格式暂不支持".to_string()))
    } else if is_tar_variant(filename) {
        extract_tar(src_path, dest_dir, file_id, limits)
    } else {
        Err(ArchiveError::UnsupportedFormat(archive_format_desc(filename).to_string()))
    }
}

/// 读取压缩包内单个文件内容到内存
pub fn read_archive_entry(
    src_path: &str,
    entry_path: &str,
    password: Option<&str>,
) -> Result<Vec<u8>, ArchiveError> {
    let path = Path::new(src_path);
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(src_path);

    let lower = filename.to_lowercase();
    if lower.ends_with(".zip") {
        read_zip_entry(src_path, entry_path, password)
    } else if lower.ends_with(".7z") {
        Err(ArchiveError::UnsupportedFormat("7Z 格式暂不支持".to_string()))
    } else if is_tar_variant(filename) {
        read_tar_entry(src_path, entry_path)
    } else {
        Err(ArchiveError::UnsupportedFormat(archive_format_desc(filename).to_string()))
    }
}

// ============================================================================
// ZIP 解压
// ============================================================================

fn extract_zip(
    src_path: &str,
    dest_dir: &str,
    password: Option<&str>,
    file_id: i64,
    limits: &ExtractLimits,
) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let file = std::fs::File::open(src_path)?;
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => return Err(ArchiveError::Zip(e.to_string())),
    };

    let mut entries = Vec::new();
    let mut total_size: u64 = 0;
    let password_bytes = password.map(|p| p.as_bytes());

    log::info!("ZIP archive has {} entries, file_id={}", archive.len(), file_id);

    for i in 0..archive.len() {
        // 先检查文件是否加密
        let is_encrypted = {
            let file = archive.by_index(i).map_err(|e| ArchiveError::Zip(e.to_string()))?;
            file.encrypted()
        };

        let mut file_entry = if is_encrypted {
            if let Some(pw) = password_bytes {
                archive.by_index_decrypt(i, pw).map_err(|e| ArchiveError::Zip(e.to_string()))?
            } else {
                return Err(ArchiveError::PasswordRequired);
            }
        } else {
            archive.by_index(i).map_err(|e| ArchiveError::Zip(e.to_string()))?
        };

        // 统一路径分隔符为 /，并处理连续分隔符和首尾分隔符
        let name: String = file_entry.name()
            .replace('\\', "/")
            .split('/')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        if name.is_empty() {
            continue;
        }
        // zip crate 的 is_dir 只认 / 结尾；统一分隔符后重新判断
        let is_dir = file_entry.name().ends_with('/') || file_entry.name().ends_with('\\');
        let name = if is_dir && name.ends_with('/') {
            name.trim_end_matches('/').to_string()
        } else {
            name
        };
        let size = file_entry.size();

        log::debug!("ZIP entry: name={}, is_dir={}, size={}", name, is_dir, size);

        // 安全检查：路径遍历
        if name.contains("..") {
            log::warn!("Skipping ZIP entry with path traversal: {}", name);
            continue;
        }

        // 大小限制检查
        if !is_dir {
            if size > limits.max_file_size {
                return Err(ArchiveError::SizeLimitExceeded {
                    max_mb: limits.max_file_size / 1_048_576,
                    actual_mb: size / 1_048_576,
                });
            }
            total_size += size;
            if total_size > limits.max_total_size {
                return Err(ArchiveError::SizeLimitExceeded {
                    max_mb: limits.max_total_size / 1_048_576,
                    actual_mb: total_size / 1_048_576,
                });
            }
        }

        // 数量限制检查
        if entries.len() >= limits.max_file_count {
            return Err(ArchiveError::FileCountExceeded {
                max: limits.max_file_count,
                actual: entries.len(),
            });
        }

        let out_path = Path::new(dest_dir).join(&name);

        if is_dir {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out_file = std::fs::File::create(&out_path)?;
            std::io::copy(&mut file_entry, &mut out_file)?;
        }

        entries.push(ArchiveEntry {
            id: None,
            file_id,
            entry_path: name,
            size: Some(size as i64),
            is_directory: is_dir,
        });
    }

    log::info!("ZIP extraction completed: {} entries, file_id={}", entries.len(), file_id);
    Ok(entries)
}

fn read_zip_entry(
    src_path: &str,
    entry_path: &str,
    password: Option<&str>,
) -> Result<Vec<u8>, ArchiveError> {
    let file = std::fs::File::open(src_path)?;
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => return Err(ArchiveError::Zip(e.to_string())),
    };

    let password_bytes = password.map(|p| p.as_bytes());
    // ZIP 内部路径统一使用 /，但传入的 entry_path 可能已经统一
    let normalized_entry = entry_path.replace('\\', "/");

    // 先检查文件是否加密
    let is_encrypted = {
        let file = archive.by_name(&normalized_entry).map_err(|e| match e {
            zip::result::ZipError::FileNotFound => {
                ArchiveError::Other(format!("文件不存在: {}", entry_path))
            }
            _ => ArchiveError::Zip(e.to_string()),
        })?;
        file.encrypted()
    };

    let mut file_entry = if is_encrypted {
        if let Some(pw) = password_bytes {
            archive.by_name_decrypt(&normalized_entry, pw).map_err(|e| match e {
                zip::result::ZipError::FileNotFound => {
                    ArchiveError::Other(format!("文件不存在: {}", entry_path))
                }
                _ => ArchiveError::Zip(e.to_string()),
            })?
        } else {
            return Err(ArchiveError::PasswordRequired);
        }
    } else {
        archive.by_name(&normalized_entry).map_err(|e| match e {
            zip::result::ZipError::FileNotFound => {
                ArchiveError::Other(format!("文件不存在: {}", entry_path))
            }
            _ => ArchiveError::Zip(e.to_string()),
        })?
    };

    let mut buf = Vec::new();
    file_entry.read_to_end(&mut buf)?;
    Ok(buf)
}

// ============================================================================
// TAR 解压（含 GZ/BZ2/XZ 变体）
// ============================================================================

fn open_tar_reader(src_path: &str) -> Result<Box<dyn Read>, ArchiveError> {
    let file = std::fs::File::open(src_path)?;
    let lower = src_path.to_lowercase();

    if lower.ends_with(".tar") {
        Ok(Box::new(file))
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        Ok(Box::new(flate2::read::GzDecoder::new(file)))
    } else if lower.ends_with(".tar.bz2") {
        Ok(Box::new(bzip2::read::BzDecoder::new(file)))
    } else if lower.ends_with(".tar.xz") {
        Ok(Box::new(xz2::read::XzDecoder::new(file)))
    } else {
        Err(ArchiveError::UnsupportedFormat("TAR 变体".to_string()))
    }
}

fn extract_tar(
    src_path: &str,
    dest_dir: &str,
    file_id: i64,
    limits: &ExtractLimits,
) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let reader = open_tar_reader(src_path)?;
    let mut archive = tar::Archive::new(reader);

    let mut entries = Vec::new();
    let mut total_size: u64 = 0;

    for entry_result in archive.entries()? {
        let mut entry = entry_result?;
        let path = entry.path()?;
        let name = path.to_string_lossy().replace('\\', "/");
        let name = name.trim_start_matches('/').trim_end_matches('/').to_string();
        if name.is_empty() {
            continue;
        }
        let is_dir = entry.header().entry_type().is_dir() || name.ends_with('/');
        let name = if is_dir { name.trim_end_matches('/').to_string() } else { name };
        let size = entry.size();

        // 安全检查
        if name.contains("..") {
            log::warn!("Skipping TAR entry with path traversal: {}", name);
            continue;
        }

        // 大小限制
        if !is_dir {
            if size > limits.max_file_size {
                return Err(ArchiveError::SizeLimitExceeded {
                    max_mb: limits.max_file_size / 1_048_576,
                    actual_mb: size / 1_048_576,
                });
            }
            total_size += size;
            if total_size > limits.max_total_size {
                return Err(ArchiveError::SizeLimitExceeded {
                    max_mb: limits.max_total_size / 1_048_576,
                    actual_mb: total_size / 1_048_576,
                });
            }
        }

        if entries.len() >= limits.max_file_count {
            return Err(ArchiveError::FileCountExceeded {
                max: limits.max_file_count,
                actual: entries.len(),
            });
        }

        let out_path = Path::new(dest_dir).join(&name);

        if is_dir {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out_file = std::fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out_file)?;
        }

        entries.push(ArchiveEntry {
            id: None,
            file_id,
            entry_path: name,
            size: Some(size as i64),
            is_directory: is_dir,
        });
    }

    Ok(entries)
}

fn read_tar_entry(src_path: &str, entry_path: &str) -> Result<Vec<u8>, ArchiveError> {
    let reader = open_tar_reader(src_path)?;
    let mut archive = tar::Archive::new(reader);

    let normalized_target = entry_path.replace('\\', "/").trim_start_matches('/').trim_end_matches('/').to_string();

    for entry_result in archive.entries()? {
        let mut entry = entry_result?;
        let path = entry.path()?;
        let name = path.to_string_lossy().replace('\\', "/").trim_start_matches('/').trim_end_matches('/').to_string();

        if name == normalized_target {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }

    Err(ArchiveError::Other(format!("文件不存在: {}", entry_path)))
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 清理压缩文件解压目录
pub fn cleanup_archive_dir(archives_path: &str, file_id: i64) -> std::io::Result<()> {
    let dir = Path::new(archives_path).join(file_id.to_string());
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// 获取压缩文件条目的本地文件系统路径
pub fn resolve_archive_entry_path(
    archives_path: &str,
    file_id: i64,
    entry_path: &str,
) -> Option<std::path::PathBuf> {
    let base = Path::new(archives_path).join(file_id.to_string());
    let resolved = base.join(entry_path);

    // 安全检查：确保解析后的路径在 base 目录下
    let canonical_base = base.canonicalize().unwrap_or(base.clone());
    let canonical_resolved = match resolved.canonicalize() {
        Ok(p) => p,
        Err(_) => resolved.clone(),
    };

    if !canonical_resolved.starts_with(&canonical_base) {
        return None;
    }

    Some(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_is_archive_file() {
        assert!(is_archive_file("test.zip"));
        assert!(is_archive_file("test.ZIP"));
        assert!(is_archive_file("test.tar.gz"));
        assert!(is_archive_file("test.tgz"));
        assert!(is_archive_file("test.tar.bz2"));
        assert!(is_archive_file("test.tar.xz"));
        assert!(is_archive_file("test.7z"));
        assert!(!is_archive_file("test.pdf"));
        assert!(!is_archive_file("test.txt"));
    }

    #[test]
    fn test_extract_zip_with_backslash_paths() {
        // 创建测试 ZIP（Windows 风格路径）
        let zip_path = "/tmp/test_htknow_zip.zip";
        let extract_dir = "/tmp/test_htknow_extract";

        // 清理
        let _ = std::fs::remove_file(zip_path);
        let _ = std::fs::remove_dir_all(extract_dir);

        {
            let file = std::fs::File::create(zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("folder\\file1.txt", options).unwrap();
            zip.write_all(b"hello world 1").unwrap();
            zip.start_file("folder\\sub\\file2.txt", options).unwrap();
            zip.write_all(b"hello world 2").unwrap();
            zip.start_file("root.txt", options).unwrap();
            zip.write_all(b"hello root").unwrap();
            zip.finish().unwrap();
        }

        let entries = extract_archive(zip_path, extract_dir, None, 1).unwrap();

        // 应该解压出 3 个文件条目（目录条目会自动从路径推断）
        assert!(
            entries.len() >= 3,
            "Expected at least 3 entries, got {}: {:?}",
            entries.len(),
            entries.iter().map(|e| &e.entry_path).collect::<Vec<_>>()
        );

        // 检查路径统一使用了 /
        let paths: Vec<String> = entries.iter().map(|e| e.entry_path.clone()).collect();
        assert!(paths.iter().any(|p| p == "folder/file1.txt"), "Missing folder/file1.txt in {:?}", paths);
        assert!(paths.iter().any(|p| p == "folder/sub/file2.txt"), "Missing folder/sub/file2.txt in {:?}", paths);
        assert!(paths.iter().any(|p| p == "root.txt"), "Missing root.txt in {:?}", paths);

        // 没有路径应该包含反斜杠
        for p in &paths {
            assert!(!p.contains('\\'), "Path should not contain backslash: {}", p);
        }

        // 检查文件内容
        let content1 = std::fs::read_to_string(format!("{}/folder/file1.txt", extract_dir)).unwrap();
        assert_eq!(content1, "hello world 1");

        let content2 = std::fs::read_to_string(format!("{}/folder/sub/file2.txt", extract_dir)).unwrap();
        assert_eq!(content2, "hello world 2");

        // 清理
        let _ = std::fs::remove_file(zip_path);
        let _ = std::fs::remove_dir_all(extract_dir);
    }

    #[test]
    fn test_extract_zip_with_forward_slash_paths() {
        let zip_path = "/tmp/test_htknow_zip2.zip";
        let extract_dir = "/tmp/test_htknow_extract2";

        let _ = std::fs::remove_file(zip_path);
        let _ = std::fs::remove_dir_all(extract_dir);

        {
            let file = std::fs::File::create(zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("docs/report.pdf", options).unwrap();
            zip.write_all(b"fake pdf").unwrap();
            zip.start_file("data/info.txt", options).unwrap();
            zip.write_all(b"info").unwrap();
            zip.finish().unwrap();
        }

        let entries = extract_archive(zip_path, extract_dir, None, 2).unwrap();
        assert!(entries.len() >= 2, "Expected at least 2 entries, got {}", entries.len());

        let paths: Vec<String> = entries.iter().map(|e| e.entry_path.clone()).collect();
        assert!(paths.iter().any(|p| p == "docs/report.pdf"));
        assert!(paths.iter().any(|p| p == "data/info.txt"));

        let _ = std::fs::remove_file(zip_path);
        let _ = std::fs::remove_dir_all(extract_dir);
    }
}
