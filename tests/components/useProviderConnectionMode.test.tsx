import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useProviderConnectionMode } from "@/components/providers/useProviderConnectionMode";

const state = vi.hoisted(() => ({
  isPending: true,
  mode: "direct",
}));
vi.mock("@/lib/query/proxy", () => ({
  useAppMode: () => ({
    isPending: state.isPending,
    data: state.isPending ? undefined : { mode: state.mode },
  }),
}));

describe("provider connection mode", () => {
  beforeEach(() =>
    Object.assign(state, {
      isPending: true,
      mode: "direct",
    }),
  );
  it("waits for the app mode and freezes editing mode for each open session", () => {
    const { result, rerender } = renderHook(
      ({ open }) => useProviderConnectionMode("claude", open),
      { initialProps: { open: true } },
    );
    expect(result.current.pending).toBe(true);
    state.isPending = false;
    rerender({ open: true });
    expect(result.current).toMatchObject({
      pending: false,
      connectionsOnly: true,
    });
    state.mode = "stack";
    rerender({ open: true });
    expect(result.current.connectionsOnly).toBe(true);
    rerender({ open: false });
    rerender({ open: true });
    expect(result.current).toMatchObject({
      pending: false,
      connectionsOnly: false,
    });
  });
  it("retains aggregation inventory editing", () => {
    Object.assign(state, {
      isPending: false,
      mode: "stack",
    });
    const { result } = renderHook(() =>
      useProviderConnectionMode("codex", true),
    );
    expect(result.current).toMatchObject({
      pending: false,
      connectionsOnly: false,
    });
  });
  it("uses an explicit mode without waiting for the app query", () => {
    const { result } = renderHook(() =>
      useProviderConnectionMode("codex", true, "direct"),
    );
    expect(result.current).toMatchObject({
      pending: false,
      connectionsOnly: true,
    });
  });
  it("does not wait or enable connection-only editing for other apps", () => {
    const { result } = renderHook(() =>
      useProviderConnectionMode("gemini", true),
    );
    expect(result.current).toMatchObject({
      pending: false,
      connectionsOnly: false,
    });
  });
});
