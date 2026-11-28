import { invoke } from "@tauri-apps/api/core";

/**
 * 支持配置方案的 Agent（与后端 services/profile.rs 的 ProfileScope 严格对应）
 *
 * 一份方案在界面上只属于一个 Agent；应用、部署和「生效中」记录都按 Agent 进行。
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
 * 配置方案的内容（与后端 ProfilePayload 严格对应）
 *
 * 槽位为 null = 方案不管这一项（应用和部署时都不动），与"空集"
 * （空数组，应用时清空启用）严格区分。
 */
export interface ProfilePayload {
  /** Claude Code project deployment only; credentials remain in the provider library. */
  claudeProjectConnection?: "inherit" | "direct";
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
  /** Only Claude Code can override its connection when deployed to a project. */
  claudeProjectConnection?: "inherit" | "direct";
  providerId: string | null;
  model: string;
  modelSettings: Record<string, string>;
  modelCatalog: Record<string, unknown> | null;
  nativeConfig?: string | null;
  /** The profile leaves MCP, Skills and instructions to the location it is used in. */
  followGlobalResources: boolean;
  mcpIds: string[];
  skillIds: string[];
  promptId: string | null;
}

/** One Agent's configuration. Where it is used (globally or in a project) is not part of it. */
export interface Profile {
  id: string;
  name: string;
  payload: ProfilePayload;
  createdAt?: number;
  updatedAt?: number;
}

/** `stale`: the profile was edited, or applied with notes, since it was last written. */
export interface ProfileUse {
  id: string;
  stale: boolean;
}

export type ProjectScope = "claude" | "codex";

export interface ProfileProject {
  path: string;
  /** false when the directory can no longer be read */
  available: boolean;
  deployed: Partial<
    Record<ProjectScope, { profileId: string; stale: boolean }>
  >;
}

export type ProfileCreateSource = "blank" | "current";

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

/**
 * 按 Agent 分组的响应字段。
 *
 * 注意：JSON key 是 camelCase（claudeDesktop），与 ProfileScope 的 kebab-case
 * 字符串不同——后者用于命令参数，前者用于响应字段。
 */
export interface PerScope<T> {
  claude: T;
  claudeDesktop: T;
  codex: T;
}

export const SCOPE_RESPONSE_KEY: Record<ProfileScope, keyof PerScope<unknown>> =
  {
    claude: "claude",
    "claude-desktop": "claudeDesktop",
    codex: "codex",
  };

/** What the Agent's global files use right now, read from the files themselves. */
export interface GlobalAgentState {
  providerId: string | null;
  model: string | null;
}

export interface ProfilesResponse {
  profiles: Profile[];
  /** The profile last applied globally for each Agent. */
  current: PerScope<ProfileUse | null>;
  globalState: PerScope<GlobalAgentState | null>;
  projects: ProfileProject[];
}

export const profilesApi = {
  /**
   * 读取方案、各 Agent 的全局状态以及已登记项目的部署状态
   */
  async list(): Promise<ProfilesResponse> {
    return await invoke("list_profiles");
  },

  /**
   * 新建配置方案：blank = 空白（沿用当前连接），current = 保存该 Agent 当前的全局配置
   */
  async create(
    name: string,
    scope: ProfileScope,
    source: ProfileCreateSource,
  ): Promise<Profile> {
    return await invoke("create_profile", { name, scope, source });
  },

  /** 复制某个 Agent 的配置为一份独立的方案 */
  async duplicate(
    sourceId: string,
    name: string,
    scope: ProfileScope,
  ): Promise<Profile> {
    return await invoke("duplicate_profile", { sourceId, name, scope });
  },

  /** 读取项目目录里已有的配置，保存为方案；不修改项目文件 */
  async importFromProject(
    name: string,
    scope: ProfileScope,
    projectPath: string,
  ): Promise<{ profile: Profile; warnings: string[] }> {
    return await invoke("import_project_profile", { name, scope, projectPath });
  },

  /**
   * 重命名和/或用该 Agent 当前的全局配置覆盖方案（resnapshot 只覆盖 scope 的配置）
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

  /** 把目录加入项目列表；不读写目录里的文件 */
  async addProject(path: string): Promise<void> {
    await invoke("add_profile_project", { path });
  },

  /** 从项目列表移除目录；已部署的文件保持原样 */
  async removeProject(path: string): Promise<void> {
    await invoke("remove_profile_project", { path });
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
    projectPath: string,
    resolution: ProjectDeployResolution | null = null,
  ): Promise<ProjectDeployResult> {
    return await invoke("deploy_profile_to_project", {
      id,
      scope,
      projectPath,
      resolution,
    });
  },

  /**
   * 删除方案中某个 Agent 的配置；已写入全局或项目的文件保持原样
   */
  async delete(id: string, scope: ProfileScope): Promise<void> {
    return await invoke("delete_profile", { id, scope });
  },

  /**
   * 把方案应用到该 Agent 的全局配置，返回 warnings（best-effort，部分失败不中断）
   */
  async apply(id: string, scope: ProfileScope): Promise<string[]> {
    return await invoke("apply_profile", { id, scope });
  },
};
