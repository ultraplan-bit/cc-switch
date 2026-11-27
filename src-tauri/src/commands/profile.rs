//! 配置方案（Profile）管理命令
//!
//! 方案是某个 Agent 的一份配置；它可以应用到全局，也可以部署到任意已登记的项目目录。

use crate::app_config::AppType;
use crate::error::AppError;
use serde::Serialize;
use tauri::{Emitter, State};

use super::run_db_task;
use crate::database::Profile;
use crate::services::profile::{
    ProfileConfig, ProfilePayload, ProfileScope, ProfileService, ProfileSource,
};
use crate::services::project_profile::{
    ConflictResolution, ProjectDeployResult, ProjectProfileService, ProjectStatus,
};
use crate::store::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDto {
    pub id: String,
    pub name: String,
    pub payload: ProfilePayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

impl From<Profile> for ProfileDto {
    fn from(profile: Profile) -> Self {
        // 单条 payload 损坏不应拖垮整个列表：降级为默认值并记日志
        let payload = serde_json::from_str(&profile.payload).unwrap_or_else(|e| {
            log::warn!(
                "解析 profile '{}' payload 失败，使用默认值: {e}",
                profile.id
            );
            ProfilePayload::default()
        });
        Self {
            id: profile.id,
            name: profile.name,
            payload,
            created_at: profile.created_at,
            updated_at: profile.updated_at,
        }
    }
}

/// 按 Agent 分组的响应容器。JSON key 是 camelCase（`claudeDesktop`），
/// 与命令参数里的分组字符串（`claude-desktop`）不同。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerScope<T> {
    pub claude: T,
    pub claude_desktop: T,
    pub codex: T,
}

/// 某个 Agent 当前使用的方案。`stale` 表示方案在应用后被修改，或应用时有警告：
/// 全局配置与它不完全一致，需要重新应用。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentProfile {
    pub id: String,
    pub stale: bool,
}

/// 某个 Agent 实际生效的全局状态（读自真实配置，而不是方案标记）。
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalState {
    pub provider_id: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfilesResponse {
    pub profiles: Vec<ProfileDto>,
    pub current: PerScope<Option<CurrentProfile>>,
    pub global_state: PerScope<Option<GlobalState>>,
    pub projects: Vec<ProjectStatus>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileMigrationResult {
    profiles: Vec<ProfileDto>,
    warnings: Vec<String>,
    ready: bool,
}

#[tauri::command]
pub async fn migrate_legacy_agent_profiles(
    state: State<'_, AppState>,
    scope: String,
) -> Result<ProfileMigrationResult, String> {
    let scope = ProfileScope::parse(&scope)?;
    profile_mutation(&state, move |state| {
        ProfileService::migrate_legacy_agent_profiles(state, scope).map(|(profiles, warnings)| {
            ProfileMigrationResult {
                profiles: profiles.into_iter().map(ProfileDto::from).collect(),
                ready: warnings.is_empty(),
                warnings,
            }
        })
    })
    .await
}

/// Profile 应用完成后的统一收尾：发事件 + 重建托盘菜单
///
/// 只对项目所属分组内的应用发 provider-switched。UI 与托盘两个入口必须
/// 共用此函数，保证事件 payload 形状一致（前端 App.tsx 的
/// provider-switched 监听依赖该形状）。
pub fn emit_profile_apply_events(
    app: &tauri::AppHandle,
    state: &AppState,
    profile_id: &str,
    scope: ProfileScope,
) {
    for app_type in scope.apps().iter() {
        let (_, auto_failover_enabled) = state.db.get_proxy_flags_sync(app_type);
        let proxy_enabled = crate::mode::current::is_proxy(app_type);
        let provider_id = crate::mode::current::provider_for(
            &state.db,
            app_type,
            crate::mode::current::Purpose::InUse,
        )
        .ok()
        .flatten()
        .unwrap_or_default();
        let event_data = serde_json::json!({
            "appType": app_type.as_str(),
            "proxyEnabled": proxy_enabled,
            "autoFailoverEnabled": auto_failover_enabled,
            "providerId": provider_id,
        });
        if let Err(e) = app.emit("provider-switched", event_data) {
            log::error!("发射 provider-switched 事件失败: {e}");
        }
    }
    if let Err(e) = app.emit(
        "profile-applied",
        serde_json::json!({ "profileId": profile_id, "scope": scope.as_str() }),
    ) {
        log::error!("发射 profile-applied 事件失败: {e}");
    }
    crate::tray::refresh_tray_menu(app);
}

#[tauri::command]
pub async fn list_profiles(state: State<'_, AppState>) -> Result<ProfilesResponse, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || list_profiles_blocking(&state))
        .await
        .map_err(|e| format!("读取项目列表任务失败: {e}"))?
}

fn current_profile(
    state: &AppState,
    scope: ProfileScope,
) -> Result<Option<CurrentProfile>, String> {
    let Some(id) = state.db.get_current_profile_id(scope.as_str())? else {
        return Ok(None);
    };
    Ok(Some(CurrentProfile {
        id,
        stale: state.db.is_current_profile_stale(scope.as_str())?,
    }))
}

/// Read failures must not hide the Profile list; the affected value is just unknown.
fn global_state(state: &AppState, scope: ProfileScope) -> Option<GlobalState> {
    let app = &scope.apps()[0];
    let provider_id =
        crate::mode::current::provider_for(&state.db, app, crate::mode::current::Purpose::InUse)
            .unwrap_or_else(|error| {
                log::warn!("读取 {} 当前供应商失败: {error}", app.as_str());
                None
            });
    let model = if app == &AppType::ClaudeDesktop {
        None
    } else {
        ProfileService::current_model(app).unwrap_or_else(|error| {
            log::warn!("读取 {} 当前模型失败: {error}", app.as_str());
            None
        })
    };
    (provider_id.is_some() || model.is_some()).then_some(GlobalState { provider_id, model })
}

fn list_profiles_blocking(state: &AppState) -> Result<ProfilesResponse, String> {
    // Seeding is serialized by the database transaction and guarded by its own marker.
    ProfileService::seed_project_registry(state)?;
    let profiles = ProfileService::list(state)?;
    let payloads: std::collections::HashMap<&str, &str> = profiles
        .iter()
        .map(|profile| (profile.id.as_str(), profile.payload.as_str()))
        .collect();
    let projects = state
        .db
        .get_profile_projects()?
        .iter()
        .map(|path| {
            ProjectProfileService::project_status_with(path, |id| {
                payloads.get(id).map(|payload| payload.to_string())
            })
        })
        .collect();
    Ok(ProfilesResponse {
        current: PerScope {
            claude: current_profile(state, ProfileScope::Claude)?,
            claude_desktop: current_profile(state, ProfileScope::ClaudeDesktop)?,
            codex: current_profile(state, ProfileScope::Codex)?,
        },
        global_state: PerScope {
            claude: global_state(state, ProfileScope::Claude),
            claude_desktop: global_state(state, ProfileScope::ClaudeDesktop),
            codex: global_state(state, ProfileScope::Codex),
        },
        projects,
        profiles: profiles.into_iter().map(ProfileDto::from).collect(),
    })
}

/// 项目的写入口（这里的命令和托盘的两项）互相串行：主线程不再替它们排队。`apply` 收尾才写
/// 当前项目指针、自动保存会整行回写旧项目，期间的清除 / 重命名 / 删除会被盖掉；新建的快照会
/// 拍到切到一半的状态。
pub(crate) static PROFILE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn profile_mutation<T, E, F>(state: &State<'_, AppState>, task: F) -> Result<T, String>
where
    T: Send + 'static,
    E: Into<String> + Send + 'static,
    F: FnOnce(&AppState) -> Result<T, E> + Send + 'static,
{
    let _serial = PROFILE_LOCK.lock().await;
    run_db_task(state, task).await
}

#[tauri::command]
pub async fn create_profile(
    state: State<'_, AppState>,
    name: String,
    scope: String,
    source: Option<ProfileSource>,
) -> Result<ProfileDto, String> {
    let scope = ProfileScope::parse(&scope)?;
    profile_mutation(&state, move |state| {
        ProfileService::create_from(state, &name, scope, source.unwrap_or_default())
            .map(ProfileDto::from)
    })
    .await
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectImportResult {
    profile: ProfileDto,
    warnings: Vec<String>,
}

#[tauri::command]
pub async fn import_project_profile(
    state: State<'_, AppState>,
    name: String,
    scope: String,
    project_path: String,
) -> Result<ProjectImportResult, String> {
    let scope = ProfileScope::parse(&scope)?;
    profile_mutation(&state, move |state| {
        ProjectProfileService::import_from_project(state, &name, scope, &project_path).map(
            |(profile, warnings)| ProjectImportResult {
                profile: profile.into(),
                warnings,
            },
        )
    })
    .await
}

#[tauri::command]
pub async fn duplicate_profile(
    state: State<'_, AppState>,
    source_id: String,
    name: String,
    scope: String,
) -> Result<ProfileDto, String> {
    let scope = ProfileScope::parse(&scope)?;
    profile_mutation(&state, move |state| {
        ProfileService::duplicate(state, &source_id, &name, scope).map(ProfileDto::from)
    })
    .await
}

#[tauri::command]
pub async fn update_profile(
    state: State<'_, AppState>,
    id: String,
    name: Option<String>,
    resnapshot: Option<bool>,
    scope: Option<String>,
) -> Result<ProfileDto, String> {
    let scope = scope.map(|s| ProfileScope::parse(&s)).transpose()?;
    profile_mutation(&state, move |state| {
        ProfileService::update(state, &id, name, resnapshot.unwrap_or(false), scope)
            .map(ProfileDto::from)
    })
    .await
}

/// 登记一个项目目录，之后任意方案都可以部署到它。
#[tauri::command]
pub async fn add_profile_project(state: State<'_, AppState>, path: String) -> Result<(), String> {
    profile_mutation(&state, move |state| {
        ProfileService::add_project(state, &path)
    })
    .await
}

/// 从列表移除一个项目目录；不改动目录里的任何文件。
#[tauri::command]
pub async fn remove_profile_project(
    state: State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    profile_mutation(&state, move |state| {
        ProfileService::remove_project(state, &path)
    })
    .await
}

#[tauri::command]
pub async fn update_profile_config(
    state: State<'_, AppState>,
    id: String,
    scope: String,
    config: ProfileConfig,
    name: Option<String>,
) -> Result<ProfileDto, String> {
    let scope = ProfileScope::parse(&scope)?;
    profile_mutation(&state, move |state| {
        ProfileService::update_config(state, &id, scope, config, name).map(ProfileDto::from)
    })
    .await
}

#[tauri::command]
pub async fn delete_profile(
    state: State<'_, AppState>,
    id: String,
    scope: String,
) -> Result<(), String> {
    let scope = ProfileScope::parse(&scope)?;
    profile_mutation(&state, move |state| {
        ProfileService::delete(state, &id, scope)
    })
    .await
}

#[tauri::command]
pub async fn clear_current_profile(
    state: State<'_, AppState>,
    scope: String,
) -> Result<(), String> {
    let scope = ProfileScope::parse(&scope)?;
    profile_mutation(&state, move |state| {
        state.db.set_current_profile_id(scope.as_str(), None)
    })
    .await
}

/// 应用项目快照（只作用于发起页所属分组内的应用）。
///
/// `ProviderService::switch` 用 block_on 拿切换锁，整段只能在阻塞线程池里跑
/// （同托盘的「应用项目」入口）。
#[tauri::command]
pub async fn apply_profile(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    scope: String,
) -> Result<Vec<String>, String> {
    let scope = ProfileScope::parse(&scope)?;
    profile_mutation(&state, move |state| {
        let warnings = ProfileService::apply(state, &id, scope)?;
        emit_profile_apply_events(&app, state, &id, scope);
        Ok::<_, AppError>(warnings)
    })
    .await
}

/// 将方案部署到指定的 Claude Code 或 Codex 项目目录。
/// 与全局 apply 分开，避免更改用户级配置和 enabled 状态。
#[tauri::command]
pub async fn deploy_profile_to_project(
    state: State<'_, AppState>,
    id: String,
    scope: String,
    project_path: String,
    resolution: Option<ConflictResolution>,
) -> Result<ProjectDeployResult, String> {
    let scope = ProfileScope::parse(&scope)?;
    profile_mutation(&state, move |state| {
        ProjectProfileService::deploy(state, &id, scope, &project_path, resolution)
            .map_err(|err| err.to_string())
    })
    .await
}
