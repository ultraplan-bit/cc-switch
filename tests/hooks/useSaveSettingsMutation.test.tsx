import type { ReactNode } from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClientProvider, onlineManager } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { useSaveSettingsMutation } from "@/lib/query/mutations";
import { createTestQueryClient } from "../utils/testQueryClient";
import type { Settings } from "@/types";

const mocks = vi.hoisted(() => ({ save: vi.fn() }));
vi.mock("@/lib/api", () => ({
  settingsApi: { save: mocks.save },
  providersApi: {},
  sessionsApi: {},
}));
vi.mock("@/hooks/useHermes", () => ({
  invalidateHermesProviderCaches: vi.fn(),
}));
vi.mock("@/hooks/useOpenClaw", () => ({
  openclawKeys: { health: ["openclaw", "health"] },
}));

it("persists local settings and invalidates their cache while offline", async () => {
  const wasOnline = onlineManager.isOnline();
  const client = createTestQueryClient();
  const settings: Settings = {
    showInTray: true,
    minimizeToTrayOnClose: true,
    claudeConfigDir: "/new/.claude",
  };
  client.setQueryData(["settings"], {
    ...settings,
    claudeConfigDir: "/old/.claude",
  });
  mocks.save.mockResolvedValue(true);
  onlineManager.setOnline(false);
  const hook = renderHook(() => useSaveSettingsMutation(), {
    wrapper: ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    ),
  });
  try {
    act(() => hook.result.current.mutate(settings));
    await waitFor(() => expect(hook.result.current.isSuccess).toBe(true));
    expect(mocks.save).toHaveBeenCalledWith(settings);
    expect(hook.result.current.isPaused).toBe(false);
    expect(client.getQueryState(["settings"])?.isInvalidated).toBe(true);
  } finally {
    hook.unmount();
    client.clear();
    onlineManager.setOnline(wasOnline);
  }
});
