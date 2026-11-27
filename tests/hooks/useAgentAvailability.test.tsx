import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { createElement, type PropsWithChildren } from "react";
import type { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ToolVersion } from "@/components/apps/useToolManagement";

const mocks = vi.hoisted(() => ({
  settingsApi: {
    getToolVersions: vi.fn(),
    getConfigStatus: vi.fn(),
    listToolInstallations: vi.fn(),
  },
  providersApi: { getClaudeDesktopStatus: vi.fn() },
}));
vi.mock("@/lib/api", () => ({ settingsApi: mocks.settingsApi }));
vi.mock("@/lib/api/settings", () => ({ settingsApi: mocks.settingsApi }));
vi.mock("@/lib/api/providers", () => ({ providersApi: mocks.providersApi }));
vi.mock("@/lib/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
}));

let queryRuntime: typeof import("@tanstack/react-query");
let queryClient: QueryClient;
let availability: typeof import("@/hooks/useAgentAvailability");
let toolManagement: typeof import("@/components/apps/useToolManagement");
let wasOnline: boolean;

const version = (name: string): ToolVersion => ({
  name,
  version: name === "codex" || name === "grok" ? "1.0.0" : null,
  latest_version: null,
  error: null,
  installed_but_broken: false,
  env_type: name === "codex" ? "wsl" : "windows",
  wsl_distro: name === "codex" ? "Ubuntu" : null,
  update_blocked_by_node: null,
});

function wrapper({ children }: PropsWithChildren) {
  return createElement(
    queryRuntime.QueryClientProvider,
    { client: queryClient },
    children,
  );
}

beforeEach(async () => {
  vi.resetModules();
  mocks.settingsApi.getToolVersions
    .mockReset()
    .mockImplementation(async (tools: string[]) => tools.map(version));
  mocks.settingsApi.getConfigStatus
    .mockReset()
    .mockResolvedValue({ exists: false, path: "C:/test/.workbuddy" });
  mocks.settingsApi.listToolInstallations.mockReset();
  mocks.providersApi.getClaudeDesktopStatus
    .mockReset()
    .mockResolvedValue({ supported: true, installed: true, configured: false });

  // Reload both the provider and the hook after resetting the module store so
  // their React Query contexts remain the same instance.
  queryRuntime = await import("@tanstack/react-query");
  availability = await import("@/hooks/useAgentAvailability");
  toolManagement = await import("@/components/apps/useToolManagement");
  queryClient = new queryRuntime.QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: Infinity } },
  });
  wasOnline = queryRuntime.onlineManager.isOnline();
});

afterEach(() => {
  cleanup();
  queryClient.clear();
  queryRuntime.onlineManager.setOnline(wasOnline);
});

describe("useAgentAvailability", () => {
  it("finishes local installation checks offline and shares mapped results between subscribers", async () => {
    queryRuntime.onlineManager.setOnline(false);
    const first = renderHook(() => availability.useAgentAvailability(), {
      wrapper,
    });
    const second = renderHook(() => availability.useAgentAvailability(), {
      wrapper,
    });

    await waitFor(() => expect(first.result.current.isPending).toBe(false));
    expect(second.result.current.isPending).toBe(false);
    expect(first.result.current.installedApps).toMatchObject({
      claude: false,
      codex: true,
      grokbuild: true,
      "claude-desktop": true,
      workbuddy: false,
    });
    expect(first.result.current.installedApps).not.toHaveProperty("grok");
    expect(second.result.current.installedApps).toEqual(
      first.result.current.installedApps,
    );
    expect(mocks.settingsApi.getToolVersions.mock.calls).toEqual(
      toolManagement.TOOL_NAMES.map((tool) => [[tool], undefined, true]),
    );
    expect(mocks.providersApi.getClaudeDesktopStatus).toHaveBeenCalledTimes(1);
    expect(mocks.settingsApi.getConfigStatus).toHaveBeenCalledTimes(1);
    expect(mocks.settingsApi.getConfigStatus).toHaveBeenCalledWith("workbuddy");
    expect(mocks.settingsApi.listToolInstallations).not.toHaveBeenCalled();
  });

  it("refreshes shared desktop queries and retains confirmed presence when subsequent checks fail", async () => {
    const { result } = renderHook(() => availability.useAgentAvailability(), {
      wrapper,
    });
    await waitFor(() => expect(result.current.isPending).toBe(false));

    mocks.providersApi.getClaudeDesktopStatus.mockResolvedValue({
      supported: true,
      installed: false,
      configured: false,
    });
    mocks.settingsApi.getConfigStatus.mockResolvedValue({
      exists: true,
      path: "C:/test/.workbuddy",
    });
    const refreshDesktopQueries = () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: ["claudeDesktopStatus"] }),
        queryClient.invalidateQueries({
          queryKey: ["config-status", "workbuddy"],
        }),
      ]);
    await act(async () => {
      await refreshDesktopQueries();
    });
    await waitFor(() =>
      expect(result.current.installedApps).toMatchObject({
        "claude-desktop": false,
        workbuddy: true,
      }),
    );
    const confirmed = result.current.installedApps;

    mocks.providersApi.getClaudeDesktopStatus.mockRejectedValue(
      new Error("Desktop probe failed"),
    );
    mocks.settingsApi.getConfigStatus.mockRejectedValue(
      new Error("WorkBuddy probe failed"),
    );
    mocks.settingsApi.getToolVersions.mockRejectedValue(
      new Error("Version probe failed"),
    );
    await act(async () => {
      await Promise.all([
        refreshDesktopQueries(),
        toolManagement.checkToolUpdatesInBackground(),
      ]);
    });

    expect(queryClient.getQueryState(["claudeDesktopStatus"])?.status).toBe(
      "error",
    );
    expect(
      queryClient.getQueryState(["config-status", "workbuddy"])?.status,
    ).toBe("error");
    expect(result.current.isPending).toBe(false);
    expect(result.current.installedApps).toEqual(confirmed);
    expect(mocks.providersApi.getClaudeDesktopStatus).toHaveBeenCalledTimes(3);
    expect(mocks.settingsApi.getConfigStatus).toHaveBeenCalledTimes(3);
  });
});
