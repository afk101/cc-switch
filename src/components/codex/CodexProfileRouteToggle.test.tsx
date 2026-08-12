import React from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { codexProfilesApi } from "@/lib/api/codexProfiles";
import type { CodexProfileState } from "@/types/codexProfile";
import { CodexProfileRouteToggle } from "./CodexProfileRouteToggle";

const { toastError } = vi.hoisted(() => ({ toastError: vi.fn() }));

vi.mock("sonner", () => ({
  toast: {
    error: toastError,
  },
}));

const enabledState: CodexProfileState = {
  profile: {
    id: "profile-a",
    name: "A",
    canonicalHomePath: "/tmp/codex-a",
    listenPort: 15_722,
    createdAt: 1,
    updatedAt: 2,
  },
  route: {
    profileId: "profile-a",
    currentProviderId: "provider-a",
  enabled: true,
  homeOwnership: "managed",
    lastError: null,
    recoveryJson: null,
    updatedAt: 3,
  },
  runtimeStatus: "running",
};

const disabledState: CodexProfileState = {
  profile: {
    id: "profile-b",
    name: "B",
    canonicalHomePath: "/tmp/codex-b",
    listenPort: 15_723,
    createdAt: 4,
    updatedAt: 5,
  },
  route: {
    profileId: "profile-b",
    currentProviderId: "provider-b",
  enabled: false,
  homeOwnership: "managed",
    lastError: null,
    recoveryJson: null,
    updatedAt: 6,
  },
  runtimeStatus: "stopped",
};

/** 使用独立 QueryClient 渲染 Profile 路由开关。 */
function renderToggle(
  props: React.ComponentProps<typeof CodexProfileRouteToggle>,
) {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <CodexProfileRouteToggle {...props} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  toastError.mockReset();
  vi.restoreAllMocks();
});

describe("CodexProfileRouteToggle", () => {
  it("只显示当前 Profile 的开启状态与端口", () => {
    renderToggle({
      profileId: "profile-a",
      state: enabledState,
      isStateLoading: false,
    });

    expect(screen.getByRole("switch")).toBeChecked();
    expect(
      screen.getByTitle(/A · \/tmp\/codex-a · 127\.0\.0\.1:15722/),
    ).toBeInTheDocument();
  });

  it("切换到正在加载的 Profile 时不保留上一项状态", () => {
    renderToggle({
      profileId: "profile-b",
      state: undefined,
      isStateLoading: true,
    });

    expect(screen.getByRole("switch")).not.toBeChecked();
    expect(screen.getByRole("switch")).toBeDisabled();
  });

  it("开启时使用当前 Profile 自己的供应商", async () => {
    const user = userEvent.setup();
    const enableRoute = vi
      .spyOn(codexProfilesApi, "enableRoute")
      .mockResolvedValue(true);
    const disableRoute = vi.spyOn(codexProfilesApi, "disableRoute");
    renderToggle({
      profileId: "profile-b",
      state: disabledState,
      isStateLoading: false,
    });

    await user.click(screen.getByRole("switch"));

    await waitFor(() =>
      expect(enableRoute).toHaveBeenCalledWith("profile-b", "provider-b", []),
    );
    expect(disableRoute).not.toHaveBeenCalled();
  });

  it("关闭时只停止当前 Profile", async () => {
    const user = userEvent.setup();
    const disableRoute = vi
      .spyOn(codexProfilesApi, "disableRoute")
      .mockResolvedValue(true);
    const enableRoute = vi.spyOn(codexProfilesApi, "enableRoute");
    renderToggle({
      profileId: "profile-a",
      state: enabledState,
      isStateLoading: false,
    });

    await user.click(screen.getByRole("switch"));

    await waitFor(() => expect(disableRoute).toHaveBeenCalledWith("profile-a"));
    expect(enableRoute).not.toHaveBeenCalled();
  });

  it("缺少当前 Profile 供应商时拒绝启动", async () => {
    const user = userEvent.setup();
    const enableRoute = vi.spyOn(codexProfilesApi, "enableRoute");
    renderToggle({
      profileId: "profile-b",
      state: {
        ...disabledState,
        route: {
          ...disabledState.route!,
          currentProviderId: null,
        },
      },
      isStateLoading: false,
    });

    await user.click(screen.getByRole("switch"));

    await waitFor(() =>
      expect(toastError).toHaveBeenCalledWith(
        "当前 CODEX_HOME 未选择可路由供应商",
      ),
    );
    expect(enableRoute).not.toHaveBeenCalled();
  });
});
