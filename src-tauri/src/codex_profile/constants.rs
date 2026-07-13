//! Codex Profile 的领域常量。

/// 默认 Codex Profile 的稳定标识。
pub const DEFAULT_CODEX_PROFILE_ID: &str = "codex-default";
/// 默认 Codex Profile 的显示名称。
pub const DEFAULT_CODEX_PROFILE_NAME: &str = "默认 Codex";
/// 旧版 Codex 路由沿用的监听端口。
pub const LEGACY_CODEX_ROUTE_PORT: u16 = 15_721;
/// 新建 Codex Profile 自动分配端口的起始值。
pub const FIRST_CUSTOM_CODEX_ROUTE_PORT: u16 = 15_722;
/// Codex 本地路由仅允许监听 loopback 地址。
pub const CODEX_ROUTE_LISTEN_HOST: &str = "127.0.0.1";
/// 每个本地监听凭证使用的随机字节数。
pub const LOCAL_TOKEN_BYTES: usize = 32;
