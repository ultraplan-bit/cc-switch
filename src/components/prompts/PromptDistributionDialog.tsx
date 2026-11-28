import { useRef, useState } from "react";
import { useQueries } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "@/lib/toast";
import { promptsApi, type AppId, type Prompt } from "@/lib/api";
import { promptKeys } from "@/lib/query/prompts";
import { AppGlyph, APP_DISPLAY_NAME } from "@/components/shell/AppGlyph";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { extractErrorMessage } from "@/utils/errorUtils";
import { MCODE_PROMPT_LIMIT, formatSize, utf8Bytes } from "./promptUtils";

interface Props {
  prompt: Prompt;
  apps: AppId[];
  initialApp: AppId;
  onClose: () => void;
  onApplied: () => void;
}

/** A revision binds each write to the destination the user reviewed. */
export function PromptDistributionDialog({
  prompt,
  apps,
  initialApp,
  onClose,
  onApplied,
}: Props) {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<AppId[]>(
    apps.includes(initialApp) ? [initialApp] : apps.slice(0, 1),
  );
  const [consentRevision, setConsentRevision] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [applied, setApplied] = useState<AppId[]>([]);
  const lock = useRef(false);
  const availableApps = useRef(apps);
  availableApps.current = apps;
  const snapshots = useQueries({
    queries: apps.map((app) => ({
      queryKey: promptKeys.target(app),
      queryFn: () => promptsApi.getPromptTarget(app),
      networkMode: "always" as const,
      refetchOnWindowFocus: false,
      staleTime: 0,
    })),
  });
  const targets = selected
    .filter((app) => apps.includes(app))
    .map((app) => ({
      app,
      query: snapshots[apps.indexOf(app)],
    }));
  const unavailable = targets.some(
    ({ app, query }) =>
      !query?.data ||
      query.isFetching ||
      query.isError ||
      Boolean(errors[app]) ||
      (app === "mcode" && utf8Bytes(prompt.content) > MCODE_PROMPT_LIMIT),
  );
  const overwrites = targets.some(
    ({ query }) =>
      Boolean(query?.data?.content?.trim()) &&
      query.data?.content !== prompt.content,
  );
  const reviewedRevisions = targets
    .map(({ app, query }) => `${app}:${query.data?.revision ?? ""}`)
    .join("|");
  const consent = consentRevision === reviewedRevisions;
  const ready =
    targets.length > 0 && !unavailable && (!overwrites || consent) && !pending;
  const apply = async () => {
    if (lock.current || !ready) return;
    lock.current = true;
    setPending(true);
    const failures: Record<string, string> = {};
    const succeeded: AppId[] = [];
    for (const { app, query } of targets) {
      if (!availableApps.current.includes(app)) {
        failures[app] = t("prompts.library.targetUnavailable");
        continue;
      }
      try {
        await promptsApi.applyPrompt(
          app,
          prompt.id,
          query.data!.revision,
          prompt.content,
        );
        succeeded.push(app);
      } catch (error) {
        failures[app] =
          extractErrorMessage(error) || t("prompts.library.writeFailed");
      }
    }
    setApplied((current) => [...new Set([...current, ...succeeded])]);
    setErrors(failures);
    setSelected(targets.map(({ app }) => app).filter((app) => app in failures));
    setConsentRevision(null);
    onApplied();
    setPending(false);
    lock.current = false;
    if (!Object.keys(failures).length) {
      toast.success(
        t("prompts.library.appliedTo", {
          apps: succeeded.map((app) => APP_DISPLAY_NAME[app]).join(", "),
        }),
      );
      onClose();
    } else {
      toast.error(
        t("prompts.library.distributionFailed", {
          count: Object.keys(failures).length,
        }),
      );
    }
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !lock.current) onClose();
      }}
    >
      <DialogContent className="max-w-xl">
        <DialogHeader>
          <DialogTitle>
            {t("prompts.library.distributeTitle", { name: prompt.name })}
          </DialogTitle>
          <DialogDescription>
            {t("prompts.library.distributeHint")}
          </DialogDescription>
        </DialogHeader>
        <div className="max-h-[55vh] space-y-3 overflow-y-auto px-6 py-4">
          {applied.length > 0 && (
            <p role="status" className="text-sm text-success-text">
              {t("prompts.library.appliedTo", {
                apps: applied.map((app) => APP_DISPLAY_NAME[app]).join(", "),
              })}
            </p>
          )}
          {apps.map((app, index) => {
            const query = snapshots[index];
            const snapshot = query.data;
            const tooLarge =
              app === "mcode" && utf8Bytes(prompt.content) > MCODE_PROMPT_LIMIT;
            return (
              <div key={app} className="rounded-panel border border-border p-3">
                <label className="flex min-h-9 cursor-pointer items-center gap-2">
                  <Checkbox
                    checked={selected.includes(app)}
                    disabled={pending || applied.includes(app)}
                    onCheckedChange={(checked) => {
                      setSelected((current) =>
                        checked
                          ? [...current, app]
                          : current.filter((value) => value !== app),
                      );
                      setConsentRevision(null);
                    }}
                  />
                  <AppGlyph app={app} size={16} />
                  <span className="font-medium">{APP_DISPLAY_NAME[app]}</span>
                  {applied.includes(app) && (
                    <span className="text-xs text-success-text">
                      {t("prompts.library.applied")}
                    </span>
                  )}
                </label>
                {snapshot && (
                  <code className="block break-all text-xs text-fg-2">
                    {snapshot.displayPath}
                  </code>
                )}
                {query.isPending && (
                  <p className="text-sm text-fg-2">{t("common.loading")}</p>
                )}
                {tooLarge && (
                  <p role="alert" className="text-sm text-danger-text">
                    {t("prompts.mcodeTooLarge", {
                      size: formatSize(utf8Bytes(prompt.content)),
                    })}
                  </p>
                )}
                {(query.isError || errors[app]) && (
                  <div role="alert" className="mt-2 text-sm text-danger-text">
                    <p>{errors[app] || extractErrorMessage(query.error)}</p>
                    <Button
                      variant="link"
                      size="compact"
                      disabled={pending || query.isFetching}
                      onClick={() => {
                        setConsentRevision(null);
                        setErrors((current) => {
                          const next = { ...current };
                          delete next[app];
                          return next;
                        });
                        void query.refetch();
                      }}
                    >
                      {t("common.retry")}
                    </Button>
                  </div>
                )}
                {selected.includes(app) &&
                  snapshot?.content &&
                  snapshot.content !== prompt.content && (
                    <details className="mt-2 text-sm">
                      <summary className="cursor-pointer text-fg-2">
                        {t("prompts.library.existingContent")}
                      </summary>
                      <pre className="mt-2 max-h-32 overflow-auto whitespace-pre-wrap break-words rounded bg-subtle p-2 text-xs">
                        {snapshot.content}
                      </pre>
                    </details>
                  )}
              </div>
            );
          })}
          {overwrites && (
            <label className="flex items-start gap-2 text-sm">
              <Checkbox
                checked={consent}
                disabled={pending}
                onCheckedChange={(checked) =>
                  setConsentRevision(
                    checked === true ? reviewedRevisions : null,
                  )
                }
              />
              <span>{t("prompts.library.overwriteConsent")}</span>
            </label>
          )}
        </div>
        <DialogFooter>
          <Button variant="neutral" disabled={pending} onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="solid"
            disabled={!ready}
            onClick={() => void apply()}
          >
            {pending ? t("common.saving") : t("prompts.library.applySelected")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
