use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;

use crate::services::skill::SkillStore;

/// 「启用到了哪些应用」的一组开关（Claude Desktop、OpenClaw 不参与）。
///
/// MCP 和 Skill 各生成一个同样的结构体；分成两个具名类型只是为了让 serde 的
/// 报错（`expected struct McpApps`）保持原样。
macro_rules! app_flags {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
        pub struct $name {
            #[serde(default)]
            pub mcode: bool,
            #[serde(default)]
            pub kimi: bool,
            #[serde(default)]
            pub dsh: bool,
            #[serde(default)]
            pub workbuddy: bool,
            #[serde(default)]
            pub agy: bool,
            #[serde(default)]
            pub copilot: bool,
            #[serde(default)]
            pub claude: bool,
            #[serde(default)]
            pub codex: bool,
            #[serde(default)]
            pub gemini: bool,
            #[serde(default)]
            pub grokbuild: bool,
            #[serde(default)]
            pub opencode: bool,
            #[serde(default)]
            pub hermes: bool,
            #[serde(default)]
            pub pi: bool,
            #[serde(default)]
            pub omp: bool,
        }

        impl $name {
            /// [`Self::enabled_apps`] 的顺序：同步、移除都按它走，不要随意调整
            const ORDER: [AppType; 14] = [
                AppType::Claude,
                AppType::Codex,
                AppType::Gemini,
                AppType::GrokBuild,
                AppType::OpenCode,
                AppType::Agy,
                AppType::Copilot,
                AppType::Dsh,
                AppType::Kimi,
                AppType::WorkBuddy,
                AppType::Mcode,
                AppType::Hermes,
                AppType::Pi,
                AppType::Omp,
            ];

            fn slot(&mut self, app: &AppType) -> Option<&mut bool> {
                match app {
                    AppType::Claude => Some(&mut self.claude),
                    AppType::Codex => Some(&mut self.codex),
                    AppType::Gemini => Some(&mut self.gemini),
                    AppType::GrokBuild => Some(&mut self.grokbuild),
                    AppType::OpenCode => Some(&mut self.opencode),
                    AppType::Hermes => Some(&mut self.hermes),
                    AppType::Mcode => Some(&mut self.mcode),
                    AppType::Kimi => Some(&mut self.kimi),
                    AppType::Dsh => Some(&mut self.dsh),
                    AppType::Pi => Some(&mut self.pi),
                    AppType::Omp => Some(&mut self.omp),
                    AppType::Agy => Some(&mut self.agy),
                    AppType::Copilot => Some(&mut self.copilot),
                    AppType::WorkBuddy => Some(&mut self.workbuddy),
                    AppType::OpenClaw | AppType::ClaudeDesktop => None,
                }
            }

            /// 检查指定应用是否启用
            pub fn is_enabled_for(&self, app: &AppType) -> bool {
                // 只维护一张 `slot` 映射表；读取时复制 14 个 bool 换取不写第二张表
                self.clone().slot(app).is_some_and(|enabled| *enabled)
            }

            /// 设置指定应用的启用状态（Claude Desktop、OpenClaw 忽略）
            pub fn set_enabled_for(&mut self, app: &AppType, enabled: bool) {
                if let Some(slot) = self.slot(app) {
                    *slot = enabled;
                }
            }

            /// 获取所有启用的应用列表
            pub fn enabled_apps(&self) -> Vec<AppType> {
                Self::ORDER
                    .into_iter()
                    .filter(|app| self.is_enabled_for(app))
                    .collect()
            }

            /// 检查是否所有应用都未启用
            pub fn is_empty(&self) -> bool {
                self.enabled_apps().is_empty()
            }

            /// 仅启用指定应用（其他应用设为禁用）
            pub fn only(app: &AppType) -> Self {
                let mut apps = Self::default();
                apps.set_enabled_for(app, true);
                apps
            }
        }
    };
}

app_flags!(
    /// MCP 服务器应用状态（标记应用到哪些客户端）
    McpApps
);
app_flags!(
    /// Skill 应用启用状态（标记 Skill 应用到哪些客户端）
    SkillApps
);

/// 已安装的 Skill（v3.10.0+ 统一结构）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkill {
    /// 唯一标识符（格式："owner/repo:directory" 或 "local:directory"）
    pub id: String,
    /// 显示名称
    pub name: String,
    /// 描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 安装目录名（在 SSOT 目录中的子目录名）
    pub directory: String,
    /// 仓库所在主机；缺省即 `github.com`，GitLab 仓库记实例的主机名
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_host: Option<String>,
    /// 仓库所有者（GitHub 用户/组织，或 GitLab 分组路径）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_owner: Option<String>,
    /// 仓库名称
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_name: Option<String>,
    /// 仓库分支
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_branch: Option<String>,
    /// README URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readme_url: Option<String>,
    /// 应用启用状态
    pub apps: SkillApps,
    /// 安装时间（Unix 时间戳）
    pub installed_at: i64,
    /// 内容哈希（SHA-256，用于更新检测）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// 最近更新时间（Unix 时间戳，0 = 从未更新）
    #[serde(default)]
    pub updated_at: i64,
}

/// 未管理的 Skill（在应用目录中发现但未被 CC Switch 管理）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnmanagedSkill {
    /// 目录名
    pub directory: String,
    /// 显示名称（从 SKILL.md 解析）
    pub name: String,
    /// 描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 在哪些应用目录中发现（如 ["claude", "codex"]）
    pub found_in: Vec<String>,
    /// 发现路径（首个匹配的完整路径）
    pub path: String,
}

/// MCP 服务器定义（v3.7.0 统一结构）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub server: serde_json::Value,
    pub apps: McpApps,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// Shared MCP collection used by explicit Agent imports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRoot {
    pub servers: Option<HashMap<String, McpServer>>,
}

impl Default for McpRoot {
    fn default() -> Self {
        Self {
            servers: Some(HashMap::new()),
        }
    }
}

/// Prompt 配置：单客户端维度
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PromptConfig {
    #[serde(default)]
    pub prompts: HashMap<String, crate::prompt::Prompt>,
}

/// Prompt 根：按客户端分开维护
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PromptRoot {
    #[serde(default)]
    pub claude: PromptConfig,
    #[serde(rename = "claude-desktop", default)]
    pub claude_desktop: PromptConfig,
    #[serde(default)]
    pub codex: PromptConfig,
    #[serde(default)]
    pub gemini: PromptConfig,
    #[serde(default)]
    pub grokbuild: PromptConfig,
    #[serde(default)]
    pub opencode: PromptConfig,
    #[serde(default)]
    pub openclaw: PromptConfig,
    #[serde(default)]
    pub hermes: PromptConfig,
}

use crate::error::AppError;
use crate::provider::ProviderManager;

/// 应用类型。不走 serde：输出用 [`AppType::as_str`]，解析只走 [`FromStr`]（唯一的别名表）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AppType {
    Claude,
    ClaudeDesktop,
    Codex,
    Gemini,
    GrokBuild,
    OpenCode,
    OpenClaw,
    Hermes,
    Pi,
    Mcode,
    Kimi,
    Dsh,
    Agy,
    Omp,
    Copilot,
    WorkBuddy,
}

/// 一个应用支持哪些功能。每个应用都在 [`AppType::spec`] 里写全每一项，新增应用时漏写
/// 就编译不过。前端 `src/config/appSpecs.ts` 的 `APP_SPECS` 有同样的几项，
/// `tests/config/appSpecs.json` 由这里的测试生成，前端测试拿它比对两边。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSpec {
    /// 序列化、命令参数和前端用的应用 ID
    pub id: &'static str,
    /// 累加式：供应商并存在一个 live 文件里，可以各自启用。
    ///
    /// GitHub Copilot CLI 是混合的：自带模型卡累加式写进 `providers.json`，官方登录卡仍是
    /// 启动式（只打开终端、不写文件，启动时照旧从 settings.json 导入）。
    pub additive: bool,
    /// 有完整的本地路由和故障转移（`proxy_config` 行）
    pub local_proxy: bool,
    /// 有设备级的直连 / 路由模式（`live-state.json`，`mode::controller`）。Claude Desktop 经
    /// 同一个路由服务转发，但没有 `proxy_config` 行、不做故障转移，所以只有这一项。
    pub route_mode: bool,
    /// 支持 Stack 模式（`mode::stack`）
    pub stack: bool,
}

impl AppType {
    /// 这个应用的功能表。不要加 `_ =>` 分支：新增应用必须在这里写全每一项。
    pub const fn spec(&self) -> AppSpec {
        match self {
            AppType::Claude => AppSpec {
                id: "claude",
                additive: false,
                local_proxy: true,
                route_mode: true,
                stack: true,
            },
            AppType::ClaudeDesktop => AppSpec {
                id: "claude-desktop",
                additive: false,
                local_proxy: false,
                route_mode: true,
                stack: true,
            },
            AppType::Codex => AppSpec {
                id: "codex",
                additive: false,
                local_proxy: true,
                route_mode: true,
                stack: true,
            },
            AppType::Gemini => AppSpec {
                id: "gemini",
                additive: false,
                local_proxy: true,
                route_mode: true,
                stack: false,
            },
            AppType::GrokBuild => AppSpec {
                id: "grokbuild",
                additive: false,
                local_proxy: true,
                route_mode: true,
                stack: false,
            },
            AppType::OpenCode => AppSpec {
                id: "opencode",
                additive: true,
                local_proxy: false,
                route_mode: false,
                stack: false,
            },
            AppType::OpenClaw => AppSpec {
                id: "openclaw",
                additive: true,
                local_proxy: false,
                route_mode: false,
                stack: false,
            },
            AppType::Hermes => AppSpec {
                id: "hermes",
                additive: true,
                local_proxy: false,
                route_mode: false,
                stack: false,
            },
            AppType::Pi => AppSpec {
                id: "pi",
                additive: true,
                local_proxy: false,
                route_mode: false,
                stack: false,
            },
            AppType::Mcode => AppSpec {
                id: "mcode",
                additive: true,
                local_proxy: false,
                route_mode: false,
                stack: false,
            },
            AppType::Kimi => AppSpec {
                id: "kimi",
                additive: true,
                local_proxy: false,
                route_mode: false,
                stack: false,
            },
            AppType::Dsh => AppSpec {
                id: "dsh",
                additive: true,
                local_proxy: false,
                route_mode: false,
                stack: false,
            },
            AppType::Agy => AppSpec {
                id: "agy",
                additive: false,
                local_proxy: false,
                route_mode: false,
                stack: false,
            },
            AppType::Omp => AppSpec {
                id: "omp",
                additive: true,
                local_proxy: false,
                route_mode: false,
                stack: false,
            },
            AppType::Copilot => AppSpec {
                id: "copilot",
                additive: true,
                local_proxy: false,
                route_mode: false,
                stack: false,
            },
            AppType::WorkBuddy => AppSpec {
                id: "workbuddy",
                additive: true,
                local_proxy: false,
                route_mode: false,
                stack: false,
            },
        }
    }

    pub fn as_str(&self) -> &str {
        self.spec().id
    }

    /// 所有应用，按默认顺序
    pub const ALL: [AppType; 16] = [
        AppType::Claude,
        AppType::ClaudeDesktop,
        AppType::Codex,
        AppType::Gemini,
        AppType::GrokBuild,
        AppType::OpenCode,
        AppType::OpenClaw,
        AppType::Hermes,
        AppType::Pi,
        AppType::Mcode,
        AppType::Kimi,
        AppType::Dsh,
        AppType::Agy,
        AppType::Omp,
        AppType::Copilot,
        AppType::WorkBuddy,
    ];

    pub fn all() -> impl Iterator<Item = AppType> {
        Self::ALL.into_iter()
    }
}

impl std::fmt::Display for AppType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AppType {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_lowercase();
        match normalized.as_str() {
            "claude" => Ok(AppType::Claude),
            "claude-desktop" | "claude_desktop" | "claudedesktop" => Ok(AppType::ClaudeDesktop),
            "codex" => Ok(AppType::Codex),
            "gemini" => Ok(AppType::Gemini),
            "grokbuild" | "grok-build" | "grok_build" | "grok" => Ok(AppType::GrokBuild),
            "opencode" => Ok(AppType::OpenCode),
            "openclaw" => Ok(AppType::OpenClaw),
            "hermes" => Ok(AppType::Hermes),
            "pi" => Ok(AppType::Pi),
            "mcode" => Ok(AppType::Mcode),
            "kimi" | "kimi-code" => Ok(AppType::Kimi),
            "dsh" | "deepseek-harness" => Ok(AppType::Dsh),
            "agy" | "antigravity" | "antigravity-cli" => Ok(AppType::Agy),
            "omp" | "oh-my-pi" | "oh_my_pi" | "ohmypi" => Ok(AppType::Omp),
            "copilot" | "copilot-cli" | "github-copilot" | "github-copilot-cli" => {
                Ok(AppType::Copilot)
            }
            "workbuddy" | "work-buddy" | "work_buddy" => Ok(AppType::WorkBuddy),
            other => Err(AppError::localized(
                "unsupported_app",
                format!("不支持的应用标识: '{other}'。可选值: claude, claude-desktop, codex, gemini, grokbuild, opencode, openclaw, hermes, pi, mcode, kimi, dsh, agy, omp, copilot, workbuddy。"),
                format!("Unsupported app id: '{other}'. Allowed: claude, claude-desktop, codex, gemini, grokbuild, opencode, openclaw, hermes, pi, mcode, kimi, dsh, agy, omp, copilot, workbuddy."),
            )),
        }
    }
}

/// 通用配置片段（按应用分治）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CommonConfigSnippets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openclaw: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hermes: Option<String>,
}

/// 显式导入使用的多应用配置容器。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiAppConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    /// 应用管理器（claude/codex）
    #[serde(flatten)]
    pub apps: HashMap<String, ProviderManager>,
    /// MCP 配置（按客户端分治）
    #[serde(default)]
    pub mcp: McpRoot,
    /// Prompt 配置（按客户端分治）
    #[serde(default)]
    pub prompts: PromptRoot,
    /// Claude Skills 配置
    #[serde(default)]
    pub skills: SkillStore,
    /// 通用配置片段（按应用分治）
    #[serde(default)]
    pub common_config_snippets: CommonConfigSnippets,
}

fn default_version() -> u32 {
    2
}

impl Default for MultiAppConfig {
    fn default() -> Self {
        let mut apps = HashMap::new();
        apps.insert("claude".to_string(), ProviderManager::default());
        apps.insert("claude-desktop".to_string(), ProviderManager::default());
        apps.insert("codex".to_string(), ProviderManager::default());
        apps.insert("gemini".to_string(), ProviderManager::default());
        apps.insert("grokbuild".to_string(), ProviderManager::default());
        apps.insert("opencode".to_string(), ProviderManager::default());
        apps.insert("openclaw".to_string(), ProviderManager::default());
        apps.insert("hermes".to_string(), ProviderManager::default());
        apps.insert("agy".to_string(), ProviderManager::default());
        apps.insert("copilot".to_string(), ProviderManager::default());

        Self {
            version: 2,
            apps,
            mcp: McpRoot::default(),
            prompts: PromptRoot::default(),
            skills: SkillStore::default(),
            common_config_snippets: CommonConfigSnippets::default(),
        }
    }
}

impl MultiAppConfig {
    /// 获取指定应用的管理器
    pub fn get_manager(&self, app: &AppType) -> Option<&ProviderManager> {
        self.apps.get(app.as_str())
    }

    /// 获取指定应用的管理器（可变引用）
    pub fn get_manager_mut(&mut self, app: &AppType) -> Option<&mut ProviderManager> {
        self.apps.get_mut(app.as_str())
    }

    /// 确保应用存在
    pub fn ensure_app(&mut self, app: &AppType) {
        if !self.apps.contains_key(app.as_str()) {
            self.apps
                .insert(app.as_str().to_string(), ProviderManager::default());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_type_parses_claude_desktop_aliases() {
        assert_eq!(
            "claude-desktop".parse::<AppType>().unwrap(),
            AppType::ClaudeDesktop
        );
        assert_eq!(
            "claude_desktop".parse::<AppType>().unwrap(),
            AppType::ClaudeDesktop
        );
        assert_eq!(
            "claudeDesktop".parse::<AppType>().unwrap(),
            AppType::ClaudeDesktop
        );
        assert_eq!(AppType::ClaudeDesktop.as_str(), "claude-desktop");
    }

    /// 前后端共用的应用功能表：`tests/config/appSpecs.json` 从这里生成，前端
    /// `tests/config/appSpecs.test.ts` 拿它比对 `APP_SPECS`。改了 [`AppType::spec`] 后用
    /// `CC_SWITCH_UPDATE_GOLDEN=1 cargo test app_specs_match_shared_fixture` 重新生成。
    #[test]
    fn app_specs_match_shared_fixture() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/config/appSpecs.json");
        let specs: Vec<AppSpec> = AppType::all().map(|app| app.spec()).collect();
        let expected = serde_json::to_string_pretty(&specs).unwrap() + "\n";
        if std::env::var_os("CC_SWITCH_UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &expected).unwrap();
        }
        let actual = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            actual, expected,
            "tests/config/appSpecs.json 和 AppType::spec 不一致；确认改动后用 CC_SWITCH_UPDATE_GOLDEN=1 重新生成，再同步前端 APP_SPECS"
        );
        for app in AppType::all() {
            assert_eq!(app.as_str().parse::<AppType>().unwrap(), app);
        }
    }

    #[test]
    fn app_type_parses_omp_aliases() {
        assert_eq!("omp".parse::<AppType>().unwrap(), AppType::Omp);
        assert_eq!("oh-my-pi".parse::<AppType>().unwrap(), AppType::Omp);
        assert_eq!("oh_my_pi".parse::<AppType>().unwrap(), AppType::Omp);
        assert_eq!("ohmypi".parse::<AppType>().unwrap(), AppType::Omp);
        assert_eq!("OMP".parse::<AppType>().unwrap(), AppType::Omp);
        assert_eq!(AppType::Omp.as_str(), "omp");
    }
}
