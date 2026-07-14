import { useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import type { CodexProfile } from "@/types/codexProfile";

interface CodexProfileManagerDialogProps {
  open: boolean;
  profiles: CodexProfile[];
  onOpenChange: (open: boolean) => void;
  onUpdatePort: (profileId: string, listenPort: number) => void;
  onDelete: (profileId: string) => void;
  onRebind: (profileId: string) => void;
  onCreate: () => void;
}

/** 管理 Codex Profile；默认 Profile 只锁定路径重绑与删除能力。 */
export function CodexProfileManagerDialog({
  open,
  profiles,
  onOpenChange,
  onUpdatePort,
  onDelete,
  onRebind,
  onCreate,
}: CodexProfileManagerDialogProps) {
  const [ports, setPorts] = useState<Record<string, string>>({});
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>管理 Codex Profile</DialogTitle>
          <DialogDescription>
            删除只删除 CC Switch 绑定和本地 token，不删除 Home 目录。
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-3 overflow-y-auto px-6 py-4">
          {profiles.map((profile) => {
            const isDefault = profile.id === "default";
            const port = ports[profile.id] ?? String(profile.listenPort);
            return (
              <section
                key={profile.id}
                className="rounded-md border border-border-default p-3"
              >
                <div className="font-medium">{profile.name}</div>
                <div className="mt-1 truncate text-xs text-muted-foreground">
                  {profile.canonicalHomePath}
                </div>
                <label
                  className="mt-3 block text-xs text-muted-foreground"
                  htmlFor={`codex-port-${profile.id}`}
                >
                  监听端口
                </label>
                <div className="mt-1 flex gap-2">
                  <Input
                    id={`codex-port-${profile.id}`}
                    aria-label="监听端口"
                    type="number"
                    value={port}
                    onChange={(event) =>
                      setPorts((current) => ({
                        ...current,
                        [profile.id]: event.target.value,
                      }))
                    }
                  />
                  <Button
                    size="sm"
                    onClick={() => onUpdatePort(profile.id, Number(port))}
                  >
                    保存端口
                  </Button>
                </div>
                <div className="mt-3 flex gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={isDefault}
                    onClick={() => onRebind(profile.id)}
                  >
                    重新绑定 Home
                  </Button>
                  <Button
                    variant="destructive"
                    size="sm"
                    disabled={isDefault}
                    onClick={() => onDelete(profile.id)}
                  >
                    删除 Profile
                  </Button>
                </div>
              </section>
            );
          })}
        </div>
        <DialogFooter>
          <Button onClick={onCreate}>新建 Profile</Button>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            关闭
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
