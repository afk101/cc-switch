import { beforeAll, describe, expect, it } from "vitest";
import { createInstance } from "i18next";
import en from "@/i18n/locales/en.json";
import ja from "@/i18n/locales/ja.json";
import zh from "@/i18n/locales/zh.json";
import type { ProfileSyncWarning } from "@/lib/api/settings";
import { formatProfileSyncWarnings } from "./postChangeSync";

const testI18n = createInstance();

beforeAll(async () => {
  await testI18n.init({
    lng: "zh",
    fallbackLng: "en",
    resources: {
      en: { translation: en },
      ja: { translation: ja },
      zh: { translation: zh },
    },
  });
});

describe("formatProfileSyncWarnings", () => {
  it("按稳定 reasonCode 映射当前语言文案且不展示 Home 路径", () => {
    const text = formatProfileSyncWarnings(
      [
        {
          profileId: "profile-a",
          profileName: "工作 Profile",
          homePath: "/private/profile-a",
          reasonCode: "apply_failed",
        },
        {
          profileId: "profile-b",
          profileName: "个人 Profile",
          homePath: "/private/profile-b",
          reasonCode: "primary_missing",
        },
      ],
      testI18n.getFixedT("en"),
    );

    expect(text).toBe(
      "工作 Profile: Failed to sync the Profile Home. Check file permissions or concurrent external changes.\n个人 Profile: No primary provider is configured for this Profile.",
    );
    expect(text).not.toContain("/private/");
  });

  it("未知 code 与旧 reason payload 只展示本地化脱敏兜底", () => {
    const forwardCompatibleAndLegacyWarnings = [
      {
        profileId: "profile-unknown",
        profileName: "Unknown Profile",
        homePath: "/private/unknown",
        reasonCode: "future_private_failure",
        reason: "token=secret-token base_url=https://private.example.com",
      },
      {
        profileId: "profile-legacy",
        profileName: "Legacy Profile",
        homePath: "/private/legacy",
        reason: "api_key=legacy-secret",
      },
    ] as unknown as ProfileSyncWarning[];
    const text = formatProfileSyncWarnings(
      forwardCompatibleAndLegacyWarnings,
      testI18n.getFixedT("ja"),
    );

    expect(text).toBe(
      "Unknown Profile: Profile の同期に失敗しました。設定とファイル権限を確認してください。\nLegacy Profile: Profile の同期に失敗しました。設定とファイル権限を確認してください。",
    );
    expect(text).not.toContain("secret");
    expect(text).not.toContain("private.example.com");
    expect(text).not.toContain("/private/");
  });

  it("没有 warning 时不生成描述", () => {
    const t = testI18n.getFixedT("zh");
    expect(formatProfileSyncWarnings(undefined, t)).toBeUndefined();
    expect(formatProfileSyncWarnings([], t)).toBeUndefined();
  });
});
