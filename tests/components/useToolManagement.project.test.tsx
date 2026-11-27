import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ToolInstallationReport } from "@/lib/api/settings";
import type { ToolVersion } from "@/components/apps/useToolManagement";
import type { Settings } from "@/types";
import { deferred, report, uninstallPlan } from "./appsPageTestHelpers";

const mocks = vi.hoisted(() => ({
  getToolVersions: vi.fn(),
  listToolInstallations: vi.fn(),
  probeToolInstallations: vi.fn(),
  runToolLifecycleAction: vi.fn(),
}));
vi.mock("@/lib/api", () => ({ settingsApi: mocks }));
vi.mock("@/lib/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
}));

const version = (name: string, installed = true): ToolVersion => ({
  name,
  version: installed ? "1.0.0" : null,
  latest_version: "1.0.0",
  error: null,
  installed_but_broken: false,
  env_type: "windows",
  wsl_distro: null,
  update_blocked_by_node: null,
});

const installedReport = (tool: string) =>
  report(tool, {
    installs: [
      {
        path: `C:/tools/${tool}.exe`,
        source: "npm",
        version: "1.0.0",
        runnable: true,
        error: null,
        is_path_default: true,
      },
    ],
  });

beforeEach(() => {
  vi.resetModules();
  mocks.getToolVersions
    .mockReset()
    .mockImplementation(async (tools: string[]) =>
      tools.map((tool) => version(tool)),
    );
  mocks.listToolInstallations
    .mockReset()
    .mockImplementation(async (tools: string[]) => tools.map(installedReport));
  mocks.probeToolInstallations
    .mockReset()
    .mockImplementation(async (tools: string[]) => tools.map(installedReport));
  mocks.runToolLifecycleAction.mockReset().mockResolvedValue(undefined);
});

async function mountTools() {
  const { useToolManagement } = await import(
    "@/components/apps/useToolManagement"
  );
  const hook = renderHook(() => useToolManagement());
  await waitFor(() => expect(hook.result.current.isLoadingTools).toBe(false));
  await waitFor(() =>
    expect(hook.result.current.installReports?.claude).toBeDefined(),
  );
  return { ...hook, useToolManagement };
}

describe("project environment tool checks", () => {
  it("retains displayed data but exposes failed or missing check results until a fresh check succeeds", async () => {
    const { result } = await mountTools();
    const oldReport = result.current.installReports?.claude;
    mocks.getToolVersions.mockRejectedValueOnce(
      new Error("version probe failed"),
    );
    mocks.probeToolInstallations.mockRejectedValueOnce(
      new Error("installation probe failed"),
    );
    await act(async () => result.current.checkTool("claude"));
    expect(result.current.toolVersionByName.get("claude")?.version).toBe(
      "1.0.0",
    );
    expect(result.current.installReports?.claude).toBe(oldReport);
    expect(result.current.toolCheckErrors.claude).toContain(
      "version probe failed",
    );
    expect(result.current.installationCheckErrors.claude).toContain(
      "installation probe failed",
    );
    expect(result.current.toolCheckErrors.codex).toBeUndefined();

    mocks.getToolVersions.mockResolvedValueOnce([]);
    mocks.probeToolInstallations.mockResolvedValueOnce([]);
    await act(async () => result.current.checkTool("claude"));
    expect(result.current.toolCheckErrors.claude).toBeTruthy();
    expect(result.current.installationCheckErrors.claude).toBeTruthy();
    await act(async () => result.current.checkTool("claude"));
    expect(result.current.toolCheckErrors.claude).toBeUndefined();
    expect(result.current.installationCheckErrors.claude).toBeUndefined();
  });

  it("does not let an older failed check overwrite a newer successful result", async () => {
    const { result } = await mountTools();
    const versions = deferred<ToolVersion[]>();
    const installations = deferred<ToolInstallationReport[]>();
    mocks.getToolVersions.mockReturnValueOnce(versions.promise);
    mocks.probeToolInstallations.mockReturnValueOnce(installations.promise);
    let oldCheck!: Promise<void>;
    act(() => {
      oldCheck = result.current.checkTool("claude");
    });
    await act(async () => result.current.checkTool("claude"));
    await act(async () => {
      versions.reject(new Error("late version error"));
      installations.reject(new Error("late installation error"));
      await oldCheck;
    });
    expect(result.current.toolCheckErrors.claude).toBeUndefined();
    expect(result.current.installationCheckErrors.claude).toBeUndefined();
    expect(result.current.installReports?.claude?.installs[0].path).toBe(
      "C:/tools/claude.exe",
    );
  });

  it("keeps installation busy until its refreshed location and source are available", async () => {
    let installed = false;
    mocks.getToolVersions.mockImplementation(async (tools: string[]) =>
      tools.map((tool) => version(tool, tool !== "claude" || installed)),
    );
    mocks.listToolInstallations.mockImplementation(async (tools: string[]) =>
      tools.map((tool) => report(tool)),
    );
    mocks.runToolLifecycleAction.mockImplementation(async () => {
      installed = true;
    });
    const { result } = await mountTools();
    const installations = deferred<ToolInstallationReport[]>();
    mocks.probeToolInstallations.mockReturnValueOnce(installations.promise);
    let installation!: Promise<void>;
    act(() => {
      installation = result.current.handleRunToolAction(["claude"], "install");
    });
    await waitFor(() =>
      expect(mocks.probeToolInstallations).toHaveBeenCalledWith(["claude"]),
    );
    expect(result.current.busyTools.has("claude")).toBe(true);
    expect(result.current.installReports?.claude?.installs).toEqual([]);
    await act(async () => {
      installations.resolve([installedReport("claude")]);
      await installation;
    });
    expect(mocks.runToolLifecycleAction).toHaveBeenCalledTimes(1);
    expect(result.current.busyTools.has("claude")).toBe(false);
    expect(result.current.installReports?.claude?.installs[0]).toMatchObject({
      path: "C:/tools/claude.exe",
      source: "npm",
      runnable: true,
    });
    expect(result.current.toolActionErrors.claude).toBeUndefined();
  });

  it("retains operation errors across remounts and clears them after a successful recheck", async () => {
    const { result, unmount, useToolManagement } = await mountTools();
    mocks.runToolLifecycleAction.mockRejectedValueOnce(
      new Error("installer command failed"),
    );
    await act(async () =>
      result.current.handleRunToolAction(["claude"], "install"),
    );
    expect(result.current.toolActionErrors.claude).toContain(
      "installer command failed",
    );
    expect(result.current.toolActionErrors.codex).toBeUndefined();
    unmount();
    const next = renderHook(() => useToolManagement());
    expect(next.result.current.toolActionErrors.claude).toContain(
      "installer command failed",
    );
    await act(async () => next.result.current.checkTool("claude"));
    expect(next.result.current.toolActionErrors.claude).toBeUndefined();
  });
});

describe("sidebar installation availability", () => {
  it("checks locally once, serially, and shares results between subscribers", async () => {
    const first = deferred<ToolVersion[]>();
    mocks.getToolVersions.mockReturnValueOnce(first.promise);
    const { useInstalledTools, TOOL_NAMES } = await import(
      "@/components/apps/useToolManagement"
    );
    const hook = renderHook(() => useInstalledTools());
    const other = renderHook(() => useInstalledTools());
    expect(hook.result.current.isPending).toBe(true);
    expect(mocks.getToolVersions).toHaveBeenCalledTimes(1);
    await act(async () => first.resolve([version("claude")]));
    await waitFor(() => expect(hook.result.current.isPending).toBe(false));
    expect(mocks.getToolVersions.mock.calls).toEqual(
      TOOL_NAMES.map((tool) => [[tool], undefined, true]),
    );
    expect(other.result.current.installedTools).toEqual(
      hook.result.current.installedTools,
    );
    expect(mocks.listToolInstallations).not.toHaveBeenCalled();
    hook.unmount();
    other.unmount();
    renderHook(() => useInstalledTools());
    expect(mocks.getToolVersions).toHaveBeenCalledTimes(TOOL_NAMES.length);
  });

  it("distinguishes absence and unknown errors while recognizing broken and WSL installs", async () => {
    mocks.getToolVersions.mockImplementation(async ([tool]: string[]) => {
      if (tool === "grok") throw new Error("probe rejected");
      if (tool === "opencode") return [];
      if (tool === "claude") return [version(tool, false)];
      if (tool === "codex") return [{ ...version(tool), env_type: "wsl" }];
      if (tool === "gemini")
        return [
          {
            ...version(tool, false),
            installed_but_broken: true,
            error: "broken",
          },
        ];
      if (tool === "hermes")
        return [{ ...version(tool, false), error: "WSL shell failed" }];
      return [version(tool)];
    });
    const { useInstalledTools } = await import(
      "@/components/apps/useToolManagement"
    );
    const { result } = renderHook(() => useInstalledTools());
    await waitFor(() => expect(result.current.isPending).toBe(false));
    expect(result.current.installedTools).toMatchObject({
      claude: false,
      codex: true,
      gemini: true,
    });
    expect(result.current.installedTools.grok).toBeUndefined();
    expect(result.current.installedTools.opencode).toBeUndefined();
    expect(result.current.installedTools.hermes).toBeUndefined();
  });

  it("reflects install and uninstall rechecks without changing user preferences", async () => {
    let installed = false;
    mocks.getToolVersions.mockImplementation(async (tools: string[]) =>
      tools.map((tool) => version(tool, tool !== "claude" || installed)),
    );
    mocks.runToolLifecycleAction.mockImplementation(async (_tools, action) => {
      installed = action !== "uninstall";
    });
    mocks.probeToolInstallations.mockImplementation(async (tools: string[]) =>
      tools.map((tool) => (installed ? installedReport(tool) : report(tool))),
    );
    const { result: tools } = await mountTools();
    const { useInstalledTools } = await import(
      "@/components/apps/useToolManagement"
    );
    const { result } = renderHook(() => useInstalledTools());
    expect(result.current.installedTools.claude).toBe(false);
    await act(async () =>
      tools.current.handleRunToolAction(["claude"], "install"),
    );
    expect(result.current.installedTools.claude).toBe(true);
    await act(async () =>
      tools.current.handleRunToolAction(["claude"], "uninstall", {
        uninstallPlan: uninstallPlan("claude"),
      }),
    );
    expect(result.current.installedTools.claude).toBe(false);
  });

  it("keeps the last confirmed presence on failed rechecks, then accepts confirmed removal", async () => {
    const { result: tools } = await mountTools();
    const { useInstalledTools } = await import(
      "@/components/apps/useToolManagement"
    );
    const { result } = renderHook(() => useInstalledTools());
    expect(result.current.installedTools.claude).toBe(true);
    mocks.getToolVersions.mockRejectedValueOnce(new Error("failed check"));
    await act(async () => tools.current.checkTool("claude"));
    expect(result.current.installedTools.claude).toBe(true);
    mocks.getToolVersions.mockResolvedValueOnce([
      { ...version("claude", false), error: "WSL unavailable" },
    ]);
    await act(async () => tools.current.checkTool("claude"));
    expect(result.current.installedTools.claude).toBe(true);
    mocks.getToolVersions.mockResolvedValueOnce([version("claude", false)]);
    await act(async () => tools.current.checkTool("claude"));
    expect(result.current.installedTools.claude).toBe(false);
  });

  it("ignores a late startup probe after a newer management check", async () => {
    const slow = deferred<ToolVersion[]>();
    mocks.getToolVersions.mockReturnValueOnce(slow.promise);
    const { useInstalledTools, useToolManagement } = await import(
      "@/components/apps/useToolManagement"
    );
    const { result } = renderHook(() => useInstalledTools());
    const tools = renderHook(() => useToolManagement());
    await waitFor(() =>
      expect(tools.result.current.isLoadingTools).toBe(false),
    );
    expect(result.current.installedTools.claude).toBe(true);
    await act(async () => slow.resolve([version("claude", false)]));
    await waitFor(() => expect(result.current.isPending).toBe(false));
    expect(result.current.installedTools.claude).toBe(true);
    expect(
      tools.result.current.toolVersionByName.get("claude")?.latest_version,
    ).toBe("1.0.0");
  });
});

describe("saved Agent directory changes", () => {
  const native: Settings = {
    showInTray: true,
    minimizeToTrayOnClose: true,
    claudeConfigDir: "C:/Users/me/.claude",
  };
  const wsl: Settings = {
    ...native,
    claudeConfigDir: "\\\\wsl$\\Ubuntu\\home\\me\\.claude",
  };

  it("clears only the changed environment and ignores old version and installation responses", async () => {
    const { result } = await mountTools();
    const { refreshToolsForDirectoryChanges } = await import(
      "@/components/apps/useToolManagement"
    );
    const oldVersions = deferred<ToolVersion[]>();
    const oldReports = deferred<ToolInstallationReport[]>();
    mocks.getToolVersions.mockReturnValueOnce(oldVersions.promise);
    mocks.probeToolInstallations.mockReturnValueOnce(oldReports.promise);
    let oldCheck!: Promise<void>;
    act(() => {
      oldCheck = result.current.checkTool("claude");
    });
    const newVersions = deferred<ToolVersion[]>();
    mocks.getToolVersions.mockReturnValueOnce(newVersions.promise);
    mocks.listToolInstallations.mockResolvedValueOnce([report("claude")]);
    let refreshed!: Promise<void>;
    act(() => {
      refreshed = refreshToolsForDirectoryChanges(native, wsl);
    });
    expect(result.current.installedTools.claude).toBeUndefined();
    expect(result.current.toolVersionByName.has("claude")).toBe(false);
    expect(result.current.installReports?.claude).toBeUndefined();
    expect(result.current.loadingTools.claude).toBe(true);
    expect(result.current.installedTools.codex).toBe(true);
    expect(result.current.installReports?.codex).toEqual(
      installedReport("codex"),
    );
    expect(mocks.getToolVersions).toHaveBeenLastCalledWith(
      ["claude"],
      undefined,
      true,
    );
    await act(async () => {
      oldVersions.resolve([version("claude")]);
      oldReports.resolve([installedReport("claude")]);
      await oldCheck;
    });
    expect(result.current.installedTools.claude).toBeUndefined();
    expect(result.current.loadingTools.claude).toBe(true);
    await act(async () => {
      newVersions.resolve([
        {
          ...version("claude", false),
          env_type: "wsl",
          wsl_distro: "Ubuntu",
          latest_version: null,
        },
      ]);
      await refreshed;
    });
    expect(result.current.installedTools.claude).toBe(false);
    expect(result.current.loadingTools.claude).toBe(false);
    expect(result.current.toolVersionByName.get("claude")?.env_type).toBe(
      "wsl",
    );
    expect(result.current.installReports?.claude?.installs).toEqual([]);
    expect(result.current.lastCheckedAt).toBeNull();
  });

  it("does not preserve confirmed presence or old operation errors after a failed check in another environment", async () => {
    const { result } = await mountTools();
    const { refreshToolsForDirectoryChanges } = await import(
      "@/components/apps/useToolManagement"
    );
    mocks.runToolLifecycleAction.mockRejectedValueOnce(
      new Error("old install failed"),
    );
    await act(async () =>
      result.current.handleRunToolAction(["claude"], "install"),
    );
    expect(result.current.toolActionErrors.claude).toBeTruthy();
    mocks.getToolVersions.mockRejectedValueOnce(new Error("WSL unavailable"));
    mocks.listToolInstallations.mockRejectedValueOnce(
      new Error("new report failed"),
    );
    await act(async () => refreshToolsForDirectoryChanges(native, wsl));
    expect(result.current.installedTools.claude).toBeUndefined();
    expect(result.current.toolVersionByName.has("claude")).toBe(false);
    expect(result.current.installReports?.claude).toBeUndefined();
    expect(result.current.toolActionErrors.claude).toBeUndefined();
    expect(result.current.toolCheckErrors.claude).toBe("WSL unavailable");
    expect(result.current.installationCheckErrors.claude).toBe(
      "new report failed",
    );
    expect(result.current.loadingTools.claude).toBe(false);
  });

  it("does not recheck unchanged sanitized directories or unrelated preferences", async () => {
    await mountTools();
    const { refreshToolsForDirectoryChanges } = await import(
      "@/components/apps/useToolManagement"
    );
    mocks.getToolVersions.mockClear();
    mocks.listToolInstallations.mockClear();
    await act(async () =>
      refreshToolsForDirectoryChanges(native, {
        ...native,
        claudeConfigDir: `  ${native.claudeConfigDir}  `,
        language: "en",
      }),
    );
    expect(mocks.getToolVersions).not.toHaveBeenCalled();
    expect(mocks.listToolInstallations).not.toHaveBeenCalled();
  });

  it("cancels an old environment's pending upgrade confirmation", async () => {
    const { result } = await mountTools();
    const { refreshToolsForDirectoryChanges } = await import(
      "@/components/apps/useToolManagement"
    );
    mocks.probeToolInstallations.mockResolvedValueOnce([
      report("claude", { needs_confirmation: true }),
    ]);
    await act(async () =>
      result.current.handleRunToolAction(["claude"], "update"),
    );
    expect(result.current.pendingUpgrade).not.toBeNull();
    const confirmOldUpgrade = result.current.handleConfirmUpgrade;
    await act(async () => refreshToolsForDirectoryChanges(native, wsl));
    expect(result.current.pendingUpgrade).toBeNull();
    expect(result.current.busyTools.has("claude")).toBe(false);
    act(confirmOldUpgrade);
    expect(mocks.runToolLifecycleAction).not.toHaveBeenCalled();
  });

  it("ignores lifecycle failures and upgrade preflights from the previous environment", async () => {
    const { result } = await mountTools();
    const { refreshToolsForDirectoryChanges } = await import(
      "@/components/apps/useToolManagement"
    );
    const action = deferred<void>();
    mocks.runToolLifecycleAction.mockReturnValueOnce(action.promise);
    let operation!: Promise<void>;
    act(() => {
      operation = result.current.handleRunToolAction(["claude"], "install");
    });
    await act(async () => refreshToolsForDirectoryChanges(native, wsl));
    await act(async () => {
      action.reject(new Error("old installer failed"));
      await operation;
    });
    expect(result.current.toolActionErrors.claude).toBeUndefined();
    expect(result.current.busyTools.has("claude")).toBe(false);

    const preflight = deferred<ToolInstallationReport[]>();
    mocks.probeToolInstallations.mockReturnValueOnce(preflight.promise);
    act(() => {
      operation = result.current.handleRunToolAction(["claude"], "update");
    });
    await act(async () => refreshToolsForDirectoryChanges(wsl, native));
    await act(async () => {
      preflight.resolve([report("claude", { needs_confirmation: true })]);
      await operation;
    });
    expect(result.current.pendingUpgrade).toBeNull();
    expect(result.current.busyTools.has("claude")).toBe(false);
    expect(mocks.runToolLifecycleAction).toHaveBeenCalledTimes(1);
  });
});
