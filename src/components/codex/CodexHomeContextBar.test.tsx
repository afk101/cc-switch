import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { CodexHomeContextBar } from "./CodexHomeContextBar";

describe("CodexHomeContextBar", () => {
  it("切换 Home 后不显示上一个 Profile 的供应商", () => {
    render(
      <CodexHomeContextBar
        profiles={[]}
        selectedProfileId="profile-b"
        state={undefined}
        isStateLoading
        onSelectProfile={() => undefined}
        onManage={() => undefined}
      />,
    );

    expect(screen.queryByText("供应商 A")).not.toBeInTheDocument();
    expect(screen.getByText("正在加载 Profile 状态…")).toBeInTheDocument();
  });
});
