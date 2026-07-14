import { useEffect, useState } from "react";
import { toast } from "sonner";
import {
  CodexProfileForm,
  type CodexProfileFormValues,
} from "@/components/codex/CodexProfileForm";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { CODEX_DEFAULT_PROFILE_ID } from "@/config/constants";
import type {
  CodexProfile,
  CodexProfileState,
  CreateCodexProfileInput,
  UpdateCodexProfileInput,
} from "@/types/codexProfile";
import { extractErrorMessage } from "@/utils/errorUtils";

type ProfileManagerView =
  | { kind: "list" }
  | { kind: "create" }
  | { kind: "edit"; profileId: string }
  | { kind: "delete-confirm"; profileId: string };

interface CodexProfileManagerDialogProps {
  open: boolean;
  profiles: CodexProfile[];
  onOpenChange: (open: boolean) => void;
  loadProfileState: (profileId: string) => Promise<CodexProfileState>;
  onCreate: (input: CreateCodexProfileInput) => Promise<CodexProfile>;
  onUpdate: (input: UpdateCodexProfileInput) => Promise<CodexProfile>;
  onStopRoute: (profileId: string) => Promise<void>;
  onDelete: (profileId: string) => Promise<void>;
}

/** 判断运行时状态是否禁止修改监听器绑定。 */
function isActiveRuntimeStatus(
  runtimeStatus: CodexProfileState["runtimeStatus"],
): boolean {
  return (
    runtimeStatus === "starting" ||
    runtimeStatus === "running" ||
    runtimeStatus === "stopping"
  );
}

/** 返回当前视图对应的标题。 */
function getViewTitle(view: ProfileManagerView): string {
  switch (view.kind) {
    case "create":
      return "新建 Codex Profile";
    case "edit":
      return "编辑 Codex Profile";
    case "delete-confirm":
      return "删除 Codex Profile";
    default:
      return "管理 Codex Profile";
  }
}

/** 返回当前视图对应的说明。 */
function getViewDescription(view: ProfileManagerView): string {
  switch (view.kind) {
    case "create":
      return "为新的 CODEX_HOME 建立独立 Profile。";
    case "edit":
      return "一次保存名称、CODEX_HOME 与监听端口。";
    case "delete-confirm":
      return "确认删除 CC Switch 中的 Profile 绑定。";
    default:
      return "删除只删除 CC Switch 绑定和本地 token，不删除 Home 目录。";
  }
}

/** 管理 Codex Profile；默认 Profile 只锁定路径重绑与删除能力。 */
export function CodexProfileManagerDialog({
  open,
  profiles,
  onOpenChange,
  loadProfileState,
  onCreate,
  onUpdate,
  onStopRoute,
  onDelete,
}: CodexProfileManagerDialogProps) {
  const [view, setView] = useState<ProfileManagerView>({ kind: "list" });
  const [editState, setEditState] = useState<CodexProfileState | null>(null);
  const [editError, setEditError] = useState<string | null>(null);
  const [isEditLoading, setIsEditLoading] = useState(false);
  const [stopRouteError, setStopRouteError] = useState<string | null>(null);
  const [isStoppingRoute, setIsStoppingRoute] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [isDeleting, setIsDeleting] = useState(false);

  useEffect(() => {
    if (!open) {
      setView({ kind: "list" });
      setEditState(null);
      setEditError(null);
      setStopRouteError(null);
      setIsStoppingRoute(false);
      setDeleteError(null);
      setIsDeleting(false);
    }
  }, [open]);

  useEffect(() => {
    if (!open || view.kind !== "edit") {
      return;
    }
    let cancelled = false;
    setIsEditLoading(true);
    setEditState(null);
    setEditError(null);
    setStopRouteError(null);
    loadProfileState(view.profileId)
      .then((state) => {
        if (!cancelled) {
          setEditState(state);
        }
      })
      .catch((error) => {
        if (!cancelled) {
          setEditError(extractErrorMessage(error) || "加载 Profile 状态失败");
        }
      })
      .finally(() => {
        if (!cancelled) {
          setIsEditLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [loadProfileState, open, view]);

  /** 创建成功后回到当前 Dialog 的列表页。 */
  async function handleCreate(values: CodexProfileFormValues): Promise<void> {
    await onCreate(values);
    toast.success("Codex Profile 创建成功");
    setView({ kind: "list" });
  }

  /** 原子保存编辑字段并回到列表页。 */
  async function handleUpdate(values: CodexProfileFormValues): Promise<void> {
    if (view.kind !== "edit" || values.listenPort === undefined) {
      throw new Error("监听端口不能为空");
    }
    await onUpdate({
      profileId: view.profileId,
      name: values.name,
      homePath: values.homePath,
      listenPort: values.listenPort,
    });
    toast.success("Codex Profile 更新成功");
    setView({ kind: "list" });
  }

  /** 停止当前编辑目标的路由，并重新读取状态以解锁受限字段。 */
  async function handleStopRoute(): Promise<void> {
    if (view.kind !== "edit") {
      return;
    }
    setStopRouteError(null);
    setIsStoppingRoute(true);
    try {
      await onStopRoute(view.profileId);
      const refreshedState = await loadProfileState(view.profileId);
      setEditState(refreshedState);
      toast.success("Codex Profile 路由已停止");
    } catch (error) {
      setStopRouteError(extractErrorMessage(error) || "停止路由失败");
    } finally {
      setIsStoppingRoute(false);
    }
  }

  /** 确认删除当前目标，失败时留在当前页展示错误。 */
  async function handleDelete(): Promise<void> {
    if (view.kind !== "delete-confirm") {
      return;
    }
    setDeleteError(null);
    setIsDeleting(true);
    try {
      await onDelete(view.profileId);
      toast.success("Codex Profile 删除成功");
      setView({ kind: "list" });
    } catch (error) {
      setDeleteError(extractErrorMessage(error) || "删除 Profile 失败");
    } finally {
      setIsDeleting(false);
    }
  }

  const deleteTarget =
    view.kind === "delete-confirm"
      ? profiles.find((profile) => profile.id === view.profileId)
      : undefined;
  const editProfile = editState?.profile;
  const editIsActive = editState
    ? isActiveRuntimeStatus(editState.runtimeStatus)
    : false;
  const canEditHome = Boolean(
    editProfile && editProfile.id !== CODEX_DEFAULT_PROFILE_ID && !editIsActive,
  );
  const canEditPort = Boolean(editProfile && !editIsActive);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{getViewTitle(view)}</DialogTitle>
          <DialogDescription>{getViewDescription(view)}</DialogDescription>
        </DialogHeader>

        {view.kind === "list" && (
          <>
            <div className="space-y-3 overflow-y-auto px-6 py-4">
              {profiles.map((profile) => (
                <section
                  key={profile.id}
                  className="rounded-md border border-border-default p-3"
                >
                  <div className="font-medium">{profile.name}</div>
                  <div className="mt-1 truncate text-xs text-muted-foreground">
                    {profile.canonicalHomePath}
                  </div>
                  <div className="mt-1 text-xs text-muted-foreground">
                    监听端口：{profile.listenPort}
                  </div>
                  <div className="mt-3 flex gap-2">
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() =>
                        setView({ kind: "edit", profileId: profile.id })
                      }
                    >
                      编辑 Profile
                    </Button>
                    <Button
                      variant="destructive"
                      size="sm"
                      disabled={profile.id === CODEX_DEFAULT_PROFILE_ID}
                      onClick={() => {
                        setDeleteError(null);
                        setView({
                          kind: "delete-confirm",
                          profileId: profile.id,
                        });
                      }}
                    >
                      删除 Profile
                    </Button>
                  </div>
                </section>
              ))}
            </div>
            <DialogFooter>
              <Button onClick={() => setView({ kind: "create" })}>
                新建 Profile
              </Button>
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                关闭
              </Button>
            </DialogFooter>
          </>
        )}

        {view.kind === "create" && (
          <div className="overflow-y-auto px-6 py-4">
            <CodexProfileForm
              key="create"
              mode="create"
              onSubmit={handleCreate}
              onCancel={() => setView({ kind: "list" })}
            />
          </div>
        )}

        {view.kind === "edit" && (
          <div className="overflow-y-auto px-6 py-4">
            {isEditLoading && (
              <p className="text-sm text-muted-foreground">
                正在加载 Profile 状态...
              </p>
            )}
            {!isEditLoading && editError && (
              <section className="space-y-4">
                <p role="alert" className="text-sm text-red-500">
                  {editError}
                </p>
                <Button
                  variant="outline"
                  onClick={() => setView({ kind: "list" })}
                >
                  返回列表
                </Button>
              </section>
            )}
            {!isEditLoading && editProfile && (
              <div className="space-y-3">
                {editIsActive && (
                  <section className="space-y-2">
                    <p className="text-sm text-amber-600 dark:text-amber-400">
                      请先停止该 Profile 的路由后再修改
                    </p>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={isStoppingRoute}
                      onClick={() => void handleStopRoute()}
                    >
                      {isStoppingRoute ? "停止中..." : "停止路由"}
                    </Button>
                  </section>
                )}
                {stopRouteError && (
                  <p role="alert" className="text-sm text-red-500">
                    {stopRouteError}
                  </p>
                )}
                <CodexProfileForm
                  key={editProfile.id}
                  mode="edit"
                  initialValues={{
                    name: editProfile.name,
                    homePath: editProfile.canonicalHomePath,
                    listenPort: editProfile.listenPort,
                  }}
                  canEditHome={canEditHome}
                  canEditPort={canEditPort}
                  onSubmit={handleUpdate}
                  onCancel={() => setView({ kind: "list" })}
                />
              </div>
            )}
          </div>
        )}

        {view.kind === "delete-confirm" && (
          <div className="space-y-4 overflow-y-auto px-6 py-4">
            {deleteTarget ? (
              <>
                <div className="rounded-md border border-border-default p-3">
                  <div className="font-medium">{deleteTarget.name}</div>
                  <div className="mt-1 break-all text-xs text-muted-foreground">
                    {deleteTarget.canonicalHomePath}
                  </div>
                </div>
                <p className="text-sm text-muted-foreground">
                  此操作只删除 CC Switch 中的绑定和本地 token，不删除 Home
                  目录，也不会删除其中的 auth.json、config.toml 或任何会话。
                </p>
              </>
            ) : (
              <p role="alert" className="text-sm text-red-500">
                找不到待删除的 Profile
              </p>
            )}
            {deleteError && (
              <p role="alert" className="text-sm text-red-500">
                {deleteError}
              </p>
            )}
            <div className="flex justify-end gap-2">
              <Button
                variant="outline"
                disabled={isDeleting}
                onClick={() => setView({ kind: "list" })}
              >
                取消
              </Button>
              <Button
                variant="destructive"
                disabled={isDeleting || !deleteTarget}
                onClick={() => void handleDelete()}
              >
                {isDeleting ? "删除中..." : "确认删除"}
              </Button>
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
