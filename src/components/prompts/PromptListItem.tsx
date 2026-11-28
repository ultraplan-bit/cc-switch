import React from "react";
import { useTranslation } from "react-i18next";
import { MoreHorizontal, Pencil } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { HoverTip } from "@/components/ui/hover-tip";
import type { Prompt } from "@/lib/api";
import { cn } from "@/lib/utils";
import { promptMenuContentClass, promptMenuItemClass } from "./PromptPageFrame";

interface PromptListItemProps {
  id: string;
  prompt: Prompt;
  /** Last applied document for the selected target. */
  active: boolean;
  pendingUpdate?: boolean;
  /** 第二行：说明（没有就用正文第一行）· 大小 · 更新时间 */
  detail: string;
  first: boolean;
  disabled?: boolean;
  applyDisabled?: boolean;
  onApply: (id: string) => void;
  onEdit: (id: string) => void;
  onDelete: (id: string) => void;
  onCopyContent?: (id: string) => void;
}

/** Shared document row with explicit distribution action. */
const PromptListItem: React.FC<PromptListItemProps> = ({
  id,
  prompt,
  active,
  pendingUpdate,
  detail,
  first,
  disabled = false,
  applyDisabled = false,
  onApply,
  onEdit,
  onDelete,
  onCopyContent,
}) => {
  const { t } = useTranslation();

  return (
    <li
      data-testid={`prompt-row-${id}`}
      onClick={(event) => {
        if (disabled) return;
        const target = event.target as HTMLElement;
        if (target.closest("button, a, input, label, [role='menu']")) return;
        onEdit(id);
      }}
      className={cn(
        "flex h-[60px] cursor-pointer items-center gap-3 pe-2 ps-4 transition-colors duration-150 hover:bg-subtle",
        !first && "border-t border-border",
        active && "bg-subtle",
      )}
    >
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex min-w-0 items-center gap-1.5">
          <span
            title={prompt.name}
            className="min-w-0 truncate text-body font-medium text-fg-1"
          >
            {prompt.name}
          </span>
          {active ? (
            <span className="inline-flex h-[18px] shrink-0 items-center whitespace-nowrap rounded-full bg-success-soft px-[7px] text-badge text-success-text">
              {t(
                pendingUpdate
                  ? "prompts.library.pending"
                  : "prompts.library.applied",
              )}
            </span>
          ) : null}
        </div>
        <span
          title={detail}
          className="min-w-0 truncate text-caption text-fg-2"
        >
          {detail}
        </span>
      </div>

      <Button
        type="button"
        variant="neutral"
        size="compact"
        disabled={disabled || applyDisabled}
        aria-label={t("prompts.library.applyAria", {
          name: prompt.name,
        })}
        onClick={() => onApply(id)}
        className="min-w-14 shrink-0"
      >
        {t(active ? "prompts.library.reapply" : "prompts.library.distribute")}
      </Button>

      <div className="flex shrink-0 gap-1">
        <HoverTip content={t("common.edit")}>
          <Button
            type="button"
            variant="quiet"
            size="icon-compact"
            disabled={disabled}
            aria-label={t("prompts.editAria", { name: prompt.name })}
            onClick={() => onEdit(id)}
          >
            <Pencil className="h-[15px] w-[15px]" strokeWidth={1.5} />
          </Button>
        </HoverTip>
        <DropdownMenu modal={false}>
          <HoverTip content={t("common.more")}>
            <DropdownMenuTrigger asChild>
              <Button
                type="button"
                variant="quiet"
                size="icon-compact"
                disabled={disabled}
                aria-label={t("prompts.rowMoreActions", { name: prompt.name })}
              >
                <MoreHorizontal
                  className="h-[15px] w-[15px]"
                  strokeWidth={1.5}
                />
              </Button>
            </DropdownMenuTrigger>
          </HoverTip>
          <DropdownMenuContent
            align="end"
            className={cn(promptMenuContentClass, "w-[240px]")}
          >
            {onCopyContent ? (
              <DropdownMenuItem
                className={promptMenuItemClass}
                onSelect={() => onCopyContent(id)}
              >
                {t("prompts.copyContent")}
              </DropdownMenuItem>
            ) : null}
            {onCopyContent ? (
              <DropdownMenuSeparator className="mx-1.5 my-1 bg-border" />
            ) : null}
            <DropdownMenuItem
              onSelect={() => onDelete(id)}
              className={cn(
                promptMenuItemClass,
                "text-danger-text focus:text-danger-text",
              )}
            >
              <span>{t("common.delete")}</span>
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </li>
  );
};

export default PromptListItem;
