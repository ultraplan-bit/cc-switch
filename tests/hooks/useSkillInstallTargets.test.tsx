import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useSkillInstallTargets } from "@/components/skills/useSkillInstallTargets";

const toggle = vi.hoisted(() => vi.fn());
vi.mock("@/hooks/useSkills", () => ({
  useToggleSkillApp: () => ({ mutateAsync: toggle }),
}));

describe("useSkillInstallTargets", () => {
  beforeEach(() => {
    localStorage.removeItem("cc-switch:skills:install-to");
    toggle.mockReset().mockResolvedValue(true);
  });

  it("remembers an empty selection and installs without enabling an Agent", async () => {
    const first = renderHook(() => useSkillInstallTargets(["claude", "codex"]));
    act(() => first.result.current.setTargets([]));
    first.unmount();
    const { result } = renderHook(() =>
      useSkillInstallTargets(["claude", "codex"]),
    );
    const install = vi.fn().mockResolvedValue([{ id: "skill" }]);

    expect(result.current.targets).toEqual([]);
    await result.current.installTo(install);
    expect(install).toHaveBeenCalledWith(null);
    expect(toggle).not.toHaveBeenCalled();
  });

  it("does not fall back to a different Agent when selected targets are hidden", async () => {
    localStorage.setItem("cc-switch:skills:install-to", '["claude"]');
    const { result } = renderHook(() => useSkillInstallTargets(["gemini"]));
    const install = vi.fn().mockResolvedValue([{ id: "skill" }]);

    await result.current.installTo(install);
    expect(result.current.targets).toEqual([]);
    expect(install).toHaveBeenCalledWith(null);
    expect(toggle).not.toHaveBeenCalled();
  });

  it("keeps selected Agent installation and reports later enable failures", async () => {
    const { result } = renderHook(() =>
      useSkillInstallTargets(["claude", "codex"]),
    );
    const install = vi.fn().mockResolvedValue([{ id: "skill" }]);
    const error = new Error("Codex directory unavailable");
    toggle.mockRejectedValue(error);

    const outcome = await result.current.installTo(install);
    expect(install).toHaveBeenCalledWith("claude");
    expect(toggle).toHaveBeenCalledWith({
      id: "skill",
      app: "codex",
      enabled: true,
    });
    expect(outcome.failures).toEqual([{ id: "skill", app: "codex", error }]);
  });
});
