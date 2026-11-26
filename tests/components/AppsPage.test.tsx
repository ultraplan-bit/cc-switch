import {
  act,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DndContextProps, DragEndEvent } from "@dnd-kit/core";
import { APP_IDS } from "@/config/appSpecs";
import {
  card,
  createToolState,
  deferred,
  isReady,
  registerScrollIntoViewStub,
  renderApps,
  report,
  resetAppsPageMocks,
  updateAllButton,
  updateButton,
  type ToolVersions,
} from "./appsPageTestHelpers";

const mocks = vi.hoisted(() => ({
  getToolVersions: vi.fn(),
  getConfigStatus: vi.fn(async () => ({
    exists: true,
    path: "/home/me/.workbuddy",
  })),
  probeToolInstallations: vi.fn(),
  listToolInstallations: vi.fn(),
  runToolLifecycleAction: vi.fn(),
  prepareToolUninstall: vi.fn(),
  copyText: vi.fn(),
  openExternal: vi.fn(),
  info: vi.fn(),
  success: vi.fn(),
  warning: vi.fn(),
  error: vi.fn(),
  settings: {} as { visibleApps?: undefined; appOrder?: string[] },
  updateSettings: vi.fn(),
  autoSaveSettings: vi.fn(async () => null),
  dragEndHandlers: [] as ((event: DragEndEvent) => void)[],
  desktopStatus: { supported: true, installed: true, configured: true },
}));

vi.mock("@/lib/api", () => ({ settingsApi: mocks }));
vi.mock("@/lib/api/providers", () => ({
  providersApi: {
    getClaudeDesktopStatus: async () => mocks.desktopStatus,
  },
}));
vi.mock("@/hooks/useSettings", () => ({
  useSettings: () => ({
    settings: mocks.settings,
    updateSettings: mocks.updateSettings,
    autoSaveSettings: mocks.autoSaveSettings,
  }),
}));
vi.mock("@dnd-kit/core", async () => {
  const actual =
    await vi.importActual<typeof import("@dnd-kit/core")>("@dnd-kit/core");
  return {
    ...actual,
    DndContext: (props: DndContextProps) => {
      if (props.onDragEnd) mocks.dragEndHandlers.push(props.onDragEnd);
      return <actual.DndContext {...props} />;
    },
  };
});
vi.mock("sonner", () => ({ toast: mocks }));
vi.mock("@/lib/clipboard", () => ({ copyText: mocks.copyText }));

const state = createToolState();
const { missing } = state;

/** 某一组里各行的应用 ID，按显示顺序。 */
function rowApps(region: string) {
  return Array.from(
    screen
      .getByRole("region", { name: region })
      .querySelectorAll<HTMLElement>("[data-tool-row]"),
    (row) => row.dataset.toolRow,
  );
}

registerScrollIntoViewStub();

beforeEach(() => {
  resetAppsPageMocks(mocks, state);
  mocks.openExternal.mockReset().mockResolvedValue(undefined);
});

describe("AppsPage layout, grouping and diagnostics", () => {
  it("opens official download pages for missing desktop apps and waits for detection before showing them", async () => {
    mocks.desktopStatus.installed = false;
    mocks.desktopStatus.configured = false;
    mocks.getConfigStatus.mockResolvedValue({
      exists: false,
      path: "/home/me/.workbuddy",
    });
    await renderApps();
    for (const [name, url] of [
      ["Claude Desktop", "https://claude.com/download"],
      ["WorkBuddy", "https://www.workbuddy.cn/"],
    ]) {
      await userEvent.click(
        card(name).getByRole("button", { name: "appsPage.officialDownload" }),
      );
      expect(mocks.openExternal).toHaveBeenLastCalledWith(url);
      expect(card(name).queryByRole("switch")).toBeNull();
    }
    expect(mocks.runToolLifecycleAction).not.toHaveBeenCalled();
    expect(mocks.autoSaveSettings).not.toHaveBeenCalled();

    mocks.desktopStatus.installed = true;
    mocks.getConfigStatus.mockResolvedValue({
      exists: true,
      path: "/home/me/.workbuddy",
    });
    await userEvent.click(
      screen.getByRole("button", { name: "appsPage.checkUpdates" }),
    );
    await waitFor(() =>
      expect(card("Claude Desktop").getByRole("switch")).toBeInTheDocument(),
    );
    await waitFor(() =>
      expect(card("WorkBuddy").getByRole("switch")).toBeInTheDocument(),
    );
    expect(
      screen.queryByRole("button", { name: "appsPage.officialDownload" }),
    ).toBeNull();
  });

  it("hides unsupported desktop downloads and reports browser opening failures", async () => {
    mocks.desktopStatus.installed = false;
    mocks.desktopStatus.supported = false;
    mocks.getConfigStatus.mockResolvedValue({
      exists: false,
      path: "/home/me/.workbuddy",
    });
    mocks.openExternal.mockRejectedValueOnce(new Error("browser unavailable"));
    await renderApps();
    expect(
      card("Claude Desktop").queryByRole("button", {
        name: "appsPage.officialDownload",
      }),
    ).toBeNull();
    await userEvent.click(
      card("WorkBuddy").getByRole("button", {
        name: "appsPage.officialDownload",
      }),
    );
    expect(mocks.error).toHaveBeenCalledWith("notifications.openLinkFailed");
  });

  it("waits for Codex loading even while the check-updates label is hidden in the DOM", async () => {
    const getVersions = mocks.getToolVersions.getMockImplementation()!;
    const codexReady = deferred<void>();
    mocks.getToolVersions.mockImplementation(async (tools: string[]) => {
      if (tools.includes("codex")) await codexReady.promise;
      return getVersions(tools);
    });
    // Finish the first dynamic import before starting a DOM wait. Under the full
    // suite, transforming AppsPage can take longer than findByText's timeout.
    await import("@/components/apps/AppsPage");
    const loading = renderApps();
    expect(await screen.findByText("appsPage.checkUpdates")).toHaveAttribute(
      "aria-hidden",
      "true",
    );
    expect(
      screen.queryByRole("button", { name: "appsPage.checkUpdates" }),
    ).toBeNull();
    expect(
      screen.getByRole("button", { name: "appsPage.checking" }),
    ).toBeDisabled();
    expect(card("Codex").queryByRole("switch")).toBeNull();
    await act(async () => codexReady.resolve());
    await loading;
    expect(updateButton("Codex")).toBeEnabled();
  });

  it("offers uninstall for installed tools and keeps missing tools installable", async () => {
    missing.add("codex");
    await renderApps();
    expect(
      card("Claude Code").getByRole("button", {
        name: "appPage.moreActions",
      }),
    ).toBeEnabled();
    expect(
      card("OpenCode").getByRole("button", {
        name: "appPage.moreActions",
      }),
    ).toBeEnabled();
    expect(
      card("Codex").queryByRole("button", {
        name: "appPage.moreActions",
      }),
    ).toBeNull();
    expect(
      card("Codex").getByRole("button", { name: "settings.toolInstall" }),
    ).toBeEnabled();
  });

  it("groups apps into installed and not installed sections", async () => {
    missing.add("codex");
    await renderApps();
    const installed = within(
      screen.getByRole("region", { name: "appsPage.sectionInstalled" }),
    );
    const notInstalled = within(
      screen.getByRole("region", { name: "appsPage.sectionNotInstalled" }),
    );
    expect(installed.getByText("Claude Code")).toBeInTheDocument();
    expect(installed.getByText("Claude Desktop")).toBeInTheDocument();
    expect(installed.queryByText("Codex")).toBeNull();
    expect(notInstalled.getByText("Codex")).toBeInTheDocument();
    expect(notInstalled.queryByText("Claude Code")).toBeNull();
  });

  it("groups Claude Desktop by whether it is installed, not whether it is connected", async () => {
    mocks.desktopStatus = {
      supported: true,
      installed: true,
      configured: false,
    };
    await renderApps();
    expect(rowApps("appsPage.sectionInstalled")).toContain("claude-desktop");
    expect(
      card("Claude Desktop").getByText("appsPage.desktopNotConnected"),
    ).toBeInTheDocument();
  });

  it("puts Claude Desktop under not installed when it is not on this machine", async () => {
    mocks.desktopStatus = {
      supported: true,
      installed: false,
      configured: false,
    };
    await renderApps();
    expect(rowApps("appsPage.sectionNotInstalled")).toEqual(["claude-desktop"]);
  });

  it("orders rows by the saved app order within each section", async () => {
    missing.add("codex").add("hermes");
    mocks.settings = {
      appOrder: [
        "hermes",
        "gemini",
        "claude",
        "claude-desktop",
        "codex",
        ...APP_IDS.filter(
          (app) =>
            !["hermes", "gemini", "claude", "claude-desktop", "codex"].includes(
              app,
            ),
        ),
      ],
    };
    await renderApps();
    expect(rowApps("appsPage.sectionInstalled").slice(0, 4)).toEqual([
      "gemini",
      "claude",
      "claude-desktop",
      "grokbuild",
    ]);
    expect(rowApps("appsPage.sectionNotInstalled")).toEqual([
      "hermes",
      "codex",
    ]);
    // 只有已安装的能拖
    expect(
      within(
        screen.getByRole("region", { name: "appsPage.sectionInstalled" }),
      ).getAllByRole("button", { name: "appsPage.dragHandle" }),
    ).toHaveLength(APP_IDS.length - 2);
    expect(
      within(
        screen.getByRole("region", { name: "appsPage.sectionNotInstalled" }),
      ).queryByRole("button", { name: "appsPage.dragHandle" }),
    ).toBeNull();
  });

  it("saves the new order when a row is dropped onto another in its section", async () => {
    await renderApps();
    // jsdom 不做布局，算不出拖动落点：直接调各组 DndContext 收到的 onDragEnd
    const onDragEnd = mocks.dragEndHandlers.at(-1);
    expect(onDragEnd).toBeDefined();
    act(() => {
      onDragEnd?.({
        active: { id: "claude" },
        over: { id: "codex" },
      } as unknown as DragEndEvent);
    });
    const next = [
      "claude-desktop",
      "codex",
      "claude",
      ...APP_IDS.filter(
        (app) => !["claude", "claude-desktop", "codex"].includes(app),
      ),
    ];
    expect(mocks.updateSettings).toHaveBeenCalledWith({ appOrder: next });
    expect(mocks.autoSaveSettings).toHaveBeenCalledWith({ appOrder: next });
  });

  it("keeps the header steady while checking for updates", async () => {
    await renderApps();
    expect(updateAllButton()).toBeEnabled();
    const pending = deferred<void>();
    const getVersions = mocks.getToolVersions.getMockImplementation()!;
    mocks.getToolVersions.mockImplementation(async (tools: string[]) => {
      await pending.promise;
      return getVersions(tools);
    });
    fireEvent.click(
      screen.getByRole("button", { name: "appsPage.checkUpdates" }),
    );
    // 检查期间各工具都在加载：「全部升级」保持显示（禁用），不先消失再出现
    expect(
      await screen.findByRole("button", { name: "appsPage.checking" }),
    ).toBeDisabled();
    expect(updateAllButton()).toBeDisabled();
    await act(async () => pending.resolve());
    expect(updateAllButton()).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "appsPage.checkUpdates" }),
    ).toBeEnabled();
  });

  it("withholds the upgrade when the new version needs a newer Node and leaves it out of update all", async () => {
    const defaultVersions = mocks.getToolVersions.getMockImplementation()!;
    mocks.getToolVersions.mockImplementation(async (tools: string[]) =>
      (await defaultVersions(tools)).map(
        (tool: { name: string; version: string | null }) => ({
          ...tool,
          update_blocked_by_node:
            tool.name === "claude" && tool.version === "1.0.0"
              ? { required: ">=24.16.0 <25 || >=26.1.0", current: "22.23.2" }
              : null,
        }),
      ),
    );
    await renderApps();
    await waitFor(() =>
      expect(
        card("Claude Code").getByText("appsPage.updateNeedsNode"),
      ).toBeInTheDocument(),
    );
    expect(
      card("Claude Code").queryByRole("button", {
        name: "settings.toolUpdate",
      }),
    ).toBeNull();
    fireEvent.click(updateAllButton());
    await waitFor(() => expect(isReady("Codex")).toBe(true));
    expect(isReady("Gemini CLI")).toBe(true);
    expect(
      mocks.runToolLifecycleAction.mock.calls.map(([tools]) => tools),
    ).toEqual([["codex"], ["gemini"]]);
  });

  it("shows diagnosed conflicts on the row, refreshes the install report and says so", async () => {
    vi.resetModules();
    const install = (path: string, isDefault: boolean) => ({
      path,
      version: "2.0.0",
      runnable: true,
      error: null,
      source: "npm",
      is_path_default: isDefault,
    });
    // 打开页面时只有一处安装；之后外部又装了一份，诊断才发现
    mocks.probeToolInstallations.mockImplementation(async (tools: string[]) =>
      tools.map((tool) =>
        tool === "claude"
          ? report(tool, {
              is_conflict: true,
              installs: [
                install("/usr/local/bin/claude", true),
                install("/opt/homebrew/bin/claude", false),
              ],
            })
          : report(tool),
      ),
    );
    await renderApps();
    expect(screen.queryByText("/opt/homebrew/bin/claude")).toBeNull();

    await userEvent.click(
      screen.getByRole("button", { name: "appsPage.moreActions" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "settings.toolDiagnose" }),
    );

    // 冲突列表直接在行里展开，入口「另有 1 处安装」也刷新出来，并有一条提示
    expect(
      await card("Claude Code").findByText("/opt/homebrew/bin/claude"),
    ).toBeInTheDocument();
    expect(
      card("Claude Code").getByRole("button", {
        name: /appsPage\.otherInstalls/,
      }),
    ).toHaveAttribute("aria-expanded", "true");
    expect(mocks.warning).toHaveBeenCalledWith(
      "settings.toolDiagnoseConflicts",
      expect.anything(),
    );
    expect(mocks.info).not.toHaveBeenCalled();
  });
});

describe("AppsPage visibility column", () => {
  it("keeps a missing WSL tool hidden even when the host has an installation", async () => {
    missing.add("codex");
    const defaultVersions = mocks.getToolVersions.getMockImplementation()!;
    mocks.getToolVersions.mockImplementation(async (tools: string[]) =>
      (await defaultVersions(tools)).map((tool: ToolVersions[number]) =>
        tool.name === "codex"
          ? { ...tool, env_type: "wsl", wsl_distro: "Ubuntu" }
          : tool,
      ),
    );
    mocks.listToolInstallations.mockImplementation(async (tools: string[]) =>
      tools.map((tool) =>
        report(tool, {
          installs:
            tool === "codex"
              ? [
                  {
                    path: "C:/tools/codex.exe",
                    source: "npm",
                    version: "1.0.0",
                    runnable: true,
                    error: null,
                    is_path_default: true,
                  },
                ]
              : [],
        }),
      ),
    );
    await renderApps();
    await screen.findByRole("region", { name: "appsPage.sectionNotInstalled" });
    expect(rowApps("appsPage.sectionNotInstalled")).toContain("codex");
    expect(card("Codex").queryByRole("switch")).toBeNull();
    expect(card("Codex").getByText("C:/tools/codex.exe")).toBeInTheDocument();
    expect(
      card("Codex").getByRole("button", { name: "appPage.moreActions" }),
    ).toBeEnabled();
  });

  it("retains an installed WSL tool after probe failure without offering an install or upgrade", async () => {
    let probeFailed = false;
    const defaultVersions = mocks.getToolVersions.getMockImplementation()!;
    mocks.getToolVersions.mockImplementation(async (tools: string[]) =>
      (await defaultVersions(tools)).map((tool: ToolVersions[number]) =>
        tool.name === "codex"
          ? {
              ...tool,
              env_type: "wsl",
              wsl_distro: "Ubuntu",
              version: probeFailed ? null : tool.version,
              error: probeFailed ? "WSL probe failed" : null,
            }
          : tool,
      ),
    );
    await renderApps();
    await screen.findByRole("region", { name: "appsPage.sectionInstalled" });
    expect(card("Codex").getByRole("switch")).toBeChecked();

    probeFailed = true;
    fireEvent.click(
      screen.getByRole("button", { name: "appsPage.checkUpdates" }),
    );
    await waitFor(() =>
      expect(card("Codex").getByText("WSL probe failed")).toBeInTheDocument(),
    );
    expect(card("Codex").getByRole("switch")).toBeChecked();
    expect(card("Codex").getByText("common.unknown")).toBeInTheDocument();
    expect(
      card("Codex").queryByRole("button", {
        name: /settings\.tool(Install|Update)/,
      }),
    ).toBeNull();

    probeFailed = false;
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "appsPage.checkUpdates" }),
      ).toBeEnabled(),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "appsPage.checkUpdates" }),
    );
    await waitFor(() => expect(updateButton("Codex")).toBeEnabled());
    expect(card("Codex").queryByText("WSL probe failed")).toBeNull();
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "appsPage.checkUpdates" }),
      ).toBeEnabled(),
    );

    mocks.getToolVersions.mockImplementation(async (tools: string[]) => {
      if (tools.includes("codex")) throw new Error("WSL request failed");
      return defaultVersions(tools);
    });
    fireEvent.click(
      screen.getByRole("button", { name: "appsPage.checkUpdates" }),
    );
    await waitFor(() =>
      expect(card("Codex").getByText("WSL request failed")).toBeInTheDocument(),
    );
    expect(card("Codex").getByText("1.0.0")).toBeInTheDocument();
    expect(card("Codex").getByRole("switch")).toBeChecked();
    expect(
      card("Codex").queryByRole("button", {
        name: /settings\.tool(Install|Update)/,
      }),
    ).toBeNull();
  });

  it("shows the switch after installation and preserves its saved preference", async () => {
    missing.add("codex");
    Object.assign(mocks.settings, {
      visibleApps: { codex: false },
    });
    await renderApps();
    await screen.findByRole("region", { name: "appsPage.sectionNotInstalled" });
    expect(card("Codex").queryByRole("switch")).toBeNull();
    expect(
      card("Codex").getByRole("button", {
        name: "profiles.environment.configureProject",
      }),
    ).toBeEnabled();

    fireEvent.click(
      card("Codex").getByRole("button", { name: "settings.toolInstall" }),
    );
    await waitFor(() =>
      expect(card("Codex").getByRole("switch")).not.toBeChecked(),
    );
    expect(mocks.runToolLifecycleAction).toHaveBeenCalledWith(
      ["codex"],
      "install",
      expect.anything(),
    );
    expect(mocks.autoSaveSettings).not.toHaveBeenCalled();
  });

  it("allows the last visible agent to be hidden", async () => {
    const visibleApps = Object.fromEntries(
      APP_IDS.map((app) => [app, app === "claude"]),
    );
    Object.assign(mocks.settings, { visibleApps });
    await renderApps();
    await screen.findByRole("region", { name: "appsPage.sectionInstalled" });

    const toggle = card("Claude Code").getByRole("switch");
    expect(toggle).toBeChecked();
    expect(toggle).toBeEnabled();
    fireEvent.click(toggle);
    expect(mocks.autoSaveSettings).toHaveBeenCalledWith({
      visibleApps: { ...visibleApps, claude: false },
    });
  });

  it("refreshes desktop installation switches along with tool checks", async () => {
    mocks.desktopStatus.installed = false;
    mocks.getConfigStatus.mockResolvedValue({
      exists: false,
      path: "/home/me/.workbuddy",
    });
    await renderApps();
    await waitFor(() =>
      expect(rowApps("appsPage.sectionNotInstalled")).toEqual([
        "claude-desktop",
        "workbuddy",
      ]),
    );
    expect(card("Claude Desktop").queryByRole("switch")).toBeNull();
    expect(card("WorkBuddy").queryByRole("switch")).toBeNull();

    mocks.desktopStatus = {
      supported: true,
      installed: true,
      configured: false,
    };
    mocks.getConfigStatus.mockResolvedValue({
      exists: true,
      path: "/home/me/.workbuddy",
    });
    fireEvent.click(
      screen.getByRole("button", { name: "appsPage.checkUpdates" }),
    );
    await waitFor(() => {
      expect(card("Claude Desktop").getByRole("switch")).toBeChecked();
      expect(card("WorkBuddy").getByRole("switch")).toBeChecked();
    });
  });

  it("renders the sidebar visibility switches small with the action color", async () => {
    vi.resetModules();
    await renderApps();
    const switches = screen.getAllByRole("switch", {
      name: /appsPage\.showInSidebar/,
    });
    expect(switches.length).toBeGreaterThan(0);
    for (const element of switches) {
      expect(element).toHaveAttribute("data-size", "sm");
      expect(element).not.toHaveAttribute("data-tone");
    }
  });

  it("gives WorkBuddy only the sidebar switch, like Claude Desktop", async () => {
    vi.resetModules();
    await renderApps();
    const row = screen
      .getByText("WorkBuddy", { selector: "span" })
      .closest("[data-tool-row]") as HTMLElement;
    expect(row).not.toBeNull();
    expect(within(row).getByRole("switch")).toBeInTheDocument();
    // 和 Claude Desktop 一样：接入状态 + 说明，没有操作按钮
    expect(
      await within(row).findByText("appsPage.desktopConnected"),
    ).toBeInTheDocument();
    expect(mocks.getConfigStatus).toHaveBeenCalledWith("workbuddy");
    // 除了「已安装」组的拖动把手，唯一的按钮是说明的问号
    expect(
      within(row)
        .getAllByRole("button")
        .filter(
          (button) =>
            button.getAttribute("aria-label") !== "appsPage.dragHandle",
        ),
    ).toEqual([within(row).getByRole("button", { name: "WorkBuddy" })]);
  });
});
