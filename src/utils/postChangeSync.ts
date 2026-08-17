import { settingsApi } from "@/lib/api";
import type { ProfileSyncWarning } from "@/lib/api/settings";
import type { TFunction } from "i18next";
import { CODEX_PROFILE_SYNC_REASON_CODES } from "@/config/constants";

const PROFILE_SYNC_REASON_TRANSLATION_KEYS: Readonly<Record<string, string>> = {
  [CODEX_PROFILE_SYNC_REASON_CODES.ROUTE_MISSING]:
    "settings.profileSyncWarnings.reasons.routeMissing",
  [CODEX_PROFILE_SYNC_REASON_CODES.PRIMARY_MISSING]:
    "settings.profileSyncWarnings.reasons.primaryMissing",
  [CODEX_PROFILE_SYNC_REASON_CODES.PROVIDER_UNAVAILABLE]:
    "settings.profileSyncWarnings.reasons.providerUnavailable",
  [CODEX_PROFILE_SYNC_REASON_CODES.STATE_UNAVAILABLE]:
    "settings.profileSyncWarnings.reasons.stateUnavailable",
  [CODEX_PROFILE_SYNC_REASON_CODES.PLAN_FAILED]:
    "settings.profileSyncWarnings.reasons.planFailed",
  [CODEX_PROFILE_SYNC_REASON_CODES.RUNTIME_UNAVAILABLE]:
    "settings.profileSyncWarnings.reasons.runtimeUnavailable",
  [CODEX_PROFILE_SYNC_REASON_CODES.APPLY_FAILED]:
    "settings.profileSyncWarnings.reasons.applyFailed",
};

/**
 * 将公开同步原因码转换为本地化文案。
 * 未知原因码与旧版 reason payload 均返回固定脱敏文案，绝不展示后端原文。
 *
 * @param warning 后端返回的结构化 Profile 同步警告
 * @param t i18next 翻译函数
 * @returns 当前界面的本地化脱敏原因
 */
function translateProfileSyncReason(
  warning: ProfileSyncWarning,
  t: TFunction,
): string {
  const translationKey = warning.reasonCode
    ? PROFILE_SYNC_REASON_TRANSLATION_KEYS[warning.reasonCode]
    : undefined;
  return t(translationKey ?? "settings.profileSyncWarnings.reasons.unknown");
}

/**
 * 将逐 Profile 脱敏 warning 格式化为可直接展示的多行本地化文本。
 *
 * @param warnings 后端返回的逐 Profile 同步警告
 * @param t i18next 翻译函数
 * @returns 多行本地化警告；没有警告时返回 undefined
 */
export function formatProfileSyncWarnings(
  warnings: ProfileSyncWarning[] | undefined,
  t: TFunction,
): string | undefined {
  if (!warnings?.length) return undefined;
  return warnings
    .map(
      (warning) =>
        `${warning.profileName}: ${translateProfileSyncReason(warning, t)}`,
    )
    .join("\n");
}

/**
 * 统一的“后置同步”工具：将当前使用的供应商写回对应应用的 live 配置。
 * 不抛出异常，由调用方根据返回值决定提示策略。
 */
export async function syncCurrentProvidersLiveSafe(): Promise<{
  ok: boolean;
  error?: Error;
  warnings?: ProfileSyncWarning[];
}> {
  try {
    const result = await settingsApi.syncCurrentProvidersLive();
    return {
      ok: !result.warning && !result.warnings?.length,
      warnings: result.warnings,
      error: result.warning ? new Error(result.warning) : undefined,
    };
  } catch (err) {
    const error = err instanceof Error ? err : new Error(String(err ?? ""));
    return { ok: false, error };
  }
}
