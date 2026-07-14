import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { CodexProfileManagerDialog } from "./CodexProfileManagerDialog";

describe("CodexProfileManagerDialog", () => {
  it("默认 Profile 可修改端口但不可删除或重绑", () => {
    render(
      <CodexProfileManagerDialog
        open
        profiles={[
          {
            id: "default",
            name: "默认 Codex",
            canonicalHomePath: "/Users/test/.codex",
            listenPort: 15_722,
            createdAt: 1,
            updatedAt: 1,
          },
        ]}
        onOpenChange={() => undefined}
        onUpdatePort={() => undefined}
        onDelete={() => undefined}
        onRebind={() => undefined}
        onCreate={() => undefined}
      />,
    );

    expect(screen.getByLabelText("监听端口")).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "重新绑定 Home" }),
    ).toBeDisabled();
    expect(screen.getByRole("button", { name: "删除 Profile" })).toBeDisabled();
  });
});
