import { Settings2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { CodexProfile, CodexProfileState } from "@/types/codexProfile";

interface CodexHomeContextBarProps {
  profiles: CodexProfile[];
  selectedProfileId: string | null;
  state: CodexProfileState | undefined;
  isStateLoading: boolean;
  onSelectProfile: (profileId: string) => void;
  onManage: () => void;
}

/** 在 Codex 供应商列表上方展示当前 CODEX_HOME 的显式上下文。 */
export function CodexHomeContextBar({
  profiles,
  selectedProfileId,
  state,
  isStateLoading,
  onSelectProfile,
  onManage,
}: CodexHomeContextBarProps) {
  return (
    <section className="flex items-end gap-3 rounded-lg border border-border-default bg-muted/20 px-3 py-2">
      <div className="min-w-0 flex-1">
        <div className="text-xs text-muted-foreground">当前 CODEX_HOME</div>
        <Select
          value={selectedProfileId ?? undefined}
          onValueChange={onSelectProfile}
        >
          <SelectTrigger aria-label="选择 Codex Home" className="mt-1 h-8">
            <SelectValue placeholder="选择 CODEX_HOME" />
          </SelectTrigger>
          <SelectContent>
            {profiles.map((profile) => (
              <SelectItem key={profile.id} value={profile.id}>
                {profile.name} · {profile.canonicalHomePath}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="flex h-8 min-w-0 flex-1 items-center text-xs text-muted-foreground">
        {isStateLoading ? (
          <span>正在加载 Profile 状态…</span>
        ) : state ? (
          <span>
            端口 {state.profile.listenPort} ·{" "}
            {state.route?.enabled ? "路由已启用" : "路由未启用"}
          </span>
        ) : (
          <span>请选择 CODEX_HOME</span>
        )}
      </div>
      <Button
        variant="outline"
        size="icon"
        aria-label="管理 Codex Profile"
        onClick={onManage}
      >
        <Settings2 className="h-4 w-4" />
      </Button>
    </section>
  );
}
