import type { Provider } from "@/types";
import {
  CODEX_PROVIDER_ROUTE_REQUIREMENTS,
  type CodexProviderRouteRequirement,
} from "@/config/constants";
import {
  extractCodexWireApi,
  isCodexChatWireApi,
} from "@/utils/providerConfigUtils";

/** 判断 Codex 供应商是否依赖本地路由，以及依赖路由的原因。 */
export function getCodexProviderRouteRequirement(
  provider: Provider,
): CodexProviderRouteRequirement | null {
  if (provider.category === "official") return null;
  if (provider.meta?.apiFormat === "openai_chat") {
    return CODEX_PROVIDER_ROUTE_REQUIREMENTS.OPENAI_CHAT;
  }

  const config = (provider.settingsConfig as Record<string, unknown>)?.config;
  if (
    typeof config === "string" &&
    isCodexChatWireApi(extractCodexWireApi(config))
  ) {
    return CODEX_PROVIDER_ROUTE_REQUIREMENTS.OPENAI_CHAT;
  }
  if (provider.meta?.isFullUrl) {
    return CODEX_PROVIDER_ROUTE_REQUIREMENTS.FULL_URL;
  }
  return null;
}
