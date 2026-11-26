//! Import project-owned Skills into the existing library without enabling Agents.

use super::*;
use crate::services::project_profile::{copy_tree, hash_tree, validate_copy_source};

impl SkillService {
    /// The boolean identifies a new library entry, for the caller's rollback.
    pub fn import_from_project(
        db: &Arc<Database>,
        source: &Path,
    ) -> Result<(InstalledSkill, bool)> {
        validate_copy_source(source)?;
        let source_hash = hash_tree(source, true)?;
        let base = source
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(Self::sanitize_install_name)
            .ok_or_else(|| anyhow!("Invalid project Skill directory: {}", source.display()))?;
        let metadata = Self::parse_skill_metadata_static(&source.join("SKILL.md"))?;

        let _state_guard = skill_state_write_guard();
        let ssot = Self::get_ssot_dir()?;
        let existing = db.get_all_installed_skills()?;
        let mut directory = base.clone();
        let mut suffix = 0;
        loop {
            let destination = ssot.join(&directory);
            if let Some(skill) = existing
                .values()
                .find(|skill| skill.directory.eq_ignore_ascii_case(&directory))
            {
                if hash_tree(&ssot.join(&skill.directory), true)
                    .is_ok_and(|hash| hash == source_hash)
                {
                    return Ok((skill.clone(), false));
                }
            } else if !existing.contains_key(&format!("local:{directory}"))
                && !destination.exists()
                && !Self::is_symlink(&destination)
            {
                break;
            }
            suffix += 1;
            directory = format!("{base}-project-{}", &source_hash[..8]);
            if suffix > 1 {
                directory.push_str(&format!("-{suffix}"));
            }
        }

        // Keep failed/changed copies out of the library and never overwrite an
        // existing resource. The deployment copier rejects nested symlinks.
        let staging = tempfile::tempdir_in(&ssot)?;
        let staged = staging.path().join("skill");
        copy_tree(source, &staged)?;
        if hash_tree(&staged, true)? != source_hash {
            return Err(anyhow!("Project Skill changed during import; please retry"));
        }
        let destination = ssot.join(&directory);
        if destination.exists() || Self::is_symlink(&destination) {
            return Err(anyhow!(
                "Skill destination changed during import; please retry"
            ));
        }
        fs::rename(&staged, &destination)?;

        let skill = InstalledSkill {
            id: format!("local:{directory}"),
            name: metadata.name.unwrap_or(base),
            description: metadata.description,
            directory,
            repo_host: None,
            repo_owner: None,
            repo_name: None,
            repo_branch: None,
            readme_url: None,
            apps: SkillApps::default(),
            installed_at: Utc::now().timestamp(),
            content_hash: Self::compute_dir_hash(&destination).ok(),
            updated_at: 0,
        };
        if let Err(error) = db.save_skill(&skill) {
            // This directory was just created by this import; preserve it if an
            // external editor has already changed it.
            if hash_tree(&destination, true).is_ok_and(|hash| hash == source_hash) {
                let _ = fs::remove_dir_all(&destination);
            }
            return Err(error.into());
        }
        Ok((skill, true))
    }

    /// Undo only a new, untouched project import; never run Agent uninstall.
    pub fn rollback_project_import(
        db: &Arc<Database>,
        imported: &InstalledSkill,
        expected_tree_hash: &str,
    ) -> Result<()> {
        let _state_guard = skill_state_write_guard();
        let current = db
            .get_installed_skill(&imported.id)?
            .ok_or_else(|| anyhow!("Imported Skill no longer exists: {}", imported.name))?;
        if !current.apps.is_empty()
            || serde_json::to_value(&current)? != serde_json::to_value(imported)?
        {
            return Err(anyhow!("Imported Skill changed; keeping {}", imported.name));
        }
        let directory = Self::require_valid_directory(&imported.directory)?;
        let destination = Self::get_ssot_dir()?.join(directory);
        if hash_tree(&destination, true)? != expected_tree_hash {
            return Err(anyhow!(
                "Imported Skill files changed; keeping {}",
                imported.name
            ));
        }
        fs::remove_dir_all(&destination)?;
        db.delete_skill(&imported.id)?;
        Ok(())
    }
}
