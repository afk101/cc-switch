import { useQuery } from "@tanstack/react-query";
import { codexProfilesApi } from "@/lib/api/codexProfiles";

/** Codex Profile 查询键；状态键必须携带 Profile ID 以防跨 Home 串数据。 */
export const codexProfileKeys = {
  all: ["codexProfiles"] as const,
  list: () => [...codexProfileKeys.all, "list"] as const,
  state: (profileId: string) =>
    [...codexProfileKeys.all, "state", profileId] as const,
  refs: (providerId: string) =>
    [...codexProfileKeys.all, "refs", providerId] as const,
};

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
