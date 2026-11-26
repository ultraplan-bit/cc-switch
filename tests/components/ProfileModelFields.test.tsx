import { useState } from "react";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi, beforeEach } from "vitest";
import {
  ProfileModelFields,
  providerModelDefaults,
} from "@/components/profiles/ProfileModelFields";
import type { ProfileConfig } from "@/lib/api/profiles";
import type { Provider } from "@/types";
import { renderWithQueryClient } from "../utils/testQueryClient";

const mocks = vi.hoisted(() => ({
  fetch: vi.fn(),
  oauth: vi.fn(),
  status: vi.fn(),
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key, i18n: { language: "en" } }),
}));
vi.mock("@/lib/api/model-fetch", async (load) => ({
  ...(await load<typeof import("@/lib/api/model-fetch")>()),
  fetchModelsForConfig: mocks.fetch,
  fetchCodexOauthModels: mocks.oauth,
}));
vi.mock("@/lib/api/auth", async (load) => ({
  ...(await load<typeof import("@/lib/api/auth")>()),
  authGetStatus: mocks.status,
}));
vi.mock("@/hooks/useModelMetadataFill", () => ({
  useModelMetadataFill: () => () => {},
}));

function Harness({
  provider,
  scope,
  initial,
}: {
  provider: Provider;
  scope: "claude" | "codex";
  initial?: Partial<ProfileConfig>;
}) {
  const [config, setConfig] = useState<ProfileConfig>({
    providerId: provider.id,
    ...providerModelDefaults(provider, scope),
    mcpIds: ["kept-mcp"],
    skillIds: ["kept-skill"],
    promptId: "kept-prompt",
    ...initial,
  });
  return (
    <>
      <ProfileModelFields
        provider={provider}
        scope={scope}
        config={config}
        setConfig={setConfig}
      />
      <output data-testid="draft">{JSON.stringify(config)}</output>
    </>
  );
}
const draft = () => JSON.parse(screen.getByTestId("draft").textContent ?? "{}");

beforeEach(() => {
  mocks.fetch.mockReset().mockResolvedValue([]);
  mocks.oauth.mockReset().mockResolvedValue([]);
  mocks.status
    .mockReset()
    .mockResolvedValue({ authenticated: true, accounts: [] });
});

describe("Profile models reuse provider editing", () => {
  it("keeps an initially empty model input focused while typing and restoring its value", () => {
    const provider: Provider = {
      id: "empty",
      name: "Empty",
      settingsConfig: {},
    };
    renderWithQueryClient(<Harness scope="claude" provider={provider} />);
    const input = screen.getByLabelText("profiles.editor.model", {
      selector: "input",
    });
    input.focus();
    fireEvent.change(input, { target: { value: "temporary" } });
    expect(
      screen.getByLabelText("profiles.editor.model", { selector: "input" }),
    ).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(draft().model).toBe("temporary");
    fireEvent.change(input, { target: { value: "" } });
    expect(
      screen.getByLabelText("profiles.editor.model", { selector: "input" }),
    ).toBe(input);
    expect(draft().model).toBe("");
  });

  it("loads legacy role defaults and lossless Codex catalog fields from the original parser", () => {
    const claude = providerModelDefaults(
      {
        id: "c",
        name: "C",
        settingsConfig: {
          env: { ANTHROPIC_MODEL: "base", ANTHROPIC_SMALL_FAST_MODEL: "small" },
        },
      },
      "claude",
    );
    expect(claude.modelSettings.ANTHROPIC_DEFAULT_HAIKU_MODEL).toBe("small");
    expect(claude.modelSettings.ANTHROPIC_DEFAULT_FABLE_MODEL).toBe("base");
    const codex = providerModelDefaults(
      {
        id: "x",
        name: "X",
        settingsConfig: {
          config:
            "model_context_window = 1000000\nmodel_auto_compact_token_limit = 900000",
          modelCatalog: {
            models: [
              {
                model: "a",
                context_window: 128000,
                base_instructions: "first\nsecond",
                supports_parallel_tool_calls: true,
                input_modalities: ["text", "image"],
                reasoning_levels: ["high"],
                default_reasoning_level: "high",
              },
            ],
          },
        },
      },
      "codex",
    );
    expect(codex.modelSettings.model_context_window).toBe("1000000");
    expect(codex.modelCatalog?.models).toEqual([
      expect.objectContaining({
        contextWindow: 128000,
        baseInstructions: "first\nsecond",
        supportsParallelToolCalls: true,
        inputModalities: ["text", "image"],
        reasoningLevels: ["high"],
      }),
    ]);
  });

  it("fetches with the referenced provider and edits role/1M values without copying connection data", async () => {
    const provider: Provider = {
      id: "c",
      name: "Configured provider",
      settingsConfig: {
        env: {
          ANTHROPIC_BASE_URL: "https://example.test/v1",
          ANTHROPIC_AUTH_TOKEN: "fixture-key",
          ANTHROPIC_MODEL: "base",
        },
      },
    };
    const before = JSON.stringify(provider);
    renderWithQueryClient(<Harness scope="claude" provider={provider} />);
    expect(screen.queryByDisplayValue("fixture-key")).toBeNull();
    expect(screen.queryByDisplayValue("https://example.test/v1")).toBeNull();
    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.fetchModels" }),
    );
    await waitFor(() =>
      expect(mocks.fetch).toHaveBeenCalledWith(
        "https://example.test/v1",
        "fixture-key",
        false,
        undefined,
        "",
        expect.any(Object),
      ),
    );
    fireEvent.change(screen.getByLabelText("providerForm.modelRoleSonnet"), {
      target: { value: "replacement" },
    });
    await waitFor(() =>
      expect(draft().modelSettings.ANTHROPIC_DEFAULT_SONNET_MODEL_NAME).toBe(
        "replacement",
      ),
    );
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "providerForm.modelRoleSonnet providerForm.modelOneMLabel",
      }),
    );
    expect(draft().modelSettings.ANTHROPIC_DEFAULT_SONNET_MODEL).toBe(
      "replacement[1m]",
    );
    expect(draft()).toEqual(
      expect.objectContaining({
        mcpIds: ["kept-mcp"],
        skillIds: ["kept-skill"],
        promptId: "kept-prompt",
      }),
    );
    expect(JSON.stringify(draft())).not.toContain("fixture-key");
    expect(JSON.stringify(provider)).toBe(before);
  });

  it("retains catalog input identity, follows the default model and keeps hidden metadata", async () => {
    const provider: Provider = {
      id: "x",
      name: "X",
      settingsConfig: {
        config: "model='alpha'",
        modelCatalog: {
          models: [
            {
              model: "alpha",
              displayName: "A",
              reasoningLevels: ["low", "high"],
              baseInstructions: "keep\nlines",
              inputModalities: ["text", "image"],
            },
          ],
        },
      },
    };
    renderWithQueryClient(<Harness scope="codex" provider={provider} />);
    const input = screen.getByLabelText("codexConfig.catalogColumnModel");
    input.focus();
    fireEvent.change(input, { target: { value: "renamed" } });
    await waitFor(() => expect(draft().model).toBe("renamed"));
    expect(screen.getByLabelText("codexConfig.catalogColumnModel")).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(draft().modelCatalog.models[0]).toEqual(
      expect.objectContaining({
        model: "renamed",
        baseInstructions: "keep\nlines",
        inputModalities: ["text", "image"],
      }),
    );
    fireEvent.click(screen.getByText("profiles.editor.runtimeSettings"));
    const effort = screen.getByLabelText("profiles.editor.mainReasoningEffort");
    expect(
      within(effort)
        .getAllByRole("option")
        .map((option) => option.getAttribute("value")),
    ).toEqual(["", "low", "high"]);
    fireEvent.click(
      screen.getByRole("checkbox", { name: "codexConfig.contextWindow1M" }),
    );
    expect(draft().modelSettings).toEqual(
      expect.objectContaining({
        model_context_window: "1000000",
        model_auto_compact_token_limit: "900000",
      }),
    );
  });

  it("shows models for official providers and resolves the bound OAuth account for fetching", async () => {
    const provider: Provider = {
      id: "official",
      name: "Official",
      category: "official",
      settingsConfig: { config: "model='official-model'" },
      meta: {
        providerType: "codex_oauth",
        authBinding: {
          source: "managed_account",
          authProvider: "codex_oauth",
          accountId: "bound-account",
        },
      },
    };
    mocks.status.mockResolvedValue({
      authenticated: true,
      accounts: [{ id: "bound-account" }],
    });
    renderWithQueryClient(<Harness scope="codex" provider={provider} />);
    expect(
      screen.getByLabelText("profiles.editor.model", { selector: "input" }),
    ).toHaveValue("official-model");
    expect(
      screen.getByLabelText("codexConfig.agentMaxThreadsLabel", {
        selector: "input",
      }),
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.fetchModels" }),
    );
    await waitFor(() =>
      expect(mocks.oauth).toHaveBeenCalledWith("bound-account"),
    );
    expect(mocks.fetch).not.toHaveBeenCalled();
    expect(draft().providerId).toBe("official");
  });
});
