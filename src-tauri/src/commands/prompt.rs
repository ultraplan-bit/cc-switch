use crate::error::AppError;
use indexmap::IndexMap;
use std::str::FromStr;

use tauri::State;

use crate::app_config::AppType;
use crate::prompt::Prompt;
use crate::services::pi_prompt_files::{
    PiPromptFileKind, PiPromptFileService, PiPromptFileSnapshot, PiPromptTemplate,
    PiPromptTemplateService,
};
use crate::services::prompt::{PromptService, PromptTargetSnapshot};
use crate::store::AppState;

#[tauri::command]
pub async fn get_prompts(state: State<'_, AppState>) -> Result<IndexMap<String, Prompt>, AppError> {
    PromptService::get_prompts(&state)
}

#[tauri::command]
pub async fn upsert_prompt(
    id: String,
    prompt: Prompt,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    PromptService::upsert_prompt(&state, &id, prompt)
}

#[tauri::command]
pub async fn delete_prompt(id: String, state: State<'_, AppState>) -> Result<(), AppError> {
    PromptService::delete_prompt(&state, &id)
}

#[tauri::command]
pub async fn apply_prompt(
    app: String,
    id: String,
    expected_revision: String,
    expected_content: String,
    state: State<'_, AppState>,
) -> Result<PromptTargetSnapshot, AppError> {
    let app_type = AppType::from_str(&app)?;
    PromptService::apply_prompt(&state, app_type, &id, &expected_revision, &expected_content)
}

#[tauri::command]
pub async fn get_prompt_target(
    app: String,
    state: State<'_, AppState>,
) -> Result<PromptTargetSnapshot, AppError> {
    PromptService::get_target(&state, AppType::from_str(&app)?)
}

#[tauri::command]
pub async fn remove_prompt_application(
    app: String,
    expected_revision: String,
    state: State<'_, AppState>,
) -> Result<PromptTargetSnapshot, AppError> {
    PromptService::remove_prompt_application(&state, AppType::from_str(&app)?, &expected_revision)
}

#[tauri::command]
pub async fn import_prompt_from_file(
    app: String,
    state: State<'_, AppState>,
) -> Result<String, AppError> {
    let app_type = AppType::from_str(&app)?;
    PromptService::import_from_file(&state, app_type)
}

#[tauri::command]
pub async fn get_pi_prompt_file(kind: PiPromptFileKind) -> Result<PiPromptFileSnapshot, String> {
    PiPromptFileService::read(kind).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn replace_pi_prompt_file(
    kind: PiPromptFileKind,
    #[allow(non_snake_case)] expectedRevision: String,
    content: String,
) -> Result<PiPromptFileSnapshot, String> {
    PiPromptFileService::replace(kind, &expectedRevision, &content)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_pi_prompt_file(
    kind: PiPromptFileKind,
    #[allow(non_snake_case)] expectedRevision: String,
) -> Result<bool, String> {
    PiPromptFileService::delete(kind, &expectedRevision).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_pi_prompt_templates() -> Result<Vec<PiPromptTemplate>, String> {
    PiPromptTemplateService::list().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn upsert_pi_prompt_template(
    slug: String,
    #[allow(non_snake_case)] originalSlug: Option<String>,
    #[allow(non_snake_case)] expectedRevision: String,
    content: String,
) -> Result<PiPromptTemplate, String> {
    PiPromptTemplateService::upsert(&slug, originalSlug.as_deref(), &expectedRevision, &content)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_pi_prompt_template(
    slug: String,
    #[allow(non_snake_case)] expectedRevision: String,
) -> Result<bool, String> {
    PiPromptTemplateService::delete(&slug, &expectedRevision).map_err(|error| error.to_string())
}
