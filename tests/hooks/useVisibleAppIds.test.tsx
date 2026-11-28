import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppId } from "@/lib/api/types";
import { useVisibleAppIds } from "@/components/mcp/useVisibleAppIds";

const mocks = vi.hoisted(() => ({
  settings: {
    data: { visibleApps: {} } as
      | { visibleApps: Partial<Record<AppId, boolean>> }
      | undefined,
    isPending: false,
  },
  availability: {
    installedApps: {} as Partial<Record<AppId, boolean>>,
    isPending: false,
  },
}));
vi.mock("@/lib/query", () => ({ useSettingsQuery: () => mocks.settings }));
vi.mock("@/hooks/useAgentAvailability", () => ({
  useAgentAvailability: () => mocks.availability,
}));

const apps = ["claude", "codex", "gemini", "pi"] as const;

describe("useVisibleAppIds", () => {
  beforeEach(() => {
    mocks.settings = { data: { visibleApps: {} }, isPending: false };
    mocks.availability = { installedApps: {}, isPending: false };
  });

  it("requires confirmed installation and enabled preferences, preserving capability order", () => {
    mocks.settings.data!.visibleApps = { claude: false };
    mocks.availability.installedApps = {
      claude: true,
      codex: true,
      gemini: false,
      pi: true,
      opencode: true,
    };
    const { result } = renderHook(() => useVisibleAppIds(apps));
    expect(result.current).toEqual(["codex", "pi"]);
    expect(mocks.settings.data!.visibleApps).toEqual({ claude: false });
  });

  it("hides options while loading and updates after install, disable, and uninstall", () => {
    mocks.availability = { installedApps: { codex: true }, isPending: true };
    const { result, rerender } = renderHook(() => useVisibleAppIds(apps));
    expect(result.current).toEqual([]);
    mocks.availability.isPending = false;
    mocks.settings.isPending = true;
    rerender();
    expect(result.current).toEqual([]);
    mocks.settings.isPending = false;
    rerender();
    expect(result.current).toEqual(["codex"]);
    mocks.availability.installedApps = { claude: true, codex: true };
    rerender();
    expect(result.current).toEqual(["claude", "codex"]);
    mocks.settings.data!.visibleApps = { codex: false };
    rerender();
    expect(result.current).toEqual(["claude"]);
    mocks.availability.installedApps = { claude: false, codex: true };
    rerender();
    expect(result.current).toEqual([]);
  });

  it("does not assume enabled preferences when settings fail to load", () => {
    mocks.settings.data = undefined;
    mocks.availability.installedApps = { claude: true, codex: true };
    const { result } = renderHook(() => useVisibleAppIds(apps));
    expect(result.current).toEqual([]);
  });
});
