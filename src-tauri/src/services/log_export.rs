use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result as AnyhowResult};
use chrono::Local;
use tempfile::NamedTempFile;
use thiserror::Error;
use zip::write::SimpleFileOptions;

use crate::constants::{
    LOGS_DIRECTORY_NAME, LOG_EXPORT_BUFFER_SIZE, LOG_EXPORT_FILE_EXTENSION, LOG_EXPORT_FILE_PREFIX,
    LOG_EXPORT_NO_LOGS_ERROR, LOG_EXPORT_TIMESTAMP_FORMAT,
};

/// 日志导出的稳定业务错误。
#[derive(Debug, Error)]
pub enum LogExportError {
    /// 日志目录不存在或不含可归档普通文件。
    #[error("{LOG_EXPORT_NO_LOGS_ERROR}")]
    NoLogs,
    /// 目标目录无法创建临时输出，可尝试下一个系统目录。
    #[error(transparent)]
    TargetUnavailable(anyhow::Error),
    /// 目标目录或 ZIP 写入等整体失败。
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

#[derive(Debug)]
struct LogSnapshot {
    source_path: PathBuf,
    relative_path: PathBuf,
    entry_name: String,
    length: u64,
}

/// 为日志归档提供可替换的源文件读取边界。
pub trait LogFileReader {
    /// 打开一个已快照的日志文件。
    fn open(&self, relative_path: &Path) -> std::io::Result<Box<dyn Read>>;
}

/// 生产环境使用的本地文件读取器。
struct FileSystemLogReader {
    root: fs::File,
    root_path: PathBuf,
}

impl FileSystemLogReader {
    /// 打开并固定日志根目录，后续读取只允许从该根目录解析。
    fn new(logs_dir: &Path) -> std::io::Result<Self> {
        Ok(Self {
            root: open_log_root(logs_dir)?,
            root_path: logs_dir.to_path_buf(),
        })
    }
}

impl LogFileReader for FileSystemLogReader {
    fn open(&self, relative_path: &Path) -> std::io::Result<Box<dyn Read>> {
        open_log_file_without_links(&self.root, &self.root_path, relative_path)
            .map(|file| Box::new(file) as Box<dyn Read>)
    }
}

/// 在 Unix 上以不跟随最终符号链接的方式固定日志根目录。
#[cfg(unix)]
fn open_log_root(logs_dir: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(logs_dir)
        .map_err(normalize_no_follow_error)
}

/// 在 Unix 上从已固定的根目录逐级打开，杜绝祖先目录与文件链接竞态。
#[cfg(unix)]
fn open_log_file_without_links(
    root: &fs::File,
    _root_path: &Path,
    relative_path: &Path,
) -> std::io::Result<fs::File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Component;

    let components = relative_path.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "日志相对路径包含非法组件",
        ));
    }

    let mut current_dir = root.try_clone()?;
    for component in &components[..components.len() - 1] {
        let Component::Normal(name) = component else {
            unreachable!("路径组件已经完成校验")
        };
        let name = CString::new(name.as_bytes()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "日志路径包含 NUL")
        })?;
        // SAFETY：目录 fd 在调用期间有效，C 字符串以 NUL 结尾；返回 fd 立即交给 OwnedFd。
        let fd = unsafe {
            libc::openat(
                current_dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(normalize_no_follow_error(std::io::Error::last_os_error()));
        }
        // SAFETY：openat 成功返回当前进程独占的新 fd。
        current_dir = unsafe { fs::File::from(OwnedFd::from_raw_fd(fd)) };
    }

    let Component::Normal(file_name) = components[components.len() - 1] else {
        unreachable!("路径组件已经完成校验")
    };
    let file_name = CString::new(file_name.as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "日志路径包含 NUL"))?;
    // SAFETY：目录 fd 与 C 字符串均有效；O_NOFOLLOW 保证最终组件不是符号链接目标。
    let fd = unsafe {
        libc::openat(
            current_dir.as_raw_fd(),
            file_name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(normalize_no_follow_error(std::io::Error::last_os_error()));
    }
    // SAFETY：openat 成功返回当前进程独占的新 fd。
    let file = unsafe { fs::File::from_raw_fd(fd) };
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
    }
    Ok(file)
}

/// 把链接替换产生的平台错误归一为可静默跳过的快照变化。
#[cfg(unix)]
fn normalize_no_follow_error(error: std::io::Error) -> std::io::Error {
    match error.raw_os_error() {
        Some(libc::ELOOP) | Some(libc::ENOTDIR) => {
            std::io::Error::from(std::io::ErrorKind::NotFound)
        }
        _ => error,
    }
}

/// Windows 上固定日志根目录；候选文件打开后会用同一 handle 验证最终边界。
#[cfg(windows)]
fn open_log_root(logs_dir: &Path) -> std::io::Result<fs::File> {
    open_windows_path(logs_dir, true)
}

/// Windows 上拒绝最终 reparse point，并基于已打开 handle 验证仍位于固定根目录。
#[cfg(windows)]
fn open_log_file_without_links(
    root: &fs::File,
    _root_path: &Path,
    relative_path: &Path,
) -> std::io::Result<fs::File> {
    use std::path::Component;

    let components = relative_path.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "日志相对路径包含非法组件",
        ));
    }

    let mut current = root.try_clone()?;
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            unreachable!("路径组件已经完成校验")
        };
        current = open_windows_relative_path(&current, name, index + 1 < components.len())?;
    }
    if !current.metadata()?.is_file() {
        return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
    }
    Ok(current)
}

/// Windows 上以 OPEN_REPARSE_POINT 打开路径，并拒绝 reparse point 本身。
#[cfg(windows)]
fn open_windows_path(path: &Path, directory: bool) -> std::io::Result<fs::File> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FileAttributeTagInfo, GetFileInformationByHandleEx,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let directory_flag = if directory {
        FILE_FLAG_BACKUP_SEMANTICS
    } else {
        0
    };
    // SAFETY：wide 是 NUL 结尾的稳定缓冲区，其余参数均为 Win32 文档允许值。
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | directory_flag,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY：CreateFileW 成功返回当前进程持有的 handle，立即转交给 File 管理。
    let file = unsafe { fs::File::from_raw_handle(handle as _) };
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY：file handle 有效，info 缓冲区尺寸与请求的信息类匹配。
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
    }
    Ok(file)
}

/// Windows 上以父目录 handle 为锚打开单个组件，并拒绝该组件是 reparse point。
#[cfg(windows)]
fn open_windows_relative_path(
    parent: &fs::File,
    name: &std::ffi::OsStr,
    directory: bool,
) -> std::io::Result<fs::File> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        NtOpenFile, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN_REPARSE_POINT,
        FILE_SYNCHRONOUS_IO_NONALERT,
    };
    use windows_sys::Win32::Foundation::{
        RtlNtStatusToDosError, OBJ_CASE_INSENSITIVE, UNICODE_STRING,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, SYNCHRONIZE,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let mut wide = name.encode_wide().collect::<Vec<_>>();
    let byte_length = wide
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "日志路径组件过长"))?;
    let unicode_name = UNICODE_STRING {
        Length: byte_length,
        MaximumLength: byte_length,
        Buffer: wide.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle() as _,
        ObjectName: &unicode_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut handle = std::ptr::null_mut();
    let mut io_status = IO_STATUS_BLOCK::default();
    let type_option = if directory {
        FILE_DIRECTORY_FILE
    } else {
        FILE_NON_DIRECTORY_FILE
    };
    // SAFETY：父 handle 在调用期间有效；结构体及 UTF-16 缓冲区生命周期覆盖本次同步调用。
    let status = unsafe {
        NtOpenFile(
            &mut handle,
            FILE_GENERIC_READ | SYNCHRONIZE,
            &attributes,
            &mut io_status,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT | type_option,
        )
    };
    if status < 0 {
        // SAFETY：RtlNtStatusToDosError 对任意 NTSTATUS 返回对应 Win32 错误码。
        let code = unsafe { RtlNtStatusToDosError(status) };
        return Err(std::io::Error::from_raw_os_error(code as i32));
    }
    // SAFETY：NtOpenFile 成功返回当前进程持有的 handle，立即转交给 File 管理。
    let file = unsafe { fs::File::from_raw_handle(handle as _) };
    if is_windows_reparse_point(&file)? {
        return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
    }
    Ok(file)
}

/// 判断 Windows 已打开 handle 是否指向 reparse point。
#[cfg(windows)]
fn is_windows_reparse_point(file: &fs::File) -> std::io::Result<bool> {
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileAttributeTagInfo, GetFileInformationByHandleEx, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_ATTRIBUTE_TAG_INFO,
    };

    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY：file handle 有效，info 缓冲区尺寸与请求的信息类匹配。
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as _,
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

/// 将日志目录中的普通文件流式写入目标目录中的 ZIP。
pub fn export_logs_archive(logs_dir: &Path, target_dir: &Path) -> Result<PathBuf, LogExportError> {
    let reader = match FileSystemLogReader::new(logs_dir) {
        Ok(reader) => reader,
        Err(error) if is_transient_source_error(&error) => return Err(LogExportError::NoLogs),
        Err(error) => return Err(anyhow::Error::from(error).into()),
    };
    export_logs_archive_with_reader(logs_dir, target_dir, &reader)
}

/// 使用指定源文件读取器流式导出日志 ZIP。
pub fn export_logs_archive_with_reader<R: LogFileReader>(
    logs_dir: &Path,
    target_dir: &Path,
    reader: &R,
) -> Result<PathBuf, LogExportError> {
    let snapshots = collect_log_snapshots(logs_dir)?;
    if snapshots.is_empty() {
        return Err(LogExportError::NoLogs);
    }

    let archive_path = available_archive_path(target_dir);
    let mut temporary_archive = NamedTempFile::new_in(target_dir).map_err(|error| {
        LogExportError::TargetUnavailable(anyhow::Error::from(error).context(format!(
            "无法在目标目录创建临时日志导出文件：{}",
            target_dir.display()
        )))
    })?;
    write_archive(temporary_archive.as_file_mut(), &snapshots, reader)?;
    temporary_archive
        .persist_noclobber(&archive_path)
        .map_err(|error| error.error)
        .with_context(|| format!("无法落位日志导出文件：{}", archive_path.display()))?;
    Ok(archive_path)
}

/// 收集不跟随符号链接的普通日志文件及其快照长度。
fn collect_log_snapshots(logs_dir: &Path) -> Result<Vec<LogSnapshot>, LogExportError> {
    let root_metadata = match fs::symlink_metadata(logs_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(LogExportError::NoLogs);
        }
        Err(error) => return Err(anyhow::Error::from(error).into()),
    };
    if !root_metadata.file_type().is_dir() {
        return Err(LogExportError::NoLogs);
    }

    let mut snapshots = Vec::new();
    collect_directory_snapshots(logs_dir, logs_dir, &mut snapshots)?;
    Ok(snapshots)
}

/// 递归收集一个真实目录中的普通文件。
fn collect_directory_snapshots(
    logs_root: &Path,
    current_dir: &Path,
    snapshots: &mut Vec<LogSnapshot>,
) -> AnyhowResult<()> {
    for entry in fs::read_dir(current_dir)
        .with_context(|| format!("无法读取日志目录：{}", current_dir.display()))?
    {
        let entry = entry.context("无法读取日志目录项")?;
        let file_type = entry.file_type().context("无法读取日志文件类型")?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_directory_snapshots(logs_root, &path, snapshots)?;
        } else if file_type.is_file() {
            let metadata = entry.metadata().context("无法读取日志文件元数据")?;
            let relative_path = path
                .strip_prefix(logs_root)
                .context("日志文件不在日志根目录中")?
                .to_path_buf();
            let entry_name = archive_entry_name(&relative_path);
            snapshots.push(LogSnapshot {
                source_path: path,
                relative_path,
                entry_name,
                length: metadata.len(),
            });
        }
    }
    Ok(())
}

/// 选择不覆盖现有 ZIP 的最终路径。
fn available_archive_path(target_dir: &Path) -> PathBuf {
    let base_name = format!(
        "{}{}",
        LOG_EXPORT_FILE_PREFIX,
        Local::now().format(LOG_EXPORT_TIMESTAMP_FORMAT)
    );
    let mut sequence = 1_u64;
    let mut candidate = target_dir.join(format!("{base_name}{LOG_EXPORT_FILE_EXTENSION}"));
    while candidate.exists() {
        sequence += 1;
        candidate = target_dir.join(format!("{base_name}-{sequence}{LOG_EXPORT_FILE_EXTENSION}"));
    }
    candidate
}

/// 把固定快照逐个写入 ZIP，并显式完成中央目录。
fn write_archive<R: LogFileReader>(
    file: &mut fs::File,
    snapshots: &[LogSnapshot],
    reader: &R,
) -> AnyhowResult<()> {
    let mut archive = zip::ZipWriter::new(file);
    for snapshot in snapshots {
        append_snapshot(snapshot, &mut archive, reader)?;
    }
    archive.finish().context("无法完成日志 ZIP 写入")?;
    Ok(())
}

/// 以固定缓冲区写入快照长度；源文件瞬时变化时放弃当前条目。
fn append_snapshot<R: LogFileReader>(
    snapshot: &LogSnapshot,
    archive: &mut zip::ZipWriter<&mut fs::File>,
    reader: &R,
) -> AnyhowResult<()> {
    let mut source = match reader.open(&snapshot.relative_path) {
        Ok(source) => source,
        Err(error) if is_transient_source_error(&error) => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("无法打开日志源文件：{}", snapshot.source_path.display())
            });
        }
    };
    archive
        .start_file(&snapshot.entry_name, SimpleFileOptions::default())
        .context("无法创建日志 ZIP 条目")?;

    let mut remaining = snapshot.length;
    let mut buffer = [0_u8; LOG_EXPORT_BUFFER_SIZE];
    while remaining > 0 {
        let read_limit = remaining.min(buffer.len() as u64) as usize;
        let count = match source.read(&mut buffer[..read_limit]) {
            Ok(0) => {
                archive.abort_file().context("无法放弃变化的日志条目")?;
                return Ok(());
            }
            Err(error) if is_transient_source_error(&error) => {
                archive.abort_file().context("无法放弃变化的日志条目")?;
                return Ok(());
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("无法读取日志源文件：{}", snapshot.source_path.display())
                });
            }
            Ok(count) => count,
        };
        archive
            .write_all(&buffer[..count])
            .with_context(|| format!("无法写入日志文件：{}", snapshot.source_path.display()))?;
        remaining -= count as u64;
    }
    Ok(())
}

/// 判断源文件错误是否来自快照后的瞬时变化。
fn is_transient_source_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::UnexpectedEof
    )
}

/// 构造使用正斜杠且包含 `logs/` 顶层目录的 ZIP 条目路径。
fn archive_entry_name(relative_path: &Path) -> String {
    let relative = relative_path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    format!("{LOGS_DIRECTORY_NAME}/{relative}")
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::cell::Cell;
    use std::fs;
    use std::io::{self, Read};
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::Duration;

    use tempfile::tempdir;
    use zip::ZipArchive;

    use super::{
        export_logs_archive, export_logs_archive_with_reader, FileSystemLogReader, LogExportError,
        LogFileReader,
    };

    struct PermissionDeniedReader;

    impl LogFileReader for PermissionDeniedReader {
        fn open(&self, _path: &Path) -> io::Result<Box<dyn Read>> {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        }
    }

    struct PermissionDeniedOnRead;

    impl Read for PermissionDeniedOnRead {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        }
    }

    struct PermissionDeniedReadOpener;

    impl LogFileReader for PermissionDeniedReadOpener {
        fn open(&self, _path: &Path) -> io::Result<Box<dyn Read>> {
            Ok(Box::new(PermissionDeniedOnRead))
        }
    }

    #[cfg(unix)]
    struct ReplaceWithSymlinkReader {
        delegate: FileSystemLogReader,
        source_path: PathBuf,
        source_relative_path: PathBuf,
        link_target: PathBuf,
        replaced: Cell<bool>,
    }

    #[cfg(unix)]
    impl LogFileReader for ReplaceWithSymlinkReader {
        fn open(&self, relative_path: &Path) -> io::Result<Box<dyn Read>> {
            use std::os::unix::fs::symlink;

            if relative_path == self.source_relative_path && !self.replaced.replace(true) {
                fs::remove_file(&self.source_path)?;
                symlink(&self.link_target, &self.source_path)?;
            }
            self.delegate.open(relative_path)
        }
    }

    #[cfg(unix)]
    struct ReplaceAncestorWithSymlinkReader {
        delegate: FileSystemLogReader,
        source_path: PathBuf,
        source_relative_path: PathBuf,
        ancestor_path: PathBuf,
        link_target: PathBuf,
        replaced: Cell<bool>,
    }

    #[cfg(unix)]
    impl LogFileReader for ReplaceAncestorWithSymlinkReader {
        fn open(&self, relative_path: &Path) -> io::Result<Box<dyn Read>> {
            use std::os::unix::fs::symlink;

            if relative_path == self.source_relative_path && !self.replaced.replace(true) {
                debug_assert!(self.source_path.starts_with(&self.ancestor_path));
                fs::remove_dir_all(&self.ancestor_path)?;
                symlink(&self.link_target, &self.ancestor_path)?;
            }
            self.delegate.open(relative_path)
        }
    }

    #[test]
    fn exports_top_level_and_nested_logs_with_portable_paths() {
        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let download_dir = tempdir().expect("创建临时下载目录");
        let logs_dir = app_config_dir.path().join("logs");
        fs::create_dir_all(logs_dir.join("proxy-bodies/profile-a")).expect("创建嵌套日志目录");
        fs::write(logs_dir.join("cc-switch.log"), b"runtime log").expect("写入顶层日志");
        fs::write(
            logs_dir.join("proxy-bodies/profile-a/request.log"),
            b"request body",
        )
        .expect("写入嵌套日志");

        let archive_path =
            export_logs_archive(&logs_dir, download_dir.path()).expect("导出日志 ZIP");

        let file_name = archive_path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("ZIP 文件名有效");
        assert!(
            file_name.starts_with("cc-switch-logs-") && file_name.ends_with(".zip"),
            "应使用标准日志导出文件名，实际为 {file_name}"
        );

        let archive_file = fs::File::open(&archive_path).expect("打开导出的 ZIP");
        let mut archive = ZipArchive::new(archive_file).expect("读取导出的 ZIP");
        let mut top_level = String::new();
        archive
            .by_name("logs/cc-switch.log")
            .expect("ZIP 包含顶层日志")
            .read_to_string(&mut top_level)
            .expect("读取顶层日志条目");
        let mut nested = String::new();
        archive
            .by_name("logs/proxy-bodies/profile-a/request.log")
            .expect("ZIP 包含嵌套日志")
            .read_to_string(&mut nested)
            .expect("读取嵌套日志条目");

        assert_eq!(
            (top_level.as_str(), nested.as_str()),
            ("runtime log", "request body")
        );
    }

    #[test]
    fn rejects_a_log_tree_without_regular_files() {
        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let download_dir = tempdir().expect("创建临时下载目录");
        let logs_dir = app_config_dir.path().join("logs");
        fs::create_dir_all(logs_dir.join("empty")).expect("创建空日志目录");

        let error = export_logs_archive(&logs_dir, download_dir.path())
            .expect_err("没有普通文件时应拒绝导出");

        assert_eq!(error.to_string(), "NO_LOGS");
        assert_eq!(
            fs::read_dir(download_dir.path())
                .expect("读取下载目录")
                .count(),
            0,
            "不得生成空 ZIP"
        );
    }

    #[test]
    fn preserves_an_existing_archive_and_uses_a_numbered_name() {
        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let download_dir = tempdir().expect("创建临时下载目录");
        let logs_dir = app_config_dir.path().join("logs");
        fs::create_dir_all(&logs_dir).expect("创建日志目录");
        fs::write(logs_dir.join("cc-switch.log"), b"new logs").expect("写入日志文件");

        let first_path = export_logs_archive(&logs_dir, download_dir.path()).expect("首次导出");
        fs::write(&first_path, b"existing archive").expect("替换既有归档内容");

        let second_path = export_logs_archive(&logs_dir, download_dir.path()).expect("再次导出");

        assert_ne!(second_path, first_path);
        assert_eq!(
            fs::read(&first_path).expect("读取既有归档"),
            b"existing archive"
        );
        assert!(
            second_path
                .file_stem()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("-2")),
            "同秒冲突时应使用递增序号"
        );
    }

    #[cfg(unix)]
    #[test]
    fn excludes_symbolic_links_and_their_targets() {
        use std::os::unix::fs::symlink;

        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let download_dir = tempdir().expect("创建临时下载目录");
        let outside_dir = tempdir().expect("创建目录外临时目录");
        let logs_dir = app_config_dir.path().join("logs");
        fs::create_dir_all(&logs_dir).expect("创建日志目录");
        fs::write(logs_dir.join("cc-switch.log"), b"runtime log").expect("写入普通日志");
        let outside_file = outside_dir.path().join("secret.log");
        fs::write(&outside_file, b"outside secret").expect("写入目录外文件");
        symlink(&outside_file, logs_dir.join("linked-secret.log")).expect("创建文件符号链接");
        symlink(outside_dir.path(), logs_dir.join("linked-directory")).expect("创建目录符号链接");

        let archive_path =
            export_logs_archive(&logs_dir, download_dir.path()).expect("导出日志 ZIP");
        let archive_file = fs::File::open(archive_path).expect("打开导出的 ZIP");
        let mut archive = ZipArchive::new(archive_file).expect("读取导出的 ZIP");

        assert!(archive.by_name("logs/cc-switch.log").is_ok());
        assert!(archive.by_name("logs/linked-secret.log").is_err());
        assert!(archive.by_name("logs/linked-directory/secret.log").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn skips_a_regular_file_replaced_by_an_external_symbolic_link_after_snapshot() {
        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let download_dir = tempdir().expect("创建临时下载目录");
        let outside_dir = tempdir().expect("创建目录外临时目录");
        let logs_dir = app_config_dir.path().join("logs");
        fs::create_dir_all(&logs_dir).expect("创建日志目录");
        let source_path = logs_dir.join("changing.log");
        fs::write(&source_path, b"original log").expect("写入待替换日志");
        fs::write(logs_dir.join("stable.log"), b"stable log").expect("写入稳定日志");
        let outside_secret = outside_dir.path().join("secret.log");
        fs::write(&outside_secret, b"outside secret").expect("写入目录外秘密");
        let reader = ReplaceWithSymlinkReader {
            delegate: FileSystemLogReader::new(&logs_dir).expect("固定日志根目录"),
            source_path,
            source_relative_path: PathBuf::from("changing.log"),
            link_target: outside_secret,
            replaced: Cell::new(false),
        };

        let archive_path = export_logs_archive_with_reader(&logs_dir, download_dir.path(), &reader)
            .expect("链接替换属于瞬时变化，应继续导出");
        let archive_file = fs::File::open(archive_path).expect("打开导出的 ZIP");
        let mut archive = ZipArchive::new(archive_file).expect("读取导出的 ZIP");

        assert!(archive.by_name("logs/changing.log").is_err());
        assert!(archive.by_name("logs/stable.log").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn skips_a_file_whose_ancestor_is_replaced_by_an_external_symbolic_link_after_snapshot() {
        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let download_dir = tempdir().expect("创建临时下载目录");
        let outside_dir = tempdir().expect("创建目录外临时目录");
        let logs_dir = app_config_dir.path().join("logs");
        let ancestor_path = logs_dir.join("nested");
        fs::create_dir_all(&ancestor_path).expect("创建嵌套日志目录");
        let source_path = ancestor_path.join("changing.log");
        fs::write(&source_path, b"original log").expect("写入待替换日志");
        fs::write(logs_dir.join("stable.log"), b"stable log").expect("写入稳定日志");
        fs::write(outside_dir.path().join("changing.log"), b"outside secret")
            .expect("写入目录外秘密");
        let reader = ReplaceAncestorWithSymlinkReader {
            delegate: FileSystemLogReader::new(&logs_dir).expect("固定日志根目录"),
            source_path,
            source_relative_path: PathBuf::from("nested/changing.log"),
            ancestor_path,
            link_target: outside_dir.path().to_path_buf(),
            replaced: Cell::new(false),
        };

        let archive_path = export_logs_archive_with_reader(&logs_dir, download_dir.path(), &reader)
            .expect("祖先链接替换属于瞬时变化，应继续导出");
        let archive_file = fs::File::open(archive_path).expect("打开导出的 ZIP");
        let mut archive = ZipArchive::new(archive_file).expect("读取导出的 ZIP");

        assert!(archive.by_name("logs/nested/changing.log").is_err());
        assert!(archive.by_name("logs/stable.log").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symbolic_link_as_the_logs_root() {
        use std::os::unix::fs::symlink;

        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let download_dir = tempdir().expect("创建临时下载目录");
        let outside_dir = tempdir().expect("创建目录外临时目录");
        fs::write(outside_dir.path().join("secret.log"), b"outside secret")
            .expect("写入目录外文件");
        let logs_dir = app_config_dir.path().join("logs");
        symlink(outside_dir.path(), &logs_dir).expect("创建日志根符号链接");

        let error = export_logs_archive(&logs_dir, download_dir.path())
            .expect_err("日志根符号链接不得被跟随");

        assert_eq!(error.to_string(), "NO_LOGS");
        assert_eq!(
            fs::read_dir(download_dir.path())
                .expect("读取下载目录")
                .count(),
            0
        );
    }

    #[test]
    fn limits_an_appending_file_to_its_enumerated_length() {
        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let download_dir = tempdir().expect("创建临时下载目录");
        let logs_dir = app_config_dir.path().join("logs");
        fs::create_dir_all(&logs_dir).expect("创建日志目录");
        let growing_log = logs_dir.join("growing.log");
        let initial_content = vec![b'a'; 32 * 1024 * 1024];
        fs::write(&growing_log, &initial_content).expect("写入初始日志");
        let append_path = growing_log.clone();
        let append_task = thread::spawn(move || {
            thread::sleep(Duration::from_millis(2));
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(append_path)
                .expect("打开追加日志");
            std::io::Write::write_all(&mut file, &vec![b'b'; 32 * 1024 * 1024]).expect("追加日志");
        });

        let archive_path =
            export_logs_archive(&logs_dir, download_dir.path()).expect("导出追加中的日志");
        append_task.join().expect("等待日志追加");
        let archive_file = fs::File::open(archive_path).expect("打开导出的 ZIP");
        let mut archive = ZipArchive::new(archive_file).expect("读取导出的 ZIP");
        let archived_size = archive
            .by_name("logs/growing.log")
            .expect("ZIP 包含追加日志")
            .size();

        assert_eq!(archived_size, initial_content.len() as u64);
    }

    #[test]
    fn skips_a_file_truncated_after_enumeration_and_keeps_other_logs() {
        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let download_dir = tempdir().expect("创建临时下载目录");
        let logs_dir = app_config_dir.path().join("logs");
        fs::create_dir_all(&logs_dir).expect("创建日志目录");
        let changing_log = logs_dir.join("changing.log");
        fs::write(&changing_log, vec![b'x'; 32 * 1024 * 1024]).expect("写入待缩短日志");
        fs::write(logs_dir.join("stable.log"), b"stable").expect("写入稳定日志");
        let truncate_path = changing_log.clone();
        let truncate_task = thread::spawn(move || {
            thread::sleep(Duration::from_millis(2));
            fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(truncate_path)
                .expect("缩短日志");
        });

        let archive_path =
            export_logs_archive(&logs_dir, download_dir.path()).expect("尽力导出变化中的日志");
        truncate_task.join().expect("等待日志缩短");
        let archive_file = fs::File::open(archive_path).expect("打开导出的 ZIP");
        let mut archive = ZipArchive::new(archive_file).expect("读取导出的 ZIP");

        assert!(archive.by_name("logs/changing.log").is_err());
        assert!(archive.by_name("logs/stable.log").is_ok());
    }

    #[test]
    fn leaves_no_final_archive_when_the_target_cannot_create_files() {
        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let target_parent = tempdir().expect("创建临时目标父目录");
        let logs_dir = app_config_dir.path().join("logs");
        fs::create_dir_all(&logs_dir).expect("创建日志目录");
        fs::write(logs_dir.join("cc-switch.log"), b"runtime log").expect("写入日志文件");
        let invalid_target = target_parent.path().join("not-a-directory");
        fs::write(&invalid_target, b"keep me").expect("创建不可用目标");

        export_logs_archive(&logs_dir, &invalid_target).expect_err("目标不可写时应失败");

        assert_eq!(fs::read(&invalid_target).expect("读取既有目标"), b"keep me");
        assert_eq!(
            fs::read_dir(target_parent.path())
                .expect("读取目标父目录")
                .count(),
            1,
            "不得留下临时或最终半成品"
        );
    }

    #[test]
    fn fails_when_a_snapshotted_source_file_cannot_be_opened_persistently() {
        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let download_dir = tempdir().expect("创建临时下载目录");
        let logs_dir = app_config_dir.path().join("logs");
        fs::create_dir_all(&logs_dir).expect("创建日志目录");
        fs::write(logs_dir.join("cc-switch.log"), b"runtime log").expect("写入日志文件");

        let error = export_logs_archive_with_reader(
            &logs_dir,
            download_dir.path(),
            &PermissionDeniedReader,
        )
        .expect_err("持续权限错误必须使导出整体失败");

        assert!(matches!(error, LogExportError::Failed(_)));
        assert_eq!(
            fs::read_dir(download_dir.path())
                .expect("读取下载目录")
                .count(),
            0,
            "失败后不得保留半成品"
        );
    }

    #[test]
    fn fails_when_a_snapshotted_source_file_cannot_be_read_persistently() {
        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let download_dir = tempdir().expect("创建临时下载目录");
        let logs_dir = app_config_dir.path().join("logs");
        fs::create_dir_all(&logs_dir).expect("创建日志目录");
        fs::write(logs_dir.join("cc-switch.log"), b"runtime log").expect("写入日志文件");

        let error = export_logs_archive_with_reader(
            &logs_dir,
            download_dir.path(),
            &PermissionDeniedReadOpener,
        )
        .expect_err("持续读取错误必须使导出整体失败");

        assert!(matches!(error, LogExportError::Failed(_)));
        assert_eq!(
            fs::read_dir(download_dir.path())
                .expect("读取下载目录")
                .count(),
            0,
            "失败后不得保留半成品"
        );
    }
}
