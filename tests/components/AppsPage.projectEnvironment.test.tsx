import {
  act,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderWithQueryClient } from "../utils/testQueryClient";
import type { Profile } from "@/lib/api/profiles";
import type { ToolInstallationReport } from "@/lib/api/settings";
import { deferred, registerScrollIntoViewStub } from "./appsPageTestHelpers";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  versions: vi.fn(),
  installations: vi.fn(),
  run: vi.fn(),
  saveSettings: vi.fn(),
  updateSettings: vi.fn(),
  toast: {
    success: vi.fn(),
    info: vi.fn(),
    warning: vi.fn(),
    error: vi.fn(),
  },
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@/lib/api", async () => ({
  profilesApi: (await import("@/lib/api/profiles")).profilesApi,
  providersApi: { updateTrayMenu: vi.fn(), getAll: vi.fn(async () => ({})) },
  promptsApi: { getPrompts: vi.fn(async () => ({})) },
  settingsApi: {
    getToolVersions: mocks.versions,
    listToolInstallations: mocks.installations,
    probeToolInstallations: mocks.installations,
    runToolLifecycleAction: mocks.run,
    getConfigStatus: vi.fn(async () => ({ exists: false })),
  },
}));
vi.mock("@/lib/api/providers", () => ({
  providersApi: {
    getClaudeDesktopStatus: vi.fn(async () => ({ installed: false })),
  },
}));
vi.mock("@/hooks/useSettings", () => ({
  useSettings: () => ({
    settings: {
      visibleApps: { claude: false, codex: false },
      showProfileSwitcher: false,
    },
    updateSettings: mocks.updateSettings,
    autoSaveSettings: mocks.saveSettings,
  }),
}));
vi.mock("@/lib/toast", () => ({ toast: mocks.toast }));

registerScrollIntoViewStub();
let installed: boolean;
let broken: boolean;
let profile: Profile;

beforeEach(() => {
  localStorage.removeItem("cc-switch:profiles:location:v2");
  vi.resetModules();
  vi.clearAllMocks();
  installed = true;
  broken = false;
  profile = {
    id: "project-profile",
    name: "Project profile",
    target: "project",
    payload: {
      providers: { claude: null, codex: null, "claude-desktop": null },
      mcp: { claude: [], codex: [], "claude-desktop": null },
      skills: { claude: [], codex: [], "claude-desktop": null },
      prompts: { claude: null, codex: null, "claude-desktop": null },
    },
  };
  mocks.versions.mockImplementation(async (tools: string[]) =>
    tools.map((name) => ({
      name,
      version: installed && !broken ? "2.0.0" : null,
      latest_version: "2.0.0",
      error: broken ? "Executable is broken" : null,
      installed_but_broken: broken,
      env_type: "windows",
      wsl_distro: null,
      update_blocked_by_node: null,
    })),
  );
  mocks.installations.mockImplementation(
    async (tools: string[]): Promise<ToolInstallationReport[]> =>
      tools.map((tool) => ({
        tool,
        installs: installed
          ? [
              {
                path: `C:/tools/${tool}.exe`,
                version: broken ? null : "2.0.0",
                runnable: !broken,
                error: broken ? "Executable is broken" : null,
                source: "native",
                is_path_default: true,
              },
            ]
          : [],
        command: `${tool} update`,
        is_conflict: false,
        needs_confirmation: false,
        anchored: true,
        unmanaged: false,
      })),
  );
  mocks.run.mockImplementation(async () => {
    installed = true;
  });
  mocks.invoke.mockImplementation(async (command: string) => {
    if (command === "list_profiles")
      return {
        profiles: [profile],
        current: { claude: null, codex: null, claudeDesktop: null },
        globalState: { claude: null, codex: null, claudeDesktop: null },
        projects: [
          { path: "/projects/other", available: true, deployed: {} },
          { path: "/projects/demo", available: true, deployed: {} },
        ],
      };
    if (command === "deploy_profile_to_project")
      return { status: "applied", changed: [], warnings: [] };
    if (command === "get_mcp_servers") return {};
    if (command === "get_installed_skills") return [];
    throw new Error(`Unexpected command: ${command}`);
  });
});

async function openEnvironment(tool: "claude" | "codex" = "claude") {
  const { AppsPage } = await import("@/components/apps/AppsPage");
  await import("@/components/apps/ProjectEnvironmentPanel");
  const view = renderWithQueryClient(<AppsPage />);
  await waitFor(() =>
    expect(
      within(view.container).getByText("appsPage.checkUpdates"),
    ).toHaveAttribute("aria-hidden", "false"),
  );
  const toolRow = () => {
    const row = view.container.querySelector(
      '[data-tool-row="' + tool + '"]',
    ) as HTMLElement;
    expect(row).not.toBeNull();
    return within(row);
  };
  const showProject = async () => {
    fireEvent.click(
      toolRow().getByRole("button", {
        name: "profiles.environment.configureProject",
      }),
    );
    const region = await screen.findByRole("region", {
      name: "profiles.environment.title",
    });
    return within(region);
  };
  const panel = await showProject();
  fireEvent.click(
    await panel.findByRole("combobox", { name: "profiles.projects.select" }),
  );
  fireEvent.click(
    await screen.findByRole("option", { name: "/projects/demo" }),
  );
  await waitFor(() =>
    expect(
      panel.getByRole("combobox", { name: "profiles.projects.select" }),
    ).toHaveAttribute("data-project-path", "/projects/demo"),
  );
  await panel.findByText("Project profile");
  const installFromManagement = async () => {
    fireEvent.click(screen.getByRole("button", { name: "nav.apps" }));
    await waitFor(() =>
      expect(
        screen.queryByRole("region", { name: "profiles.environment.title" }),
      ).toBeNull(),
    );
    fireEvent.click(
      toolRow().getByRole("button", { name: "settings.toolInstall" }),
    );
    return showProject();
  };
  return { view, panel, installFromManagement };
}

describe("Agent management project environment", () => {
  it("uses an existing hidden agent and the requested project without installing or changing display settings", async () => {
    const { panel } = await openEnvironment();
    expect(panel.getByText("profiles.environment.ready")).toBeInTheDocument();
    expect(panel.queryByText("C:/tools/claude.exe")).not.toBeInTheDocument();
    expect(
      panel.queryByRole("button", { name: "settings.toolInstall" }),
    ).toBeNull();
    expect(
      panel.getByRole("combobox", { name: "profiles.projects.select" }),
    ).toHaveAttribute("data-project-path", "/projects/demo");
    // Agent management only deploys to projects; it never applies globally.
    expect(panel.queryByRole("button", { name: "profiles.apply" })).toBeNull();
    expect(
      panel.queryByRole("group", { name: "profiles.scope.label" }),
    ).toBeNull();
    fireEvent.click(
      panel.getByRole("button", { name: "profiles.projectDeploy.button" }),
    );
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("deploy_profile_to_project", {
        id: profile.id,
        scope: "claude",
        projectPath: "/projects/demo",
        resolution: null,
        allowMissingConnection: false,
      }),
    );
    expect(mocks.run).not.toHaveBeenCalled();
    expect(mocks.saveSettings).not.toHaveBeenCalled();
    expect(mocks.updateSettings).not.toHaveBeenCalled();
  });

  it("keeps the selected Profile through installation and enables deployment only after rechecking", async () => {
    installed = false;
    const install = deferred<void>();
    mocks.run.mockImplementation(async () => {
      await install.promise;
      installed = true;
    });
    const environment = await openEnvironment("codex");
    let panel = environment.panel;
    let deploy = panel.getByRole("button", {
      name: "profiles.projectDeploy.button",
    });
    expect(deploy).toBeDisabled();
    expect(panel.getByRole("button", { name: "common.edit" })).toBeEnabled();
    panel = await environment.installFromManagement();
    deploy = panel.getByRole("button", {
      name: "profiles.projectDeploy.button",
    });
    expect(deploy).toBeDisabled();
    expect(panel.getByText("Project profile")).toBeInTheDocument();
    await act(async () => install.resolve());
    await waitFor(() => expect(deploy).toBeEnabled());
    expect(mocks.run).toHaveBeenCalledTimes(1);
    expect(mocks.run).toHaveBeenCalledWith(["codex"], "install", {});
    expect(panel.queryByText("C:/tools/codex.exe")).not.toBeInTheDocument();
    expect(mocks.installations).toHaveBeenCalledWith(["codex"]);
    expect(
      mocks.invoke.mock.calls.some(
        ([name]) => name === "deploy_profile_to_project",
      ),
    ).toBe(false);
  });

  it("keeps deployment blocked after a failed install and allows a successful recheck to recover", async () => {
    installed = false;
    mocks.run.mockRejectedValueOnce(new Error("Installer failed"));
    const environment = await openEnvironment();
    const panel = await environment.installFromManagement();
    await panel.findByText("Installer failed");
    expect(
      panel.getByRole("button", { name: "profiles.projectDeploy.button" }),
    ).toBeDisabled();
    installed = true;
    fireEvent.click(
      panel.getByRole("button", { name: "profiles.environment.recheck" }),
    );
    await waitFor(() =>
      expect(
        panel.getByRole("button", { name: "profiles.projectDeploy.button" }),
      ).toBeEnabled(),
    );
    expect(panel.queryByText("Installer failed")).not.toBeInTheDocument();
  });

  it("does not treat a successful installer command with a broken executable as ready", async () => {
    installed = false;
    mocks.run.mockImplementation(async () => {
      installed = true;
      broken = true;
    });
    const environment = await openEnvironment("codex");
    const panel = await environment.installFromManagement();
    await waitFor(() =>
      expect(panel.getAllByText("Executable is broken").length).toBeGreaterThan(
        0,
      ),
    );
    expect(
      panel.getByRole("button", { name: "profiles.projectDeploy.button" }),
    ).toBeDisabled();
    expect(panel.getByRole("button", { name: "common.edit" })).toBeEnabled();
  });

  it("opens the project flow from a tool row even with the Profile header hidden", async () => {
    const { AppsPage } = await import("@/components/apps/AppsPage");
    await import("@/components/apps/ProjectEnvironmentPanel");
    const view = renderWithQueryClient(<AppsPage />);
    await waitFor(() =>
      expect(
        within(view.container).getByText("appsPage.checkUpdates"),
      ).toHaveAttribute("aria-hidden", "false"),
    );
    const codexRow = (
      await screen.findByText("Codex", { selector: "span" })
    ).closest("[data-tool-row]")! as HTMLElement;
    fireEvent.click(
      within(codexRow).getByRole("button", {
        name: "profiles.environment.configureProject",
      }),
    );
    const panel = within(
      await screen.findByRole("region", { name: "profiles.environment.title" }),
    );
    fireEvent.click(
      await panel.findByRole("combobox", { name: "profiles.projects.select" }),
    );
    fireEvent.click(
      await screen.findByRole("option", { name: "/projects/demo" }),
    );
    expect(mocks.run).not.toHaveBeenCalled();
    expect(mocks.saveSettings).not.toHaveBeenCalled();
  });

  it("blocks deployment when a fresh native installation report contradicts the version result", async () => {
    const installations = mocks.installations.getMockImplementation()!;
    mocks.installations.mockImplementation(async (tools: string[]) => {
      const reports = (await installations(tools)) as ToolInstallationReport[];
      return reports.map((report) => ({
        ...report,
        installs: report.installs.map((install) => ({
          ...install,
          runnable: false,
          error: "Default executable failed",
        })),
      }));
    });
    const { panel } = await openEnvironment();
    expect(
      panel.getByRole("button", { name: "profiles.projectDeploy.button" }),
    ).toBeDisabled();
    expect(
      panel.queryByText("profiles.environment.ready"),
    ).not.toBeInTheDocument();
    expect(panel.getByText("Default executable failed")).toBeInTheDocument();
  });
});
