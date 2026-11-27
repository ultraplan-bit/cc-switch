import type { AppId } from "@/lib/api/types";
import type { Profile, ProfileScope } from "@/lib/api/profiles";

/**
 * 应用页 → 配置方案所属的 Agent（后端 ProfileScope::for_app 的前端镜像，两处同步）
 *
 * 不在映射里的应用不支持配置方案，其标签页不渲染方案页。
 */
export const APP_PROFILE_SCOPE: Partial<Record<AppId, ProfileScope>> = {
  claude: "claude",
  "claude-desktop": "claude-desktop",
  codex: "codex",
};

/** 路径最后一段，用作项目的显示名；完整路径另行显示 */
export function projectName(path: string) {
  return (
    path
      .replace(/[\\/]+$/, "")
      .split(/[\\/]/)
      .at(-1) || path
  );
}

/**
 * 方案是否包含某个 Agent 的配置（任一槽位非 null 即视为包含）。
 * 方案页只列出包含当前 Agent 配置的方案。与后端 scope_captured 保持一致。
 */
export function hasScopeSnapshot(profile: Profile, scope: ProfileScope) {
  const {
    providers,
    models,
    modelSettings,
    modelCatalog,
    nativeConfig,
    mcp,
    skills,
    prompts,
  } = profile.payload;
  return (
    providers[scope] !== null ||
    models?.[scope] != null ||
    modelSettings?.[scope] != null ||
    modelCatalog?.[scope] != null ||
    nativeConfig?.[scope] != null ||
    mcp[scope] !== null ||
    skills[scope] !== null ||
    prompts[scope] !== null
  );
}

/** 方案不管理 MCP、Skills 和指令：应用或部署时保持所在位置的现状 */
export function followsGlobalResources(profile: Profile, scope: ProfileScope) {
  const { mcp, skills, prompts } = profile.payload;
  return (
    mcp[scope] === null && skills[scope] === null && prompts[scope] === null
  );
}
