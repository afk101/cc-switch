// Provider 类型常量
export const PROVIDER_TYPES = {
  GITHUB_COPILOT: "github_copilot",
  CODEX_OAUTH: "codex_oauth",
} as const;

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

// Codex 供应商需要本地路由的原因
export const CODEX_PROVIDER_ROUTE_REQUIREMENTS = {
  OPENAI_CHAT: "openaiChat",
  FULL_URL: "fullUrl",
} as const;

export type CodexProviderRouteRequirement =
  (typeof CODEX_PROVIDER_ROUTE_REQUIREMENTS)[keyof typeof CODEX_PROVIDER_ROUTE_REQUIREMENTS];
