import { beforeEach, describe, expect, it, vi } from "vitest";
import { checkAllEnvConflicts } from "@/lib/api/env";

const invoke = vi.hoisted(() => vi.fn().mockResolvedValue([]));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

describe("environment conflict scope", () => {
  beforeEach(() => invoke.mockClear());

  it("checks only available Agents that support conflict detection", async () => {
    expect(await checkAllEnvConflicts(["codex", "pi"])).toEqual({ codex: [] });
    expect(invoke.mock.calls).toEqual([
      ["check_env_conflicts", { app: "codex" }],
    ]);
  });

  it("does not check every Agent for an empty selection", async () => {
    expect(await checkAllEnvConflicts([])).toEqual({});
    expect(invoke).not.toHaveBeenCalled();
  });
});
