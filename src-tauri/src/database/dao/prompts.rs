//! Shared prompt documents and separate, device-local application records.

use crate::app_config::AppType;
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::prompt::{Prompt, PromptApplication};
use indexmap::IndexMap;
use rusqlite::{params, OptionalExtension};

impl Database {
    pub fn get_prompts(&self) -> Result<IndexMap<String, Prompt>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, name, content, description, created_at, updated_at
                 FROM prompts ORDER BY created_at ASC, id ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                Ok((
                    id.clone(),
                    Prompt {
                        id,
                        name: row.get(1)?,
                        content: row.get(2)?,
                        description: row.get(3)?,
                        created_at: row.get(4)?,
                        updated_at: row.get(5)?,
                    },
                ))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        rows.collect::<Result<IndexMap<_, _>, _>>()
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub fn save_prompt(&self, prompt: &Prompt) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO prompts (id, name, content, description, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, content = excluded.content,
                 description = excluded.description, created_at = excluded.created_at,
                 updated_at = excluded.updated_at",
            params![
                prompt.id,
                prompt.name,
                prompt.content,
                prompt.description,
                prompt.created_at,
                prompt.updated_at,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Removing a library entry never revokes ownership of already-deployed files.
    pub fn delete_prompt(&self, id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute("DELETE FROM prompts WHERE id = ?1", [id])
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn get_prompt_application(
        &self,
        app: &AppType,
    ) -> Result<Option<PromptApplication>, AppError> {
        let conn = lock_conn!(self.conn);
        conn.query_row(
            "SELECT prompt_id, path, content_hash FROM prompt_applications WHERE app_type = ?1",
            params![app],
            |row| {
                Ok(PromptApplication {
                    prompt_id: row.get(0)?,
                    path: row.get(1)?,
                    content_hash: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(|e| AppError::Database(e.to_string()))
    }

    pub fn save_prompt_application(
        &self,
        app: &AppType,
        application: &PromptApplication,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO prompt_applications (app_type, prompt_id, path, content_hash)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(app_type) DO UPDATE SET prompt_id = excluded.prompt_id,
                 path = excluded.path, content_hash = excluded.content_hash",
            params![
                app,
                application.prompt_id,
                application.path,
                application.content_hash,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_prompt_application(&self, app: &AppType) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM prompt_applications WHERE app_type = ?1",
            params![app],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_documents_and_agent_application_records_change_independently() {
        let db = Database::memory().unwrap();
        let mut prompt = Prompt {
            id: "shared".into(),
            name: "Project rules".into(),
            content: "original rules".into(),
            description: None,
            created_at: Some(1),
            updated_at: Some(1),
        };
        db.save_prompt(&prompt).unwrap();
        for (app, path) in [
            (AppType::Claude, "/claude/CLAUDE.md"),
            (AppType::Codex, "/codex/AGENTS.md"),
        ] {
            db.save_prompt_application(
                &app,
                &PromptApplication {
                    prompt_id: prompt.id.clone(),
                    path: path.into(),
                    content_hash: "original-hash".into(),
                },
            )
            .unwrap();
        }
        prompt.content = "updated library rules".into();
        db.save_prompt(&prompt).unwrap();
        assert_eq!(db.get_prompts().unwrap().len(), 1);
        assert_eq!(db.get_prompts().unwrap()["shared"].content, prompt.content);
        let claude = db
            .get_prompt_application(&AppType::Claude)
            .unwrap()
            .unwrap();
        let codex = db.get_prompt_application(&AppType::Codex).unwrap().unwrap();
        assert_eq!(claude.prompt_id, codex.prompt_id);
        assert_eq!(claude.path, "/claude/CLAUDE.md");
        assert_eq!(codex.path, "/codex/AGENTS.md");
        assert_eq!(claude.content_hash, "original-hash");
        assert_eq!(codex.content_hash, "original-hash");

        db.delete_prompt("shared").unwrap();
        assert!(db.get_prompts().unwrap().is_empty());
        assert_eq!(
            db.get_prompt_application(&AppType::Claude).unwrap(),
            Some(claude)
        );
        db.delete_prompt_application(&AppType::Claude).unwrap();
        assert!(db
            .get_prompt_application(&AppType::Claude)
            .unwrap()
            .is_none());
        assert_eq!(
            db.get_prompt_application(&AppType::Codex).unwrap(),
            Some(codex)
        );
    }
}
