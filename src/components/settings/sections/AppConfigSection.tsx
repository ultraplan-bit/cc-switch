import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import type { AppId } from "@/lib/api";
import { APP_IDS } from "@/config/appSpecs";
import type {
  ResolvedDirectories,
  SettingsFormState,
} from "@/hooks/useSettings";
import type { Settings } from "@/types";
import { Button } from "@/components/ui/button";
import { DirectoryInput } from "@/components/settings/DirectorySettings";
import {
  DIRECTORY_APP,
  DIRECTORY_APPS,
  type DirectoryAppId,
  sanitizeDir,
} from "@/config/directoryApps";
import { CodexAuthSettings } from "@/components/settings/CodexAuthSettings";
import { APP_DISPLAY_NAME, AppGlyph } from "@/components/shell/AppGlyph";
import {
  SettingsBlock,
  SettingsCard,
  SettingsRow,
  SettingsSwitchRow,
} from "@/components/settings/SettingsLayout";

interface AppConfigSectionProps {
  app: AppId;
  onAppChange: (app: AppId) => void;
  settings: SettingsFormState;
  savedSettings?: Settings;
  resolvedDirs: ResolvedDirectories;
  isSaving: boolean;
  pendingDataDirectory: boolean;
  onAutoSave: (updates: Partial<SettingsFormState>) => Promise<boolean>;
  onDirectoryChange: (app: DirectoryAppId, value?: string) => void;
  onBrowseDirectory: (app: DirectoryAppId) => Promise<void>;
  onResetDirectory: (app: DirectoryAppId) => Promise<void>;
  onSaveDirectories: () => Promise<void>;
}

/**
 * 只挂载所选 Agent 的设置。目录草稿留在父级，切换 Agent 不丢失。
 * 保存会提交所有目录草稿，因此待保存名单和按钮始终显示在选择器下方。
 */
export function AppConfigSection({
  app,
  onAppChange,
  settings,
  savedSettings,
  resolvedDirs,
  isSaving,
  pendingDataDirectory,
  onAutoSave,
  onDirectoryChange,
  onBrowseDirectory,
  onResetDirectory,
  onSaveDirectories,
}: AppConfigSectionProps) {
  const { t } = useTranslation();

  const pendingDirectories = DIRECTORY_APPS.filter(
    ({ field }) =>
      sanitizeDir(settings[field]) !== sanitizeDir(savedSettings?.[field]),
  ).map(({ app: pendingApp }) => APP_DISPLAY_NAME[pendingApp]);
  if (pendingDataDirectory) pendingDirectories.push("CC Switch");
  const directoryApp = DIRECTORY_APPS.find((entry) => entry.app === app)?.app;

  const directoryRow = (app: DirectoryAppId) => (
    <SettingsRow
      label={t("settings.appConfig.configDir")}
      help={{
        title: t("settings.appConfig.configDir"),
        body: t("settings.configDirectoryDescription"),
      }}
    >
      <DirectoryInput
        label=""
        ariaLabel={`${APP_DISPLAY_NAME[app]} ${t("settings.appConfig.configDir")}`}
        value={settings[DIRECTORY_APP[app].field]}
        resolvedValue={resolvedDirs[app]}
        placeholder={t(DIRECTORY_APP[app].placeholderKey)}
        onChange={(value) => onDirectoryChange(app, value)}
        onBrowse={() => onBrowseDirectory(app)}
        onReset={() => onResetDirectory(app)}
      />
    </SettingsRow>
  );

  const blockTitle = (
    <span className="flex items-center gap-2">
      <AppGlyph app={app} size={16} badgeClassName="bg-app" />
      {APP_DISPLAY_NAME[app]}
    </span>
  );

  return (
    <>
      <div className="space-y-3">
        <label className="flex flex-wrap items-center gap-3 text-body font-medium text-fg-1">
          {t("settings.appConfig.selectAgent")}
          <select
            value={app}
            onChange={(event) => onAppChange(event.target.value as AppId)}
            className="h-10 min-w-0 max-w-full rounded-control border border-border bg-surface px-3 text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            {APP_IDS.map((id) => (
              <option key={id} value={id}>
                {APP_DISPLAY_NAME[id]}
              </option>
            ))}
          </select>
        </label>
        <p className="text-body text-fg-2">
          {t("settings.appConfig.selectionHint")}
        </p>
        {pendingDirectories.length > 0 && (
          <div className="flex flex-wrap items-center gap-3 rounded-control border border-border bg-subtle p-3">
            <p role="status" className="min-w-0 flex-1 text-body text-fg-2">
              {t("settings.appConfig.pendingDirectories", {
                apps: pendingDirectories.join(", "),
              })}
            </p>
            <Button
              variant="solid"
              size="compact"
              disabled={isSaving}
              onClick={() => void onSaveDirectories()}
            >
              {isSaving && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
              {t("settings.appConfig.saveDirectories")}
            </Button>
          </div>
        )}
      </div>
      <SettingsBlock
        key={app}
        id={`app-config-${app}`}
        className="rounded-panel"
        title={blockTitle}
      >
        <SettingsCard>
          {app === "claude" && (
            <>
              <SettingsSwitchRow
                label={t("settings.enableClaudePluginIntegration")}
                help={{
                  title: t("settings.enableClaudePluginIntegration"),
                  body: t("settings.enableClaudePluginIntegrationDescription"),
                }}
                checked={!!settings.enableClaudePluginIntegration}
                onCheckedChange={(value) =>
                  void onAutoSave({ enableClaudePluginIntegration: value })
                }
              />
              <SettingsSwitchRow
                label={t("settings.skipClaudeOnboarding")}
                help={{
                  title: t("settings.skipClaudeOnboarding"),
                  body: t("settings.skipClaudeOnboardingDescription"),
                }}
                checked={!!settings.skipClaudeOnboarding}
                onCheckedChange={(value) =>
                  void onAutoSave({ skipClaudeOnboarding: value })
                }
              />
            </>
          )}
          {app === "codex" && (
            <CodexAuthSettings
              settings={settings}
              onChange={onAutoSave}
              codexConfigDir={savedSettings?.codexConfigDir}
            />
          )}
          {directoryApp ? (
            directoryRow(directoryApp)
          ) : (
            <p className="px-4 py-5 text-body text-fg-2">
              {t("settings.appConfig.empty")}
            </p>
          )}
        </SettingsCard>
      </SettingsBlock>
    </>
  );
}
