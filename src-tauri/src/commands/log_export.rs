use std::path::PathBuf;

use tauri::{AppHandle, Manager};

use crate::config::get_app_config_dir;
use crate::constants::LOGS_DIRECTORY_NAME;
use crate::services::export_logs_archive;

/// 导出当前生效应用配置目录中的日志到系统下载目录。
#[tauri::command]
pub async fn export_logs(app: AppHandle) -> Result<String, String> {
    let app_config_dir = get_app_config_dir();
    let download_dir = app
        .path()
        .download_dir()
        .map_err(|error| format!("无法获取系统下载目录：{error}"))?;
    export_logs_from_directories(app_config_dir, download_dir).await
}

/// 从已解析的系统目录执行后台日志导出。
pub async fn export_logs_from_directories(
    app_config_dir: PathBuf,
    download_dir: PathBuf,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let logs_dir = app_config_dir.join(LOGS_DIRECTORY_NAME);
        export_logs_archive(&logs_dir, &download_dir)
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("日志导出后台任务失败：{error}"))?
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
            download_dir.path().to_path_buf(),
        )
        .await
        .expect("导出当前配置目录日志");

        let exported_path = std::path::PathBuf::from(&result);
        assert_eq!(exported_path.parent(), Some(download_dir.path()));
        assert!(exported_path.is_file());
        assert!(result.starts_with(download_dir.path().to_string_lossy().as_ref()));
    }
}
