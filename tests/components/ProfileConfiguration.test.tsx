import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ProfileEditPanel } from "@/components/profiles/ProfileEditPanel";
import { hasScopeSnapshot } from "@/components/profiles/scope";
import type { Profile, ProfileScope } from "@/lib/api/profiles";
import { renderWithQueryClient } from "../utils/testQueryClient";

const mocks = vi.hoisted(() => ({ save: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@/lib/api", () => ({
  providersApi: {
    getAll: async () => ({
      first: { id: "first", name: "First", settingsConfig: {} },
      second: {
        id: "second",
        name: "Second",
        settingsConfig: { env: { ANTHROPIC_MODEL: "provider-model" } },
      },
    }),
  },
  promptsApi: { getPrompts: async () => ({}) },
}));
vi.mock("@/hooks/useMcp", () => ({
  useAllMcpServers: () => ({ data: {}, isPending: false }),
}));
vi.mock("@/hooks/useSkills", () => ({
  useInstalledSkills: () => ({ data: [], isPending: false }),
}));
vi.mock("@/lib/query/profiles", () => ({
  useSaveProfileConfigMutation: () => ({
    mutateAsync: mocks.save,
    isPending: false,
  }),
}));
vi.mock("@/components/JsonEditor", () => ({
  default: ({
    value,
    onChange,
    ariaLabel,
  }: {
    value: string;
    onChange: (value: string) => void;
    ariaLabel: string;
  }) => (
    <textarea
      aria-label={ariaLabel}
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));

function profile(scope: ProfileScope, nativeConfig: string): Profile {
  const empty = { claude: null, codex: null, "claude-desktop": null };
  return {
    id: "profile",
    name: "My configuration",
    payload: {
      providers: { ...empty, [scope]: "first" },
      models: {
        ...empty,
        [scope]: scope === "claude-desktop" ? "" : "profile-model",
      },
      modelSettings: { ...empty, [scope]: {} },
      nativeConfig: { ...empty, [scope]: nativeConfig },
      mcp: { ...empty, [scope]: [] },
      skills: { ...empty, [scope]: [] },
      prompts: empty,
    },
  };
}

beforeEach(() => {
  mocks.save.mockReset().mockResolvedValue({});
  mocks.invoke.mockReset().mockResolvedValue({});
});

describe("Profile owns Agent configuration", () => {
  it("inherits the global connection by default and saves only the project connection choice", async () => {
    renderWithQueryClient(
      <ProfileEditPanel
        profile={profile("claude", "{}")}
        scope="claude"
        onClose={vi.fn()}
      />,
    );
    const connection = await screen.findByLabelText(
      "profiles.editor.projectConnection",
    );
    expect(connection).toHaveValue("inherit");
    fireEvent.change(connection, { target: { value: "direct" } });
    expect(
      screen.getByText("profiles.editor.projectConnectionCredentialsHint"),
    ).toBeVisible();
    fireEvent.change(screen.getByLabelText("profiles.editor.provider"), {
      target: { value: "second" },
    });
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() => expect(mocks.save).toHaveBeenCalledOnce());
    expect(mocks.save.mock.calls[0][0].config).toMatchObject({
      claudeProjectConnection: "direct",
      providerId: "second",
      model: "profile-model",
      nativeConfig: "{}",
    });
    expect(
      mocks.invoke.mock.calls.every(
        ([command]) => command === "get_profile_connection_status",
      ),
    ).toBe(true);
  });

  it("requires an existing provider for direct access and permits returning to inheritance", async () => {
    const snapshot = profile("claude", "{}");
    snapshot.payload.claudeProjectConnection = "direct";
    snapshot.payload.providers.claude = "removed-provider";
    renderWithQueryClient(
      <ProfileEditPanel profile={snapshot} scope="claude" onClose={vi.fn()} />,
    );
    const connection = await screen.findByLabelText(
      "profiles.editor.projectConnection",
    );
    expect(connection).toHaveValue("direct");
    expect(screen.getByRole("button", { name: "common.save" })).toBeDisabled();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "profiles.editor.projectConnectionProviderRequired",
    );
    fireEvent.change(screen.getByLabelText("profiles.editor.provider"), {
      target: { value: "" },
    });
    expect(screen.getByRole("button", { name: "common.save" })).toBeDisabled();
    fireEvent.click(
      screen.getByRole("tab", { name: "profiles.editor.behaviorTab" }),
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "profiles.editor.projectConnectionProviderRequired",
    );
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.editor.modelsTab" }),
    );
    fireEvent.change(connection, { target: { value: "inherit" } });
    expect(screen.queryByRole("alert")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() => expect(mocks.save).toHaveBeenCalledOnce());
    expect(mocks.save.mock.calls[0][0].config).toMatchObject({
      claudeProjectConnection: "inherit",
      providerId: null,
    });
  });

  it.each(["codex", "claude-desktop"] as const)(
    "does not expose or overwrite Claude project connection when editing %s",
    async (scope) => {
      const snapshot = profile(scope, scope === "codex" ? "" : "{}");
      snapshot.payload.claudeProjectConnection = "direct";
      renderWithQueryClient(
        <ProfileEditPanel profile={snapshot} scope={scope} onClose={vi.fn()} />,
      );
      await screen.findByLabelText("profiles.editor.provider");
      expect(
        screen.queryByLabelText("profiles.editor.projectConnection"),
      ).toBeNull();
      fireEvent.click(screen.getByRole("button", { name: "common.save" }));
      await waitFor(() => expect(mocks.save).toHaveBeenCalledOnce());
      expect(mocks.save.mock.calls[0][0].config).not.toHaveProperty(
        "claudeProjectConnection",
      );
    },
  );

  it("preserves models, cleared overrides and native behavior when changing only the provider reference", async () => {
    const native = JSON.stringify({
      permissions: { allow: ["Read"] },
      hooks: {},
      customFutureOption: true,
    });
    renderWithQueryClient(
      <ProfileEditPanel
        profile={profile("claude", native)}
        scope="claude"
        onClose={vi.fn()}
      />,
    );
    fireEvent.change(await screen.findByLabelText("profiles.editor.provider"), {
      target: { value: "second" },
    });
    expect(
      screen.getByLabelText("profiles.editor.model", { selector: "input" }),
    ).toHaveValue("profile-model");
    fireEvent.click(
      screen.getByRole("tab", { name: "profiles.editor.behaviorTab" }),
    );
    expect(
      screen.getByRole("textbox", { name: "provider.configJson" }),
    ).toHaveValue(native);
    fireEvent.click(
      screen.getByRole("checkbox", { name: "claudeConfig.hideAttribution" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() => expect(mocks.save).toHaveBeenCalledOnce());
    const saved = mocks.save.mock.calls[0][0].config;
    expect(saved.providerId).toBe("second");
    expect(saved.model).toBe("profile-model");
    expect(saved.modelSettings).toEqual({});
    expect(JSON.parse(saved.nativeConfig)).toEqual({
      permissions: { allow: ["Read"] },
      hooks: {},
      customFutureOption: true,
      attribution: { commit: "", pr: "", sessionUrl: false },
    });
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "apply_profile",
      expect.anything(),
    );
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "update_provider",
      expect.anything(),
    );
  });

  it("blocks invalid TOML even after leaving the behavior tab and retains the draft for repair", async () => {
    const close = vi.fn();
    renderWithQueryClient(
      <ProfileEditPanel
        profile={profile("codex", 'approval_policy = "on-request"')}
        scope="codex"
        onClose={close}
      />,
    );
    fireEvent.click(
      await screen.findByRole("tab", { name: "profiles.editor.behaviorTab" }),
    );
    const editor = screen.getByRole("textbox", {
      name: "codexConfig.configToml",
    });
    fireEvent.change(editor, { target: { value: "[broken" } });
    fireEvent.click(
      screen.getByRole("tab", { name: "profiles.editor.modelsTab" }),
    );
    expect(screen.getByRole("button", { name: "common.save" })).toBeDisabled();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "profiles.editor.nativeInvalid",
    );
    expect(mocks.save).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "profiles.editor.behaviorTab" }),
    );
    expect(editor).toHaveValue("[broken");
    expect(
      screen.queryByRole("checkbox", { name: "codexConfig.contextWindow1M" }),
    ).toBeNull();
    fireEvent.change(editor, {
      target: {
        value: 'approval_policy = "never"\n[features]\ncustom_feature = true\n',
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() => expect(mocks.save).toHaveBeenCalledOnce());
    expect(mocks.save.mock.calls[0][0].config.nativeConfig).toContain(
      "custom_feature = true",
    );
    expect(close).toHaveBeenCalledOnce();
  });

  it("exposes Desktop global policy without Claude CLI controls and validates a JSON object", async () => {
    renderWithQueryClient(
      <ProfileEditPanel
        profile={profile("claude-desktop", "{}")}
        scope="claude-desktop"
        onClose={vi.fn()}
      />,
    );
    fireEvent.click(
      await screen.findByRole("tab", { name: "profiles.editor.behaviorTab" }),
    );
    expect(
      screen.queryByRole("checkbox", { name: "claudeConfig.hideAttribution" }),
    ).toBeNull();
    fireEvent.change(
      screen.getByRole("textbox", { name: "provider.configJson" }),
      { target: { value: "[]" } },
    );
    expect(screen.getByRole("button", { name: "common.save" })).toBeDisabled();
    fireEvent.change(
      screen.getByRole("textbox", { name: "provider.configJson" }),
      { target: { value: '{"disableDeploymentModeChooser":false}' } },
    );
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() => expect(mocks.save).toHaveBeenCalledOnce());
    expect(mocks.save.mock.calls[0][0].config.nativeConfig).toContain(
      "disableDeploymentModeChooser",
    );
  });

  it("recognizes native-only snapshots without inventing another Agent snapshot", () => {
    const snapshot = profile("claude", "{}");
    snapshot.payload.providers.claude = null;
    snapshot.payload.models = undefined;
    snapshot.payload.modelSettings = undefined;
    snapshot.payload.mcp.claude = null;
    snapshot.payload.skills.claude = null;
    expect(hasScopeSnapshot(snapshot, "claude")).toBe(true);
    expect(hasScopeSnapshot(snapshot, "codex")).toBe(false);
  });
});
