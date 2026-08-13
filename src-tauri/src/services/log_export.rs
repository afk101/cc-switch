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
    entry_name: String,
    length: u64,
}

/// 为日志归档提供可替换的源文件读取边界。
pub trait LogFileReader {
    /// 打开一个已快照的日志文件。
    fn open(&self, path: &Path) -> std::io::Result<Box<dyn Read>>;
}

/// 生产环境使用的本地文件读取器。
struct FileSystemLogReader;

impl LogFileReader for FileSystemLogReader {
    fn open(&self, path: &Path) -> std::io::Result<Box<dyn Read>> {
        fs::File::open(path).map(|file| Box::new(file) as Box<dyn Read>)
    }
}

/// 将日志目录中的普通文件流式写入目标目录中的 ZIP。
pub fn export_logs_archive(logs_dir: &Path, target_dir: &Path) -> Result<PathBuf, LogExportError> {
    export_logs_archive_with_reader(logs_dir, target_dir, &FileSystemLogReader)
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
                .context("日志文件不在日志根目录中")?;
            let entry_name = archive_entry_name(relative_path);
            snapshots.push(LogSnapshot {
                source_path: path,
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
    let mut source = match reader.open(&snapshot.source_path) {
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
    use std::fs;
    use std::io::{self, Read};
    use std::path::Path;
    use std::thread;
    use std::time::Duration;

    use tempfile::tempdir;
    use zip::ZipArchive;

    use super::{
        export_logs_archive, export_logs_archive_with_reader, LogExportError, LogFileReader,
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
