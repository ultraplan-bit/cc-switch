import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Check,
  FolderCog,
  Pencil,
  Trash2,
  Unlink,
  Upload,
  X,
} from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { HoverTip } from "@/components/ui/hover-tip";
import { Input } from "@/components/ui/input";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { settingsApi } from "@/lib/api";
import {
  useDeployProfileToProjectMutation,
  useDeleteProfileMutation,
  useProfilesQuery,
  useSetProfileProjectPathMutation,
  useUpdateProfileMutation,
} from "@/lib/query/profiles";
import { toast } from "@/lib/toast";
import { extractErrorMessage } from "@/utils/errorUtils";
import type {
  ProfileScope,
  ProjectDeployConflict,
  ProjectDeployResolution,
} from "@/lib/api/profiles";

interface ProfileManageDialogProps {
  isOpen: boolean;
  scope: ProfileScope;
  onClose: () => void;
}

type PendingConfirm = {
  id: string;
  name: string;
};

/**
 * 项目管理对话框（纯快照式）
 *
 * 项目列表全应用共享，重命名/删除作用于共享实体。
 * 快照内容由切换时的自动保存维护，不提供手动重拍入口。
 */
export function ProfileManageDialog({
  isOpen,
  scope,
  onClose,
}: ProfileManageDialogProps) {
  const { t } = useTranslation();
  const { data } = useProfilesQuery();
  const updateMutation = useUpdateProfileMutation();
  const deleteMutation = useDeleteProfileMutation();
  const setProjectPathMutation = useSetProfileProjectPathMutation();
  const deployMutation = useDeployProfileToProjectMutation();

  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingName, setEditingName] = useState("");
  const [confirm, setConfirm] = useState<PendingConfirm | null>(null);
  const [confirmUnbind, setConfirmUnbind] = useState<PendingConfirm | null>(
    null,
  );
  const [pendingDeploy, setPendingDeploy] = useState<{
    id: string;
    scope: ProfileScope;
    conflicts: ProjectDeployConflict[];
  } | null>(null);

  const profiles = data?.profiles ?? [];

  const startRename = (id: string, name: string) => {
    setEditingId(id);
    setEditingName(name);
  };

  const cancelRename = () => {
    setEditingId(null);
    setEditingName("");
  };

  const saveRename = () => {
    const name = editingName.trim();
    if (!name || !editingId) return;
    updateMutation.mutate({ id: editingId, name }, { onSuccess: cancelRename });
  };

  const handleConfirm = () => {
    if (!confirm) return;
    deleteMutation.mutate(confirm.id);
    setConfirm(null);
  };

  const pickProjectPath = async (id: string, currentPath?: string | null) => {
    try {
      const projectPath = await settingsApi.pickDirectory(
        currentPath ?? undefined,
      );
      if (projectPath !== null) {
        setProjectPathMutation.mutate({ id, projectPath });
      }
    } catch (error) {
      toast.error(
        t("profiles.projectPath.pickFailed", {
          detail: extractErrorMessage(error) || t("common.unknown"),
        }),
      );
    }
  };

  const handleUnbind = () => {
    if (!confirmUnbind) return;
    setProjectPathMutation.mutate(
      { id: confirmUnbind.id, projectPath: null },
      { onSettled: () => setConfirmUnbind(null) },
    );
  };

  const deployToProject = async (
    id: string,
    resolution: ProjectDeployResolution | null = null,
    targetScope: ProfileScope = scope,
  ) => {
    try {
      const result = await deployMutation.mutateAsync({
        id,
        scope: targetScope,
        resolution,
      });
      if (result.status === "needsResolution") {
        setPendingDeploy({
          id,
          scope: targetScope,
          conflicts: result.conflicts,
        });
        return;
      }
      setPendingDeploy(null);
      if (result.status === "skipped") {
        toast.info(t("profiles.projectDeploy.skipped"));
        return;
      }
      toast.success(t("profiles.projectDeploy.success"));
      if (result.warnings.length > 0) {
        toast.warning(result.warnings.join("\n"), { duration: 7000 });
      }
      if (
        result.warnings.some((warning) =>
          warning.startsWith("sensitiveMcpSkipped:"),
        )
      ) {
        toast.warning(t("profiles.projectDeploy.sensitiveMcpSkipped"), {
          duration: 7000,
        });
      }
      if (scope === "codex") {
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

  return (
    <>
      <Dialog
        open={isOpen}
        onOpenChange={(open) => {
          if (!open) {
            cancelRename();
            onClose();
          }
        }}
      >
        <DialogContent className="max-w-md">
          <DialogHeader className="space-y-3 border-b-0 bg-transparent pb-0">
            <DialogTitle>{t("profiles.manageTitle")}</DialogTitle>
            <DialogDescription>
              {t("profiles.manageDescription")}
            </DialogDescription>
          </DialogHeader>
          <div className="max-h-[50vh] space-y-1 overflow-y-auto px-6 pb-4 pt-3">
            {profiles.length === 0 && (
              <div className="py-4 text-center text-sm text-fg-2">
                {t("profiles.empty")}
              </div>
            )}
            {profiles.map((profile) => (
              <div
                key={profile.id}
                className="flex items-center gap-1.5 rounded-md px-2 py-1.5 hover:bg-subtle"
              >
                {editingId === profile.id ? (
                  <>
                    <Input
                      value={editingName}
                      onChange={(e) => setEditingName(e.target.value)}
                      className="h-7 flex-1"
                      autoFocus
                      onKeyDown={(e) => {
                        if (e.key === "Enter") saveRename();
                        if (e.key === "Escape") cancelRename();
                      }}
                    />
                    <HoverTip content={t("common.confirm")}>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7"
                        aria-label={t("common.confirm")}
                        onClick={saveRename}
                        disabled={
                          !editingName.trim() || updateMutation.isPending
                        }
                      >
                        <Check className="h-3.5 w-3.5" />
                      </Button>
                    </HoverTip>
                    <HoverTip content={t("common.cancel")}>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7"
                        aria-label={t("common.cancel")}
                        onClick={cancelRename}
                      >
                        <X className="h-3.5 w-3.5" />
                      </Button>
                    </HoverTip>
                  </>
                ) : (
                  <>
                    <div className="flex min-w-0 flex-1 flex-col">
                      <span className="truncate text-sm">{profile.name}</span>
                      {profile.projectPath ? (
                        <span
                          className="truncate text-xs text-fg-2"
                          title={profile.projectPath}
                        >
                          {profile.projectPath}
                        </span>
                      ) : (
                        <span className="text-xs text-fg-2">
                          {t("profiles.projectPath.none")}
                        </span>
                      )}
                    </div>
                    <div className="flex shrink-0 items-center gap-0.5">
                      {profile.projectPath &&
                        (scope === "claude" || scope === "codex") && (
                          <HoverTip
                            content={t("profiles.projectDeploy.button")}
                          >
                            <Button
                              variant="ghost"
                              size="icon"
                              className="h-7 w-7"
                              aria-label={t("profiles.projectDeploy.button")}
                              onClick={() => deployToProject(profile.id)}
                              disabled={deployMutation.isPending}
                            >
                              <Upload className="h-3.5 w-3.5" />
                            </Button>
                          </HoverTip>
                        )}
                      <HoverTip
                        content={t(
                          profile.projectPath
                            ? "profiles.projectPath.change"
                            : "profiles.projectPath.pick",
                        )}
                      >
                        <Button
                          variant="ghost"
                          size="icon"
                          className="h-7 w-7"
                          aria-label={t(
                            profile.projectPath
                              ? "profiles.projectPath.change"
                              : "profiles.projectPath.pick",
                          )}
                          onClick={() =>
                            pickProjectPath(profile.id, profile.projectPath)
                          }
                          disabled={setProjectPathMutation.isPending}
                        >
                          <FolderCog className="h-3.5 w-3.5" />
                        </Button>
                      </HoverTip>
                      {profile.projectPath && (
                        <HoverTip content={t("profiles.projectPath.unbind")}>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="h-7 w-7"
                            aria-label={t("profiles.projectPath.unbind")}
                            onClick={() =>
                              setConfirmUnbind({
                                id: profile.id,
                                name: profile.name,
                              })
                            }
                            disabled={setProjectPathMutation.isPending}
                          >
                            <Unlink className="h-3.5 w-3.5" />
                          </Button>
                        </HoverTip>
                      )}
                      <HoverTip content={t("profiles.rename")}>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="h-7 w-7"
                          aria-label={t("profiles.rename")}
                          onClick={() => startRename(profile.id, profile.name)}
                        >
                          <Pencil className="h-3.5 w-3.5" />
                        </Button>
                      </HoverTip>
                      <HoverTip content={t("profiles.delete")}>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="h-7 w-7"
                          aria-label={t("profiles.delete")}
                          onClick={() =>
                            setConfirm({
                              id: profile.id,
                              name: profile.name,
                            })
                          }
                        >
                          <Trash2 className="h-3.5 w-3.5 text-destructive" />
                        </Button>
                      </HoverTip>
                    </div>
                  </>
                )}
              </div>
            ))}
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => {
                cancelRename();
                onClose();
              }}
            >
              {t("common.close")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {confirm && (
        <ConfirmDialog
          isOpen
          title={t("profiles.deleteConfirmTitle")}
          message={t("profiles.deleteConfirmMessage", { name: confirm.name })}
          variant="destructive"
          onConfirm={handleConfirm}
          onCancel={() => setConfirm(null)}
        />
      )}
      {confirmUnbind && (
        <ConfirmDialog
          isOpen
          title={t("profiles.projectPath.unbindConfirmTitle")}
          message={t("profiles.projectPath.unbindConfirmMessage", {
            name: confirmUnbind.name,
          })}
          pending={setProjectPathMutation.isPending}
          onConfirm={handleUnbind}
          onCancel={() => setConfirmUnbind(null)}
        />
      )}
      {pendingDeploy && (
        <Dialog
          open
          onOpenChange={(open) => {
            if (!open && !deployMutation.isPending) setPendingDeploy(null);
          }}
        >
          <DialogContent className="max-w-lg" zIndex="alert">
            <DialogHeader>
              <DialogTitle>
                {t("profiles.projectDeploy.conflictTitle")}
              </DialogTitle>
              <DialogDescription>
                {t("profiles.projectDeploy.conflictMessage", {
                  files: pendingDeploy.conflicts.length,
                })}
              </DialogDescription>
            </DialogHeader>
            <div className="max-h-[35vh] space-y-2 overflow-y-auto px-6 pb-2">
              {pendingDeploy.conflicts.map((conflict) => (
                <div key={conflict.path} className="rounded-md bg-subtle p-2">
                  <code className="block break-all text-xs">
                    {conflict.path}
                  </code>
                  <p className="mt-1 text-xs text-fg-2">{conflict.reason}</p>
                </div>
              ))}
            </div>
            <DialogFooter className="flex-wrap gap-2">
              <Button
                type="button"
                variant="outline"
                onClick={() =>
                  deployToProject(pendingDeploy.id, "skip", pendingDeploy.scope)
                }
                disabled={deployMutation.isPending}
              >
                {t("profiles.projectDeploy.skip")}
              </Button>
              <Button
                type="button"
                variant="outline"
                onClick={() =>
                  deployToProject(
                    pendingDeploy.id,
                    "keepCurrent",
                    pendingDeploy.scope,
                  )
                }
                disabled={deployMutation.isPending}
              >
                {t("profiles.projectDeploy.keepCurrent")}
              </Button>
              <Button
                type="button"
                variant="destructive"
                onClick={() =>
                  deployToProject(
                    pendingDeploy.id,
                    "overwrite",
                    pendingDeploy.scope,
                  )
                }
                disabled={deployMutation.isPending}
              >
                {deployMutation.isPending
                  ? t("profiles.projectDeploy.pending")
                  : t("profiles.projectDeploy.overwrite")}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}
    </>
  );
}
