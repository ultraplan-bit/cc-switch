import { useState } from "react";
import { isWindows } from "@/lib/platform";
import type {
  ProfileProject,
  ProfileScope,
  ProjectScope,
} from "@/lib/api/profiles";

const STORAGE_KEY = "cc-switch:profiles:location:v2";

type StoredLocation = { mode: "global" | "project"; path?: string };
type Stored = Partial<Record<ProfileScope, StoredLocation>>;

/** The registry stores normalized paths; a freshly picked one can differ in slashes or case. */
export function samePath(a: string, b: string) {
  const normalize = (path: string) =>
    (isWindows() ? path.replace(/\\/g, "/").toLowerCase() : path).replace(
      /\/+$/,
      "",
    );
  return normalize(a) === normalize(b);
}

function read(): Stored {
  try {
    const saved: unknown = JSON.parse(
      localStorage.getItem(STORAGE_KEY) ?? "{}",
    );
    if (!saved || typeof saved !== "object") return {};
    const result: Stored = {};
    for (const scope of ["claude", "codex", "claude-desktop"] as const) {
      const item = (saved as Record<string, unknown>)[scope];
      if (!item || typeof item !== "object") continue;
      const { mode, path } = item as Record<string, unknown>;
      result[scope] = {
        mode: mode === "project" ? "project" : "global",
        path: typeof path === "string" ? path : undefined,
      };
    }
    return result;
  } catch {
    return {};
  }
}

export type ProfileLocation =
  | { mode: "global"; project?: undefined }
  | { mode: "project"; project: ProfileProject | undefined };

/**
 * Which location the page is looking at, per Agent. This is a viewing preference
 * only: changing it never applies or deploys anything.
 */
export function useProfileLocation(
  scope: ProfileScope,
  projects: ProfileProject[] | undefined,
  options: { projectOnly?: boolean; initialProjectPath?: string } = {},
) {
  const [stored, setStored] = useState<Stored>(() => {
    const saved = read();
    return options.initialProjectPath === undefined
      ? saved
      : {
          ...saved,
          [scope]: { mode: "project", path: options.initialProjectPath },
        };
  });
  const own = stored[scope];
  const projectMode =
    scope !== "claude-desktop" &&
    (options.projectOnly === true || own?.mode === "project");

  const update = (next: StoredLocation) => {
    const merged = { ...stored, [scope]: next };
    setStored(merged);
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(merged));
    } catch {
      // Remembering a viewing preference must not block configuration.
    }
  };

  const location: ProfileLocation = projectMode
    ? {
        mode: "project",
        // A remembered project that was removed falls back to the first one.
        project:
          projects?.find(
            (item) => own?.path && samePath(item.path, own.path),
          ) ?? projects?.[0],
      }
    : { mode: "global" };

  return {
    location,
    setMode: (mode: "global" | "project") => update({ ...own, mode }),
    selectProject: (path: string) => update({ mode: "project", path }),
  };
}

export function isProjectScope(scope: ProfileScope): scope is ProjectScope {
  return scope !== "claude-desktop";
}
