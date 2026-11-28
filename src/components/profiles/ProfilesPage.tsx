import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Check, Pencil, Plus, Unlink, Upload } from "lucide-react";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { HoverTip } from "@/components/ui/hover-tip";
import { Notice } from "@/components/ui/notice";
import { SearchField } from "@/components/ui/search-field";
import { SegmentedControl } from "@/components/ui/segmented-control";
import { providersApi, settingsApi } from "@/lib/api";
import type { AppId } from "@/lib/api/types";
import {
  SCOPE_RESPONSE_KEY,
  type Profile,
  type ProfileScope,
  type ProfileTarget,
} from "@/lib/api/profiles";
import {
  useAddProfileProjectMutation,
  useApplyProfileMutation,
  useProfilesQuery,
  useRemoveProfileProjectMutation,
} from "@/lib/query/profiles";
import { toast } from "@/lib/toast";
import { extractErrorMessage } from "@/utils/errorUtils";
import { ProfileActionsMenu } from "./ProfileActionsMenu";
import { ProfileCard, type ProfileUseStatus } from "./ProfileCard";
import { ProfileCreateDialog } from "./ProfileCreateDialog";
import { ProfileEditPanel } from "./ProfileEditPanel";
import { ProjectPicker } from "./ProjectPicker";
import {
  ProjectProfileConflictDialog,
  useProjectProfileDeployment,
} from "./ProjectProfileDeployment";
import { APP_PROFILE_SCOPE, hasScopeSnapshot } from "./scope";
import { isProjectScope, useProfileLocation } from "./useProfileLocation";

interface ProfilesPageProps {
  activeApp: AppId;
  /** Opened from Agent management: only project deployment, no global application. */
  projectOnly?: boolean;
  initialProjectPath?: string;
  /** The Agent is not verified as runnable yet; profiles can still be prepared. */
  deploymentDisabled?: boolean;
}

/**
 * Global and project profiles are independent records. Project profiles can be
 * reused across registered directories, each with its own deployment status.
 */
export function ProfilesPage({ activeApp, ...options }: ProfilesPageProps) {
  const scope = APP_PROFILE_SCOPE[activeApp];
  if (!scope || (options.projectOnly && !isProjectScope(scope))) return null;
  return <ProfilesPageContent scope={scope} {...options} />;
}

function ProfilesPageContent({
  scope,
  projectOnly = false,
  initialProjectPath,
  deploymentDisabled = false,
}: Omit<ProfilesPageProps, "activeApp"> & { scope: ProfileScope }) {
  const { t } = useTranslation();
  const [search, setSearch] = useState("");
  const [creating, setCreating] = useState<{
    scope: ProfileScope;
    target: ProfileTarget;
    projectPath?: string;
  } | null>(null);
  // The editor keeps the Agent it was opened for, even if the page moves on.
  const [editing, setEditing] = useState<{
    profile: Profile;
    scope: ProfileScope;
  } | null>(null);
  const [removingProject, setRemovingProject] = useState<string | null>(null);

  const { data, isPending, isError, refetch } = useProfilesQuery();
  const { data: providers } = useQuery({
    queryKey: ["providers", scope, "profile-editor"],
    queryFn: () => providersApi.getAll(scope),
    networkMode: "always",
  });
  const applyMutation = useApplyProfileMutation();
  const addProjectMutation = useAddProfileProjectMutation();
  const removeProjectMutation = useRemoveProfileProjectMutation();
  const deployment = useProjectProfileDeployment(scope, !deploymentDisabled);
  const { location, setMode, selectProject } = useProfileLocation(
    scope,
    data?.projects,
    { projectOnly, initialProjectPath },
  );

  useEffect(() => setSearch(""), [scope, location.mode]);

  const providerName = (id: string | null | undefined) =>
    id ? (providers?.[id]?.name ?? id) : null;
  const profiles = (data?.profiles ?? []).filter(
    (profile) =>
      profile.target === location.mode && hasScopeSnapshot(profile, scope),
  );
  const keyword = profiles.length > 5 ? search.trim().toLocaleLowerCase() : "";
  const visibleProfiles = profiles.filter((profile) =>
    [
      profile.name,
      profile.payload.models?.[scope],
      providerName(profile.payload.providers[scope]),
    ].some((value) => value?.toLocaleLowerCase().includes(keyword)),
  );

  const project = location.project;
  const globalUse = data?.current[SCOPE_RESPONSE_KEY[scope]] ?? null;
  const projectUse = isProjectScope(scope)
    ? project?.deployed[scope]
    : undefined;
  const use =
    location.mode === "global"
      ? globalUse
      : projectUse && { id: projectUse.profileId, stale: projectUse.stale };
  const usedProfile = use && profiles.find((profile) => profile.id === use.id);
  const globalState = data?.globalState?.[SCOPE_RESPONSE_KEY[scope]];
  const statusOf = (profile: Profile): ProfileUseStatus =>
    use?.id !== profile.id ? null : use.stale ? "stale" : "active";

  const addProject = async () => {
    let picked: string | null;
    try {
      picked = await settingsApi.pickDirectory();
    } catch (error) {
      toast.error(
        t("profiles.projectPath.pickFailed", {
          detail: extractErrorMessage(error) || t("common.unknown"),
        }),
      );
      return;
    }
    if (picked === null) return;
    const path = picked;
    addProjectMutation.mutate(path, { onSuccess: () => selectProject(path) });
  };

  const primaryAction = (profile: Profile) => {
    const status = statusOf(profile);
    // The card that is behind its location is the one that needs attention.
    const variant = status === "stale" ? "default" : "outline";
    if (location.mode === "project") {
      return (
        <Button
          type="button"
          size="sm"
          variant={variant}
          disabled={
            !project?.available || deploymentDisabled || deployment.isPending
          }
          onClick={() =>
            project && void deployment.deployToProject(profile.id, project.path)
          }
        >
          <Upload className="h-3.5 w-3.5" />
          {t(
            status === "active"
              ? "profiles.projectDeploy.again"
              : "profiles.projectDeploy.button",
          )}
        </Button>
      );
    }
    return (
      <Button
        type="button"
        size="sm"
        variant={variant}
        disabled={applyMutation.isPending}
        onClick={() => applyMutation.mutate({ id: profile.id, scope })}
      >
        <Check className="h-3.5 w-3.5" />
        {t(status === "active" ? "profiles.applyAgain" : "profiles.apply")}
      </Button>
    );
  };

  return (
    <>
      <section
        aria-label={t("profiles.context.title")}
        data-profile-context
        className={
          projectOnly ? "min-w-0 space-y-4" : "min-w-0 space-y-4 px-6 pb-8 pt-4"
        }
      >
        <div className="flex min-w-0 flex-wrap items-center gap-3">
          {!projectOnly && isProjectScope(scope) && (
            <SegmentedControl
              aria-label={t("profiles.scope.label")}
              value={location.mode}
              onValueChange={setMode}
              items={[
                { value: "global", label: t("profiles.scope.global") },
                { value: "project", label: t("profiles.scope.project") },
              ]}
            />
          )}
          {location.mode === "project" && (
            <ProjectPicker
              projects={data?.projects ?? []}
              selected={project}
              onSelect={selectProject}
              onAdd={() => void addProject()}
              adding={addProjectMutation.isPending}
            />
          )}
          <Button
            type="button"
            className="ms-auto h-9"
            onClick={() =>
              setCreating({
                scope,
                target: location.mode,
                projectPath: project?.path,
              })
            }
          >
            <Plus className="h-4 w-4" />
            {t("profiles.createFromCurrent")}
          </Button>
        </div>

        {location.mode === "global" ? (
          <div
            role="status"
            aria-label={t("profiles.inEffect.global")}
            className="flex min-w-0 flex-wrap items-baseline gap-x-5 gap-y-1 rounded-panel bg-subtle px-4 py-3 text-sm"
          >
            <span className="font-medium text-fg-1">
              {t("profiles.inEffect.global")}
            </span>
            {globalState && (
              <span className="min-w-0 break-all text-fg-2">
                {t("profiles.editor.provider")}:{" "}
                <span className="text-fg-1">
                  {providerName(globalState.providerId) ??
                    t("profiles.inEffect.none")}
                </span>
              </span>
            )}
            {globalState && scope !== "claude-desktop" && (
              <span className="min-w-0 break-all text-fg-2">
                {t("profiles.editor.model")}:{" "}
                <span className="text-fg-1">
                  {globalState.model || t("profiles.defaultModel")}
                </span>
              </span>
            )}
            <span className="min-w-0 break-words text-fg-2">
              {t("profiles.title")}:{" "}
              <span className="text-fg-1">
                {usedProfile?.name ?? t("profiles.inEffect.noProfile")}
              </span>
              {usedProfile && globalUse?.stale && (
                <span className="text-warning-text">
                  {" "}
                  · {t("profiles.pendingApply")}
                </span>
              )}
            </span>
          </div>
        ) : project ? (
          <div
            role="status"
            aria-label={t("profiles.inEffect.project")}
            className="flex min-w-0 flex-wrap items-center gap-x-5 gap-y-1 rounded-panel bg-subtle px-4 py-3 text-sm"
          >
            <span
              className="min-w-0 flex-[1_1_14rem] truncate font-mono text-xs text-fg-2"
              title={project.path}
            >
              {project.path}
            </span>
            <span className="min-w-0 break-words text-fg-2">
              {t("profiles.inEffect.project")}:{" "}
              <span className="text-fg-1">
                {usedProfile?.name ??
                  t(
                    projectUse
                      ? "profiles.inEffect.deletedProfile"
                      : "profiles.inEffect.notDeployed",
                  )}
              </span>
              {usedProfile && projectUse?.stale && (
                <span className="text-warning-text">
                  {" "}
                  · {t("profiles.pendingDeploy")}
                </span>
              )}
            </span>
            <HoverTip content={t("profiles.projects.remove")}>
              <Button
                type="button"
                variant="quiet"
                size="icon-compact"
                className="h-8 w-8"
                aria-label={t("profiles.projects.remove")}
                onClick={() => setRemovingProject(project.path)}
              >
                <Unlink className="h-4 w-4" />
              </Button>
            </HoverTip>
          </div>
        ) : (
          data && (
            <div className="rounded-panel border border-dashed border-border px-5 py-8 text-center text-sm text-fg-2">
              {t("profiles.projects.empty")}
            </div>
          )
        )}
        {location.mode === "project" && project && !project.available && (
          <Notice tone="warning" title={t("profiles.projects.unavailable")} />
        )}

        {profiles.length > 5 && (
          <SearchField
            containerClassName="max-w-md"
            value={search}
            onValueChange={setSearch}
            clearLabel={t("common.clear")}
            aria-label={t("profiles.searchPlaceholder")}
            placeholder={t("profiles.searchPlaceholder")}
          />
        )}
        {!data && (isPending || isError) ? (
          <div
            role="status"
            className="flex items-center gap-3 text-sm text-fg-2"
          >
            {t(isError ? "profiles.loadFailed" : "common.loading")}
            {isError && (
              <Button
                size="sm"
                variant="outline"
                onClick={() => void refetch()}
              >
                {t("common.retry")}
              </Button>
            )}
          </div>
        ) : visibleProfiles.length ? (
          <ul aria-label={t("profiles.title")} className="grid gap-3">
            {visibleProfiles.map((profile) => (
              <ProfileCard
                key={profile.id}
                profile={profile}
                scope={scope}
                status={statusOf(profile)}
                statusLabels={
                  location.mode === "global"
                    ? {
                        active: t("profiles.applied"),
                        stale: t("profiles.pendingApply"),
                      }
                    : {
                        active: t("profiles.projectDeploy.current"),
                        stale: t("profiles.pendingDeploy"),
                      }
                }
                providerName={
                  providerName(profile.payload.providers[scope]) ??
                  t("profiles.editor.inheritProvider")
                }
              >
                {(location.mode === "global" || project) &&
                  primaryAction(profile)}
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => setEditing({ profile, scope })}
                >
                  <Pencil className="h-3.5 w-3.5" />
                  {t("common.edit")}
                </Button>
                <ProfileActionsMenu
                  profile={profile}
                  scope={scope}
                  allowTargetCopy={!projectOnly}
                  onCopied={(copied) => {
                    setMode(copied.target);
                    setSearch("");
                    setEditing({ profile: copied, scope });
                  }}
                />
              </ProfileCard>
            ))}
          </ul>
        ) : (
          <div className="rounded-panel border border-dashed border-border px-5 py-8 text-center text-sm text-fg-2">
            {t(keyword ? "profiles.noMatches" : "profiles.empty")}
          </div>
        )}
      </section>

      {creating && (
        <ProfileCreateDialog
          scope={creating.scope}
          target={creating.target}
          projectPath={creating.projectPath}
          onClose={() => setCreating(null)}
          onCreated={(created) => {
            const createdScope = creating.scope;
            setCreating(null);
            setSearch("");
            setEditing({ profile: created, scope: createdScope });
          }}
        />
      )}
      {removingProject && (
        <ConfirmDialog
          isOpen
          title={t("profiles.projects.remove")}
          message={t("profiles.projects.removeConfirm", {
            path: removingProject,
          })}
          variant="info"
          pending={removeProjectMutation.isPending}
          onConfirm={() =>
            removeProjectMutation.mutate(removingProject, {
              onSettled: () => setRemovingProject(null),
            })
          }
          onCancel={() => setRemovingProject(null)}
        />
      )}
      <ProjectProfileConflictDialog
        deployment={deployment}
        deploymentDisabled={deploymentDisabled}
      />
      {editing && (
        <ProfileEditPanel
          profile={editing.profile}
          scope={editing.scope}
          onClose={() => setEditing(null)}
        />
      )}
    </>
  );
}
