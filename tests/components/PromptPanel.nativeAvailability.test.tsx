import { QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import PromptPanel from "@/components/prompts/PromptPanel";
import { promptsApi, type AppId } from "@/lib/api";
import { createTestQueryClient } from "../utils/testQueryClient";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key, i18n: { language: "en" } }),
}));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
vi.mock("@/lib/toast", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(promptsApi, "getPrompts").mockResolvedValue({});
  vi.spyOn(promptsApi, "getPromptTarget").mockResolvedValue({
    path: "/pi/AGENTS.md",
    displayPath: "~/.pi/agent/AGENTS.md",
    content: null,
    revision: "missing",
    appliedPromptId: null,
    managed: false,
    matchesLibrary: false,
  });
  vi.spyOn(promptsApi, "getPiPromptFile").mockResolvedValue({
    exists: false,
    revision: "missing",
    content: "",
  });
  vi.spyOn(promptsApi, "listPiPromptTemplates").mockResolvedValue([]);
  vi.spyOn(promptsApi, "replacePiPromptFile").mockResolvedValue({
    exists: true,
    revision: "saved",
    content: "native draft",
  });
  vi.spyOn(promptsApi, "upsertPiPromptTemplate").mockResolvedValue({
    slug: "draft",
    revision: "saved",
    content: "",
  });
});

function renderPanel() {
  const client = createTestQueryClient();
  const onAppChange = vi.fn();
  const onNavigationBlockedChange = vi.fn();
  const panel = (apps: AppId[]) => (
    <QueryClientProvider client={client}>
      <PromptPanel
        appId="pi"
        apps={apps}
        onAppChange={onAppChange}
        onNavigationBlockedChange={onNavigationBlockedChange}
      />
    </QueryClientProvider>
  );
  const view = render(panel(["pi", "claude"]));
  return {
    onAppChange,
    onNavigationBlockedChange,
    rerenderApps: (apps: AppId[]) => view.rerender(panel(apps)),
  };
}

async function openDraft(kind: "system" | "templates") {
  fireEvent.click(
    await screen.findByRole("button", {
      name:
        kind === "system" ? "pi.prompts.systemTab" : "pi.prompts.templatesTab",
    }),
  );
  if (kind === "system") {
    const create = await screen.findByRole("button", {
      name: "pi.prompts.create APPEND_SYSTEM.md",
    });
    await waitFor(() => expect(create).toBeEnabled());
    fireEvent.click(create);
    fireEvent.change(
      screen.getByPlaceholderText("pi.prompts.instructionPlaceholder"),
      { target: { value: "native draft" } },
    );
  } else {
    const create = (
      await screen.findAllByRole("button", { name: "pi.prompts.newTemplate" })
    )[0];
    await waitFor(() => expect(create).toBeEnabled());
    fireEvent.click(create);
    fireEvent.change(screen.getByPlaceholderText("pi.prompts.templateSlug"), {
      target: { value: "draft" },
    });
  }
}

describe("shared library Pi native availability", () => {
  it.each(["system", "templates"] as const)(
    "retains a %s draft, blocks navigation and stops new writes after Pi disappears",
    async (kind) => {
      const view = renderPanel();
      await openDraft(kind);
      await waitFor(() =>
        expect(view.onNavigationBlockedChange).toHaveBeenLastCalledWith(true),
      );
      const segments = screen.getByRole("group", {
        name: "pi.prompts.kindLabel",
      });
      fireEvent.click(
        within(segments).getByRole("button", { name: "prompts.library.title" }),
      );
      expect(
        screen.getByPlaceholderText(
          kind === "system"
            ? "pi.prompts.instructionPlaceholder"
            : "pi.prompts.templateSlug",
        ),
      ).toHaveValue(kind === "system" ? "native draft" : "draft");
      view.rerenderApps(["claude"]);
      const save = screen.getByRole("button", {
        name: kind === "system" ? "common.save" : "pi.prompts.createTemplate",
      });
      expect(save).toBeDisabled();
      fireEvent.click(save);
      expect(promptsApi.replacePiPromptFile).not.toHaveBeenCalled();
      expect(promptsApi.upsertPiPromptTemplate).not.toHaveBeenCalled();
      expect(view.onAppChange).not.toHaveBeenCalled();
      expect(view.onNavigationBlockedChange).toHaveBeenLastCalledWith(true);
      fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
      await waitFor(() =>
        expect(view.onAppChange).toHaveBeenCalledWith("claude"),
      );
      expect(view.onNavigationBlockedChange).toHaveBeenLastCalledWith(false);
    },
  );

  it("lets an already started Pi save finish before selecting another Agent", async () => {
    let finish!: (value: {
      exists: boolean;
      revision: string;
      content: string;
    }) => void;
    vi.mocked(promptsApi.replacePiPromptFile).mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const view = renderPanel();
    await openDraft("system");
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(promptsApi.replacePiPromptFile).toHaveBeenCalledOnce(),
    );
    view.rerenderApps(["claude"]);
    expect(view.onAppChange).not.toHaveBeenCalled();
    expect(view.onNavigationBlockedChange).toHaveBeenLastCalledWith(true);
    await act(async () =>
      finish({ exists: true, revision: "saved", content: "native draft" }),
    );
    await waitFor(() =>
      expect(view.onAppChange).toHaveBeenCalledWith("claude"),
    );
    expect(promptsApi.replacePiPromptFile).toHaveBeenCalledOnce();
  });
});
