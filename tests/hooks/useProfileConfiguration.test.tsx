import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import type { PropsWithChildren } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useProfileConfiguration } from "@/lib/query/profiles";
import { createTestQueryClient } from "../utils/testQueryClient";

const migrate = vi.hoisted(() => vi.fn());
vi.mock("@/lib/api", () => ({ profilesApi: { migrateLegacy: migrate } }));

beforeEach(() => migrate.mockReset());

describe("Profile configuration migration", () => {
  it("shares successful migration between views and only rechecks on invalidation", async () => {
    const client = createTestQueryClient();
    const wrapper = ({ children }: PropsWithChildren) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    migrate.mockResolvedValue({ profiles: [], warnings: [] });
    const first = renderHook(() => useProfileConfiguration("claude"), {
      wrapper,
    });
    await waitFor(() => expect(first.result.current.isSuccess).toBe(true));
    const second = renderHook(() => useProfileConfiguration("claude"), {
      wrapper,
    });
    expect(second.result.current.isSuccess).toBe(true);
    expect(migrate).toHaveBeenCalledTimes(1);
    await act(() =>
      client.invalidateQueries({
        queryKey: ["profile-configuration", "claude"],
      }),
    );
    expect(migrate).toHaveBeenCalledTimes(2);
  });

  it("reports partial migration, refreshes preserved records and retries explicitly", async () => {
    const client = createTestQueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const wrapper = ({ children }: PropsWithChildren) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    migrate.mockResolvedValueOnce({
      profiles: [{ id: "preserved" }],
      warnings: ["Invalid provider config"],
    });
    const { result } = renderHook(() => useProfileConfiguration("codex"), {
      wrapper,
    });
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(result.current.error?.message).toBe("Invalid provider config");
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["profiles"] });
    expect(migrate).toHaveBeenCalledTimes(1);
    migrate.mockResolvedValueOnce({ profiles: [], warnings: [] });
    await act(() => result.current.refetch());
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
  });
});
