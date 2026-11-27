import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { parse, stringify } from "smol-toml";
import JsonEditor from "@/components/JsonEditor";
import { Label } from "@/components/ui/label";
import {
  isCodexRemoteCompactionEnabled,
  setCodexRemoteCompaction,
} from "@/utils/codexConfigUtils";
import {
  CLAUDE_MODEL_ROLE_FIELDS,
  CLAUDE_SUBAGENT_MODEL_FIELD,
} from "./modelSettings";

const record = (value: unknown): Record<string, unknown> =>
  value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};

// Mirrors the backend Profile connection boundary; the backend enforces it on save.
const claudeModelKeys = new Set<string>([
  "ANTHROPIC_MODEL",
  "ANTHROPIC_SMALL_FAST_MODEL",
  CLAUDE_SUBAGENT_MODEL_FIELD,
  ...CLAUDE_MODEL_ROLE_FIELDS.flatMap(({ modelField, displayNameField }) => [
    modelField,
    displayNameField,
  ]),
]);
const claudeConnectionEnv = (key: string) =>
  !claudeModelKeys.has(key) &&
  (/^(ANTHROPIC_|AWS_|VERTEX_REGION_)/.test(key) ||
    /^CLAUDE_CODE_SKIP_.*_AUTH$/.test(key) ||
    [
      "CLOUD_ML_REGION",
      "GOOGLE_APPLICATION_CREDENTIALS",
      "CLAUDE_CODE_OAUTH_TOKEN",
      "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
      "CLAUDE_CODE_OAUTH_SCOPES",
      "CLAUDE_CODE_API_KEY_HELPER_TTL_MS",
      "CLAUDE_CODE_USE_BEDROCK",
      "CLAUDE_CODE_USE_VERTEX",
      "CLAUDE_CODE_USE_FOUNDRY",
      "CLAUDE_CODE_USE_GATEWAY",
      "CLAUDE_CODE_USE_MANTLE",
      "CLAUDE_CODE_USE_ANTHROPIC_AWS",
      "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
    ].includes(key));
const claudeConnectionTop = (key: string) =>
  [
    "apiKeyHelper",
    "apiBaseUrl",
    "apiKey",
    "awsAuthRefresh",
    "awsCredentialExport",
    "gcpAuthRefresh",
  ].includes(key);
const codexConnectionTop = (key: string) =>
  [
    "model_provider",
    "openai_base_url",
    "model_providers",
    "experimental_bearer_token",
    "base_url",
    "wire_api",
    "auth",
  ].includes(key);
const select = (
  value: Record<string, unknown>,
  predicate: (key: string) => boolean,
) =>
  Object.fromEntries(Object.entries(value).filter(([key]) => predicate(key)));

export function extractConnectionSettings(
  app: "claude" | "codex",
  settings: Record<string, unknown>,
): Record<string, unknown> {
  if (app === "codex") {
    const config = parse(
      typeof settings.config === "string" ? settings.config : "",
    );
    return {
      auth: record(settings.auth),
      config: stringify(select(config, codexConnectionTop)),
    };
  }
  return {
    ...select(settings, claudeConnectionTop),
    env: select(record(settings.env), claudeConnectionEnv),
  };
}

export function mergeConnectionSettings(
  app: "claude" | "codex",
  settings: Record<string, unknown>,
  connection: Record<string, unknown>,
): Record<string, unknown> {
  const projected = extractConnectionSettings(app, connection);
  if (app === "codex") {
    const config = parse(
      typeof settings.config === "string" ? settings.config : "",
    );
    return {
      ...settings,
      ...projected,
      config: stringify({
        ...select(config, (key) => !codexConnectionTop(key)),
        ...parse(projected.config as string),
      }),
    };
  }
  return {
    ...select(settings, (key) => !claudeConnectionTop(key)),
    ...projected,
    env: {
      ...select(record(settings.env), (key) => !claudeConnectionEnv(key)),
      ...record(projected.env),
    },
  };
}

export function ConnectionConfigEditor({
  app,
  value,
  onChange,
  onValidityChange,
  darkMode,
  providerName,
  showRemoteCompaction = true,
}: {
  app: "claude" | "codex";
  value: Record<string, unknown>;
  onChange: (value: Record<string, unknown>) => void;
  onValidityChange: (valid: boolean) => void;
  darkMode: boolean;
  providerName?: string;
  showRemoteCompaction?: boolean;
}) {
  const { t } = useTranslation();
  const connection = extractConnectionSettings(app, value);
  const serialized =
    app === "codex"
      ? String(connection.config ?? "")
      : JSON.stringify(connection, null, 2);
  const serializedAuth = JSON.stringify(connection.auth ?? {}, null, 2);
  const [draft, setDraft] = useState(serialized);
  const [error, setError] = useState("");
  const [authDraft, setAuthDraft] = useState(serializedAuth);
  const [authError, setAuthError] = useState("");
  useEffect(() => {
    setDraft(serialized);
    setError("");
  }, [serialized]);
  useEffect(() => {
    setAuthDraft(serializedAuth);
    setAuthError("");
  }, [serializedAuth]);
  useEffect(() => {
    onValidityChange(!error && !authError);
  }, [error, authError, onValidityChange]);
  return (
    <div className="space-y-2">
      <Label>
        {t("provider.connectionConfig", { defaultValue: "高级连接配置" })}
      </Label>
      <p className="text-caption text-fg-2">
        {t("provider.connectionConfigHint", {
          defaultValue:
            "仅编辑认证、端点和连接参数。模型与 Agent 行为请在 Profile 中配置。",
        })}
      </p>
      {app === "codex" && showRemoteCompaction && (
        <label
          className="inline-flex cursor-pointer items-center gap-2 text-sm text-fg-2"
          title={t("codexConfig.remoteCompactionHint")}
        >
          <input
            type="checkbox"
            className="ui-checkbox"
            checked={isCodexRemoteCompactionEnabled(draft)}
            disabled={Boolean(error)}
            onChange={(event) => {
              if (error) return;
              const config = setCodexRemoteCompaction(
                draft,
                event.target.checked,
                providerName,
              );
              setDraft(config);
              onChange(
                mergeConnectionSettings("codex", value, {
                  ...connection,
                  config,
                }),
              );
            }}
          />
          {t("codexConfig.enableRemoteCompaction")}
        </label>
      )}
      <JsonEditor
        value={draft}
        rows={6}
        language={app === "codex" ? "javascript" : "json"}
        darkMode={darkMode}
        showValidation={app !== "codex"}
        onChange={(text) => {
          setDraft(text);
          try {
            const parsed: unknown =
              app === "codex"
                ? { auth: value.auth ?? {}, config: text }
                : JSON.parse(text);
            if (!parsed || typeof parsed !== "object" || Array.isArray(parsed))
              throw new Error("配置必须是 JSON 对象");
            const connection = parsed as Record<string, unknown>;
            if (
              app === "claude" &&
              "env" in connection &&
              (!connection.env ||
                typeof connection.env !== "object" ||
                Array.isArray(connection.env))
            ) {
              throw new Error("env 必须是 JSON 对象");
            }
            const projected = extractConnectionSettings(app, connection);
            if (app === "codex") {
              if (
                Object.keys(connection).some(
                  (key) => key !== "auth" && key !== "config",
                ) ||
                Object.keys(
                  parse(
                    typeof connection.config === "string"
                      ? connection.config
                      : "",
                  ),
                ).some((key) => !codexConnectionTop(key))
              ) {
                throw new Error(
                  t("provider.connectionConfigOnly", {
                    defaultValue: "模型和行为字段请移到 Profile。",
                  }),
                );
              }
            } else if (
              Object.keys(connection).some(
                (key) => key !== "env" && !claudeConnectionTop(key),
              ) ||
              Object.keys(record(connection.env)).some(
                (key) => !claudeConnectionEnv(key),
              )
            ) {
              throw new Error(
                t("provider.connectionConfigOnly", {
                  defaultValue: "模型和行为字段请移到 Profile。",
                }),
              );
            }
            setError("");
            onChange(mergeConnectionSettings(app, value, projected));
          } catch (cause) {
            setError(cause instanceof Error ? cause.message : String(cause));
          }
        }}
      />
      {app === "codex" && (
        <details className="space-y-2">
          <summary className="cursor-pointer text-caption text-fg-2">
            auth.json
          </summary>
          <JsonEditor
            value={authDraft}
            rows={4}
            language="json"
            darkMode={darkMode}
            showValidation
            onChange={(text) => {
              setAuthDraft(text);
              try {
                const auth: unknown = JSON.parse(text);
                if (!auth || typeof auth !== "object" || Array.isArray(auth))
                  throw new Error("auth.json 必须是 JSON 对象");
                setAuthError("");
                onChange({ ...value, auth });
              } catch (cause) {
                setAuthError(
                  cause instanceof Error ? cause.message : String(cause),
                );
              }
            }}
          />
        </details>
      )}
      {authError && (
        <p role="alert" className="text-caption text-destructive">
          {authError}
        </p>
      )}
      {error && (
        <p role="alert" className="text-caption text-destructive">
          {error}
        </p>
      )}
    </div>
  );
}
