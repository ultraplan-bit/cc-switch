import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Check,
  Copy,
  FolderCog,
  MoreHorizontal,
  Pencil,
  RefreshCw,
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { settingsApi } from "@/lib/api";
import {
  useDeleteProfileMutation,
  useDuplicateProfileMutation,
  useProfilesQuery,
  useSetProfileProjectPathMutation,
  useUpdateProfileMutation,
} from "@/lib/query/profiles";
import { toast } from "@/lib/toast";
import { extractErrorMessage } from "@/utils/errorUtils";
import { hasScopeSnapshot } from "./scope";
import {
  ProjectProfileConflictDialog,
  useProjectProfileDeployment,
} from "./ProjectProfileDeployment";
import { ProfileEditPanel } from "./ProfileEditPanel";
import type { Profile, ProfileScope } from "@/lib/api/profiles";

interface ProfileManageDialogProps {
  isOpen: boolean;
  scope: ProfileScope;
  onClose: () => void;
  onEditProfile?: (profile: Profile, scope: ProfileScope) => void;
  projectOnly?: boolean;
  deploymentDisabled?: boolean;
}

type PendingConfirm = {
  id: string;
  name: string;
};

/**
 * Profile 管理对话框
 *
 * Profile 列表全应用共享，重命名/删除作用于共享实体。
 * Profile 显式保存和应用；项目配置仅部署到绑定目录。
 */
export function ProfileManageDialog({
  isOpen,
  scope,
  onClose,
  onEditProfile,
  projectOnly = false,
  deploymentDisabled = false,
}: ProfileManageDialogProps) {
  const { t } = useTranslation();
  const { data } = useProfilesQuery();
  const updateMutation = useUpdateProfileMutation();
  const deleteMutation = useDeleteProfileMutation();
  const duplicateMutation = useDuplicateProfileMutation();
  const setProjectPathMutation = useSetProfileProjectPathMutation();
  const deployment = useProjectProfileDeployment(scope, !deploymentDisabled);
  const { deployToProject } = deployment;

  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingProfile, setEditingProfile] = useState<{
    profile: Profile;
    scope: ProfileScope;
  } | null>(null);
  const [editingName, setEditingName] = useState("");
  const [confirm, setConfirm] = useState<PendingConfirm | null>(null);
  const [confirmUnbind, setConfirmUnbind] = useState<PendingConfirm | null>(
    null,
  );
  const [confirmCapture, setConfirmCapture] = useState<
    (PendingConfirm & { scope: ProfileScope }) | null
  >(null);
  const [duplicate, setDuplicate] = useState<{
    id: string;
    name: string;
    projectPath: string;
  } | null>(null);

  const profiles = (data?.profiles ?? []).filter(
    (profile) => !projectOnly || Boolean(profile.projectPath),
  );

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

  const pickDuplicatePath = async () => {
    try {
      const projectPath = await settingsApi.pickDirectory();
      if (projectPath !== null) {
        setDuplicate((current) => current && { ...current, projectPath });
      }
    } catch (error) {
      toast.error(
        t("profiles.projectPath.pickFailed", {
          detail: extractErrorMessage(error) || t("common.unknown"),
        }),
      );
    }
  };

  const handleCapture = () => {
    if (!confirmCapture || updateMutation.isPending) return;
    updateMutation.mutate(
      { id: confirmCapture.id, scope: confirmCapture.scope, resnapshot: true },
      { onSuccess: () => setConfirmCapture(null) },
    );
  };

  const handleDuplicate = () => {
    if (
      !duplicate?.name.trim() ||
      !duplicate.projectPath ||
      duplicateMutation.isPending
    )
      return;
    duplicateMutation.mutate(
      {
        sourceId: duplicate.id,
        name: duplicate.name.trim(),
        projectPath: duplicate.projectPath,
      },
      {
        onSuccess: () => setDuplicate(null),
      },
    );
  };

  return (
    <>
      <Dialog
        open={isOpen && !editingProfile}
        onOpenChange={(open) => {
          if (!open) {
            cancelRename();
            onClose();
          }
        }}
      >
        <DialogContent className="max-w-2xl">
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
                className="flex min-w-0 flex-wrap items-center gap-3 rounded-control px-3 py-3 hover:bg-subtle"
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
                      <div className="flex min-w-0 items-center gap-1.5">
                        <span className="truncate text-sm">{profile.name}</span>
                        {profile.projectPath &&
                          profile.deployedScopes?.includes(scope) && (
                            <HoverTip
                              content={t("profiles.projectDeploy.current")}
                            >
                              <span
                                role="img"
                                aria-label={t("profiles.projectDeploy.current")}
                                className="shrink-0 text-fg-2"
                              >
                                <Check className="h-3.5 w-3.5" />
                              </span>
                            </HoverTip>
                          )}
                      </div>
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
                    <div className="ms-auto flex shrink-0 items-center gap-1">
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        className="h-8 px-2.5"
                        onClick={() => {
                          if (onEditProfile) {
                            onEditProfile(profile, scope);
                            onClose();
                          } else setEditingProfile({ profile, scope });
                        }}
                      >
                        <Pencil className="mr-1.5 h-3.5 w-3.5" />
                        {t("common.edit")}
                      </Button>
                      {profile.projectPath &&
                        (scope === "claude" || scope === "codex") && (
                          <HoverTip
                            content={t("profiles.projectDeploy.button")}
                          >
                            <Button
                              type="button"
                              variant="quiet"
                              size="icon-compact"
                              className="h-8 w-8"
                              aria-label={t("profiles.projectDeploy.button")}
                              onClick={() => deployToProject(profile.id)}
                              disabled={
                                deploymentDisabled ||
                                deployment.isPending ||
                                !hasScopeSnapshot(profile, scope)
                              }
                            >
                              <Upload className="h-4 w-4" />
                            </Button>
                          </HoverTip>
                        )}
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button
                            type="button"
                            variant="quiet"
                            size="icon-compact"
                            className="h-8 w-8"
                            aria-label={t("profiles.context.rowActions", {
                              name: profile.name,
                            })}
                          >
                            <MoreHorizontal className="h-4 w-4" />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end">
                          <DropdownMenuItem
                            onSelect={() =>
                              startRename(profile.id, profile.name)
                            }
                          >
                            <Pencil className="h-4 w-4" />
                            {t("profiles.rename")}
                          </DropdownMenuItem>
                          <DropdownMenuItem
                            disabled={duplicateMutation.isPending}
                            onSelect={() =>
                              setDuplicate({
                                id: profile.id,
                                name: profile.name,
                                projectPath: "",
                              })
                            }
                          >
                            <Copy className="h-4 w-4" />
                            {t("profiles.copyToProject")}
                          </DropdownMenuItem>
                          <DropdownMenuItem
                            disabled={setProjectPathMutation.isPending}
                            onSelect={() =>
                              pickProjectPath(profile.id, profile.projectPath)
                            }
                          >
                            <FolderCog className="h-4 w-4" />
                            {t(
                              profile.projectPath
                                ? "profiles.projectPath.change"
                                : "profiles.projectPath.pick",
                            )}
                          </DropdownMenuItem>
                          {profile.projectPath &&
                            (scope === "claude" || scope === "codex") && (
                              <DropdownMenuItem
                                disabled={updateMutation.isPending}
                                onSelect={() =>
                                  setConfirmCapture({
                                    id: profile.id,
                                    name: profile.name,
                                    scope,
                                  })
                                }
                              >
                                <RefreshCw className="h-4 w-4" />
                                {t("profiles.captureCurrent")}
                              </DropdownMenuItem>
                            )}
                          {profile.projectPath && (
                            <DropdownMenuItem
                              disabled={setProjectPathMutation.isPending}
                              onSelect={() =>
                                setConfirmUnbind({
                                  id: profile.id,
                                  name: profile.name,
                                })
                              }
                            >
                              <Unlink className="h-4 w-4" />
                              {t("profiles.projectPath.unbind")}
                            </DropdownMenuItem>
                          )}
                          <DropdownMenuSeparator />
                          <DropdownMenuItem
                            className="text-destructive focus:text-destructive"
                            onSelect={() =>
                              setConfirm({ id: profile.id, name: profile.name })
                            }
                          >
                            <Trash2 className="h-4 w-4" />
                            {t("profiles.delete")}
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
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
      {confirmCapture && (
        <ConfirmDialog
          isOpen
          title={t("profiles.captureCurrent")}
          message={t("profiles.captureCurrentDescription", {
            name: confirmCapture.name,
          })}
          variant="info"
          pending={updateMutation.isPending}
          onConfirm={handleCapture}
          onCancel={() => setConfirmCapture(null)}
        />
      )}
      {duplicate && (
        <Dialog
          open
          onOpenChange={(open) => {
            if (!open && !duplicateMutation.isPending) setDuplicate(null);
          }}
        >
          <DialogContent className="max-w-md" zIndex="alert">
            <DialogHeader>
              <DialogTitle>{t("profiles.duplicateTitle")}</DialogTitle>
              <DialogDescription>
                {t("profiles.duplicateDescription")}
              </DialogDescription>
            </DialogHeader>
            <div className="space-y-3 px-6">
              <Input
                value={duplicate.name}
                onChange={(event) =>
                  setDuplicate({ ...duplicate, name: event.target.value })
                }
                aria-label={t("profiles.namePlaceholder")}
                placeholder={t("profiles.namePlaceholder")}
                disabled={duplicateMutation.isPending}
                autoFocus
              />
              <div className="space-y-1.5">
                <span className="text-xs text-fg-2">
                  {t("profiles.projectPath.label")}
                </span>
                <div className="flex min-w-0 items-center gap-2">
                  <Input
                    aria-label={t("profiles.projectPath.label")}
                    value={duplicate.projectPath}
                    readOnly
                    placeholder={t("profiles.projectPath.placeholder")}
                    className="min-w-0 flex-1"
                  />
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="shrink-0"
                    onClick={pickDuplicatePath}
                    disabled={duplicateMutation.isPending}
                  >
                    <FolderCog className="mr-1.5 h-4 w-4" />
                    {t("profiles.projectPath.pick")}
                  </Button>
                </div>
              </div>
            </div>
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                onClick={() => setDuplicate(null)}
                disabled={duplicateMutation.isPending}
              >
                {t("common.cancel")}
              </Button>
              <Button
                type="button"
                onClick={handleDuplicate}
                disabled={
                  !duplicate.name.trim() ||
                  !duplicate.projectPath ||
                  duplicateMutation.isPending
                }
              >
                {t("profiles.copyToProject")}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}
      <ProjectProfileConflictDialog
        deployment={deployment}
        deploymentDisabled={deploymentDisabled}
      />
      {editingProfile && (
        <ProfileEditPanel
          profile={editingProfile.profile}
          scope={editingProfile.scope}
          onClose={() => setEditingProfile(null)}
        />
      )}
    </>
  );
}
