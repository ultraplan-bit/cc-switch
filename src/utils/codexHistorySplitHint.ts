import type { TFunction } from "i18next";
import type { ExternalToast } from "sonner";
import type { CodexHistorySplitHint } from "@/lib/api/providers";

/**
 * Codex 切换跨了组（官方 openai ↔ 第三方 custom）、统一会话历史又关着时，在「切换成功」
 * 那条提示下面多一行：新版 Codex 只能继续当前组的会话，另一组只能查看。只是提醒，
 * 不改写已有会话，
 * 多停几秒让人看得完。
 */
export function codexHistorySplitToastExtras(
  hint: CodexHistorySplitHint,
  t: TFunction,
): Pick<ExternalToast, "description" | "duration"> {
  const previous =
    hint.previousBucket === "openai"
      ? t("notifications.codexHistorySplit.officialBucket", {
          defaultValue: "官方订阅",
        })
      : t("notifications.codexHistorySplit.thirdPartyBucket", {
          defaultValue: "第三方供应商",
        });

  return {
    description: t("notifications.codexHistorySplit.hint", {
      defaultValue: "之前在{{previous}}下的 {{count}} 个会话现在只能查看",
      previous,
      count: hint.lockedThreads,
    }),
    duration: 6000,
  };
}
