//! Shared instruction documents and explicit Agent distribution.
use indexmap::IndexMap;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::app_config::AppType;
use crate::config::{resolve_symlink_target, write_text_file};
use crate::error::AppError;
use crate::prompt::{Prompt, PromptApplication};
use crate::prompt_files::{display_path, prompt_file_path, validate_prompt_content};
use crate::services::pi_prompt_files::PiAgentsFileGuard;
use crate::store::AppState;

static PROMPT_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptTargetSnapshot {
    pub path: String,
    pub display_path: String,
    pub content: Option<String>,
    pub revision: String,
    pub applied_prompt_id: Option<String>,
    pub managed: bool,
    pub matches_library: bool,
}

pub struct PromptService;

impl PromptService {
    /// Reads and edits of library definitions never project or backfill files.
    pub fn get_prompts(state: &AppState) -> Result<IndexMap<String, Prompt>, AppError> {
        state.db.get_prompts()
    }

    pub fn upsert_prompt(state: &AppState, id: &str, prompt: Prompt) -> Result<(), AppError> {
        let _guard = crate::sync_utils::lock_file_operation(&PROMPT_LOCK);
        if id.trim().is_empty() || prompt.id != id || prompt.name.trim().is_empty() {
            return Err(AppError::InvalidInput(
                "Prompt ID must match the requested ID and its name must not be empty".into(),
            ));
        }
        state.db.save_prompt(&prompt)
    }

    /// Deployed contents and ownership survive library deletion.
    pub fn delete_prompt(state: &AppState, id: &str) -> Result<(), AppError> {
        let _guard = crate::sync_utils::lock_file_operation(&PROMPT_LOCK);
        state.db.delete_prompt(id)
    }

    pub fn get_target(state: &AppState, app: AppType) -> Result<PromptTargetSnapshot, AppError> {
        let _guard = crate::sync_utils::lock_file_operation(&PROMPT_LOCK);
        PromptTarget::open(&app)?.snapshot(state, &app)
    }

    pub fn apply_prompt(
        state: &AppState,
        app: AppType,
        id: &str,
        expected_revision: &str,
        expected_content: &str,
    ) -> Result<PromptTargetSnapshot, AppError> {
        let _guard = crate::sync_utils::lock_file_operation(&PROMPT_LOCK);
        let prompts = state.db.get_prompts()?;
        let prompt = prompts
            .get(id)
            .filter(|prompt| prompt.content == expected_content);
        let Some(prompt) = prompt else {
            return Err(AppError::Conflict(
                "The library document changed after preview; reopen distribution before applying"
                    .into(),
            ));
        };
        let target = PromptTarget::open(&app)?;
        apply_to_target(state, &app, prompt, expected_revision, &target)?;
        target.snapshot(state, &app)
    }

    /// A Profile/deep link cannot silently replace unowned or externally edited
    /// instructions. The library UI can preview and explicitly replace them.
    pub fn enable_prompt(state: &AppState, app: AppType, id: &str) -> Result<(), AppError> {
        let _guard = crate::sync_utils::lock_file_operation(&PROMPT_LOCK);
        let target = PromptTarget::open(&app)?;
        let snapshot = target.snapshot(state, &app)?;
        let prompts = state.db.get_prompts()?;
        let prompt = prompts
            .get(id)
            .ok_or_else(|| AppError::InvalidInput(format!("Prompt not found: {id}")))?;
        if !snapshot.managed && snapshot.content.as_ref() == Some(&prompt.content) {
            // A matching handwritten file needs no write. Only the explicit
            // library distribution flow may claim it for later removal.
            return Ok(());
        }
        if !snapshot.managed
            && snapshot
                .content
                .as_ref()
                .is_some_and(|content| !content.is_empty() && content != &prompt.content)
        {
            return Err(AppError::Conflict(format!(
                "{} contains unowned or externally edited instructions; review and apply it from the prompt library",
                snapshot.display_path
            )));
        }
        apply_to_target(state, &app, prompt, &snapshot.revision, &target)
    }

    pub fn remove_prompt_application(
        state: &AppState,
        app: AppType,
        expected_revision: &str,
    ) -> Result<PromptTargetSnapshot, AppError> {
        let _guard = crate::sync_utils::lock_file_operation(&PROMPT_LOCK);
        let target = PromptTarget::open(&app)?;
        remove_from_target(state, &app, expected_revision, &target)?;
        target.snapshot(state, &app)
    }

    pub fn disable_prompt(state: &AppState, app: AppType) -> Result<(), AppError> {
        let _guard = crate::sync_utils::lock_file_operation(&PROMPT_LOCK);
        let target = PromptTarget::open(&app)?;
        let snapshot = target.snapshot(state, &app)?;
        if snapshot.applied_prompt_id.is_none() {
            return Ok(());
        }
        remove_from_target(state, &app, &snapshot.revision, &target)
    }

    /// Explicit import only; identical documents share an ID without claiming
    /// ownership of the source file.
    pub fn import_from_file(state: &AppState, app: AppType) -> Result<String, AppError> {
        let _guard = crate::sync_utils::lock_file_operation(&PROMPT_LOCK);
        let target = PromptTarget::open(&app)?;
        let content = target
            .read()?
            .ok_or_else(|| AppError::InvalidInput("Instruction file does not exist".into()))?;
        if content.trim().is_empty() {
            return Err(AppError::InvalidInput("Instruction file is empty".into()));
        }
        if let Some(prompt) = state
            .db
            .get_prompts()?
            .values()
            .find(|p| p.content == content)
        {
            return Ok(prompt.id.clone());
        }
        let timestamp = chrono::Utc::now().timestamp();
        let id = uuid::Uuid::new_v4().to_string();
        state.db.save_prompt(&Prompt {
            id: id.clone(),
            name: format!(
                "{} · {}",
                app.as_str(),
                target.path.file_name().unwrap().to_string_lossy()
            ),
            content,
            description: Some(format!("Imported from {}", display_path(&target.path))),
            created_at: Some(timestamp),
            updated_at: Some(timestamp),
        })?;
        Ok(id)
    }

    pub fn get_current_file_content(app: AppType) -> Result<Option<String>, AppError> {
        let _guard = crate::sync_utils::lock_file_operation(&PROMPT_LOCK);
        PromptTarget::open(&app)?.read()
    }
}

struct PromptTarget {
    path: PathBuf,
    pi: Option<PiAgentsFileGuard>,
}

impl PromptTarget {
    fn open(app: &AppType) -> Result<Self, AppError> {
        Ok(Self {
            path: prompt_file_path(app)?,
            pi: if app == &AppType::Pi {
                Some(PiAgentsFileGuard::acquire()?)
            } else {
                None
            },
        })
    }

    fn read(&self) -> Result<Option<String>, AppError> {
        if let Some(guard) = &self.pi {
            return Ok(guard.read()?.content);
        }
        match std::fs::read_to_string(&self.path) {
            Ok(content) => Ok(Some(content)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(AppError::io(&self.path, error)),
        }
    }

    fn snapshot(&self, state: &AppState, app: &AppType) -> Result<PromptTargetSnapshot, AppError> {
        let content = self.read()?;
        let path = self.path.to_string_lossy().to_string();
        let owned_path = target_identity(&self.path)?.to_string_lossy().into_owned();
        let application = state
            .db
            .get_prompt_application(app)?
            .filter(|application| application.path == owned_path);
        let managed = application.as_ref().is_some_and(|application| {
            content
                .as_deref()
                .is_some_and(|content| content_hash(content) == application.content_hash)
        });
        let prompts = state.db.get_prompts()?;
        let matches_library = managed
            && application.as_ref().is_some_and(|application| {
                prompts
                    .get(&application.prompt_id)
                    .is_some_and(|prompt| content.as_ref() == Some(&prompt.content))
            });
        Ok(PromptTargetSnapshot {
            revision: target_revision(&self.path, content.as_deref())?,
            display_path: display_path(&self.path),
            path,
            content,
            applied_prompt_id: application.map(|application| application.prompt_id),
            managed,
            matches_library,
        })
    }

    fn replace(&self, expected_revision: &str, content: &str) -> Result<(), AppError> {
        let current = self.read()?;
        if target_revision(&self.path, current.as_deref())? != expected_revision {
            return Err(changed_target(&self.path));
        }
        if let Some(guard) = &self.pi {
            let revision = current
                .as_deref()
                .map(content_hash)
                .unwrap_or_else(|| "missing".into());
            guard.replace(&revision, content)
        } else {
            write_text_file(&self.path, content)
        }
    }
}

fn content_hash(content: &str) -> String {
    crate::live::engine::sha256_hex(content.as_bytes())
}

/// Resolve directory links as well as a final file link. Canonicalizing the
/// nearest existing ancestor keeps missing and newly-created targets in the
/// same namespace, including Windows' verbatim path prefix.
fn target_identity(path: &Path) -> Result<PathBuf, AppError> {
    let resolved = resolve_symlink_target(path);
    let mut ancestor = if resolved.is_absolute() {
        resolved
    } else {
        std::env::current_dir()
            .map_err(|error| AppError::io(path, error))?
            .join(resolved)
    };
    let mut missing = Vec::new();
    loop {
        match ancestor.canonicalize() {
            Ok(mut canonical) => {
                for name in missing.into_iter().rev() {
                    canonical.push(name);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = ancestor
                    .file_name()
                    .ok_or_else(|| AppError::io(&ancestor, error))?
                    .to_os_string();
                missing.push(name);
                ancestor.pop();
            }
            Err(error) => return Err(AppError::io(&ancestor, error)),
        }
    }
}

/// Bind the preview to the configured path and its full physical destination.
fn target_revision(path: &Path, content: Option<&str>) -> Result<String, AppError> {
    let identity = serde_json::json!([
        path.to_string_lossy(),
        target_identity(path)?.to_string_lossy(),
        content,
    ]);
    Ok(content_hash(&identity.to_string()))
}

fn changed_target(path: &Path) -> AppError {
    AppError::Conflict(format!(
        "{} changed after preview; reload the target before applying instructions",
        display_path(path)
    ))
}

fn apply_to_target(
    state: &AppState,
    app: &AppType,
    prompt: &Prompt,
    expected_revision: &str,
    target: &PromptTarget,
) -> Result<(), AppError> {
    validate_prompt_content(app, &prompt.content)?;
    let application = PromptApplication {
        prompt_id: prompt.id.clone(),
        path: target_identity(&target.path)?
            .to_string_lossy()
            .into_owned(),
        content_hash: content_hash(&prompt.content),
    };
    // Existing helper rolls back the file when the DB commit fails, while
    // preserving any external edit made after our write.
    crate::mcode_config::write_and_commit(
        &target.path,
        || target.replace(expected_revision, &prompt.content),
        || state.db.save_prompt_application(app, &application),
    )
}

fn remove_from_target(
    state: &AppState,
    app: &AppType,
    expected_revision: &str,
    target: &PromptTarget,
) -> Result<(), AppError> {
    let snapshot = target.snapshot(state, app)?;
    if snapshot.revision != expected_revision {
        return Err(changed_target(&target.path));
    }
    if snapshot.applied_prompt_id.is_none() {
        return Err(AppError::Conflict(
            "This target has no managed prompt application".into(),
        ));
    }
    if snapshot.content.is_none() {
        return state.db.delete_prompt_application(app);
    }
    if !snapshot.managed {
        return Err(AppError::Conflict(format!(
            "{} was edited outside the library; keeping its contents",
            snapshot.display_path
        )));
    }
    crate::mcode_config::write_and_commit(
        &target.path,
        || target.replace(expected_revision, ""),
        || state.db.delete_prompt_application(app),
    )
}

#[cfg(test)]
mod tests;
