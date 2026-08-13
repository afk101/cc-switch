import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LogConfigPanel } from "@/components/settings/LogConfigPanel";
import { settingsApi } from "@/lib/api/settings";

const { toastError, toastSuccess } = vi.hoisted(() => ({
  toastError: vi.fn(),
  toastSuccess: vi.fn(),
}));

vi.mock("sonner", () => ({
  toast: {
    success: toastSuccess,
    error: toastError,
  },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

describe("LogConfigPanel", () => {
  beforeEach(() => {
    toastSuccess.mockReset();
    toastError.mockReset();
    vi.spyOn(settingsApi, "getLogConfig").mockResolvedValue({
      enabled: true,
      level: "info",
    });
  });

  it("导出期间禁止重复触发并在成功后展示完整路径", async () => {
    const user = userEvent.setup();
    let finishExport: ((path: string) => void) | undefined;
    const exportLogs = vi.spyOn(settingsApi, "exportLogs").mockReturnValue(
      new Promise((resolve) => {
        finishExport = resolve;
      }),
    );
    render(<LogConfigPanel />);
    const exportButton = await screen.findByRole("button", {
      name: "settings.advanced.logConfig.export",
    });

    await user.click(exportButton);

    expect(exportButton).toBeDisabled();
    expect(exportButton).toHaveTextContent(
      "settings.advanced.logConfig.exporting",
    );
    await user.click(exportButton);
    expect(exportLogs).toHaveBeenCalledTimes(1);

    finishExport?.("/Users/test/Downloads/cc-switch-logs-20260813-120000.zip");
    await waitFor(() =>
      expect(toastSuccess).toHaveBeenCalledWith(
        "settings.advanced.logConfig.exportSuccess",
        {
          description:
            "/Users/test/Downloads/cc-switch-logs-20260813-120000.zip",
        },
      ),
    );
    expect(exportButton).toBeEnabled();
  });

  it.each([
    ["NO_LOGS", "settings.advanced.logConfig.exportNoLogs"],
    [
      "LOG_EXPORT_FAILED: unavailable",
      "settings.advanced.logConfig.exportFailed",
    ],
  ])("导出错误 %s 显示本地化提示并恢复按钮", async (error, message) => {
    const user = userEvent.setup();
    vi.spyOn(settingsApi, "exportLogs").mockRejectedValue(error);
    render(<LogConfigPanel />);
    const exportButton = await screen.findByRole("button", {
      name: "settings.advanced.logConfig.export",
    });

    await user.click(exportButton);

    await waitFor(() => expect(toastError).toHaveBeenCalledWith(message));
    expect(exportButton).toBeEnabled();
  });
});
