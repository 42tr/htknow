//! 7Z 流式解压：所有输出由应用写入临时目录，成功后才提交。

use std::{fs::File, io, path::Path};

use sevenz_rust2::{ArchiveReader, EncoderMethod, Error, Password};
use tempfile::NamedTempFile;

use super::{ArchiveEntry, ArchiveError, ByteBudget, ExtractLimits};

fn map_error(error: Error) -> ArchiveError {
    match error {
        Error::PasswordRequired => ArchiveError::PasswordRequired,
        Error::MaybeBadPassword(_) => ArchiveError::Other("7Z 解密失败：密码错误或压缩包损坏".into()),
        Error::Io(error, _) | Error::FileOpen(error, _) => ArchiveError::Io(error),
        Error::UnsupportedCompressionMethod(method) => ArchiveError::UnsupportedFormat(format!("7Z 压缩算法 {method}")),
        other => ArchiveError::Other(format!("7Z 解压失败: {other}")),
    }
}

fn open(src: &str, password: Option<&str>, limits: &ExtractLimits) -> Result<ArchiveReader<File>, ArchiveError> {
    let mut reader = ArchiveReader::open(src, Password::from(password.unwrap_or_default())).map_err(map_error)?;
    // 固实压缩需要顺序读取；避免每个请求启动多个解码线程。
    reader.set_thread_count(1);
    let archive = reader.archive();
    if archive.files.len() > limits.max_file_count {
        return Err(ArchiveError::FileCountExceeded { max: limits.max_file_count, actual: archive.files.len() });
    }
    if password.unwrap_or_default().is_empty()
        && archive
            .blocks
            .iter()
            .any(|block| block.coders.iter().any(|coder| coder.encoder_method_id() == EncoderMethod::ID_AES256_SHA256))
    {
        return Err(ArchiveError::PasswordRequired);
    }
    Ok(reader)
}

fn normalize_name(name: &str) -> Option<String> {
    let name = name.replace('\\', "/");
    if name.starts_with('/') || name.contains([':', '\0']) || name.split('/').any(|part| part == "..") {
        return None;
    }
    let name = name.split('/').filter(|part| !part.is_empty() && *part != ".").collect::<Vec<_>>().join("/");
    (!name.is_empty()).then_some(name)
}

fn entry_name(entry: &sevenz_rust2::ArchiveEntry) -> Option<String> {
    // 不恢复符号链接、Windows reparse point 或更新包里的删除标记。
    let kind = (entry.windows_attributes >> 16) & 0o170000;
    if entry.is_anti_item
        || (entry.has_windows_attributes
            && (entry.windows_attributes & 0x400 != 0 || !matches!(kind, 0 | 0o100000 | 0o040000)))
    {
        return None;
    }
    normalize_name(&entry.name)
}

/// 必须消费每个条目的数据，固实压缩中的后续文件依赖前面的数据。
/// 被过滤的路径也计入实际字节预算，且不会由库自动写盘。
fn visit(
    reader: &mut ArchiveReader<File>, limits: &ExtractLimits,
    mut each: impl FnMut(&sevenz_rust2::ArchiveEntry, &mut dyn io::Read, &mut ByteBudget) -> Result<(), ArchiveError>,
) -> Result<(), ArchiveError> {
    let mut budget = ByteBudget::new(limits);
    let encrypted = reader
        .archive()
        .blocks
        .iter()
        .any(|block| block.coders.iter().any(|coder| coder.encoder_method_id() == EncoderMethod::ID_AES256_SHA256));
    let mut callback_error = None;
    let result = reader.for_each_entries(|entry, data| {
        let result = budget.check_declared(entry.size).and_then(|_| each(entry, data, &mut budget));
        if let Err(error) = result {
            callback_error = Some(match error {
                ArchiveError::Io(ref io_error)
                    if encrypted
                        && matches!(
                            io_error.kind(),
                            io::ErrorKind::InvalidData
                                | io::ErrorKind::InvalidInput
                                | io::ErrorKind::UnexpectedEof
                                | io::ErrorKind::Other
                        ) =>
                {
                    ArchiveError::Other("7Z 解密失败：密码错误或压缩包损坏".into())
                }
                other => other,
            });
            // false 仅停止当前固实块；返回错误才能立即停止整个压缩包。
            return Err(Error::Other("停止解压".into()));
        }
        Ok(true)
    });
    if let Some(error) = callback_error {
        return Err(error);
    }
    result.map_err(map_error)
}

fn copy(
    entry: &sevenz_rust2::ArchiveEntry, mut data: &mut dyn io::Read, budget: &mut ByteBudget,
    output: &mut impl io::Write,
) -> Result<u64, ArchiveError> {
    let size = budget.copy_from(&mut data, output)?;
    if size != entry.size {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "7Z 文件损坏：解压后的大小与文件头不符").into());
    }
    Ok(size)
}

pub(super) fn extract(
    src: &str, dest: &str, password: Option<&str>, file_id: i64, limits: &ExtractLimits,
) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let mut reader = open(src, password, limits)?;
    let staging = tempfile::tempdir_in(dest)?;
    let mut entries = Vec::new();
    visit(&mut reader, limits, |entry, data, budget| {
        let Some(name) = entry_name(entry) else {
            copy(entry, data, budget, &mut io::sink())?;
            return Ok(());
        };
        let output = staging.path().join(&name);
        let size = if entry.is_directory {
            copy(entry, data, budget, &mut io::sink())?;
            std::fs::create_dir_all(output)?;
            0
        } else {
            std::fs::create_dir_all(output.parent().unwrap())?;
            copy(entry, data, budget, &mut File::create(output)?)?
        };
        entries.push(ArchiveEntry {
            id: None,
            file_id,
            entry_path: name,
            size: Some(size as i64),
            is_directory: entry.is_directory,
        });
        Ok(())
    })?;
    for entry in std::fs::read_dir(staging.path())? {
        let entry = entry?;
        std::fs::rename(entry.path(), Path::new(dest).join(entry.file_name()))?;
    }
    Ok(entries)
}

pub(super) fn read_entry(src: &str, target: &str, password: Option<&str>) -> Result<NamedTempFile, ArchiveError> {
    let target = normalize_name(target).ok_or_else(|| ArchiveError::Other("无效的压缩包内路径".into()))?;
    let limits = ExtractLimits::default();
    let mut reader = open(src, password, &limits)?;
    if !reader.archive().files.iter().any(|entry| !entry.is_directory && entry_name(entry).as_deref() == Some(&target))
    {
        return Err(ArchiveError::Other(format!("文件不存在: {target}")));
    }
    let mut temp = NamedTempFile::new()?;
    visit(&mut reader, &limits, |entry, data, budget| {
        if !entry.is_directory && entry_name(entry).as_deref() == Some(&target) {
            // 与完整解压一致，重复路径以最后一个条目为准。
            temp.as_file_mut().set_len(0)?;
            std::io::Seek::rewind(temp.as_file_mut())?;
            copy(entry, data, budget, &mut temp)?;
        } else {
            copy(entry, data, budget, &mut io::sink())?;
        }
        Ok(())
    })?;
    Ok(temp)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Seek, SeekFrom, Write};

    use sevenz_rust2::{ArchiveWriter, EncoderConfiguration, SourceReader, encoder_options::AesEncoderOptions};

    use super::*;

    fn make_archive(
        files: &[(&str, &[u8])], solid: bool, password: Option<&str>, encrypt_header: bool,
    ) -> NamedTempFile {
        let temp = NamedTempFile::new().unwrap();
        let mut writer = ArchiveWriter::new(temp.reopen().unwrap()).unwrap();
        writer.set_encrypt_header(encrypt_header);
        if let Some(password) = password {
            writer.set_content_methods(vec![
                AesEncoderOptions::new(password.into()).into(),
                EncoderConfiguration::new(EncoderMethod::LZMA2),
            ]);
        }
        if solid {
            let entries = files.iter().map(|(name, _)| sevenz_rust2::ArchiveEntry::new_file(name)).collect();
            let readers = files.iter().map(|(_, data)| SourceReader::new(Cursor::new(*data))).collect();
            writer.push_archive_entries(entries, readers).unwrap();
        } else {
            writer
                .push_archive_entry(sevenz_rust2::ArchiveEntry::new_directory("空目录"), None::<Cursor<&[u8]>>)
                .unwrap();
            for (name, data) in files {
                writer
                    .push_archive_entry(sevenz_rust2::ArchiveEntry::new_file(name), Some(Cursor::new(*data)))
                    .unwrap();
            }
        }
        writer.finish().unwrap();
        temp
    }

    fn unpack(
        src: &Path, password: Option<&str>, limits: &ExtractLimits,
    ) -> (tempfile::TempDir, Result<Vec<ArchiveEntry>, ArchiveError>) {
        let dest = tempfile::tempdir().unwrap();
        let result = super::super::extract_archive_with_limits(
            src.to_str().unwrap(),
            dest.path().to_str().unwrap(),
            "文件.7Z",
            password,
            42,
            limits,
        );
        (dest, result)
    }

    fn download(src: &Path, name: &str, password: Option<&str>) -> Result<Vec<u8>, ArchiveError> {
        let file = super::super::read_archive_entry(src.to_str().unwrap(), "文件.7z", name, password)?;
        Ok(std::fs::read(file.path())?)
    }

    #[test]
    fn sevenz_extracts_unicode_directories_empty_files_and_windows_paths() {
        let src = make_archive(&[("资料\\说明.txt", "中文内容".as_bytes()), ("empty.txt", b"")], false, None, false);
        let (dest, result) = unpack(src.path(), None, &ExtractLimits::default());
        let entries = result.unwrap();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|entry| entry.file_id == 42));
        assert!(entries.iter().any(|entry| entry.is_directory && entry.entry_path == "空目录"));
        assert_eq!(std::fs::read_to_string(dest.path().join("资料/说明.txt")).unwrap(), "中文内容");
        assert_eq!(download(src.path(), "资料/说明.txt", None).unwrap(), "中文内容".as_bytes());
        assert!(download(src.path(), "empty.txt", None).unwrap().is_empty());
        assert!(download(src.path(), "missing.txt", None).is_err());
    }

    #[test]
    fn sevenz_reads_later_solid_entries() {
        let src = make_archive(&[("first.txt", b"first"), ("nested/second.txt", b"second")], true, None, false);
        let (dest, result) = unpack(src.path(), None, &ExtractLimits::default());
        assert_eq!(result.unwrap().len(), 2);
        assert_eq!(std::fs::read(dest.path().join("nested/second.txt")).unwrap(), b"second");
        assert_eq!(download(src.path(), "nested/second.txt", None).unwrap(), b"second");
    }

    #[test]
    fn sevenz_prompts_for_data_and_header_passwords_and_allows_retry() {
        for encrypt_header in [false, true] {
            let src = make_archive(&[("secret.txt", b"private data")], true, Some("密码🔒"), encrypt_header);
            let (dest, result) = unpack(src.path(), None, &ExtractLimits::default());
            assert!(matches!(result, Err(ArchiveError::PasswordRequired)), "{result:?}");
            assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
            assert!(matches!(download(src.path(), "secret.txt", None), Err(ArchiveError::PasswordRequired)));
            let (dest, result) = unpack(src.path(), Some("wrong"), &ExtractLimits::default());
            let error = result.unwrap_err();
            assert!(error.to_string().contains("密码"), "encrypted_header={encrypt_header}: {error:?}");
            assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
            let (dest, result) = unpack(src.path(), Some("密码🔒"), &ExtractLimits::default());
            assert_eq!(result.unwrap().len(), 1);
            assert_eq!(std::fs::read(dest.path().join("secret.txt")).unwrap(), b"private data");
            assert_eq!(download(src.path(), "secret.txt", Some("密码🔒")).unwrap(), b"private data");
        }
    }

    #[test]
    fn sevenz_enforces_file_total_and_count_limits_without_partial_output() {
        let src = make_archive(&[("one.txt", b"123456"), ("two.txt", b"123456")], true, None, false);
        for limits in [
            ExtractLimits { max_file_size: 5, ..Default::default() },
            ExtractLimits { max_total_size: 10, ..Default::default() },
            ExtractLimits { max_file_count: 1, ..Default::default() },
        ] {
            let (dest, result) = unpack(src.path(), None, &limits);
            assert!(
                matches!(result, Err(ArchiveError::SizeLimitExceeded { .. } | ArchiveError::FileCountExceeded { .. })),
                "{result:?}"
            );
            assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn sevenz_filters_unsafe_solid_paths_and_still_decodes_later_files() {
        let src = make_archive(
            &[("../escape.txt", b"bad"), ("/absolute.txt", b"bad"), ("C:\\drive.txt", b"bad"), ("safe.txt", b"safe")],
            true,
            None,
            false,
        );
        let (dest, result) = unpack(src.path(), None, &ExtractLimits::default());
        let entries = result.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry_path, "safe.txt");
        assert_eq!(std::fs::read(dest.path().join("safe.txt")).unwrap(), b"safe");
        assert_eq!(download(src.path(), "safe.txt", None).unwrap(), b"safe");
        assert!(download(src.path(), "../escape.txt", None).is_err());
        let (_, result) = unpack(src.path(), None, &ExtractLimits { max_total_size: 8, ..Default::default() });
        assert!(matches!(result, Err(ArchiveError::SizeLimitExceeded { .. })));
    }

    #[test]
    fn sevenz_rejects_links_and_anti_items() {
        let mut entry = sevenz_rust2::ArchiveEntry::new_file("link");
        entry.has_windows_attributes = true;
        entry.windows_attributes = 0o120777 << 16;
        assert!(entry_name(&entry).is_none());
        let src = NamedTempFile::new().unwrap();
        let mut writer = ArchiveWriter::new(src.reopen().unwrap()).unwrap();
        writer.push_archive_entry(entry.clone(), Some(Cursor::new(b"../escape"))).unwrap();
        writer
            .push_archive_entry(sevenz_rust2::ArchiveEntry::new_file("safe.txt"), Some(Cursor::new(b"safe")))
            .unwrap();
        writer.finish().unwrap();
        let (dest, result) = unpack(src.path(), None, &ExtractLimits::default());
        assert_eq!(result.unwrap().len(), 1);
        assert!(!dest.path().join("link").exists());
        assert_eq!(download(src.path(), "safe.txt", None).unwrap(), b"safe");
        assert!(download(src.path(), "link", None).is_err());
        entry.windows_attributes = 0x400;
        assert!(entry_name(&entry).is_none());
        entry.windows_attributes = 0o100644 << 16;
        assert_eq!(entry_name(&entry).as_deref(), Some("link"));
        entry.is_anti_item = true;
        assert!(entry_name(&entry).is_none());
    }

    #[test]
    fn sevenz_checks_actual_bytes_when_declared_size_is_wrong() {
        let entry = sevenz_rust2::ArchiveEntry { size: 1, ..Default::default() };
        let limits = ExtractLimits { max_file_size: 3, ..Default::default() };
        let error =
            copy(&entry, &mut Cursor::new(b"too large"), &mut ByteBudget::new(&limits), &mut io::sink()).unwrap_err();
        assert!(matches!(error, ArchiveError::SizeLimitExceeded { .. }));
    }

    #[test]
    fn sevenz_rejects_corruption_and_cleans_staging() {
        let mut src = make_archive(&[("first.txt", b"first"), ("second.txt", b"second")], true, None, false);
        src.seek(SeekFrom::Start(35)).unwrap();
        let mut byte = [0];
        src.read_exact(&mut byte).unwrap();
        src.seek(SeekFrom::Start(35)).unwrap();
        src.write_all(&[byte[0] ^ 0xff]).unwrap();
        src.flush().unwrap();
        let (dest, result) = unpack(src.path(), None, &ExtractLimits::default());
        assert!(result.is_err());
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
        assert!(download(src.path(), "second.txt", None).is_err());
    }

    #[test]
    fn sevenz_extracts_independent_upstream_fixtures() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/7z");
        for (name, password) in [
            ("single_file_with_content_lzma.7z", None),
            ("solid.7z", None),
            ("bzip2_file.7z", None),
            ("ppmd.7z", None),
            ("encrypted.7z", Some("sevenz-rust")),
        ] {
            let src = root.join(name);
            let (dest, result) = unpack(&src, password, &ExtractLimits::default());
            let entries = result.unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(!entries.is_empty());
            for entry in entries.iter().filter(|entry| !entry.is_directory) {
                let expected = std::fs::read(dest.path().join(&entry.entry_path)).unwrap();
                assert_eq!(expected.len() as i64, entry.size.unwrap());
                assert_eq!(download(&src, &entry.entry_path, password).unwrap(), expected);
            }
        }
    }
}
