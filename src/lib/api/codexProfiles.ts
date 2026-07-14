import { invoke } from "@tauri-apps/api/core";
import type {
  CodexProfile,
  CodexProfileRef,
  CodexProfileState,
} from "@/types/codexProfile";

/** Codex Profile 专用 Tauri 命令封装。 */
export const codexProfilesApi = {
  /** 获取全部可选 CODEX_HOME。 */
  list: (): Promise<CodexProfile[]> => invoke("list_codex_profiles"),
  /** 创建新的自定义 CODEX_HOME Profile。 */
  create: (name: string, homePath: string): Promise<CodexProfile> =>
    invoke("create_codex_profile", { name, homePath }),
  /** 修改 Profile 显示名称。 */
  rename: (profileId: string, name: string): Promise<CodexProfile> =>
    invoke("rename_codex_profile", { profileId, name }),
  /** 重新绑定自定义 Profile 的 CODEX_HOME。 */
  rebind: (profileId: string, homePath: string): Promise<CodexProfile> =>
    invoke("rebind_codex_profile", { profileId, homePath }),
  /** 修改 Profile 专属监听端口。 */
  updatePort: (profileId: string, listenPort: number): Promise<boolean> =>
    invoke("update_codex_profile_port", { profileId, listenPort }),
  /** 获取单个 Profile 的状态，不复用其他 Profile 的缓存。 */
  getState: (profileId: string): Promise<CodexProfileState> =>
    invoke("get_codex_profile_state", { profileId }),
  /** 启用指定 Profile 的独立路由。 */
  enableRoute: (
    profileId: string,
    providerId: string,
    failoverIds: string[],
  ): Promise<boolean> =>
    invoke("enable_codex_profile_route", {
      profileId,
      providerId,
      failoverIds,
    }),
  /** 切换指定 Profile 的供应商。 */
  switchProvider: (
    profileId: string,
    providerId: string,
    failoverIds: string[],
  ): Promise<boolean> =>
    invoke("switch_codex_profile_provider", {
      profileId,
      providerId,
      failoverIds,
    }),
  /** 关闭指定 Profile 的路由。 */
  disableRoute: (profileId: string): Promise<boolean> =>
    invoke("disable_codex_profile_route", { profileId }),
  /** 删除自定义 Profile 的绑定与私有 token。 */
  delete: (profileId: string): Promise<boolean> =>
    invoke("delete_codex_profile", { profileId }),
  /** 获取引用某个全局供应商的 Profile。 */
  listProviderRefs: (providerId: string): Promise<CodexProfileRef[]> =>
    invoke("list_codex_provider_profile_refs", { providerId }),
};
