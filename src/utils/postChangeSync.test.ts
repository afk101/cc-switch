import { describe, expect, it } from "vitest";
import { formatProfileSyncWarnings } from "./postChangeSync";

describe("formatProfileSyncWarnings", () => {
  it("按 Profile 名称展示脱敏原因且不展示 Home 路径", () => {
    const text = formatProfileSyncWarnings([
      {
        profileId: "profile-a",
        profileName: "工作 Profile",
        homePath: "/private/profile-a",
        reason: "Profile Home 同步失败，请检查文件权限",
      },
      {
        profileId: "profile-b",
        profileName: "个人 Profile",
        homePath: "/private/profile-b",
        reason: "Profile 未配置主供应商",
      },
    ]);

    expect(text).toBe(
      "工作 Profile: Profile Home 同步失败，请检查文件权限\n个人 Profile: Profile 未配置主供应商",
    );
    expect(text).not.toContain("/private/");
  });

  it("没有 warning 时不生成描述", () => {
    expect(formatProfileSyncWarnings(undefined)).toBeUndefined();
    expect(formatProfileSyncWarnings([])).toBeUndefined();
  });
});
