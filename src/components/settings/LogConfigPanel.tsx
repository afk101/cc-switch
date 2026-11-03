import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "@/lib/toast";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { Button } from "@/components/ui/button";
import { HelpTip } from "@/components/ui/help-tip";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { settingsApi, type LogConfig } from "@/lib/api/settings";
import { LOG_EXPORT_ERROR_CODES } from "@/config/constants";

const LOG_LEVELS = ["error", "warn", "info", "debug", "trace"] as const;

/** 把后端稳定错误码映射到本地化消息键。 */
function getExportErrorMessageKey(error: unknown): string {
  return String(error).startsWith(LOG_EXPORT_ERROR_CODES.NO_LOGS)
    ? "settings.advanced.logConfig.exportNoLogs"
    : "settings.advanced.logConfig.exportFailed";
}

export function LogConfigPanel() {
  const { t } = useTranslation();
  const [config, setConfig] = useState<LogConfig>({
    enabled: true,
    level: "info",
  });
  const [isLoading, setIsLoading] = useState(true);
  const [isExporting, setIsExporting] = useState(false);

  useEffect(() => {
    settingsApi
      .getLogConfig()
      .then(setConfig)
      .catch((e) => console.error("Failed to load log config:", e))
      .finally(() => setIsLoading(false));
  }, []);

  const handleChange = async (updates: Partial<LogConfig>) => {
    const newConfig = { ...config, ...updates };
    setConfig(newConfig);
    try {
      await settingsApi.setLogConfig(newConfig);
    } catch (e) {
      console.error("Failed to save log config:", e);
      toast.error(String(e));
      setConfig(config);
    }
  };

  /** 在后台导出日志，并通过消息展示完整保存路径。 */
  const handleExport = async () => {
    if (isExporting) return;
    setIsExporting(true);
    try {
      const path = await settingsApi.exportLogs();
      toast.success(t("settings.advanced.logConfig.exportSuccess"), {
        description: path,
      });
    } catch (e) {
      toast.error(t(getExportErrorMessageKey(e)));
    } finally {
      setIsExporting(false);
    }
  };

  if (isLoading) return null;

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-1">
          <Label htmlFor="diagnostic-log-enabled">
            {t("settings.advanced.logConfig.enabled")}
          </Label>
          <HelpTip title={t("settings.advanced.logConfig.enabled")}>
            {t("settings.advanced.logConfig.enabledDescription")}
          </HelpTip>
        </div>
        <Switch
          id="diagnostic-log-enabled"
          checked={config.enabled}
          onCheckedChange={(checked) => handleChange({ enabled: checked })}
        />
      </div>

      <div className="flex items-center justify-between">
        <div className="flex items-center gap-1">
          <Label htmlFor="diagnostic-log-level">
            {t("settings.advanced.logConfig.level")}
          </Label>
          <HelpTip title={t("settings.advanced.logConfig.level")}>
            <div className="space-y-1.5">
              <p>{t("settings.advanced.logConfig.levelDescription")}</p>
              {LOG_LEVELS.map((level) => (
                <p key={level}>
                  <span className="font-mono">{level}</span> —{" "}
                  {t(`settings.advanced.logConfig.levelDesc.${level}`)}
                </p>
              ))}
            </div>
          </HelpTip>
        </div>
        <Select
          value={config.level}
          disabled={!config.enabled}
          onValueChange={(value) =>
            handleChange({ level: value as LogConfig["level"] })
          }
        >
          <SelectTrigger id="diagnostic-log-level" className="w-[120px]">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {LOG_LEVELS.map((level) => (
              <SelectItem key={level} value={level}>
                {t(`settings.advanced.logConfig.levels.${level}`)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className="flex items-center justify-between">
        <div className="space-y-0.5">
          <Label>{t("settings.advanced.logConfig.export")}</Label>
          <p className="text-xs text-muted-foreground">
            {t("settings.advanced.logConfig.exportDescription")}
          </p>
        </div>
        <Button
          type="button"
          variant="outline"
          disabled={isExporting}
          onClick={handleExport}
        >
          {t(
            isExporting
              ? "settings.advanced.logConfig.exporting"
              : "settings.advanced.logConfig.export",
          )}
        </Button>
      </div>

      {/* 日志级别说明 */}
      <div className="rounded-lg bg-muted/50 p-4 text-xs space-y-1.5">
        <p className="font-medium text-muted-foreground mb-2">
          {t("settings.advanced.logConfig.levelHint")}
        </p>
        <div className="grid gap-1 text-muted-foreground">
          <p>
            <span className="font-mono text-red-500">error</span> -{" "}
            {t("settings.advanced.logConfig.levelDesc.error")}
          </p>
          <p>
            <span className="font-mono text-orange-500">warn</span> -{" "}
            {t("settings.advanced.logConfig.levelDesc.warn")}
          </p>
          <p>
            <span className="font-mono text-blue-500">info</span> -{" "}
            {t("settings.advanced.logConfig.levelDesc.info")}
          </p>
          <p>
            <span className="font-mono text-green-500">debug</span> -{" "}
            {t("settings.advanced.logConfig.levelDesc.debug")}
          </p>
          <p>
            <span className="font-mono text-gray-500">trace</span> -{" "}
            {t("settings.advanced.logConfig.levelDesc.trace")}
          </p>
        </div>
      </div>
    </div>
  );
}
