use std::path::PathBuf;

use tauri::{AppHandle, Manager};

use crate::config::get_app_config_dir;
use crate::constants::LOGS_DIRECTORY_NAME;
use crate::services::{export_logs_archive, LogExportError};

/// 导出当前生效应用配置目录中的日志到系统下载目录。
#[tauri::command]
pub async fn export_logs(app: AppHandle) -> Result<String, String> {
    let app_config_dir = get_app_config_dir();
    let download_dir = app.path().download_dir().ok();
    let desktop_dir = app.path().desktop_dir().ok();
    export_logs_from_directories(app_config_dir, download_dir, desktop_dir).await
}

/// 从已解析的系统目录依次尝试下载目录和桌面目录。
pub async fn export_logs_from_directories(
    app_config_dir: PathBuf,
    download_dir: Option<PathBuf>,
    desktop_dir: Option<PathBuf>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let logs_dir = app_config_dir.join(LOGS_DIRECTORY_NAME);
        export_to_first_available_directory(&logs_dir, [download_dir, desktop_dir])
    })
    .await
    .map_err(|error| format!("日志导出后台任务失败：{error}"))?
}

/// 按给定顺序尝试目标目录，并保留无日志业务结果。
fn export_to_first_available_directory(
    logs_dir: &std::path::Path,
    target_dirs: [Option<PathBuf>; 2],
) -> Result<String, String> {
    let mut attempted_target = false;
    for target_dir in target_dirs.into_iter().flatten() {
        attempted_target = true;
        match export_logs_archive(logs_dir, &target_dir) {
            Ok(path) => return Ok(path.to_string_lossy().into_owned()),
            Err(LogExportError::NoLogs) => return Err(LogExportError::NoLogs.to_string()),
            Err(LogExportError::Failed(_)) => continue,
        }
    }
    let reason = if attempted_target {
        "所有日志导出目标目录均不可用"
    } else {
        "无法解析日志导出目标目录"
    };
    Err(format!("LOG_EXPORT_FAILED: {reason}"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::export_logs_from_directories;

    #[tokio::test]
    async fn exports_from_current_app_config_logs_to_download_directory() {
        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let download_dir = tempdir().expect("创建临时下载目录");
        let logs_dir = app_config_dir.path().join("logs");
        fs::create_dir_all(&logs_dir).expect("创建日志目录");
        fs::write(logs_dir.join("cc-switch.log"), b"custom config log").expect("写入日志文件");

        let result = export_logs_from_directories(
            app_config_dir.path().to_path_buf(),
            Some(download_dir.path().to_path_buf()),
            None,
        )
        .await
        .expect("导出当前配置目录日志");

        let exported_path = std::path::PathBuf::from(&result);
        assert_eq!(exported_path.parent(), Some(download_dir.path()));
        assert!(exported_path.is_file());
        assert!(result.starts_with(download_dir.path().to_string_lossy().as_ref()));
    }

    #[tokio::test]
    async fn falls_back_to_desktop_when_download_directory_is_unavailable() {
        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let desktop_dir = tempdir().expect("创建临时桌面目录");
        let unavailable_download = app_config_dir.path().join("download-file");
        let logs_dir = app_config_dir.path().join("logs");
        fs::create_dir_all(&logs_dir).expect("创建日志目录");
        fs::write(logs_dir.join("cc-switch.log"), b"fallback log").expect("写入日志文件");
        fs::write(&unavailable_download, b"not a directory").expect("创建无效下载目标");

        let result = export_logs_from_directories(
            app_config_dir.path().to_path_buf(),
            Some(unavailable_download),
            Some(desktop_dir.path().to_path_buf()),
        )
        .await
        .expect("回退桌面导出");

        assert_eq!(
            std::path::Path::new(&result).parent(),
            Some(desktop_dir.path())
        );
    }

    #[tokio::test]
    async fn fails_when_download_and_desktop_are_both_unavailable() {
        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let logs_dir = app_config_dir.path().join("logs");
        fs::create_dir_all(&logs_dir).expect("创建日志目录");
        fs::write(logs_dir.join("cc-switch.log"), b"runtime log").expect("写入日志文件");

        let error = export_logs_from_directories(app_config_dir.path().to_path_buf(), None, None)
            .await
            .expect_err("两个系统目录都不可用时应失败");

        assert!(error.starts_with("LOG_EXPORT_FAILED:"));
    }
}
