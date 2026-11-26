import {
  act,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { onlineManager } from "@tanstack/react-query";
import { ProfileSwitcher } from "@/components/profiles/ProfileSwitcher";
import { ProfileManageDialog } from "@/components/profiles/ProfileManageDialog";
import { ProfileEditPanel } from "@/components/profiles/ProfileEditPanel";
import { hasScopeSnapshot } from "@/components/profiles/scope";
import { profilesApi } from "@/lib/api/profiles";
import type { Profile, ProfileConfig, ProfileScope } from "@/lib/api/profiles";
import { renderWithQueryClient } from "../utils/testQueryClient";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  migrate: vi.fn(),
  create: vi.fn(),
  importProject: vi.fn(),
  duplicate: vi.fn(),
  update: vi.fn(),
  saveConfig: vi.fn(),
  apply: vi.fn(),
  clear: vi.fn(),
  setProjectPath: vi.fn(),
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
  projectPath: string | null = null,
  capturedScope: "claude" | "codex" = "claude",
): Profile => ({
  id: "demo",
  name: "Demo",
  projectPath,
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
let currentIds: {
  claude: string | null;
  claudeDesktop: string | null;
  codex: string | null;
};

beforeEach(() => {
  localStorage.removeItem("cc-switch:profiles:project-selection:v1");
  Object.values(mocks).forEach((mock) => mock.mockReset());
  profiles = [];
  currentIds = { claude: null, claudeDesktop: null, codex: null };
  mocks.updateTrayMenu.mockResolvedValue(true);
  mocks.migrate.mockResolvedValue({ profiles: [], warnings: [] });
  mocks.pickDirectory.mockResolvedValue("/projects/demo");
  mocks.create.mockResolvedValue(profile());
  mocks.apply.mockResolvedValue([]);
  mocks.clear.mockResolvedValue(undefined);
  mocks.duplicate.mockImplementation(
    ({ name, projectPath }: { name: string; projectPath: string }) => {
      const copy = { ...profile(projectPath), id: "copy", name };
      profiles = [...profiles, copy];
      return Promise.resolve(copy);
    },
  );
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
                mcp: { ...item.payload.mcp, [scope]: config.mcpIds },
                skills: { ...item.payload.skills, [scope]: config.skillIds },
                prompts: {
                  ...item.payload.prompts,
                  [scope]: config.promptId ?? "",
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
  mocks.setProjectPath.mockImplementation(
    ({ id, projectPath }: { id: string; projectPath: string | null }) => {
      const updated = {
        ...profiles.find((item) => item.id === id)!,
        projectPath,
      };
      profiles = profiles.map((item) => (item.id === id ? updated : item));
      return Promise.resolve(updated);
    },
  );
  mocks.invoke.mockImplementation((command: string, args: unknown) => {
    if (command === "migrate_legacy_agent_profiles") return mocks.migrate(args);
    switch (command) {
      case "list_profiles":
        return Promise.resolve({
          profiles,
          currentIds,
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
      case "clear_current_profile":
        return mocks.clear(args);
      case "set_profile_project_path":
        return mocks.setProjectPath(args);
      case "deploy_profile_to_project":
        return mocks.deploy(args);
      default:
        throw new Error(`Unexpected command: ${command}`);
    }
  });
  Element.prototype.scrollIntoView = vi.fn();
});

async function openCreate(
  activeApp: "claude" | "claude-desktop" | "codex" = "claude",
) {
  renderWithQueryClient(<ProfileSwitcher activeApp={activeApp} />);
  fireEvent.click(
    screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
  );
  fireEvent.click(
    await screen.findByRole("option", { name: "profiles.createFromCurrent" }),
  );
  return screen.getByPlaceholderText("profiles.namePlaceholder");
}

async function chooseProject(path: string) {
  const trigger = screen.getByRole("combobox", {
    name: "profiles.projectContext",
  });
  await waitFor(async () => {
    if (trigger.getAttribute("aria-expanded") !== "true")
      await userEvent.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "true");
  });
  await userEvent.click(
    await screen.findByRole("option", {
      name: path || "profiles.scope.global",
    }),
  );
  await waitFor(() =>
    expect(trigger).toHaveAttribute("aria-expanded", "false"),
  );
}

async function openProfileActions(row?: HTMLElement) {
  const query = row ? within(row) : screen;
  const trigger = await query.findByRole("button", {
    name: "profiles.context.rowActions",
  });
  await waitFor(async () => {
    if (trigger.getAttribute("aria-expanded") !== "true")
      await userEvent.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "true");
  });
  await screen.findByRole("menu");
}

async function openContextActions() {
  const trigger = screen.getByRole("button", { name: "profiles.context.more" });
  await waitFor(async () => {
    if (trigger.getAttribute("aria-expanded") !== "true")
      await userEvent.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "true");
  });
  await screen.findByRole("menu");
}

describe("project environment profile entry", () => {
  it("restores a project selection without exposing global actions and waits for readiness to deploy", async () => {
    profiles = [
      { ...profile(), id: "global", name: "Global" },
      {
        ...profile("/projects/demo", "codex"),
        id: "selected",
        name: "Selected",
      },
      {
        ...profile("/projects/demo", "codex"),
        id: "last",
        name: "Last applied",
        deployedScopes: ["codex"],
      },
    ];
    currentIds.codex = "global";
    const props = {
      activeApp: "codex" as const,
      projectOnly: true,
      initialProjectPath: "/projects/demo",
      initialProfileId: "selected",
    };
    const { rerender } = renderWithQueryClient(
      <ProfileSwitcher {...props} deploymentDisabled />,
    );
    await screen.findByText("Selected");
    expect(screen.getByLabelText("profiles.projectContext")).toHaveAttribute(
      "data-project-path",
      "/projects/demo",
    );
    expect(
      screen.queryByRole("option", { name: "profiles.scope.global" }),
    ).toBeNull();
    const deploy = screen.getByRole("button", {
      name: "profiles.projectDeploy.button",
    });
    expect(deploy).toBeDisabled();
    fireEvent.click(deploy);
    expect(mocks.deploy).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
    );
    expect(
      await screen.findByRole("option", { name: /Last applied/ }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Global/ })).toBeNull();
    expect(
      screen.queryByRole("option", { name: "profiles.manage" }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "profiles.none" })).toBeNull();
    fireEvent.click(screen.getByRole("option", { name: /Selected/ }));
    rerender(<ProfileSwitcher {...props} />);
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.projectDeploy.button" }),
    );
    await waitFor(() =>
      expect(mocks.deploy).toHaveBeenCalledWith({
        id: "selected",
        scope: "codex",
        resolution: null,
      }),
    );
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(mocks.clear).not.toHaveBeenCalled();
  });

  it("can prepare a project and optional resources before installation without applying anything", async () => {
    mocks.create.mockImplementation(
      ({ name, projectPath }: { name: string; projectPath: string }) => {
        const created = { ...profile(projectPath), name };
        profiles = [created];
        return Promise.resolve(created);
      },
    );
    renderWithQueryClient(
      <ProfileSwitcher activeApp="claude" projectOnly deploymentDisabled />,
    );
    expect(screen.getByLabelText("profiles.projectContext")).toHaveAttribute(
      "data-project-path",
      "",
    );
    expect(
      screen.getByRole("combobox", { name: "profiles.projectContext" }),
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
    );
    fireEvent.click(
      await screen.findByRole("option", { name: "profiles.createFromCurrent" }),
    );
    expect(
      screen.queryByRole("radio", { name: "profiles.scope.global" }),
    ).toBeNull();
    const name = screen.getByPlaceholderText("profiles.namePlaceholder");
    fireEvent.change(name, { target: { value: "Prepared project" } });
    fireEvent.keyDown(name, { key: "Enter" });
    expect(mocks.create).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: "common.confirm" }),
    ).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.projectPath.pick" }),
    );
    await waitFor(() =>
      expect(screen.getByLabelText("profiles.projectPath.label")).toHaveValue(
        "/projects/demo",
      ),
    );
    fireEvent.click(screen.getByRole("button", { name: "common.confirm" }));
    await screen.findByLabelText("profiles.editor.model", {
      selector: "input",
    });
    expect(mocks.create).toHaveBeenCalledWith({
      name: "Prepared project",
      scope: "claude",
      projectPath: "/projects/demo",
    });
    fireEvent.click(
      screen.getByRole("tab", { name: /profiles.editor.resourcesTab/ }),
    );
    fireEvent.click(screen.getByRole("checkbox", { name: "MCP A" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Skill A" }));
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(mocks.saveConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          scope: "claude",
          config: expect.objectContaining({
            mcpIds: ["mcp-a"],
            skillIds: ["skill-a"],
          }),
        }),
      ),
    );
    await waitFor(() =>
      expect(screen.queryByText("profiles.editor.title")).toBeNull(),
    );
    expect(
      screen.getByRole("button", { name: "profiles.projectDeploy.button" }),
    ).toBeDisabled();
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("keeps per-Agent project selections independent without Agent management actions", async () => {
    profiles = [
      {
        ...profile("/projects/demo"),
        id: "claude-profile",
        name: "Claude project",
      },
    ];
    const props = {
      projectOnly: true,
      initialProjectPath: "/projects/demo",
      initialProfileId: "claude-profile",
    };
    const { rerender } = renderWithQueryClient(
      <ProfileSwitcher {...props} activeApp="claude" />,
    );
    await screen.findByText("Claude project");
    await openContextActions();
    expect(
      screen.queryByRole("menuitem", {
        name: "profiles.environment.manageAgent",
      }),
    ).toBeNull();
    expect(
      screen.getByRole("menuitem", { name: "profiles.manage" }),
    ).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    rerender(<ProfileSwitcher {...props} activeApp="codex" />);
    expect(screen.getByLabelText("profiles.projectContext")).toHaveAttribute(
      "data-project-path",
      "",
    );
    rerender(<ProfileSwitcher {...props} activeApp="claude" />);
    expect(screen.getByLabelText("profiles.projectContext")).toHaveAttribute(
      "data-project-path",
      "/projects/demo",
    );
    expect(screen.getByText("Claude project")).toBeInTheDocument();
    expect(mocks.apply).not.toHaveBeenCalled();
  });

  it("disables conflict confirmation when readiness changes but still allows cancellation", async () => {
    profiles = [profile("/projects/demo")];
    mocks.deploy
      .mockResolvedValueOnce({
        status: "needsResolution",
        conflicts: [{ path: "CLAUDE.md", reason: "user edit" }],
        warnings: [],
      })
      .mockResolvedValueOnce({ status: "skipped" });
    const props = {
      activeApp: "claude" as const,
      projectOnly: true,
      initialProjectPath: "/projects/demo",
      initialProfileId: "demo",
    };
    const { rerender } = renderWithQueryClient(<ProfileSwitcher {...props} />);
    fireEvent.click(
      await screen.findByRole("button", {
        name: "profiles.projectDeploy.button",
      }),
    );
    await screen.findByText("CLAUDE.md");
    rerender(<ProfileSwitcher {...props} deploymentDisabled />);
    expect(
      screen.getByRole("button", { name: "profiles.projectDeploy.overwrite" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", {
        name: "profiles.projectDeploy.keepCurrent",
      }),
    ).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.projectDeploy.skip" }),
    );
    await waitFor(() =>
      expect(mocks.deploy).toHaveBeenLastCalledWith({
        id: "demo",
        scope: "claude",
        resolution: "skip",
      }),
    );
    expect(mocks.deploy).toHaveBeenCalledTimes(2);
    expect(mocks.apply).not.toHaveBeenCalled();
  });
});

describe("profile folder creation", () => {
  it("imports existing project configuration before installation and opens the shared editor", async () => {
    mocks.importProject.mockImplementation(
      ({ name, projectPath }: { name: string; projectPath: string }) => {
        const imported = { ...profile(projectPath), name };
        profiles = [imported];
        return Promise.resolve({
          profile: imported,
          warnings: ["Skipped unreadable Skill"],
        });
      },
    );
    renderWithQueryClient(
      <ProfileSwitcher activeApp="claude" projectOnly deploymentDisabled />,
    );
    fireEvent.click(
      screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
    );
    fireEvent.click(
      await screen.findByRole("option", { name: "profiles.createFromCurrent" }),
    );
    fireEvent.change(screen.getByPlaceholderText("profiles.namePlaceholder"), {
      target: { value: "Imported" },
    });
    fireEvent.change(screen.getByLabelText("profiles.createSource.label"), {
      target: { value: "project" },
    });
    expect(
      screen.getByText("profiles.createSource.description"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "common.confirm" }),
    ).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.projectPath.pick" }),
    );
    await waitFor(() =>
      expect(screen.getByLabelText("profiles.projectPath.label")).toHaveValue(
        "/projects/demo",
      ),
    );
    fireEvent.click(screen.getByRole("button", { name: "common.confirm" }));
    await screen.findByLabelText("profiles.editor.model", {
      selector: "input",
    });
    expect(mocks.importProject).toHaveBeenCalledWith({
      name: "Imported",
      scope: "claude",
      projectPath: "/projects/demo",
    });
    expect(mocks.warning).toHaveBeenCalledWith(
      "profiles.import.completedWithNotes",
      expect.objectContaining({ description: "Skipped unreadable Skill" }),
    );
    expect(mocks.create).not.toHaveBeenCalled();
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("confirms leaving a changed create form and preserves the draft when staying", async () => {
    const name = await openCreate();
    fireEvent.change(name, { target: { value: "Draft" } });
    fireEvent.click(screen.getByRole("button", { name: "common.back" }));
    expect(
      await screen.findByText("common.unsavedLeaveTitle"),
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "common.unsavedLeaveCancel" }),
    );
    expect(name).toHaveValue("Draft");
    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.click(
      await screen.findByRole("button", { name: "common.unsavedLeaveConfirm" }),
    );
    // FullScreenPanel remains mounted until AnimatePresence finishes its exit.
    await waitFor(() =>
      expect(
        screen.queryByPlaceholderText("profiles.namePlaceholder"),
      ).toBeNull(),
    );
    expect(mocks.create).not.toHaveBeenCalled();
  });

  it("closes a changed create form directly on explicit cancellation", async () => {
    const name = await openCreate();
    fireEvent.change(name, {
      target: { value: "Explicit cancel" },
    });
    fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
    expect(screen.queryByText("common.unsavedLeaveTitle")).toBeNull();
    await waitFor(() =>
      expect(
        screen.queryByPlaceholderText("profiles.namePlaceholder"),
      ).toBeNull(),
    );
    expect(mocks.create).not.toHaveBeenCalled();
  });

  it("does not confirm create form changes that were restored", async () => {
    const name = await openCreate();
    fireEvent.change(name, { target: { value: "temporary" } });
    fireEvent.change(name, { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "common.back" }));
    expect(screen.queryByText("common.unsavedLeaveTitle")).toBeNull();
    await waitFor(() =>
      expect(
        screen.queryByPlaceholderText("profiles.namePlaceholder"),
      ).toBeNull(),
    );
  });

  it("keeps the import form and selected directory when reading the project fails", async () => {
    mocks.importProject.mockRejectedValue(new Error("Malformed project TOML"));
    await openCreate("codex");
    fireEvent.click(
      screen.getByRole("radio", { name: "profiles.scope.project" }),
    );
    fireEvent.change(screen.getByPlaceholderText("profiles.namePlaceholder"), {
      target: { value: "Retry project" },
    });
    fireEvent.change(screen.getByLabelText("profiles.createSource.label"), {
      target: { value: "project" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.projectPath.pick" }),
    );
    await waitFor(() =>
      expect(screen.getByLabelText("profiles.projectPath.label")).toHaveValue(
        "/projects/demo",
      ),
    );
    fireEvent.click(screen.getByRole("button", { name: "common.confirm" }));
    await waitFor(() => expect(mocks.error).toHaveBeenCalled());
    expect(screen.getByPlaceholderText("profiles.namePlaceholder")).toHaveValue(
      "Retry project",
    );
    expect(screen.getByLabelText("profiles.createSource.label")).toHaveValue(
      "project",
    );
    expect(
      screen.getByRole("button", { name: "common.confirm" }),
    ).toBeEnabled();
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("loads, edits and saves local project configuration while offline", async () => {
    profiles = [profile("/projects/demo")];
    onlineManager.setOnline(false);
    try {
      renderWithQueryClient(
        <ProfileSwitcher
          activeApp="claude"
          projectOnly
          initialProjectPath="/projects/demo"
          initialProfileId="demo"
        />,
      );
      fireEvent.click(
        await screen.findByRole("button", { name: "common.edit" }),
      );
      fireEvent.change(
        await screen.findByLabelText("profiles.editor.model", {
          selector: "input",
        }),
        {
          target: { value: "offline-model" },
        },
      );
      fireEvent.click(screen.getByRole("button", { name: "common.save" }));
      await waitFor(() =>
        expect(mocks.saveConfig).toHaveBeenCalledWith(
          expect.objectContaining({
            config: expect.objectContaining({ model: "offline-model" }),
          }),
        ),
      );
      await waitFor(() =>
        expect(screen.queryByText("profiles.editor.title")).toBeNull(),
      );
    } finally {
      onlineManager.setOnline(true);
    }
  });

  it("requires a folder for project scope even when submitted with Enter", async () => {
    const name = await openCreate();
    fireEvent.change(name, { target: { value: "Demo" } });
    fireEvent.click(
      screen.getByRole("radio", { name: "profiles.scope.project" }),
    );
    expect(
      screen.getByRole("button", { name: "common.confirm" }),
    ).toBeDisabled();
    fireEvent.keyDown(name, { key: "Enter" });
    expect(mocks.create).not.toHaveBeenCalled();
  });

  it("keeps Claude Desktop creation global", async () => {
    const name = await openCreate("claude-desktop");
    expect(
      screen.queryByRole("radio", { name: "profiles.scope.project" }),
    ).toBeNull();
    fireEvent.change(name, { target: { value: "Desktop" } });
    fireEvent.click(screen.getByRole("button", { name: "common.confirm" }));
    await waitFor(() =>
      expect(mocks.create).toHaveBeenCalledWith({
        name: "Desktop",
        scope: "claude-desktop",
        projectPath: null,
      }),
    );
  });

  it("keeps create(name, scope) compatible with an omitted folder", async () => {
    await profilesApi.create("Demo", "codex");
    expect(mocks.invoke).toHaveBeenCalledWith("create_profile", {
      name: "Demo",
      scope: "codex",
      projectPath: null,
    });
  });

  it.each([null, "/projects/demo"])(
    "creates with folder %s in one request and ignores duplicate submission",
    async (projectPath) => {
      let finish!: (value: Profile) => void;
      mocks.create.mockReturnValueOnce(
        new Promise<Profile>((resolve) => {
          finish = resolve;
        }),
      );
      const name = await openCreate();
      fireEvent.change(name, { target: { value: " Demo " } });
      if (projectPath) {
        fireEvent.click(
          screen.getByRole("radio", { name: "profiles.scope.project" }),
        );
        fireEvent.click(
          screen.getByRole("button", { name: "profiles.projectPath.pick" }),
        );
        await waitFor(() =>
          expect(
            screen.getByLabelText("profiles.projectPath.label"),
          ).toHaveValue(projectPath),
        );
      }

      act(() => {
        fireEvent.keyDown(name, { key: "Enter" });
        fireEvent.keyDown(name, { key: "Enter" });
      });
      await waitFor(() => expect(mocks.create).toHaveBeenCalledTimes(1));
      expect(mocks.create).toHaveBeenCalledWith({
        name: "Demo",
        scope: "claude",
        projectPath,
      });
      expect(mocks.setProjectPath).not.toHaveBeenCalled();
      expect(
        screen.getByRole("button", { name: "common.cancel" }),
      ).toBeDisabled();
      fireEvent.keyDown(window, { key: "Escape" });
      expect(name).toBeInTheDocument();

      await act(async () => finish(profile(projectPath)));
      await screen.findByLabelText("profiles.editor.model", {
        selector: "input",
      });
    },
  );

  it.each(["invalid folder", "folder no longer exists"])(
    "keeps form values after %s and allows a corrected retry",
    async (reason) => {
      mocks.create.mockRejectedValueOnce(new Error(reason));
      const name = await openCreate();
      fireEvent.change(name, { target: { value: "Demo" } });
      fireEvent.click(
        screen.getByRole("radio", { name: "profiles.scope.project" }),
      );
      fireEvent.click(
        screen.getByRole("button", { name: "profiles.projectPath.pick" }),
      );
      await waitFor(() =>
        expect(screen.getByLabelText("profiles.projectPath.label")).toHaveValue(
          "/projects/demo",
        ),
      );
      fireEvent.click(screen.getByRole("button", { name: "common.confirm" }));
      await waitFor(() => expect(mocks.error).toHaveBeenCalled());
      expect(name).toHaveValue("Demo");
      expect(screen.getByLabelText("profiles.projectPath.label")).toHaveValue(
        "/projects/demo",
      );
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "common.confirm" }),
        ).toBeEnabled(),
      );

      fireEvent.click(
        screen.getByRole("radio", { name: "profiles.scope.global" }),
      );
      fireEvent.click(screen.getByRole("button", { name: "common.confirm" }));
      await screen.findByLabelText("profiles.editor.model", {
        selector: "input",
      });
      expect(mocks.create).toHaveBeenCalledTimes(2);
      expect(mocks.create).toHaveBeenLastCalledWith({
        name: "Demo",
        scope: "claude",
        projectPath: null,
      });
      expect(mocks.setProjectPath).not.toHaveBeenCalled();
    },
  );
});

describe("global and project profile separation", () => {
  it("keeps an implicitly selected deployed profile selected after editing its scope, ready for explicit application", async () => {
    const item = profile("/projects/demo");
    item.payload.providers.claude = "provider-a";
    item.payload.models = {
      claude: "claude-original",
      "claude-desktop": null,
      codex: "codex-original",
    };
    item.deployedScopes = ["claude", "codex"];
    profiles = [item];
    const saveConfig = mocks.saveConfig.getMockImplementation()!;
    mocks.saveConfig.mockImplementation(
      (args: {
        id: string;
        scope: ProfileScope;
        config: ProfileConfig;
        name?: string;
      }) => {
        saveConfig(args);
        const updated = profiles.find((record) => record.id === args.id)!;
        updated.deployedScopes = updated.deployedScopes?.filter(
          (scope) => scope !== args.scope,
        );
        return Promise.resolve(updated);
      },
    );
    mocks.deploy.mockImplementationOnce(
      ({ id, scope }: { id: string; scope: ProfileScope }) => {
        profiles = profiles.map((record) =>
          record.id === id
            ? {
                ...record,
                deployedScopes: [...(record.deployedScopes ?? []), scope],
              }
            : record,
        );
        return Promise.resolve({
          status: "applied",
          changed: [],
          warnings: [],
        });
      },
    );
    renderWithQueryClient(<ProfileSwitcher activeApp="claude" />);
    await screen.findByRole("button", { name: "profiles.context.more" });
    await chooseProject("/projects/demo");
    expect(screen.getByText("Demo")).toBeInTheDocument();
    expect(screen.queryByText("profiles.pendingSelection")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "common.edit" }));
    fireEvent.change(
      await screen.findByLabelText("profiles.editor.model", {
        selector: "input",
      }),
      {
        target: { value: "updated-model" },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(screen.queryByText("profiles.editor.title")).toBeNull(),
    );
    expect(screen.getByText("Demo")).toBeInTheDocument();
    expect(screen.getByText("profiles.pendingSelection")).toBeInTheDocument();
    expect(profiles[0].deployedScopes).toEqual(["codex"]);
    expect(profiles[0].payload.models?.codex).toBe("codex-original");
    const apply = screen.getByRole("button", {
      name: "profiles.projectDeploy.button",
    });
    expect(apply).toBeEnabled();
    expect(mocks.deploy).not.toHaveBeenCalled();
    expect(mocks.apply).not.toHaveBeenCalled();
    fireEvent.click(apply);
    await waitFor(() =>
      expect(mocks.deploy).toHaveBeenCalledWith({
        id: "demo",
        scope: "claude",
        resolution: null,
      }),
    );
    await waitFor(() =>
      expect(screen.queryByText("profiles.pendingSelection")).toBeNull(),
    );
  });

  it("synchronizes a profile edited from management into the header's viewing context without applying global configuration", async () => {
    const item = profile("/projects/demo");
    item.deployedScopes = ["claude"];
    profiles = [{ ...profile(), id: "global", name: "Global" }, item];
    currentIds.claude = "global";
    const saveConfig = mocks.saveConfig.getMockImplementation()!;
    mocks.saveConfig.mockImplementation(
      (args: {
        id: string;
        scope: ProfileScope;
        config: ProfileConfig;
        name?: string;
      }) => {
        saveConfig(args);
        const updated = profiles.find((record) => record.id === args.id)!;
        updated.deployedScopes = [];
        return Promise.resolve(updated);
      },
    );
    renderWithQueryClient(<ProfileSwitcher activeApp="claude" />);
    await screen.findByText("Global");
    fireEvent.click(
      screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
    );
    fireEvent.click(
      await screen.findByRole("option", { name: "profiles.manage" }),
    );
    const row = screen.getByText("Demo").closest(".flex-col")!.parentElement!;
    fireEvent.click(within(row).getByRole("button", { name: "common.edit" }));
    fireEvent.change(
      await screen.findByLabelText("profiles.editor.model", {
        selector: "input",
      }),
      {
        target: { value: "updated-model" },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(screen.queryByText("profiles.editor.title")).toBeNull(),
    );
    fireEvent.click(screen.getByRole("button", { name: "common.close" }));
    expect(screen.getByLabelText("profiles.projectContext")).toHaveAttribute(
      "data-project-path",
      "/projects/demo",
    );
    expect(screen.getByText("Demo")).toBeInTheDocument();
    expect(screen.getByText("profiles.pendingSelection")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "profiles.projectDeploy.button" }),
    ).toBeEnabled();
    expect(currentIds.claude).toBe("global");
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("selects a project and its profile without global writes, then explicitly deploys the selection", async () => {
    const user = userEvent.setup();
    profiles = [
      { ...profile(null, "codex"), id: "global", name: "Global" },
      { ...profile("/projects/demo", "codex"), id: "a", name: "Project A" },
      {
        ...profile("/projects/demo", "codex"),
        id: "b",
        name: "Project B",
        deployedScopes: ["codex"],
      },
      { ...profile("/projects/other", "codex"), id: "other", name: "Other" },
    ];
    currentIds.codex = "global";
    mocks.deploy.mockImplementationOnce(
      ({ id, scope }: { id: string; scope: ProfileScope }) => {
        profiles = profiles.map((item) => ({
          ...item,
          deployedScopes: item.id === id ? [scope] : [],
        }));
        return Promise.resolve({
          status: "applied",
          changed: [],
          warnings: [],
        });
      },
    );
    renderWithQueryClient(<ProfileSwitcher activeApp="codex" />);
    await screen.findByText("Global");
    await chooseProject("/projects/demo");
    expect(screen.getByText("Project B")).toBeInTheDocument();
    await user.click(
      screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
    );
    expect(screen.queryByRole("option", { name: /Global/ })).toBeNull();
    expect(screen.queryByRole("option", { name: /Other/ })).toBeNull();
    await user.click(screen.getByRole("option", { name: /Project A/ }));
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(mocks.deploy).not.toHaveBeenCalled();
    expect(screen.getByText("profiles.pendingSelection")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "profiles.projectDeploy.button" }),
    );
    await waitFor(() =>
      expect(mocks.deploy).toHaveBeenCalledWith({
        id: "a",
        scope: "codex",
        resolution: null,
      }),
    );
    await waitFor(() =>
      expect(screen.queryByText("profiles.pendingSelection")).toBeNull(),
    );
    expect(currentIds.codex).toBe("global");
    expect(mocks.apply).not.toHaveBeenCalled();
  });

  it("keeps project selections separate when switching agent pages", async () => {
    profiles = [
      { ...profile("/projects/demo"), id: "claude", name: "Claude profile" },
      {
        ...profile("/projects/demo", "codex"),
        id: "codex",
        name: "Codex profile",
      },
    ];
    const { rerender } = renderWithQueryClient(
      <ProfileSwitcher activeApp="claude" />,
    );
    await screen.findByRole("button", { name: "profiles.context.more" });
    await chooseProject("/projects/demo");
    fireEvent.click(
      screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
    );
    fireEvent.click(
      await screen.findByRole("option", { name: /Claude profile/ }),
    );
    rerender(<ProfileSwitcher activeApp="codex" />);
    expect(screen.getByLabelText("profiles.projectContext")).toHaveAttribute(
      "data-project-path",
      "",
    );
    await chooseProject("/projects/demo");
    await act(() => new Promise((resolve) => setTimeout(resolve, 220)));
    fireEvent.click(
      screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
    );
    fireEvent.click(
      await screen.findByRole("option", { name: /Codex profile/ }),
    );
    rerender(<ProfileSwitcher activeApp="claude" />);
    expect(screen.getByLabelText("profiles.projectContext")).toHaveAttribute(
      "data-project-path",
      "/projects/demo",
    );
    expect(screen.getByText("Claude profile")).toBeInTheDocument();
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("provides an explicit apply action for the selected global Profile", async () => {
    profiles = [{ ...profile(), id: "global", name: "Global" }];
    currentIds.claude = "global";
    renderWithQueryClient(<ProfileSwitcher activeApp="claude" />);
    const apply = await screen.findByRole("button", { name: "profiles.apply" });
    fireEvent.click(apply);
    await waitFor(() =>
      expect(mocks.apply).toHaveBeenCalledWith({
        id: "global",
        scope: "claude",
      }),
    );
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("keeps the saved global Profile selected after its applied marker is cleared", async () => {
    profiles = [{ ...profile(), id: "global", name: "Global" }];
    currentIds.claude = "global";
    const saveConfig = mocks.saveConfig.getMockImplementation()!;
    mocks.saveConfig.mockImplementation((args) => {
      currentIds.claude = null;
      return saveConfig(args);
    });
    renderWithQueryClient(<ProfileSwitcher activeApp="claude" />);
    fireEvent.click(await screen.findByRole("button", { name: "common.edit" }));
    fireEvent.change(await screen.findByLabelText("profiles.editor.name"), {
      target: { value: "Saved draft" },
    });
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(screen.queryByText("profiles.editor.title")).toBeNull(),
    );
    expect(await screen.findByText("Saved draft")).toBeInTheDocument();
    expect(screen.getByText("profiles.pendingSelection")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "common.edit" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "profiles.apply" }));
    await waitFor(() =>
      expect(mocks.apply).toHaveBeenCalledWith({
        id: "global",
        scope: "claude",
      }),
    );
  });

  it("allows explicitly reapplying the current global profile", async () => {
    profiles = [{ ...profile(), id: "global", name: "Global" }];
    currentIds.claude = "global";
    renderWithQueryClient(<ProfileSwitcher activeApp="claude" />);
    await screen.findByText("Global");
    fireEvent.click(
      screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
    );
    fireEvent.click(await screen.findByRole("option", { name: /Global/ }));
    await waitFor(() =>
      expect(mocks.apply).toHaveBeenCalledWith({
        id: "global",
        scope: "claude",
      }),
    );
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("preserves the project deployment scope while resolving a header conflict", async () => {
    profiles = [
      { ...profile("/projects/demo", "codex"), deployedScopes: ["codex"] },
    ];
    mocks.deploy.mockResolvedValueOnce({
      status: "needsResolution",
      conflicts: [{ path: "AGENTS.md", reason: "user instructions" }],
      warnings: [],
    });
    const { rerender } = renderWithQueryClient(
      <ProfileSwitcher activeApp="codex" />,
    );
    await screen.findByRole("button", { name: "profiles.context.more" });
    await chooseProject("/projects/demo");
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.projectDeploy.button" }),
    );
    await screen.findByText("AGENTS.md");
    rerender(<ProfileSwitcher activeApp="claude" />);
    fireEvent.click(
      screen.getByRole("button", {
        name: "profiles.projectDeploy.keepCurrent",
      }),
    );
    await waitFor(() =>
      expect(mocks.deploy).toHaveBeenLastCalledWith({
        id: "demo",
        scope: "codex",
        resolution: "keepCurrent",
      }),
    );
    expect(mocks.apply).not.toHaveBeenCalled();
  });

  it("only offers global profiles for global application and keeps project management reachable", async () => {
    const user = userEvent.setup();
    profiles = [
      profile("/projects/demo"),
      { ...profile(), id: "global", name: "Global" },
    ];
    renderWithQueryClient(<ProfileSwitcher activeApp="codex" />);
    await user.click(
      screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
    );
    expect(
      await screen.findByRole("option", { name: /Global/ }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Demo/ })).toBeNull();
    await user.click(screen.getByRole("option", { name: /Global/ }));
    await waitFor(() =>
      expect(mocks.apply).toHaveBeenCalledWith({
        id: "global",
        scope: "codex",
      }),
    );
    expect(mocks.apply).toHaveBeenCalledTimes(1);
    await waitFor(() =>
      expect(
        screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
      ).toHaveAttribute("aria-expanded", "false"),
    );
    // The shared popover blocks trigger clicks during its 200 ms animation.
    await act(() => new Promise((resolve) => setTimeout(resolve, 220)));
    await user.click(
      screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
    );
    await user.click(
      await screen.findByRole("option", { name: "profiles.manage" }),
    );
    expect(
      await within(screen.getByRole("dialog")).findByText("/projects/demo"),
    ).toBeInTheDocument();
  });

  it("keeps project management available when there are no global profiles", async () => {
    profiles = [profile("/projects/demo")];
    renderWithQueryClient(<ProfileSwitcher activeApp="claude" />);
    fireEvent.click(
      screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
    );
    fireEvent.click(
      await screen.findByRole("option", { name: "profiles.manage" }),
    );
    expect(
      await within(screen.getByRole("dialog")).findByText("/projects/demo"),
    ).toBeInTheDocument();
    expect(mocks.apply).not.toHaveBeenCalled();
  });
});

describe("persistent project viewing context", () => {
  it("remembers each Agent selection across reopening without applying configuration", async () => {
    profiles = [
      {
        ...profile("/projects/claude"),
        id: "claude-project",
        name: "Claude project",
      },
      {
        ...profile("/projects/codex", "codex"),
        id: "codex-project",
        name: "Codex project",
      },
    ];
    const first = renderWithQueryClient(
      <ProfileSwitcher activeApp="claude" projectOnly />,
    );
    await screen.findByRole("button", { name: "profiles.context.more" });
    await chooseProject("/projects/claude");
    fireEvent.click(
      screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
    );
    fireEvent.click(
      await screen.findByRole("option", { name: /Claude project/ }),
    );
    first.unmount();
    const second = renderWithQueryClient(
      <ProfileSwitcher activeApp="codex" projectOnly />,
    );
    await screen.findByRole("button", { name: "profiles.context.more" });
    expect(screen.getByLabelText("profiles.projectContext")).toHaveAttribute(
      "data-project-path",
      "",
    );
    await chooseProject("/projects/codex");
    fireEvent.click(
      screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
    );
    fireEvent.click(
      await screen.findByRole("option", { name: /Codex project/ }),
    );
    second.unmount();
    renderWithQueryClient(<ProfileSwitcher activeApp="claude" projectOnly />);
    await screen.findByText("Claude project");
    expect(screen.getByLabelText("profiles.projectContext")).toHaveAttribute(
      "data-project-path",
      "/projects/claude",
    );
    expect(
      JSON.parse(
        localStorage.getItem("cc-switch:profiles:project-selection:v1")!,
      ),
    ).toEqual({
      claude: { path: "/projects/claude", profileId: "claude-project" },
      codex: { path: "/projects/codex", profileId: "codex-project" },
    });
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("prioritizes explicit project context over the saved viewing preference", async () => {
    profiles = [
      { ...profile("/projects/saved"), id: "saved", name: "Saved" },
      { ...profile("/projects/explicit"), id: "explicit", name: "Explicit" },
    ];
    localStorage.setItem(
      "cc-switch:profiles:project-selection:v1",
      JSON.stringify({
        claude: { path: "/projects/saved", profileId: "saved" },
      }),
    );
    renderWithQueryClient(
      <ProfileSwitcher
        activeApp="claude"
        projectOnly
        initialProjectPath="/projects/explicit"
        initialProfileId="explicit"
      />,
    );
    await screen.findByText("Explicit");
    expect(screen.getByLabelText("profiles.projectContext")).toHaveAttribute(
      "data-project-path",
      "/projects/explicit",
    );
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("discards unavailable project and Profile selections without falling into global application", async () => {
    profiles = [{ ...profile("/projects/demo"), deployedScopes: ["codex"] }];
    localStorage.setItem(
      "cc-switch:profiles:project-selection:v1",
      JSON.stringify({
        claude: { path: "/projects/removed", profileId: "removed" },
        codex: { path: "/projects/demo", profileId: "deleted-profile" },
      }),
    );
    const { rerender } = renderWithQueryClient(
      <ProfileSwitcher activeApp="claude" projectOnly />,
    );
    await screen.findByRole("button", { name: "profiles.context.more" });
    expect(screen.getByLabelText("profiles.projectContext")).toHaveAttribute(
      "data-project-path",
      "",
    );
    rerender(<ProfileSwitcher activeApp="codex" projectOnly />);
    await screen.findByText("Demo");
    expect(screen.getByLabelText("profiles.projectContext")).toHaveAttribute(
      "data-project-path",
      "/projects/demo",
    );
    await waitFor(() =>
      expect(
        JSON.parse(
          localStorage.getItem("cc-switch:profiles:project-selection:v1")!,
        ),
      ).toEqual({
        claude: { path: "" },
        codex: { path: "/projects/demo" },
      }),
    );
    expect(mocks.apply).not.toHaveBeenCalled();
  });
});

describe("legacy Profile migration status", () => {
  it("preserves usable Profile actions after migration failure and permits retry", async () => {
    profiles = [{ ...profile("/projects/demo"), deployedScopes: ["claude"] }];
    mocks.migrate.mockRejectedValueOnce(
      new Error("Legacy data could not be read"),
    );
    renderWithQueryClient(
      <ProfileSwitcher
        activeApp="claude"
        initialProjectPath="/projects/demo"
      />,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Legacy data could not be read",
    );
    expect(screen.getByRole("button", { name: "common.edit" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "common.retry" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(mocks.migrate).toHaveBeenCalledTimes(2);
    expect(mocks.migrate).toHaveBeenCalledWith({ scope: "claude" });
    expect(mocks.apply).not.toHaveBeenCalled();
  });
});

describe("Profile context layout", () => {
  it("searches full paths and distinguishes projects with the same folder name", async () => {
    const firstPath = "C:/Users/Developer/projects/team-a/demo";
    const secondPath = "C:/Users/Developer/projects/team-b/demo";
    profiles = [
      { ...profile(firstPath), id: "a", name: "Team A" },
      { ...profile(secondPath), id: "b", name: "Team B" },
    ];
    renderWithQueryClient(<ProfileSwitcher activeApp="claude" />);
    await screen.findByRole("button", { name: "profiles.context.more" });
    await userEvent.click(
      screen.getByRole("combobox", { name: "profiles.projectContext" }),
    );
    expect(
      await screen.findByRole("option", { name: firstPath }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("option", { name: secondPath }),
    ).toBeInTheDocument();
    await userEvent.type(
      screen.getByPlaceholderText("profiles.context.searchProjects"),
      "team-b",
    );
    expect(screen.queryByRole("option", { name: firstPath })).toBeNull();
    await userEvent.click(screen.getByRole("option", { name: secondPath }));
    const picker = screen.getByRole("combobox", {
      name: "profiles.projectContext",
    });
    expect(picker).toHaveTextContent("demo");
    expect(picker).toHaveAttribute("title", secondPath);
    const context = screen.getByRole("region", {
      name: "profiles.context.title",
    });
    expect(within(context).getByText(secondPath)).toHaveAttribute(
      "title",
      secondPath,
    );
    expect(
      within(context).getByText("profiles.context.globalProvidersHint"),
    ).toBeInTheDocument();
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("keeps Profile actions separate from Agent management", async () => {
    profiles = [{ ...profile("/projects/demo"), deployedScopes: ["claude"] }];
    renderWithQueryClient(
      <ProfileSwitcher
        activeApp="claude"
        initialProjectPath="/projects/demo"
      />,
    );
    await screen.findByText("Demo");
    expect(
      screen.getByRole("button", { name: "common.edit" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "profiles.projectDeploy.button" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", {
        name: "profiles.environment.manageAgent",
      }),
    ).toBeNull();
    await openContextActions();
    expect(
      screen.queryByRole("menuitem", {
        name: "profiles.environment.manageAgent",
      }),
    ).toBeNull();
    expect(
      screen.getByRole("menuitem", { name: "profiles.manage" }),
    ).toBeInTheDocument();
  });
});

describe("profile folder management", () => {
  it("opens existing project management from an unavailable Agent while filtering global profiles", async () => {
    profiles = [
      { ...profile(), id: "global", name: "Global" },
      profile("/projects/demo"),
    ];
    renderWithQueryClient(
      <ProfileSwitcher activeApp="claude" projectOnly deploymentDisabled />,
    );
    await screen.findByRole("button", { name: "profiles.context.more" });
    fireEvent.click(
      screen.getByRole("combobox", { name: "profiles.switcherAriaLabel" }),
    );
    fireEvent.click(
      await screen.findByRole("option", { name: "profiles.manage" }),
    );
    const manager = await screen.findByRole("dialog", {
      name: "profiles.manageTitle",
    });
    expect(within(manager).getByText("Demo")).toBeInTheDocument();
    expect(within(manager).queryByText("Global")).toBeNull();
    expect(
      within(manager).getByRole("button", {
        name: "profiles.projectDeploy.button",
      }),
    ).toBeDisabled();
    await openProfileActions(manager);
    for (const name of [
      "profiles.copyToProject",
      "profiles.projectPath.change",
      "profiles.projectPath.unbind",
      "profiles.delete",
    ]) {
      expect(screen.getByRole("menuitem", { name })).not.toHaveAttribute(
        "data-disabled",
      );
    }
    fireEvent.click(
      screen.getByRole("menuitem", { name: "profiles.projectPath.unbind" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "common.confirm" }),
    );
    await waitFor(() =>
      expect(mocks.setProjectPath).toHaveBeenCalledWith({
        id: "demo",
        projectPath: null,
      }),
    );
    await waitFor(() => expect(within(manager).queryByText("Demo")).toBeNull());
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("disables management conflict writes when the Agent stops being ready and retains cancel", async () => {
    profiles = [profile("/projects/demo")];
    mocks.deploy
      .mockResolvedValueOnce({
        status: "needsResolution",
        conflicts: [{ path: "CLAUDE.md", reason: "User edit" }],
        warnings: [],
      })
      .mockResolvedValueOnce({ status: "skipped" });
    const { rerender } = renderWithQueryClient(
      <ProfileManageDialog
        isOpen
        projectOnly
        scope="claude"
        onClose={vi.fn()}
      />,
    );
    fireEvent.click(
      await screen.findByRole("button", {
        name: "profiles.projectDeploy.button",
      }),
    );
    await screen.findByText("CLAUDE.md");
    rerender(
      <ProfileManageDialog
        isOpen
        projectOnly
        scope="claude"
        deploymentDisabled
        onClose={vi.fn()}
      />,
    );
    expect(
      screen.getByRole("button", { name: "profiles.projectDeploy.overwrite" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", {
        name: "profiles.projectDeploy.keepCurrent",
      }),
    ).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.projectDeploy.skip" }),
    );
    await waitFor(() =>
      expect(mocks.deploy).toHaveBeenLastCalledWith({
        id: "demo",
        scope: "claude",
        resolution: "skip",
      }),
    );
  });

  it.each(["modelSettings", "modelCatalog"] as const)(
    "recognizes a %s-only snapshot for project deployment",
    (field) => {
      const item = profile("/projects/demo");
      item.payload.mcp.claude = null;
      item.payload.skills.claude = null;
      item.payload[field] = { claude: {}, "claude-desktop": null, codex: null };
      expect(hasScopeSnapshot(item, "claude")).toBe(true);
      expect(hasScopeSnapshot(item, "codex")).toBe(false);
    },
  );
  it("shows deployment markers for the current agent's profile within a shared folder", async () => {
    profiles = [
      {
        ...profile("/projects/demo"),
        id: "claude-profile",
        name: "Claude profile",
        deployedScopes: ["claude"],
      },
      {
        ...profile("/projects/demo", "codex"),
        id: "codex-profile",
        name: "Codex profile",
        deployedScopes: ["codex"],
      },
    ];
    currentIds.claude = "codex-profile";
    const { rerender } = renderWithQueryClient(
      <ProfileManageDialog isOpen scope="claude" onClose={vi.fn()} />,
    );
    const claudeName = await screen.findByText("Claude profile");
    expect(
      within(claudeName.parentElement!).getByRole("img", {
        name: "profiles.projectDeploy.current",
      }),
    ).toBeInTheDocument();
    expect(
      within(screen.getByText("Codex profile").parentElement!).queryByRole(
        "img",
      ),
    ).toBeNull();
    rerender(<ProfileManageDialog isOpen scope="codex" onClose={vi.fn()} />);
    expect(
      within(screen.getByText("Claude profile").parentElement!).queryByRole(
        "img",
      ),
    ).toBeNull();
    expect(
      within(screen.getByText("Codex profile").parentElement!).getByRole(
        "img",
        { name: "profiles.projectDeploy.current" },
      ),
    ).toBeInTheDocument();
  });

  it("refreshes deployment markers from the profile list after a successful deployment", async () => {
    profiles = [profile("/projects/demo")];
    mocks.deploy.mockImplementationOnce(
      ({ id, scope }: { id: string; scope: "claude" }) => {
        profiles = profiles.map((item) =>
          item.id === id ? { ...item, deployedScopes: [scope] } : item,
        );
        return Promise.resolve({
          status: "applied",
          changed: [],
          warnings: [],
        });
      },
    );
    renderWithQueryClient(
      <ProfileManageDialog isOpen scope="claude" onClose={vi.fn()} />,
    );
    const deployButton = await screen.findByRole("button", {
      name: "profiles.projectDeploy.button",
    });
    expect(
      screen.queryByRole("img", { name: "profiles.projectDeploy.current" }),
    ).toBeNull();
    const readsBefore = mocks.invoke.mock.calls.filter(
      ([command]) => command === "list_profiles",
    ).length;
    fireEvent.click(deployButton);
    expect(
      await screen.findByRole("img", {
        name: "profiles.projectDeploy.current",
      }),
    ).toBeInTheDocument();
    expect(
      mocks.invoke.mock.calls.filter(([command]) => command === "list_profiles")
        .length,
    ).toBeGreaterThan(readsBefore);
  });

  it("keeps deployment disabled until the selected agent has a snapshot", async () => {
    profiles = [profile("/projects/demo")];
    renderWithQueryClient(
      <ProfileManageDialog isOpen scope="codex" onClose={vi.fn()} />,
    );
    const deployButton = await screen.findByRole("button", {
      name: "profiles.projectDeploy.button",
    });
    expect(deployButton).toBeDisabled();
    await openProfileActions();
    expect(
      screen.getByRole("menuitem", { name: "profiles.captureCurrent" }),
    ).not.toHaveAttribute("data-disabled");
    fireEvent.click(deployButton);
    expect(mocks.deploy).not.toHaveBeenCalled();
  });

  it("updates a project snapshot only after confirmation and keeps the chosen agent scope", async () => {
    profiles = [profile("/projects/demo")];
    let finish!: (value: Profile) => void;
    mocks.update.mockReturnValueOnce(
      new Promise<Profile>((resolve) => {
        finish = resolve;
      }),
    );
    const { rerender } = renderWithQueryClient(
      <ProfileManageDialog isOpen scope="codex" onClose={vi.fn()} />,
    );
    await openProfileActions();
    fireEvent.click(
      screen.getByRole("menuitem", { name: "profiles.captureCurrent" }),
    );
    expect(mocks.update).not.toHaveBeenCalled();
    expect(
      screen.getByText("profiles.captureCurrentDescription"),
    ).toBeInTheDocument();
    rerender(<ProfileManageDialog isOpen scope="claude" onClose={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "common.confirm" }));
    await waitFor(() =>
      expect(mocks.update).toHaveBeenCalledWith({
        id: "demo",
        name: undefined,
        resnapshot: true,
        scope: "codex",
      }),
    );
    expect(
      screen.getByRole("button", { name: "common.cancel" }),
    ).toBeDisabled();
    await act(async () => finish(profile("/projects/demo")));
    await waitFor(() =>
      expect(
        screen.queryByText("profiles.captureCurrentDescription"),
      ).toBeNull(),
    );
    expect(mocks.deploy).not.toHaveBeenCalled();
    expect(mocks.apply).not.toHaveBeenCalled();
  });

  it.each(["claude", "claude-desktop"] as const)(
    "does not offer project snapshot capture for a global %s profile",
    async (scope) => {
      profiles = [profile()];
      renderWithQueryClient(
        <ProfileManageDialog isOpen scope={scope} onClose={vi.fn()} />,
      );
      expect(await screen.findByText("Demo")).toBeInTheDocument();
      await openProfileActions();
      expect(
        screen.queryByRole("menuitem", { name: "profiles.captureCurrent" }),
      ).toBeNull();
    },
  );

  it("copies the selected profile to an independent record for another folder", async () => {
    profiles = [{ ...profile("/projects/source"), deployedScopes: ["claude"] }];
    renderWithQueryClient(
      <ProfileManageDialog isOpen scope="claude" onClose={vi.fn()} />,
    );
    await openProfileActions();
    fireEvent.click(
      screen.getByRole("menuitem", { name: "profiles.copyToProject" }),
    );
    fireEvent.change(screen.getByLabelText("profiles.namePlaceholder"), {
      target: { value: " Demo copy " },
    });
    expect(
      screen.getByRole("button", { name: "profiles.copyToProject" }),
    ).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.projectPath.pick" }),
    );
    await waitFor(() =>
      expect(screen.getByLabelText("profiles.projectPath.label")).toHaveValue(
        "/projects/demo",
      ),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.copyToProject" }),
    );
    await waitFor(() =>
      expect(mocks.duplicate).toHaveBeenCalledWith({
        sourceId: "demo",
        name: "Demo copy",
        projectPath: "/projects/demo",
      }),
    );
    expect(await screen.findByText("Demo copy")).toBeInTheDocument();
    expect(screen.getByText("/projects/source")).toBeInTheDocument();
    expect(
      within(screen.getByText("Demo copy").parentElement!).queryByRole("img"),
    ).toBeNull();
    expect(
      screen.getAllByRole("img", { name: "profiles.projectDeploy.current" }),
    ).toHaveLength(1);
    expect(mocks.create).not.toHaveBeenCalled();
    expect(mocks.setProjectPath).not.toHaveBeenCalled();
  });

  it("does not offer project deployment for Claude Desktop", async () => {
    profiles = [profile("/projects/demo")];
    renderWithQueryClient(
      <ProfileManageDialog isOpen scope="claude-desktop" onClose={vi.fn()} />,
    );
    expect(await screen.findByText("/projects/demo")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "profiles.projectDeploy.button" }),
    ).toBeNull();
    await openProfileActions();
    expect(
      screen.queryByRole("menuitem", { name: "profiles.captureCurrent" }),
    ).toBeNull();
  });

  it("binds a folder and unbinds it only after confirmation", async () => {
    profiles = [profile()];
    renderWithQueryClient(
      <ProfileManageDialog isOpen scope="claude" onClose={vi.fn()} />,
    );
    await openProfileActions();
    fireEvent.click(
      screen.getByRole("menuitem", { name: "profiles.projectPath.pick" }),
    );
    expect(await screen.findByText("/projects/demo")).toBeInTheDocument();
    expect(mocks.setProjectPath).toHaveBeenLastCalledWith({
      id: "demo",
      projectPath: "/projects/demo",
    });

    await openProfileActions();
    fireEvent.click(
      screen.getByRole("menuitem", { name: "profiles.projectPath.unbind" }),
    );
    expect(mocks.setProjectPath).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "common.confirm" }));
    await waitFor(() =>
      expect(mocks.setProjectPath).toHaveBeenLastCalledWith({
        id: "demo",
        projectPath: null,
      }),
    );
    await waitFor(() =>
      expect(screen.queryByText("/projects/demo")).not.toBeInTheDocument(),
    );
  });

  it.each(["keepCurrent", "overwrite", "skip"] as const)(
    "asks how to handle conflicts before %s deployment",
    async (resolution) => {
      profiles = [profile("/projects/demo")];
      mocks.deploy
        .mockResolvedValueOnce({
          status: "needsResolution",
          conflicts: [
            { path: ".mcp.json#mcp_servers.fs", reason: "edited by user" },
          ],
          warnings: [],
        })
        .mockResolvedValueOnce(
          resolution === "skip"
            ? { status: "skipped" }
            : {
                status: "applied",
                changed: [".mcp.json"],
                warnings: [],
              },
        );
      renderWithQueryClient(
        <ProfileManageDialog isOpen scope="claude" onClose={vi.fn()} />,
      );

      fireEvent.click(
        await screen.findByRole("button", {
          name: "profiles.projectDeploy.button",
        }),
      );
      expect(
        await screen.findByText(".mcp.json#mcp_servers.fs"),
      ).toBeInTheDocument();
      expect(mocks.deploy).toHaveBeenLastCalledWith({
        id: "demo",
        scope: "claude",
        resolution: null,
      });

      fireEvent.click(
        screen.getByRole("button", {
          name: `profiles.projectDeploy.${resolution}`,
        }),
      );
      await waitFor(() =>
        expect(mocks.deploy).toHaveBeenLastCalledWith({
          id: "demo",
          scope: "claude",
          resolution,
        }),
      );
      await waitFor(() =>
        expect(
          resolution === "skip" ? mocks.info : mocks.success,
        ).toHaveBeenCalledWith(
          resolution === "skip"
            ? "profiles.projectDeploy.skipped"
            : "profiles.projectDeploy.success",
        ),
      );
      if (resolution === "skip") expect(mocks.success).not.toHaveBeenCalled();
    },
  );

  it("retains the selected agent scope while resolving a deployment conflict", async () => {
    profiles = [profile("/projects/demo", "codex")];
    mocks.deploy
      .mockResolvedValueOnce({
        status: "needsResolution",
        conflicts: [{ path: ".codex/config.toml", reason: "edited" }],
        warnings: [],
      })
      .mockResolvedValueOnce({
        status: "applied",
        changed: [],
        warnings: ["sensitiveMcpSkipped:private"],
      });
    const { rerender } = renderWithQueryClient(
      <ProfileManageDialog isOpen scope="codex" onClose={vi.fn()} />,
    );
    fireEvent.click(
      await screen.findByRole("button", {
        name: "profiles.projectDeploy.button",
      }),
    );
    expect(await screen.findByText(".codex/config.toml")).toBeInTheDocument();
    rerender(<ProfileManageDialog isOpen scope="claude" onClose={vi.fn()} />);
    fireEvent.click(
      screen.getByRole("button", {
        name: "profiles.projectDeploy.keepCurrent",
      }),
    );
    await waitFor(() =>
      expect(mocks.deploy).toHaveBeenLastCalledWith({
        id: "demo",
        scope: "codex",
        resolution: "keepCurrent",
      }),
    );
    await waitFor(() =>
      expect(mocks.info).toHaveBeenCalledWith(
        "profiles.projectDeploy.codexTrustWarning",
        { duration: 7000 },
      ),
    );
    expect(mocks.warning).toHaveBeenCalledWith(
      "profiles.projectDeploy.completedWithNotes",
      {
        description: "profiles.projectDeploy.sensitiveMcpSkipped",
        duration: 10000,
        closeButton: true,
      },
    );
    expect(mocks.warning).toHaveBeenCalledTimes(1);
    expect(mocks.success).not.toHaveBeenCalled();
  });

  it("shows skipped-resource reasons before confirming conflicts and in the deployment result", async () => {
    profiles = [profile("/projects/demo")];
    const warning =
      "MCP 'Project docs' (docs) was deleted; project content is preserved";
    mocks.deploy
      .mockResolvedValueOnce({
        status: "needsResolution",
        conflicts: [{ path: ".mcp.json", reason: "edited by user" }],
        warnings: [warning, "sensitiveMcpSkipped:private"],
      })
      .mockResolvedValueOnce({
        status: "applied",
        changed: [],
        warnings: [warning],
      });
    renderWithQueryClient(
      <ProfileManageDialog isOpen scope="claude" onClose={vi.fn()} />,
    );
    fireEvent.click(
      await screen.findByRole("button", {
        name: "profiles.projectDeploy.button",
      }),
    );
    const notes = await screen.findByRole("status");
    expect(within(notes).getByText(warning)).toBeInTheDocument();
    expect(
      within(notes).getByText("profiles.projectDeploy.sensitiveMcpSkipped"),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("sensitiveMcpSkipped:private"),
    ).not.toBeInTheDocument();
    expect(mocks.deploy).toHaveBeenCalledTimes(1);
    fireEvent.click(
      screen.getByRole("button", {
        name: "profiles.projectDeploy.keepCurrent",
      }),
    );
    await waitFor(() =>
      expect(mocks.warning).toHaveBeenCalledWith(
        "profiles.projectDeploy.completedWithNotes",
        { description: warning, duration: 10000, closeButton: true },
      ),
    );
    expect(mocks.success).not.toHaveBeenCalled();
  });

  it.each(["create", "manage"])(
    "reports picker errors in the %s dialog without changing the folder",
    async (dialog) => {
      mocks.pickDirectory.mockRejectedValueOnce(
        new Error("dialog unavailable"),
      );
      if (dialog === "create") {
        await openCreate();
        fireEvent.click(
          screen.getByRole("radio", { name: "profiles.scope.project" }),
        );
      } else {
        profiles = [profile()];
        renderWithQueryClient(
          <ProfileManageDialog isOpen scope="claude" onClose={vi.fn()} />,
        );
        await openProfileActions();
      }
      fireEvent.click(
        await screen.findByRole(dialog === "create" ? "button" : "menuitem", {
          name: "profiles.projectPath.pick",
        }),
      );
      await waitFor(() =>
        expect(mocks.error).toHaveBeenCalledWith(
          "profiles.projectPath.pickFailed",
        ),
      );
      expect(mocks.setProjectPath).not.toHaveBeenCalled();
      expect(mocks.create).not.toHaveBeenCalled();
      if (dialog === "create") {
        expect(
          screen.getByPlaceholderText("profiles.namePlaceholder"),
        ).toBeInTheDocument();
      } else {
        expect(
          screen.getByText("profiles.projectPath.none"),
        ).toBeInTheDocument();
      }
    },
  );
});

describe("direct profile configuration editing", () => {
  it("reports invalid numeric settings after saving from another section and keeps resource choices for retry", async () => {
    const item = profile("/projects/demo", "codex");
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
    const item = profile("/projects/demo", "codex");
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
      <ProfileEditPanel
        profile={profile("/projects/demo")}
        scope="claude"
        onClose={onClose}
      />,
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
      <ProfileEditPanel
        profile={profile("/projects/demo")}
        scope="claude"
        onClose={onClose}
      />,
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
        ...profile("/projects/demo"),
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
        ...profile("/projects/demo"),
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
    renderWithQueryClient(
      <ProfileManageDialog isOpen scope="claude" onClose={vi.fn()} />,
    );
    fireEvent.click(
      (await screen.findAllByRole("button", { name: "common.edit" }))[0],
    );
    expect(
      await screen.findByLabelText("profiles.editor.model", {
        selector: "input",
      }),
    ).toHaveValue("independent-a");
    fireEvent.change(screen.getByLabelText("profiles.editor.provider"), {
      target: { value: "provider-b" },
    });
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
    profiles = [profile("/projects/demo", "codex")];
    const { rerender } = renderWithQueryClient(
      <ProfileManageDialog isOpen scope="codex" onClose={vi.fn()} />,
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
    rerender(<ProfileManageDialog isOpen scope="claude" onClose={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(mocks.saveConfig).toHaveBeenCalledWith({
        id: "demo",
        scope: "codex",
        name: "Demo",
        config: {
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
    profiles = [profile("/projects/demo")];
    let finish!: (value: Profile) => void;
    mocks.saveConfig.mockRejectedValueOnce(new Error("save failed"));
    renderWithQueryClient(
      <ProfileManageDialog isOpen scope="claude" onClose={vi.fn()} />,
    );
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
    const item = profile("/projects/demo");
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
    const item = profile("/projects/demo");
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
    const item = profile("/projects/demo");
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
    const item = profile("/projects/demo");
    item.payload.mcp.claude = null;
    item.payload.skills.claude = null;
    item.payload.models = {
      claude: null,
      "claude-desktop": null,
      codex: "stored-model",
    };
    profiles = [item];
    renderWithQueryClient(
      <ProfileManageDialog isOpen scope="codex" onClose={vi.fn()} />,
    );
    expect(
      await screen.findByRole("button", {
        name: "profiles.projectDeploy.button",
      }),
    ).toBeEnabled();
  });
});
