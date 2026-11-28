import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { RoutingSection } from "@/components/settings/sections/RoutingSection";
import type { AppMode } from "@/types/proxy";

const state = vi.hoisted(() => ({
  apps: ["codex"] as string[],
  modes: {} as Record<string, { mode: AppMode; routeProviderId?: string }>,
  running: false,
  providersQueryOptions: vi.fn((app: string) => ({
    queryKey: ["providers", app],
  })),
  stop: vi.fn(),
  stopWithRestore: vi.fn(),
}));

vi.mock("@/components/mcp/useVisibleAppIds", () => ({
  useVisibleAppIds: (ids: readonly string[]) =>
    ids.filter((app) => state.apps.includes(app)),
}));
vi.mock("@/lib/query", () => ({
  providersQueryOptions: state.providersQueryOptions,
}));
vi.mock("@tanstack/react-query", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tanstack/react-query")>()),
  useQueries: ({ queries }: { queries: { queryKey: string[] }[] }) =>
    queries.map(({ queryKey }) => ({
      data:
        queryKey[2] === "mode"
          ? (state.modes[queryKey[1]] ?? { mode: "direct" })
          : { providers: { route: { name: `${queryKey[1]} provider` } } },
    })),
}));
vi.mock("@/lib/query/proxy", () => ({
  useProxyStatusQuery: () => ({ data: { running: state.running } }),
  useGlobalProxyConfig: () => ({ data: undefined }),
  useUpdateGlobalProxyConfig: () => ({ mutateAsync: vi.fn() }),
  useProxyClientHost: () => ({ data: "" }),
  useSetProxyClientHost: () => ({ mutateAsync: vi.fn() }),
}));
vi.mock("@/hooks/useProxyStatus", () => ({
  useProxyStatus: () => ({
    startProxyServer: vi.fn(),
    stopProxyServer: state.stop,
    stopWithRestore: state.stopWithRestore,
    isPending: false,
  }),
}));
vi.mock("@/components/proxy/AutoFailoverConfigPanel", () => ({
  AutoFailoverConfigPanel: ({ appType }: { appType: string }) => (
    <div data-testid="failover-config">{appType}</div>
  ),
}));
vi.mock("@/components/settings/RectifierConfigPanel", () => ({
  RectifierConfigPanel: () => null,
}));

describe("RoutingSection Agent availability", () => {
  beforeEach(() => {
    state.apps = ["codex"];
    state.modes = {};
    state.running = false;
    state.providersQueryOptions.mockClear();
  });

  it("only exposes available routing Agents and replaces a stale failover selection", () => {
    state.apps = ["codex", "gemini"];
    const view = render(<RoutingSection onOpenApp={vi.fn()} />);
    expect(screen.getByTestId("failover-config")).toHaveTextContent("codex");
    expect(screen.queryByRole("button", { name: "Claude Code" })).toBeNull();
    expect(state.providersQueryOptions.mock.calls.map(([app]) => app)).toEqual([
      "codex",
      "gemini",
    ]);

    fireEvent.click(screen.getByRole("button", { name: "Gemini CLI" }));
    expect(screen.getByTestId("failover-config")).toHaveTextContent("gemini");
    state.apps = ["codex"];
    view.rerender(<RoutingSection onOpenApp={vi.fn()} />);
    expect(screen.getByTestId("failover-config")).toHaveTextContent("codex");
    expect(screen.queryByRole("button", { name: "Gemini CLI" })).toBeNull();

    state.apps = [];
    view.rerender(<RoutingSection onOpenApp={vi.fn()} />);
    expect(screen.queryByTestId("failover-config")).toBeNull();
  });

  it("hides unavailable running Agents without permitting a global stop to affect them", () => {
    state.running = true;
    state.modes = {
      claude: { mode: "route", routeProviderId: "route" },
      codex: { mode: "route", routeProviderId: "route" },
    };
    const onOpenApp = vi.fn();
    render(<RoutingSection onOpenApp={onOpenApp} />);
    expect(screen.queryByText("Claude Code")).toBeNull();
    fireEvent.click(
      screen.getByRole("button", { name: "routingSettings.goTo" }),
    );
    expect(onOpenApp).toHaveBeenCalledWith("codex");
    expect(
      screen.getByRole("button", { name: "routingSettings.stop" }),
    ).toHaveAttribute("aria-disabled", "true");
    expect(
      screen.queryByRole("button", { name: "routingSettings.exitAll" }),
    ).toBeNull();
    expect(state.stop).not.toHaveBeenCalled();
    expect(state.stopWithRestore).not.toHaveBeenCalled();
  });
});
