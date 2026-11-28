import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Copy, MoreHorizontal, Pencil, RefreshCw, Trash2 } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  useDeleteProfileMutation,
  useDuplicateProfileMutation,
  useUpdateProfileMutation,
} from "@/lib/query/profiles";
import type { Profile, ProfileScope } from "@/lib/api/profiles";

type Action = "rename" | "duplicate" | "copyTarget" | "capture" | "delete";

/** Occasional actions on one profile. Everything here changes the saved profile only. */
export function ProfileActionsMenu({
  profile,
  scope,
  onCopied,
  allowTargetCopy = true,
}: {
  profile: Profile;
  scope: ProfileScope;
  onCopied: (profile: Profile) => void;
  allowTargetCopy?: boolean;
}) {
  const { t } = useTranslation();
  const [action, setAction] = useState<Action | null>(null);
  const [name, setName] = useState("");
  const updateMutation = useUpdateProfileMutation();
  const duplicateMutation = useDuplicateProfileMutation();
  const deleteMutation = useDeleteProfileMutation();
  const close = () => setAction(null);
  const naming =
    action === "rename" || action === "duplicate" || action === "copyTarget";
  const otherTarget = profile.target === "global" ? "project" : "global";
  const copyLabel =
    otherTarget === "global"
      ? "profiles.copyToGlobal"
      : "profiles.copyToProject";
  const namePending = updateMutation.isPending || duplicateMutation.isPending;

  const submitName = () => {
    const trimmed = name.trim();
    if (!trimmed || namePending) return;
    if (action === "rename") {
      updateMutation.mutate(
        { id: profile.id, name: trimmed },
        { onSuccess: close },
      );
    } else {
      duplicateMutation.mutate(
        {
          sourceId: profile.id,
          name: trimmed,
          scope,
          target: action === "copyTarget" ? otherTarget : profile.target,
        },
        {
          onSuccess: (copied) => {
            close();
            if (action === "copyTarget") onCopied(copied);
          },
        },
      );
    }
  };

  return (
    <>
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
            onSelect={() => {
              setName(profile.name);
              setAction("rename");
            }}
          >
            <Pencil className="h-4 w-4" />
            {t("profiles.rename")}
          </DropdownMenuItem>
          <DropdownMenuItem
            onSelect={() => {
              setName(t("profiles.duplicateName", { name: profile.name }));
              setAction("duplicate");
            }}
          >
            <Copy className="h-4 w-4" />
            {t("profiles.duplicate")}
          </DropdownMenuItem>
          {allowTargetCopy && scope !== "claude-desktop" && (
            <DropdownMenuItem
              onSelect={() => {
                setName(t("profiles.duplicateName", { name: profile.name }));
                setAction("copyTarget");
              }}
            >
              <Copy className="h-4 w-4" />
              {t(copyLabel)}
            </DropdownMenuItem>
          )}
          {profile.target === "global" && (
            <DropdownMenuItem onSelect={() => setAction("capture")}>
              <RefreshCw className="h-4 w-4" />
              {t("profiles.captureCurrent")}
            </DropdownMenuItem>
          )}
          <DropdownMenuSeparator />
          <DropdownMenuItem
            className="text-destructive focus:text-destructive"
            onSelect={() => setAction("delete")}
          >
            <Trash2 className="h-4 w-4" />
            {t("profiles.delete")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      {naming && (
        <Dialog
          open
          onOpenChange={(open) => {
            if (!open && !namePending) close();
          }}
        >
          <DialogContent className="max-w-md">
            <DialogHeader>
              <DialogTitle>
                {t(
                  action === "rename"
                    ? "profiles.rename"
                    : action === "copyTarget"
                      ? copyLabel
                      : "profiles.duplicateTitle",
                )}
              </DialogTitle>
              {action === "copyTarget" && (
                <DialogDescription>
                  {t("profiles.copyTargetHint")}
                </DialogDescription>
              )}
            </DialogHeader>
            <form
              id="profile-name-form"
              className="px-6"
              onSubmit={(event) => {
                event.preventDefault();
                submitName();
              }}
            >
              <Input
                value={name}
                onChange={(event) => setName(event.target.value)}
                aria-label={t("profiles.editor.name")}
                placeholder={t("profiles.namePlaceholder")}
                disabled={namePending}
                autoFocus
              />
            </form>
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                onClick={close}
                disabled={namePending}
              >
                {t("common.cancel")}
              </Button>
              <Button
                type="submit"
                form="profile-name-form"
                disabled={!name.trim() || namePending}
              >
                {t("common.confirm")}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}
      {action === "capture" && (
        <ConfirmDialog
          isOpen
          title={t("profiles.captureCurrent")}
          message={t("profiles.captureCurrentDescription", {
            name: profile.name,
          })}
          variant="info"
          pending={updateMutation.isPending}
          onConfirm={() =>
            updateMutation.mutate(
              { id: profile.id, scope, resnapshot: true },
              { onSuccess: close },
            )
          }
          onCancel={close}
        />
      )}
      {action === "delete" && (
        <ConfirmDialog
          isOpen
          title={t("profiles.deleteConfirmTitle")}
          message={t("profiles.deleteConfirmMessage", { name: profile.name })}
          variant="destructive"
          pending={deleteMutation.isPending}
          onConfirm={() =>
            deleteMutation.mutate(
              { id: profile.id, scope },
              { onSettled: close },
            )
          }
          onCancel={close}
        />
      )}
    </>
  );
}
