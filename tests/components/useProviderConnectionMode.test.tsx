import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useProviderConnectionMode } from "@/components/providers/useProviderConnectionMode";

const state = vi.hoisted(() => ({
  isPending: true,
  isFetching: true,
  isSuccess: false,
  mode: "direct",
}));
vi.mock("@/lib/query/profiles", () => ({
  useProfileConfiguration: () => state,
}));
vi.mock("@/lib/query/proxy", () => ({
  useAppMode: () => ({ isPending: false, data: { mode: state.mode } }),
}));

describe("provider connection mode", () => {
  beforeEach(() =>
    Object.assign(state, {
      isPending: true,
      isFetching: true,
      isSuccess: false,
      mode: "direct",
    }),
  );
  it("waits for migration and freezes editing mode across refetches", () => {
    const { result, rerender } = renderHook(
      ({ open }) => useProviderConnectionMode("claude", open),
      { initialProps: { open: true } },
    );
    expect(result.current.pending).toBe(true);
    Object.assign(state, {
      isPending: false,
      isFetching: false,
      isSuccess: true,
    });
    rerender({ open: true });
    expect(result.current).toMatchObject({
      pending: false,
      connectionsOnly: true,
    });
    Object.assign(state, { isFetching: true, isSuccess: false });
    rerender({ open: true });
    expect(result.current.connectionsOnly).toBe(true);
    rerender({ open: false });
    Object.assign(state, { isFetching: false });
    rerender({ open: true });
    expect(result.current).toMatchObject({
      pending: false,
      connectionsOnly: false,
    });
  });
  it("retains aggregation inventory editing", () => {
    Object.assign(state, {
      isPending: false,
      isFetching: false,
      isSuccess: true,
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
});
