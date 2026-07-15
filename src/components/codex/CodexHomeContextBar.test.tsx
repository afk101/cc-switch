import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { CodexHomeContextBar } from "./CodexHomeContextBar";

describe("CodexHomeContextBar", () => {
  it("让端口状态与 Home 选择框沿同一水平线居中", () => {
    render(
      <CodexHomeContextBar
        profiles={[]}
        selectedProfileId="profile-b"
        state={{
          profile: {
            id: "profile-b",
            name: "工作",
            canonicalHomePath: "/Users/test/.codex-work",
            listenPort: 15_722,
            createdAt: 1,
            updatedAt: 1,
          },
          route: null,
          runtimeStatus: "stopped",
        }}
        isStateLoading={false}
        onSelectProfile={() => undefined}
        onManage={() => undefined}
      />,
    );

    const section = screen
      .getByRole("combobox", { name: "选择 Codex Home" })
      .closest("section");
    const status = screen.getByText("端口 15722 · 路由未启用").parentElement;

    expect(section).toHaveClass("items-end");
    expect(status).toHaveClass("flex", "h-8", "items-center");
  });

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
