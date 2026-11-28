import { useEffect, useState } from "react";
import type { AppId } from "@/lib/api";
import type { AppMode } from "@/types/proxy";
import { useAppMode } from "@/lib/query/proxy";

/** Resolve the app mode before mounting a form, then keep its layout for that edit session. */
export function useProviderConnectionMode(
  appId: AppId,
  open: boolean,
  modeView?: AppMode,
  providerId = "new",
) {
  const supportsConnections = appId === "claude" || appId === "codex";
  const appMode = useAppMode(
    appId,
    supportsConnections && open && modeView === undefined,
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
    if (supportsConnections && modeView === undefined && appMode.isPending)
      return;
    setResolved((current) =>
      current?.key === key
        ? current
        : {
            key,
            connectionsOnly:
              supportsConnections &&
              (modeView ?? appMode.data?.mode) !== "stack",
          },
    );
  }, [
    open,
    key,
    supportsConnections,
    modeView,
    appMode.isPending,
    appMode.data?.mode,
  ]);
  return {
    connectionsOnly: resolved?.key === key && resolved.connectionsOnly,
    pending: open && supportsConnections && resolved?.key !== key,
  };
}
