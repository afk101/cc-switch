import React from "react";
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { codexProfilesApi } from "@/lib/api/codexProfiles";
import type {
  CodexProfile,
  UpdateCodexProfileInput,
} from "@/types/codexProfile";
import {
  codexProfileKeys,
  useCreateCodexProfile,
  useDeleteCodexProfile,
  useUpdateCodexProfile,
} from "./codexProfiles";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const profile: CodexProfile = {
  id: "profile-a",
  name: "A",
  canonicalHomePath: "/tmp/a",
  listenPort: 15_730,
  createdAt: 1,
  updatedAt: 2,
};

/** 为 mutation 测试创建隔离的 QueryClient。 */
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
    return React.createElement(
      QueryClientProvider,
      { client: queryClient },
      children,
    );
  }
  return QueryWrapper;
}

beforeEach(() => {
  invoke.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("codexProfilesApi", () => {
  it("创建 Profile 时传递可选端口", async () => {
    await codexProfilesApi.create({
      name: "工作",
      homePath: "/tmp/codex-work",
      listenPort: 15_730,
    });

    expect(invoke).toHaveBeenCalledWith("create_codex_profile", {
      name: "工作",
      homePath: "/tmp/codex-work",
      listenPort: 15_730,
    });
  });

  it("编辑 Profile 只调用一次原子命令", async () => {
    await codexProfilesApi.update({
      profileId: "profile-a",
      name: "A",
      homePath: "/tmp/a",
      listenPort: 15_730,
    });

    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("update_codex_profile", {
      profileId: "profile-a",
      name: "A",
      homePath: "/tmp/a",
      listenPort: 15_730,
    });
  });
});

describe("codexProfileKeys", () => {
  it("按 Profile 隔离 Codex 状态查询键", () => {
    expect(codexProfileKeys.state("profile-a")).not.toEqual(
      codexProfileKeys.state("profile-b"),
    );
  });
});

describe("Codex Profile mutations", () => {
  it("创建成功后刷新 Profile 列表", async () => {
    const queryClient = createQueryClient();
    const invalidateQueries = vi.spyOn(queryClient, "invalidateQueries");
    vi.spyOn(codexProfilesApi, "create").mockResolvedValue(profile);
    const { result } = renderHook(() => useCreateCodexProfile(), {
      wrapper: createQueryWrapper(queryClient),
    });

    await act(async () => {
      await result.current.mutateAsync({
        name: "A",
        homePath: "/tmp/a",
      });
    });

    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: codexProfileKeys.list(),
    });
  });

  it("更新成功后只刷新列表和目标 Profile 状态", async () => {
    const queryClient = createQueryClient();
    const invalidateQueries = vi.spyOn(queryClient, "invalidateQueries");
    const updateInput: UpdateCodexProfileInput = {
      profileId: "profile-a",
      name: "A",
      homePath: "/tmp/a",
      listenPort: 15_730,
    };
    vi.spyOn(codexProfilesApi, "update").mockResolvedValue(profile);
    const { result } = renderHook(() => useUpdateCodexProfile(), {
      wrapper: createQueryWrapper(queryClient),
    });

    await act(async () => {
      await result.current.mutateAsync(updateInput);
    });

    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: codexProfileKeys.list(),
    });
    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: codexProfileKeys.state("profile-a"),
    });
    expect(invalidateQueries).not.toHaveBeenCalledWith({
      queryKey: codexProfileKeys.state("profile-b"),
    });
  });

  it("删除成功后移除目标状态并刷新列表", async () => {
    const queryClient = createQueryClient();
    const removeQueries = vi.spyOn(queryClient, "removeQueries");
    const invalidateQueries = vi.spyOn(queryClient, "invalidateQueries");
    vi.spyOn(codexProfilesApi, "delete").mockResolvedValue(true);
    const { result } = renderHook(() => useDeleteCodexProfile(), {
      wrapper: createQueryWrapper(queryClient),
    });

    await act(async () => {
      await result.current.mutateAsync("profile-a");
    });

    expect(removeQueries).toHaveBeenCalledWith({
      queryKey: codexProfileKeys.state("profile-a"),
    });
    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: codexProfileKeys.list(),
    });
  });
});
