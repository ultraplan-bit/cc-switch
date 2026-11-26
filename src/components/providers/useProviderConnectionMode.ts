import { useEffect, useState } from "react";
import type { AppId } from "@/lib/api";
import type { AppMode } from "@/types/proxy";
import { useProfileConfiguration } from "@/lib/query/profiles";
import { useAppMode } from "@/lib/query/proxy";

/** Resolve migration before mounting a form, then keep its layout for that edit session. */
export function useProviderConnectionMode(
  appId: AppId,
  open: boolean,
  modeView?: AppMode,
  providerId = "new",
) {
  const scope = appId === "claude" || appId === "codex" ? appId : undefined;
  const migration = useProfileConfiguration(scope, open);
  const appMode = useAppMode(
    appId,
    Boolean(scope) && open && modeView === undefined,
  );
  const key = `${appId}:${providerId}`;
  const [resolved, setResolved] = useState<{
    key: string;
    connectionsOnly: boolean;
  } | null>(null);
  useEffect(() => {
    if (!open) {
      setResolved(null);
      return;
    }
    if (
      scope &&
      (migration.isPending ||
        migration.isFetching ||
        (modeView === undefined && appMode.isPending))
    )
      return;
    setResolved((current) =>
      current?.key === key
        ? current
        : {
            key,
            connectionsOnly:
              Boolean(scope) &&
              migration.isSuccess &&
              (modeView ?? appMode.data?.mode) !== "stack",
          },
    );
  }, [
    open,
    key,
    scope,
    migration.isPending,
    migration.isFetching,
    migration.isSuccess,
    modeView,
    appMode.isPending,
    appMode.data?.mode,
  ]);
  return {
    connectionsOnly: resolved?.key === key && resolved.connectionsOnly,
    pending: open && Boolean(scope) && resolved?.key !== key,
    migrationError: scope && migration.isError ? migration.error : null,
  };
}
