//! 压缩文件解压模块
//!
//! 支持 ZIP、TAR（及其 GZ/BZ2/XZ 变体）格式的解压。
//! 7Z 和 RAR 格式暂不支持。

use std::{
    io::{Read, Write},
    path::Path,
};

use encoding_rs::GB18030;
use serde::Serialize;
use tempfile::NamedTempFile;
use utoipa::ToSchema;

/// 智能解码文件名：先尝试 UTF-8，发现乱码时回退到 GB18030（兼容 GBK/GB2312）
fn decode_filename(raw: &[u8]) -> String {
    // 先尝试 UTF-8
    if let Ok(utf8) = std::str::from_utf8(raw) {
        // 如果 UTF-8 解码成功且不包含替换字符，直接使用
        if !utf8.contains('\u{FFFD}') {
            return utf8.to_string();
        }
    }

    // 尝试 GB18030（兼容 GBK/GB2312）
    let (decoded, _, had_errors) = GB18030.decode(raw);
    if !had_errors {
        return decoded.to_string();
    }

    // 回退：UTF-8 lossy
    String::from_utf8_lossy(raw).to_string()
}

/// 判断原始文件名是否表示目录（以 / 或 \ 结尾）
fn is_dir_from_raw(raw: &[u8]) -> bool {
    raw.ends_with(b"/") || raw.ends_with(b"\\")
}

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

const MIB: u64 = 1_048_576;

/// 解压字节预算：按**实际读到的字节数**计费。
///
/// 压缩包条目头部里的 `size()` 只是声明值，可以被伪造——zip bomb 会声明一个很小的体积，
/// 实际解压却膨胀到数 GB。因此限额必须在拷贝过程中按真实字节数判断；原先的
/// `std::io::copy` 是无界拷贝，声明值一旦说谎就能把磁盘写满。
struct ByteBudget {
    max_file_size: u64,
    max_total_size: u64,
    written: u64,
}

impl ByteBudget {
    fn new(limits: &ExtractLimits) -> Self {
        Self { max_file_size: limits.max_file_size, max_total_size: limits.max_total_size, written: 0 }
    }

    /// 单条目读取（压缩包内下载）：只受单文件上限约束。
    fn single_entry(limits: &ExtractLimits) -> Self {
        Self { max_file_size: limits.max_file_size, max_total_size: limits.max_file_size, written: 0 }
    }

    /// 基于头部声明值的快速预检：能在解压前拒绝就提前拒绝，但不是唯一依据。
    fn check_declared(&self, declared: u64) -> Result<(), ArchiveError> {
        if declared > self.max_file_size {
            return Err(ArchiveError::SizeLimitExceeded {
                max_mb: self.max_file_size / MIB,
                actual_mb: declared / MIB,
            });
        }
        if self.written + declared > self.max_total_size {
            return Err(ArchiveError::SizeLimitExceeded {
                max_mb: self.max_total_size / MIB,
                actual_mb: (self.written + declared) / MIB,
            });
        }
        Ok(())
    }

    /// 限量拷贝，返回实际写入的字节数。任一上限被突破立即报错（已写出的内容由调用方清理）。
    fn copy_from<R: Read, W: Write>(&mut self, reader: &mut R, writer: &mut W) -> Result<u64, ArchiveError> {
        let mut buffer = vec![0u8; 64 * 1024];
        let mut written = 0u64;
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            written += read as u64;
            if written > self.max_file_size {
                return Err(ArchiveError::SizeLimitExceeded {
                    max_mb: self.max_file_size / MIB,
                    actual_mb: written / MIB,
                });
            }
            if self.written + written > self.max_total_size {
                return Err(ArchiveError::SizeLimitExceeded {
                    max_mb: self.max_total_size / MIB,
                    actual_mb: (self.written + written) / MIB,
                });
            }
            writer.write_all(&buffer[..read])?;
        }
        writer.flush()?;
        self.written += written;
        Ok(written)
    }
}

/// 支持的压缩格式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    SevenZ,
    Tar,
    TarGz,
    Tgz,
    TarBz2,
    TarXz,
    Unknown,
}

impl ArchiveFormat {
    /// 根据文件名识别压缩格式（大小写不敏感）
    pub fn from_filename(filename: &str) -> Self {
        let lower = filename.to_lowercase();
        if lower.ends_with(".zip") {
            ArchiveFormat::Zip
        } else if lower.ends_with(".7z") {
            ArchiveFormat::SevenZ
        } else if lower.ends_with(".tar.gz") {
            ArchiveFormat::TarGz
        } else if lower.ends_with(".tgz") {
            ArchiveFormat::Tgz
        } else if lower.ends_with(".tar.bz2") {
            ArchiveFormat::TarBz2
        } else if lower.ends_with(".tar.xz") {
            ArchiveFormat::TarXz
        } else if lower.ends_with(".tar") {
            ArchiveFormat::Tar
        } else {
            ArchiveFormat::Unknown
        }
    }

    /// 是否为已识别的压缩格式（含暂不解压的 7Z）
    pub fn is_archive(self) -> bool {
        !matches!(self, ArchiveFormat::Unknown)
    }

    /// 当前是否可直接解压
    pub fn is_supported(self) -> bool {
        matches!(self, ArchiveFormat::Zip) || self.is_tar_variant()
    }

    /// 是否为 tar 变体
    pub fn is_tar_variant(self) -> bool {
        matches!(
            self,
            ArchiveFormat::Tar
                | ArchiveFormat::TarGz
                | ArchiveFormat::Tgz
                | ArchiveFormat::TarBz2
                | ArchiveFormat::TarXz
        )
    }

    /// 格式描述文本
    pub fn desc(self) -> &'static str {
        match self {
            ArchiveFormat::Zip => "ZIP",
            ArchiveFormat::SevenZ => "7Z",
            ArchiveFormat::Tar => "TAR",
            ArchiveFormat::TarGz | ArchiveFormat::Tgz => "TAR.GZ",
            ArchiveFormat::TarBz2 => "TAR.BZ2",
            ArchiveFormat::TarXz => "TAR.XZ",
            ArchiveFormat::Unknown => "未知",
        }
    }
}

/// 判断文件是否为支持的压缩格式
pub fn is_archive_file(filename: &str) -> bool {
    ArchiveFormat::from_filename(filename).is_archive()
}

/// 将压缩文件解压到指定目录
///
/// `filename` 为原始文件名（含扩展名），用于判断压缩格式。
/// 返回解压后的文件列表
pub fn extract_archive(
    src_path: &str, dest_dir: &str, filename: &str, password: Option<&str>, file_id: i64,
) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let limits = ExtractLimits::default();
    extract_archive_with_limits(src_path, dest_dir, filename, password, file_id, &limits)
}

/// 带限制的解压
pub fn extract_archive_with_limits(
    src_path: &str, dest_dir: &str, filename: &str, password: Option<&str>, file_id: i64, limits: &ExtractLimits,
) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let format = ArchiveFormat::from_filename(filename);
    if !format.is_archive() {
        return Err(ArchiveError::UnsupportedFormat(format.desc().to_string()));
    }

    // 创建目标目录
    std::fs::create_dir_all(dest_dir)?;

    match format {
        ArchiveFormat::Zip => extract_zip(src_path, dest_dir, password, file_id, limits),
        ArchiveFormat::SevenZ => Err(ArchiveError::UnsupportedFormat("7Z 格式暂不支持".to_string())),
        _ if format.is_tar_variant() => extract_tar(src_path, dest_dir, file_id, limits),
        _ => Err(ArchiveError::UnsupportedFormat(format.desc().to_string())),
    }
}

/// 读取压缩包内单个文件内容到临时文件
///
/// `filename` 为原始文件名（含扩展名），用于判断压缩格式。
/// 返回的 `NamedTempFile` 需要由调用方在合适时机消费/关闭。
pub fn read_archive_entry(
    src_path: &str, filename: &str, entry_path: &str, password: Option<&str>,
) -> Result<NamedTempFile, ArchiveError> {
    let format = ArchiveFormat::from_filename(filename);
    match format {
        ArchiveFormat::Zip => read_zip_entry(src_path, entry_path, password),
        ArchiveFormat::SevenZ => Err(ArchiveError::UnsupportedFormat("7Z 格式暂不支持".to_string())),
        _ if format.is_tar_variant() => read_tar_entry(src_path, entry_path),
        _ => Err(ArchiveError::UnsupportedFormat(format.desc().to_string())),
    }
}

// ============================================================================
// ZIP 解压
// ============================================================================

fn extract_zip(
    src_path: &str, dest_dir: &str, password: Option<&str>, file_id: i64, limits: &ExtractLimits,
) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let file = std::fs::File::open(src_path)?;
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => return Err(ArchiveError::Zip(e.to_string())),
    };

    let mut entries = Vec::new();
    let mut budget = ByteBudget::new(limits);
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

        let raw_bytes = file_entry.name_raw();
        let raw_name = decode_filename(raw_bytes);
        // 统一路径分隔符为 /，并处理连续分隔符和首尾分隔符
        let name: String =
            raw_name.replace('\\', "/").split('/').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/");
        if name.is_empty() {
            log::info!(
                "ZIP entry {}: raw={:?} utf8={:?} -> skipped (empty after normalization)",
                i,
                raw_bytes,
                raw_name
            );
            continue;
        }
        let is_dir = is_dir_from_raw(raw_bytes);
        let name = if is_dir && name.ends_with('/') { name.trim_end_matches('/').to_string() } else { name };
        let size = file_entry.size();

        log::info!(
            "ZIP entry {}: raw={:?} decoded={:?} -> normalized={}, is_dir={}, size={}",
            i,
            raw_bytes,
            raw_name,
            name,
            is_dir,
            size
        );

        // 安全检查：路径遍历
        if name.contains("..") {
            log::warn!("Skipping ZIP entry with path traversal: {}", name);
            continue;
        }

        // 大小限制检查（声明值预检；真实体积在拷贝时再按字节数校验一次）
        if !is_dir {
            budget.check_declared(size)?;
        }

        // 数量限制检查
        if entries.len() >= limits.max_file_count {
            return Err(ArchiveError::FileCountExceeded { max: limits.max_file_count, actual: entries.len() });
        }

        let out_path = Path::new(dest_dir).join(&name);

        let actual_size = if is_dir {
            std::fs::create_dir_all(&out_path)?;
            0
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out_file = std::fs::File::create(&out_path)?;
            budget.copy_from(&mut file_entry, &mut out_file)?
        };

        entries.push(ArchiveEntry {
            id: None,
            file_id,
            entry_path: name,
            size: Some(actual_size as i64),
            is_directory: is_dir,
        });
    }

    log::info!("ZIP extraction completed: {} entries, file_id={}", entries.len(), file_id);
    Ok(entries)
}

fn read_zip_entry(src_path: &str, entry_path: &str, password: Option<&str>) -> Result<NamedTempFile, ArchiveError> {
    let file = std::fs::File::open(src_path)?;
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => return Err(ArchiveError::Zip(e.to_string())),
    };

    let password_bytes = password.map(|p| p.as_bytes());
    let normalized_target = entry_path.replace('\\', "/").trim_start_matches('/').trim_end_matches('/').to_string();

    // 遍历所有条目，用解码后的文件名匹配（处理 GBK 编码中文文件名）
    for i in 0..archive.len() {
        let file_ref = archive.by_index(i).map_err(|e| ArchiveError::Zip(e.to_string()))?;
        let decoded_name = decode_filename(file_ref.name_raw());
        let normalized_name = decoded_name.replace('\\', "/").trim_start_matches('/').trim_end_matches('/').to_string();

        if normalized_name == normalized_target {
            let is_encrypted = file_ref.encrypted();
            drop(file_ref);

            let mut file_entry = if is_encrypted {
                if let Some(pw) = password_bytes {
                    archive.by_index_decrypt(i, pw).map_err(|e| ArchiveError::Zip(e.to_string()))?
                } else {
                    return Err(ArchiveError::PasswordRequired);
                }
            } else {
                archive.by_index(i).map_err(|e| ArchiveError::Zip(e.to_string()))?
            };

            let mut temp = NamedTempFile::new()?;
            ByteBudget::single_entry(&ExtractLimits::default()).copy_from(&mut file_entry, &mut temp)?;
            temp.flush()?;
            return Ok(temp);
        }
    }

    Err(ArchiveError::Other(format!("文件不存在: {}", entry_path)))
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
    src_path: &str, dest_dir: &str, file_id: i64, limits: &ExtractLimits,
) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let reader = open_tar_reader(src_path)?;
    let mut archive = tar::Archive::new(reader);

    let mut entries = Vec::new();
    let mut budget = ByteBudget::new(limits);

    for entry_result in archive.entries()? {
        let mut entry = entry_result?;
        let raw_path = entry.path_bytes();
        let decoded = decode_filename(&raw_path);
        let name = decoded.replace('\\', "/").trim_start_matches('/').trim_end_matches('/').to_string();
        if name.is_empty() {
            continue;
        }
        let is_dir = entry.header().entry_type().is_dir() || decoded.ends_with('/') || decoded.ends_with('\\');
        let name = if is_dir && name.ends_with('/') { name.trim_end_matches('/').to_string() } else { name };
        let size = entry.size();

        // 安全检查
        if name.contains("..") {
            log::warn!("Skipping TAR entry with path traversal: {}", name);
            continue;
        }

        // 大小限制（声明值预检；真实体积在拷贝时再按字节数校验一次）
        if !is_dir {
            budget.check_declared(size)?;
        }

        if entries.len() >= limits.max_file_count {
            return Err(ArchiveError::FileCountExceeded { max: limits.max_file_count, actual: entries.len() });
        }

        let out_path = Path::new(dest_dir).join(&name);

        let actual_size = if is_dir {
            std::fs::create_dir_all(&out_path)?;
            0
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out_file = std::fs::File::create(&out_path)?;
            budget.copy_from(&mut entry, &mut out_file)?
        };

        entries.push(ArchiveEntry {
            id: None,
            file_id,
            entry_path: name,
            size: Some(actual_size as i64),
            is_directory: is_dir,
        });
    }

    Ok(entries)
}

fn read_tar_entry(src_path: &str, entry_path: &str) -> Result<NamedTempFile, ArchiveError> {
    let reader = open_tar_reader(src_path)?;
    let mut archive = tar::Archive::new(reader);

    let normalized_target = entry_path.replace('\\', "/").trim_start_matches('/').trim_end_matches('/').to_string();

    for entry_result in archive.entries()? {
        let mut entry = entry_result?;
        let raw_path = entry.path_bytes();
        let decoded = decode_filename(&raw_path);
        let name = decoded.replace('\\', "/").trim_start_matches('/').trim_end_matches('/').to_string();

        if name == normalized_target {
            let mut temp = NamedTempFile::new()?;
            ByteBudget::single_entry(&ExtractLimits::default()).copy_from(&mut entry, &mut temp)?;
            temp.flush()?;
            return Ok(temp);
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
pub fn resolve_archive_entry_path(archives_path: &str, file_id: i64, entry_path: &str) -> Option<std::path::PathBuf> {
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
    use std::io::Write;

    use super::*;

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

        let entries = extract_archive(zip_path, extract_dir, "test.zip", None, 1).unwrap();

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

        let entries = extract_archive(zip_path, extract_dir, "test.zip", None, 2).unwrap();
        assert!(entries.len() >= 2, "Expected at least 2 entries, got {}", entries.len());

        let paths: Vec<String> = entries.iter().map(|e| e.entry_path.clone()).collect();
        assert!(paths.iter().any(|p| p == "docs/report.pdf"));
        assert!(paths.iter().any(|p| p == "data/info.txt"));

        let _ = std::fs::remove_file(zip_path);
        let _ = std::fs::remove_dir_all(extract_dir);
    }
}
