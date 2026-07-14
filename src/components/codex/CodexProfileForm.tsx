import { useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  CODEX_PROFILE_MAX_PORT,
  CODEX_PROFILE_MIN_PORT,
} from "@/config/constants";
import { extractErrorMessage } from "@/utils/errorUtils";

export interface CodexProfileFormValues {
  name: string;
  homePath: string;
  listenPort?: number;
}

interface CodexProfileFormProps {
  mode: "create" | "edit";
  initialValues?: CodexProfileFormValues;
  canEditHome?: boolean;
  canEditPort?: boolean;
  onSubmit: (values: CodexProfileFormValues) => Promise<void> | void;
  onCancel: () => void;
}

/** 规范化必填文本，空值时抛出可展示错误。 */
function validateRequiredText(value: string, label: string): string {
  const normalized = value.trim();
  if (!normalized) {
    throw new Error(`${label}不能为空`);
  }
  return normalized;
}

/** 将可选端口文本转换为合法整数，空值表示自动分配。 */
function parseOptionalPort(value: string): number | undefined {
  const normalized = value.trim();
  if (!normalized) {
    return undefined;
  }
  const port = Number(normalized);
  if (
    !Number.isInteger(port) ||
    port < CODEX_PROFILE_MIN_PORT ||
    port > CODEX_PROFILE_MAX_PORT
  ) {
    throw new Error("监听端口必须是 1–65535 的整数");
  }
  return port;
}

/** 将受控字段转换为可以提交的 Profile 表单值。 */
function buildFormValues(
  mode: CodexProfileFormProps["mode"],
  name: string,
  homePath: string,
  portText: string,
): CodexProfileFormValues {
  const listenPort = parseOptionalPort(portText);
  if (mode === "edit" && listenPort === undefined) {
    throw new Error("监听端口不能为空");
  }
  return {
    name: validateRequiredText(name, "Profile 名称"),
    homePath: validateRequiredText(homePath, "CODEX_HOME 路径"),
    listenPort,
  };
}

/** 编辑或创建 Codex Profile 的受控表单。 */
export function CodexProfileForm({
  mode,
  initialValues,
  canEditHome = true,
  canEditPort = true,
  onSubmit,
  onCancel,
}: CodexProfileFormProps) {
  const [name, setName] = useState(initialValues?.name ?? "");
  const [homePath, setHomePath] = useState(initialValues?.homePath ?? "");
  const [portText, setPortText] = useState(
    initialValues?.listenPort === undefined
      ? ""
      : String(initialValues.listenPort),
  );
  const [error, setError] = useState<string | null>(null);
  const [isSubmitting, setIsSubmitting] = useState(false);

  /** 校验受控字段并提交，失败时保留当前输入。 */
  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    try {
      const values = buildFormValues(mode, name, homePath, portText);
      setIsSubmitting(true);
      await onSubmit(values);
    } catch (submitError) {
      setError(extractErrorMessage(submitError) || "Profile 操作失败");
    } finally {
      setIsSubmitting(false);
    }
  }

  const submitLabel = mode === "create" ? "创建" : "保存";

  return (
    <form className="space-y-4" onSubmit={handleSubmit}>
      <div className="space-y-2">
        <Label htmlFor="codex-profile-name">Profile 名称</Label>
        <Input
          id="codex-profile-name"
          value={name}
          disabled={isSubmitting}
          onChange={(event) => setName(event.target.value)}
        />
      </div>

      <div className="space-y-2">
        <Label htmlFor="codex-profile-home">CODEX_HOME 路径</Label>
        <Input
          id="codex-profile-home"
          value={homePath}
          disabled={isSubmitting || !canEditHome}
          onChange={(event) => setHomePath(event.target.value)}
        />
      </div>

      <div className="space-y-2">
        <Label htmlFor="codex-profile-port">监听端口</Label>
        <Input
          id="codex-profile-port"
          type="text"
          inputMode="numeric"
          placeholder={mode === "create" ? "留空自动分配" : undefined}
          value={portText}
          disabled={isSubmitting || !canEditPort}
          onChange={(event) => setPortText(event.target.value)}
        />
      </div>

      {error && (
        <p role="alert" className="text-sm text-red-500">
          {error}
        </p>
      )}

      <div className="flex justify-end gap-2">
        <Button
          type="button"
          variant="outline"
          disabled={isSubmitting}
          onClick={onCancel}
        >
          取消
        </Button>
        <Button type="submit" disabled={isSubmitting}>
          {isSubmitting ? `${submitLabel}中...` : submitLabel}
        </Button>
      </div>
    </form>
  );
}
