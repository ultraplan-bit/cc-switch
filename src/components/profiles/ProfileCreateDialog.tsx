import { useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FolderCog } from "lucide-react";
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
import { settingsApi } from "@/lib/api";
import {
  useCreateProfileMutation,
  useImportProjectProfileMutation,
} from "@/lib/query/profiles";
import type { Profile, ProfileScope, ProfileTarget } from "@/lib/api/profiles";
import { toast } from "@/lib/toast";
import { extractErrorMessage } from "@/utils/errorUtils";

type Source = "blank" | "current" | "project";

interface ProfileCreateDialogProps {
  scope: ProfileScope;
  target: ProfileTarget;
  /** Preselected folder when the user is looking at a project. */
  projectPath?: string;
  onClose: () => void;
  onCreated: (profile: Profile) => void;
}

/** Creating only saves a profile. It is applied or deployed later, from its card. */
export function ProfileCreateDialog({
  scope,
  target,
  projectPath: initialProjectPath,
  onClose,
  onCreated,
}: ProfileCreateDialogProps) {
  const { t } = useTranslation();
  const sourceName = useId();
  const [name, setName] = useState("");
  const [source, setSource] = useState<Source>("blank");
  const [projectPath, setProjectPath] = useState(initialProjectPath ?? "");
  const createMutation = useCreateProfileMutation();
  const importMutation = useImportProjectProfileMutation();
  const pending = createMutation.isPending || importMutation.isPending;
  // Enter and the button can both submit before the pending state re-renders.
  const submitting = useRef(false);
  const sources: Source[] =
    target === "global" || scope === "claude-desktop"
      ? ["blank", "current"]
      : ["blank", "current", "project"];
  const ready =
    Boolean(name.trim()) && (source !== "project" || Boolean(projectPath));

  const submit = async () => {
    if (!ready || submitting.current) return;
    submitting.current = true;
    try {
      const created =
        source === "project"
          ? (
              await importMutation.mutateAsync({
                name: name.trim(),
                scope,
                projectPath,
              })
            ).profile
          : await createMutation.mutateAsync({
              name: name.trim(),
              scope,
              source,
              target,
            });
      onCreated(created);
    } catch {
      // The mutation reports the failure; the form stays open for another try.
    } finally {
      submitting.current = false;
    }
  };

  const pickProjectPath = async () => {
    try {
      const picked = await settingsApi.pickDirectory(projectPath || undefined);
      if (picked !== null) setProjectPath(picked);
    } catch (error) {
      toast.error(
        t("profiles.projectPath.pickFailed", {
          detail: extractErrorMessage(error) || t("common.unknown"),
        }),
      );
    }
  };

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !pending) onClose();
      }}
    >
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("profiles.createFromCurrent")}</DialogTitle>
          <DialogDescription>
            {t(
              target === "global"
                ? "profiles.createGlobalDescription"
                : "profiles.createProjectDescription",
            )}
          </DialogDescription>
        </DialogHeader>
        <form
          id="profile-create-form"
          className="space-y-4 px-6"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <Input
            value={name}
            onChange={(event) => setName(event.target.value)}
            aria-label={t("profiles.editor.name")}
            placeholder={t("profiles.namePlaceholder")}
            disabled={pending}
            autoFocus
          />
          <fieldset className="space-y-2" disabled={pending}>
            <legend className="mb-2 text-xs text-fg-2">
              {t("profiles.createSource.label")}
            </legend>
            {sources.map((value) => (
              <label
                key={value}
                className="flex cursor-pointer items-start gap-3 rounded-control border border-border px-3 py-2.5 text-sm has-[:checked]:border-primary/50 has-[:checked]:bg-primary/5"
              >
                <input
                  type="radio"
                  className="mt-0.5"
                  name={sourceName}
                  checked={source === value}
                  onChange={() => setSource(value)}
                />
                <span className="min-w-0">
                  <span className="block font-medium">
                    {t(`profiles.createSource.${value}`)}
                  </span>
                  <span className="mt-0.5 block text-xs text-fg-2">
                    {t(`profiles.createSource.${value}Hint`)}
                  </span>
                </span>
              </label>
            ))}
          </fieldset>
          {source === "project" && (
            <div className="space-y-1.5">
              <span className="text-xs text-fg-2">
                {t("profiles.projectPath.label")}
              </span>
              <div className="flex min-w-0 items-center gap-2">
                <Input
                  aria-label={t("profiles.projectPath.label")}
                  value={projectPath}
                  readOnly
                  placeholder={t("profiles.projectPath.placeholder")}
                  className="min-w-0 flex-1"
                />
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="shrink-0"
                  onClick={pickProjectPath}
                  disabled={pending}
                >
                  <FolderCog className="mr-1.5 h-4 w-4" />
                  {t("profiles.projectPath.pick")}
                </Button>
              </div>
            </div>
          )}
        </form>
        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            onClick={onClose}
            disabled={pending}
          >
            {t("common.cancel")}
          </Button>
          <Button
            type="submit"
            form="profile-create-form"
            disabled={!ready || pending}
          >
            {t("profiles.createAndEdit")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
