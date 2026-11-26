export const CLAUDE_MODEL_ROLE_FIELDS = [
  {
    role: "sonnet",
    labelKey: "providerForm.modelRoleSonnet",
    modelField: "ANTHROPIC_DEFAULT_SONNET_MODEL",
    displayNameField: "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
    inputId: "claudeDefaultSonnetModel",
  },
  {
    role: "opus",
    labelKey: "providerForm.modelRoleOpus",
    modelField: "ANTHROPIC_DEFAULT_OPUS_MODEL",
    displayNameField: "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
    inputId: "claudeDefaultOpusModel",
  },
  {
    role: "fable",
    labelKey: "providerForm.modelRoleFable",
    modelField: "ANTHROPIC_DEFAULT_FABLE_MODEL",
    displayNameField: "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME",
    inputId: "claudeDefaultFableModel",
  },
  {
    role: "haiku",
    labelKey: "providerForm.modelRoleHaiku",
    modelField: "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    displayNameField: "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
    inputId: "claudeDefaultHaikuModel",
  },
] as const;

export const CLAUDE_SUBAGENT_MODEL_FIELD =
  "CLAUDE_CODE_SUBAGENT_MODEL" as const;

export const CODEX_AGENT_MODEL_FIELDS = {
  subagentModel: "default_subagent_model",
  subagentReasoningEffort: "default_subagent_reasoning_effort",
  maxThreads: "max_threads",
} as const;
