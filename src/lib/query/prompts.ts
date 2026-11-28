import type { AppId } from "@/lib/api";

export const promptKeys = {
  all: ["prompts"] as const,
  list: () => ["prompts", "library"] as const,
  target: (app: AppId) => ["prompts", "target", app] as const,
};
