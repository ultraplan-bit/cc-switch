import { useMemo } from "react";
import type { AppId } from "@/lib/api/types";
import { DEFAULT_VISIBLE_APPS } from "@/config/appConfig";
import { useSettingsQuery } from "@/lib/query";
import { useAgentAvailability } from "@/hooks/useAgentAvailability";

/**
 * 配置入口只列已安装且已启用的 Agent；探测期间不猜测可用目标。
 * 只决定界面和操作目标，不改写已有资源绑定或用户的显示偏好。
 */
export function useVisibleAppIds<T extends AppId>(appIds: readonly T[]): T[] {
  const { data: settings, isPending: isSettingsPending } = useSettingsQuery();
  const { installedApps, isPending: isAvailabilityPending } =
    useAgentAvailability();
  const visibleApps = settings?.visibleApps;
  const hasSettings = settings !== undefined;
  return useMemo(() => {
    if (!hasSettings || isSettingsPending || isAvailabilityPending) return [];
    const visible = { ...DEFAULT_VISIBLE_APPS, ...visibleApps };
    return appIds.filter((app) => visible[app] && installedApps[app] === true);
  }, [
    appIds,
    hasSettings,
    visibleApps,
    installedApps,
    isSettingsPending,
    isAvailabilityPending,
  ]);
}
