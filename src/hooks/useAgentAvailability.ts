import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { providersApi } from "@/lib/api/providers";
import { settingsApi } from "@/lib/api/settings";
import type { AppId } from "@/lib/api/types";
import {
  TOOL_APP_IDS,
  TOOL_NAMES,
  useInstalledTools,
} from "@/components/apps/useToolManagement";

/** Installation presence only; sidebar preferences and project readiness remain separate. */
export function useAgentAvailability() {
  const { installedTools, isPending } = useInstalledTools();
  const desktop = useQuery({
    queryKey: ["claudeDesktopStatus"],
    queryFn: () => providersApi.getClaudeDesktopStatus(),
    staleTime: 30_000,
    retry: false,
    networkMode: "always",
  });
  const workbuddy = useQuery({
    queryKey: ["config-status", "workbuddy"],
    queryFn: () => settingsApi.getConfigStatus("workbuddy"),
    staleTime: 30_000,
    retry: false,
    networkMode: "always",
  });
  const installedApps = useMemo(() => {
    const apps: Partial<Record<AppId, boolean>> = {};
    for (const tool of TOOL_NAMES) {
      apps[TOOL_APP_IDS[tool]] = installedTools[tool];
    }
    apps["claude-desktop"] = desktop.data?.installed;
    apps.workbuddy = workbuddy.data?.exists;
    return apps;
  }, [installedTools, desktop.data?.installed, workbuddy.data?.exists]);

  return {
    installedApps,
    isPending: isPending || desktop.isPending || workbuddy.isPending,
  };
}
