import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "@/lib/toast";
import { profilesApi, providersApi } from "@/lib/api";
import type {
  ProfileConfig,
  ProfileCreateSource,
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

/** Every profile write changes the list and the tray menu; the caller supplies the wording. */
function useProfileWrite<Input, Output>(
  mutationFn: (input: Input) => Promise<Output>,
  failedKey: string,
  successKey?: string,
) {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  return useMutation({
    networkMode: "always",
    mutationFn,
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["profiles"] });
      await updateTrayMenuSafely();
      if (successKey) toast.success(t(successKey), { closeButton: true });
    },
    onError: (error: Error) => {
      toast.error(
        t(failedKey, {
          detail: extractErrorMessage(error) || t("common.unknown"),
        }),
        { closeButton: true },
      );
    },
  });
}

export const useCreateProfileMutation = () =>
  useProfileWrite(
    ({
      name,
      scope,
      source,
    }: {
      name: string;
      scope: ProfileScope;
      source: ProfileCreateSource;
    }) => profilesApi.create(name, scope, source),
    "profiles.createFailed",
    "profiles.createSuccess",
  );

export const useDuplicateProfileMutation = () =>
  useProfileWrite(
    ({
      sourceId,
      name,
      scope,
    }: {
      sourceId: string;
      name: string;
      scope: ProfileScope;
    }) => profilesApi.duplicate(sourceId, name, scope),
    "profiles.createFailed",
    "profiles.createSuccess",
  );

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

export const useUpdateProfileMutation = () =>
  useProfileWrite(
    ({
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
    "profiles.updateFailed",
    "profiles.updateSuccess",
  );

export const useSaveProfileConfigMutation = () =>
  useProfileWrite(
    ({
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
    "profiles.updateFailed",
    "profiles.updateSuccess",
  );

export const useDeleteProfileMutation = () =>
  useProfileWrite(
    ({ id, scope }: { id: string; scope: ProfileScope }) =>
      profilesApi.delete(id, scope),
    "profiles.deleteFailed",
    "profiles.deleteSuccess",
  );

export const useAddProfileProjectMutation = () =>
  useProfileWrite(
    (path: string) => profilesApi.addProject(path),
    "profiles.projects.addFailed",
  );

export const useRemoveProfileProjectMutation = () =>
  useProfileWrite(
    (path: string) => profilesApi.removeProject(path),
    "profiles.projects.removeFailed",
    "profiles.projects.removed",
  );

export const useDeployProfileToProjectMutation = () => {
  const queryClient = useQueryClient();
  return useMutation({
    networkMode: "always",
    mutationFn: ({
      id,
      scope,
      projectPath,
      resolution,
    }: {
      id: string;
      scope: ProfileScope;
      projectPath: string;
      resolution?: ProjectDeployResolution | null;
    }) => profilesApi.deployToProject(id, scope, projectPath, resolution),
    onSuccess: async (result) => {
      if (result.status === "applied") {
        await queryClient.invalidateQueries({ queryKey: ["profiles"] });
      }
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
      // A failed application can still have switched the connection.
      void queryClient.invalidateQueries({ queryKey: ["profiles"] });
      const detail = extractErrorMessage(error) || t("common.unknown");
      toast.error(t("profiles.applyFailed", { detail }), {
        closeButton: true,
      });
    },
  });
};
