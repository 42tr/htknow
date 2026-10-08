//! UnRAR 的 TEST 模式仅通过回调输出字节；路径、写盘和大小限制由 Rust 控制。
//! 不使用原生库的 EXTRACT 模式，避免它创建链接或自行选择输出路径。

use std::{
    ffi::CString,
    fs::File,
    io::Write,
    path::Path,
    ptr,
    sync::{Mutex, MutexGuard},
};

use tempfile::NamedTempFile;
use unrar_sys as native;

use super::{ArchiveEntry, ArchiveError, ByteBudget, ExtractLimits, MIB, decode_filename};

// UnRAR DLL 使用单字节结构体对齐；unrar_sys 0.5.8 的 HeaderDataEx 未声明 packed。
// 保留其零初始化的 Reserved 区，供新版 DLL 的扩展字段使用。
#[repr(C, packed)]
struct PackedHeader {
    archive_name: [std::ffi::c_char; 1024],
    archive_name_w: [native::WCHAR; 1024],
    filename: [std::ffi::c_char; 1024],
    filename_w: [native::WCHAR; 1024],
    flags: u32,
    pack_size: u32,
    pack_size_high: u32,
    unp_size: u32,
    unp_size_high: u32,
    host_os: u32,
    file_crc: u32,
    file_time: u32,
    unp_ver: u32,
    method: u32,
    file_attr: u32,
    comment_buffer: *mut std::ffi::c_char,
    comment_buffer_size: u32,
    comment_size: u32,
    comment_state: u32,
    dict_size: u32,
    hash_type: u32,
    hash: [std::ffi::c_char; 32],
    redir_type: u32,
    redir_name: *mut native::WCHAR,
    redir_name_size: u32,
    dir_target: u32,
    mtime_low: u32,
    mtime_high: u32,
    ctime_low: u32,
    ctime_high: u32,
    atime_low: u32,
    atime_high: u32,
    reserved: [u32; 988],
}
impl Default for PackedHeader {
    fn default() -> Self {
        // SAFETY: 该 FFI 输出结构只包含整数、数组和可空原始指针，零值均有效。
        unsafe { std::mem::zeroed() }
    }
}

// DLL 使用全局 ErrorHandler；每个句柄全程持锁，避免并发调用污染原生状态。
static RAR_LOCK: Mutex<()> = Mutex::new(());

#[repr(C, packed)]
struct PackedOpenOptions {
    archive_name: *const std::ffi::c_char,
    archive_name_w: *const native::WCHAR,
    open_mode: u32,
    open_result: u32,
    comment_buffer: *mut std::ffi::c_char,
    comment_buffer_size: u32,
    comment_size: u32,
    comment_state: u32,
    flags: u32,
    callback: Option<native::Callback>,
    user_data: native::LPARAM,
    op_flags: u32,
    comment_buffer_w: *mut native::WCHAR,
    reserved: [u32; 25],
}
impl Default for PackedOpenOptions {
    fn default() -> Self {
        // SAFETY: 全零初始化整数、可空原始指针及可空函数指针。
        unsafe { std::mem::zeroed() }
    }
}

struct CallbackState {
    password: Option<CString>,
    password_w: Vec<native::WCHAR>,
    output: Option<File>,
    budget: ByteBudget,
    file_size: u64,
    error: Option<ArchiveError>,
}

impl CallbackState {
    fn write(&mut self, data: &[u8]) -> Result<(), ArchiveError> {
        let file_size = self.file_size.saturating_add(data.len() as u64);
        let total_size = self.budget.written.saturating_add(data.len() as u64);
        if file_size > self.budget.max_file_size {
            return Err(ArchiveError::SizeLimitExceeded {
                max_mb: self.budget.max_file_size / MIB,
                actual_mb: file_size / MIB,
            });
        }
        if total_size > self.budget.max_total_size {
            return Err(ArchiveError::SizeLimitExceeded {
                max_mb: self.budget.max_total_size / MIB,
                actual_mb: total_size / MIB,
            });
        }
        if let Some(output) = self.output.as_mut() {
            output.write_all(data)?;
        }
        self.file_size = file_size;
        self.budget.written = total_size;
        Ok(())
    }
}

extern "C" fn callback(msg: native::UINT, user: native::LPARAM, p1: native::LPARAM, p2: native::LPARAM) -> i32 {
    if user == 0 {
        return -1;
    }
    // SAFETY: 每个同步的 UnRAR 调用都使用仍存活且地址稳定的 Box<CallbackState>。
    // 原生库不会异步调用回调，也不会并行写入该状态。
    let state = unsafe { &mut *(user as *mut CallbackState) };
    if state.error.is_some() {
        return -1;
    }
    let result = match msg {
        native::UCM_PROCESSDATA if p1 != 0 && p2 > 0 => {
            // SAFETY: UCM_PROCESSDATA 的 p1/p2 是原生库提供的有效字节缓冲区及长度，
            // 仅在当前回调期间借用。
            let data = unsafe { std::slice::from_raw_parts(p1 as *const u8, p2 as usize) };
            state.write(data)
        }
        native::UCM_NEEDPASSWORD | native::UCM_NEEDPASSWORDW => {
            if let Some(password) = &state.password {
                if msg == native::UCM_NEEDPASSWORD {
                    let bytes = password.as_bytes_with_nul();
                    if p1 == 0 || p2 <= 0 || bytes.len() > p2 as usize {
                        Err(ArchiveError::InvalidPassword)
                    } else {
                        // SAFETY: 密码回调提供容量为 p2 的可写缓冲区，已检查含 NUL 的长度。
                        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), p1 as *mut u8, bytes.len()) };
                        Ok(())
                    }
                } else if p1 == 0 || p2 <= 0 || state.password_w.len() > p2 as usize {
                    Err(ArchiveError::InvalidPassword)
                } else {
                    // SAFETY: 同上，宽字符密码缓冲区的容量以 WCHAR 为单位。
                    unsafe {
                        ptr::copy_nonoverlapping(
                            state.password_w.as_ptr(),
                            p1 as *mut native::WCHAR,
                            state.password_w.len(),
                        )
                    };
                    Ok(())
                }
            } else {
                Err(ArchiveError::PasswordRequired)
            }
        }
        native::UCM_CHANGEVOLUME | native::UCM_CHANGEVOLUMEW => {
            // 上传接口只接收单个压缩包，不允许原生库尝试读取旁边的其他文件。
            Err(ArchiveError::UnsupportedFormat("RAR 分卷压缩包暂不支持，请上传完整的单卷文件".into()))
        }
        _ => Ok(()),
    };
    match result {
        Ok(()) => 0,
        Err(error) => {
            state.error = Some(error);
            -1
        }
    }
}

fn native_error(code: i32) -> ArchiveError {
    match code {
        native::ERAR_MISSING_PASSWORD => ArchiveError::PasswordRequired,
        native::ERAR_BAD_PASSWORD => ArchiveError::InvalidPassword,
        _ => ArchiveError::Other(format!("RAR 解压错误（错误码 {code}）：文件损坏或格式不受支持")),
    }
}

struct RarArchive {
    handle: *const native::Handle,
    state: Box<CallbackState>,
    encrypted: bool,
    _guard: MutexGuard<'static, ()>,
}

impl Drop for RarArchive {
    fn drop(&mut self) {
        // SAFETY: handle 来自成功的打开操作，仅在此关闭一次；回调状态此时仍存活。
        unsafe { native::RARCloseArchive(self.handle) };
    }
}

impl RarArchive {
    fn open(src: &str, password: Option<&str>, limits: &ExtractLimits) -> Result<Self, ArchiveError> {
        let guard = RAR_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let path = CString::new(src).map_err(|_| ArchiveError::Other("RAR 文件路径包含 NUL".into()))?;
        let password_c = password.map(CString::new).transpose().map_err(|_| ArchiveError::InvalidPassword)?;
        #[cfg(windows)]
        let password_w = password.unwrap_or_default().encode_utf16().map(|c| c as native::WCHAR).chain([0]).collect();
        #[cfg(not(windows))]
        let password_w = password.unwrap_or_default().chars().map(|c| c as native::WCHAR).chain([0]).collect();
        let mut state = Box::new(CallbackState {
            password: password_c,
            password_w,
            output: None,
            budget: ByteBudget::new(limits),
            file_size: 0,
            error: None,
        });
        let mut options = PackedOpenOptions::default();
        options.open_mode = native::RAR_OM_EXTRACT;
        options.archive_name = path.as_ptr();
        options.callback = Some(callback);
        options.user_data = (&mut *state as *mut CallbackState) as native::LPARAM;
        // SAFETY: options、路径和回调状态均在调用期间存活。回调也处理打开时的头部密码请求。
        let handle = unsafe { native::RAROpenArchiveEx(ptr::from_mut(&mut options).cast()) };
        if handle.is_null() {
            return Err(state.error.take().unwrap_or_else(|| native_error(options.open_result as i32)));
        }
        let mut archive =
            Self { handle, state, encrypted: options.flags & native::ROADF_ENCHEADERS != 0, _guard: guard };
        archive.check(options.open_result as i32)?;
        if options.flags & native::ROADF_VOLUME != 0 {
            return Err(ArchiveError::UnsupportedFormat("RAR 分卷压缩包暂不支持，请上传完整的单卷文件".into()));
        }
        // 文件数据密码也通过宽字符回调提供，避免 ANSI 接口受系统 locale 影响。
        Ok(archive)
    }

    fn check(&mut self, code: i32) -> Result<(), ArchiveError> {
        if let Some(error) = self.state.error.take() {
            return Err(error);
        }
        // RAR4 没有独立的密码校验值，错误密码与损坏的密文均报告 CRC/数据错误。
        if code == native::ERAR_BAD_DATA && self.encrypted && self.state.password.is_some() {
            return Err(ArchiveError::Other("RAR 解密失败：密码错误或压缩包损坏".into()));
        }
        if code != native::ERAR_SUCCESS {
            return Err(native_error(code));
        }
        Ok(())
    }

    fn next(&mut self) -> Result<Option<PackedHeader>, ArchiveError> {
        let mut header = PackedHeader::default();
        // SAFETY: handle 有效；header 为正确初始化的输出结构体，回调状态仍存活。
        let code = unsafe { native::RARReadHeaderEx(self.handle, ptr::from_mut(&mut header).cast()) };
        if code == native::ERAR_END_ARCHIVE && self.state.error.is_none() {
            return Ok(None);
        }
        self.check(code)?;
        self.encrypted = header.flags & native::RHDF_ENCRYPTED != 0;
        Ok(Some(header))
    }

    fn process(&mut self, output: Option<&NamedTempFile>) -> Result<u64, ArchiveError> {
        self.state.file_size = 0;
        self.state.output = output.map(NamedTempFile::reopen).transpose()?;
        // SAFETY: 只调用 SKIP/TEST，不由原生库写盘。状态存活且只能通过同步回调修改。
        let code = unsafe {
            native::RARProcessFile(
                self.handle,
                if output.is_some() { native::RAR_TEST } else { native::RAR_SKIP },
                ptr::null(),
                ptr::null(),
            )
        };
        self.state.output = None;
        self.check(code)?;
        Ok(self.state.file_size)
    }

    fn read_file(&mut self, header: &PackedHeader, output: &NamedTempFile) -> Result<u64, ArchiveError> {
        let declared = size(header);
        self.state.budget.check_declared(declared)?;
        let actual = self.process(Some(output))?;
        if actual != declared {
            return Err(ArchiveError::Other(if self.encrypted {
                "RAR 解密失败：密码错误或压缩包损坏".into()
            } else {
                "RAR 文件损坏：解压后的大小与文件头不符".into()
            }));
        }
        Ok(actual)
    }
}

fn entry_name(header: &PackedHeader) -> Option<String> {
    let filename_w = header.filename_w;
    let wide: Vec<_> = filename_w.iter().copied().take_while(|&c| c != 0).collect();
    #[cfg(windows)]
    let decoded = String::from_utf16_lossy(&wide);
    #[cfg(not(windows))]
    let decoded: String = wide.iter().map(|&c| char::from_u32(c as u32).unwrap_or('\u{FFFD}')).collect();
    let decoded = if decoded.is_empty() {
        let filename = header.filename;
        let raw: Vec<_> = filename.iter().copied().take_while(|&c| c != 0).map(|c| c as u8).collect();
        decode_filename(&raw)
    } else {
        decoded
    };
    normalize_name(&decoded)
}

fn normalize_name(name: &str) -> Option<String> {
    let name = name.replace('\\', "/");
    if name.starts_with('/') || name.contains(':') || name.split('/').any(|part| part == "..") {
        return None;
    }
    let name = name.split('/').filter(|p| !p.is_empty() && *p != ".").collect::<Vec<_>>().join("/");
    (!name.is_empty()).then_some(name)
}

fn size(header: &PackedHeader) -> u64 {
    (u64::from(header.unp_size_high) << 32) | u64::from(header.unp_size)
}

pub(super) fn extract(
    src: &str, dest: &str, password: Option<&str>, file_id: i64, limits: &ExtractLimits,
) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let mut archive = RarArchive::open(src, password, limits)?;
    // 全部成功后才移动文件，错误密码或超限不会留下可下载的部分内容。
    let staging = tempfile::tempdir_in(dest)?;
    let mut entries = Vec::new();
    let mut count = 0;
    while let Some(header) = archive.next()? {
        count += 1;
        if count > limits.max_file_count {
            return Err(ArchiveError::FileCountExceeded { max: limits.max_file_count, actual: count });
        }
        let name = match entry_name(&header).filter(|_| header.redir_type == 0) {
            Some(name) => name,
            None => {
                archive.process(None)?;
                continue;
            }
        };
        let is_directory = header.flags & native::RHDF_DIRECTORY != 0;
        let out = staging.path().join(&name);
        let actual_size = if is_directory {
            std::fs::create_dir_all(&out)?;
            archive.process(None)?;
            0
        } else {
            archive.state.budget.check_declared(size(&header))?;
            std::fs::create_dir_all(out.parent().unwrap())?;
            let temp = NamedTempFile::new_in(out.parent().unwrap())?;
            let actual = archive.read_file(&header, &temp)?;
            temp.persist(&out).map_err(|e| e.error)?;
            actual
        };
        entries.push(ArchiveEntry {
            id: None,
            file_id,
            entry_path: name,
            size: Some(actual_size as i64),
            is_directory,
        });
    }
    // 只移动第一层，目录及其子文件保持完整结构。
    for entry in std::fs::read_dir(staging.path())? {
        let entry = entry?;
        std::fs::rename(entry.path(), Path::new(dest).join(entry.file_name()))?;
    }
    Ok(entries)
}

pub(super) fn read_entry(src: &str, target: &str, password: Option<&str>) -> Result<NamedTempFile, ArchiveError> {
    let target = normalize_name(target).ok_or_else(|| ArchiveError::Other("无效的压缩包内路径".into()))?;
    let limits = ExtractLimits::default();
    let mut archive = RarArchive::open(src, password, &limits)?;
    archive.state.budget = ByteBudget::single_entry(&limits);
    let mut count = 0;
    while let Some(header) = archive.next()? {
        count += 1;
        if count > limits.max_file_count {
            return Err(ArchiveError::FileCountExceeded { max: limits.max_file_count, actual: count });
        }
        if entry_name(&header).as_deref() == Some(&target)
            && header.redir_type == 0
            && header.flags & native::RHDF_DIRECTORY == 0
        {
            archive.state.budget.check_declared(size(&header))?;
            let temp = NamedTempFile::new()?;
            archive.read_file(&header, &temp)?;
            return Ok(temp);
        }
        archive.process(None)?;
    }
    Err(ArchiveError::Other(format!("文件不存在: {target}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rar").join(name).to_string_lossy().into_owned()
    }

    fn unpack(
        name: &str, password: Option<&str>, limits: &ExtractLimits,
    ) -> (tempfile::TempDir, Result<Vec<ArchiveEntry>, ArchiveError>) {
        let dest = tempfile::tempdir().unwrap();
        let result = super::super::extract_archive_with_limits(
            &fixture(name),
            dest.path().to_str().unwrap(),
            name,
            password,
            42,
            limits,
        );
        (dest, result)
    }

    #[test]
    fn rar4_and_rar5_extract_directories_and_unicode() {
        for name in ["rar3-subdirs.rar", "rar5-subdirs.rar"] {
            let (dest, result) = unpack(name, None, &ExtractLimits::default());
            let entries = result.unwrap();
            assert_eq!(entries.len(), 10, "{name}");
            assert!(entries.iter().all(|entry| entry.file_id == 42));
            assert!(dest.path().join("sub/empty").is_dir());
            assert_eq!(std::fs::read(dest.path().join("sub/dir2/file2.txt")).unwrap().len(), 6);
            assert_eq!(std::fs::read(dest.path().join("sub/üȵĩöḋè/file.txt")).unwrap().len(), 5);
            for entry in entries.iter().filter(|e| !e.is_directory) {
                let downloaded =
                    super::super::read_archive_entry(&fixture(name), name, &entry.entry_path, None).unwrap();
                assert_eq!(
                    std::fs::read(downloaded.path()).unwrap(),
                    std::fs::read(dest.path().join(&entry.entry_path)).unwrap()
                );
            }
        }
    }

    #[test]
    fn rar5_compressed_and_solid_contents_match() {
        for name in ["rar5-crc.rar", "rar5-solid.rar"] {
            let (dest, result) = unpack(name, None, &ExtractLimits::default());
            let entries = result.unwrap();
            assert_eq!(entries.len(), 2);
            let expected = std::fs::read(dest.path().join("stest1.txt")).unwrap();
            assert_eq!(expected.len(), 2048);
            assert_eq!(std::fs::read(dest.path().join("stest2.txt")).unwrap(), expected);
            let downloaded = read_entry(&fixture(name), "stest2.txt", None).unwrap();
            assert_eq!(std::fs::read(downloaded.path()).unwrap(), expected);
        }
    }

    #[test]
    fn rar_encrypted_data_and_headers_require_password_and_allow_retry() {
        for (name, password) in [
            ("rar4-crypted.rar", "unrar"),
            ("rar4-comment-hpw-password.rar", "password"),
            ("rar5-psw.rar", "password"),
            ("rar5-hpsw.rar", "password"),
        ] {
            let (dest, result) = unpack(name, None, &ExtractLimits::default());
            assert!(matches!(result, Err(ArchiveError::PasswordRequired)), "{name}: {result:?}");
            assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
            let result =
                extract(&fixture(name), dest.path().to_str().unwrap(), Some("wrong"), 42, &ExtractLimits::default());
            assert!(result.as_ref().unwrap_err().to_string().contains("密码"), "{name}: {result:?}");
            assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
            let entries =
                extract(&fixture(name), dest.path().to_str().unwrap(), Some(password), 42, &ExtractLimits::default())
                    .unwrap();
            assert!(!entries.is_empty(), "{name}");
            let entry = entries.iter().find(|e| !e.is_directory).unwrap();
            assert!(matches!(read_entry(&fixture(name), &entry.entry_path, None), Err(ArchiveError::PasswordRequired)));
            let downloaded = read_entry(&fixture(name), &entry.entry_path, Some(password)).unwrap();
            assert_eq!(
                std::fs::read(downloaded.path()).unwrap(),
                std::fs::read(dest.path().join(&entry.entry_path)).unwrap()
            );
        }
    }

    #[test]
    fn rar_limits_reject_without_leaving_partial_files() {
        for limits in [
            ExtractLimits { max_file_size: 2047, ..ExtractLimits::default() },
            ExtractLimits { max_total_size: 4095, ..ExtractLimits::default() },
            ExtractLimits { max_file_count: 1, ..ExtractLimits::default() },
        ] {
            let (dest, result) = unpack("rar5-crc.rar", None, &limits);
            assert!(
                matches!(result, Err(ArchiveError::SizeLimitExceeded { .. } | ArchiveError::FileCountExceeded { .. })),
                "{result:?}"
            );
            assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
        }
        let limits = ExtractLimits { max_file_size: 2048, max_total_size: 4096, max_file_count: 2 };
        assert!(unpack("rar5-crc.rar", None, &limits).1.is_ok());
    }

    #[test]
    fn rar_links_cannot_escape_destination() {
        let (dest, result) = unpack("rar5-evil-symlink-traversal.rar", None, &ExtractLimits::default());
        let entries = result.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry_path, "up/pwned.txt");
        assert!(dest.path().join("up").is_dir());
        assert!(!std::fs::symlink_metadata(dest.path().join("up")).unwrap().file_type().is_symlink());
        assert!(read_entry(&fixture("rar5-evil-symlink-traversal.rar"), "up", None).is_err());
    }

    #[test]
    fn rar_rejects_unsafe_paths_and_preserves_regular_names() {
        for path in ["../outside", "a/../../outside", "/absolute", "\\\\server\\share", "C:\\outside", ".", ""] {
            assert!(normalize_name(path).is_none(), "{path}");
        }
        assert_eq!(normalize_name("./目录\\子目录//file..txt"), Some("目录/子目录/file..txt".into()));
        assert!(read_entry(&fixture("rar5-crc.rar"), "../stest1.txt", None).is_err());
        assert!(read_entry(&fixture("rar5-crc.rar"), "missing.txt", None).is_err());
    }

    #[test]
    fn rar_extracts_chinese_and_windows_paths_but_skips_traversal() {
        let (dest, result) = unpack("paths.rar", None, &ExtractLimits::default());
        let entries = result.unwrap();
        assert_eq!(entries.len(), 2, "{entries:?}");
        for name in ["docs/中文.txt", "folder/hello.txt"] {
            assert_eq!(std::fs::read(dest.path().join(name)).unwrap(), b"hello rar\n");
            let downloaded = read_entry(&fixture("paths.rar"), name, None).unwrap();
            assert_eq!(std::fs::read(downloaded.path()).unwrap(), b"hello rar\n");
        }
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 2);
    }

    #[test]
    fn rar_volumes_are_explicitly_rejected() {
        let (dest, result) = unpack("volume.part1.rar", None, &ExtractLimits::default());
        assert!(matches!(result, Err(ArchiveError::UnsupportedFormat(_))), "{result:?}");
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
    }

    #[test]
    fn rar_actual_bytes_stop_callback_at_file_or_total_limit() {
        for limits in [
            ExtractLimits { max_file_size: 4, ..ExtractLimits::default() },
            ExtractLimits { max_total_size: 4, ..ExtractLimits::default() },
        ] {
            let mut archive = RarArchive::open(&fixture("rar5-crc.rar"), None, &limits).unwrap();
            let temp = NamedTempFile::new().unwrap();
            archive.state.output = Some(temp.reopen().unwrap());
            let user = (&mut *archive.state as *mut CallbackState) as native::LPARAM;
            let data = b"abc";
            assert_eq!(callback(native::UCM_PROCESSDATA, user, data.as_ptr() as native::LPARAM, 3), 0);
            assert_eq!(callback(native::UCM_PROCESSDATA, user, data.as_ptr() as native::LPARAM, 3), -1);
            assert!(matches!(archive.state.error, Some(ArchiveError::SizeLimitExceeded { .. })));
            assert_eq!(std::fs::read(temp.path()).unwrap(), data);
        }
    }

    #[test]
    fn rar_corrupt_archive_is_rejected() {
        let src = NamedTempFile::new().unwrap();
        std::fs::write(src.path(), b"not a rar archive").unwrap();
        let dest = tempfile::tempdir().unwrap();
        assert!(
            extract(src.path().to_str().unwrap(), dest.path().to_str().unwrap(), None, 42, &ExtractLimits::default())
                .is_err()
        );
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
    }
}
