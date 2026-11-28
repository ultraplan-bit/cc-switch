import { createElement, type ReactNode } from "react";
import { QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { usePromptActions } from "@/hooks/usePromptActions";
import { promptsApi, type AppId } from "@/lib/api";
import { createTestQueryClient } from "../utils/testQueryClient";
const errors = vi.hoisted(() => vi.fn());
vi.mock("@/lib/toast", () => ({ toast: { error: errors } }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
const prompt = { id: "p", name: "Shared", content: "library" };
function snapshot(app: string) {
  return {
    path: `${app}/AGENTS.md`,
    displayPath: `${app}/AGENTS.md`,
    content: "external",
    revision: app,
    appliedPromptId: null,
    managed: false,
    matchesLibrary: false,
  };
}
function setup() {
  const client = createTestQueryClient();
  return renderHook(({ app }: { app: AppId }) => usePromptActions(app), {
    initialProps: { app: "claude" as AppId },
    wrapper: ({ children }: { children: ReactNode }) =>
      createElement(QueryClientProvider, { client }, children),
  });
}
beforeEach(() => {
  vi.restoreAllMocks();
  errors.mockReset();
  vi.spyOn(promptsApi, "getPrompts").mockResolvedValue({ p: prompt });
  vi.spyOn(promptsApi, "getPromptTarget").mockImplementation(async (app) =>
    snapshot(app),
  );
  vi.spyOn(promptsApi, "upsertPrompt").mockResolvedValue(undefined);
  vi.spyOn(promptsApi, "applyPrompt").mockResolvedValue(snapshot("claude"));
});
describe("usePromptActions shared library", () => {
  it("never backfills a target file into the library on read", async () => {
    const { result } = setup();
    await waitFor(() =>
      expect(result.current.target.data?.content).toBe("external"),
    );
    expect(result.current.prompts.p.content).toBe("library");
    expect(promptsApi.upsertPrompt).not.toHaveBeenCalled();
  });
  it("keeps a slow previous target request separate after changing Agent", async () => {
    let finish!: (value: ReturnType<typeof snapshot>) => void;
    vi.mocked(promptsApi.getPromptTarget).mockImplementation((app) =>
      app === "claude"
        ? new Promise((done) => {
            finish = done;
          })
        : Promise.resolve(snapshot(app)),
    );
    const { result, rerender } = setup();
    rerender({ app: "codex" });
    await waitFor(() =>
      expect(result.current.target.data?.revision).toBe("codex"),
    );
    await act(async () => {
      finish(snapshot("claude"));
    });
    expect(result.current.target.data?.revision).toBe("codex");
    expect(result.current.prompts.p.name).toBe("Shared");
  });
  it("keeps a successful save successful even if the following reload fails", async () => {
    const { result } = setup();
    await waitFor(() => expect(result.current.library.isSuccess).toBe(true));
    vi.mocked(promptsApi.getPrompts).mockRejectedValue(
      new Error("refresh unavailable"),
    );
    let saved = false;
    await act(async () => {
      saved = await result.current.savePrompt("p", {
        ...prompt,
        content: "edited",
      });
    });
    expect(saved).toBe(true);
    expect(result.current.prompts.p.content).toBe("edited");
    expect(promptsApi.applyPrompt).not.toHaveBeenCalled();
  });
});
