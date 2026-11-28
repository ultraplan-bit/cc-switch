/**
 * GitHub Copilot CLI 卡片的配置形状（与后端 `copilot_config.rs` 严格对应）。
 *
 * - 官方卡：`{ env: { COPILOT_MODEL? }, reasoningEffort? }`，只用于「打开终端」，不写任何文件。
 * - 自带模型（BYOK）卡：`{ provider, models, reasoningEffort? }`。「添加」把它写进 Copilot 原生的
 *   `providers.json`：`provider` 补上 `name = 卡片 id` 成为一个供应商条目，`models` 每项补上
 *   `provider = 卡片 id`；Copilot 的 `/model` 里显示为 `<卡片 id>/<模型 id>`。`reasoningEffort`
 *   是卡片自己的字段，只给「打开终端」的 `--reasoning-effort` 用，不进 providers.json。
 *
 * 这个文件不引用预设目录，供卡片、额度、终端等轻量地判断卡片类型。
 */

export const COPILOT_PROVIDER_TYPES = ["openai", "azure", "anthropic"] as const;
export type CopilotProviderType = (typeof COPILOT_PROVIDER_TYPES)[number];
export const COPILOT_WIRE_APIS = ["completions", "responses"] as const;
export type CopilotWireApi = (typeof COPILOT_WIRE_APIS)[number];

/** providers.json 的供应商条目去掉 `name`（name 永远是卡片 id）。未列出的键原样保留。 */
export interface CopilotProviderNode {
  type?: CopilotProviderType;
  wireApi?: CopilotWireApi;
  transport?: "http" | "websockets";
  baseUrl: string;
  apiKey?: string;
  bearerToken?: string;
  azure?: { apiVersion?: string } & Record<string, unknown>;
  headers?: Record<string, string>;
  [key: string]: unknown;
}

/** providers.json 的模型条目去掉 `provider`（写文件时补上卡片 id）。未列出的键原样保留。 */
export interface CopilotModel {
  id: string;
  name?: string;
  wireModel?: string;
  maxPromptTokens?: number;
  maxContextWindowTokens?: number;
  maxOutputTokens?: number;
  [key: string]: unknown;
}

/** Copilot reserves output tokens before building the prompt. Let it choose its
 * default when a catalog advertises the whole context window as output. */
export function copilotCatalogOutputLimit(
  output: number | undefined,
  context: number | undefined,
): number | undefined {
  return output && output > 0 && (!context || output < context)
    ? output
    : undefined;
}

export interface CopilotByokConfig {
  provider: CopilotProviderNode;
  models: CopilotModel[];
  reasoningEffort?: string;
}

/** Copilot CLI `--reasoning-effort` 的取值（与后端 `REASONING_EFFORTS` 一致，tests/config/backendConstants.test.ts 校验）。 */
export const COPILOT_REASONING_EFFORTS = [
  "none",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
] as const;

/**
 * 卡片 id 就是 providers.json 里的供应商名：非空、不含 `/`、空白和控制字符，不超过 64 字符。
 * 表单只让输入 `[A-Za-z0-9._-]`，比后端的规则更严。
 */
export const COPILOT_PROVIDER_ID_MAX = 64;
export function isValidCopilotProviderId(id: string): boolean {
  return (
    id.length > 0 &&
    id.length <= COPILOT_PROVIDER_ID_MAX &&
    !/[/\s\p{Cc}]/u.test(id)
  );
}
export const normalizeCopilotProviderId = (value: string) =>
  value.replace(/[^A-Za-z0-9._-]/g, "").slice(0, COPILOT_PROVIDER_ID_MAX);

const isPlainObject = (value: unknown): value is Record<string, unknown> =>
  Boolean(value) && typeof value === "object" && !Array.isArray(value);

/** 官方卡 `env` 中的字符串值，如 `COPILOT_MODEL`。 */
export function copilotEnvOf(settingsConfig: unknown): Record<string, string> {
  const env = isPlainObject(settingsConfig) ? settingsConfig.env : undefined;
  if (!isPlainObject(env)) return {};
  return Object.fromEntries(
    Object.entries(env).filter(
      (entry): entry is [string, string] => typeof entry[1] === "string",
    ),
  );
}

function copilotProviderNodeOf(
  settingsConfig: unknown,
): CopilotProviderNode | null {
  const provider = isPlainObject(settingsConfig)
    ? settingsConfig.provider
    : undefined;
  return isPlainObject(provider) ? (provider as CopilotProviderNode) : null;
}

/** 有 `provider` 节点就是自带模型卡。 */
export function isCopilotByok(settingsConfig: unknown): boolean {
  return copilotProviderNodeOf(settingsConfig) !== null;
}

/** 官方卡：category 是 official，或者配置里没有自带模型。 */
export function isCopilotOfficialCard(provider: {
  category?: string;
  settingsConfig: unknown;
}): boolean {
  return (
    provider.category === "official" || !isCopilotByok(provider.settingsConfig)
  );
}

export function reasoningEffortOf(settingsConfig: unknown): string {
  const value = isPlainObject(settingsConfig)
    ? settingsConfig.reasoningEffort
    : undefined;
  return typeof value === "string" &&
    (COPILOT_REASONING_EFFORTS as readonly string[]).includes(value.trim())
    ? value.trim()
    : "";
}

/** 读出自带模型卡的配置；不是自带模型卡时给一份空白配置。 */
export function copilotByokConfigOf(
  settingsConfig: unknown,
): CopilotByokConfig {
  const reasoningEffort = reasoningEffortOf(settingsConfig);
  const node = copilotProviderNodeOf(settingsConfig);
  let config: CopilotByokConfig;
  if (node) {
    const models = isPlainObject(settingsConfig)
      ? settingsConfig.models
      : undefined;
    config = {
      provider: {
        ...node,
        baseUrl: typeof node.baseUrl === "string" ? node.baseUrl : "",
      },
      models: Array.isArray(models)
        ? models.filter(isPlainObject).map((model) => ({
            ...model,
            id: typeof model.id === "string" ? model.id : "",
          }))
        : [],
    };
  } else {
    config = emptyCopilotByokConfig();
  }
  return reasoningEffort ? { ...config, reasoningEffort } : config;
}

export function emptyCopilotByokConfig(): CopilotByokConfig {
  return {
    provider: { type: "openai", wireApi: "completions", baseUrl: "" },
    models: [],
  };
}

/**
 * 表单里的「接口格式」：一项同时决定 `type` 和 `wireApi`。Anthropic 只有 Messages 接口，
 * 不写 `wireApi`。
 */
export const COPILOT_API_FORMATS = [
  {
    id: "openai-completions",
    label: "OpenAI Chat Completions",
    type: "openai",
    wireApi: "completions",
  },
  {
    id: "openai-responses",
    label: "OpenAI Responses",
    type: "openai",
    wireApi: "responses",
  },
  { id: "anthropic", label: "Anthropic Messages", type: "anthropic" },
  {
    id: "azure-completions",
    label: "Azure OpenAI Chat Completions",
    type: "azure",
    wireApi: "completions",
  },
  {
    id: "azure-responses",
    label: "Azure OpenAI Responses",
    type: "azure",
    wireApi: "responses",
  },
] as const satisfies ReadonlyArray<{
  id: string;
  label: string;
  type: CopilotProviderType;
  wireApi?: CopilotWireApi;
}>;
export type CopilotApiFormatId = (typeof COPILOT_API_FORMATS)[number]["id"];

/** 按 Copilot CLI 的默认值（openai / completions）读出当前格式。 */
export function copilotApiFormatOf(
  provider: Pick<CopilotProviderNode, "type" | "wireApi">,
): CopilotApiFormatId {
  const type = provider.type || "openai";
  if (type === "anthropic") return "anthropic";
  const responses = provider.wireApi === "responses";
  if (type === "azure")
    return responses ? "azure-responses" : "azure-completions";
  return responses ? "openai-responses" : "openai-completions";
}

/**
 * 换接口格式：同时改 `type` / `wireApi`；离开 Azure 时去掉 `azure`，离开 Responses 时去掉
 * websockets 传输（Copilot 只允许 websockets 配 responses，否则整个文件被拒）。
 */
export function applyCopilotApiFormat(
  provider: CopilotProviderNode,
  id: CopilotApiFormatId,
): CopilotProviderNode {
  const format = COPILOT_API_FORMATS.find((item) => item.id === id);
  if (!format) return provider;
  const next: CopilotProviderNode = { ...provider, type: format.type };
  if ("wireApi" in format) next.wireApi = format.wireApi;
  else delete next.wireApi;
  if (format.type !== "azure") delete next.azure;
  if (next.wireApi !== "responses" && next.transport === "websockets") {
    delete next.transport;
  }
  return next;
}
