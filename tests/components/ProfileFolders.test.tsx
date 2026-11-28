import {
  act,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ProfilesPage } from "@/components/profiles/ProfilesPage";
import { ProfileEditPanel } from "@/components/profiles/ProfileEditPanel";
import type {
  Profile,
  ProfileConfig,
  ProfilesResponse,
  ProfileScope,
} from "@/lib/api/profiles";
import { renderWithQueryClient } from "../utils/testQueryClient";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  create: vi.fn(),
  importProject: vi.fn(),
  duplicate: vi.fn(),
  update: vi.fn(),
  saveConfig: vi.fn(),
  apply: vi.fn(),
  addProject: vi.fn(),
  removeProject: vi.fn(),
  deploy: vi.fn(),
  pickDirectory: vi.fn(),
  updateTrayMenu: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
  info: vi.fn(),
  warning: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@/components/JsonEditor", () => ({
  default: ({
    value,
    onChange,
    ariaLabel,
  }: {
    value: string;
    onChange: (value: string) => void;
    ariaLabel?: string;
  }) => (
    <textarea
      aria-label={ariaLabel}
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));
vi.mock("@/lib/api", async () => ({
  profilesApi: (await import("@/lib/api/profiles")).profilesApi,
  settingsApi: { pickDirectory: mocks.pickDirectory },
  providersApi: {
    updateTrayMenu: mocks.updateTrayMenu,
    getAll: (app: string) => mocks.invoke("get_providers", { app }),
  },
  promptsApi: {
    getPrompts: (app: string) => mocks.invoke("get_prompts", { app }),
  },
}));
vi.mock("@/lib/toast", () => ({
  toast: {
    success: mocks.success,
    error: mocks.error,
    info: mocks.info,
    warning: mocks.warning,
  },
}));

const profile = (
  capturedScope: ProfileScope = "claude",
  target: Profile["target"] = "global",
): Profile => ({
  id: "demo",
  name: "Demo",
  target,
  payload: {
    providers: { claude: null, "claude-desktop": null, codex: null },
    mcp: {
      claude: null,
      "claude-desktop": null,
      codex: null,
      [capturedScope]: [],
    },
    skills: {
      claude: null,
      "claude-desktop": null,
      codex: null,
      [capturedScope]: [],
    },
    prompts: { claude: null, "claude-desktop": null, codex: null },
  },
});

let profiles: Profile[];
const providerLibrary = {
  "provider-a": {
    id: "provider-a",
    name: "Provider A",
    settingsConfig: {
      env: { ANTHROPIC_MODEL: "claude-default" },
      config: 'model = "codex-default"',
    },
    meta: { stackModels: [{ model: "model-candidate" }] },
  },
  "provider-b": {
    id: "provider-b",
    name: "Provider B",
    settingsConfig: {
      env: { ANTHROPIC_MODEL: "claude-other" },
      config: 'model = "codex-other"',
    },
  },
};
let current: ProfilesResponse["current"];
let projects: ProfilesResponse["projects"];

beforeEach(() => {
  localStorage.removeItem("cc-switch:profiles:location:v2");
  Object.values(mocks).forEach((mock) => mock.mockReset());
  profiles = [];
  projects = [];
  current = { claude: null, claudeDesktop: null, codex: null };
  mocks.updateTrayMenu.mockResolvedValue(true);
  mocks.pickDirectory.mockResolvedValue("/projects/demo");
  mocks.create.mockImplementation(({ scope, target }) =>
    Promise.resolve(profile(scope, target)),
  );
  mocks.apply.mockResolvedValue([]);
  mocks.duplicate.mockResolvedValue(profile());
  mocks.update.mockImplementation(({ id }: { id: string }) =>
    Promise.resolve(profiles.find((item) => item.id === id)),
  );
  mocks.saveConfig.mockImplementation(
    ({
      id,
      scope,
      config,
      name,
    }: {
      id: string;
      scope: ProfileScope;
      config: ProfileConfig;
      name?: string;
    }) => {
      profiles = profiles.map((item) =>
        item.id !== id
          ? item
          : {
              ...item,
              name: name ?? item.name,
              payload: {
                ...item.payload,
                ...(scope === "claude" && item.target === "project"
                  ? { claudeProjectConnection: config.claudeProjectConnection }
                  : {}),
                providers: {
                  ...item.payload.providers,
                  [scope]: config.providerId,
                },
                models: {
                  ...(item.payload.models ?? {
                    claude: null,
                    "claude-desktop": null,
                    codex: null,
                  }),
                  [scope]: config.model,
                },
                modelSettings: {
                  ...(item.payload.modelSettings ?? {
                    claude: null,
                    "claude-desktop": null,
                    codex: null,
                  }),
                  [scope]: config.modelSettings,
                },
                modelCatalog: {
                  ...(item.payload.modelCatalog ?? {
                    claude: null,
                    "claude-desktop": null,
                    codex: null,
                  }),
                  [scope]: config.modelCatalog,
                },
                mcp: {
                  ...item.payload.mcp,
                  [scope]: config.followGlobalResources ? null : config.mcpIds,
                },
                skills: {
                  ...item.payload.skills,
                  [scope]: config.followGlobalResources
                    ? null
                    : config.skillIds,
                },
                prompts: {
                  ...item.payload.prompts,
                  [scope]: config.followGlobalResources
                    ? null
                    : (config.promptId ?? ""),
                },
              },
            },
      );
      return Promise.resolve(profiles.find((item) => item.id === id));
    },
  );
  mocks.deploy.mockResolvedValue({
    status: "applied",
    changed: [],
    warnings: [],
  });
  mocks.addProject.mockImplementation(({ path }) => {
    projects = [...projects, { path, available: true, deployed: {} }];
  });
  mocks.removeProject.mockImplementation(({ path }) => {
    projects = projects.filter((project) => project.path !== path);
  });
  mocks.invoke.mockImplementation((command: string, args: unknown) => {
    switch (command) {
      case "list_profiles":
        return Promise.resolve({
          profiles,
          current,
          projects,
          globalState: {
            claude: { providerId: "provider-a", model: "live-model" },
            codex: null,
            claudeDesktop: null,
          },
        });
      case "create_profile":
        return mocks.create(args);
      case "import_project_profile":
        return mocks.importProject(args);
      case "duplicate_profile":
        return mocks.duplicate(args);
      case "update_profile":
        return mocks.update(args);
      case "update_profile_config":
        return mocks.saveConfig(args);
      case "get_providers":
        return Promise.resolve(providerLibrary);
      case "get_mcp_servers":
        return Promise.resolve({
          "mcp-a": {
            id: "mcp-a",
            name: "MCP A",
            server: { command: "echo" },
            apps: {},
          },
          "mcp-b": {
            id: "mcp-b",
            name: "MCP B",
            server: { command: "echo" },
            apps: {},
          },
        });
      case "get_installed_skills":
        return Promise.resolve([
          {
            id: "skill-a",
            name: "Skill A",
            directory: "skill-a",
            apps: {},
            installedAt: 1,
            updatedAt: 0,
          },
          {
            id: "skill-b",
            name: "Skill B",
            directory: "skill-b",
            apps: {},
            installedAt: 1,
            updatedAt: 0,
          },
        ]);
      case "get_prompts":
        return Promise.resolve({
          "prompt-a": {
            id: "prompt-a",
            name: "Instructions A",
            content: "Project instruction",
            enabled: true,
          },
        });
      case "apply_profile":
        return mocks.apply(args);
      case "add_profile_project":
        return mocks.addProject(args);
      case "remove_profile_project":
        return mocks.removeProject(args);
      case "deploy_profile_to_project":
        return mocks.deploy(args);
      default:
        throw new Error(`Unexpected command: ${command}`);
    }
  });
  Element.prototype.scrollIntoView = vi.fn();
});

describe("ProfilesPage", () => {
  it.each(["claude", "codex"] as const)(
    "keeps a new user's %s profiles empty without creating or migrating on open",
    async (activeApp) => {
      renderWithQueryClient(<ProfilesPage activeApp={activeApp} />);
      expect(await screen.findByText("profiles.empty")).toBeVisible();
      expect(screen.queryByRole("listitem")).toBeNull();
      expect(profiles).toEqual([]);
      expect(mocks.invoke).toHaveBeenCalledWith("list_profiles");
      expect(
        mocks.invoke.mock.calls.some(
          ([command]) => command === "migrate_legacy_agent_profiles",
        ),
      ).toBe(false);
      expect(mocks.create).not.toHaveBeenCalled();
      expect(mocks.importProject).not.toHaveBeenCalled();
    },
  );

  it("separates profiles by Agent and target while reusing project profiles across directories", async () => {
    profiles = [
      profile(),
      { ...profile("claude", "project"), id: "project", name: "Project" },
      { ...profile("codex"), id: "other", name: "Codex only" },
      {
        ...profile("codex", "project"),
        id: "other-project",
        name: "Codex project",
      },
    ];
    current.claude = { id: "demo", stale: true };
    projects = [
      {
        path: "/first",
        available: true,
        deployed: { claude: { profileId: "project", stale: false } },
      },
      { path: "/second", available: true, deployed: {} },
    ];
    renderWithQueryClient(<ProfilesPage activeApp="claude" />);
    const row = await screen.findByRole("listitem", { name: "Demo" });
    expect(screen.queryByText("Codex only")).toBeNull();
    expect(screen.queryByRole("listitem", { name: "Project" })).toBeNull();
    expect(screen.queryByText("Codex project")).toBeNull();
    expect(within(row).getByText("profiles.pendingApply")).toBeVisible();
    expect(
      screen.getByRole("status", { name: "profiles.inEffect.global" }),
    ).toHaveTextContent("live-model");
    fireEvent.click(
      within(row).getByRole("button", { name: "profiles.apply" }),
    );
    await waitFor(() =>
      expect(mocks.apply).toHaveBeenCalledWith({ id: "demo", scope: "claude" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.scope.project" }),
    );
    const projectRow = await screen.findByRole("listitem", { name: "Project" });
    expect(screen.queryByRole("listitem", { name: "Demo" })).toBeNull();
    expect(screen.queryByText("Codex project")).toBeNull();
    expect(
      within(projectRow).getByText("profiles.projectDeploy.current"),
    ).toBeVisible();
    await userEvent.click(
      screen.getByRole("combobox", { name: "profiles.projects.select" }),
    );
    await userEvent.click(
      await screen.findByRole("option", { name: "/second" }),
    );
    expect(mocks.deploy).not.toHaveBeenCalled();
    fireEvent.click(
      within(projectRow).getByRole("button", {
        name: "profiles.projectDeploy.button",
      }),
    );
    await waitFor(() =>
      expect(mocks.deploy).toHaveBeenCalledWith({
        id: "project",
        scope: "claude",
        projectPath: "/second",
        resolution: null,
      }),
    );
    expect(mocks.apply).toHaveBeenCalledTimes(1);
  });

  it("registers and removes a directory independently of the profiles", async () => {
    profiles = [profile("claude", "project")];
    renderWithQueryClient(<ProfilesPage activeApp="claude" projectOnly />);
    expect(await screen.findByText("profiles.projects.empty")).toBeVisible();
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.projects.add" }),
    );
    expect(
      await screen.findByRole("combobox", { name: "profiles.projects.select" }),
    ).toHaveAttribute("title", "/projects/demo");
    expect(mocks.addProject).toHaveBeenCalledWith({ path: "/projects/demo" });
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.projects.remove" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "common.confirm" }),
    );
    await screen.findByText("profiles.projects.empty");
    expect(mocks.removeProject).toHaveBeenCalledWith({
      path: "/projects/demo",
    });
    expect(screen.getByRole("listitem", { name: "Demo" })).toBeVisible();
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("duplicates a profile independently of an unavailable deployment location", async () => {
    profiles = [profile("claude", "project")];
    projects = [{ path: "/offline", available: false, deployed: {} }];
    renderWithQueryClient(<ProfilesPage activeApp="claude" projectOnly />);
    const row = within(await screen.findByRole("listitem", { name: "Demo" }));
    expect(
      row.getByRole("button", { name: "profiles.projectDeploy.button" }),
    ).toBeDisabled();
    await userEvent.click(
      row.getByRole("button", { name: "profiles.context.rowActions" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "profiles.duplicate" }),
    );
    const dialog = within(await screen.findByRole("dialog"));
    fireEvent.change(dialog.getByLabelText("profiles.editor.name"), {
      target: { value: "Second" },
    });
    fireEvent.click(dialog.getByRole("button", { name: "common.confirm" }));
    await waitFor(() =>
      expect(mocks.duplicate).toHaveBeenCalledWith({
        sourceId: "demo",
        name: "Second",
        scope: "claude",
        target: "project",
      }),
    );
    expect(mocks.addProject).not.toHaveBeenCalled();
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it.each(["global", "project"] as const)(
    "copies a %s profile to the other target and edits only the independent copy",
    async (sourceTarget) => {
      const destination = sourceTarget === "global" ? "project" : "global";
      const original = profile("claude", sourceTarget);
      original.payload.models = {
        claude: "original-model",
        codex: null,
        "claude-desktop": null,
      };
      profiles = [original];
      current.claude =
        sourceTarget === "global" ? { id: original.id, stale: false } : null;
      projects = [{ path: "/demo", available: true, deployed: {} }];
      const originalSnapshot = structuredClone(original);
      mocks.duplicate.mockImplementation(({ name, target }) => {
        const copied = {
          ...structuredClone(original),
          id: "copy",
          name,
          target,
        };
        profiles = [...profiles, copied];
        return Promise.resolve(copied);
      });
      renderWithQueryClient(<ProfilesPage activeApp="claude" />);
      if (sourceTarget === "project")
        fireEvent.click(
          screen.getByRole("button", { name: "profiles.scope.project" }),
        );
      const row = within(await screen.findByRole("listitem", { name: "Demo" }));
      await userEvent.click(
        row.getByRole("button", { name: "profiles.context.rowActions" }),
      );
      await userEvent.click(
        await screen.findByRole("menuitem", {
          name:
            destination === "project"
              ? "profiles.copyToProject"
              : "profiles.copyToGlobal",
        }),
      );
      const dialog = within(await screen.findByRole("dialog"));
      fireEvent.change(dialog.getByLabelText("profiles.editor.name"), {
        target: { value: "Independent copy" },
      });
      fireEvent.click(dialog.getByRole("button", { name: "common.confirm" }));
      const model = await screen.findByLabelText("profiles.editor.model", {
        selector: "input",
      });
      expect(mocks.duplicate).toHaveBeenCalledWith({
        sourceId: original.id,
        name: "Independent copy",
        scope: "claude",
        target: destination,
      });
      expect(model).toHaveValue("original-model");
      fireEvent.change(model, { target: { value: "copy-model" } });
      fireEvent.click(screen.getByRole("button", { name: "common.save" }));
      await waitFor(() =>
        expect(screen.queryByText("profiles.editor.title")).toBeNull(),
      );
      expect(profiles.find((item) => item.id === original.id)).toEqual(
        originalSnapshot,
      );
      expect(profiles.find((item) => item.id === "copy")).toMatchObject({
        target: destination,
        payload: { models: { claude: "copy-model" } },
      });
      expect(
        screen.getByRole("listitem", { name: "Independent copy" }),
      ).toBeVisible();
      expect(screen.queryByRole("listitem", { name: "Demo" })).toBeNull();
      expect(mocks.apply).not.toHaveBeenCalled();
      expect(mocks.deploy).not.toHaveBeenCalled();
      expect(mocks.update).not.toHaveBeenCalled();
      expect(projects[0].deployed).toEqual({});
    },
  );

  it("does not offer a global resnapshot or global copy in the project-only flow", async () => {
    profiles = [profile("claude", "project")];
    renderWithQueryClient(<ProfilesPage activeApp="claude" projectOnly />);
    await userEvent.click(
      await screen.findByRole("button", {
        name: "profiles.context.rowActions",
      }),
    );
    expect(
      screen.queryByRole("menuitem", { name: "profiles.captureCurrent" }),
    ).toBeNull();
    expect(
      screen.queryByRole("menuitem", { name: "profiles.copyToGlobal" }),
    ).toBeNull();
    expect(
      screen.getByRole("menuitem", { name: "profiles.duplicate" }),
    ).toBeVisible();
  });

  it("only lists global Desktop profiles and offers no project conversion", async () => {
    profiles = [
      profile("claude-desktop"),
      {
        ...profile("claude-desktop", "project"),
        id: "invalid-project",
        name: "Unsupported project",
      },
    ];
    renderWithQueryClient(<ProfilesPage activeApp="claude-desktop" />);
    await userEvent.click(
      await screen.findByRole("button", {
        name: "profiles.context.rowActions",
      }),
    );
    expect(screen.queryByText("Unsupported project")).toBeNull();
    expect(
      screen.queryByRole("menuitem", { name: "profiles.copyToProject" }),
    ).toBeNull();
    expect(
      screen.getByRole("menuitem", { name: "profiles.captureCurrent" }),
    ).toBeVisible();
  });

  it("preserves the destination of a deployment conflict and checks readiness again", async () => {
    profiles = [profile("claude", "project")];
    projects = [{ path: "/demo", available: true, deployed: {} }];
    mocks.deploy.mockResolvedValueOnce({
      status: "needsResolution",
      conflicts: [{ path: "CLAUDE.md", reason: "edited" }],
      warnings: ["review this file"],
    });
    const { rerender } = renderWithQueryClient(
      <ProfilesPage activeApp="claude" projectOnly />,
    );
    fireEvent.click(
      await screen.findByRole("button", {
        name: "profiles.projectDeploy.button",
      }),
    );
    const overwrite = await screen.findByRole("button", {
      name: "profiles.projectDeploy.overwrite",
    });
    expect(screen.getByText("review this file")).toBeVisible();
    rerender(<ProfilesPage activeApp="codex" projectOnly deploymentDisabled />);
    expect(overwrite).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "profiles.projectDeploy.skip" }),
    ).toBeEnabled();
    rerender(<ProfilesPage activeApp="codex" projectOnly />);
    fireEvent.click(overwrite);
    await waitFor(() =>
      expect(mocks.deploy).toHaveBeenLastCalledWith({
        id: "demo",
        scope: "claude",
        projectPath: "/demo",
        resolution: "overwrite",
      }),
    );
  });

  it.each([
    ["global", "blank"],
    ["global", "current"],
    ["project", "blank"],
    ["project", "current"],
    ["project", "project"],
  ] as const)(
    "creates a %s profile from %s and opens the editor without applying",
    async (target, source) => {
      mocks.importProject.mockResolvedValue({
        profile: profile("claude", "project"),
        warnings: [],
      });
      renderWithQueryClient(
        <ProfilesPage activeApp="claude" projectOnly={target === "project"} />,
      );
      fireEvent.click(
        await screen.findByRole("button", {
          name: "profiles.createFromCurrent",
        }),
      );
      const dialog = within(screen.getByRole("dialog"));
      if (target === "global")
        expect(
          dialog.queryByRole("radio", {
            name: /profiles.createSource.project/,
          }),
        ).toBeNull();
      fireEvent.change(
        dialog.getByRole("textbox", { name: "profiles.editor.name" }),
        { target: { value: " Demo " } },
      );
      fireEvent.click(
        dialog.getByRole("radio", {
          name: new RegExp("profiles.createSource." + source),
        }),
      );
      if (source === "project") {
        expect(
          dialog.getByRole("button", { name: "profiles.createAndEdit" }),
        ).toBeDisabled();
        fireEvent.click(
          dialog.getByRole("button", { name: "profiles.projectPath.pick" }),
        );
        await waitFor(() =>
          expect(
            dialog.getByRole("textbox", { name: "profiles.projectPath.label" }),
          ).toHaveValue("/projects/demo"),
        );
      }
      fireEvent.click(
        dialog.getByRole("button", { name: "profiles.createAndEdit" }),
      );
      await screen.findByLabelText("profiles.editor.model", {
        selector: "input",
      });
      if (source === "project")
        expect(mocks.importProject).toHaveBeenCalledWith({
          name: "Demo",
          scope: "claude",
          projectPath: "/projects/demo",
        });
      else
        expect(mocks.create).toHaveBeenCalledWith({
          name: "Demo",
          scope: "claude",
          source,
          target,
        });
      expect(mocks.apply).not.toHaveBeenCalled();
      expect(mocks.deploy).not.toHaveBeenCalled();
    },
  );

  it("keeps Claude Desktop global and closes an unfinished create dialog directly", async () => {
    renderWithQueryClient(<ProfilesPage activeApp="claude-desktop" />);
    expect(
      screen.queryByRole("button", { name: "profiles.scope.project" }),
    ).toBeNull();
    fireEvent.click(
      await screen.findByRole("button", { name: "profiles.createFromCurrent" }),
    );
    expect(
      screen.queryByRole("radio", { name: /profiles.createSource.project/ }),
    ).toBeNull();
    fireEvent.change(screen.getByPlaceholderText("profiles.namePlaceholder"), {
      target: { value: "draft" },
    });
    fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(mocks.create).not.toHaveBeenCalled();
  });

  it("keeps the original Agent and target while a create dialog is open", async () => {
    const { rerender } = renderWithQueryClient(
      <ProfilesPage activeApp="claude" projectOnly />,
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "profiles.createFromCurrent" }),
    );
    fireEvent.change(screen.getByLabelText("profiles.editor.name"), {
      target: { value: "Prepared project" },
    });
    rerender(<ProfilesPage activeApp="codex" />);
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.createAndEdit" }),
    );
    await screen.findByLabelText("profiles.editor.projectConnection");
    expect(mocks.create).toHaveBeenCalledWith({
      name: "Prepared project",
      scope: "claude",
      source: "blank",
      target: "project",
    });
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(mocks.saveConfig).toHaveBeenCalledWith(
        expect.objectContaining({ scope: "claude" }),
      ),
    );
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("only shows search above five profiles and searches connection and model", async () => {
    profiles = Array.from({ length: 5 }, (_, i) => ({
      ...profile(),
      id: String(i),
      name: "Profile " + i,
    }));
    const first = renderWithQueryClient(<ProfilesPage activeApp="claude" />);
    await screen.findByRole("listitem", { name: "Profile 0" });
    expect(
      screen.queryByPlaceholderText("profiles.searchPlaceholder"),
    ).toBeNull();
    first.unmount();
    const extra = profile();
    extra.payload.providers.claude = "provider-b";
    extra.payload.models = {
      claude: "custom-model",
      codex: null,
      "claude-desktop": null,
    };
    profiles.push(extra);
    renderWithQueryClient(<ProfilesPage activeApp="claude" />);
    const search = await screen.findByPlaceholderText(
      "profiles.searchPlaceholder",
    );
    for (const value of ["Provider B", "custom-model"]) {
      fireEvent.change(search, { target: { value } });
      await waitFor(() =>
        expect(screen.getAllByRole("listitem")).toHaveLength(1),
      );
      expect(screen.getByRole("listitem", { name: "Demo" })).toBeVisible();
    }
  });
});

describe("direct profile configuration editing", () => {
  it("retains a project connection choice after saving and reopening without applying globally", async () => {
    const item = profile("claude", "project");
    profiles = [item];
    projects = [{ path: "/demo", available: true, deployed: {} }];
    const { unmount } = renderWithQueryClient(
      <ProfilesPage activeApp="claude" projectOnly />,
    );
    fireEvent.click(await screen.findByRole("button", { name: "common.edit" }));
    const connection = await screen.findByLabelText(
      "profiles.editor.projectConnection",
    );
    expect(connection).toHaveValue("inherit");
    fireEvent.change(
      screen.getByLabelText("profiles.editor.projectModelProvider"),
      {
        target: { value: "provider-b" },
      },
    );
    expect(connection).toHaveValue("inherit");
    fireEvent.change(connection, { target: { value: "direct" } });
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(screen.queryByText("profiles.editor.title")).toBeNull(),
    );
    expect(profiles[0]).toMatchObject({
      target: "project",
      payload: {
        claudeProjectConnection: "direct",
        providers: { claude: "provider-b" },
      },
    });
    expect(mocks.deploy).not.toHaveBeenCalled();
    unmount();
    renderWithQueryClient(<ProfilesPage activeApp="claude" projectOnly />);
    fireEvent.click(await screen.findByRole("button", { name: "common.edit" }));
    expect(
      await screen.findByLabelText("profiles.editor.projectConnection"),
    ).toHaveValue("direct");
    expect(screen.getByLabelText("profiles.editor.provider")).toHaveValue(
      "provider-b",
    );
    fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.projectDeploy.button" }),
    );
    await waitFor(() =>
      expect(mocks.deploy).toHaveBeenCalledWith({
        id: item.id,
        scope: "claude",
        projectPath: "/demo",
        resolution: null,
      }),
    );
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(mocks.update).not.toHaveBeenCalled();
  });

  it("saves a profile that leaves resources under global control", async () => {
    const item = profile();
    item.payload.mcp.claude = ["mcp-a"];
    renderWithQueryClient(
      <ProfileEditPanel profile={item} scope="claude" onClose={vi.fn()} />,
    );
    fireEvent.click(
      await screen.findByRole("tab", { name: /profiles.editor.resourcesTab/ }),
    );
    fireEvent.click(
      screen.getByRole("switch", { name: "profiles.editor.manageResources" }),
    );
    expect(screen.queryByRole("checkbox", { name: "MCP A" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(mocks.saveConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          config: expect.objectContaining({ followGlobalResources: true }),
        }),
      ),
    );
  });
  it("reports invalid numeric settings after saving from another section and keeps resource choices for retry", async () => {
    const item = profile("codex");
    profiles = [item];
    mocks.saveConfig.mockRejectedValueOnce(new Error("Invalid context window"));
    renderWithQueryClient(
      <ProfileEditPanel profile={item} scope="codex" onClose={vi.fn()} />,
    );
    await screen.findByLabelText("profiles.editor.model", {
      selector: "input",
    });
    fireEvent.click(screen.getByText("profiles.editor.runtimeSettings"));
    fireEvent.change(
      screen.getByLabelText("codexConfig.catalogColumnContext", {
        selector: "input",
      }),
      { target: { value: "-1" } },
    );
    fireEvent.click(
      screen.getByRole("tab", { name: /profiles.editor.resourcesTab/ }),
    );
    fireEvent.click(screen.getByRole("checkbox", { name: "MCP A" }));
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Invalid context window",
    );
    expect(mocks.saveConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        config: expect.objectContaining({
          followGlobalResources: false,
          modelSettings: expect.objectContaining({
            model_context_window: "-1",
          }),
          mcpIds: ["mcp-a"],
        }),
      }),
    );
    expect(screen.getByRole("checkbox", { name: "MCP A" })).toBeChecked();
    fireEvent.click(
      screen.getByRole("tab", { name: "profiles.editor.modelsTab" }),
    );
    expect(
      screen.getByLabelText("codexConfig.catalogColumnContext", {
        selector: "input",
      }),
    ).toHaveValue(-1);
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("keeps invalid catalog feedback visible across sections and returns to the unfinished model", async () => {
    const item = profile("codex");
    item.payload.modelCatalog = {
      claude: null,
      "claude-desktop": null,
      codex: { models: [{ model: "" }] },
    };
    renderWithQueryClient(
      <ProfileEditPanel profile={item} scope="codex" onClose={vi.fn()} />,
    );
    await screen.findByRole("tab", { name: "profiles.editor.modelsTab" });
    fireEvent.click(
      screen.getByRole("tab", { name: /profiles.editor.resourcesTab/ }),
    );
    fireEvent.click(screen.getByRole("checkbox", { name: "Skill A" }));
    expect(screen.getByRole("alert")).toBeVisible();
    expect(screen.getByRole("button", { name: "common.save" })).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.editor.modelsTab" }),
    );
    expect(
      screen.getByRole("tab", { name: "profiles.editor.modelsTab" }),
    ).toHaveAttribute("aria-selected", "true");
    expect(
      screen.getByLabelText("实际请求模型", { selector: "input" }),
    ).toHaveValue("");
    fireEvent.click(
      screen.getByRole("tab", { name: /profiles.editor.resourcesTab/ }),
    );
    expect(screen.getByRole("checkbox", { name: "Skill A" })).toBeChecked();
    expect(mocks.saveConfig).not.toHaveBeenCalled();
  });

  it("confirms editor back and Escape using the current draft, and preserves it when leaving is cancelled", async () => {
    const onClose = vi.fn();
    renderWithQueryClient(
      <ProfileEditPanel profile={profile()} scope="claude" onClose={onClose} />,
    );
    const model = await screen.findByLabelText("profiles.editor.model", {
      selector: "input",
    });
    fireEvent.change(model, { target: { value: "unsaved-model" } });
    fireEvent.click(screen.getByRole("button", { name: "common.back" }));
    expect(onClose).not.toHaveBeenCalled();
    expect(
      await screen.findByText("common.unsavedLeaveTitle"),
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "common.unsavedLeaveCancel" }),
    );
    expect(model).toHaveValue("unsaved-model");
    fireEvent.keyDown(window, { key: "Escape" });
    expect(
      await screen.findByText("common.unsavedLeaveTitle"),
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "common.unsavedLeaveConfirm" }),
    );
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(mocks.saveConfig).not.toHaveBeenCalled();
  });

  it("does not confirm an editor draft restored to its initial values or an explicit cancel", async () => {
    const onClose = vi.fn();
    renderWithQueryClient(
      <ProfileEditPanel profile={profile()} scope="claude" onClose={onClose} />,
    );
    const model = await screen.findByLabelText("profiles.editor.model", {
      selector: "input",
    });
    fireEvent.change(model, { target: { value: "temporary" } });
    fireEvent.change(model, { target: { value: "" } });
    fireEvent.click(
      screen.getByRole("tab", { name: /profiles.editor.resourcesTab/ }),
    );
    const mcp = screen.getByRole("checkbox", { name: "MCP A" });
    fireEvent.click(mcp);
    fireEvent.click(mcp);
    fireEvent.click(screen.getByRole("button", { name: "common.back" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(screen.queryByText("common.unsavedLeaveTitle")).toBeNull();
    fireEvent.change(model, { target: { value: "cancelled" } });
    fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
    expect(onClose).toHaveBeenCalledTimes(2);
    expect(screen.queryByText("common.unsavedLeaveTitle")).toBeNull();
  });

  it("edits a project's independent model and resource references without preparing or changing global configuration", async () => {
    profiles = [
      {
        ...profile("claude", "project"),
        id: "a",
        name: "Project A",
        payload: {
          ...profile().payload,
          providers: {
            claude: "provider-a",
            "claude-desktop": null,
            codex: null,
          },
          models: {
            claude: "independent-a",
            "claude-desktop": null,
            codex: null,
          },
        },
      },
      {
        ...profile("claude", "project"),
        id: "b",
        name: "Project B",
        payload: {
          ...profile().payload,
          providers: {
            claude: "provider-a",
            "claude-desktop": null,
            codex: null,
          },
          models: {
            claude: "independent-b",
            "claude-desktop": null,
            codex: null,
          },
        },
      },
    ];
    renderWithQueryClient(<ProfilesPage activeApp="claude" projectOnly />);
    fireEvent.click(
      (await screen.findAllByRole("button", { name: "common.edit" }))[0],
    );
    expect(
      await screen.findByLabelText("profiles.editor.model", {
        selector: "input",
      }),
    ).toHaveValue("independent-a");
    fireEvent.change(
      screen.getByLabelText("profiles.editor.projectModelProvider"),
      {
        target: { value: "provider-b" },
      },
    );
    expect(
      screen.getByLabelText("profiles.editor.model", { selector: "input" }),
    ).toHaveValue("independent-a");
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.editor.resetModels" }),
    );
    expect(
      screen.getByLabelText("profiles.editor.model", { selector: "input" }),
    ).toHaveValue("claude-other");
    fireEvent.change(
      screen.getByLabelText("profiles.editor.model", { selector: "input" }),
      {
        target: { value: " private-model " },
      },
    );
    fireEvent.change(screen.getByLabelText("profiles.editor.name"), {
      target: { value: " Project A edited " },
    });
    fireEvent.click(
      screen.getByRole("tab", { name: /profiles.editor.resourcesTab/ }),
    );
    fireEvent.click(screen.getByRole("checkbox", { name: "MCP A" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Skill B" }));
    fireEvent.click(
      screen.getByRole("tab", { name: "profiles.editor.instructionsTab" }),
    );
    fireEvent.change(screen.getByLabelText("profiles.editor.prompt"), {
      target: { value: "prompt-a" },
    });
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(mocks.saveConfig).toHaveBeenCalledWith({
        id: "a",
        scope: "claude",
        name: "Project A edited",
        config: {
          claudeProjectConnection: "inherit",
          followGlobalResources: false,
          providerId: "provider-b",
          model: "private-model",
          modelSettings: {
            ANTHROPIC_DEFAULT_HAIKU_MODEL: "claude-other",
            ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME: "claude-other",
            ANTHROPIC_DEFAULT_SONNET_MODEL: "claude-other",
            ANTHROPIC_DEFAULT_SONNET_MODEL_NAME: "claude-other",
            ANTHROPIC_DEFAULT_OPUS_MODEL: "claude-other",
            ANTHROPIC_DEFAULT_OPUS_MODEL_NAME: "claude-other",
            ANTHROPIC_DEFAULT_FABLE_MODEL: "claude-other",
            ANTHROPIC_DEFAULT_FABLE_MODEL_NAME: "claude-other",
            CLAUDE_CODE_SUBAGENT_MODEL: "",
          },
          modelCatalog: null,
          mcpIds: ["mcp-a"],
          skillIds: ["skill-b"],
          promptId: "prompt-a",
        },
      }),
    );
    await waitFor(() =>
      expect(screen.queryByText("profiles.editor.title")).toBeNull(),
    );
    expect(
      profiles.find((item) => item.id === "b")?.payload.models?.claude,
    ).toBe("independent-b");
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(mocks.deploy).not.toHaveBeenCalled();
    expect(mocks.update).not.toHaveBeenCalled();
    const commands = mocks.invoke.mock.calls.map(([command]) => command);
    expect(commands).not.toContain("switch_provider");
    expect(commands).not.toContain("toggle_mcp_app");
    expect(commands).not.toContain("toggle_skill_app");
    expect(commands).not.toContain("enable_prompt");
  });

  it("keeps the originally edited agent scope when the parent page changes", async () => {
    profiles = [profile("codex")];
    const { rerender } = renderWithQueryClient(
      <ProfilesPage activeApp="codex" />,
    );
    fireEvent.click(await screen.findByRole("button", { name: "common.edit" }));
    await screen.findByLabelText("profiles.editor.model", {
      selector: "input",
    });
    fireEvent.change(
      screen.getByLabelText("profiles.editor.model", { selector: "input" }),
      {
        target: { value: "codex-independent" },
      },
    );
    rerender(<ProfilesPage activeApp="claude" />);
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(mocks.saveConfig).toHaveBeenCalledWith({
        id: "demo",
        scope: "codex",
        name: "Demo",
        config: {
          followGlobalResources: false,
          providerId: null,
          model: "codex-independent",
          modelSettings: {
            model_reasoning_effort: "",
            plan_mode_reasoning_effort: "",
            model_context_window: "",
            model_auto_compact_token_limit: "",
            "agents.default_subagent_model": "",
            "agents.default_subagent_reasoning_effort": "",
            "agents.max_threads": "",
          },
          modelCatalog: {
            models: [],
            autoReviewModel: "",
            officialSpeedTiers: false,
          },
          mcpIds: [],
          skillIds: [],
          promptId: null,
        },
      }),
    );
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("keeps failed edits for retry and prevents duplicate saving or dismissal while pending", async () => {
    profiles = [profile()];
    let finish!: (value: Profile) => void;
    mocks.saveConfig.mockRejectedValueOnce(new Error("save failed"));
    renderWithQueryClient(<ProfilesPage activeApp="claude" />);
    fireEvent.click(await screen.findByRole("button", { name: "common.edit" }));
    fireEvent.change(
      await screen.findByLabelText("profiles.editor.model", {
        selector: "input",
      }),
      {
        target: { value: "unsaved-model" },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("save failed");
    expect(
      screen.getByLabelText("profiles.editor.model", { selector: "input" }),
    ).toHaveValue("unsaved-model");
    mocks.saveConfig.mockReturnValueOnce(
      new Promise<Profile>((resolve) => {
        finish = resolve;
      }),
    );
    const save = screen.getByRole("button", { name: "common.save" });
    act(() => {
      fireEvent.click(save);
      fireEvent.click(save);
    });
    await waitFor(() => expect(mocks.saveConfig).toHaveBeenCalledTimes(2));
    expect(
      screen.getByRole("button", { name: "common.cancel" }),
    ).toBeDisabled();
    expect(
      screen.getByLabelText("profiles.editor.model", { selector: "input" }),
    ).toBeDisabled();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByText("profiles.editor.title")).toBeInTheDocument();
    await act(async () => finish(profiles[0]));
    await waitFor(() =>
      expect(screen.queryByText("profiles.editor.title")).toBeNull(),
    );
  });

  it("does not erase missing resource references when editing another field", async () => {
    const item = profile();
    item.payload.mcp.claude = ["missing-mcp"];
    item.payload.skills.claude = ["missing-skill"];
    profiles = [item];
    renderWithQueryClient(
      <ProfileEditPanel profile={item} scope="claude" onClose={vi.fn()} />,
    );
    await screen.findByLabelText("profiles.editor.model", {
      selector: "input",
    });
    fireEvent.click(
      screen.getByRole("tab", { name: /profiles.editor.resourcesTab/ }),
    );
    const mcpSection = screen
      .getByText(/profiles.editor.mcp/)
      .closest("fieldset")!;
    const skillSection = screen
      .getByText(/profiles.editor.skills/)
      .closest("fieldset")!;
    expect(
      within(mcpSection).getByRole("checkbox", {
        name: "missing-mcp profiles.editor.unavailable",
      }),
    ).toBeChecked();
    expect(
      within(skillSection).getByRole("checkbox", {
        name: "missing-skill profiles.editor.unavailable",
      }),
    ).toBeChecked();
    fireEvent.change(screen.getByLabelText("profiles.editor.name"), {
      target: { value: "Renamed" },
    });
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(mocks.saveConfig).toHaveBeenCalledWith({
        id: "demo",
        scope: "claude",
        name: "Renamed",
        config: {
          followGlobalResources: false,
          providerId: null,
          model: "",
          modelSettings: {
            ANTHROPIC_DEFAULT_HAIKU_MODEL: "",
            ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME: "",
            ANTHROPIC_DEFAULT_SONNET_MODEL: "",
            ANTHROPIC_DEFAULT_SONNET_MODEL_NAME: "",
            ANTHROPIC_DEFAULT_OPUS_MODEL: "",
            ANTHROPIC_DEFAULT_OPUS_MODEL_NAME: "",
            ANTHROPIC_DEFAULT_FABLE_MODEL: "",
            ANTHROPIC_DEFAULT_FABLE_MODEL_NAME: "",
            CLAUDE_CODE_SUBAGENT_MODEL: "",
          },
          modelCatalog: null,
          mcpIds: ["missing-mcp"],
          skillIds: ["missing-skill"],
          promptId: null,
        },
      }),
    );
  });

  it("shows saved names and IDs for missing resources and lets removal be undone before saving", async () => {
    const item = profile();
    item.payload.mcp.claude = ["deleted-mcp"];
    item.payload.skills.claude = ["deleted-skill"];
    item.payload.prompts.claude = "deleted-prompt";
    item.payload.resourceNames = {
      claude: {
        mcp: { "deleted-mcp": "Project docs" },
        skills: { "deleted-skill": "Code review" },
        prompts: { "deleted-prompt": "Team instructions" },
      },
      codex: {
        mcp: { "deleted-mcp": "Different agent name" },
        skills: {},
        prompts: {},
      },
      "claude-desktop": { mcp: {}, skills: {}, prompts: {} },
    };
    profiles = [item];
    renderWithQueryClient(
      <ProfileEditPanel profile={item} scope="claude" onClose={vi.fn()} />,
    );
    fireEvent.click(
      await screen.findByRole("tab", { name: /profiles.editor.resourcesTab/ }),
    );
    const mcp = await screen.findByRole("checkbox", {
      name: /Project docs.*deleted-mcp/,
    });
    expect(mcp).toBeChecked();
    expect(
      screen.getByRole("checkbox", { name: /Code review.*deleted-skill/ }),
    ).toBeChecked();
    fireEvent.click(
      screen.getByRole("tab", { name: "profiles.editor.instructionsTab" }),
    );
    expect(
      screen.getByRole("option", { name: /Team instructions.*deleted-prompt/ }),
    ).toBeInTheDocument();
    expect(screen.queryByText("Different agent name")).not.toBeInTheDocument();
    const prompt = screen.getByLabelText("profiles.editor.prompt");
    fireEvent.change(prompt, { target: { value: "" } });
    expect(
      screen.getByRole("option", { name: /Team instructions.*deleted-prompt/ }),
    ).toBeInTheDocument();
    fireEvent.change(prompt, { target: { value: "deleted-prompt" } });
    expect(prompt).toHaveValue("deleted-prompt");
    fireEvent.click(
      screen.getByRole("tab", { name: /profiles.editor.resourcesTab/ }),
    );
    fireEvent.click(mcp);
    expect(mcp).not.toBeChecked();
    fireEvent.click(mcp);
    expect(mcp).toBeChecked();
    fireEvent.click(
      screen.getByRole("checkbox", { name: /Code review.*deleted-skill/ }),
    );
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(mocks.saveConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          config: expect.objectContaining({
            followGlobalResources: false,
            mcpIds: ["deleted-mcp"],
            skillIds: [],
            promptId: "deleted-prompt",
          }),
        }),
      ),
    );
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("blocks saving when a resource library failed to load, then retries without losing the profile's values", async () => {
    const item = profile();
    item.payload.models = {
      claude: "stored-model",
      "claude-desktop": null,
      codex: null,
    };
    profiles = [item];
    const invoke = mocks.invoke.getMockImplementation()!;
    let unavailable = true;
    mocks.invoke.mockImplementation((command: string, args: unknown) =>
      command === "get_mcp_servers" && unavailable
        ? Promise.reject(new Error("offline"))
        : invoke(command, args),
    );
    renderWithQueryClient(
      <ProfileEditPanel profile={item} scope="claude" onClose={vi.fn()} />,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "profiles.editor.loadFailed",
    );
    expect(screen.queryByRole("button", { name: "common.save" })).toBeNull();
    expect(mocks.saveConfig).not.toHaveBeenCalled();
    unavailable = false;
    fireEvent.click(screen.getByRole("button", { name: "common.retry" }));
    expect(
      await screen.findByLabelText("profiles.editor.model", {
        selector: "input",
      }),
    ).toHaveValue("stored-model");
  });

  it("treats a model-only configuration as available for the selected agent", async () => {
    const item = profile();
    item.payload.mcp.claude = null;
    item.payload.skills.claude = null;
    item.payload.models = {
      claude: null,
      "claude-desktop": null,
      codex: "stored-model",
    };
    profiles = [item];
    renderWithQueryClient(<ProfilesPage activeApp="codex" />);
    expect(
      await screen.findByRole("button", {
        name: "profiles.apply",
      }),
    ).toBeEnabled();
  });
});
