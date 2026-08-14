//! Codex `/responses` 链路的诊断 body dump 工具。
//!
//! 通过环境变量 `CC_SWITCH_DUMP_BODY=1` 打开；不开启时 [`BodyDumper::try_new`]
//! 返回 `None`，调用方保持零开销。
//!
//! 每个请求写入一个独立文件：`<app_config_dir>/logs/proxy-bodies/<profile-id>/<ts>-<request_id>.log`，
//! 追加写入以下内容：
//! 1. 客户端 → CC Switch 的方法、URL、脱敏后的 header、请求 body 原文；
//! 2. CC Switch → 上游的 URL、脱敏后的 header、发送前定稿的 body；
//! 3. 上游 → CC Switch 的状态码、脱敏后的 header、非流式 body 原文
//!    或流式 SSE 采样（前 [`SSE_HEAD_EVENTS`] 个事件 + 末 [`SSE_TAIL_EVENTS`] 个事件）。
//!
//! Header 中的敏感字段（`authorization` / `api-key` / `cookie` 等）会被替换为
//! `***REDACTED***`。请求 / 响应 body 本身不脱敏，因为对话内容是排查所依赖的
//! 原始信号；导出日志前请自行清理敏感段。

use axum::http::HeaderMap;
use serde_json::Value;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// SSE 采样窗口：保留最开始的若干个事件用于观察请求生命周期起始行为。
const SSE_HEAD_EVENTS: usize = 200;
/// SSE 采样窗口：保留最后若干个事件用于观察异常结束时的上下文。
const SSE_TAIL_EVENTS: usize = 50;
/// body dump 文件扩展名。
const DUMP_LOG_EXTENSION: &str = "log";
/// body dump 文件名中的日期前缀长度（YYYYMMDD）。
const DUMP_DATE_KEY_LEN: usize = 8;
/// 需要脱敏的 header 名（大小写不敏感匹配）。
const REDACTED_HEADERS: &[&str] = &[
    "authorization",
    "api-key",
    "x-api-key",
    "x-goog-api-key",
    "cookie",
    "set-cookie",
    "proxy-authorization",
    "openai-organization",
    "chatgpt-account-id",
];

/// 编译期常量：构建时由 build.rs 通过 cargo:rustc-env 注入，未设置时默认 "0"（关闭）。
const DUMP_ENABLED: bool = {
    let val = env!("CC_SWITCH_DUMP_BODY");
    // const 上下文中 PartialEq / eq_ignore_ascii_case 尚未稳定，
    // 因此用 const fn 辅助做字节级比较。
    const fn is_zero(s: &str) -> bool {
        let b = s.as_bytes();
        b.len() == 1 && b[0] == b'0'
    }
    const fn is_false_ignore_case(s: &str) -> bool {
        let b = s.as_bytes();
        if b.len() != 5 {
            return false;
        }
        b[0].to_ascii_lowercase() == b'f'
            && b[1].to_ascii_lowercase() == b'a'
            && b[2].to_ascii_lowercase() == b'l'
            && b[3].to_ascii_lowercase() == b's'
            && b[4].to_ascii_lowercase() == b'e'
    }

    !val.is_empty() && !is_zero(val) && !is_false_ignore_case(val)
};

/// 是否启用 body dump。
#[inline]
pub fn is_enabled() -> bool {
    DUMP_ENABLED
}

/// 单个请求生命周期的 body 落盘器。
///
/// 内部持有一个 `Mutex<File>`，允许在请求生命周期内的多个阶段追加写入
/// （客户端请求、上游请求、上游响应）而不会互相截断。
pub struct BodyDumper {
    inner: Arc<Mutex<File>>,
    request_id: String,
}

impl BodyDumper {
    /// 仅在开关打开时创建 dumper；否则返回 `None`，调用方保持零成本。
    ///
    /// * `request_id` — 本次请求的追踪 ID，会写入文件名和文件头。
    /// * `endpoint` — 触发本次请求的端点（例如 `/responses`），仅用于日志头部注释。
    pub fn try_new(request_id: &str, endpoint: &str) -> Option<Arc<Self>> {
        if !is_enabled() {
            return None;
        }
        Self::try_new_inner(request_id, endpoint, None)
            .ok()
            .map(Arc::new)
    }

    /// 为指定 Codex Profile 创建诊断器，Profile 标识会成为隔离后的日志目录名。
    pub fn try_new_for_profile(
        profile_id: &str,
        request_id: &str,
        endpoint: &str,
    ) -> Option<Arc<Self>> {
        if !is_enabled() {
            return None;
        }
        Self::try_new_inner(request_id, endpoint, Some(profile_id))
            .ok()
            .map(Arc::new)
    }

    /// 内部构造：单独抽出便于错误处理，避免调用点被 IO 错误污染。
    fn try_new_inner(
        request_id: &str,
        endpoint: &str,
        profile_id: Option<&str>,
    ) -> std::io::Result<Self> {
        let dir = dump_dir(profile_id)?;
        Self::try_new_in_directory(request_id, endpoint, &dir)
    }

    /// 在已准备好的目录中创建请求级 dumper，不执行历史日志维护。
    fn try_new_in_directory(
        request_id: &str,
        endpoint: &str,
        dir: &std::path::Path,
    ) -> std::io::Result<Self> {
        let now = chrono::Local::now();
        let file_name = format!("{}-{}.log", now.format("%Y%m%d-%H%M%S"), request_id);
        let path: PathBuf = dir.join(file_name);
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(
            file,
            "# CC Switch body dump\n\
             # request_id: {request_id}\n\
             # endpoint: {endpoint}\n\
             # created_at: {}\n",
            now.format("%Y-%m-%d %H:%M:%S%.3f %:z"),
        )?;
        Ok(Self {
            inner: Arc::new(Mutex::new(file)),
            request_id: request_id.to_string(),
        })
    }

    /// 请求 ID（供外部关联）。
    #[allow(dead_code)]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// 记录客户端 → CC Switch 的请求。
    ///
    /// * `method` — HTTP 方法字符串（例如 `POST`）。
    /// * `uri` — 完整 URI 字符串（含 query）。
    /// * `headers` — 客户端上送的 header 集合，会经过脱敏。
    /// * `body_bytes` — 客户端上送的原始 body 字节；若能解析为 JSON 会 pretty 输出。
    pub fn dump_client_request(
        &self,
        method: &str,
        uri: &str,
        headers: &HeaderMap,
        body_bytes: &[u8],
    ) {
        let mut section = String::new();
        section.push_str("===== [Client → CC Switch] Request =====\n");
        section.push_str(&format!("Method: {method}\n"));
        section.push_str(&format!("URI: {uri}\n"));
        section.push_str("Headers:\n");
        section.push_str(&format_headers(headers));
        section.push_str("\nBody:\n");
        section.push_str(&format_body(body_bytes));
        section.push_str("\n\n");
        self.write_section(&section);
    }

    /// 记录 CC Switch → 上游的请求。
    ///
    /// * `url` — 出站请求的最终 URL 字符串。
    /// * `headers` — 已定稿的出站 header 集合，会经过脱敏。
    /// * `body_bytes` — 已定稿的出站 body 字节；若能解析为 JSON 会 pretty 输出。
    pub fn dump_upstream_request(&self, url: &str, headers: &HeaderMap, body_bytes: &[u8]) {
        let mut section = String::new();
        section.push_str("===== [CC Switch → Upstream] Request =====\n");
        section.push_str(&format!("URL: {url}\n"));
        section.push_str("Headers:\n");
        section.push_str(&format_headers(headers));
        section.push_str("\nBody:\n");
        section.push_str(&format_body(body_bytes));
        section.push_str("\n\n");
        self.write_section(&section);
    }

    /// 记录上游返回的非流式响应。
    ///
    /// * `status` — HTTP 状态码。
    /// * `headers` — 响应 header，会经过脱敏。
    /// * `body_bytes` — 响应 body 字节（如果调用方已解压则传解压后的内容）。
    pub fn dump_upstream_response_non_streaming(
        &self,
        status: u16,
        headers: &HeaderMap,
        body_bytes: &[u8],
    ) {
        let mut section = String::new();
        section.push_str("===== [Upstream → CC Switch] Response (non-streaming) =====\n");
        section.push_str(&format!("Status: {status}\n"));
        section.push_str("Headers:\n");
        section.push_str(&format_headers(headers));
        section.push_str("\nBody:\n");
        section.push_str(&format_body(body_bytes));
        section.push_str("\n\n");
        self.write_section(&section);
    }

    /// 记录上游返回的 SSE 采样。
    ///
    /// * `status` — HTTP 状态码。
    /// * `headers` — 响应 header，会经过脱敏。
    /// * `head_events` — 前 [`SSE_HEAD_EVENTS`] 个事件文本。
    /// * `tail_events` — 后 [`SSE_TAIL_EVENTS`] 个事件文本。
    /// * `total_events` — 采样期间累计的完整事件数（用于评估中间被丢弃了多少）。
    pub fn dump_upstream_response_sse(
        &self,
        status: u16,
        headers: &HeaderMap,
        head_events: &[String],
        tail_events: &[String],
        total_events: usize,
    ) {
        let mut section = String::new();
        section.push_str("===== [Upstream → CC Switch] Response (SSE sampled) =====\n");
        section.push_str(&format!("Status: {status}\n"));
        section.push_str(&format!(
            "Total events observed: {total_events} (head kept: {}, tail kept: {})\n",
            head_events.len(),
            tail_events.len()
        ));
        section.push_str("Headers:\n");
        section.push_str(&format_headers(headers));
        section.push_str("\n---- Head events ----\n");
        for evt in head_events {
            section.push_str(evt);
            if !evt.ends_with('\n') {
                section.push('\n');
            }
            section.push_str("----\n");
        }
        if total_events > head_events.len() + tail_events.len() {
            section.push_str(&format!(
                "... (omitted {} middle events) ...\n",
                total_events - head_events.len() - tail_events.len()
            ));
        }
        section.push_str("---- Tail events ----\n");
        for evt in tail_events {
            section.push_str(evt);
            if !evt.ends_with('\n') {
                section.push('\n');
            }
            section.push_str("----\n");
        }
        section.push_str("\n\n");
        self.write_section(&section);
    }

    /// 追加一段文本到 dump 文件。IO 错误只落警告日志，不影响主流程。
    fn write_section(&self, section: &str) {
        if let Ok(mut file) = self.inner.lock() {
            if let Err(e) = file.write_all(section.as_bytes()) {
                log::warn!("[BodyDump] 写入 dump 文件失败: {e}");
            }
        }
    }
}

/// 计算 dump 文件所在目录，Profile 路由使用隔离后的子目录。
fn dump_dir(profile_id: Option<&str>) -> std::io::Result<PathBuf> {
    let base = body_dump_root();
    let dir = profile_id
        .map(sanitize_profile_id)
        .map(|profile_id| base.join(profile_id))
        .unwrap_or(base);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 返回 body dump 日志树根目录，不主动创建目录。
fn body_dump_root() -> PathBuf {
    crate::panic_hook::get_log_dir().join(crate::constants::BODY_DUMP_DIRECTORY_NAME)
}

/// 将 Profile 标识净化为安全、稳定的单层目录名。
fn sanitize_profile_id(profile_id: &str) -> String {
    let sanitized: String = profile_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();

    if sanitized.is_empty() {
        "unknown-profile".to_string()
    } else {
        sanitized
    }
}

// 清理早于今天的 body dump 日志；失败只记录警告，不影响代理主流程。

/// Body dump 树级清理的可观察结果。
#[derive(Debug, Default)]
pub struct BodyDumpCleanupSummary {
    removed_files: usize,
    error_count: usize,
    error_samples: Vec<String>,
}

impl BodyDumpCleanupSummary {
    /// 记录非幂等文件系统错误，并限制保存的样本数量。
    fn record_error(&mut self, operation: &str, path: &std::path::Path, error: &std::io::Error) {
        if error.kind() == std::io::ErrorKind::NotFound {
            return;
        }
        self.error_count = self.error_count.saturating_add(1);
        if self.error_samples.len() < crate::constants::BODY_DUMP_CLEANUP_ERROR_SAMPLE_LIMIT {
            self.error_samples
                .push(format!("{operation} {}: {error}", path.display()));
        }
    }

    /// 在一次清理结束后输出单条有界错误摘要。
    fn log_errors(&self) {
        if self.error_count == 0 {
            return;
        }
        log::warn!(
            "[BodyDump] 全局历史日志清理部分失败: errors={}, samples=[{}]",
            self.error_count,
            self.error_samples.join(" | ")
        );
    }
}

/// 清理日志根层及恰好一层真实 Profile 目录中的过期 body dump 日志。
pub fn cleanup_body_dump_tree(root: &std::path::Path, today_key: &str) -> BodyDumpCleanupSummary {
    let mut summary = BodyDumpCleanupSummary::default();
    cleanup_dump_directory(root, today_key, true, &mut summary);
    summary.log_errors();
    summary
}

/// 在 blocking task 中执行一次应用级 body dump 维护。
pub(crate) async fn run_body_dump_maintenance() {
    let root = body_dump_root();
    let today_key = chrono::Local::now()
        .format(crate::constants::BODY_DUMP_DATE_FORMAT)
        .to_string();
    run_body_dump_maintenance_at(root, today_key).await;
}

/// 对指定日志根目录执行 maintenance tick，供隔离目录测试复用。
pub(crate) async fn run_body_dump_maintenance_at(root: PathBuf, today_key: String) {
    let task = tauri::async_runtime::spawn_blocking(move || {
        cleanup_body_dump_tree(&root, &today_key);
    });
    if let Err(error) = task.await {
        log::warn!("[BodyDump] 全局历史日志清理任务异常结束: {error}");
    }
}

/// 清理指定目录的一层普通日志，并按需进入其真实子目录一次。
fn cleanup_dump_directory(
    dir: &std::path::Path,
    today_key: &str,
    visit_profile_directories: bool,
    summary: &mut BodyDumpCleanupSummary,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            summary.record_error("读取目录失败", dir, &error);
            return;
        }
    };

    for entry_result in entries {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(error) => {
                summary.record_error("读取目录项失败", dir, &error);
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                summary.record_error("读取文件类型失败", &path, &error);
                continue;
            }
        };
        if file_type.is_file() {
            remove_expired_dump_file(&path, today_key, summary);
        } else if visit_profile_directories && file_type.is_dir() {
            cleanup_dump_directory(&path, today_key, false, summary);
        }
    }
}

/// 删除可确认早于今天的标准 body dump 普通文件。
fn remove_expired_dump_file(
    path: &std::path::Path,
    today_key: &str,
    summary: &mut BodyDumpCleanupSummary,
) {
    let Some(date_key) = dump_file_date_key(path) else {
        return;
    };
    if date_key >= today_key {
        return;
    }
    match std::fs::remove_file(path) {
        Ok(()) => summary.removed_files = summary.removed_files.saturating_add(1),
        Err(error) => summary.record_error("删除文件失败", path, &error),
    }
}

/// 从 dump 文件名中提取 `YYYYMMDD` 日期键；无法确认是 dump log 时返回 `None`。
fn dump_file_date_key(path: &std::path::Path) -> Option<&str> {
    if path.extension().and_then(|ext| ext.to_str()) != Some(DUMP_LOG_EXTENSION) {
        return None;
    }

    let file_name = path.file_name()?.to_str()?;
    if file_name.len() <= DUMP_DATE_KEY_LEN
        || file_name.as_bytes().get(DUMP_DATE_KEY_LEN) != Some(&b'-')
    {
        return None;
    }

    let date_key = &file_name[..DUMP_DATE_KEY_LEN];
    if date_key.bytes().all(|b| b.is_ascii_digit())
        && chrono::NaiveDate::parse_from_str(date_key, crate::constants::BODY_DUMP_DATE_FORMAT)
            .is_ok()
    {
        Some(date_key)
    } else {
        None
    }
}

/// 把 header 集合格式化为多行字符串；敏感字段替换为 `***REDACTED***`。
fn format_headers(headers: &HeaderMap) -> String {
    let mut out = String::new();
    for (name, value) in headers.iter() {
        let name_str = name.as_str();
        let display = if REDACTED_HEADERS
            .iter()
            .any(|k| k.eq_ignore_ascii_case(name_str))
        {
            "***REDACTED***".to_string()
        } else {
            value
                .to_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|_| "<non-utf8>".to_string())
        };
        out.push_str(&format!("  {name_str}: {display}\n"));
    }
    if out.is_empty() {
        out.push_str("  (empty)\n");
    }
    out
}

/// 尝试把字节流当作 JSON pretty 输出；失败则按 UTF-8 lossy 原文写入。
fn format_body(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "(empty body)".to_string();
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(value) => serde_json::to_string_pretty(&value)
            .unwrap_or_else(|_| String::from_utf8_lossy(bytes).into_owned()),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// SSE 事件采样器：只保留前 [`SSE_HEAD_EVENTS`] 个和最后 [`SSE_TAIL_EVENTS`] 个事件。
///
/// 中间事件按顺序被丢弃，避免占用大量内存；总数量单独计数用于事后回顾。
pub struct SseSampler {
    head: Vec<String>,
    tail: VecDeque<String>,
    total: usize,
}

impl SseSampler {
    /// 创建空采样器。
    pub fn new() -> Self {
        Self {
            head: Vec::with_capacity(SSE_HEAD_EVENTS),
            tail: VecDeque::with_capacity(SSE_TAIL_EVENTS),
            total: 0,
        }
    }

    /// 记录一个完整事件文本；调用方负责拆分完整 SSE block。
    pub fn record(&mut self, event: String) {
        self.total = self.total.saturating_add(1);
        if self.head.len() < SSE_HEAD_EVENTS {
            self.head.push(event);
            return;
        }
        if self.tail.len() == SSE_TAIL_EVENTS {
            self.tail.pop_front();
        }
        self.tail.push_back(event);
    }

    /// 转成 (head, tail, total) 三元组。
    pub fn finish(self) -> (Vec<String>, Vec<String>, usize) {
        (self.head, self.tail.into_iter().collect(), self.total)
    }
}

impl Default for SseSampler {
    fn default() -> Self {
        Self::new()
    }
}

/// 把字节缓冲区中所有已成形的 SSE 事件（以 `\n\n` 分隔）取出。
///
/// 返回值为完整事件的文本列表；剩余不完整的字节保留在 `buffer` 里等待下一次调用。
pub fn drain_sse_events(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut events = Vec::new();
    loop {
        let Some(pos) = find_event_boundary(buffer) else {
            break;
        };
        // pos 是 "\n\n" 起点；把事件文本取出（不含分隔符），并把分隔符从 buffer 头部剥掉。
        let block: Vec<u8> = buffer.drain(..pos).collect();
        // 剥掉紧随其后的 "\n\n"（长度固定 2）。
        buffer.drain(..2);
        // SSE 允许 `\r\n\r\n`；额外兼容一下 CR。
        let text = String::from_utf8_lossy(&block).into_owned();
        events.push(text.trim_end_matches('\r').to_string());
    }
    events
}

/// 找到缓冲区中首个 `"\n\n"` 的位置。
fn find_event_boundary(buffer: &[u8]) -> Option<usize> {
    buffer.windows(2).position(|w| w == b"\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    /// 敏感 header 应该被替换，普通 header 保持原样。
    #[test]
    fn format_headers_redacts_sensitive_keys() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_static("Bearer sk-secret"),
        );
        headers.insert(
            HeaderName::from_static("x-request-id"),
            HeaderValue::from_static("abc"),
        );
        let rendered = format_headers(&headers);
        assert!(rendered.contains("authorization: ***REDACTED***"));
        assert!(rendered.contains("x-request-id: abc"));
    }

    /// 合法 JSON 会被 pretty 输出。
    #[test]
    fn format_body_pretty_prints_json() {
        let out = format_body(br#"{"a":1,"b":[2,3]}"#);
        assert!(out.contains("\"a\": 1"));
        assert!(out.contains("\"b\": [\n"));
    }

    /// 非 JSON 保持原文。
    #[test]
    fn format_body_falls_back_to_lossy_utf8() {
        let out = format_body(b"not json");
        assert_eq!(out, "not json");
    }

    /// dump 文件名应从前 8 位提取日期键。
    #[test]
    fn dump_file_date_key_extracts_yyyymmdd_prefix() {
        let path = std::path::Path::new("20260707-235959-request.log");
        assert_eq!(dump_file_date_key(path), Some("20260707"));
    }

    /// 非 log 文件不参与清理。
    #[test]
    fn dump_file_date_key_ignores_non_log_files() {
        let path = std::path::Path::new("20260707-235959-request.txt");
        assert_eq!(dump_file_date_key(path), None);
    }

    /// 不符合日期前缀格式的 log 文件不参与清理。
    #[test]
    fn dump_file_date_key_ignores_malformed_log_names() {
        let path = std::path::Path::new("body-dump.log");
        assert_eq!(dump_file_date_key(path), None);
    }

    /// Profile 标识必须被净化为单个稳定目录名，不能逃逸到日志根目录外。
    #[test]
    fn sanitize_profile_id_blocks_path_separator_and_control_chars() {
        assert_eq!(sanitize_profile_id("work/profile\\a\n"), "work_profile_a_");
    }

    /// 清理策略删除早于今天的 dump log，保留今天和未来日期的 dump log。
    #[test]
    fn cleanup_body_dump_tree_removes_only_logs_before_today() {
        let dir = tempfile::tempdir().expect("tempdir");
        let old = dir.path().join("20260707-235959-old.log");
        let today = dir.path().join("20260708-000001-today.log");
        let future = dir.path().join("20260709-000001-future.log");
        let malformed = dir.path().join("body-dump.log");
        let note = dir.path().join("20260707-235959-note.txt");

        std::fs::write(&old, "old").expect("write old");
        std::fs::write(&today, "today").expect("write today");
        std::fs::write(&future, "future").expect("write future");
        std::fs::write(&malformed, "malformed").expect("write malformed");
        std::fs::write(&note, "note").expect("write note");

        cleanup_body_dump_tree(dir.path(), "20260708");

        assert!(!old.exists());
        assert!(today.exists());
        assert!(future.exists());
        assert!(malformed.exists());
        assert!(note.exists());
    }

    /// maintenance tick 失败后不得阻断下一轮对历史日志的收敛。
    #[tokio::test]
    async fn body_dump_maintenance_tick_recovers_after_a_failed_run() {
        let parent = tempfile::tempdir().expect("tempdir");
        let root = parent.path().join("proxy-bodies");
        std::fs::write(&root, "not a directory").expect("write blocking root fixture");

        run_body_dump_maintenance_at(root.clone(), "20260708".to_string()).await;
        assert!(root.is_file());

        std::fs::remove_file(&root).expect("remove blocking root fixture");
        let inactive_profile = root.join("inactive-profile");
        std::fs::create_dir_all(&inactive_profile).expect("create inactive profile");
        let legacy_old = root.join("20260707-235959-legacy.log");
        let inactive_old = inactive_profile.join("20260707-235959-inactive.log");
        std::fs::write(&legacy_old, "old").expect("write legacy fixture");
        std::fs::write(&inactive_old, "old").expect("write inactive fixture");

        run_body_dump_maintenance_at(root, "20260708".to_string()).await;

        assert_eq!((legacy_old.exists(), inactive_old.exists()), (false, false));
    }

    /// 创建请求级 dumper 只能创建当天日志，不得顺带扫描和删除历史日志。
    #[test]
    fn body_dumper_creation_does_not_cleanup_history() {
        let dir = tempfile::tempdir().expect("tempdir");
        let old = dir.path().join("20260707-235959-old.log");
        std::fs::write(&old, "old").expect("write old fixture");

        let dumper =
            BodyDumper::try_new_in_directory("request-without-retention", "/responses", dir.path())
                .expect("create dumper");

        assert!(old.exists());
        assert_eq!(dumper.request_id(), "request-without-retention");
    }

    /// 一次树级清理应同时收敛根层和所有一层 Profile 目录中的过期日志。
    #[test]
    fn cleanup_body_dump_tree_removes_expired_logs_from_root_and_profiles() {
        let root = tempfile::tempdir().expect("tempdir");
        let inactive_profile = root.path().join("inactive-profile");
        let deleted_profile = root.path().join("deleted-profile");
        std::fs::create_dir_all(&inactive_profile).expect("create inactive profile");
        std::fs::create_dir_all(&deleted_profile).expect("create deleted profile");

        let root_old = root.path().join("20260707-235959-root.log");
        let inactive_old = inactive_profile.join("20260707-235959-inactive.log");
        let deleted_old = deleted_profile.join("20260707-235959-deleted.log");
        std::fs::write(&root_old, "old").expect("write root old");
        std::fs::write(&inactive_old, "old").expect("write inactive old");
        std::fs::write(&deleted_old, "old").expect("write deleted old");

        cleanup_body_dump_tree(root.path(), "20260708");

        assert_eq!(
            [
                root_old.exists(),
                inactive_old.exists(),
                deleted_old.exists()
            ],
            [false, false, false]
        );
    }

    /// 树级清理必须保留未过期、无法确认归属及超过固定深度的对象。
    #[test]
    fn cleanup_body_dump_tree_preserves_nonexpired_and_unowned_entries() {
        let root = tempfile::tempdir().expect("tempdir");
        let profile = root.path().join("profile-a");
        let nested = profile.join("nested");
        let empty = root.path().join("empty-profile");
        std::fs::create_dir_all(&nested).expect("create nested directory");
        std::fs::create_dir_all(&empty).expect("create empty directory");

        let today = root.path().join("20260708-000001-today.log");
        let future = profile.join("20260709-000001-future.log");
        let invalid_date = root.path().join("20260230-000001-invalid.log");
        let malformed = profile.join("body-dump.log");
        let other_extension = profile.join("20260707-000001-note.txt");
        let deeply_nested = nested.join("20260707-000001-deep.log");
        for path in [
            &today,
            &future,
            &invalid_date,
            &malformed,
            &other_extension,
            &deeply_nested,
        ] {
            std::fs::write(path, "keep").expect("write retained fixture");
        }

        cleanup_body_dump_tree(root.path(), "20260708");

        assert_eq!(
            [
                today.exists(),
                future.exists(),
                invalid_date.exists(),
                malformed.exists(),
                other_extension.exists(),
                deeply_nested.exists(),
                empty.exists(),
            ],
            [true, true, true, true, true, true, true]
        );
    }

    /// 单个 Profile 无法读取时应继续清理其它 Profile，并只保留有限错误样本。
    #[cfg(unix)]
    #[test]
    fn cleanup_body_dump_tree_reports_bounded_errors_and_continues() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("tempdir");
        let accessible = root.path().join("accessible");
        std::fs::create_dir(&accessible).expect("create accessible profile");
        let removable = accessible.join("20260707-000001-old.log");
        std::fs::write(&removable, "old").expect("write removable fixture");

        let mut inaccessible_profiles = Vec::new();
        for index in 0..7 {
            let profile = root.path().join(format!("inaccessible-{index}"));
            std::fs::create_dir(&profile).expect("create inaccessible profile");
            std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o000))
                .expect("restrict profile permissions");
            inaccessible_profiles.push(profile);
        }

        let summary = cleanup_body_dump_tree(root.path(), "20260708");

        for profile in &inaccessible_profiles {
            std::fs::set_permissions(profile, std::fs::Permissions::from_mode(0o700))
                .expect("restore profile permissions");
        }
        assert_eq!(
            (
                removable.exists(),
                summary.error_count,
                summary.error_samples.len()
            ),
            (false, 7, 5)
        );
    }

    /// 根目录不存在时清理应安静结束，且不能创建目录。
    #[test]
    fn cleanup_body_dump_tree_does_not_create_missing_root() {
        let parent = tempfile::tempdir().expect("tempdir");
        let missing_root = parent.path().join("missing-proxy-bodies");

        let summary = cleanup_body_dump_tree(&missing_root, "20260708");

        assert_eq!((missing_root.exists(), summary.error_count), (false, 0));
    }

    /// 清理不得跟随文件或 Profile 目录 symlink，也不得删除特殊文件。
    #[cfg(unix)]
    #[test]
    fn cleanup_body_dump_tree_skips_symlinks_and_special_files() {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;

        let root = tempfile::tempdir().expect("tempdir");
        let external = tempfile::tempdir().expect("external tempdir");
        let external_old = external.path().join("20260707-000001-external.log");
        std::fs::write(&external_old, "keep").expect("write external fixture");

        let linked_profile = root.path().join("linked-profile");
        let linked_file = root.path().join("20260707-000001-linked.log");
        symlink(external.path(), &linked_profile).expect("link profile directory");
        symlink(&external_old, &linked_file).expect("link dump file");
        let socket_path = root.path().join("20260707-000001-socket.log");
        let _socket = UnixListener::bind(&socket_path).expect("bind unix socket");

        cleanup_body_dump_tree(root.path(), "20260708");

        assert_eq!(
            [
                linked_profile.exists(),
                linked_file.exists(),
                socket_path.exists(),
                external_old.exists(),
            ],
            [true, true, true, true]
        );
    }

    /// 删除失败不能阻止同目录后续项及其它 Profile 的清理。
    #[cfg(unix)]
    #[test]
    fn cleanup_body_dump_tree_continues_after_delete_errors() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("tempdir");
        let read_only = root.path().join("read-only");
        let accessible = root.path().join("accessible");
        std::fs::create_dir(&read_only).expect("create read-only profile");
        std::fs::create_dir(&accessible).expect("create accessible profile");
        let blocked_a = read_only.join("20260707-000001-blocked-a.log");
        let blocked_b = read_only.join("20260707-000002-blocked-b.log");
        let removable = accessible.join("20260707-000001-removable.log");
        std::fs::write(&blocked_a, "keep").expect("write blocked fixture a");
        std::fs::write(&blocked_b, "keep").expect("write blocked fixture b");
        std::fs::write(&removable, "old").expect("write removable fixture");
        std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o500))
            .expect("restrict delete permissions");

        let summary = cleanup_body_dump_tree(root.path(), "20260708");

        std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o700))
            .expect("restore delete permissions");
        assert_eq!(
            (
                blocked_a.exists(),
                blocked_b.exists(),
                removable.exists(),
                summary.error_count,
            ),
            (true, true, false, 2)
        );
    }

    /// 前 N 个事件全部保留，超出部分挤入末尾环形缓冲；中间被丢弃但总数正确。
    #[test]
    fn sse_sampler_keeps_head_and_tail() {
        let head = SSE_HEAD_EVENTS;
        let tail = SSE_TAIL_EVENTS;
        let extra = 20;
        let total = head + tail + extra;

        let mut sampler = SseSampler::new();
        for i in 0..total {
            sampler.record(format!("event-{i}"));
        }
        let (h, t, n) = sampler.finish();
        assert_eq!(n, total);
        assert_eq!(h.len(), head);
        assert_eq!(t.len(), tail);
        assert_eq!(h.first().map(String::as_str), Some("event-0"));
        assert_eq!(
            t.last().map(String::as_str),
            Some(&*format!("event-{}", total - 1))
        );
    }

    /// 缓冲区中的完整事件被抽取；未闭合部分保留待续。
    #[test]
    fn drain_sse_events_splits_by_double_newline() {
        let mut buf: Vec<u8> =
            b"event: a\ndata: 1\n\nevent: b\ndata: 2\n\nevent: c\ndata: 3".to_vec();
        let events = drain_sse_events(&mut buf);
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("event: a"));
        assert!(events[1].contains("event: b"));
        assert_eq!(String::from_utf8(buf).unwrap(), "event: c\ndata: 3");
    }
}
