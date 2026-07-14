/** 独立 CODEX_HOME 的持久化描述。 */
export interface CodexProfile {
  id: string;
  name: string;
  canonicalHomePath: string;
  listenPort: number;
  createdAt: number;
  updatedAt: number;
}

/** Profile 的持久化路由状态。 */
export interface CodexProfileRoute {
  profileId: string;
  currentProviderId: string | null;
  enabled: boolean;
  lastError: string | null;
  recoveryJson: string | null;
  updatedAt: number;
}

/** Profile 运行时监听器状态。 */
export type CodexRuntimeStatus =
  | "stopped"
  | "starting"
  | "running"
  | "stopping"
  | { failed: { message: string } };

/** Profile 配置、路由与运行时的联合快照。 */
export interface CodexProfileState {
  profile: CodexProfile;
  route: CodexProfileRoute | null;
  runtimeStatus: CodexRuntimeStatus;
}

/** 仍引用某个全局供应商的 Profile 摘要。 */
export interface CodexProfileRef {
  id: string;
  name: string;
}
