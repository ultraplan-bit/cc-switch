import { useId, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { CircleAlert, CircleCheck, Cpu, Layers, Plug } from "lucide-react";
import type { Profile, ProfileScope } from "@/lib/api/profiles";
import { followsGlobalResources } from "./scope";

/** How the profile relates to the location being viewed (global, or one project). */
export type ProfileUseStatus = "active" | "stale" | null;

interface ProfileCardProps {
  profile: Profile;
  scope: ProfileScope;
  providerName: string;
  status: ProfileUseStatus;
  /** Status wording differs between the global location and a project. */
  statusLabels: { active: string; stale: string };
  children: ReactNode;
}

export function ProfileCard({
  profile,
  scope,
  providerName,
  status,
  statusLabels,
  children,
}: ProfileCardProps) {
  const { t } = useTranslation();
  const nameId = useId();
  const desktop = scope === "claude-desktop";
  const project = profile.target === "project";
  const projectDirect =
    project &&
    scope === "claude" &&
    profile.payload.claudeProjectConnection === "direct";
  const model = profile.payload.models?.[scope];
  const mcpCount = profile.payload.mcp[scope]?.length ?? 0;
  const skillCount = profile.payload.skills[scope]?.length ?? 0;
  return (
    <li
      aria-labelledby={nameId}
      data-profile-status={status ?? "none"}
      className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-3 rounded-panel border border-border bg-surface px-4 py-3.5"
    >
      <span
        aria-hidden="true"
        className="grid h-10 w-10 shrink-0 place-items-center rounded-control bg-subtle text-fg-2"
      >
        <Layers size={20} />
      </span>
      <div className="min-w-0 flex-[1_1_16rem] space-y-1.5">
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          <span
            id={nameId}
            className="min-w-0 break-words text-base font-semibold leading-tight text-fg-1"
          >
            {profile.name}
          </span>
          {status === "active" && (
            <span className="inline-flex items-center gap-1 rounded-full bg-success-soft px-2 py-0.5 text-caption font-medium text-success-text">
              <CircleCheck aria-hidden="true" size={14} />
              {statusLabels.active}
            </span>
          )}
          {status === "stale" && (
            <span className="inline-flex items-center gap-1 rounded-full bg-warning-soft px-2 py-0.5 text-caption font-medium text-warning-text">
              <CircleAlert aria-hidden="true" size={14} />
              {statusLabels.stale}
            </span>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-caption text-fg-2">
          <span className="inline-flex min-w-0 items-center gap-1">
            <Plug aria-hidden="true" size={13} className="shrink-0" />
            <span className="break-all">
              {project && !projectDirect
                ? t("profiles.editor.projectConnectionInherit")
                : providerName}
            </span>
          </span>
          {projectDirect && (
            <span>{t("profiles.editor.projectConnectionDirect")}</span>
          )}
          {project && !projectDirect && profile.payload.providers[scope] && (
            <span className="break-all">
              {t("profiles.editor.projectModelProvider")}: {providerName}
            </span>
          )}
          {!desktop && (
            <span className="inline-flex min-w-0 items-center gap-1">
              <Cpu aria-hidden="true" size={13} className="shrink-0" />
              <span className="break-all">
                {model || t("profiles.defaultModel")}
              </span>
            </span>
          )}
          {!desktop &&
            (followsGlobalResources(profile, scope) ? (
              <span>{t("profiles.followsResources")}</span>
            ) : (
              <>
                {mcpCount > 0 && (
                  <span>
                    {t("profiles.editor.mcp")} · {mcpCount}
                  </span>
                )}
                {skillCount > 0 && (
                  <span>
                    {t("profiles.editor.skills")} · {skillCount}
                  </span>
                )}
              </>
            ))}
        </div>
      </div>
      <div className="ms-auto flex shrink-0 flex-wrap items-center gap-2">
        {children}
      </div>
    </li>
  );
}
