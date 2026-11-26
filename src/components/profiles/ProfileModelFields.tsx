import { useMemo, useState, type Dispatch, type SetStateAction } from "react";
import { useQuery } from "@tanstack/react-query";
import { FormProvider, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { parse as parseToml } from "smol-toml";
import { ClaudeFormFields } from "@/components/providers/forms/ClaudeFormFields";
import { CodexFormFields } from "@/components/providers/forms/CodexFormFields";
import { mapCodexCatalogModelForForm } from "@/components/providers/forms/hooks/useCodexConfigState";
import { parseModelsFromConfig } from "@/components/providers/forms/hooks/useModelState";
import { CLAUDE_MODEL_ROLE_FIELDS } from "@/components/providers/forms/modelSettings";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { authGetStatus, type ManagedAuthProvider } from "@/lib/api/auth";
import { resolveManagedAccountId } from "@/lib/authBinding";
import type { ProfileConfig, ProfileScope } from "@/lib/api/profiles";
import type { CodexApiFormat, CodexCatalogModel, Provider } from "@/types";
import {
  extractCodexBaseUrl,
  extractCodexExperimentalBearerToken,
  extractCodexModelName,
} from "@/utils/codexConfigUtils";
import { getApiKeyFromConfig } from "@/utils/providerConfigUtils";

const noop = () => {};
const asRecord = (value: unknown): Record<string, unknown> =>
  value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};

export function providerModelDefaults(
  provider: Provider | undefined,
  scope: ProfileScope,
): Pick<ProfileConfig, "model" | "modelSettings" | "modelCatalog"> {
  if (scope === "claude-desktop")
    return { model: "", modelSettings: {}, modelCatalog: null };
  if (scope === "claude") {
    const models = parseModelsFromConfig(
      JSON.stringify(provider?.settingsConfig ?? {}),
    );
    return {
      model: models.model,
      modelSettings: {
        ANTHROPIC_DEFAULT_HAIKU_MODEL: models.haiku,
        ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME: models.haikuName,
        ANTHROPIC_DEFAULT_SONNET_MODEL: models.sonnet,
        ANTHROPIC_DEFAULT_SONNET_MODEL_NAME: models.sonnetName,
        ANTHROPIC_DEFAULT_OPUS_MODEL: models.opus,
        ANTHROPIC_DEFAULT_OPUS_MODEL_NAME: models.opusName,
        ANTHROPIC_DEFAULT_FABLE_MODEL: models.fable,
        ANTHROPIC_DEFAULT_FABLE_MODEL_NAME: models.fableName,
        CLAUDE_CODE_SUBAGENT_MODEL: models.subagent,
      },
      modelCatalog: null,
    };
  }
  const text = provider?.settingsConfig.config;
  let document: Record<string, unknown> = {};
  try {
    if (typeof text === "string") document = asRecord(parseToml(text));
  } catch {
    /* An unreadable provider does not erase the Profile draft. */
  }
  const modelSettings: Record<string, string> = {};
  for (const key of [
    "model_reasoning_effort",
    "plan_mode_reasoning_effort",
    "model_context_window",
    "model_auto_compact_token_limit",
    "agents.default_subagent_model",
    "agents.default_subagent_reasoning_effort",
    "agents.max_threads",
  ]) {
    const value = key.startsWith("agents.")
      ? asRecord(document.agents)[key.slice(7)]
      : document[key];
    modelSettings[key] =
      typeof value === "string" || typeof value === "number"
        ? String(value)
        : "";
  }
  const raw = asRecord(provider?.settingsConfig.modelCatalog);
  return {
    model: extractCodexModelName(text) ?? "",
    modelSettings,
    modelCatalog: {
      ...raw,
      models: Array.isArray(raw.models)
        ? raw.models.map(mapCodexCatalogModelForForm)
        : [],
      autoReviewModel:
        typeof raw.autoReviewModel === "string" ? raw.autoReviewModel : "",
      officialSpeedTiers: raw.officialSpeedTiers === true,
    },
  };
}

interface ProfileModelFieldsProps {
  provider: Provider | undefined;
  scope: "claude" | "codex";
  config: ProfileConfig;
  setConfig: Dispatch<SetStateAction<ProfileConfig>>;
}

export function ProfileModelFields(props: ProfileModelFieldsProps) {
  const form = useForm();
  return (
    <FormProvider {...form}>
      <ProfileModelEditor {...props} />
    </FormProvider>
  );
}

function ProfileModelEditor({
  provider,
  scope,
  config,
  setConfig,
}: ProfileModelFieldsProps) {
  const { t } = useTranslation();
  const baseUrl =
    scope === "claude"
      ? String(provider?.settingsConfig.env?.ANTHROPIC_BASE_URL ?? "")
      : (extractCodexBaseUrl(provider?.settingsConfig.config ?? "") ?? "");
  const providerType = provider?.meta?.providerType;
  const isCopilot =
    providerType === "github_copilot" ||
    (scope === "claude" && baseUrl.includes("githubcopilot.com"));
  const isCodexOauth =
    providerType === "codex_oauth" ||
    (scope === "codex" && provider?.category === "official");
  const isXaiOauth = providerType === "xai_oauth";
  const authProvider: ManagedAuthProvider | undefined = isCopilot
    ? "github_copilot"
    : isCodexOauth
      ? "codex_oauth"
      : isXaiOauth
        ? "xai_oauth"
        : undefined;
  const accountId = authProvider
    ? resolveManagedAccountId(provider?.meta, authProvider)
    : null;
  const authStatus = useQuery({
    queryKey: ["managed-auth-status", authProvider],
    queryFn: () => authGetStatus(authProvider!),
    enabled: Boolean(authProvider),
    networkMode: "always",
    staleTime: 30000,
  });
  const authenticated = Boolean(
    authStatus.data?.authenticated &&
      (!accountId ||
        authStatus.data.accounts.some(
          (account) =>
            account.id === accountId &&
            !account.requires_reauth &&
            !account.reauth_required,
        )),
  );
  const settings = config.modelSettings;
  const updateSetting = (key: string, value: string) =>
    setConfig((current) => ({
      ...current,
      modelSettings: { ...current.modelSettings, [key]: value },
    }));
  const catalog = config.modelCatalog ?? { models: [] };
  const rows = useMemo(
    () =>
      Array.isArray(catalog.models)
        ? catalog.models.map(mapCodexCatalogModelForForm)
        : [],
    [catalog.models],
  );
  const updateCatalog = (patch: Record<string, unknown>) =>
    setConfig((current) => ({
      ...current,
      modelCatalog: { ...(current.modelCatalog ?? { models: [] }), ...patch },
    }));
  const shared = {
    providerId: provider?.id,
    category: provider?.category,
    websiteUrl: provider?.websiteUrl ?? "",
    shouldShowApiKeyLink: false,
    isFullUrl: provider?.meta?.isFullUrl ?? false,
    onFullUrlChange: noop,
    isEndpointModalOpen: false,
    onEndpointModalToggle: noop,
    autoSelect: false,
    onAutoSelectChange: noop,
    speedTestEndpoints: [],
    onApiKeyChange: noop,
    onBaseUrlChange: noop,
    onApiFormatChange: noop,
    customUserAgent: provider?.meta?.customUserAgent ?? "",
    onCustomUserAgentChange: noop,
    localProxyHeadersOverride: JSON.stringify(
      provider?.meta?.localProxyRequestOverrides?.headers ?? {},
    ),
    onLocalProxyHeadersOverrideChange: noop,
    localProxyBodyOverride: "",
    onLocalProxyBodyOverrideChange: noop,
    isCopilotPreset: isCopilot,
    isCopilotAuthenticated: authenticated,
    selectedGitHubAccountId: isCopilot ? accountId : null,
    isCodexOauthPreset: isCodexOauth,
    selectedCodexAccountId: isCodexOauth ? accountId : null,
    isXaiOauthPreset: isXaiOauth,
    isXaiOauthAuthenticated: authenticated,
    selectedXaiAccountId: isXaiOauth ? accountId : null,
  };
  // Suggestions are the saved editor context, not the text currently being
  // typed. Adding every keystroke would switch the shared input's empty/list
  // branches and remount the focused input. The parent keys by provider.
  const [claudeSuggestions] = useState(() => {
    const defaults = providerModelDefaults(provider, "claude");
    const suggestions = new Set(
      [
        config.model,
        defaults.model,
        ...CLAUDE_MODEL_ROLE_FIELDS.map((field) => settings[field.modelField]),
        ...CLAUDE_MODEL_ROLE_FIELDS.map(
          (field) => defaults.modelSettings[field.modelField],
        ),
        ...(provider?.meta?.stackModels ?? []).map((row) => row.model),
      ].filter((model): model is string => Boolean(model)),
    );
    return [...suggestions].map((id) => ({
      id,
      ownedBy: provider?.name ?? null,
    }));
  });
  if (scope === "claude") {
    return (
      <div className="space-y-4">
        <div className="flex justify-end">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={!provider}
            onClick={() =>
              setConfig((current) => ({
                ...current,
                ...providerModelDefaults(provider, scope),
              }))
            }
          >
            {t("profiles.editor.resetModels")}
          </Button>
        </div>
        <ClaudeFormFields
          {...shared}
          variant="models"
          shouldShowApiKey={false}
          apiKey={getApiKeyFromConfig(
            JSON.stringify(provider?.settingsConfig ?? {}),
            "claude",
          )}
          baseUrl={baseUrl}
          shouldShowSpeedTest={false}
          shouldShowModelSelector
          modelSuggestions={claudeSuggestions}
          isCodexOauthAuthenticated={authenticated}
          templateValueEntries={[]}
          templateValues={{}}
          templatePresetName=""
          onTemplateValueChange={noop}
          claudeModel={config.model}
          defaultHaikuModel={settings.ANTHROPIC_DEFAULT_HAIKU_MODEL ?? ""}
          defaultHaikuModelName={
            settings.ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME ?? ""
          }
          defaultSonnetModel={settings.ANTHROPIC_DEFAULT_SONNET_MODEL ?? ""}
          defaultSonnetModelName={
            settings.ANTHROPIC_DEFAULT_SONNET_MODEL_NAME ?? ""
          }
          defaultOpusModel={settings.ANTHROPIC_DEFAULT_OPUS_MODEL ?? ""}
          defaultOpusModelName={
            settings.ANTHROPIC_DEFAULT_OPUS_MODEL_NAME ?? ""
          }
          defaultFableModel={settings.ANTHROPIC_DEFAULT_FABLE_MODEL ?? ""}
          defaultFableModelName={
            settings.ANTHROPIC_DEFAULT_FABLE_MODEL_NAME ?? ""
          }
          subagentModel={settings.CLAUDE_CODE_SUBAGENT_MODEL ?? ""}
          onModelChange={(field, value) =>
            field === "ANTHROPIC_MODEL"
              ? setConfig((current) => ({ ...current, model: value }))
              : updateSetting(field, value)
          }
          apiFormat={provider?.meta?.apiFormat ?? "anthropic"}
          reasoningEffort="auto"
          onReasoningEffortChange={noop}
          apiKeyField={provider?.meta?.apiKeyField ?? "ANTHROPIC_AUTH_TOKEN"}
          onApiKeyFieldChange={noop}
        />
      </div>
    );
  }
  const apiFormat: CodexApiFormat =
    provider?.meta?.apiFormat === "openai_chat" ||
    provider?.meta?.apiFormat === "anthropic"
      ? provider.meta.apiFormat
      : "openai_responses";
  const apiKey =
    provider?.settingsConfig.auth?.OPENAI_API_KEY ||
    extractCodexExperimentalBearerToken(
      provider?.settingsConfig.config ?? "",
    ) ||
    "";
  const reasoningLevels = rows.find(
    (row) => row.model.trim() === config.model.trim(),
  )?.reasoningLevels ?? [
    "none",
    "minimal",
    "low",
    "medium",
    "high",
    "xhigh",
    "max",
    "ultra",
  ];
  return (
    <div className="space-y-6">
      <div className="flex justify-end">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={!provider}
          onClick={() =>
            setConfig((current) => ({
              ...current,
              ...providerModelDefaults(provider, scope),
            }))
          }
        >
          {t("profiles.editor.resetModels")}
        </Button>
      </div>
      <CodexFormFields
        {...shared}
        variant="models"
        codexApiKey={apiKey}
        codexBaseUrl={baseUrl}
        shouldShowSpeedTest={false}
        codexModel={config.model}
        onModelChange={(model) =>
          setConfig((current) => ({ ...current, model }))
        }
        subagentModel={settings["agents.default_subagent_model"] ?? ""}
        onSubagentModelChange={(value) =>
          updateSetting("agents.default_subagent_model", value)
        }
        subagentReasoningEffort={
          settings["agents.default_subagent_reasoning_effort"] ?? ""
        }
        onSubagentReasoningEffortChange={(value) =>
          updateSetting("agents.default_subagent_reasoning_effort", value)
        }
        agentMaxThreads={settings["agents.max_threads"] ?? ""}
        onAgentMaxThreadsChange={(value) =>
          updateSetting("agents.max_threads", value)
        }
        apiFormat={apiFormat}
        copilotApiFormat={
          provider?.meta?.codexCopilotApiFormat === "openai_responses"
            ? "openai_responses"
            : provider?.meta?.codexCopilotApiFormat === "openai_chat"
              ? "openai_chat"
              : "auto"
        }
        anthropicAuthField="ANTHROPIC_AUTH_TOKEN"
        onAnthropicAuthFieldChange={noop}
        impersonateClaudeCode={false}
        onImpersonateClaudeCodeChange={noop}
        maxOutputTokens=""
        onMaxOutputTokensChange={noop}
        promptCacheRouting="auto"
        onPromptCacheRoutingChange={noop}
        catalogModels={rows}
        onCatalogModelsChange={(models: CodexCatalogModel[]) =>
          updateCatalog({ models })
        }
        autoReviewModel={
          typeof catalog.autoReviewModel === "string"
            ? catalog.autoReviewModel
            : ""
        }
        onAutoReviewModelChange={(autoReviewModel) =>
          updateCatalog({ autoReviewModel })
        }
        officialSpeedTiers={catalog.officialSpeedTiers === true}
        onOfficialSpeedTiersChange={(officialSpeedTiers) =>
          updateCatalog({ officialSpeedTiers })
        }
      />
      <details
        className="rounded-panel border border-border p-4"
        open={
          Object.entries(settings).some(
            ([key, value]) =>
              [
                "model_reasoning_effort",
                "plan_mode_reasoning_effort",
                "model_context_window",
                "model_auto_compact_token_limit",
              ].includes(key) && Boolean(value),
          ) || undefined
        }
      >
        <summary className="cursor-pointer text-sm font-medium">
          {t("profiles.editor.runtimeSettings")}
        </summary>
        <div className="mt-4 grid gap-4 sm:grid-cols-2">
          {(
            [
              ["model_reasoning_effort", "profiles.editor.mainReasoningEffort"],
              [
                "plan_mode_reasoning_effort",
                "profiles.editor.planReasoningEffort",
              ],
            ] as const
          ).map(([key, label]) => (
            <label key={key} className="space-y-2 text-sm">
              <span>{t(label)}</span>
              <select
                className="h-10 w-full rounded-control border border-border bg-surface px-3"
                value={settings[key] ?? ""}
                onChange={(event) => updateSetting(key, event.target.value)}
              >
                <option value="">
                  {t("codexConfig.subagentReasoningEffortDefault")}
                </option>
                {[
                  ...new Set([
                    ...(settings[key] ? [settings[key]] : []),
                    ...reasoningLevels,
                  ]),
                ].map((level) => (
                  <option key={level} value={level}>
                    {level}
                  </option>
                ))}
              </select>
            </label>
          ))}
          {(
            [
              ["model_context_window", "codexConfig.catalogColumnContext"],
              [
                "model_auto_compact_token_limit",
                "codexConfig.autoCompactLimit",
              ],
            ] as const
          ).map(([key, label]) => (
            <label key={key} className="space-y-2 text-sm">
              <span>{t(label)}</span>
              <Input
                type="number"
                min={1}
                inputMode="numeric"
                value={settings[key] ?? ""}
                onChange={(event) => updateSetting(key, event.target.value)}
              />
            </label>
          ))}
          <label className="flex items-center gap-2 text-sm sm:col-span-2">
            <input
              className="ui-checkbox"
              type="checkbox"
              checked={settings.model_context_window === "1000000"}
              onChange={(event) =>
                setConfig((current) => ({
                  ...current,
                  modelSettings: {
                    ...current.modelSettings,
                    model_context_window: event.target.checked ? "1000000" : "",
                    model_auto_compact_token_limit: event.target.checked
                      ? current.modelSettings.model_auto_compact_token_limit ||
                        "900000"
                      : "",
                  },
                }))
              }
            />
            {t("codexConfig.contextWindow1M")}
          </label>
        </div>
      </details>
    </div>
  );
}
