import React from "react";
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CODEX_DEFAULT_PROFILE_ID } from "@/config/constants";
import { codexProfilesApi } from "@/lib/api/codexProfiles";
import type {
  CodexProfile,
  CodexProfileState,
  CreateCodexProfileInput,
} from "@/types/codexProfile";
import { useCodexProfileManagement } from "./useCodexProfileManagement";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const defaultProfile: CodexProfile = {
  id: CODEX_DEFAULT_PROFILE_ID,
  name: "默认 Codex",
  canonicalHomePath: "/Users/test/.codex",
  listenPort: 15_721,
  createdAt: 1,
  updatedAt: 1,
};

const currentProfile: CodexProfile = {
  id: "profile-current",
  name: "当前",
  canonicalHomePath: "/Users/test/.codex-current",
  listenPort: 15_722,
  createdAt: 1,
  updatedAt: 1,
};

const newProfile: CodexProfile = {
  id: "profile-new",
  name: "新建",
  canonicalHomePath: "/Users/test/.codex-new",
  listenPort: 15_723,
  createdAt: 2,
  updatedAt: 2,
};

const createInput: CreateCodexProfileInput = {
  name: "新建",
  homePath: "/Users/test/.codex-new",
};

/** 创建禁用自动重试的 QueryClient。 */
function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
}

/** 为 hook 测试创建绑定指定 QueryClient 的 wrapper。 */
function createQueryWrapper(queryClient: QueryClient) {
  /** 渲染隔离的 TanStack Query 上下文。 */
  function QueryWrapper({ children }: React.PropsWithChildren) {
    return (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    );
  }
  return QueryWrapper;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("useCodexProfileManagement", () => {
  it("创建成功后选中新 Profile", async () => {
    const queryClient = createQueryClient();
    const onSelectProfile = vi.fn();
    vi.spyOn(codexProfilesApi, "create").mockResolvedValue(newProfile);
    const { result } = renderHook(
      () =>
        useCodexProfileManagement({
          profiles: [defaultProfile],
          selectedProfileId: CODEX_DEFAULT_PROFILE_ID,
          onSelectProfile,
        }),
      { wrapper: createQueryWrapper(queryClient) },
    );

    await act(async () => {
      await result.current.createProfile(createInput);
    });

    expect(onSelectProfile).toHaveBeenCalledWith("profile-new");
  });

  it("删除当前 Profile 后优先选择默认 Profile", async () => {
    const queryClient = createQueryClient();
    const onSelectProfile = vi.fn();
    vi.spyOn(codexProfilesApi, "delete").mockResolvedValue(true);
    const { result } = renderHook(
      () =>
        useCodexProfileManagement({
          profiles: [defaultProfile, currentProfile],
          selectedProfileId: "profile-current",
          onSelectProfile,
        }),
      { wrapper: createQueryWrapper(queryClient) },
    );

    await act(async () => {
      await result.current.deleteProfile("profile-current");
    });

    expect(onSelectProfile).toHaveBeenCalledWith(CODEX_DEFAULT_PROFILE_ID);
  });

  it("delete_current_profile_uses_first_remaining_when_default_is_absent", async () => {
    const queryClient = createQueryClient();
    const onSelectProfile = vi.fn();
    vi.spyOn(codexProfilesApi, "delete").mockResolvedValue(true);
    const { result } = renderHook(
      () =>
        useCodexProfileManagement({
          profiles: [currentProfile, newProfile],
          selectedProfileId: "profile-current",
          onSelectProfile,
        }),
      { wrapper: createQueryWrapper(queryClient) },
    );

    await act(async () => {
      await result.current.deleteProfile("profile-current");
    });

    expect(onSelectProfile).toHaveBeenCalledWith("profile-new");
  });

  it("delete_non_selected_profile_keeps_selection", async () => {
    const queryClient = createQueryClient();
    const onSelectProfile = vi.fn();
    vi.spyOn(codexProfilesApi, "delete").mockResolvedValue(true);
    const { result } = renderHook(
      () =>
        useCodexProfileManagement({
          profiles: [defaultProfile, currentProfile, newProfile],
          selectedProfileId: "profile-current",
          onSelectProfile,
        }),
      { wrapper: createQueryWrapper(queryClient) },
    );

    await act(async () => {
      await result.current.deleteProfile("profile-new");
    });

    expect(onSelectProfile).not.toHaveBeenCalled();
  });

  it("加载状态时不读取或覆盖其他 Profile 缓存", async () => {
    const queryClient = createQueryClient();
    const onSelectProfile = vi.fn();
    const otherState: CodexProfileState = {
      profile: newProfile,
      route: null,
      runtimeStatus: "stopped",
    };
    const currentState: CodexProfileState = {
      profile: currentProfile,
      route: null,
      runtimeStatus: "running",
    };
    queryClient.setQueryData(
      ["codexProfiles", "state", "profile-new"],
      otherState,
    );
    const getState = vi
      .spyOn(codexProfilesApi, "getState")
      .mockResolvedValue(currentState);
    const { result } = renderHook(
      () =>
        useCodexProfileManagement({
          profiles: [currentProfile, newProfile],
          selectedProfileId: "profile-current",
          onSelectProfile,
        }),
      { wrapper: createQueryWrapper(queryClient) },
    );

    let loadedState: CodexProfileState | undefined;
    await act(async () => {
      loadedState = await result.current.loadProfileState("profile-current");
    });

    expect(getState).toHaveBeenCalledWith("profile-current");
    expect(loadedState).toEqual(currentState);
    expect(
      queryClient.getQueryData(["codexProfiles", "state", "profile-new"]),
    ).toEqual(otherState);
  });
});
