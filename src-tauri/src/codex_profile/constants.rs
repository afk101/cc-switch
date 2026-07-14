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
/// Profile 排空已进入请求的最长等待时间，超时后仍停止监听器。
pub const CODEX_ROUTE_DRAIN_TIMEOUT_SECONDS: u64 = 15;
/// 每个本地监听凭证使用的随机字节数。
pub const LOCAL_TOKEN_BYTES: usize = 32;
/// CC Switch 私有密钥根目录下的通用 secrets 目录名。
pub const CODEX_PROFILE_SECRET_PARENT_DIRECTORY: &str = "secrets";
/// Codex Profile 私有密钥目录名。
pub const CODEX_PROFILE_SECRET_DIRECTORY: &str = "codex-profiles";
/// 单个 Profile 本地监听凭证的文件名。
pub const CODEX_PROFILE_TOKEN_FILENAME: &str = "listener-token";
/// 旧单例代理接管配置中使用的兼容占位符。
pub const LEGACY_PROXY_MANAGED_TOKEN: &str = "PROXY_MANAGED";
/// 已迁移旧路由等待创建兼容本地凭证时使用的数据库设置键。
pub const CODEX_LEGACY_TOKEN_PENDING_PROFILE_SETTING: &str =
    "codex_legacy_token_pending_profile_id";
/// 路由切换已持久化准备快照，后续阶段可据此回滚。
pub const CODEX_ROUTE_RECOVERY_PHASE_SWITCH_PREPARED: &str = "switch_prepared";
/// 启用已改动 Home 或运行时但路由持久化失败，必须先人工或自动收敛。
pub const CODEX_ROUTE_RECOVERY_PHASE_ENABLE_PERSIST_FAILED: &str = "enable_persist_failed";
/// 关闭已恢复 Home 但监听器停止失败，后续操作必须先完成关闭。
pub const CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOP_FAILED: &str = "disable_stop_failed";
/// 删除已移除本地凭证但数据库删除失败，后续操作必须先补建凭证。
pub const CODEX_ROUTE_RECOVERY_PHASE_DELETE_DATABASE_FAILED: &str = "delete_database_failed";
