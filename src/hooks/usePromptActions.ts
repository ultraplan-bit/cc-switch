import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "@/lib/toast";
import { promptsApi, type AppId, type Prompt } from "@/lib/api";
import { promptKeys } from "@/lib/query/prompts";
import { extractErrorMessage } from "@/utils/errorUtils";

const EMPTY_PROMPTS: Record<string, Prompt> = {};

/** The library has one identity; changing Agent only changes the target snapshot. */
export function usePromptActions(appId: AppId, available = true) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const writeLock = useRef(false);
  const [pending, setPending] = useState(false);
  const library = useQuery({
    queryKey: promptKeys.list(),
    queryFn: () => promptsApi.getPrompts(),
    networkMode: "always",
  });
  const target = useQuery({
    queryKey: promptKeys.target(appId),
    queryFn: () => promptsApi.getPromptTarget(appId),
    networkMode: "always",
    enabled: available,
  });

  const refresh = async () => {
    await queryClient.invalidateQueries({ queryKey: promptKeys.all });
    return queryClient.getQueryState(promptKeys.list())?.status !== "error";
  };
  const write = async (action: () => Promise<void>): Promise<boolean> => {
    if (writeLock.current) return false;
    writeLock.current = true;
    setPending(true);
    try {
      await action();
      try {
        if (!(await refresh())) toast.error(t("prompts.library.refreshFailed"));
      } catch {
        toast.error(t("prompts.library.refreshFailed"));
      }
      return true;
    } catch (error) {
      toast.error(
        extractErrorMessage(error) || t("prompts.library.writeFailed"),
      );
      return false;
    } finally {
      writeLock.current = false;
      setPending(false);
    }
  };

  return {
    prompts: library.data ?? EMPTY_PROMPTS,
    library,
    target,
    pending,
    refresh,
    savePrompt: (id: string, prompt: Prompt) =>
      write(async () => {
        await promptsApi.upsertPrompt(id, prompt);
        queryClient.setQueryData<Record<string, Prompt>>(
          promptKeys.list(),
          (current) => ({ ...current, [id]: prompt }),
        );
      }),
    deletePrompt: (id: string) =>
      write(async () => {
        await promptsApi.deletePrompt(id);
        queryClient.setQueryData<Record<string, Prompt>>(
          promptKeys.list(),
          (current) => {
            const next = { ...current };
            delete next[id];
            return next;
          },
        );
      }),
    importFromFile: () =>
      available
        ? write(async () => {
            await promptsApi.importFromFile(appId);
          })
        : Promise.resolve(false),
    removeApplication: (revision: string) =>
      available
        ? write(async () => {
            await promptsApi.removePromptApplication(appId, revision);
          })
        : Promise.resolve(false),
  };
}
