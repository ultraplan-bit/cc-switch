import { invoke } from "@tauri-apps/api/core";

/**
 * Profile 操作的应用分组（与后端 services/profile.rs 的 ProfileScope 严格对应）
 *
 * 项目实体全应用共享，但快照/应用/当前指针按组进行；Claude Code 与
 * Claude Desktop 的供应商独立切换，因此各自有独立分组。
 */
export type ProfileScope = "claude" | "claude-desktop" | "codex";

/**
 * 按 app 分槽的载荷容器（与后端 services/profile.rs 的 PerApp<T> 严格对应）
 */
export interface PerApp<T> {
  claude: T;
  "claude-desktop": T;
  codex: T;
}

/**
 * 项目 Profile 的配置快照（与后端 ProfilePayload 严格对应）
 *
 * 所有槽位 null = 该侧从未拍过快照（应用时不动），与"拍到的就是空集"
 * （空数组，应用时清空启用）严格区分。
 */
export interface ProfilePayload {
  providers: PerApp<string | null>;
  models?: PerApp<string | null>;
  /** Agent-native model settings independent of the selected provider. */
  modelSettings?: PerApp<Record<string, string> | null>;
  /** Codex model catalog and Auto Review configuration snapshot. */
  modelCatalog?: PerApp<Record<string, unknown> | null>;
  /** Agent behavior only: JSON for Claude, TOML for Codex. Connections stay in providers. */
  nativeConfig?: PerApp<string | null>;
  mcp: PerApp<string[] | null>;
  skills: PerApp<string[] | null>;
  prompts: PerApp<string | null>;
  /** Last known display names only; resource definitions remain in the library. */
  resourceNames?: PerApp<ProfileResourceNames>;
  /** Imported library IDs retain their original project MCP keys and Skill directories. */
  projectAliases?: PerApp<{
    mcp: Record<string, string>;
    skills: Record<string, string>;
  }>;
}

export interface ProfileResourceNames {
  mcp: Record<string, string>;
  skills: Record<string, string>;
  prompts: Record<string, string>;
}

export interface ProfileConfig {
  providerId: string | null;
  model: string;
  modelSettings: Record<string, string>;
  modelCatalog: Record<string, unknown> | null;
  nativeConfig?: string | null;
  mcpIds: string[];
  skillIds: string[];
  promptId: string | null;
}

export interface Profile {
  id: string;
  name: string;
  payload: ProfilePayload;
  projectPath?: string | null;
  deployedScopes?: ProfileScope[];
  createdAt?: number;
  updatedAt?: number;
}

export type ProjectDeployResolution = "overwrite" | "keepCurrent" | "skip";

export interface ProjectDeployConflict {
  path: string;
  reason: string;
}

export type ProjectDeployResult =
  | {
      status: "needsResolution";
      conflicts: ProjectDeployConflict[];
      warnings: string[];
    }
  | { status: "applied"; changed: string[]; warnings: string[] }
  | { status: "skipped" };

/** 每个分组当前激活的项目 id（未使用项目时为 null）
 *
 * 注意：JSON key 是 camelCase（claudeDesktop），与 ProfileScope 的 kebab-case
 * 字符串不同——后者用于命令参数，前者用于响应字段。
 */
export interface CurrentProfileIds {
  claude: string | null;
  claudeDesktop: string | null;
  codex: string | null;
}

export interface ProfilesResponse {
  profiles: Profile[];
  currentIds: CurrentProfileIds;
}

export interface ProfileMigrationResult {
  profiles: Profile[];
  warnings: string[];
  ready?: boolean;
}

export const profilesApi = {
  /** Idempotently preserve legacy configuration before using Profile-only editing. */
  async migrateLegacy(scope: ProfileScope): Promise<ProfileMigrationResult> {
    return await invoke("migrate_legacy_agent_profiles", { scope });
  },
  /**
   * 获取所有项目及各分组当前激活项目 id
   */
  async list(): Promise<ProfilesResponse> {
    return await invoke("list_profiles");
  },

  /**
   * 创建新项目（只拍发起页所属分组的当前状态，其余分组槽位留空）
   */
  async create(
    name: string,
    scope: ProfileScope,
    projectPath: string | null = null,
  ): Promise<Profile> {
    return await invoke("create_profile", { name, scope, projectPath });
  },

  async duplicate(
    sourceId: string,
    name: string,
    projectPath: string | null = null,
  ): Promise<Profile> {
    return await invoke("duplicate_profile", { sourceId, name, projectPath });
  },

  async importFromProject(
    name: string,
    scope: ProfileScope,
    projectPath: string,
  ): Promise<{ profile: Profile; warnings: string[] }> {
    return await invoke("import_project_profile", { name, scope, projectPath });
  },

  /**
   * 更新项目：重命名（作用于共享实体）和/或以当前状态重拍快照
   * （resnapshot 只覆盖 scope 分组的槽位，其余分组原样保留）
   */
  async update(
    id: string,
    options: { name?: string; resnapshot?: boolean; scope?: ProfileScope },
  ): Promise<Profile> {
    return await invoke("update_profile", {
      id,
      name: options.name,
      resnapshot: options.resnapshot,
      scope: options.scope,
    });
  },

  /**
   * 绑定项目目录路径；null 表示解绑
   */
  async setProjectPath(
    id: string,
    projectPath: string | null,
  ): Promise<Profile> {
    return await invoke("set_profile_project_path", { id, projectPath });
  },

  async saveConfig(
    id: string,
    scope: ProfileScope,
    config: ProfileConfig,
    name?: string,
  ): Promise<Profile> {
    return await invoke("update_profile_config", { id, scope, config, name });
  },

  async deployToProject(
    id: string,
    scope: ProfileScope,
    resolution: ProjectDeployResolution | null = null,
  ): Promise<ProjectDeployResult> {
    return await invoke("deploy_profile_to_project", { id, scope, resolution });
  },

  /**
   * 删除项目
   */
  async delete(id: string): Promise<void> {
    return await invoke("delete_profile", { id });
  },

  /**
   * 应用项目快照（只作用于发起页所属分组内的应用），返回 warnings
   * （best-effort，部分失败不中断）
   */
  async apply(id: string, scope: ProfileScope): Promise<string[]> {
    return await invoke("apply_profile", { id, scope });
  },

  /**
   * 不使用项目：仅清除某分组的激活标记，不改动任何配置
   */
  async clearCurrent(scope: ProfileScope): Promise<void> {
    return await invoke("clear_current_profile", { scope });
  },
};
