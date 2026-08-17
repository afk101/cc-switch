// Provider 类型常量
export const PROVIDER_TYPES = {
  GITHUB_COPILOT: "github_copilot",
  CODEX_OAUTH: "codex_oauth",
  XAI_OAUTH: "xai_oauth",
} as const;

// 托管 OAuth 供应商类型：真实凭据由本地代理按请求注入，因此无论上游是否
// 需要格式转换，都必须开启路由接管才能通过认证。新增此类预设时只需把
// providerType 加进本数组，needsRouting 判定即自动覆盖，无需逐个特判。
export const OAUTH_PROVIDER_TYPES: readonly string[] = [
  PROVIDER_TYPES.GITHUB_COPILOT,
  PROVIDER_TYPES.CODEX_OAUTH,
  PROVIDER_TYPES.XAI_OAUTH,
];

/** 判断某 providerType 是否为托管 OAuth（凭据由代理注入、必须开启路由）。 */
export function isOAuthProviderType(
  providerType: string | null | undefined,
): boolean {
  return providerType != null && OAUTH_PROVIDER_TYPES.includes(providerType);
}

// 用量脚本模板类型常量
export const TEMPLATE_TYPES = {
  CUSTOM: "custom",
  GENERAL: "general",
  NEW_API: "newapi",
  GITHUB_COPILOT: "github_copilot",
  TOKEN_PLAN: "token_plan",
  BALANCE: "balance",
  OFFICIAL_SUBSCRIPTION: "official_subscription",
} as const;

export type TemplateType = (typeof TEMPLATE_TYPES)[keyof typeof TEMPLATE_TYPES];

// 默认 Codex Profile 的稳定标识
export const CODEX_DEFAULT_PROFILE_ID = "codex-default";

// Codex Profile 可使用的监听端口范围
export const CODEX_PROFILE_MIN_PORT = 1;
export const CODEX_PROFILE_MAX_PORT = 65_535;

// Codex Profile 显式同步公开原因码
export const CODEX_PROFILE_SYNC_REASON_CODES = {
  ROUTE_MISSING: "route_missing",
  PRIMARY_MISSING: "primary_missing",
  PROVIDER_UNAVAILABLE: "provider_unavailable",
  STATE_UNAVAILABLE: "state_unavailable",
  PLAN_FAILED: "plan_failed",
  RUNTIME_UNAVAILABLE: "runtime_unavailable",
  APPLY_FAILED: "apply_failed",
} as const;

// Codex 供应商需要本地路由的原因
export const CODEX_PROVIDER_ROUTE_REQUIREMENTS = {
  OPENAI_CHAT: "openaiChat",
  FULL_URL: "fullUrl",
} as const;

export type CodexProviderRouteRequirement =
  (typeof CODEX_PROVIDER_ROUTE_REQUIREMENTS)[keyof typeof CODEX_PROVIDER_ROUTE_REQUIREMENTS];

// Tauri 日志导出命令名称
export const LOG_EXPORT_COMMAND = "exportLogs";

// 日志导出的稳定错误码
export const LOG_EXPORT_ERROR_CODES = {
  NO_LOGS: "NO_LOGS",
} as const;
