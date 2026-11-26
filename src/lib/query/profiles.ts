import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "@/lib/toast";
import { profilesApi, providersApi } from "@/lib/api";
import type {
  ProfileConfig,
  ProfileScope,
  ProjectDeployResolution,
} from "@/lib/api/profiles";
import { extractErrorMessage } from "@/utils/errorUtils";

const updateTrayMenuSafely = async () => {
  try {
    await providersApi.updateTrayMenu();
  } catch (trayError) {
    console.error("Failed to update tray menu after profile change", trayError);
  }
};

/** A failed/partial migration must keep legacy configuration editing available. */
export const useProfileConfiguration = (
  scope?: ProfileScope,
  enabled = true,
) => {
  const queryClient = useQueryClient();
  return useQuery({
    queryKey: ["profile-configuration", scope],
    networkMode: "always",
    enabled: Boolean(scope) && enabled,
    staleTime: Infinity,
    retry: false,
    queryFn: async () => {
      const result = await profilesApi.migrateLegacy(scope!);
      await queryClient.invalidateQueries({ queryKey: ["profiles"] });
      if (result.warnings.length || result.ready === false) {
        throw new Error(result.warnings.join("\n"));
      }
      return result;
    },
  });
};

export const useProfilesQuery = () => {
  return useQuery({
    networkMode: "always",
    queryKey: ["profiles"],
    queryFn: () => profilesApi.list(),
  });
};

export const useCreateProfileMutation = () => {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    networkMode: "always",
    mutationFn: ({
      name,
      scope,
      projectPath,
    }: {
      name: string;
      scope: ProfileScope;
      projectPath?: string | null;
    }) => profilesApi.create(name, scope, projectPath),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["profiles"] });
      await updateTrayMenuSafely();
      toast.success(t("profiles.createSuccess"), { closeButton: true });
    },
    onError: (error: Error) => {
      const detail = extractErrorMessage(error) || t("common.unknown");
      toast.error(t("profiles.createFailed", { detail }), {
        closeButton: true,
      });
    },
  });
};

export const useDuplicateProfileMutation = () => {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    networkMode: "always",
    mutationFn: ({
      sourceId,
      name,
      projectPath,
    }: {
      sourceId: string;
      name: string;
      projectPath: string;
    }) => profilesApi.duplicate(sourceId, name, projectPath),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["profiles"] });
      await updateTrayMenuSafely();
      toast.success(t("profiles.createSuccess"), { closeButton: true });
    },
    onError: (error: Error) => {
      const detail = extractErrorMessage(error) || t("common.unknown");
      toast.error(t("profiles.createFailed", { detail }), {
        closeButton: true,
      });
    },
  });
};

export const useImportProjectProfileMutation = () => {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  return useMutation({
    networkMode: "always",
    mutationFn: ({
      name,
      scope,
      projectPath,
    }: {
      name: string;
      scope: ProfileScope;
      projectPath: string;
    }) => profilesApi.importFromProject(name, scope, projectPath),
    onSuccess: async ({ warnings }) => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["profiles"] }),
        queryClient.invalidateQueries({ queryKey: ["mcp", "all"] }),
        queryClient.invalidateQueries({ queryKey: ["skills", "installed"] }),
        queryClient.invalidateQueries({ queryKey: ["prompts"] }),
      ]);
      await updateTrayMenuSafely();
      if (warnings.length) {
        toast.warning(t("profiles.import.completedWithNotes"), {
          description: warnings.join("\n"),
          duration: 10000,
          closeButton: true,
        });
      } else {
        toast.success(t("profiles.createSuccess"), { closeButton: true });
      }
    },
    onError: (error: Error) => {
      toast.error(
        t("profiles.createFailed", {
          detail: extractErrorMessage(error) || t("common.unknown"),
        }),
        { closeButton: true },
      );
    },
  });
};

export const useUpdateProfileMutation = () => {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    networkMode: "always",
    mutationFn: ({
      id,
      name,
      resnapshot,
      scope,
    }: {
      id: string;
      name?: string;
      resnapshot?: boolean;
      scope?: ProfileScope;
    }) => profilesApi.update(id, { name, resnapshot, scope }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["profiles"] });
      await updateTrayMenuSafely();
      toast.success(t("profiles.updateSuccess"), { closeButton: true });
    },
    onError: (error: Error) => {
      const detail = extractErrorMessage(error) || t("common.unknown");
      toast.error(t("profiles.updateFailed", { detail }), {
        closeButton: true,
      });
    },
  });
};

export const useSetProfileProjectPathMutation = () => {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    networkMode: "always",
    mutationFn: ({
      id,
      projectPath,
    }: {
      id: string;
      projectPath: string | null;
    }) => profilesApi.setProjectPath(id, projectPath),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["profiles"] });
      await updateTrayMenuSafely();
      toast.success(t("profiles.projectPath.saveSuccess"));
    },
    onError: (error: Error) => {
      toast.error(
        t("profiles.projectPath.saveFailed", {
          detail: extractErrorMessage(error) || t("common.unknown"),
        }),
      );
    },
  });
};

export const useSaveProfileConfigMutation = () => {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  return useMutation({
    networkMode: "always",
    mutationFn: ({
      id,
      scope,
      config,
      name,
    }: {
      id: string;
      scope: ProfileScope;
      config: ProfileConfig;
      name?: string;
    }) => profilesApi.saveConfig(id, scope, config, name),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["profiles"] });
      await updateTrayMenuSafely();
      toast.success(t("profiles.updateSuccess"), { closeButton: true });
    },
    onError: (error: Error) => {
      toast.error(
        t("profiles.updateFailed", {
          detail: extractErrorMessage(error) || t("common.unknown"),
        }),
        { closeButton: true },
      );
    },
  });
};

export const useDeployProfileToProjectMutation = () => {
  const queryClient = useQueryClient();
  return useMutation({
    networkMode: "always",
    mutationFn: ({
      id,
      scope,
      resolution,
    }: {
      id: string;
      scope: ProfileScope;
      resolution?: ProjectDeployResolution | null;
    }) => profilesApi.deployToProject(id, scope, resolution),
    onSuccess: async (result) => {
      if (result.status === "applied") {
        await queryClient.invalidateQueries({ queryKey: ["profiles"] });
      }
    },
  });
};

export const useDeleteProfileMutation = () => {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    networkMode: "always",
    mutationFn: (id: string) => profilesApi.delete(id),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["profiles"] });
      await updateTrayMenuSafely();
      toast.success(t("profiles.deleteSuccess"), { closeButton: true });
    },
    onError: (error: Error) => {
      const detail = extractErrorMessage(error) || t("common.unknown");
      toast.error(t("profiles.deleteFailed", { detail }), {
        closeButton: true,
      });
    },
  });
};

export const useClearProfileMutation = () => {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    networkMode: "always",
    mutationFn: (scope: ProfileScope) => profilesApi.clearCurrent(scope),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["profiles"] });
      await updateTrayMenuSafely();
      toast.success(t("profiles.clearSuccess"), { closeButton: true });
    },
    onError: (error: Error) => {
      const detail = extractErrorMessage(error) || t("common.unknown");
      toast.error(t("profiles.applyFailed", { detail }), {
        closeButton: true,
      });
    },
  });
};

export const useApplyProfileMutation = () => {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    networkMode: "always",
    mutationFn: ({ id, scope }: { id: string; scope: ProfileScope }) =>
      profilesApi.apply(id, scope),
    onSuccess: async (warnings) => {
      await queryClient.invalidateQueries({ queryKey: ["profiles"] });
      await queryClient.invalidateQueries({
        queryKey: ["providers", "claude"],
      });
      await queryClient.invalidateQueries({
        queryKey: ["providers", "claude-desktop"],
      });
      await queryClient.invalidateQueries({ queryKey: ["providers", "codex"] });
      await queryClient.invalidateQueries({ queryKey: ["mcp", "all"] });
      await queryClient.invalidateQueries({ queryKey: ["skills"] });
      await updateTrayMenuSafely();

      if (warnings.length > 0) {
        toast.warning(
          t("profiles.applyWarnings", {
            warningCount: warnings.length,
            details: warnings.join("\n"),
          }),
          { closeButton: true, duration: 10000 },
        );
      } else {
        toast.success(t("profiles.applySuccess"), { closeButton: true });
      }
    },
    onError: (error: Error) => {
      const detail = extractErrorMessage(error) || t("common.unknown");
      toast.error(t("profiles.applyFailed", { detail }), {
        closeButton: true,
      });
    },
  });
};
