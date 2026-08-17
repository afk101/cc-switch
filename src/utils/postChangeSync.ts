import { settingsApi } from "@/lib/api";
import type { ProfileSyncWarning } from "@/lib/api/settings";

/** 将逐 Profile 脱敏 warning 格式化为可直接展示的多行文本。 */
export function formatProfileSyncWarnings(
  warnings: ProfileSyncWarning[] | undefined,
): string | undefined {
  if (!warnings?.length) return undefined;
  return warnings
    .map((warning) => `${warning.profileName}: ${warning.reason}`)
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
