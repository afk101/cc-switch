use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use tauri::{AppHandle, Manager};

use crate::config::get_app_config_dir;
use crate::constants::{LOGS_DIRECTORY_NAME, LOG_EXPORT_BUSY_ERROR, LOG_EXPORT_FAILED_ERROR};
use crate::services::{export_logs_archive, LogExportError};

static LOG_EXPORT_COORDINATOR: OnceLock<Arc<LogExportCoordinator>> = OnceLock::new();

/// 跨窗口日志导出的单飞协调器。
#[derive(Debug, Default)]
pub struct LogExportCoordinator {
    is_exporting: AtomicBool,
}

impl LogExportCoordinator {
    /// 创建空闲的日志导出协调器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 尝试获取一次日志导出执行权。
    fn try_acquire(self: &Arc<Self>) -> Option<LogExportLease> {
        self.is_exporting
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| LogExportLease {
                coordinator: Arc::clone(self),
            })
    }
}

/// 在执行结束或任务取消时释放单飞状态。
struct LogExportLease {
    coordinator: Arc<LogExportCoordinator>,
}

impl Drop for LogExportLease {
    fn drop(&mut self) {
        self.coordinator
            .is_exporting
            .store(false, Ordering::Release);
    }
}

/// 导出当前生效应用配置目录中的日志到系统下载目录。
#[tauri::command]
#[allow(non_snake_case)]
pub async fn exportLogs(app: AppHandle) -> Result<String, String> {
    let app_config_dir = get_app_config_dir();
    let download_dir = app.path().download_dir().ok();
    let desktop_dir = app.path().desktop_dir().ok();
    let coordinator =
        Arc::clone(LOG_EXPORT_COORDINATOR.get_or_init(|| Arc::new(LogExportCoordinator::new())));
    export_logs_once_with(
        coordinator,
        app_config_dir,
        download_dir,
        desktop_dir,
        export_logs_archive,
    )
    .await
}

/// 在单飞协调器保护下执行一次日志导出。
pub async fn export_logs_once_with<F>(
    coordinator: Arc<LogExportCoordinator>,
    app_config_dir: PathBuf,
    download_dir: Option<PathBuf>,
    desktop_dir: Option<PathBuf>,
    exporter: F,
) -> Result<String, String>
where
    F: FnMut(&Path, &Path) -> Result<PathBuf, LogExportError> + Send + 'static,
{
    let _lease = coordinator
        .try_acquire()
        .ok_or_else(|| LOG_EXPORT_BUSY_ERROR.to_string())?;
    export_logs_from_directories_with(app_config_dir, download_dir, desktop_dir, exporter).await
}

/// 从已解析的系统目录依次尝试下载目录和桌面目录。
pub async fn export_logs_from_directories(
    app_config_dir: PathBuf,
    download_dir: Option<PathBuf>,
    desktop_dir: Option<PathBuf>,
) -> Result<String, String> {
    export_logs_from_directories_with(
        app_config_dir,
        download_dir,
        desktop_dir,
        export_logs_archive,
    )
    .await
}

/// 使用指定归档器从已解析的系统目录依次尝试下载目录和桌面目录。
pub async fn export_logs_from_directories_with<F>(
    app_config_dir: PathBuf,
    download_dir: Option<PathBuf>,
    desktop_dir: Option<PathBuf>,
    exporter: F,
) -> Result<String, String>
where
    F: FnMut(&Path, &Path) -> Result<PathBuf, LogExportError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || {
        let logs_dir = app_config_dir.join(LOGS_DIRECTORY_NAME);
        export_to_first_available_directory(&logs_dir, [download_dir, desktop_dir], exporter)
    })
    .await
    .map_err(|error| format!("{LOG_EXPORT_FAILED_ERROR}: 日志导出后台任务失败：{error}"))?
}

/// 按给定顺序尝试目标目录，并保留无日志业务结果。
fn export_to_first_available_directory<F>(
    logs_dir: &std::path::Path,
    target_dirs: [Option<PathBuf>; 2],
    mut exporter: F,
) -> Result<String, String>
where
    F: FnMut(&Path, &Path) -> Result<PathBuf, LogExportError>,
{
    let mut attempted_target = false;
    for target_dir in target_dirs.into_iter().flatten() {
        attempted_target = true;
        match exporter(logs_dir, &target_dir) {
            Ok(path) => return Ok(path.to_string_lossy().into_owned()),
            Err(LogExportError::NoLogs) => return Err(LogExportError::NoLogs.to_string()),
            Err(LogExportError::TargetUnavailable(_)) => continue,
            Err(LogExportError::Failed(error)) => {
                return Err(format!("{LOG_EXPORT_FAILED_ERROR}: {error}"));
            }
        }
    }
    let reason = if attempted_target {
        "所有日志导出目标目录均不可用"
    } else {
        "无法解析日志导出目标目录"
    };
    Err(format!("{LOG_EXPORT_FAILED_ERROR}: {reason}"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex};

    use anyhow::anyhow;
    use tempfile::tempdir;

    use super::{
        export_logs_from_directories, export_logs_from_directories_with, export_logs_once_with,
        LogExportCoordinator,
    };
    use crate::constants::LOG_EXPORT_BUSY_ERROR;
    use crate::services::LogExportError;

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

    #[tokio::test]
    async fn does_not_fall_back_after_archiving_has_started() {
        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let download_dir = tempdir().expect("创建临时下载目录");
        let desktop_dir = tempdir().expect("创建临时桌面目录");
        let attempted_targets = Arc::new(Mutex::new(Vec::new()));
        let observed_targets = Arc::clone(&attempted_targets);

        let error = export_logs_from_directories_with(
            app_config_dir.path().to_path_buf(),
            Some(download_dir.path().to_path_buf()),
            Some(desktop_dir.path().to_path_buf()),
            move |_logs_dir, target_dir| {
                observed_targets
                    .lock()
                    .expect("记录尝试目标")
                    .push(target_dir.to_path_buf());
                Err(LogExportError::Failed(anyhow!("ZIP finish failed")))
            },
        )
        .await
        .expect_err("归档开始后的失败必须整体失败");

        assert!(error.starts_with("LOG_EXPORT_FAILED:"));
        assert_eq!(
            attempted_targets.lock().expect("读取尝试目标").as_slice(),
            [download_dir.path().to_path_buf()],
            "不得在 ZIP 写入失败后回退桌面"
        );
    }

    #[tokio::test]
    async fn rejects_a_concurrent_command_without_starting_another_archive() {
        let app_config_dir = tempdir().expect("创建临时应用配置目录");
        let download_dir = tempdir().expect("创建临时下载目录");
        let coordinator = Arc::new(LogExportCoordinator::new());
        let invocation_count = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_count = Arc::clone(&invocation_count);
        let first_coordinator = Arc::clone(&coordinator);
        let first_config_dir = app_config_dir.path().to_path_buf();
        let first_download_dir = download_dir.path().to_path_buf();
        let first_export = tokio::spawn(async move {
            export_logs_once_with(
                first_coordinator,
                first_config_dir,
                Some(first_download_dir.clone()),
                None,
                move |_logs_dir, _target_dir| {
                    first_count.fetch_add(1, Ordering::SeqCst);
                    started_tx.send(()).expect("通知归档已开始");
                    release_rx.recv().expect("等待释放归档");
                    Ok(first_download_dir.join("first.zip"))
                },
            )
            .await
        });
        tokio::task::yield_now().await;
        started_rx.recv().expect("等待第一次归档开始");

        let second_count = Arc::clone(&invocation_count);
        let error = export_logs_once_with(
            Arc::clone(&coordinator),
            app_config_dir.path().to_path_buf(),
            Some(download_dir.path().to_path_buf()),
            None,
            move |_logs_dir, target_dir| {
                second_count.fetch_add(1, Ordering::SeqCst);
                Ok(target_dir.join("second.zip"))
            },
        )
        .await
        .expect_err("并发命令必须返回忙碌错误");

        assert_eq!(error, LOG_EXPORT_BUSY_ERROR);
        assert_eq!(invocation_count.load(Ordering::SeqCst), 1);
        release_tx.send(()).expect("释放第一次归档");
        first_export
            .await
            .expect("等待第一次命令")
            .expect("第一次命令成功");
    }
}
