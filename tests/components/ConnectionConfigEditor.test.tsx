import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { parse } from "smol-toml";
import {
  ConnectionConfigEditor,
  extractConnectionSettings,
  mergeConnectionSettings,
} from "@/components/providers/forms/ConnectionConfigEditor";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

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

  it("enables remote compaction only for the selected Codex connection", () => {
    const settings = {
      auth: { OPENAI_API_KEY: "fixture-key", account_id: "fixture-account" },
      config: [
        'model = "saved-model"',
        'approval_policy = "on-request"',
        'model_provider = "custom"',
        "[model_providers.custom]",
        'name = "Saved provider"',
        'base_url = "https://custom.example/v1"',
        'http_headers = { X-Client = "cc" }',
        "[model_providers.backup]",
        'name = "Backup"',
        'base_url = "https://backup.example/v1"',
        "[features]",
        "shell_tool = true",
      ].join("\n"),
    };
    const before = structuredClone(settings);
    const onChange = vi.fn();
    render(
      <ConnectionConfigEditor
        app="codex"
        value={settings}
        providerName="Current provider"
        onChange={onChange}
        onValidityChange={vi.fn()}
        darkMode={false}
      />,
    );
    const toggle = screen.getByRole("checkbox", {
      name: "codexConfig.enableRemoteCompaction",
    });
    expect(toggle).not.toBeChecked();
    fireEvent.click(toggle);
    expect(onChange).toHaveBeenCalledTimes(1);
    const changed = onChange.mock.calls[0][0];
    const expected = parse(settings.config);
    (expected.model_providers as Record<string, { name: string }>).custom.name =
      "OpenAI";
    expect(parse(changed.config)).toEqual(expected);
    expect(changed.auth).toEqual(settings.auth);
    expect(settings).toEqual(before);
  });

  it("reads an enabled remote compaction setting and restores the current provider name when disabled", () => {
    const settings = {
      auth: { OPENAI_API_KEY: "fixture-key" },
      config:
        'model_provider = "custom"\n[model_providers.custom]\nname = "OpenAI"\nbase_url = "https://custom.example/v1"\n',
    };
    const onChange = vi.fn();
    render(
      <ConnectionConfigEditor
        app="codex"
        value={settings}
        providerName="Renamed provider"
        onChange={onChange}
        onValidityChange={vi.fn()}
        darkMode={false}
      />,
    );
    const toggle = screen.getByRole("checkbox", {
      name: "codexConfig.enableRemoteCompaction",
    });
    expect(toggle).toBeChecked();
    fireEvent.click(toggle);
    expect(parse(onChange.mock.calls[0][0].config)).toEqual({
      model_provider: "custom",
      model_providers: {
        custom: {
          name: "Renamed provider",
          base_url: "https://custom.example/v1",
        },
      },
    });
  });

  it("disables remote compaction while preserving an invalid TOML draft", () => {
    const onChange = vi.fn();
    const onValidityChange = vi.fn();
    render(
      <ConnectionConfigEditor
        app="codex"
        value={{
          auth: {},
          config:
            'model_provider = "custom"\n[model_providers.custom]\nname = "Custom"\n',
        }}
        onChange={onChange}
        onValidityChange={onValidityChange}
        darkMode={false}
      />,
    );
    const editor = screen.getAllByLabelText("connection-json")[0];
    const invalid = 'model_provider = "custom"\n[model_providers.custom';
    fireEvent.change(editor, { target: { value: invalid } });
    const toggle = screen.getByRole("checkbox", {
      name: "codexConfig.enableRemoteCompaction",
    });
    expect(toggle).toBeDisabled();
    fireEvent.click(toggle);
    expect(editor).toHaveValue(invalid);
    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(onValidityChange).toHaveBeenLastCalledWith(false);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("hides the compatibility switch when the official provider disables it", () => {
    render(
      <ConnectionConfigEditor
        app="codex"
        value={{ auth: {}, config: "" }}
        showRemoteCompaction={false}
        onChange={vi.fn()}
        onValidityChange={vi.fn()}
        darkMode={false}
      />,
    );
    expect(
      screen.queryByRole("checkbox", {
        name: "codexConfig.enableRemoteCompaction",
      }),
    ).not.toBeInTheDocument();
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
