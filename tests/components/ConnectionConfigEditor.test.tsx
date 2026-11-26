import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { parse } from "smol-toml";
import {
  ConnectionConfigEditor,
  extractConnectionSettings,
  mergeConnectionSettings,
} from "@/components/providers/forms/ConnectionConfigEditor";

vi.mock("@/components/JsonEditor", () => ({
  default: ({
    value,
    onChange,
  }: {
    value: string;
    onChange: (value: string) => void;
  }) => (
    <textarea
      aria-label="connection-json"
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));

describe("provider connection editor", () => {
  it.each([null, [], "AWS_REGION=us-east-1", 42])(
    "rejects malformed env without deleting cloud credentials: %j",
    (env) => {
      const onChange = vi.fn();
      const onValidityChange = vi.fn();
      render(
        <ConnectionConfigEditor
          app="claude"
          value={{
            env: {
              AWS_SECRET_ACCESS_KEY: "saved-secret",
              AWS_REGION: "us-east-1",
            },
          }}
          onChange={onChange}
          onValidityChange={onValidityChange}
          darkMode={false}
        />,
      );
      fireEvent.change(screen.getByLabelText("connection-json"), {
        target: { value: JSON.stringify({ env }) },
      });
      expect(screen.getByRole("alert")).toHaveTextContent(
        "env 必须是 JSON 对象",
      );
      expect(onValidityChange).toHaveBeenLastCalledWith(false);
      expect(onChange).not.toHaveBeenCalled();
    },
  );
  it("keeps cloud credentials and helpers editable without exposing Agent behavior", () => {
    const settings = {
      env: {
        AWS_REGION: "us-east-1",
        ANTHROPIC_MODEL: "saved-model",
        DEBUG: "1",
        ANTHROPIC_CUSTOM_HEADERS: "X-Key: secret",
      },
      apiKeyHelper: "read-key",
      hooks: { Stop: [] },
    };
    expect(extractConnectionSettings("claude", settings)).toEqual({
      apiKeyHelper: "read-key",
      env: {
        AWS_REGION: "us-east-1",
        ANTHROPIC_CUSTOM_HEADERS: "X-Key: secret",
      },
    });
    const merged = mergeConnectionSettings("claude", settings, {
      env: { AWS_REGION: "eu-west-1" },
    });
    expect(merged).toEqual({
      env: {
        AWS_REGION: "eu-west-1",
        ANTHROPIC_MODEL: "saved-model",
        DEBUG: "1",
      },
      hooks: { Stop: [] },
    });
  });

  it("retains Codex provider headers while preserving model and approval settings", () => {
    const settings = {
      auth: { OPENAI_API_KEY: "key" },
      config:
        'model = "m-1"\napproval_policy = "never"\nmodel_provider = "custom"\n[model_providers.custom]\nbase_url = "https://old.example"\nhttp_headers = { X-Client = "cc" }\n',
    };
    const connection = extractConnectionSettings("codex", settings);
    expect(parse(connection.config as string)).not.toHaveProperty("model");
    const merged = mergeConnectionSettings("codex", settings, {
      ...connection,
      auth: { OPENAI_API_KEY: "new" },
    });
    expect(parse(merged.config as string)).toMatchObject({
      model: "m-1",
      approval_policy: "never",
      model_providers: { custom: { http_headers: { "X-Client": "cc" } } },
    });
  });

  it("blocks behavior fields rather than silently discarding typed JSON", () => {
    const onChange = vi.fn();
    const onValidityChange = vi.fn();
    render(
      <ConnectionConfigEditor
        app="claude"
        value={{ env: {} }}
        onChange={onChange}
        onValidityChange={onValidityChange}
        darkMode={false}
      />,
    );
    fireEvent.change(screen.getByLabelText("connection-json"), {
      target: { value: '{"hooks":{"Stop":[]}}' },
    });
    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(onValidityChange).toHaveBeenLastCalledWith(false);
    expect(onChange).not.toHaveBeenCalled();
  });
});
