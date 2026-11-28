import { beforeEach, describe, expect, it } from "vitest";
import {
  parseView,
  readStoredView,
  storeView,
  VIEW_STORAGE_KEY,
} from "@/lib/navigation";

describe("current navigation views", () => {
  beforeEach(() => localStorage.clear());

  it.each(["providers", "profiles", "openclawConfig", "settings"] as const)(
    "restores the current %s view",
    (view) => {
      storeView(view);
      expect(readStoredView()).toBe(view);
    },
  );

  it.each(["openclawEnv", "openclawTools", "openclawAgents", "unknown"])(
    "rejects %s without rewriting the saved value",
    (view) => {
      expect(parseView(view)).toBeNull();
      localStorage.setItem(VIEW_STORAGE_KEY, view);
      expect(readStoredView()).toBe("providers");
      expect(localStorage.getItem(VIEW_STORAGE_KEY)).toBe(view);
    },
  );
});
