import React, { useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { CircleAlert } from "lucide-react";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import {
  Sheet,
  SheetDescription,
  SheetPageContent,
} from "@/components/ui/sheet";
import type { Prompt } from "@/lib/api";
import { fieldClass } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { charCount, formatSize, utf8Bytes } from "./promptUtils";

interface PromptFormPanelProps {
  editingId?: string;
  initialData?: Prompt;
  onSave: (id: string, prompt: Prompt) => Promise<void | boolean>;
  onClose: () => void;
}

/** 和 Input / Textarea 同一套外观（ui/input.tsx 的 fieldClass） */
export const promptFieldClass = fieldClass;

export function FieldError({ id, children }: { id: string; children: string }) {
  return (
    <span
      id={id}
      className="flex items-start gap-1 text-caption text-danger-text"
    >
      <CircleAlert
        aria-hidden="true"
        strokeWidth={1.5}
        className="mt-0.5 h-3.5 w-3.5 shrink-0"
      />
      <span>{children}</span>
    </span>
  );
}

/**
 * Edits one shared library document. Saving never writes an Agent file.
 */
const PromptFormPanel: React.FC<PromptFormPanelProps> = ({
  editingId,
  initialData,
  onSave,
  onClose,
}) => {
  const { t } = useTranslation();
  const baseId = useId();
  const [name, setName] = useState(initialData?.name ?? "");
  const [description, setDescription] = useState(
    initialData?.description ?? "",
  );
  const [content, setContent] = useState(initialData?.content ?? "");
  const initialDraft = useRef(
    JSON.stringify([
      initialData?.name ?? "",
      initialData?.description ?? "",
      initialData?.content ?? "",
    ]),
  );
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const nameRef = useRef<HTMLInputElement>(null);

  const bodyBytes = utf8Bytes(content);
  const nameError = !name.trim() ? t("prompts.nameRequired") : "";
  const showNameError = attempted && Boolean(nameError);

  const handleSave = async () => {
    if (savingRef.current) return;
    if (nameError) {
      setAttempted(true);
      nameRef.current?.focus();
      return;
    }

    savingRef.current = true;
    setSaving(true);
    try {
      const id = editingId || crypto.randomUUID();
      const timestamp = Math.floor(Date.now() / 1000);
      const prompt: Prompt = {
        id,
        name: name.trim(),
        description: description.trim() || undefined,
        content,
        createdAt: initialData?.createdAt || timestamp,
        updatedAt: timestamp,
      };
      const saved = await onSave(id, prompt);
      if (saved !== false) {
        onClose();
      }
    } catch {
      // Error handled by hook
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  };

  const handleClose = () => {
    if (savingRef.current) return;
    if (JSON.stringify([name, description, content]) !== initialDraft.current)
      setConfirmDiscard(true);
    else onClose();
  };

  const title = editingId
    ? t("prompts.editTitle")
    : t("prompts.library.addTitle");
  const submitLabel = editingId ? t("common.save") : t("prompts.addSubmit");
  const countText = t("prompts.charCount", {
    chars: charCount(content).toLocaleString(),
    size: formatSize(bodyBytes),
  });

  const nameHintId = `${baseId}-name-hint`;
  const countId = `${baseId}-count`;

  return (
    <>
      <Sheet
        open
        modal={false}
        onOpenChange={(open) => {
          if (!open) handleClose();
        }}
      >
        <SheetPageContent
          title={title}
          closeLabel={t("common.back")}
          onEscapeKeyDown={(event) => {
            if (savingRef.current) event.preventDefault();
          }}
        >
          <SheetDescription className="px-6 pt-3 text-sm text-fg-2">
            {t("prompts.library.saveHint")}
          </SheetDescription>

          <div className="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto scroll-stable overscroll-contain px-6 pb-6 pt-5">
            <div className="flex shrink-0 flex-col gap-1.5">
              <label
                htmlFor={`${baseId}-name`}
                className="text-body font-medium text-fg-1"
              >
                {t("prompts.name")}
                <span aria-hidden="true" className="ms-0.5 text-danger-text">
                  *
                </span>
                <span className="sr-only">{t("prompts.requiredMark")}</span>
              </label>
              <input
                ref={nameRef}
                id={`${baseId}-name`}
                type="text"
                value={name}
                onChange={(event) => setName(event.target.value)}
                disabled={saving}
                placeholder={t("prompts.namePlaceholder")}
                aria-required="true"
                aria-invalid={showNameError}
                aria-describedby={showNameError ? nameHintId : undefined}
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="off"
                className={cn(promptFieldClass, "h-8")}
              />
              {showNameError ? (
                <FieldError id={nameHintId}>{nameError}</FieldError>
              ) : null}
            </div>

            <div className="flex shrink-0 flex-col gap-1.5">
              <label
                htmlFor={`${baseId}-desc`}
                className="text-body font-medium text-fg-1"
              >
                {t("prompts.description")}
              </label>
              <input
                id={`${baseId}-desc`}
                type="text"
                value={description}
                onChange={(event) => setDescription(event.target.value)}
                disabled={saving}
                placeholder={t("prompts.descriptionPlaceholder")}
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="off"
                className={cn(promptFieldClass, "h-8")}
              />
            </div>

            <div className="flex min-h-[220px] flex-1 flex-col gap-1.5">
              <label
                htmlFor={`${baseId}-body`}
                className="text-body font-medium text-fg-1"
              >
                {t("prompts.content")}
              </label>
              <textarea
                id={`${baseId}-body`}
                value={content}
                onChange={(event) => setContent(event.target.value)}
                disabled={saving}
                spellCheck={false}
                autoCorrect="off"
                autoCapitalize="off"
                placeholder={t("prompts.library.contentPlaceholder")}
                aria-describedby={countId}
                className={cn(
                  promptFieldClass,
                  "min-h-[180px] flex-1 resize-none px-3 py-2.5 font-mono text-caption leading-[18px]",
                )}
              />
              <div className="flex items-start gap-3">
                <span
                  id={countId}
                  className={cn(
                    "ms-auto shrink-0 whitespace-nowrap text-caption tabular-nums",
                    "text-fg-2",
                  )}
                >
                  {countText}
                </span>
              </div>
            </div>
          </div>

          <div className="flex h-14 shrink-0 items-center gap-2 border-t border-border px-6">
            <div className="flex-1" />
            <Button
              type="button"
              variant="neutral"
              size="regular"
              onClick={handleClose}
              disabled={saving}
            >
              {t("common.cancel")}
            </Button>
            <Button
              type="button"
              variant="solid"
              size="regular"
              onClick={() => void handleSave()}
              disabled={saving}
            >
              {saving ? t("common.saving") : submitLabel}
            </Button>
          </div>
        </SheetPageContent>
      </Sheet>
      <ConfirmDialog
        isOpen={confirmDiscard}
        title={t("prompts.library.discardTitle")}
        message={t("prompts.library.discardHint")}
        confirmText={t("prompts.library.discard")}
        cancelText={t("prompts.library.keepEditing")}
        onConfirm={onClose}
        onCancel={() => setConfirmDiscard(false)}
      />
    </>
  );
};

export default PromptFormPanel;
