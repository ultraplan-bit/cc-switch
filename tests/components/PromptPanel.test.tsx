import { QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import PromptPanel from "@/components/prompts/PromptPanel";
import { promptsApi, type AppId, type Prompt } from "@/lib/api";
import type { PromptTargetSnapshot } from "@/lib/api/prompts";
import { createTestQueryClient } from "../utils/testQueryClient";

const mocks = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, values?: Record<string, unknown>) =>
      values?.name ? `${key}:${values.name}` : key,
    i18n: { language: "en" },
  }),
}));
vi.mock("@/lib/toast", () => ({
  toast: { success: mocks.success, error: mocks.error },
}));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
vi.mock("@/components/prompts/PiNativePromptResources", async () => {
  const { forwardRef } = await import("react");
  return {
    PI_PROMPT_SAVE_MUTATION_KEY: ["pi", "promptSave"],
    PiSystemPromptFiles: () => <div>native-system-files</div>,
    PiPromptTemplates: forwardRef(() => <div>native-command-templates</div>),
  };
});

const prompt: Prompt = {
  id: "shared",
  name: "Team rules",
  content: "# Shared rules",
  createdAt: 1,
};
let library: Record<string, Prompt>;
let targets: Record<string, PromptTargetSnapshot>;
function target(
  app: string,
  content: string | null = null,
): PromptTargetSnapshot {
  const file = app === "claude" ? "CLAUDE.md" : "AGENTS.md";
  return {
    path: `/home/test/.${app}/${file}`,
    displayPath: `~/.${app}/${file}`,
    content,
    revision: `${app}:${content ?? "missing"}`,
    appliedPromptId: null,
    managed: false,
    matchesLibrary: false,
  };
}
function renderPanel(
  appId: AppId = "claude",
  onNavigationBlockedChange = vi.fn(),
) {
  const client = createTestQueryClient();
  const props = {
    apps: ["claude", "codex", "pi"] as AppId[],
    onAppChange: vi.fn(),
    onNavigationBlockedChange,
  };
  const result = render(
    <QueryClientProvider client={client}>
      <PromptPanel {...props} appId={appId} />
    </QueryClientProvider>,
  );
  return {
    ...result,
    client,
    rerenderApp: (app: AppId) =>
      result.rerender(
        <QueryClientProvider client={client}>
          <PromptPanel {...props} appId={app} />
        </QueryClientProvider>,
      ),
  };
}
beforeEach(() => {
  vi.restoreAllMocks();
  mocks.success.mockReset();
  mocks.error.mockReset();
  library = { shared: { ...prompt } };
  targets = {
    claude: target("claude"),
    codex: target("codex"),
    pi: target("pi"),
  };
  vi.spyOn(promptsApi, "getPrompts").mockImplementation(async () => ({
    ...library,
  }));
  vi.spyOn(promptsApi, "getPromptTarget").mockImplementation(async (app) => ({
    ...targets[app],
  }));
  vi.spyOn(promptsApi, "upsertPrompt").mockImplementation(async (id, value) => {
    library[id] = value;
  });
  vi.spyOn(promptsApi, "deletePrompt").mockImplementation(async (id) => {
    delete library[id];
  });
  vi.spyOn(promptsApi, "applyPrompt").mockImplementation(async (app, id) => {
    targets[app] = {
      ...targets[app],
      content: library[id].content,
      appliedPromptId: id,
      managed: true,
      matchesLibrary: true,
    };
    return targets[app];
  });
  vi.spyOn(promptsApi, "removePromptApplication").mockImplementation(
    async (app) => {
      targets[app] = target(app, "");
      return targets[app];
    },
  );
  vi.spyOn(promptsApi, "importFromFile").mockResolvedValue("imported");
});

describe("shared prompt library", () => {
  it("shows the same library when only the distribution target changes", async () => {
    const view = renderPanel();
    await screen.findByText("Team rules");
    expect(screen.getByText("~/.claude/CLAUDE.md")).toBeVisible();
    view.rerenderApp("codex");
    expect(screen.getByText("Team rules")).toBeVisible();
    await screen.findByText("~/.codex/AGENTS.md");
    expect(
      vi
        .mocked(promptsApi.getPrompts)
        .mock.calls.every((args) => args.length === 0),
    ).toBe(true);
  });
  it("saves only the library, preserves typed Markdown, and guards navigation while editing", async () => {
    const navigation = vi.fn();
    renderPanel("claude", navigation);
    fireEvent.click(
      await screen.findByRole("button", {
        name: "prompts.editAria:Team rules",
      }),
    );
    await waitFor(() => expect(navigation).toHaveBeenLastCalledWith(true));
    fireEvent.change(screen.getByLabelText("prompts.content"), {
      target: { value: "  # Edited\n\n" },
    });
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(promptsApi.upsertPrompt).toHaveBeenCalledWith(
        "shared",
        expect.objectContaining({ content: "  # Edited\n\n" }),
      ),
    );
    expect(promptsApi.applyPrompt).not.toHaveBeenCalled();
    expect(promptsApi.removePromptApplication).not.toHaveBeenCalled();
  });
  it("allows editing the shared library when a target cannot be read", async () => {
    vi.mocked(promptsApi.getPromptTarget).mockRejectedValue(
      new Error("unreadable"),
    );
    renderPanel();
    fireEvent.click(
      await screen.findByRole("button", {
        name: "prompts.editAria:Team rules",
      }),
    );
    expect(screen.getByLabelText("prompts.content")).toHaveValue(
      prompt.content,
    );
  });
  it("previews CLAUDE.md and AGENTS.md and requires consent before overwriting", async () => {
    targets.claude = target("claude", "handwritten");
    renderPanel();
    fireEvent.click(
      await screen.findByRole("button", {
        name: "prompts.library.applyAria:Team rules",
      }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "prompts.library.distributeTitle:Team rules",
    });
    await waitFor(() =>
      expect(within(dialog).getByText("~/.claude/CLAUDE.md")).toBeVisible(),
    );
    expect(within(dialog).getByText("~/.codex/AGENTS.md")).toBeVisible();
    const apply = within(dialog).getByRole("button", {
      name: "prompts.library.applySelected",
    });
    expect(apply).toBeDisabled();
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "Codex" }));
    fireEvent.click(
      within(dialog).getByRole("checkbox", {
        name: "prompts.library.overwriteConsent",
      }),
    );
    fireEvent.click(apply);
    await waitFor(() =>
      expect(promptsApi.applyPrompt).toHaveBeenCalledWith(
        "claude",
        "shared",
        "claude:handwritten",
        prompt.content,
      ),
    );
    expect(promptsApi.applyPrompt).toHaveBeenCalledWith(
      "codex",
      "shared",
      "codex:missing",
      prompt.content,
    );
    expect(promptsApi.upsertPrompt).not.toHaveBeenCalled();
  });
  it("keeps partial distribution failures visible without retrying successful targets", async () => {
    vi.mocked(promptsApi.applyPrompt).mockImplementation(async (app) => {
      if (app === "codex") throw new Error("target changed");
      return targets[app];
    });
    renderPanel();
    fireEvent.click(
      await screen.findByRole("button", {
        name: "prompts.library.applyAria:Team rules",
      }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "prompts.library.distributeTitle:Team rules",
    });
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "Codex" }));
    const apply = within(dialog).getByRole("button", {
      name: "prompts.library.applySelected",
    });
    await waitFor(() => expect(apply).toBeEnabled());
    fireEvent.click(apply);
    await within(dialog).findByText("target changed");
    expect(
      within(dialog).getByRole("checkbox", { name: /Claude Code/ }),
    ).toBeDisabled();
    expect(apply).toBeDisabled();
    expect(promptsApi.applyPrompt).toHaveBeenCalledTimes(2);
    expect(within(dialog).getByText("prompts.library.appliedTo")).toBeVisible();
    expect(mocks.success).not.toHaveBeenCalled();
    expect(mocks.error).toHaveBeenCalledWith(
      "prompts.library.distributionFailed",
    );
  });
  it("can deselect an oversized MCode target and distribute to a different Agent", async () => {
    library.shared = { ...prompt, content: "x".repeat(32769) };
    targets.mcode = target("mcode");
    renderPanel("mcode");
    fireEvent.click(
      await screen.findByRole("button", {
        name: "prompts.library.applyAria:Team rules",
      }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "prompts.library.distributeTitle:Team rules",
    });
    const mcode = within(dialog).getByRole("checkbox", {
      name: "MiniMax Code",
    });
    expect(mcode).toBeEnabled();
    fireEvent.click(mcode);
    fireEvent.click(
      within(dialog).getByRole("checkbox", { name: "Claude Code" }),
    );
    const apply = within(dialog).getByRole("button", {
      name: "prompts.library.applySelected",
    });
    await waitFor(() => expect(apply).toBeEnabled());
    fireEvent.click(apply);
    await waitFor(() =>
      expect(promptsApi.applyPrompt).toHaveBeenCalledWith(
        "claude",
        "shared",
        "claude:missing",
        library.shared.content,
      ),
    );
    expect(
      vi
        .mocked(promptsApi.applyPrompt)
        .mock.calls.some(([app]) => app === "mcode"),
    ).toBe(false);
  });
  it("reports total distribution failure without a success toast", async () => {
    vi.mocked(promptsApi.applyPrompt).mockRejectedValue(
      new Error("write denied"),
    );
    renderPanel();
    fireEvent.click(
      await screen.findByRole("button", {
        name: "prompts.library.applyAria:Team rules",
      }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "prompts.library.distributeTitle:Team rules",
    });
    const apply = within(dialog).getByRole("button", {
      name: "prompts.library.applySelected",
    });
    await waitFor(() => expect(apply).toBeEnabled());
    fireEvent.click(apply);
    await within(dialog).findByText("write denied");
    expect(mocks.success).not.toHaveBeenCalled();
    expect(mocks.error).toHaveBeenCalledWith(
      "prompts.library.distributionFailed",
    );
    expect(
      within(dialog).queryByText("prompts.library.appliedTo"),
    ).not.toBeInTheDocument();
  });
  it("requires fresh overwrite consent when a preview revision changes", async () => {
    targets.claude = target("claude", "before");
    const { client } = renderPanel();
    fireEvent.click(
      await screen.findByRole("button", {
        name: "prompts.library.applyAria:Team rules",
      }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "prompts.library.distributeTitle:Team rules",
    });
    const consent = within(dialog).getByRole("checkbox", {
      name: "prompts.library.overwriteConsent",
    });
    fireEvent.click(consent);
    const apply = within(dialog).getByRole("button", {
      name: "prompts.library.applySelected",
    });
    await waitFor(() => expect(apply).toBeEnabled());
    targets.claude = target("claude", "changed after preview");
    await act(async () => {
      await client.invalidateQueries({
        queryKey: ["prompts", "target", "claude"],
      });
    });
    // Query invalidation resolves before the observers' scheduled render.
    await within(dialog).findByText("changed after preview");
    await waitFor(() => {
      expect(consent).not.toBeChecked();
      expect(apply).toBeDisabled();
    });
    expect(promptsApi.applyPrompt).not.toHaveBeenCalled();
  });
  it("deletes a library document without withdrawing deployed content", async () => {
    targets.claude = {
      ...target("claude", prompt.content),
      appliedPromptId: prompt.id,
      managed: true,
      matchesLibrary: true,
    };
    renderPanel();
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", {
        name: "prompts.rowMoreActions:Team rules",
      }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: "common.delete" }),
    );
    expect(promptsApi.deletePrompt).not.toHaveBeenCalled();
    const dialog = screen.getByRole("dialog", {
      name: "prompts.library.deleteTitle",
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "common.confirm" }),
    );
    await waitFor(() =>
      expect(promptsApi.deletePrompt).toHaveBeenCalledWith("shared"),
    );
    expect(promptsApi.removePromptApplication).not.toHaveBeenCalled();
  });
  it("withdraws only after confirmation using the reviewed revision", async () => {
    targets.claude = {
      ...target("claude", prompt.content),
      appliedPromptId: prompt.id,
      managed: true,
      matchesLibrary: true,
    };
    renderPanel();
    fireEvent.click(
      await screen.findByRole("button", { name: "prompts.library.remove" }),
    );
    const dialog = screen.getByRole("dialog", {
      name: "prompts.library.remove",
    });
    expect(promptsApi.removePromptApplication).not.toHaveBeenCalled();
    fireEvent.click(
      within(dialog).getByRole("button", { name: "common.confirm" }),
    );
    await waitFor(() =>
      expect(promptsApi.removePromptApplication).toHaveBeenCalledWith(
        "claude",
        "claude:# Shared rules",
      ),
    );
  });
  it("keeps Pi native system and command templates next to the same shared library", async () => {
    renderPanel("pi");
    await screen.findByText("Team rules");
    fireEvent.click(
      screen.getByRole("button", { name: "pi.prompts.systemTab" }),
    );
    expect(screen.getByText("native-system-files")).toBeVisible();
    fireEvent.click(
      screen.getByRole("button", { name: "pi.prompts.templatesTab" }),
    );
    expect(screen.getByText("native-command-templates")).toBeVisible();
    fireEvent.click(
      within(
        screen.getByRole("group", { name: "pi.prompts.kindLabel" }),
      ).getByRole("button", { name: "prompts.library.title" }),
    );
    expect(screen.getByText("Team rules")).toBeVisible();
  });
});
