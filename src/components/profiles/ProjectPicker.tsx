import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, ChevronsUpDown, FolderOpen, FolderPlus } from "lucide-react";
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
import { Button } from "@/components/ui/button";
import type { ProfileProject } from "@/lib/api/profiles";
import { cn } from "@/lib/utils";
import { projectName } from "./scope";

interface ProjectPickerProps {
  projects: ProfileProject[];
  selected: ProfileProject | undefined;
  onSelect: (path: string) => void;
  onAdd: () => void;
  adding: boolean;
}

/** Chooses which registered project the page shows. Adding a folder is a separate button. */
export function ProjectPicker({
  projects,
  selected,
  onSelect,
  onAdd,
  adding,
}: ProjectPickerProps) {
  const { t } = useTranslation();
  const labelId = useId();
  const [open, setOpen] = useState(false);

  return (
    <div className="flex min-w-0 flex-wrap items-center gap-2">
      {projects.length > 0 && (
        <Popover open={open} onOpenChange={setOpen}>
          <span id={labelId} className="sr-only">
            {t("profiles.projects.select")}
          </span>
          <PopoverTrigger asChild>
            <Button
              type="button"
              variant="outline"
              role="combobox"
              aria-expanded={open}
              aria-labelledby={labelId}
              title={selected?.path}
              data-project-path={selected?.path ?? ""}
              className="h-9 w-72 min-w-0 max-w-full justify-between gap-2 px-3"
            >
              <FolderOpen className="h-4 w-4 shrink-0 text-fg-3" />
              <span className="min-w-0 flex-1 truncate text-start">
                {selected
                  ? projectName(selected.path)
                  : t("profiles.projects.select")}
              </span>
              <ChevronsUpDown className="h-3.5 w-3.5 shrink-0 text-fg-3" />
            </Button>
          </PopoverTrigger>
          <PopoverContent
            align="start"
            className="z-[100] w-[min(26rem,calc(100vw-3rem))] p-0"
          >
            <Command label={t("profiles.projects.search")}>
              <CommandInput placeholder={t("profiles.projects.search")} />
              <CommandList>
                <CommandEmpty>{t("profiles.projects.noMatches")}</CommandEmpty>
                <CommandGroup>
                  {projects.map((project) => (
                    <CommandItem
                      key={project.path}
                      value={project.path}
                      aria-label={project.path}
                      keywords={[projectName(project.path), project.path]}
                      onSelect={() => {
                        onSelect(project.path);
                        setOpen(false);
                      }}
                    >
                      <Check
                        className={cn(
                          "mr-2 h-4 w-4 shrink-0",
                          selected?.path === project.path
                            ? "opacity-100"
                            : "opacity-0",
                        )}
                      />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate">
                          {projectName(project.path)}
                          {!project.available &&
                            ` · ${t("profiles.editor.unavailable")}`}
                        </span>
                        <span
                          className="block truncate text-xs text-fg-3"
                          title={project.path}
                        >
                          {project.path}
                        </span>
                      </span>
                    </CommandItem>
                  ))}
                </CommandGroup>
              </CommandList>
            </Command>
          </PopoverContent>
        </Popover>
      )}
      <Button
        type="button"
        variant="outline"
        className="h-9"
        disabled={adding}
        onClick={onAdd}
      >
        <FolderPlus className="h-4 w-4" />
        {t("profiles.projects.add")}
      </Button>
    </div>
  );
}
