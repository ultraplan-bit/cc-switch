import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Check,
  ChevronsUpDown,
  FolderCog,
  FolderOpen,
  MoreHorizontal,
  Pencil,
  Plus,
  Upload,
  X,
} from "lucide-react";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { FullScreenPanel } from "@/components/common/FullScreenPanel";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { HoverTip } from "@/components/ui/hover-tip";
import { Input } from "@/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";
import { settingsApi } from "@/lib/api";
import type { AppId } from "@/lib/api/types";
import {
  useApplyProfileMutation,
  useClearProfileMutation,
  useCreateProfileMutation,
  useImportProjectProfileMutation,
  useProfilesQuery,
  useProfileConfiguration,
} from "@/lib/query/profiles";
import { ProfileManageDialog } from "./ProfileManageDialog";
import { ProfileEditPanel } from "./ProfileEditPanel";
import {
  ProjectProfileConflictDialog,
  useProjectProfileDeployment,
} from "./ProjectProfileDeployment";
import { APP_PROFILE_SCOPE, hasScopeSnapshot } from "./scope";
import type {
  CurrentProfileIds,
  Profile,
  ProfileScope,
} from "@/lib/api/profiles";
import { toast } from "@/lib/toast";
import { extractErrorMessage } from "@/utils/errorUtils";

const CURRENT_ID_KEY: Record<ProfileScope, keyof CurrentProfileIds> = {
  claude: "claude",
  "claude-desktop": "claudeDesktop",
  codex: "codex",
};

const PROJECT_SELECTION_KEY = "cc-switch:profiles:project-selection:v1";

function projectName(path: string) {
  return (
    path
      .replace(/[\\/]+$/, "")
      .split(/[\\/]/)
      .at(-1) || path
  );
}
type ProjectSelection = Partial<
  Record<ProfileScope, { path: string; profileId?: string }>
>;

function readProjectSelection(): ProjectSelection {
  try {
    const saved: unknown = JSON.parse(
      localStorage.getItem(PROJECT_SELECTION_KEY) ?? "{}",
    );
    if (!saved || typeof saved !== "object") return {};
    const result: ProjectSelection = {};
    for (const scope of ["claude", "codex"] as const) {
      const item = (saved as Record<string, unknown>)[scope];
      if (
        !item ||
        typeof item !== "object" ||
        !("path" in item) ||
        typeof item.path !== "string"
      )
        continue;
      result[scope] = {
        path: item.path,
        profileId:
          "profileId" in item && typeof item.profileId === "string"
            ? item.profileId
            : undefined,
      };
    }
    return result;
  } catch {
    return {};
  }
}

interface ProfileSwitcherProps {
  activeApp: AppId;
  projectOnly?: boolean;
  initialProjectPath?: string;
  initialProfileId?: string;
  deploymentDisabled?: boolean;
}

/**
 * Profile 上下文区：与全局供应商页头分开，项目操作只作用于所选目录。
 *
 * 全局 Profile 切换按应用分组进行；项目 Profile 从管理对话框部署到
 * 绑定目录。列表全应用共享，与侧栏切换查看应用的语义不同。
 */
export function ProfileSwitcher({
  activeApp,
  projectOnly = false,
  initialProjectPath,
  initialProfileId,
  deploymentDisabled = false,
}: ProfileSwitcherProps) {
  const { t } = useTranslation();
  const projectSelectId = useId();
  const sourceSelectId = useId();
  const [open, setOpen] = useState(false);
  const [projectPickerOpen, setProjectPickerOpen] = useState(false);
  const [globalSelection, setGlobalSelection] = useState<
    Partial<Record<ProfileScope, string | null>>
  >({});
  const [isCreateOpen, setIsCreateOpen] = useState(false);
  const [isManageOpen, setIsManageOpen] = useState(false);
  const [newName, setNewName] = useState("");
  const [projectPath, setProjectPath] = useState<string | null>(null);
  const [projectScoped, setProjectScoped] = useState(projectOnly);
  const [createSource, setCreateSource] = useState<"current" | "project">(
    "current",
  );
  const [confirmDiscardCreate, setConfirmDiscardCreate] = useState(false);
  const createDraft = JSON.stringify({
    newName,
    projectPath,
    projectScoped,
    createSource,
  });
  const initialCreateDraft = useRef(createDraft);
  const [projectSelection, setProjectSelection] = useState<ProjectSelection>(
    () => {
      const saved = readProjectSelection();
      const initialScope = APP_PROFILE_SCOPE[activeApp];
      return initialScope && initialProjectPath !== undefined
        ? {
            ...saved,
            [initialScope]: {
              path: initialProjectPath,
              profileId: initialProfileId,
            },
          }
        : saved;
    },
  );
  const persistedSelection = useRef<ProjectSelection>({});
  const [editingProfile, setEditingProfile] = useState<{
    profile: Profile;
    scope: ProfileScope;
    returnToManage?: boolean;
  } | null>(null);
  const creating = useRef(false);

  const configuration = useProfileConfiguration(APP_PROFILE_SCOPE[activeApp]);
  const { data } = useProfilesQuery();
  const applyMutation = useApplyProfileMutation();
  const clearMutation = useClearProfileMutation();
  const createMutation = useCreateProfileMutation();
  const importMutation = useImportProjectProfileMutation();
  const createPending = createMutation.isPending || importMutation.isPending;

  useEffect(() => {
    if (!data) return;
    setGlobalSelection((previous) => {
      const next = { ...previous };
      let changed = false;
      for (const targetScope of Object.keys(CURRENT_ID_KEY) as ProfileScope[]) {
        const selected = previous[targetScope];
        const current = data.currentIds?.[CURRENT_ID_KEY[targetScope]];
        if (selected === undefined && current) {
          next[targetScope] = current;
          changed = true;
        } else if (
          selected &&
          !data.profiles.some(
            (profile) => profile.id === selected && !profile.projectPath,
          )
        ) {
          next[targetScope] = null;
          changed = true;
        }
      }
      return changed ? next : previous;
    });
  }, [data]);

  useEffect(() => {
    if (!data) return;
    const valid: ProjectSelection = {};
    for (const targetScope of ["claude", "codex"] as const) {
      const selection = projectSelection[targetScope];
      if (!selection) continue;
      const matching = data.profiles.filter(
        (profile) => profile.projectPath === selection.path,
      );
      valid[targetScope] = {
        path: matching.length ? selection.path : "",
        profileId: matching.some(
          (profile) => profile.id === selection.profileId,
        )
          ? selection.profileId
          : undefined,
      };
    }
    if (JSON.stringify(valid) !== JSON.stringify(projectSelection)) {
      setProjectSelection(valid);
      return;
    }
    const stored = readProjectSelection();
    let changed = false;
    for (const targetScope of ["claude", "codex"] as const) {
      if (
        JSON.stringify(persistedSelection.current[targetScope]) ===
        JSON.stringify(valid[targetScope])
      )
        continue;
      stored[targetScope] = valid[targetScope];
      changed = true;
    }
    if (changed) {
      try {
        localStorage.setItem(PROJECT_SELECTION_KEY, JSON.stringify(stored));
      } catch {
        // Remembering a viewing preference must not block project configuration.
      }
    }
    persistedSelection.current = valid;
  }, [data, projectSelection]);

  // Profile 仅作用于受支持的应用——在其他应用的标签页展示会误导用户
  // 以为当前应用也被切换了，因此只在有所属分组的应用页面渲染
  const scope = APP_PROFILE_SCOPE[activeApp];
  const deployment = useProjectProfileDeployment(
    scope ?? "claude",
    !deploymentDisabled,
  );
  if (!scope || (projectOnly && scope === "claude-desktop")) {
    return null;
  }

  const allProfiles = data?.profiles ?? [];
  const projectPaths = [
    ...new Set(
      allProfiles
        .map((profile) => profile.projectPath)
        .filter((path): path is string => Boolean(path)),
    ),
  ];
  const selectedPath = projectSelection[scope]?.path ?? "";
  const selectedProjectPath =
    scope !== "claude-desktop" && projectPaths.includes(selectedPath)
      ? selectedPath
      : "";
  const profiles = allProfiles.filter((profile) =>
    selectedProjectPath
      ? profile.projectPath === selectedProjectPath
      : !projectOnly && !profile.projectPath,
  );
  const currentId = data?.currentIds?.[CURRENT_ID_KEY[scope]] ?? null;
  const currentProfile = selectedProjectPath
    ? profiles.find((profile) => profile.deployedScopes?.includes(scope))
    : profiles.find((profile) => profile.id === currentId);
  const selectedProfile = selectedProjectPath
    ? (profiles.find(
        (profile) => profile.id === projectSelection[scope]?.profileId,
      ) ?? currentProfile)
    : globalSelection[scope] === null
      ? undefined
      : (profiles.find((profile) => profile.id === globalSelection[scope]) ??
        currentProfile);

  const handleApply = (id: string) => {
    setOpen(false);
    if (selectedProjectPath) {
      setProjectSelection((current) => ({
        ...current,
        [scope]: { path: selectedProjectPath, profileId: id },
      }));
    } else if (!projectOnly) {
      setGlobalSelection((current) => ({ ...current, [scope]: id }));
      applyMutation.mutate({ id, scope });
    }
  };

  const closeCreatePanel = () => {
    if (creating.current) return;
    setConfirmDiscardCreate(false);
    setIsCreateOpen(false);
    setNewName("");
    setProjectPath(null);
    setProjectScoped(projectOnly);
    setCreateSource("current");
  };

  const requestCloseCreatePanel = () => {
    if (creating.current || confirmDiscardCreate) return;
    if (createDraft !== initialCreateDraft.current)
      setConfirmDiscardCreate(true);
    else closeCreatePanel();
  };

  const rememberProjectSelection = (
    profile: Profile,
    targetScope: ProfileScope,
  ) => {
    const path = profile.projectPath;
    if (path && targetScope !== "claude-desktop") {
      setProjectSelection((current) => ({
        ...current,
        [targetScope]: { path, profileId: profile.id },
      }));
    } else if (!path) {
      setGlobalSelection((current) => ({
        ...current,
        [targetScope]: profile.id,
      }));
    }
  };

  const handleCreate = () => {
    const name = newName.trim();
    const isProject =
      scope !== "claude-desktop" && (projectOnly || projectScoped);
    if (!name || creating.current || (isProject && !projectPath)) return;
    creating.current = true;
    const openCreatedProfile = (created: Profile) => {
      creating.current = false;
      rememberProjectSelection(created, scope);
      setEditingProfile({ profile: created, scope });
      closeCreatePanel();
    };
    if (isProject && createSource === "project" && projectPath) {
      importMutation.mutate(
        { name, scope, projectPath },
        {
          onSuccess: ({ profile }) => openCreatedProfile(profile),
          onSettled: () => {
            creating.current = false;
          },
        },
      );
      return;
    }
    createMutation.mutate(
      { name, scope, projectPath: isProject ? projectPath : null },
      {
        onSuccess: openCreatedProfile,
        onSettled: () => {
          creating.current = false;
        },
      },
    );
  };

  const pickProjectPath = async () => {
    try {
      const pickedPath = await settingsApi.pickDirectory();
      if (pickedPath !== null) setProjectPath(pickedPath);
    } catch (error) {
      toast.error(
        t("profiles.projectPath.pickFailed", {
          detail: extractErrorMessage(error) || t("common.unknown"),
        }),
      );
    }
  };

  const handleEditProfile = (profile: Profile, targetScope: ProfileScope) => {
    rememberProjectSelection(profile, targetScope);
    setEditingProfile({ profile, scope: targetScope, returnToManage: true });
    setIsManageOpen(false);
  };

  const closeProfileEditor = () => {
    const returnToManage = editingProfile?.returnToManage;
    setEditingProfile(null);
    if (returnToManage) setIsManageOpen(true);
  };

  return (
    <>
      <section
        aria-label={t("profiles.context.title")}
        data-profile-context
        className={cn(
          "min-w-0 space-y-2",
          !projectOnly &&
            "shrink-0 border-b border-border bg-surface px-6 py-3",
        )}
      >
        <div className="flex min-w-0 flex-wrap items-end gap-x-4 gap-y-3">
          <div className="flex min-w-0 flex-[1_1_22rem] flex-wrap items-end gap-3">
            {scope !== "claude-desktop" && (
              <div className="min-w-0 flex-[1_1_12rem] space-y-1">
                <span id={projectSelectId} className="text-caption text-fg-3">
                  {t("profiles.projectContext")}
                </span>
                <Popover
                  open={projectPickerOpen}
                  onOpenChange={setProjectPickerOpen}
                >
                  <PopoverTrigger asChild>
                    <Button
                      type="button"
                      variant="outline"
                      role="combobox"
                      aria-expanded={projectPickerOpen}
                      aria-labelledby={projectSelectId}
                      title={
                        selectedProjectPath ||
                        t(
                          projectOnly
                            ? "profiles.environment.selectProject"
                            : "profiles.scope.global",
                        )
                      }
                      data-project-path={selectedProjectPath}
                      className="h-9 w-full min-w-0 justify-between gap-2 px-3"
                    >
                      <FolderOpen className="h-4 w-4 shrink-0 text-fg-3" />
                      <span className="min-w-0 flex-1 truncate text-start">
                        {selectedProjectPath
                          ? projectName(selectedProjectPath)
                          : t(
                              projectOnly
                                ? "profiles.environment.selectProject"
                                : "profiles.scope.global",
                            )}
                      </span>
                      <ChevronsUpDown className="h-3.5 w-3.5 shrink-0 text-fg-3" />
                    </Button>
                  </PopoverTrigger>
                  <PopoverContent
                    align="start"
                    className="z-[100] w-[min(26rem,calc(100vw-3rem))] p-0"
                  >
                    <Command label={t("profiles.context.searchProjects")}>
                      <CommandInput
                        placeholder={t("profiles.context.searchProjects")}
                      />
                      <CommandList>
                        <CommandEmpty>{t("profiles.empty")}</CommandEmpty>
                        <CommandGroup>
                          <CommandItem
                            value="__global__"
                            onSelect={() => {
                              setProjectSelection((current) => ({
                                ...current,
                                [scope]: { path: "" },
                              }));
                              setProjectPickerOpen(false);
                            }}
                          >
                            <Check
                              className={cn(
                                "mr-2 h-4 w-4 shrink-0",
                                !selectedProjectPath
                                  ? "opacity-100"
                                  : "opacity-0",
                              )}
                            />
                            {t(
                              projectOnly
                                ? "profiles.environment.selectProject"
                                : "profiles.scope.global",
                            )}
                          </CommandItem>
                          {projectPaths.map((path) => (
                            <CommandItem
                              key={path}
                              value={path}
                              aria-label={path}
                              keywords={[projectName(path), path]}
                              onSelect={() => {
                                setProjectSelection((current) => ({
                                  ...current,
                                  [scope]: { path },
                                }));
                                setProjectPickerOpen(false);
                              }}
                            >
                              <Check
                                className={cn(
                                  "mr-2 h-4 w-4 shrink-0",
                                  selectedProjectPath === path
                                    ? "opacity-100"
                                    : "opacity-0",
                                )}
                              />
                              <span className="min-w-0 flex-1">
                                <span className="block truncate">
                                  {projectName(path)}
                                </span>
                                <span
                                  className="block truncate text-xs text-fg-3"
                                  title={path}
                                >
                                  {path}
                                </span>
                              </span>
                            </CommandItem>
                          ))}
                        </CommandGroup>
                      </CommandList>
                    </Command>
                  </PopoverContent>
                </Popover>
              </div>
            )}
            <div className="min-w-0 flex-[1_1_10rem] space-y-1">
              <span className="text-caption text-fg-3">Profile</span>
              <Popover open={open} onOpenChange={setOpen}>
                <PopoverTrigger asChild>
                  <button
                    type="button"
                    role="combobox"
                    aria-expanded={open}
                    aria-label={t("profiles.switcherAriaLabel", {
                      name:
                        selectedProfile?.name ??
                        (selectedProjectPath
                          ? t("profiles.selectProfile")
                          : t("profiles.none")),
                    })}
                    title={t(`profiles.switcherTooltip.${scope}`)}
                    className={cn(
                      "inline-flex h-9 w-full min-w-0 items-center gap-2 rounded-control border border-border px-3 text-sm font-medium transition-colors",
                      "hover:bg-black/5 dark:hover:bg-white/5",
                      selectedProfile ? "text-fg-1" : "text-fg-2",
                    )}
                  >
                    <FolderOpen className="h-4 w-4 shrink-0 opacity-70" />
                    <span className="min-w-0 flex-1 truncate text-start">
                      {selectedProfile?.name ??
                        (selectedProjectPath
                          ? t("profiles.selectProfile")
                          : t("profiles.none"))}
                    </span>
                    {selectedProfile &&
                      selectedProfile.id !== currentProfile?.id && (
                        <span className="text-xs text-fg-2">
                          {t("profiles.pendingSelection")}
                        </span>
                      )}
                    <ChevronsUpDown className="h-3.5 w-3.5 shrink-0 opacity-50" />
                  </button>
                </PopoverTrigger>
                <PopoverContent
                  side="bottom"
                  align="start"
                  sideOffset={6}
                  aria-label={t(`profiles.switcherTooltip.${scope}`)}
                  className="z-[100] w-[min(22rem,calc(100vw-3rem))] p-0"
                >
                  <Command label={t("profiles.searchPlaceholder")}>
                    <CommandInput
                      placeholder={t("profiles.searchPlaceholder")}
                    />
                    <CommandList>
                      <CommandEmpty>{t("profiles.empty")}</CommandEmpty>
                      {profiles.length > 0 && (
                        <CommandGroup>
                          {profiles.map((profile) => (
                            <CommandItem
                              key={profile.id}
                              value={profile.id}
                              keywords={[profile.name]}
                              onSelect={() => handleApply(profile.id)}
                            >
                              <Check
                                className={cn(
                                  "mr-2 h-4 w-4 shrink-0",
                                  selectedProfile?.id === profile.id
                                    ? "opacity-100"
                                    : "opacity-0",
                                )}
                              />
                              <span className="flex min-w-0 flex-1 flex-col">
                                <span className="truncate">{profile.name}</span>
                                {profile.projectPath && (
                                  <span
                                    className="max-w-[10rem] truncate text-xs text-fg-2"
                                    title={profile.projectPath}
                                  >
                                    {profile.projectPath}
                                  </span>
                                )}
                              </span>
                              {selectedProjectPath &&
                                profile.deployedScopes?.includes(scope) && (
                                  <span className="ml-auto pl-2 text-xs text-fg-2">
                                    {t("profiles.projectDeploy.current")}
                                  </span>
                                )}
                              {!hasScopeSnapshot(profile, scope) && (
                                <span className="ml-auto shrink-0 pl-2 text-xs text-fg-2">
                                  {t("profiles.noSnapshotForScope")}
                                </span>
                              )}
                            </CommandItem>
                          ))}
                        </CommandGroup>
                      )}
                      <div className="mx-1 my-1 h-px bg-border" />
                      <CommandGroup>
                        <CommandItem
                          value="__create__"
                          keywords={[t("profiles.createFromCurrent")]}
                          onSelect={() => {
                            setOpen(false);
                            initialCreateDraft.current = JSON.stringify({
                              newName,
                              projectPath: selectedProjectPath || null,
                              projectScoped:
                                projectOnly || Boolean(selectedProjectPath),
                              createSource,
                            });
                            setProjectPath(selectedProjectPath || null);
                            setProjectScoped(
                              projectOnly || Boolean(selectedProjectPath),
                            );
                            setIsCreateOpen(true);
                          }}
                        >
                          <Plus className="mr-2 h-4 w-4 shrink-0" />
                          {t("profiles.createFromCurrent")}
                        </CommandItem>
                        {!projectOnly && !selectedProjectPath && currentId && (
                          <CommandItem
                            value="__clear__"
                            keywords={[t("profiles.none")]}
                            onSelect={() => {
                              setOpen(false);
                              setGlobalSelection((current) => ({
                                ...current,
                                [scope]: null,
                              }));
                              clearMutation.mutate(scope);
                            }}
                          >
                            <X className="mr-2 h-4 w-4 shrink-0" />
                            {t("profiles.none")}
                          </CommandItem>
                        )}
                        {allProfiles.some(
                          (profile) => !projectOnly || profile.projectPath,
                        ) && (
                          <CommandItem
                            value="__manage__"
                            keywords={[t("profiles.manage")]}
                            onSelect={() => {
                              setOpen(false);
                              setIsManageOpen(true);
                            }}
                          >
                            <FolderCog className="mr-2 h-4 w-4 shrink-0" />
                            {t("profiles.manage")}
                          </CommandItem>
                        )}
                      </CommandGroup>
                    </CommandList>
                  </Command>
                </PopoverContent>
              </Popover>
            </div>
          </div>
          <div className="flex shrink-0 flex-wrap items-center gap-2">
            {!projectOnly && !selectedProjectPath && selectedProfile && (
              <Button
                type="button"
                size="sm"
                className="h-9 px-3"
                disabled={applyMutation.isPending || createPending}
                onClick={() =>
                  applyMutation.mutate({ id: selectedProfile.id, scope })
                }
              >
                <Check className="mr-1 h-3.5 w-3.5" />
                {t("profiles.apply")}
              </Button>
            )}
            {selectedProfile && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-9 px-3"
                onClick={() => {
                  rememberProjectSelection(selectedProfile, scope);
                  setEditingProfile({ profile: selectedProfile, scope });
                }}
              >
                <Pencil className="mr-1 h-3.5 w-3.5" />
                {t("common.edit")}
              </Button>
            )}
            {selectedProjectPath && (
              <Button
                type="button"
                size="sm"
                className="h-9 px-3"
                onClick={() => {
                  if (selectedProfile && !deploymentDisabled)
                    void deployment.deployToProject(selectedProfile.id);
                }}
                disabled={
                  !selectedProfile ||
                  !hasScopeSnapshot(selectedProfile, scope) ||
                  deploymentDisabled ||
                  deployment.isPending
                }
              >
                <Upload className="mr-1 h-3.5 w-3.5" />
                {t("profiles.projectDeploy.button")}
              </Button>
            )}
            {allProfiles.some(
              (profile) => !projectOnly || profile.projectPath,
            ) && (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    type="button"
                    variant="quiet"
                    size="icon-compact"
                    className="h-9 w-9"
                    aria-label={t("profiles.context.more")}
                  >
                    <MoreHorizontal className="h-4 w-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  {allProfiles.some(
                    (profile) => !projectOnly || profile.projectPath,
                  ) && (
                    <DropdownMenuItem onSelect={() => setIsManageOpen(true)}>
                      <FolderCog className="h-4 w-4" />
                      {t("profiles.manage")}
                    </DropdownMenuItem>
                  )}
                </DropdownMenuContent>
              </DropdownMenu>
            )}
          </div>
        </div>
        {configuration.isFetching && (
          <p role="status" className="text-caption text-fg-3">
            {t("profiles.migration.pending")}
          </p>
        )}
        {configuration.isError && (
          <div
            role="alert"
            className="flex min-w-0 flex-wrap items-center gap-2 text-caption text-destructive"
          >
            <p className="min-w-0 flex-1 break-words">
              {t("profiles.migration.failed")}:{" "}
              {extractErrorMessage(configuration.error)}
            </p>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={configuration.isFetching}
              onClick={() => void configuration.refetch()}
            >
              {t("common.retry")}
            </Button>
          </div>
        )}
        {selectedProjectPath && (
          <div className="min-w-0 space-y-1">
            <p
              className="truncate text-caption text-fg-3"
              title={selectedProjectPath}
            >
              {selectedProjectPath}
            </p>
            {!projectOnly && (
              <p className="text-caption text-fg-2">
                {t("profiles.context.globalProvidersHint")}
              </p>
            )}
          </div>
        )}
      </section>

      <FullScreenPanel
        isOpen={isCreateOpen}
        title={t("profiles.createFromCurrent")}
        subtitle={t(`profiles.createDescription.${scope}`)}
        onClose={requestCloseCreatePanel}
        trackUnsavedChanges
        contentClassName="mx-auto max-w-3xl"
        footer={
          <>
            <Button
              variant="outline"
              onClick={closeCreatePanel}
              disabled={createPending}
            >
              {t("common.cancel")}
            </Button>
            <Button
              onClick={handleCreate}
              disabled={
                !newName.trim() ||
                createPending ||
                ((projectOnly || projectScoped) && !projectPath)
              }
            >
              {t("common.confirm")}
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          <Input
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            placeholder={t("profiles.namePlaceholder")}
            disabled={createPending}
            autoFocus
            onKeyDown={(e) => {
              if (e.key === "Enter") handleCreate();
            }}
          />
          {scope !== "claude-desktop" && (
            <div className="mt-3 space-y-2">
              {!projectOnly && (
                <>
                  <span className="text-xs text-fg-2">
                    {t("profiles.scope.label")}
                  </span>
                  <div
                    className="flex gap-2"
                    role="radiogroup"
                    aria-label={t("profiles.scope.label")}
                  >
                    <label className="flex items-center gap-2 rounded-control border border-border px-3 py-1.5 text-sm">
                      <input
                        type="radio"
                        name="profile-scope"
                        checked={!projectScoped}
                        onChange={() => {
                          setProjectScoped(false);
                          setProjectPath(null);
                          setCreateSource("current");
                        }}
                        disabled={createPending}
                      />
                      {t("profiles.scope.global")}
                    </label>
                    <label className="flex items-center gap-2 rounded-control border border-border px-3 py-1.5 text-sm">
                      <input
                        type="radio"
                        name="profile-scope"
                        checked={projectScoped}
                        onChange={() => setProjectScoped(true)}
                        disabled={createPending}
                      />
                      {t("profiles.scope.project")}
                    </label>
                  </div>
                </>
              )}
              {(projectOnly || projectScoped) && (
                <div className="space-y-1.5">
                  <label htmlFor={sourceSelectId} className="text-xs text-fg-2">
                    {t("profiles.createSource.label")}
                  </label>
                  <select
                    id={sourceSelectId}
                    value={createSource}
                    onChange={(event) =>
                      setCreateSource(
                        event.target.value === "project"
                          ? "project"
                          : "current",
                      )
                    }
                    disabled={createPending}
                    className="h-10 w-full rounded-control border border-border bg-surface px-3 text-sm"
                  >
                    <option value="current">
                      {t("profiles.createSource.current")}
                    </option>
                    <option value="project">
                      {t("profiles.createSource.project")}
                    </option>
                  </select>
                  {createSource === "project" && (
                    <p className="pb-2 text-sm text-fg-2">
                      {t("profiles.createSource.description")}
                    </p>
                  )}
                  <span className="text-xs text-fg-2">
                    {t("profiles.projectPath.label")}
                  </span>
                  <div className="flex min-w-0 items-center gap-2">
                    <Input
                      aria-label={t("profiles.projectPath.label")}
                      value={projectPath ?? ""}
                      readOnly
                      placeholder={t("profiles.projectPath.placeholder")}
                      className="min-w-0 flex-1"
                    />
                    <HoverTip content={t("profiles.projectPath.pick")}>
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        className="shrink-0"
                        aria-label={t("profiles.projectPath.pick")}
                        data-unsaved-edit
                        onClick={pickProjectPath}
                        disabled={createPending}
                      >
                        <FolderCog className="mr-1.5 h-4 w-4" />
                        {t("profiles.projectPath.pick")}
                      </Button>
                    </HoverTip>
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      </FullScreenPanel>

      <ConfirmDialog
        isOpen={confirmDiscardCreate}
        title={t("common.unsavedLeaveTitle")}
        message={t("common.unsavedLeaveMessage")}
        confirmText={t("common.unsavedLeaveConfirm")}
        cancelText={t("common.unsavedLeaveCancel")}
        onConfirm={closeCreatePanel}
        onCancel={() => setConfirmDiscardCreate(false)}
      />

      <ProfileManageDialog
        isOpen={isManageOpen}
        scope={scope}
        onClose={() => setIsManageOpen(false)}
        onEditProfile={handleEditProfile}
        projectOnly={projectOnly}
        deploymentDisabled={deploymentDisabled}
      />
      <ProjectProfileConflictDialog
        deployment={deployment}
        deploymentDisabled={deploymentDisabled}
      />
      {editingProfile && (
        <ProfileEditPanel
          profile={editingProfile.profile}
          scope={editingProfile.scope}
          onClose={closeProfileEditor}
        />
      )}
    </>
  );
}
