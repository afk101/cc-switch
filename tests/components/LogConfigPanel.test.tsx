import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LogConfigPanel } from "@/components/settings/LogConfigPanel";
import { settingsApi } from "@/lib/api/settings";

const { toastSuccess } = vi.hoisted(() => ({ toastSuccess: vi.fn() }));

vi.mock("sonner", () => ({
  toast: {
    success: toastSuccess,
    error: vi.fn(),
  },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

describe("LogConfigPanel", () => {
  beforeEach(() => {
    toastSuccess.mockReset();
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
});
