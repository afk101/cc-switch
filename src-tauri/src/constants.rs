/// 日志目录名称。
pub const LOGS_DIRECTORY_NAME: &str = "logs";

/// 日志导出文件名前缀。
pub const LOG_EXPORT_FILE_PREFIX: &str = "cc-switch-logs-";

/// 日志导出文件扩展名。
pub const LOG_EXPORT_FILE_EXTENSION: &str = ".zip";

/// 日志导出文件的本地时间格式。
pub const LOG_EXPORT_TIMESTAMP_FORMAT: &str = "%Y%m%d-%H%M%S";

/// 日志导出流式复制缓冲区大小。
pub const LOG_EXPORT_BUFFER_SIZE: usize = 64 * 1024;

/// 没有可归档日志时返回的稳定错误码。
pub const LOG_EXPORT_NO_LOGS_ERROR: &str = "NO_LOGS";

/// 日志导出整体失败时返回的稳定错误码。
pub const LOG_EXPORT_FAILED_ERROR: &str = "LOG_EXPORT_FAILED";

/// 已有日志导出正在运行时返回的稳定错误码。
pub const LOG_EXPORT_BUSY_ERROR: &str = "LOG_EXPORT_BUSY";

/// Body dump 文件名日期前缀的解析格式。
pub const BODY_DUMP_DATE_FORMAT: &str = "%Y%m%d";

/// Body dump 清理摘要最多保留的错误样本数。
pub const BODY_DUMP_CLEANUP_ERROR_SAMPLE_LIMIT: usize = 5;

/// Body dump 日志根目录名称。
pub const BODY_DUMP_DIRECTORY_NAME: &str = "proxy-bodies";

/// 应用级维护任务的执行间隔（24 小时）。
pub const PERIODIC_MAINTENANCE_INTERVAL_SECS: u64 = 24 * 60 * 60;
