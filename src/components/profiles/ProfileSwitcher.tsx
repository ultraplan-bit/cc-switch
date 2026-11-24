import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Check,
  ChevronsUpDown,
  FolderCog,
  FolderOpen,
  Plus,
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { HoverTip } from "@/components/ui/hover-tip";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { settingsApi } from "@/lib/api";
import type { AppId } from "@/lib/api/types";
import {
  useApplyProfileMutation,
  useClearProfileMutation,
  useCreateProfileMutation,
  useProfilesQuery,
} from "@/lib/query/profiles";
import { ProfileManageDialog } from "./ProfileManageDialog";
import { APP_PROFILE_SCOPE, hasScopeSnapshot } from "./scope";
import type { CurrentProfileIds, ProfileScope } from "@/lib/api/profiles";
import { toast } from "@/lib/toast";
import { extractErrorMessage } from "@/utils/errorUtils";

const CURRENT_ID_KEY: Record<ProfileScope, keyof CurrentProfileIds> = {
  claude: "claude",
  "claude-desktop": "claudeDesktop",
  codex: "codex",
};

interface ProfileSwitcherProps {
  activeApp: AppId;
}

/**
 * 项目 Profile 切换器（header 左侧入口）
 *
 * 项目列表全应用共享（用户拥有的项目就那几个），但切换按分组进行：
 * Claude 组（Claude Code 的供应商/MCP/Skills/记忆文件 + Claude Desktop
 * 的供应商）与 Codex 组各自指向自己的当前项目、只应用组内快照。
 * 与侧栏的应用列表（只切换查看的应用）语义不同。
 */
export function ProfileSwitcher({ activeApp }: ProfileSwitcherProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [isCreateOpen, setIsCreateOpen] = useState(false);
  const [isManageOpen, setIsManageOpen] = useState(false);
  const [newName, setNewName] = useState("");
  const [projectPath, setProjectPath] = useState<string | null>(null);
  const creating = useRef(false);

  const { data } = useProfilesQuery();
  const applyMutation = useApplyProfileMutation();
  const clearMutation = useClearProfileMutation();
  const createMutation = useCreateProfileMutation();

  // Profile 仅作用于受支持的应用——在其他应用的标签页展示会误导用户
  // 以为当前应用也被切换了，因此只在有所属分组的应用页面渲染
  const scope = APP_PROFILE_SCOPE[activeApp];
  if (!scope) {
    return null;
  }

  const profiles = data?.profiles ?? [];
  const currentId = data?.currentIds?.[CURRENT_ID_KEY[scope]] ?? null;
  const currentProfile = profiles.find((p) => p.id === currentId);

  const handleApply = (id: string) => {
    setOpen(false);
    if (id !== currentId) {
      applyMutation.mutate({ id, scope });
    }
  };

  const closeCreateDialog = () => {
    if (creating.current) return;
    setIsCreateOpen(false);
    setNewName("");
    setProjectPath(null);
  };

  const handleCreate = () => {
    const name = newName.trim();
    if (!name || creating.current) return;
    creating.current = true;
    createMutation.mutate(
      { name, scope, projectPath },
      {
        onSuccess: () => {
          creating.current = false;
          closeCreateDialog();
        },
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

  return (
    <>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <button
            type="button"
            role="combobox"
            aria-expanded={open}
            aria-label={t("profiles.switcherAriaLabel", {
              name: currentProfile?.name ?? t("profiles.none"),
            })}
            title={t(`profiles.switcherTooltip.${scope}`)}
            className={cn(
              "inline-flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-sm font-medium transition-colors",
              "hover:bg-black/5 dark:hover:bg-white/5",
              currentProfile ? "text-fg-1" : "text-fg-2",
            )}
          >
            <FolderOpen className="h-4 w-4 shrink-0 opacity-70" />
            <span className="max-w-[9rem] truncate">
              {currentProfile?.name ?? t("profiles.none")}
            </span>
            <ChevronsUpDown className="h-3.5 w-3.5 shrink-0 opacity-50" />
          </button>
        </PopoverTrigger>
        <PopoverContent
          side="bottom"
          align="start"
          sideOffset={6}
          aria-label={t(`profiles.switcherTooltip.${scope}`)}
          className="z-[100] w-64 p-0"
        >
          <Command label={t("profiles.searchPlaceholder")}>
            <CommandInput placeholder={t("profiles.searchPlaceholder")} />
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
                          currentId === profile.id
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
                    setProjectPath(null);
                    setIsCreateOpen(true);
                  }}
                >
                  <Plus className="mr-2 h-4 w-4 shrink-0" />
                  {t("profiles.createFromCurrent")}
                </CommandItem>
                {currentId && (
                  <CommandItem
                    value="__clear__"
                    keywords={[t("profiles.none")]}
                    onSelect={() => {
                      setOpen(false);
                      clearMutation.mutate(scope);
                    }}
                  >
                    <X className="mr-2 h-4 w-4 shrink-0" />
                    {t("profiles.none")}
                  </CommandItem>
                )}
                {profiles.length > 0 && (
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

      <Dialog
        open={isCreateOpen}
        onOpenChange={(open) => {
          if (!open) closeCreateDialog();
        }}
      >
        <DialogContent className="max-w-md" zIndex="alert">
          <DialogHeader className="space-y-3 border-b-0 bg-transparent pb-0">
            <DialogTitle>{t("profiles.createFromCurrent")}</DialogTitle>
            <DialogDescription>
              {t(`profiles.createDescription.${scope}`)}
            </DialogDescription>
          </DialogHeader>
          <div className="px-6 pt-3">
            <Input
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              placeholder={t("profiles.namePlaceholder")}
              disabled={createMutation.isPending}
              autoFocus
              onKeyDown={(e) => {
                if (e.key === "Enter") handleCreate();
              }}
            />
            <div className="mt-3 space-y-1.5">
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
                    onClick={pickProjectPath}
                    disabled={createMutation.isPending}
                  >
                    <FolderCog className="mr-1.5 h-4 w-4" />
                    {t("profiles.projectPath.pick")}
                  </Button>
                </HoverTip>
                {projectPath !== null && (
                  <HoverTip content={t("profiles.projectPath.clear")}>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="h-8 w-8 shrink-0"
                      aria-label={t("profiles.projectPath.clear")}
                      onClick={() => setProjectPath(null)}
                      disabled={createMutation.isPending}
                    >
                      <X className="h-4 w-4" />
                    </Button>
                  </HoverTip>
                )}
              </div>
            </div>
          </div>
          <DialogFooter className="flex gap-2 border-t-0 bg-transparent pt-2 sm:justify-end">
            <Button
              variant="outline"
              onClick={closeCreateDialog}
              disabled={createMutation.isPending}
            >
              {t("common.cancel")}
            </Button>
            <Button
              onClick={handleCreate}
              disabled={!newName.trim() || createMutation.isPending}
            >
              {t("common.confirm")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ProfileManageDialog
        isOpen={isManageOpen}
        scope={scope}
        onClose={() => setIsManageOpen(false)}
      />
    </>
  );
}
