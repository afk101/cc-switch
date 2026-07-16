//! Codex Profile 的领域常量。

/// 默认 Codex Profile 的稳定标识。
pub const DEFAULT_CODEX_PROFILE_ID: &str = "codex-default";
/// 默认 Codex Profile 的显示名称。
pub const DEFAULT_CODEX_PROFILE_NAME: &str = "默认 Codex";
/// 旧版 Codex 路由沿用的监听端口。
pub const LEGACY_CODEX_ROUTE_PORT: u16 = 15_721;
/// 新建 Codex Profile 自动分配端口的起始值。
pub const FIRST_CUSTOM_CODEX_ROUTE_PORT: u16 = 15_722;
/// Codex Profile 可使用的最小监听端口。
pub const MIN_CODEX_ROUTE_PORT: u16 = 1;
/// Codex 本地路由仅允许监听 loopback 地址。
pub const CODEX_ROUTE_LISTEN_HOST: &str = "127.0.0.1";
/// Codex 当前模型供应商字段。
pub const CODEX_MODEL_PROVIDER_FIELD: &str = "model_provider";
/// Codex 模型供应商配置父表。
pub const CODEX_MODEL_PROVIDERS_TABLE: &str = "model_providers";
/// Codex Profile 路由接管的服务地址字段。
pub const CODEX_ROUTE_FIELD_BASE_URL: &str = "base_url";
/// Codex Profile 路由接管的协议字段。
pub const CODEX_ROUTE_FIELD_WIRE_API: &str = "wire_api";
/// Codex Profile 路由接管的本地凭证字段。
pub const CODEX_ROUTE_FIELD_BEARER_TOKEN: &str = "experimental_bearer_token";
/// Codex Profile 本地路由接收的固定 wire API。
pub const CODEX_ROUTE_WIRE_API_RESPONSES: &str = "responses";
/// listener token 缺失或不匹配时使用的脱敏描述。
pub const CODEX_ROUTE_TOKEN_MISMATCH_DETAIL: &str = "本地路由凭证缺失或不匹配";
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
/// 生命周期操作已经持久化，尚未执行运行时副作用。
pub const CODEX_ROUTE_RECOVERY_PHASE_PREPARED: &str = "prepared";
/// 启用运行时已启动，尚未完成路由持久化。
pub const CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED: &str = "enable_started";
/// 启用 Home 已写入，尚未完成路由持久化。
pub const CODEX_ROUTE_RECOVERY_PHASE_ENABLE_HOME_APPLIED: &str = "enable_home_applied";
/// 启用已改动 Home 或运行时但路由持久化失败，必须先人工或自动收敛。
pub const CODEX_ROUTE_RECOVERY_PHASE_ENABLE_PERSIST_FAILED: &str = "enable_persist_failed";
/// 关闭已恢复 Home 但监听器停止失败，后续操作必须先完成关闭。
pub const CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOP_FAILED: &str = "disable_stop_failed";
/// 关闭时 Home 恢复失败，后续操作必须先重试恢复 Home 再停止监听器。
pub const CODEX_ROUTE_RECOVERY_PHASE_DISABLE_HOME_RESTORE_FAILED: &str =
    "disable_home_restore_failed";
/// 删除已移除本地凭证但数据库删除失败，后续操作必须先补建凭证。
pub const CODEX_ROUTE_RECOVERY_PHASE_DELETE_DATABASE_FAILED: &str = "delete_database_failed";
/// 关闭运行时已停止，尚未将关闭状态保存到数据库。
pub const CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOPPED: &str = "disable_stopped";
/// 删除本地凭证前的持久化阶段。
pub const CODEX_ROUTE_RECOVERY_PHASE_DELETE_TOKEN_PENDING: &str = "delete_token_pending";
/// 切换运行时快照已替换，尚未写入目标路由。
pub const CODEX_ROUTE_RECOVERY_PHASE_SWITCH_RUNTIME_SWAPPED: &str = "switch_runtime_swapped";
/// 切换目标路由已保存，尚未替换故障转移列表。
pub const CODEX_ROUTE_RECOVERY_PHASE_SWITCH_ROUTE_SAVED: &str = "switch_route_saved";
/// 切换故障转移列表已替换，尚未清除操作记录。
pub const CODEX_ROUTE_RECOVERY_PHASE_SWITCH_FAILOVERS_SAVED: &str = "switch_failovers_saved";
/// 启动对账的补偿操作名称。
pub const CODEX_ROUTE_RECOVERY_OPERATION_RECONCILE: &str = "reconcile";
/// 启动对账已记录旧、新目标指纹，尚未改写 Home。
pub const CODEX_ROUTE_RECOVERY_PHASE_RECONCILE_PREPARED: &str = "reconcile_prepared";
/// 启动对账已改写 Home，尚未重定位备份目标指纹。
pub const CODEX_ROUTE_RECOVERY_PHASE_RECONCILE_HOME_APPLIED: &str = "reconcile_home_applied";
/// 生命周期补偿尚未收敛时返回给调用方的安全错误前缀。
pub const CODEX_ROUTE_COMPENSATION_UNCONVERGED_ERROR: &str =
    "Codex Profile 路由变更失败，补偿未收敛";
