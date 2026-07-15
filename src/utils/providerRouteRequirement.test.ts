import { describe, expect, it } from "vitest";
import type { Provider } from "@/types";
import { getCodexProviderRouteRequirement } from "./providerRouteRequirement";

/** 创建路由需求测试使用的最小 Codex 供应商。 */
function createProvider(overrides: Partial<Provider> = {}): Provider {
  return {
    id: "provider-a",
    name: "Provider A",
    category: "custom",
    settingsConfig: {
      auth: {},
      config:
        'model_provider = "custom"\n[model_providers.custom]\nwire_api = "responses"\n',
    },
    ...overrides,
  } as Provider;
}

describe("getCodexProviderRouteRequirement", () => {
  it("官方供应商无需路由", () => {
    expect(
      getCodexProviderRouteRequirement(
        createProvider({ category: "official" }),
      ),
    ).toBeNull();
  });

  it("OpenAI Chat 格式需要路由", () => {
    expect(
      getCodexProviderRouteRequirement(
        createProvider({ meta: { apiFormat: "openai_chat" } }),
      ),
    ).toBe("openaiChat");
  });

  it("完整 URL 模式需要路由", () => {
    expect(
      getCodexProviderRouteRequirement(
        createProvider({ meta: { isFullUrl: true } }),
      ),
    ).toBe("fullUrl");
  });

  it("原生 Responses 供应商无需路由", () => {
    expect(getCodexProviderRouteRequirement(createProvider())).toBeNull();
  });
});
