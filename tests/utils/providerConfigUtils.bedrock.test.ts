import { describe, expect, it } from "vitest";
import {
  getApiKeyFromConfig,
  hasApiKeyField,
  setApiKeyInConfig,
} from "@/utils/providerConfigUtils";

const bedrockConfig = (env: Record<string, string>, extra = {}) =>
  JSON.stringify({
    ...extra,
    env: { CLAUDE_CODE_USE_BEDROCK: "1", AWS_REGION: "us-west-2", ...env },
  });

describe("Bedrock API Key field", () => {
  it("reads and writes env.AWS_BEARER_TOKEN_BEDROCK", () => {
    const config = bedrockConfig({ AWS_BEARER_TOKEN_BEDROCK: "" });
    expect(hasApiKeyField(config, "claude")).toBe(true);

    const updated = setApiKeyInConfig(config, "bedrock-key", {
      appType: "claude",
    });
    const parsed = JSON.parse(updated);
    expect(parsed.env.AWS_BEARER_TOKEN_BEDROCK).toBe("bedrock-key");
    expect(parsed).not.toHaveProperty("apiKey");
    expect(parsed.env).not.toHaveProperty("ANTHROPIC_AUTH_TOKEN");
    expect(getApiKeyFromConfig(updated, "claude")).toBe("bedrock-key");
  });

  it("reads only the native Bedrock token field", () => {
    const both = bedrockConfig(
      { AWS_BEARER_TOKEN_BEDROCK: "env-key" },
      { apiKey: "stale-key" },
    );
    expect(getApiKeyFromConfig(both, "claude")).toBe("env-key");
    const topLevelOnly = bedrockConfig({}, { apiKey: "unused-key" });
    expect(getApiKeyFromConfig(topLevelOnly, "claude")).toBe("");
    expect(hasApiKeyField(topLevelOnly, "claude")).toBe(false);
    expect(setApiKeyInConfig(topLevelOnly, "new-key")).toBe(topLevelOnly);
  });
});
