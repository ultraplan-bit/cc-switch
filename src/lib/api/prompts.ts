import { invoke } from "@tauri-apps/api/core";
import type { AppId } from "./types";

export interface Prompt {
  id: string;
  name: string;
  content: string;
  description?: string;
  createdAt?: number;
  updatedAt?: number;
}

/** 提示词目标文件：path 是完整路径（复制用），displayPath 把主目录写成 ~（显示用）。 */
export interface PromptFileLocation {
  path: string;
  displayPath: string;
}

export interface PromptTargetSnapshot extends PromptFileLocation {
  content: string | null;
  revision: string;
  appliedPromptId: string | null;
  managed: boolean;
  matchesLibrary: boolean;
}

export type PiPromptFileKind = "system_override" | "system_append";

export interface PiPromptFileSnapshot {
  exists: boolean;
  revision: string;
  content: string;
}

export interface PiPromptTemplate {
  slug: string;
  content: string;
  revision: string;
}

export const promptsApi = {
  async getPrompts(): Promise<Record<string, Prompt>> {
    return await invoke("get_prompts");
  },

  async upsertPrompt(id: string, prompt: Prompt): Promise<void> {
    return await invoke("upsert_prompt", { id, prompt });
  },

  async deletePrompt(id: string): Promise<void> {
    return await invoke("delete_prompt", { id });
  },

  async getPromptTarget(app: AppId): Promise<PromptTargetSnapshot> {
    return await invoke("get_prompt_target", { app });
  },

  async applyPrompt(
    app: AppId,
    id: string,
    expectedRevision: string,
    expectedContent: string,
  ): Promise<PromptTargetSnapshot> {
    return await invoke("apply_prompt", {
      app,
      id,
      expectedRevision,
      expectedContent,
    });
  },

  async removePromptApplication(
    app: AppId,
    expectedRevision: string,
  ): Promise<PromptTargetSnapshot> {
    return await invoke("remove_prompt_application", { app, expectedRevision });
  },

  async importFromFile(app: AppId): Promise<string> {
    return await invoke("import_prompt_from_file", { app });
  },

  async getPiPromptFile(kind: PiPromptFileKind): Promise<PiPromptFileSnapshot> {
    return await invoke("get_pi_prompt_file", { kind });
  },

  async replacePiPromptFile(
    kind: PiPromptFileKind,
    expectedRevision: string,
    content: string,
  ): Promise<PiPromptFileSnapshot> {
    return await invoke("replace_pi_prompt_file", {
      kind,
      expectedRevision,
      content,
    });
  },

  async deletePiPromptFile(
    kind: PiPromptFileKind,
    expectedRevision: string,
  ): Promise<boolean> {
    return await invoke("delete_pi_prompt_file", { kind, expectedRevision });
  },

  async listPiPromptTemplates(): Promise<PiPromptTemplate[]> {
    return await invoke("list_pi_prompt_templates");
  },

  async upsertPiPromptTemplate(
    slug: string,
    expectedRevision: string,
    content: string,
    originalSlug?: string,
  ): Promise<PiPromptTemplate> {
    return await invoke("upsert_pi_prompt_template", {
      slug,
      originalSlug: originalSlug ?? null,
      expectedRevision,
      content,
    });
  },

  async deletePiPromptTemplate(
    slug: string,
    expectedRevision: string,
  ): Promise<boolean> {
    return await invoke("delete_pi_prompt_template", {
      slug,
      expectedRevision,
    });
  },
};
