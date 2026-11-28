import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "@/lib/toast";
import type { SettingsFormState } from "@/hooks/useSettings";
import { SettingsSwitchRow } from "@/components/settings/SettingsLayout";
import { settingsApi } from "@/lib/api";
import { formatBytes } from "@/components/settings/BackupStorageSection";
import { CodexQuotaProtectionSettings } from "./CodexQuotaProtectionSettings";

interface CodexAuthSettingsProps {
  settings: SettingsFormState;
  /** 返回 false（或 resolve 为 false）表示保存失败；其余返回值视为成功 */
  onChange: (
    updates: Partial<SettingsFormState>,
  ) => void | boolean | Promise<void | boolean>;
  /** 已保存的 Codex 配置目录覆盖（不是输入框里的草稿）：变了就重读会话压缩状态 */
  codexConfigDir?: string;
}

export function CodexAuthSettings({
  settings,
  onChange,
  codexConfigDir,
}: CodexAuthSettingsProps) {
  const { t } = useTranslation();
  // 会话压缩开关以 config.toml 为准，不走设置表单的自动保存
  const [sessionCompression, setSessionCompression] = useState(false);
  const [sessionCompressionSaving, setSessionCompressionSaving] =
    useState(false);
  const [sessionsBytes, setSessionsBytes] = useState<number | null>(null);

  // 后端按已保存的目录读写 config.toml，同页保存新目录后要跟着重读
  useEffect(() => {
    let cancelled = false;
    setSessionsBytes(null);
    void settingsApi
      .getCodexSessionCompression()
      .catch(() => false)
      .then((enabled) => {
        if (!cancelled) setSessionCompression(enabled);
      });
    void settingsApi
      .getCodexSessionsDiskUsage()
      .catch(() => null)
      .then((bytes) => {
        if (!cancelled) setSessionsBytes(bytes);
      });
    return () => {
      cancelled = true;
    };
  }, [codexConfigDir]);

  const handleSessionCompressionChange = async (checked: boolean) => {
    setSessionCompressionSaving(true);
    try {
      setSessionCompression(
        await settingsApi.setCodexSessionCompression(checked),
      );
    } catch (error) {
      console.error("Failed to toggle codex session compression:", error);
      toast.error(t("settings.codexSessionCompressionFailed"));
    } finally {
      setSessionCompressionSaving(false);
    }
  };

  // 设置 → 应用配置 → Codex 卡片里的认证、额度与会话设置
  // 经典子 agent 开关在 Codex 聚合页签的「聚合设置」里。
  return (
    <>
      <CodexQuotaProtectionSettings settings={settings} onChange={onChange} />
      <SettingsSwitchRow
        label={t("settings.preserveCodexOfficialAuthOnSwitch")}
        help={{
          title: t("settings.preserveCodexOfficialAuthOnSwitch"),
          body: t("settings.preserveCodexOfficialAuthOnSwitchDescription"),
        }}
        checked={settings.preserveCodexOfficialAuthOnSwitch ?? false}
        onCheckedChange={(value) =>
          onChange({ preserveCodexOfficialAuthOnSwitch: value })
        }
      />

      <SettingsSwitchRow
        label={t("settings.unifyCodexSessionHistory")}
        help={{
          title: t("settings.unifyCodexSessionHistory"),
          body: t("settings.unifyCodexSessionHistoryDescription"),
        }}
        checked={settings.unifyCodexSessionHistory ?? false}
        onCheckedChange={(value) =>
          void onChange({ unifyCodexSessionHistory: value })
        }
      />

      <SettingsSwitchRow
        label={t("settings.codexSessionCompression")}
        help={{
          title: t("settings.codexSessionCompression"),
          body: t("settings.codexSessionCompressionDescription"),
        }}
        description={
          sessionsBytes === null
            ? undefined
            : t("settings.codexSessionCompressionUsage", {
                size: formatBytes(sessionsBytes),
              })
        }
        checked={sessionCompression}
        disabled={sessionCompressionSaving}
        onCheckedChange={(value) => void handleSessionCompressionChange(value)}
      />
    </>
  );
}
