import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { useDeployProfileToProjectMutation } from "@/lib/query/profiles";
import type {
  ProfileScope,
  ProjectDeployConflict,
  ProjectDeployResolution,
} from "@/lib/api/profiles";
import { toast } from "@/lib/toast";
import { extractErrorMessage } from "@/utils/errorUtils";

function displayWarnings(warnings: string[], sensitiveMessage: string) {
  return [
    ...new Set(
      warnings.map((warning) =>
        warning.startsWith("sensitiveMcpSkipped:") ? sensitiveMessage : warning,
      ),
    ),
  ];
}

export function useProjectProfileDeployment(
  scope: ProfileScope,
  canDeploy = true,
) {
  const { t } = useTranslation();
  const mutation = useDeployProfileToProjectMutation();
  const [pendingDeploy, setPendingDeploy] = useState<{
    id: string;
    scope: ProfileScope;
    projectPath: string;
    conflicts: ProjectDeployConflict[];
    warnings: string[];
  } | null>(null);

  const deployToProject = async (
    id: string,
    projectPath: string,
    resolution: ProjectDeployResolution | null = null,
    targetScope: ProfileScope = scope,
  ) => {
    if (!canDeploy && resolution !== "skip") return;
    try {
      const result = await mutation.mutateAsync({
        id,
        scope: targetScope,
        projectPath,
        resolution,
      });
      if (result.status === "needsResolution") {
        setPendingDeploy({
          id,
          scope: targetScope,
          projectPath,
          conflicts: result.conflicts,
          warnings: result.warnings,
        });
        return;
      }
      setPendingDeploy(null);
      if (result.status === "skipped") {
        toast.info(t("profiles.projectDeploy.skipped"));
        return;
      }
      const visibleWarnings = displayWarnings(
        result.warnings,
        t("profiles.projectDeploy.sensitiveMcpSkipped"),
      );
      if (visibleWarnings.length > 0) {
        toast.warning(t("profiles.projectDeploy.completedWithNotes"), {
          description: visibleWarnings.join("\n"),
          duration: 10000,
          closeButton: true,
        });
      } else {
        toast.success(t("profiles.projectDeploy.success"));
      }
      if (targetScope === "codex") {
        toast.info(t("profiles.projectDeploy.codexTrustWarning"), {
          duration: 7000,
        });
      }
    } catch (error) {
      toast.error(
        t("profiles.projectDeploy.failed", {
          detail: extractErrorMessage(error) || t("common.unknown"),
        }),
      );
    }
  };

  return {
    deployToProject,
    pendingDeploy,
    setPendingDeploy,
    isPending: mutation.isPending,
  };
}

export function ProjectProfileConflictDialog({
  deployment,
  deploymentDisabled = false,
}: {
  deployment: ReturnType<typeof useProjectProfileDeployment>;
  deploymentDisabled?: boolean;
}) {
  const { t } = useTranslation();
  const { pendingDeploy, setPendingDeploy, isPending, deployToProject } =
    deployment;
  if (!pendingDeploy) return null;

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !isPending) setPendingDeploy(null);
      }}
    >
      <DialogContent className="max-w-lg" zIndex="alert">
        <DialogHeader>
          <DialogTitle>{t("profiles.projectDeploy.conflictTitle")}</DialogTitle>
          <DialogDescription>
            {t("profiles.projectDeploy.conflictMessage", {
              files: pendingDeploy.conflicts.length,
            })}
          </DialogDescription>
          <code className="block break-all text-xs text-fg-2">
            {pendingDeploy.projectPath}
          </code>
        </DialogHeader>
        <div className="max-h-[35vh] space-y-2 overflow-y-auto px-6 pb-2">
          {pendingDeploy.warnings.length > 0 && (
            <div
              role="status"
              className="rounded-md border border-border p-3 text-sm"
            >
              <p className="font-medium">{t("profiles.projectDeploy.notes")}</p>
              <ul className="mt-1 list-disc space-y-1 pl-4 text-fg-2">
                {displayWarnings(
                  pendingDeploy.warnings,
                  t("profiles.projectDeploy.sensitiveMcpSkipped"),
                ).map((warning) => (
                  <li key={warning} className="break-words">
                    {warning}
                  </li>
                ))}
              </ul>
            </div>
          )}
          {pendingDeploy.conflicts.map((conflict) => (
            <div key={conflict.path} className="rounded-md bg-subtle p-2">
              <code className="block break-all text-xs">{conflict.path}</code>
              <p className="mt-1 text-xs text-fg-2">{conflict.reason}</p>
            </div>
          ))}
        </div>
        <DialogFooter className="flex-wrap gap-2">
          {(
            [
              ["skip", "skip", "outline"],
              ["keepCurrent", "keepCurrent", "outline"],
              ["overwrite", "overwrite", "destructive"],
            ] as const
          ).map(([resolution, label, variant]) => (
            <Button
              key={resolution}
              type="button"
              variant={variant}
              onClick={() =>
                deployToProject(
                  pendingDeploy.id,
                  pendingDeploy.projectPath,
                  resolution,
                  pendingDeploy.scope,
                )
              }
              disabled={
                isPending || (deploymentDisabled && resolution !== "skip")
              }
            >
              {isPending && resolution === "overwrite"
                ? t("profiles.projectDeploy.pending")
                : t(`profiles.projectDeploy.${label}`)}
            </Button>
          ))}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
