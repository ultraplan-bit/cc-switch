import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CodexOAuthSection } from "@/components/providers/forms/CodexOAuthSection";
import { CopilotAuthSection } from "@/components/providers/forms/CopilotAuthSection";

const authMocks = vi.hoisted(() => ({
  refetchCodex: vi.fn(),
  refetchCopilot: vi.fn(),
  storageError: null as string | null,
}));

const failedStatus = (refetchStatus: () => void) => ({
  accounts: [],
  defaultAccountId: null,
  storageError: null,
  isStatusSuccess: false,
  isStatusError: true,
  hasAnyAccount: false,
  pollingState: "idle" as const,
  deviceCode: null,
  error: null,
  isPolling: false,
  isAddingAccount: false,
  isRemovingAccount: false,
  isSettingDefaultAccount: false,
  addAccount: vi.fn(),
  removeAccount: vi.fn(),
  setDefaultAccount: vi.fn(),
  cancelAuth: vi.fn(),
  logout: vi.fn(),
  refetchStatus,
});

vi.mock("@/lib/query/accountPlans", () => ({
  useAccountQuotaPlans: () => ({}),
}));

vi.mock("@/components/providers/forms/hooks/useCodexOauth", () => ({
  useCodexOauth: () => failedStatus(authMocks.refetchCodex),
}));

vi.mock("@/components/providers/forms/hooks/useCopilotAuth", () => ({
  useCopilotAuth: () => ({
    ...failedStatus(authMocks.refetchCopilot),
    ...(authMocks.storageError
      ? {
          storageError: authMocks.storageError,
          isStatusSuccess: true,
          isStatusError: false,
        }
      : {}),
  }),
}));

// 「N 个供应商在用」要读供应商列表（React Query）；这里不关心，给空
vi.mock("@/components/providers/forms/hooks/useManagedAccountUsers", () => ({
  useManagedAccountUsers: () => () => [],
}));

describe("managed auth status failures", () => {
  beforeEach(() => {
    authMocks.storageError = null;
    authMocks.refetchCodex.mockResolvedValue(undefined);
    authMocks.refetchCopilot.mockResolvedValue(undefined);
  });

  it("shows a retryable error instead of an empty Codex account selector", () => {
    const onAccountSelect = vi.fn();
    render(
      <CodexOAuthSection
        mode="select"
        selectedAccountId="acct-existing"
        onAccountSelect={onAccountSelect}
      />,
    );

    expect(screen.getByRole("alert")).toHaveTextContent(
      "无法加载 ChatGPT 账号状态，请重试。",
    );
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(onAccountSelect).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    expect(authMocks.refetchCodex).toHaveBeenCalledTimes(1);
  });

  it("shows a retryable error instead of an empty Copilot account selector", () => {
    const onAccountSelect = vi.fn();
    render(
      <CopilotAuthSection
        mode="select"
        selectedAccountId="acct-existing"
        onAccountSelect={onAccountSelect}
      />,
    );

    expect(screen.getByRole("alert")).toHaveTextContent(
      "无法加载 GitHub Copilot 账号状态，请重试。",
    );
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(onAccountSelect).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    expect(authMocks.refetchCopilot).toHaveBeenCalledTimes(1);
  });

  it("keeps a bound Copilot account when the status response reports unreadable storage", () => {
    authMocks.storageError = "Unsupported Copilot account format";
    const onAccountSelect = vi.fn();
    render(
      <CopilotAuthSection
        mode="select"
        selectedAccountId="acct-existing"
        onAccountSelect={onAccountSelect}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(authMocks.storageError);
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(onAccountSelect).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    expect(authMocks.refetchCopilot).toHaveBeenCalledTimes(1);
  });

  it("offers storage retry in the auth center before adding an account", () => {
    authMocks.storageError = "Unreadable account file";
    render(<CopilotAuthSection mode="manage" />);
    expect(screen.getByText(/Unreadable account file/)).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "添加账号" }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    expect(authMocks.refetchCopilot).toHaveBeenCalledTimes(1);
  });
});
