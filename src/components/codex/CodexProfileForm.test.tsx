import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { CodexProfileForm } from "./CodexProfileForm";

describe("CodexProfileForm", () => {
  it("端口留空时提交自动分配请求", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const user = userEvent.setup();
    render(
      <CodexProfileForm mode="create" onSubmit={onSubmit} onCancel={vi.fn()} />,
    );

    await user.type(screen.getByLabelText("Profile 名称"), "工作");
    await user.type(
      screen.getByLabelText("CODEX_HOME 路径"),
      "/tmp/codex-work",
    );
    await user.click(screen.getByRole("button", { name: "创建" }));

    expect(onSubmit).toHaveBeenCalledWith({
      name: "工作",
      homePath: "/tmp/codex-work",
      listenPort: undefined,
    });
  });

  it("手动端口转换为数字后提交", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const user = userEvent.setup();
    render(
      <CodexProfileForm mode="create" onSubmit={onSubmit} onCancel={vi.fn()} />,
    );

    await user.type(screen.getByLabelText("Profile 名称"), "工作");
    await user.type(screen.getByLabelText("CODEX_HOME 路径"), "/tmp/work");
    await user.type(screen.getByLabelText("监听端口"), "15730");
    await user.click(screen.getByRole("button", { name: "创建" }));

    expect(onSubmit).toHaveBeenCalledWith({
      name: "工作",
      homePath: "/tmp/work",
      listenPort: 15_730,
    });
  });

  it("空名称时阻止提交", async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(
      <CodexProfileForm mode="create" onSubmit={onSubmit} onCancel={vi.fn()} />,
    );

    await user.type(screen.getByLabelText("CODEX_HOME 路径"), "/tmp/work");
    await user.click(screen.getByRole("button", { name: "创建" }));

    expect(screen.getByRole("alert")).toHaveTextContent("Profile 名称不能为空");
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("空 Home 时阻止提交", async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(
      <CodexProfileForm mode="create" onSubmit={onSubmit} onCancel={vi.fn()} />,
    );

    await user.type(screen.getByLabelText("Profile 名称"), "工作");
    await user.click(screen.getByRole("button", { name: "创建" }));

    expect(screen.getByRole("alert")).toHaveTextContent(
      "CODEX_HOME 路径不能为空",
    );
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it.each(["0", "65536", "1.5", "abc"])(
    "非法端口 %s 时阻止提交",
    async (port) => {
      const onSubmit = vi.fn();
      const user = userEvent.setup();
      render(
        <CodexProfileForm
          mode="create"
          onSubmit={onSubmit}
          onCancel={vi.fn()}
        />,
      );

      await user.type(screen.getByLabelText("Profile 名称"), "工作");
      await user.type(screen.getByLabelText("CODEX_HOME 路径"), "/tmp/work");
      await user.type(screen.getByLabelText("监听端口"), port);
      await user.click(screen.getByRole("button", { name: "创建" }));

      expect(screen.getByRole("alert")).toHaveTextContent(
        "监听端口必须是 1–65535 的整数",
      );
      expect(onSubmit).not.toHaveBeenCalled();
    },
  );

  it("编辑模式不允许清空端口", async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(
      <CodexProfileForm
        mode="edit"
        initialValues={{
          name: "工作",
          homePath: "/tmp/work",
          listenPort: 15_730,
        }}
        onSubmit={onSubmit}
        onCancel={vi.fn()}
      />,
    );

    await user.clear(screen.getByLabelText("监听端口"));
    await user.click(screen.getByRole("button", { name: "保存" }));

    expect(screen.getByRole("alert")).toHaveTextContent("监听端口不能为空");
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("后端失败时保留输入并显示错误", async () => {
    const onSubmit = vi.fn().mockRejectedValue(new Error("端口已被占用"));
    const user = userEvent.setup();
    render(
      <CodexProfileForm mode="create" onSubmit={onSubmit} onCancel={vi.fn()} />,
    );

    await user.type(screen.getByLabelText("Profile 名称"), "工作");
    await user.type(screen.getByLabelText("CODEX_HOME 路径"), "/tmp/work");
    await user.type(screen.getByLabelText("监听端口"), "15730");
    await user.click(screen.getByRole("button", { name: "创建" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("端口已被占用");
    expect(screen.getByLabelText("Profile 名称")).toHaveValue("工作");
    expect(screen.getByLabelText("CODEX_HOME 路径")).toHaveValue("/tmp/work");
    expect(screen.getByLabelText("监听端口")).toHaveValue("15730");
  });

  it("运行中编辑只允许修改名称", () => {
    render(
      <CodexProfileForm
        mode="edit"
        initialValues={{
          name: "工作",
          homePath: "/tmp/work",
          listenPort: 15_730,
        }}
        canEditHome={false}
        canEditPort={false}
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
      />,
    );

    expect(screen.getByLabelText("Profile 名称")).toBeEnabled();
    expect(screen.getByLabelText("CODEX_HOME 路径")).toBeDisabled();
    expect(screen.getByLabelText("监听端口")).toBeDisabled();
  });

  it("提交期间禁用提交和取消按钮", async () => {
    let resolveSubmit: (() => void) | undefined;
    const onSubmit = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveSubmit = resolve;
        }),
    );
    const user = userEvent.setup();
    render(
      <CodexProfileForm mode="create" onSubmit={onSubmit} onCancel={vi.fn()} />,
    );
    await user.type(screen.getByLabelText("Profile 名称"), "工作");
    await user.type(screen.getByLabelText("CODEX_HOME 路径"), "/tmp/work");

    await user.click(screen.getByRole("button", { name: "创建" }));

    expect(screen.getByRole("button", { name: "创建中..." })).toBeDisabled();
    expect(screen.getByRole("button", { name: "取消" })).toBeDisabled();
    await act(async () => {
      resolveSubmit?.();
    });
  });
});
