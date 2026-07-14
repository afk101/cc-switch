import React from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { CODEX_DEFAULT_PROFILE_ID } from "@/config/constants";
import type { CodexProfile } from "@/types/codexProfile";
import { CodexProfileManagerDialog } from "./CodexProfileManagerDialog";

const defaultProfile: CodexProfile = {
  id: CODEX_DEFAULT_PROFILE_ID,
  name: "默认 Codex",
  canonicalHomePath: "/Users/test/.codex",
  listenPort: 15_721,
  createdAt: 1,
  updatedAt: 1,
};

const customProfile: CodexProfile = {
  id: "profile-work",
  name: "工作",
  canonicalHomePath: "/Users/test/.codex-work",
  listenPort: 15_722,
  createdAt: 1,
  updatedAt: 1,
};

/** 创建管理 Dialog 的完整默认 props，并允许单项覆盖。 */
function createManagerProps(
  overrides: Partial<
    React.ComponentProps<typeof CodexProfileManagerDialog>
  > = {},
) {
  return {
    open: true,
    profiles: [defaultProfile, customProfile],
    onOpenChange: vi.fn(),
    loadProfileState: vi.fn().mockResolvedValue({
      profile: customProfile,
      route: null,
      runtimeStatus: "stopped" as const,
    }),
    onCreate: vi.fn().mockResolvedValue(customProfile),
    onUpdate: vi.fn().mockResolvedValue(customProfile),
    onDelete: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
}

/** 渲染管理 Dialog 并返回独立 userEvent。 */
function renderManager(
  overrides: Partial<
    React.ComponentProps<typeof CodexProfileManagerDialog>
  > = {},
) {
  const props = createManagerProps(overrides);
  return {
    props,
    user: userEvent.setup(),
    ...render(<CodexProfileManagerDialog {...props} />),
  };
}

describe("CodexProfileManagerDialog", () => {
  it("点击新建后在同一 Dialog 显示创建表单且不调用 prompt", async () => {
    const promptSpy = vi.spyOn(window, "prompt");
    const { user } = renderManager();

    await user.click(screen.getByRole("button", { name: "新建 Profile" }));

    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(screen.getByRole("dialog")).toHaveTextContent("新建 Codex Profile");
    expect(screen.getByLabelText("CODEX_HOME 路径")).toBeVisible();
    expect(promptSpy).not.toHaveBeenCalled();
  });

  it("使用 codex-default 识别默认 Profile", () => {
    renderManager({ profiles: [defaultProfile] });

    expect(screen.getByRole("button", { name: "删除 Profile" })).toBeDisabled();
  });

  it("运行中的 Profile 编辑页禁用 Home 和端口", async () => {
    const loadProfileState = vi.fn().mockResolvedValue({
      profile: customProfile,
      route: {
        profileId: customProfile.id,
        currentProviderId: "provider-a",
        enabled: true,
        lastError: null,
        recoveryJson: null,
        updatedAt: 2,
      },
      runtimeStatus: "running" as const,
    });
    const { user } = renderManager({
      profiles: [customProfile],
      loadProfileState,
    });

    await user.click(screen.getByRole("button", { name: "编辑 Profile" }));

    expect(
      await screen.findByText("请先停止该 Profile 的路由后再修改"),
    ).toBeVisible();
    expect(screen.getByLabelText("Profile 名称")).toBeEnabled();
    expect(screen.getByLabelText("CODEX_HOME 路径")).toBeDisabled();
    expect(screen.getByLabelText("监听端口")).toBeDisabled();
  });

  it("默认 Profile 编辑页允许名称和端口但锁定 Home", async () => {
    const loadProfileState = vi.fn().mockResolvedValue({
      profile: defaultProfile,
      route: null,
      runtimeStatus: "stopped" as const,
    });
    const { user } = renderManager({
      profiles: [defaultProfile],
      loadProfileState,
    });

    await user.click(screen.getByRole("button", { name: "编辑 Profile" }));

    expect(await screen.findByLabelText("Profile 名称")).toBeEnabled();
    expect(screen.getByLabelText("CODEX_HOME 路径")).toBeDisabled();
    expect(screen.getByLabelText("监听端口")).toBeEnabled();
  });

  it("停止状态编辑时只提交一次原子更新", async () => {
    const { props, user } = renderManager({ profiles: [customProfile] });
    await user.click(screen.getByRole("button", { name: "编辑 Profile" }));
    const nameInput = await screen.findByLabelText("Profile 名称");
    await user.clear(nameInput);
    await user.type(nameInput, "工作新名称");

    await user.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => {
      expect(props.onUpdate).toHaveBeenCalledTimes(1);
    });
    expect(props.onUpdate).toHaveBeenCalledWith({
      profileId: customProfile.id,
      name: "工作新名称",
      homePath: customProfile.canonicalHomePath,
      listenPort: customProfile.listenPort,
    });
  });

  it("编辑状态加载失败时就地显示错误并允许返回", async () => {
    const { user } = renderManager({
      profiles: [customProfile],
      loadProfileState: vi.fn().mockRejectedValue(new Error("状态读取失败")),
    });

    await user.click(screen.getByRole("button", { name: "编辑 Profile" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("状态读取失败");
    await user.click(screen.getByRole("button", { name: "返回列表" }));
    expect(screen.getByRole("button", { name: "新建 Profile" })).toBeVisible();
  });

  it("创建成功后返回列表并保留单一 Dialog", async () => {
    const { props, user } = renderManager();
    await user.click(screen.getByRole("button", { name: "新建 Profile" }));
    await user.type(screen.getByLabelText("Profile 名称"), "新工作");
    await user.type(
      screen.getByLabelText("CODEX_HOME 路径"),
      "/Users/test/.codex-new",
    );

    await user.click(screen.getByRole("button", { name: "创建" }));

    await waitFor(() => {
      expect(props.onCreate).toHaveBeenCalledWith({
        name: "新工作",
        homePath: "/Users/test/.codex-new",
        listenPort: undefined,
      });
    });
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(screen.getByRole("button", { name: "新建 Profile" })).toBeVisible();
  });

  it("删除确认明确保留 Home 配置和会话", async () => {
    const { user } = renderManager({ profiles: [customProfile] });

    await user.click(screen.getByRole("button", { name: "删除 Profile" }));

    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(screen.getByText(customProfile.canonicalHomePath)).toBeVisible();
    expect(screen.getByText(/不删除 Home/)).toBeVisible();
    expect(screen.getByText(/auth\.json/)).toBeVisible();
    expect(screen.getByText(/config\.toml/)).toBeVisible();
    expect(screen.getByText(/会话/)).toBeVisible();
  });

  it("确认删除后调用 action 并返回列表", async () => {
    const { props, user } = renderManager({ profiles: [customProfile] });
    await user.click(screen.getByRole("button", { name: "删除 Profile" }));

    await user.click(screen.getByRole("button", { name: "确认删除" }));

    await waitFor(() => {
      expect(props.onDelete).toHaveBeenCalledWith(customProfile.id);
    });
    expect(screen.getByRole("button", { name: "新建 Profile" })).toBeVisible();
  });

  it("删除失败时保留确认页并就地显示错误", async () => {
    const { user } = renderManager({
      profiles: [customProfile],
      onDelete: vi.fn().mockRejectedValue(new Error("路由仍在停止中")),
    });
    await user.click(screen.getByRole("button", { name: "删除 Profile" }));

    await user.click(screen.getByRole("button", { name: "确认删除" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "路由仍在停止中",
    );
    expect(screen.getByRole("button", { name: "确认删除" })).toBeEnabled();
  });

  it("关闭重开后恢复列表页", async () => {
    const user = userEvent.setup();
    const props = createManagerProps({ open: true });
    const { rerender } = render(<CodexProfileManagerDialog {...props} />);
    await user.click(screen.getByRole("button", { name: "新建 Profile" }));
    await user.type(screen.getByLabelText("Profile 名称"), "未提交");

    rerender(<CodexProfileManagerDialog {...props} open={false} />);
    rerender(<CodexProfileManagerDialog {...props} open />);

    expect(screen.getByRole("button", { name: "新建 Profile" })).toBeVisible();
    expect(screen.queryByDisplayValue("未提交")).not.toBeInTheDocument();
  });
});
