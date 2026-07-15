import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { codexProfilesApi } from "@/lib/api/codexProfiles";
import type {
  CodexProfile,
  CreateCodexProfileInput,
  UpdateCodexProfileInput,
} from "@/types/codexProfile";

/** 修改单个 Codex Profile 路由开关所需的完整作用域。 */
export interface SetCodexProfileRouteEnabledInput {
  profileId: string;
  providerId: string | null;
  enabled: boolean;
}

/** Codex Profile 查询键；状态键必须携带 Profile ID 以防跨 Home 串数据。 */
export const codexProfileKeys = {
  all: ["codexProfiles"] as const,
  list: () => [...codexProfileKeys.all, "list"] as const,
  state: (profileId: string) =>
    [...codexProfileKeys.all, "state", profileId] as const,
  refs: (providerId: string) =>
    [...codexProfileKeys.all, "refs", providerId] as const,
};

/** 将 TanStack mutation 的变量适配为单参数路由停止调用。 */
function disableCodexProfileRoute(profileId: string): Promise<boolean> {
  return codexProfilesApi.disableRoute(profileId);
}

/** 根据目标状态调用单个 Profile 的启用或关闭命令。 */
function setCodexProfileRouteEnabled({
  profileId,
  providerId,
  enabled,
}: SetCodexProfileRouteEnabledInput): Promise<boolean> {
  if (!enabled) {
    return codexProfilesApi.disableRoute(profileId);
  }
  if (!providerId) {
    return Promise.reject(new Error("当前 CODEX_HOME 未选择可路由供应商"));
  }
  return codexProfilesApi.enableRoute(profileId, providerId);
}

/** 获取全部 Codex Profile。 */
export function useCodexProfiles() {
  return useQuery({
    queryKey: codexProfileKeys.list(),
    queryFn: codexProfilesApi.list,
  });
}

/** 获取指定 Profile 的路由状态；切换 ID 时不显示上一个 Profile 的旧数据。 */
export function useCodexProfileState(profileId: string | null) {
  return useQuery({
    queryKey: codexProfileKeys.state(profileId ?? ""),
    queryFn: () => codexProfilesApi.getState(profileId!),
    enabled: Boolean(profileId),
  });
}

/** 创建 Profile 并在成功后刷新列表缓存。 */
export function useCreateCodexProfile() {
  const queryClient = useQueryClient();
  return useMutation<CodexProfile, Error, CreateCodexProfileInput>({
    mutationFn: codexProfilesApi.create,
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: codexProfileKeys.list(),
      });
    },
  });
}

/** 原子更新 Profile 并只刷新列表与目标状态缓存。 */
export function useUpdateCodexProfile() {
  const queryClient = useQueryClient();
  return useMutation<CodexProfile, Error, UpdateCodexProfileInput>({
    mutationFn: codexProfilesApi.update,
    onSuccess: async (_, input) => {
      await queryClient.invalidateQueries({
        queryKey: codexProfileKeys.list(),
      });
      await queryClient.invalidateQueries({
        queryKey: codexProfileKeys.state(input.profileId),
      });
    },
  });
}

/** 停止指定 Profile 路由并只刷新该 Profile 的运行时状态。 */
export function useDisableCodexProfileRoute() {
  const queryClient = useQueryClient();
  return useMutation<boolean, Error, string>({
    mutationFn: disableCodexProfileRoute,
    onSuccess: async (_, profileId) => {
      await queryClient.invalidateQueries({
        queryKey: codexProfileKeys.state(profileId),
      });
    },
  });
}

/** 切换指定 Profile 路由，并只刷新该 Profile 的状态缓存。 */
export function useSetCodexProfileRouteEnabled() {
  const queryClient = useQueryClient();
  return useMutation<boolean, Error, SetCodexProfileRouteEnabledInput>({
    mutationFn: setCodexProfileRouteEnabled,
    onSuccess: async (_, input) => {
      await queryClient.invalidateQueries({
        queryKey: codexProfileKeys.state(input.profileId),
      });
    },
  });
}

/** 删除 Profile、移除目标状态缓存并刷新列表。 */
export function useDeleteCodexProfile() {
  const queryClient = useQueryClient();
  return useMutation<boolean, Error, string>({
    mutationFn: codexProfilesApi.delete,
    onSuccess: async (_, profileId) => {
      queryClient.removeQueries({
        queryKey: codexProfileKeys.state(profileId),
      });
      await queryClient.invalidateQueries({
        queryKey: codexProfileKeys.list(),
      });
    },
  });
}
