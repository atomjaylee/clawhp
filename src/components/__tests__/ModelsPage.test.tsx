import type { ReactNode } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ModelsPage from "../ModelsPage";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@/components/ui/scroll-area", () => ({
  ScrollArea: ({ children, className }: { children: ReactNode; className?: string }) => (
    <div className={className}>{children}</div>
  ),
}));

describe("ModelsPage", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "list_providers") {
        return [];
      }

      if (command === "get_primary_model") {
        return "";
      }

      if (command === "fetch_remote_models") {
        return {
          success: true,
          stdout: JSON.stringify(["claude-3-7-sonnet"]),
          stderr: "",
          code: 0,
        };
      }

      throw new Error(`Unexpected invoke command: ${command}`);
    });
  });

  it("hides compatibility protocol options in the add provider flow", async () => {
    render(<ModelsPage />);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("list_providers");
      expect(invokeMock).toHaveBeenCalledWith("get_primary_model");
    });

    fireEvent.click(screen.getAllByRole("button", { name: "添加 Provider" })[0]);

    expect(screen.queryByText("兼容协议")).not.toBeInTheDocument();
    expect(screen.queryByText("OpenAI 兼容")).not.toBeInTheDocument();
    expect(screen.queryByText("Anthropic 兼容")).not.toBeInTheDocument();
    expect(screen.getByText("API 地址")).toBeInTheDocument();
    expect(screen.getByText("API Key")).toBeInTheDocument();
    expect(screen.getByText("Provider 名称")).toBeInTheDocument();
  });

  it("keeps preset protocol selection internally when fetching models", async () => {
    render(<ModelsPage />);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("list_providers");
      expect(invokeMock).toHaveBeenCalledWith("get_primary_model");
    });

    fireEvent.click(screen.getAllByRole("button", { name: "添加 Provider" })[0]);
    fireEvent.click(screen.getByRole("button", { name: /Anthropic/i }));
    fireEvent.change(screen.getByPlaceholderText("sk-ant-..."), {
      target: { value: "sk-ant-test" },
    });
    fireEvent.click(screen.getByRole("button", { name: "获取模型列表" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("fetch_remote_models", {
        baseUrl: "https://api.anthropic.com/v1",
        apiKey: "sk-ant-test",
        apiAdapter: "anthropic-messages",
      });
    });
  });

  it("offers a Bailian preset with the official base URL and provider name", async () => {
    render(<ModelsPage />);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("list_providers");
      expect(invokeMock).toHaveBeenCalledWith("get_primary_model");
    });

    fireEvent.click(screen.getAllByRole("button", { name: "添加 Provider" })[0]);
    fireEvent.click(screen.getByRole("button", { name: /阿里百炼/i }));

    expect(screen.getByDisplayValue("https://coding.dashscope.aliyuncs.com/v1")).toBeInTheDocument();
    expect(screen.getByDisplayValue("bailian")).toBeInTheDocument();
  });
});
