import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LogConfigPanel } from "@/components/settings/LogConfigPanel";
import { settingsApi } from "@/lib/api/settings";

const { toastError, toastSuccess } = vi.hoisted(
  /** 创建可复位的消息 mock。 */
  () => ({
    toastError: vi.fn(),
    toastSuccess: vi.fn(),
  }),
);

/** 提供组件可观察的消息边界。 */
vi.mock(
  "sonner",
  /** 返回消息模块 mock。 */
  () => ({
    toast: {
      success: toastSuccess,
      error: toastError,
    },
  }),
);

/** 提供返回稳定消息键的翻译边界。 */
vi.mock(
  "react-i18next",
  /** 返回翻译模块 mock。 */
  () => ({
    /** 返回测试使用的翻译函数。 */
    useTranslation: () => ({
      /** 原样返回消息键。 */
      t: (key: string) => key,
    }),
  }),
);

/** 验证日志设置面板的公开交互。 */
function verifyLogConfigPanelInteractions() {
  beforeEach(
    /** 重置测试边界并提供默认日志配置。 */
    () => {
      toastSuccess.mockReset();
      toastError.mockReset();
      vi.spyOn(settingsApi, "getLogConfig").mockResolvedValue({
        enabled: true,
        level: "info",
      });
    },
  );

  /** 驱动一次受控完成的日志导出。 */
  async function verifySuccessfulExport() {
    const user = userEvent.setup();
    let finishExport: ((path: string) => void) | undefined;
    /** 保存 Promise 完成函数，供测试显式结束导出。 */
    function retainExportResolver(resolve: (path: string) => void) {
      finishExport = resolve;
    }
    const exportLogs = vi.spyOn(settingsApi, "exportLogs").mockReturnValue(
      /** 保持导出未完成，直到测试显式释放。 */
      new Promise(retainExportResolver),
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
    await waitFor(
      /** 等待成功消息反映后端返回的完整路径。 */
      () =>
        expect(toastSuccess).toHaveBeenCalledWith(
          "settings.advanced.logConfig.exportSuccess",
          {
            description:
              "/Users/test/Downloads/cc-switch-logs-20260813-120000.zip",
          },
        ),
    );
    expect(exportButton).toBeEnabled();
  }

  it("导出期间禁止重复触发并在成功后展示完整路径", verifySuccessfulExport);

  it.each([
    ["NO_LOGS", "settings.advanced.logConfig.exportNoLogs"],
    [
      "LOG_EXPORT_FAILED: unavailable",
      "settings.advanced.logConfig.exportFailed",
    ],
  ])(
    "导出错误 %s 显示本地化提示并恢复按钮",
    /** 验证稳定错误类别映射与恢复行为。 */
    async (error, message) => {
      const user = userEvent.setup();
      vi.spyOn(settingsApi, "exportLogs").mockRejectedValue(error);
      render(<LogConfigPanel />);
      const exportButton = await screen.findByRole("button", {
        name: "settings.advanced.logConfig.export",
      });

      await user.click(exportButton);

      await waitFor(
        /** 等待错误消息出现。 */
        () => expect(toastError).toHaveBeenCalledWith(message),
      );
      expect(exportButton).toBeEnabled();
    },
  );
}

describe("LogConfigPanel", verifyLogConfigPanelInteractions);
