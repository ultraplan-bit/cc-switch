import { useId, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { parse as parseToml } from "smol-toml";
import { CommonConfigEditor } from "@/components/providers/forms/CommonConfigEditor";
import { CodexConfigSection } from "@/components/providers/forms/CodexConfigSections";
import { FullScreenPanel } from "@/components/common/FullScreenPanel";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { PageTabs } from "@/components/ui/page-tabs";
import { providersApi, promptsApi } from "@/lib/api";
import { promptKeys } from "@/lib/query/prompts";
import { useAllMcpServers } from "@/hooks/useMcp";
import { useInstalledSkills } from "@/hooks/useSkills";
import { useSaveProfileConfigMutation } from "@/lib/query/profiles";
import type { Profile, ProfileConfig, ProfileScope } from "@/lib/api/profiles";
import type { Prompt } from "@/lib/api/prompts";
import type { InstalledSkill } from "@/lib/api/skills";
import type { McpServersMap, Provider } from "@/types";
import { extractErrorMessage } from "@/utils/errorUtils";
import {
  ProfileModelFields,
  providerModelDefaults,
} from "./ProfileModelFields";

interface ProfileEditPanelProps {
  profile: Profile;
  scope: ProfileScope;
  onClose: () => void;
}

export function ProfileEditPanel({
  profile,
  scope,
  onClose,
}: ProfileEditPanelProps) {
  const { t } = useTranslation();
  const providers = useQuery({
    networkMode: "always",
    queryKey: ["providers", scope, "profile-editor"],
    queryFn: () => providersApi.getAll(scope),
  });
  const mcp = useAllMcpServers();
  const skills = useInstalledSkills();
  const prompts = useQuery({
    networkMode: "always",
    queryKey: promptKeys.list(scope),
    queryFn: () => promptsApi.getPrompts(scope),
    enabled: scope !== "claude-desktop",
  });
  const required =
    scope === "claude-desktop"
      ? [providers]
      : [providers, mcp, skills, prompts];
  const failed = required.find(
    (query) => query.isError && query.data === undefined,
  );
  const loading = required.some(
    (query) => query.data === undefined && query.isPending,
  );

  if (!failed && !loading) {
    return (
      <ProfileEditForm
        key={`${profile.id}:${scope}`}
        profile={profile}
        scope={scope}
        onClose={onClose}
        providers={providers.data ?? {}}
        mcp={mcp.data ?? {}}
        skills={skills.data ?? []}
        prompts={prompts.data ?? {}}
      />
    );
  }
  return (
    <FullScreenPanel
      isOpen
      title={t("profiles.editor.title")}
      subtitle={t("profiles.editor.description")}
      onClose={onClose}
      contentClassName="mx-auto max-w-3xl"
      footer={
        <>
          <Button variant="outline" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          {failed && (
            <Button
              onClick={() => {
                void Promise.all(required.map((query) => query.refetch()));
              }}
            >
              {t("common.retry")}
            </Button>
          )}
        </>
      }
    >
      <div className="py-5" role={failed ? "alert" : "status"}>
        {failed
          ? t("profiles.editor.loadFailed", {
              detail: extractErrorMessage(failed.error),
            })
          : t("common.loading")}
      </div>
    </FullScreenPanel>
  );
}

function ProfileEditForm({
  profile,
  scope,
  onClose,
  providers,
  mcp,
  skills,
  prompts,
}: ProfileEditPanelProps & {
  providers: Record<string, Provider>;
  mcp: McpServersMap;
  skills: InstalledSkill[];
  prompts: Record<string, Prompt>;
}) {
  const { t } = useTranslation();
  const id = useId();
  const saving = useRef(false);
  const mutation = useSaveProfileConfigMutation();
  const [name, setName] = useState(profile.name);
  const [tab, setTab] = useState<
    "models" | "behavior" | "resources" | "instructions"
  >("models");
  const [resourceSearch, setResourceSearch] = useState("");
  const [config, setConfig] = useState<ProfileConfig>(() => {
    const providerId = profile.payload.providers[scope] ?? null;
    const defaults = providerModelDefaults(providers[providerId ?? ""], scope);
    return {
      providerId,
      model: profile.payload.models?.[scope] ?? defaults.model,
      modelSettings:
        profile.payload.modelSettings?.[scope] ?? defaults.modelSettings,
      modelCatalog:
        profile.payload.modelCatalog?.[scope] ?? defaults.modelCatalog,
      nativeConfig: profile.payload.nativeConfig?.[scope],
      mcpIds: [...(profile.payload.mcp[scope] ?? [])],
      skillIds: [...(profile.payload.skills[scope] ?? [])],
      promptId: profile.payload.prompts[scope] || null,
    };
  });
  const [error, setError] = useState("");
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  const draft = JSON.stringify({
    name,
    ...config,
    mcpIds: [...config.mcpIds].sort(),
    skillIds: [...config.skillIds].sort(),
    modelSettings: Object.fromEntries(
      Object.entries(config.modelSettings)
        .filter(([, value]) => value !== "")
        .sort(([a], [b]) => a.localeCompare(b)),
    ),
  });
  const initialDraft = useRef(draft);
  const requestClose = () => {
    if (saving.current || mutation.isPending || confirmDiscard) return;
    if (draft !== initialDraft.current) setConfirmDiscard(true);
    else onClose();
  };
  const provider = providers[config.providerId ?? ""];
  const catalogRows = Array.isArray(config.modelCatalog?.models)
    ? config.modelCatalog.models
    : [];
  const invalidModel =
    scope === "codex" &&
    catalogRows.some(
      (row) => !row || typeof row.model !== "string" || !row.model.trim(),
    );
  let nativeError = "";
  if (config.nativeConfig?.trim()) {
    try {
      const parsed =
        scope === "codex"
          ? parseToml(config.nativeConfig)
          : JSON.parse(config.nativeConfig);
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
        nativeError = t("profiles.editor.nativeObjectRequired");
      }
    } catch (failure) {
      nativeError = extractErrorMessage(failure);
    }
  }
  const save = async () => {
    if (
      !name.trim() ||
      saving.current ||
      mutation.isPending ||
      invalidModel ||
      nativeError
    )
      return;
    saving.current = true;
    setError("");
    try {
      await mutation.mutateAsync({
        id: profile.id,
        scope,
        name: name.trim(),
        config: {
          ...config,
          model: config.model.trim(),
          modelSettings: Object.fromEntries(
            Object.entries(config.modelSettings).map(([key, value]) => [
              key,
              value.trim(),
            ]),
          ),
        },
      });
      onClose();
    } catch (failure) {
      setError(extractErrorMessage(failure) || t("common.unknown"));
    } finally {
      saving.current = false;
    }
  };
  const toggleResource = (
    key: "mcpIds" | "skillIds",
    resourceId: string,
    checked: boolean,
  ) => {
    setConfig((current) => ({
      ...current,
      [key]: checked
        ? [...new Set([...current[key], resourceId])]
        : current[key].filter((selected) => selected !== resourceId),
    }));
  };
  const resources = [
    {
      key: "mcpIds" as const,
      kind: "mcp" as const,
      label: t("profiles.editor.mcp"),
      available: Object.values(mcp),
    },
    {
      key: "skillIds" as const,
      kind: "skills" as const,
      label: t("profiles.editor.skills"),
      available: skills,
    },
  ];
  const originalPromptId = profile.payload.prompts[scope];
  const missingPromptId = [config.promptId, originalPromptId].find(
    (promptId) => promptId && !prompts[promptId],
  );
  const missingPromptName = missingPromptId
    ? profile.payload.resourceNames?.[scope]?.prompts?.[missingPromptId]
    : undefined;
  const selectedPrompt = config.promptId ? prompts[config.promptId] : undefined;
  const selectClass =
    "h-10 w-full rounded-control border border-border bg-surface px-3 text-sm text-fg-1";
  return (
    <FullScreenPanel
      isOpen
      title={t("profiles.editor.title")}
      subtitle={profile.name}
      onClose={requestClose}
      trackUnsavedChanges
      contentClassName="mx-auto max-w-5xl"
      footer={
        <>
          <span className="mr-auto hidden text-xs text-fg-2 sm:block">
            {t(
              profile.projectPath
                ? "profiles.editor.saveHint"
                : "profiles.editor.globalSaveHint",
            )}
          </span>
          <Button
            type="button"
            variant="outline"
            disabled={mutation.isPending}
            onClick={onClose}
          >
            {t("common.cancel")}
          </Button>
          <Button
            type="submit"
            form="profile-editor-form"
            disabled={
              !name.trim() ||
              mutation.isPending ||
              invalidModel ||
              Boolean(nativeError)
            }
          >
            {t("common.save")}
          </Button>
        </>
      }
    >
      <ConfirmDialog
        isOpen={confirmDiscard}
        title={t("common.unsavedLeaveTitle")}
        message={t("common.unsavedLeaveMessage")}
        confirmText={t("common.unsavedLeaveConfirm")}
        cancelText={t("common.unsavedLeaveCancel")}
        onConfirm={() => {
          setConfirmDiscard(false);
          onClose();
        }}
        onCancel={() => setConfirmDiscard(false)}
      />
      <div className="space-y-2">
        <p className="text-sm text-fg-2">
          {t("profiles.editor.scopeSummary", {
            agent:
              scope === "claude"
                ? "Claude Code"
                : scope === "codex"
                  ? "Codex"
                  : "Claude Desktop",
          })}
        </p>
        {profile.projectPath && (
          <p
            className="truncate font-mono text-xs text-fg-2"
            title={profile.projectPath}
          >
            {profile.projectPath}
          </p>
        )}
      </div>
      <div data-unsaved-ignore>
        <PageTabs
          value={tab}
          onValueChange={setTab}
          aria-label={t("profiles.editor.sections")}
          idPrefix={`${id}-tab`}
          controls={`${id}-panel`}
          items={[
            { value: "models", label: t("profiles.editor.modelsTab") },
            { value: "behavior", label: t("profiles.editor.behaviorTab") },
            ...(scope === "claude-desktop"
              ? []
              : [
                  {
                    value: "resources" as const,
                    label: `${t("profiles.editor.resourcesTab")} (${config.mcpIds.length + config.skillIds.length})`,
                  },
                  {
                    value: "instructions" as const,
                    label: t("profiles.editor.instructionsTab"),
                  },
                ]),
          ]}
        />
      </div>
      <form
        id="profile-editor-form"
        noValidate
        className="space-y-5"
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        {invalidModel && (
          <div
            role="alert"
            className="flex flex-wrap items-center gap-2 text-sm text-danger-text"
          >
            <span>{t("profiles.editor.incompleteModels")}</span>
            {tab !== "models" && (
              <Button
                type="button"
                variant="link"
                size="sm"
                onClick={() => setTab("models")}
              >
                {t("profiles.editor.modelsTab")}
              </Button>
            )}
          </div>
        )}
        {nativeError && (
          <div
            role="alert"
            className="flex flex-wrap items-center gap-2 text-sm text-danger-text"
          >
            <span>
              {t("profiles.editor.nativeInvalid", { detail: nativeError })}
            </span>
            {tab !== "behavior" && (
              <Button
                type="button"
                variant="link"
                size="sm"
                onClick={() => setTab("behavior")}
              >
                {t("profiles.editor.behaviorTab")}
              </Button>
            )}
          </div>
        )}
        <fieldset disabled={mutation.isPending} className="space-y-5">
          <div
            id={`${id}-panel`}
            role="tabpanel"
            aria-labelledby={`${id}-tab-${tab}`}
          >
            <div hidden={tab !== "models"} className="space-y-6">
              <div className="grid gap-4 rounded-panel border border-border bg-surface p-4 sm:grid-cols-2">
                <label className="space-y-2 text-sm">
                  <span>{t("profiles.editor.name")}</span>
                  <Input
                    value={name}
                    onChange={(event) => setName(event.target.value)}
                    autoFocus
                  />
                </label>
                <label className="space-y-2 text-sm">
                  <span>{t("profiles.editor.provider")}</span>
                  <select
                    className={selectClass}
                    value={config.providerId ?? ""}
                    onChange={(event) => {
                      const providerId = event.target.value || null;
                      setConfig((current) => ({
                        ...current,
                        providerId,
                      }));
                    }}
                  >
                    <option value="">
                      {t("profiles.editor.inheritProvider")}
                    </option>
                    {config.providerId && !providers[config.providerId] && (
                      <option value={config.providerId}>
                        {t("profiles.editor.missing", {
                          id: config.providerId,
                        })}
                      </option>
                    )}
                    {Object.values(providers).map((item) => (
                      <option key={item.id} value={item.id}>
                        {item.name}
                      </option>
                    ))}
                  </select>
                </label>
                <p className="text-xs leading-relaxed text-fg-2 sm:col-span-2">
                  {t("profiles.editor.providerReferenceHint")}
                </p>
              </div>
              {scope !== "claude-desktop" && (
                <ProfileModelFields
                  key={`${scope}:${provider?.id ?? "inherit"}`}
                  provider={provider}
                  scope={scope}
                  config={config}
                  setConfig={setConfig}
                />
              )}
            </div>
            <div
              hidden={tab !== "behavior"}
              className="space-y-4 rounded-panel border border-border bg-surface p-4"
            >
              {scope === "codex" ? (
                <CodexConfigSection
                  context="profile"
                  value={config.nativeConfig ?? ""}
                  onChange={(nativeConfig) =>
                    setConfig((current) => ({ ...current, nativeConfig }))
                  }
                />
              ) : (
                <CommonConfigEditor
                  context="profile"
                  showQuickToggles={scope === "claude"}
                  value={config.nativeConfig ?? "{}"}
                  onChange={(nativeConfig) =>
                    setConfig((current) => ({ ...current, nativeConfig }))
                  }
                />
              )}
            </div>
            {scope !== "claude-desktop" && (
              <div hidden={tab !== "resources"} className="space-y-5">
                <div data-unsaved-ignore>
                  <Input
                    aria-label={t("profiles.editor.searchResources")}
                    placeholder={t("profiles.editor.searchResources")}
                    value={resourceSearch}
                    onChange={(event) => setResourceSearch(event.target.value)}
                  />
                </div>
                <div className="grid gap-5 lg:grid-cols-2">
                  {resources.map(({ key, kind, label, available }) => {
                    const selected = config[key];
                    const availableIds = new Set(
                      available.map((item) => item.id),
                    );
                    const missingIds = [
                      ...new Set([
                        ...(profile.payload[kind][scope] ?? []),
                        ...selected,
                      ]),
                    ].filter((resourceId) => !availableIds.has(resourceId));
                    const entries = [
                      ...available.map((item) => ({
                        id: item.id,
                        name: item.name,
                        description: item.description,
                        missing: false,
                      })),
                      ...missingIds.map((resourceId) => ({
                        id: resourceId,
                        name:
                          profile.payload.resourceNames?.[scope]?.[kind]?.[
                            resourceId
                          ] || resourceId,
                        description: undefined,
                        missing: true,
                      })),
                    ];
                    const search = resourceSearch.trim().toLocaleLowerCase();
                    const visible = entries.filter(
                      (item) =>
                        !search ||
                        `${item.name} ${item.id} ${item.description ?? ""}`
                          .toLocaleLowerCase()
                          .includes(search),
                    );
                    return (
                      <fieldset
                        key={key}
                        className="min-w-0 rounded-panel border border-border p-4"
                      >
                        <legend className="px-1 text-sm font-medium">
                          {label} ·{" "}
                          {t("profiles.editor.selectedCount", {
                            count: selected.length,
                          })}
                        </legend>
                        {missingIds.length > 0 && (
                          <p className="mb-3 text-xs text-fg-2">
                            {t("profiles.editor.missingResourcesHint")}
                          </p>
                        )}
                        {visible.length === 0 && (
                          <p className="py-4 text-sm text-fg-2">
                            {t(
                              entries.length
                                ? "profiles.editor.noMatchingResources"
                                : "profiles.editor.emptyResources",
                            )}
                          </p>
                        )}
                        <div className="max-h-[50vh] space-y-2 overflow-y-auto">
                          {visible.map((resource) => (
                            <label
                              key={resource.id}
                              className={`flex min-h-11 cursor-pointer items-start gap-3 rounded-control border px-3 py-2.5 text-sm ${selected.includes(resource.id) ? "border-primary/30 bg-primary/5" : "border-border hover:bg-muted/40"}`}
                            >
                              <Checkbox
                                className="mt-0.5"
                                checked={selected.includes(resource.id)}
                                onCheckedChange={(checked) =>
                                  toggleResource(
                                    key,
                                    resource.id,
                                    checked === true,
                                  )
                                }
                              />
                              <span className="min-w-0">
                                <span className="block break-words font-medium">
                                  {resource.name}
                                </span>
                                {resource.description && (
                                  <span className="mt-1 line-clamp-2 block text-xs text-fg-2">
                                    {resource.description}
                                  </span>
                                )}
                                {resource.missing && (
                                  <span className="block break-all text-xs text-fg-2">
                                    {t("profiles.editor.unavailable")}
                                    {resource.name !== resource.id &&
                                      ` (${resource.id})`}
                                  </span>
                                )}
                              </span>
                            </label>
                          ))}
                        </div>
                      </fieldset>
                    );
                  })}
                </div>
              </div>
            )}
            {scope !== "claude-desktop" && (
              <div hidden={tab !== "instructions"} className="space-y-4">
                <label className="block space-y-2 text-sm">
                  <span>{t("profiles.editor.prompt")}</span>
                  <select
                    className={selectClass}
                    value={config.promptId ?? ""}
                    onChange={(event) =>
                      setConfig((current) => ({
                        ...current,
                        promptId: event.target.value || null,
                      }))
                    }
                  >
                    <option value="">{t("profiles.editor.noPrompt")}</option>
                    {missingPromptId && (
                      <option value={missingPromptId}>
                        {missingPromptName
                          ? `${missingPromptName} (${missingPromptId}) · ${t("profiles.editor.unavailable")}`
                          : t("profiles.editor.missing", {
                              id: missingPromptId,
                            })}
                      </option>
                    )}
                    {Object.values(prompts).map((prompt) => (
                      <option key={prompt.id} value={prompt.id}>
                        {prompt.name}
                      </option>
                    ))}
                  </select>
                </label>
                <p className="text-sm text-fg-2">
                  {t("profiles.editor.instructionHint")}
                </p>
                {selectedPrompt && (
                  <pre className="max-h-[50vh] overflow-auto whitespace-pre-wrap break-words rounded-panel border border-border bg-surface p-4 font-mono text-xs leading-relaxed">
                    {selectedPrompt.content}
                  </pre>
                )}
              </div>
            )}
          </div>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
        </fieldset>
      </form>
    </FullScreenPanel>
  );
}
