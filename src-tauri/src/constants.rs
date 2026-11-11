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

/// WisCode 生产配置目录；本功能只读该目录，不创建或修改其登录信息。
pub const WISCODE_DIRECTORY_NAME: &str = ".wiscode";
/// WisCode 已有认证文件名。
pub const WISCODE_AUTH_FILE_NAME: &str = "auth.json";
/// 复用 WisGPT 的每日额度接口，不查询其他模型的额度桶。
pub const WISGPT_DAILY_QUOTA_PATH: &str = "/api/llm/daily-token";
/// 菜单栏取数不能长时间等待，也不做自动重试。
pub const WISGPT_QUOTA_TIMEOUT_SECS: u64 = 10;

/// 固定自签名证书，仅供本地 HTTPS 回归服务器使用，不属于用户认证配置。
#[cfg(test)]
pub const WISGPT_TEST_CERT_DER_BASE64: &str = "MIIBfjCCASOgAwIBAgIUKOcUKHeQ9GGCV6qPgWOjJOggxWswCgYIKoZIzj0EAwIwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MTExMTA5NTkwNVoXDTM2MTEwODA5NTkwNVowFDESMBAGA1UEAwwJbG9jYWxob3N0MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEESHwQyloa9LsbX/3HOOgHi4vDQDPNN8+G1J5iNuiRx7Wa1b9JKK07kCPbgmqpjwhlCnCObBqNSB3xmeIJ3/L2qNTMFEwHQYDVR0OBBYEFCQ4gmS6eviW3coZkGTyEJpdHuU5MB8GA1UdIwQYMBaAFCQ4gmS6eviW3coZkGTyEJpdHuU5MA8GA1UdEwEB/wQFMAMBAf8wCgYIKoZIzj0EAwIDSQAwRgIhAKiH1YNdsNv3pgW1yT8z0TcQaV31ULtWag5IsK1mNSl4AiEAwWAZ4T8UEW6uomb7Pwdx/SAYWyXKUmzOGWwcSogPPlg=";
/// 对应上面公开测试证书的固定测试私钥；不能用于生产 TLS 或账号认证。
#[cfg(test)]
pub const WISGPT_TEST_KEY_DER_BASE64: &str = "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg16fNuqz2hVVKlEc6P3IVL0jx+xk/ZqmWXjwQpIYKJCGhRANCAAQRIfBDKWhr0uxtf/cc46AeLi8NAM803z4bUnmI26JHHtZrVv0korTuQI9uCaqmPCGUKcI5sGo1IHfGZ4gnf8va";
