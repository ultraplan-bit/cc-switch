import { useState } from "react";
import { useTranslation } from "react-i18next";
import { FullScreenPanel } from "@/components/common/FullScreenPanel";
import { ProfilesPage } from "@/components/profiles/ProfilesPage";
import { ToolErrorMessage } from "@/components/settings/ToolErrorMessage";
import { Button } from "@/components/ui/button";
import { toolDisplayName, type useToolManagement } from "./useToolManagement";

export interface ProjectEnvironmentTarget {
  tool: "claude" | "codex";
  projectPath?: string;
}

export function ProjectEnvironmentPanel({
  target,
  tools,
  onClose,
}: {
  target: ProjectEnvironmentTarget;
  tools: ReturnType<typeof useToolManagement>;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [checking, setChecking] = useState(false);
  const info = tools.toolVersionByName.get(target.tool);
  const report = tools.installReports?.[target.tool];
  const defaultInstall =
    report?.installs.find((install) => install.is_path_default) ??
    (report?.installs.length === 1 ? report.installs[0] : undefined);
  // Installation enumeration is host-side; WSL execution is verified by its version probe.
  const installationVerified =
    info?.env_type === "wsl" ||
    Boolean(defaultInstall?.runnable && !defaultInstall.error);
  const busy = tools.busyTools.has(target.tool);
  const loading =
    checking ||
    Boolean(tools.loadingTools[target.tool]) ||
    (tools.isLoadingTools && !info);
  const errors = [
    ...new Set(
      [
        tools.toolCheckErrors[target.tool],
        tools.installationCheckErrors[target.tool],
        tools.toolActionErrors[target.tool],
        info?.error,
        defaultInstall?.error,
      ].filter((error): error is string => Boolean(error)),
    ),
  ];
  const ready =
    Boolean(info?.version) &&
    !info?.installed_but_broken &&
    report !== undefined &&
    installationVerified &&
    errors.length === 0 &&
    !loading &&
    !busy;
  const status = busy
    ? t("profiles.environment.busy")
    : loading
      ? t("common.loading")
      : info?.installed_but_broken ||
          (info?.env_type !== "wsl" &&
            defaultInstall &&
            !defaultInstall.runnable)
        ? t("appsPage.notRunnable")
        : errors.length > 0 ||
            !info ||
            !report ||
            (info.version && !installationVerified)
          ? t("profiles.environment.checkFailed")
          : ready
            ? t("profiles.environment.ready")
            : t("common.notInstalled");

  return (
    <FullScreenPanel
      isOpen
      title={t("profiles.environment.projectConfiguration")}
      subtitle={toolDisplayName(target.tool)}
      backLabel={t("nav.apps")}
      onClose={onClose}
      contentClassName="mx-auto w-full max-w-5xl"
    >
      <section
        aria-label={t("profiles.environment.title")}
        className="space-y-5"
      >
        <p className="text-sm text-fg-2">
          {t("profiles.environment.projectHint")}
        </p>
        <div className="space-y-3 rounded-panel border border-border p-4">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p role="status" className="text-sm font-medium">
              {status}
            </p>
            <Button
              variant="outline"
              disabled={loading || busy}
              onClick={async () => {
                setChecking(true);
                try {
                  await tools.checkTool(target.tool);
                } finally {
                  setChecking(false);
                }
              }}
            >
              {t("profiles.environment.recheck")}
            </Button>
          </div>
          {errors.map((error) => (
            <ToolErrorMessage key={error} message={error} />
          ))}
          {info?.env_type === "wsl" && (
            <p className="text-sm text-fg-2">
              {t("profiles.environment.wslHint", {
                distro: info.wsl_distro || "WSL",
              })}
            </p>
          )}
        </div>
        <div className="space-y-3 rounded-panel border border-border p-4">
          {!ready && (
            <p className="text-sm text-warning-text">
              {t("profiles.environment.prepareHint")}
            </p>
          )}
          <ProfilesPage
            activeApp={target.tool}
            projectOnly
            initialProjectPath={target.projectPath}
            deploymentDisabled={!ready}
          />
        </div>
      </section>
    </FullScreenPanel>
  );
}
