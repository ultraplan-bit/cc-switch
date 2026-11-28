import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import PromptFormPanel from "@/components/prompts/PromptFormPanel";
import type { Prompt } from "@/lib/api";
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
const prompt: Prompt = {
  id: "team",
  name: "Team",
  content: "# Rules",
  createdAt: 1,
};
describe("PromptFormPanel", () => {
  it("does not reset a draft when saved query data refreshes", () => {
    const onSave = vi.fn();
    const onClose = vi.fn();
    const view = render(
      <PromptFormPanel
        editingId="team"
        initialData={prompt}
        onSave={onSave}
        onClose={onClose}
      />,
    );
    fireEvent.change(screen.getByLabelText("prompts.content"), {
      target: { value: "draft" },
    });
    view.rerender(
      <PromptFormPanel
        editingId="team"
        initialData={{ ...prompt, content: "background update" }}
        onSave={onSave}
        onClose={onClose}
      />,
    );
    expect(screen.getByLabelText("prompts.content")).toHaveValue("draft");
  });
  it("confirms before closing a dirty draft and keeps it when cancelled", async () => {
    const onClose = vi.fn();
    render(
      <PromptFormPanel
        editingId="team"
        initialData={prompt}
        onSave={vi.fn()}
        onClose={onClose}
      />,
    );
    fireEvent.change(screen.getByLabelText("prompts.content"), {
      target: { value: "draft" },
    });
    fireEvent.click(screen.getByRole("button", { name: "common.back" }));
    const dialog = await screen.findByRole("dialog", {
      name: "prompts.library.discardTitle",
    });
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(
      within(dialog).getByRole("button", {
        name: "prompts.library.keepEditing",
      }),
    );
    expect(screen.getByLabelText("prompts.content")).toHaveValue("draft");
    fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
    fireEvent.click(
      screen.getByRole("button", { name: "prompts.library.discard" }),
    );
    expect(onClose).toHaveBeenCalledOnce();
  });
  it("submits once and refuses to close while saving", async () => {
    let resolve!: () => void;
    const onSave = vi.fn(
      () =>
        new Promise<void>((done) => {
          resolve = done;
        }),
    );
    const onClose = vi.fn();
    render(
      <PromptFormPanel
        initialData={prompt}
        editingId="team"
        onSave={onSave}
        onClose={onClose}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
    fireEvent.click(screen.getByRole("button", { name: "common.back" }));
    expect(onSave).toHaveBeenCalledOnce();
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => {
      resolve();
    });
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  });
  it("preserves exact Markdown and saves no application state", async () => {
    const onSave = vi.fn().mockResolvedValue(true);
    render(
      <PromptFormPanel
        initialData={prompt}
        editingId="team"
        onSave={onSave}
        onClose={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByLabelText("prompts.content"), {
      target: { value: "  # Rules\n\n" },
    });
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() => expect(onSave).toHaveBeenCalled());
    expect(onSave.mock.calls[0][1]).toEqual(
      expect.objectContaining({ content: "  # Rules\n\n" }),
    );
    expect(onSave.mock.calls[0][1]).not.toHaveProperty("enabled");
  });
  it("retains the form after save rejection and validates the name", async () => {
    const onSave = vi.fn().mockResolvedValue(false);
    const onClose = vi.fn();
    render(<PromptFormPanel onSave={onSave} onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: "prompts.addSubmit" }));
    expect(screen.getByText("prompts.nameRequired")).toBeVisible();
    fireEvent.change(screen.getByLabelText(/prompts.name/), {
      target: { value: "New" },
    });
    fireEvent.click(screen.getByRole("button", { name: "prompts.addSubmit" }));
    await waitFor(() => expect(onSave).toHaveBeenCalledOnce());
    expect(onClose).not.toHaveBeenCalled();
  });
});
