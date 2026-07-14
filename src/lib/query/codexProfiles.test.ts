import { describe, expect, it } from "vitest";
import { codexProfileKeys } from "./codexProfiles";

describe("codexProfileKeys", () => {
  it("按 Profile 隔离 Codex 状态查询键", () => {
    expect(codexProfileKeys.state("profile-a")).not.toEqual(
      codexProfileKeys.state("profile-b"),
    );
  });
});
