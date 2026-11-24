import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ProfileSwitcher } from "@/components/profiles/ProfileSwitcher";
import { ProfileManageDialog } from "@/components/profiles/ProfileManageDialog";
import { profilesApi } from "@/lib/api/profiles";
import type { Profile } from "@/lib/api/profiles";
import { renderWithQueryClient } from "../utils/testQueryClient";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  create: vi.fn(),
  setProjectPath: vi.fn(),
  deploy: vi.fn(),
  pickDirectory: vi.fn(),
  updateTrayMenu: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@/lib/api", async () => ({
  profilesApi: (await import("@/lib/api/profiles")).profilesApi,
  settingsApi: { pickDirectory: mocks.pickDirectory },
  providersApi: { updateTrayMenu: mocks.updateTrayMenu },
}));
vi.mock("@/lib/toast", () => ({
  toast: { success: mocks.success, error: mocks.error },
}));

const profile = (projectPath: string | null = null): Profile => ({
  id: "demo",
  name: "Demo",
  projectPath,
  payload: {
    providers: { claude: null, "claude-desktop": null, codex: null },
    mcp: { claude: [], "claude-desktop": null, codex: null },
    skills: { claude: [], "claude-desktop": null, codex: null },
    prompts: { claude: null, "claude-desktop": null, codex: null },
  },
});

let profiles: Profile[];

beforeEach(() => {
  Object.values(mocks).forEach((mock) => mock.mockReset());
  profiles = [];
  mocks.updateTrayMenu.mockResolvedValue(true);
  mocks.pickDirectory.mockResolvedValue("/projects/demo");
  mocks.create.mockResolvedValue(profile());
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
    switch (command) {
      case "list_profiles":
        return Promise.resolve({
          profiles,
          currentIds: { claude: null, claudeDesktop: null, codex: null },
        });
      case "create_profile":
        return mocks.create(args);
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

async function openCreate() {
  renderWithQueryClient(<ProfileSwitcher activeApp="claude" />);
  fireEvent.click(screen.getByRole("combobox"));
  fireEvent.click(
    await screen.findByRole("option", { name: "profiles.createFromCurrent" }),
  );
  return screen.getByPlaceholderText("profiles.namePlaceholder");
}

describe("profile folder creation", () => {
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
      fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
      expect(screen.getByRole("dialog")).toBeInTheDocument();

      await act(async () => finish(profile(projectPath)));
      await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    },
  );

  it.each(["folder already bound", "folder no longer exists"])(
    "keeps form values after %s and allows a corrected retry",
    async (reason) => {
      mocks.create.mockRejectedValueOnce(new Error(reason));
      const name = await openCreate();
      fireEvent.change(name, { target: { value: "Demo" } });
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
        screen.getByRole("button", { name: "profiles.projectPath.clear" }),
      );
      fireEvent.click(screen.getByRole("button", { name: "common.confirm" }));
      await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
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

describe("profile folder management", () => {
  it("binds a folder and unbinds it only after confirmation", async () => {
    profiles = [profile()];
    renderWithQueryClient(
      <ProfileManageDialog isOpen scope="claude" onClose={vi.fn()} />,
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "profiles.projectPath.pick" }),
    );
    expect(await screen.findByText("/projects/demo")).toBeInTheDocument();
    expect(mocks.setProjectPath).toHaveBeenLastCalledWith({
      id: "demo",
      projectPath: "/projects/demo",
    });

    fireEvent.click(
      screen.getByRole("button", { name: "profiles.projectPath.unbind" }),
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

  it("asks how to handle project file conflicts before deployment", async () => {
    profiles = [profile("/projects/demo")];
    mocks.deploy
      .mockResolvedValueOnce({
        status: "needsResolution",
        conflicts: [
          { path: ".mcp.json#mcp_servers.fs", reason: "edited by user" },
        ],
        warnings: [],
      })
      .mockResolvedValueOnce({
        status: "applied",
        changed: [".mcp.json"],
        warnings: [],
      });
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
        name: "profiles.projectDeploy.keepCurrent",
      }),
    );
    await waitFor(() =>
      expect(mocks.deploy).toHaveBeenLastCalledWith({
        id: "demo",
        scope: "claude",
        resolution: "keepCurrent",
      }),
    );
    await waitFor(() =>
      expect(mocks.success).toHaveBeenCalledWith(
        "profiles.projectDeploy.success",
      ),
    );
  });

  it.each(["create", "manage"])(
    "reports picker errors in the %s dialog without changing the folder",
    async (dialog) => {
      mocks.pickDirectory.mockRejectedValueOnce(
        new Error("dialog unavailable"),
      );
      if (dialog === "create") {
        await openCreate();
      } else {
        profiles = [profile()];
        renderWithQueryClient(
          <ProfileManageDialog isOpen scope="claude" onClose={vi.fn()} />,
        );
      }
      fireEvent.click(
        await screen.findByRole("button", {
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
      expect(screen.getByRole("dialog")).toBeInTheDocument();
    },
  );
});
