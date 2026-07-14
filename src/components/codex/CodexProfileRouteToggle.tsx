import { Loader2, Radio } from "lucide-react";
import { toast } from "sonner";
import { Switch } from "@/components/ui/switch";
import { useSetCodexProfileRouteEnabled } from "@/lib/query/codexProfiles";
import { cn } from "@/lib/utils";
import type {
  CodexProfileState,
  CodexRuntimeStatus,
} from "@/types/codexProfile";
import { extractErrorMessage } from "@/utils/errorUtils";

interface CodexProfileRouteToggleProps {
  className?: string;
  profileId: string | null;
  state: CodexProfileState | undefined;
  isStateLoading: boolean;
}

/** 将 Profile runtime 状态转换为顶部开关的简短文案。 */
function getRuntimeLabel(status: CodexRuntimeStatus): string {
  if (typeof status === "object") {
    return "路由失败";
  }
  switch (status) {
    case "running":
      return "路由运行中";
    case "starting":
      return "路由启动中";
    case "stopping":
      return "路由停止中";
    default:
      return "路由已停止";
  }
}

/** 只读取并切换当前 Codex Profile 独立路由的顶部开关。 */
export function CodexProfileRouteToggle({
  className,
  profileId,
  state,
  isStateLoading,
}: CodexProfileRouteToggleProps) {
  const mutation = useSetCodexProfileRouteEnabled();
  const scopedState = state?.profile.id === profileId ? state : undefined;
  const routeEnabled = !isStateLoading && scopedState?.route?.enabled === true;
  const isPending = mutation.isPending;
  const isDisabled =
    isStateLoading || !profileId || !scopedState || mutation.isPending;
  const tooltipText = scopedState
    ? `${scopedState.profile.name} · ${scopedState.profile.canonicalHomePath} · 127.0.0.1:${scopedState.profile.listenPort} · ${getRuntimeLabel(scopedState.runtimeStatus)}`
    : "正在加载当前 CODEX_HOME 路由状态";

  /** 将用户选择限定到当前 Profile 的路由 mutation。 */
  const handleToggle = async (enabled: boolean): Promise<void> => {
    if (!profileId || !scopedState) {
      return;
    }
    try {
      await mutation.mutateAsync({
        profileId,
        providerId: scopedState.route?.currentProviderId ?? null,
        enabled,
      });
    } catch (error) {
      toast.error(extractErrorMessage(error));
    }
  };

  return (
    <div
      className={cn(
        "flex h-8 items-center gap-1 rounded-lg bg-muted/50 px-1.5 transition-all",
        className,
      )}
      title={tooltipText}
    >
      {isPending || isStateLoading ? (
        <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
      ) : (
        <Radio
          className={cn(
            "h-4 w-4 transition-colors",
            routeEnabled
              ? "animate-pulse text-emerald-500"
              : "text-muted-foreground",
          )}
        />
      )}
      <Switch
        aria-label={
          scopedState
            ? `切换 ${scopedState.profile.name} 路由`
            : "切换 Codex Profile 路由"
        }
        checked={routeEnabled}
        onCheckedChange={handleToggle}
        disabled={isDisabled}
      />
    </div>
  );
}
