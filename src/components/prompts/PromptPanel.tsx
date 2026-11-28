import React, { useEffect, useRef, useState } from "react";
import { useIsMutating } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "@/lib/toast";
import { settingsApi, type AppId, type Prompt } from "@/lib/api";
import { usePromptActions } from "@/hooks/usePromptActions";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { Button } from "@/components/ui/button";
import { SegmentedControl } from "@/components/ui/segmented-control";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import PromptFormPanel from "./PromptFormPanel";
import {
  PromptEmptyState,
  PromptLibrary,
  filterPromptEntries,
} from "./PromptLibrary";
import { PromptDistributionDialog } from "./PromptDistributionDialog";
import { PromptPageFrame } from "./PromptPageFrame";
import {
  PI_PROMPT_SAVE_MUTATION_KEY,
  PiPromptTemplates,
  PiSystemPromptFiles,
  type PiPromptTemplatesHandle,
} from "./PiNativePromptResources";
import { copyText, promptFileName } from "./promptUtils";

export interface PromptPanelProps {
  appId: AppId;
  apps: AppId[];
  onAppChange: (app: AppId) => void;
  open?: boolean;
  onInteractionBlockedChange?: (blocked: boolean) => void;
  onNavigationBlockedChange?: (blocked: boolean) => void;
}
type Tab = "library" | "system" | "templates";
const PromptPanel: React.FC<PromptPanelProps> = ({
  appId,
  apps,
  onAppChange,
  open = true,
  onInteractionBlockedChange,
  onNavigationBlockedChange,
}) => {
  const { t } = useTranslation();
  const actions = usePromptActions(appId);
  const { prompts, library, target, pending } = actions;
  const [editing, setEditing] = useState<{
    id?: string;
    prompt?: Prompt;
  } | null>(null);
  const [distributing, setDistributing] = useState<Prompt | null>(null);
  const [deleting, setDeleting] = useState<Prompt | null>(null);
  const [removing, setRemoving] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [tab, setTab] = useState<Tab>("library");
  const templateRef = useRef<PiPromptTemplatesHandle>(null);
  const nativeSaving =
    useIsMutating({ mutationKey: PI_PROMPT_SAVE_MUTATION_KEY }) > 0;
  const refreshRef = useRef(actions.refresh);
  refreshRef.current = actions.refresh;
  const blocked =
    pending ||
    nativeSaving ||
    Boolean(editing || distributing || deleting || removing);
  const loading = library.isPending;
  const controlsDisabled =
    blocked || loading || (library.isError && !library.data);
  const activeTab = appId === "pi" ? tab : "library";
  const snapshot = target.data;
  const appliedPrompt = snapshot?.appliedPromptId
    ? prompts[snapshot.appliedPromptId]
    : undefined;
  useEffect(() => {
    onInteractionBlockedChange?.(controlsDisabled);
    onNavigationBlockedChange?.(blocked);
  }, [
    blocked,
    controlsDisabled,
    onInteractionBlockedChange,
    onNavigationBlockedChange,
  ]);
  useEffect(
    () => () => {
      onInteractionBlockedChange?.(false);
      onNavigationBlockedChange?.(false);
    },
    [onInteractionBlockedChange, onNavigationBlockedChange],
  );
  useEffect(() => {
    if (!open) return;
    const refresh = () => {
      void refreshRef.current();
    };
    window.addEventListener("prompt-imported", refresh);
    return () => window.removeEventListener("prompt-imported", refresh);
  }, [open]);
  useTauriEvent("profile-applied", () => {
    void refreshRef.current();
  });
  const save = async (id: string, prompt: Prompt) => {
    const saved = await actions.savePrompt(id, prompt);
    if (saved) toast.success(t("prompts.library.saved"));
    return saved;
  };
  const importFile = async () => {
    if (
      controlsDisabled ||
      target.isError ||
      target.isFetching ||
      !snapshot?.content?.trim()
    )
      return;
    if (await actions.importFromFile())
      toast.success(t("prompts.library.imported"));
  };
  const fileName = promptFileName(appId);
  const changeApp = (app: AppId) => {
    if (!blocked) {
      setTab("library");
      onAppChange(app);
    }
  };
  const copyContent = async (id: string) => {
    if (await copyText(prompts[id].content))
      toast.success(t("prompts.copiedContent", { name: prompts[id].name }));
    else toast.error(t("prompts.copyFailed"));
  };
  const remove = async () => {
    if (!removing) return;
    if (await actions.removeApplication(removing)) {
      setRemoving(null);
      toast.success(t("prompts.library.removed"));
    }
  };
  const deletePrompt = async () => {
    if (!deleting) return;
    if (await actions.deletePrompt(deleting.id)) {
      setDeleting(null);
      toast.success(t("prompts.library.deleted"));
    }
  };
  return (
    <PromptPageFrame
      app={appId}
      apps={apps}
      onAppChange={changeApp}
      appSwitchDisabled={blocked}
      count={Object.keys(prompts).length}
      help={{
        title: t("prompts.library.title"),
        text: t("prompts.library.help"),
      }}
      primary={
        activeTab === "system"
          ? null
          : {
              label: t(
                activeTab === "templates"
                  ? "pi.prompts.newTemplate"
                  : "prompts.add",
              ),
              disabled: controlsDisabled,
              onClick: () =>
                activeTab === "templates"
                  ? templateRef.current?.openCreate()
                  : setEditing({}),
            }
      }
      moreItems={[
        {
          key: "import",
          label: t("prompts.importCurrent", { file: fileName }),
          reason:
            controlsDisabled || target.isFetching || target.isError
              ? t("prompts.library.targetUnavailable")
              : !snapshot?.content?.trim()
                ? t("prompts.importReason.empty", { file: fileName })
                : undefined,
          onSelect: () => void importFile(),
        },
        {
          key: "open-folder",
          label: t("prompts.openFolder"),
          reason: blocked ? t("common.saving") : undefined,
          onSelect: () => {
            void settingsApi
              .openConfigFolder(appId)
              .catch(() => toast.error(t("prompts.openFolderFailed")));
          },
        },
        {
          key: "refresh",
          label: t("common.refresh"),
          reason: blocked ? t("common.saving") : undefined,
          onSelect: () => void actions.refresh(),
        },
        {
          key: "copy-path",
          label: t("prompts.copyFilePath"),
          reason: !snapshot
            ? t("prompts.library.targetUnavailable")
            : undefined,
          onSelect: () => {
            if (snapshot) void copyText(snapshot.path);
          },
        },
      ]}
      segments={
        appId === "pi" ? (
          <SegmentedControl
            size="sm"
            aria-label={t("pi.prompts.kindLabel")}
            value={activeTab}
            onValueChange={(value: Tab) => {
              if (!blocked) {
                setTab(value);
                setSearch("");
              }
            }}
            items={[
              { value: "library", label: t("prompts.library.title") },
              { value: "system", label: t("pi.prompts.systemTab") },
              { value: "templates", label: t("pi.prompts.templatesTab") },
            ]}
          />
        ) : undefined
      }
      target={
        activeTab === "library"
          ? {
              label: t("prompts.library.targetFile"),
              path: snapshot?.displayPath ?? fileName,
            }
          : null
      }
      search={
        activeTab === "system"
          ? null
          : {
              value: search,
              onChange: setSearch,
              placeholder: t(
                activeTab === "templates"
                  ? "pi.prompts.searchTemplates"
                  : "prompts.searchPlaceholder",
              ),
              ariaLabel: t(
                activeTab === "templates"
                  ? "pi.prompts.searchTemplatesLabel"
                  : "prompts.searchAriaLabel",
              ),
              resultCount:
                activeTab === "library"
                  ? filterPromptEntries(Object.entries(prompts), search).length
                  : undefined,
            }
      }
      notices={
        activeTab === "library" ? (
          <>
            <p className="text-sm text-fg-2">{t("prompts.library.saveHint")}</p>
            {target.isError ? (
              <p role="alert" className="text-sm text-danger-text">
                {t("prompts.library.targetUnavailable")}{" "}
                <Button
                  variant="link"
                  size="compact"
                  onClick={() => void target.refetch()}
                >
                  {t("common.retry")}
                </Button>
              </p>
            ) : snapshot?.appliedPromptId ? (
              <div className="flex flex-wrap items-center gap-2 text-sm">
                <span>
                  {appliedPrompt
                    ? t(
                        snapshot.matchesLibrary
                          ? "prompts.library.current"
                          : "prompts.library.currentPending",
                        { name: appliedPrompt.name },
                      )
                    : t("prompts.library.deletedSource")}
                </span>
                {snapshot.managed || snapshot.content === null ? (
                  <Button
                    variant="neutral"
                    size="compact"
                    disabled={blocked || target.isFetching}
                    onClick={() => setRemoving(snapshot.revision)}
                  >
                    {t("prompts.library.remove")}
                  </Button>
                ) : (
                  <span className="text-fg-2">
                    {t("prompts.library.externalChanges")}
                  </span>
                )}
              </div>
            ) : snapshot?.content?.trim() ? (
              <p className="text-sm text-fg-2">
                {t("prompts.library.externalFile")}
              </p>
            ) : null}
          </>
        ) : undefined
      }
    >
      {activeTab === "system" ? (
        <PiSystemPromptFiles />
      ) : activeTab === "templates" ? (
        <PiPromptTemplates
          ref={templateRef}
          search={search}
          onClearSearch={() => setSearch("")}
        />
      ) : library.isError && !library.data ? (
        <div role="alert" className="py-10 text-center">
          <p>{t("prompts.loadFailed")}</p>
          <Button variant="neutral" onClick={() => void library.refetch()}>
            {t("common.retry")}
          </Button>
        </div>
      ) : loading ? (
        <p role="status" className="py-10 text-center">
          {t("prompts.loading")}
        </p>
      ) : Object.keys(prompts).length === 0 ? (
        <PromptEmptyState
          displayPath={snapshot?.displayPath ?? fileName}
          fileText={snapshot?.content ?? null}
          disabled={controlsDisabled}
          onImport={() => void importFile()}
          onAdd={() => setEditing({})}
        />
      ) : (
        <PromptLibrary
          prompts={prompts}
          searchQuery={search}
          listLabel={t("prompts.library.title")}
          disabled={controlsDisabled}
          appliedPromptId={snapshot?.appliedPromptId}
          matchesLibrary={snapshot?.matchesLibrary}
          onClearSearch={() => setSearch("")}
          onApply={(id) => setDistributing(prompts[id])}
          onEdit={(id) => setEditing({ id, prompt: prompts[id] })}
          onDelete={(id) => setDeleting(prompts[id])}
          onCopyContent={(id) => void copyContent(id)}
        />
      )}
      {editing && (
        <PromptFormPanel
          key={editing.id ?? "new"}
          editingId={editing.id}
          initialData={editing.prompt}
          onSave={save}
          onClose={() => setEditing(null)}
        />
      )}
      {distributing && (
        <PromptDistributionDialog
          prompt={distributing}
          apps={apps.includes(appId) ? apps : [appId, ...apps]}
          initialApp={appId}
          onClose={() => setDistributing(null)}
          onApplied={() => {
            void actions.refresh();
          }}
        />
      )}
      <ConfirmDialog
        isOpen={Boolean(deleting)}
        title={t("prompts.library.deleteTitle")}
        message={t("prompts.library.deleteHint", { name: deleting?.name })}
        pending={pending}
        onConfirm={() => void deletePrompt()}
        onCancel={() => setDeleting(null)}
      />
      <ConfirmDialog
        isOpen={Boolean(removing)}
        title={t("prompts.library.remove")}
        message={t("prompts.library.removeHint", {
          path: snapshot?.displayPath,
        })}
        pending={pending}
        onConfirm={() => void remove()}
        onCancel={() => setRemoving(null)}
      />
    </PromptPageFrame>
  );
};
export default PromptPanel;
