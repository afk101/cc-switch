use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;
use zip::write::SimpleFileOptions;

use crate::constants::{
    LOGS_DIRECTORY_NAME, LOG_EXPORT_FILE_EXTENSION, LOG_EXPORT_FILE_PREFIX,
    LOG_EXPORT_TIMESTAMP_FORMAT,
};

/// 将日志目录中的普通文件流式写入下载目录中的 ZIP。
pub fn export_logs_archive(logs_dir: &Path, download_dir: &Path) -> Result<PathBuf> {
    let archive_name = format!(
        "{}{}{}",
        LOG_EXPORT_FILE_PREFIX,
        Local::now().format(LOG_EXPORT_TIMESTAMP_FORMAT),
        LOG_EXPORT_FILE_EXTENSION
    );
    let archive_path = download_dir.join(archive_name);
    let archive_file = fs::File::create(&archive_path)
        .with_context(|| format!("无法创建日志导出文件：{}", archive_path.display()))?;
    let mut archive = zip::ZipWriter::new(archive_file);
    append_directory(logs_dir, logs_dir, &mut archive)?;
    archive.finish().context("无法完成日志 ZIP 写入")?;
    Ok(archive_path)
}

/// 递归追加目录中的普通文件，并保留相对于日志根目录的结构。
fn append_directory(
    logs_root: &Path,
    current_dir: &Path,
    archive: &mut zip::ZipWriter<fs::File>,
) -> Result<()> {
    for entry in fs::read_dir(current_dir)
        .with_context(|| format!("无法读取日志目录：{}", current_dir.display()))?
    {
        let entry = entry.context("无法读取日志目录项")?;
        let file_type = entry.file_type().context("无法读取日志文件类型")?;
        let path = entry.path();
        if file_type.is_dir() {
            append_directory(logs_root, &path, archive)?;
        } else if file_type.is_file() {
            append_file(logs_root, &path, archive)?;
        }
    }
    Ok(())
}

/// 以固定缓冲区流式追加单个普通日志文件。
fn append_file(
    logs_root: &Path,
    source_path: &Path,
    archive: &mut zip::ZipWriter<fs::File>,
) -> Result<()> {
    let relative_path = source_path
        .strip_prefix(logs_root)
        .context("日志文件不在日志根目录中")?;
    let entry_name = archive_entry_name(relative_path);
    archive
        .start_file(entry_name, SimpleFileOptions::default())
        .context("无法创建日志 ZIP 条目")?;
    let mut source = fs::File::open(source_path)
        .with_context(|| format!("无法打开日志文件：{}", source_path.display()))?;
    io::copy(&mut source, archive)
        .with_context(|| format!("无法写入日志文件：{}", source_path.display()))?;
    Ok(())
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
    use std::io::Read;

    use tempfile::tempdir;
    use zip::ZipArchive;

    use super::export_logs_archive;

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
}
