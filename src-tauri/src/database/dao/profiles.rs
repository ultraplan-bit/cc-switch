//! 项目 Profile 数据访问对象
//!
//! profiles 表存放全应用共享的项目实体（供应商/MCP/Skills/Prompt 快照），
//! payload 为原始 JSON 文本（按 app 分槽），解析在 service 层进行。
//! 各应用分组（scope）独立的 current 标记存放于 settings 表（key-value）。

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use rusqlite::params;

/// 每个 scope 的 current 标记 key = 前缀 + scope（如 current_profile_id_claude）
const CURRENT_PROFILE_ID_KEY_PREFIX: &str = "current_profile_id_";
/// The current Profile was edited, or applied with warnings, and no longer
/// describes the live configuration exactly.
const CURRENT_PROFILE_STALE_KEY_PREFIX: &str = "current_profile_stale_";
/// JSON array of normalized project directories any Profile can be deployed to.
const PROFILE_PROJECTS_KEY: &str = "profile_projects_v1";

fn current_profile_key(scope: &str) -> String {
    format!("{CURRENT_PROFILE_ID_KEY_PREFIX}{scope}")
}

fn current_profile_stale_key(scope: &str) -> String {
    format!("{CURRENT_PROFILE_STALE_KEY_PREFIX}{scope}")
}

/// Project directories compare the way the platform resolves them.
pub(crate) fn same_project_path(left: &str, right: &str) -> bool {
    #[cfg(windows)]
    return left.to_lowercase() == right.to_lowercase();
    #[cfg(not(windows))]
    return left == right;
}

fn parse_project_registry(raw: Option<String>) -> Result<Vec<String>, AppError> {
    raw.map(|raw| serde_json::from_str(&raw))
        .transpose()
        .map(Option::unwrap_or_default)
        .map_err(|error| AppError::Database(format!("读取项目登记表失败: {error}")))
}

fn push_project(projects: &mut Vec<String>, path: &str) -> bool {
    if projects.iter().any(|known| same_project_path(known, path)) {
        return false;
    }
    projects.push(path.to_string());
    true
}

fn read_setting(
    transaction: &rusqlite::Transaction<'_>,
    key: &str,
) -> Result<Option<String>, AppError> {
    match transaction.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
        row.get(0)
    }) {
        Ok(value) => Ok(Some(value)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(AppError::Database(error.to_string())),
    }
}

/// 项目 Profile 记录（全应用共享，无所属分组）
#[derive(Debug, Clone)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// 原始 JSON 快照文本（ProfilePayload），解析在 service 层
    pub payload: String,
    pub sort_order: Option<i64>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
}

impl Database {
    /// 获取所有项目（按 sort_order 优先、created_at 兜底排序）
    pub fn get_all_profiles(&self) -> Result<Vec<Profile>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, name, payload, sort_order, created_at, updated_at
                 FROM profiles
                 ORDER BY sort_order IS NULL, sort_order, created_at, id",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(Profile {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    payload: row.get(2)?,
                    sort_order: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut profiles = Vec::new();
        for row in rows {
            profiles.push(row.map_err(|e| AppError::Database(e.to_string()))?);
        }
        Ok(profiles)
    }

    /// 获取单个项目
    pub fn get_profile(&self, id: &str) -> Result<Option<Profile>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, name, payload, sort_order, created_at, updated_at
                 FROM profiles WHERE id = ?1",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        match stmt.query_row(params![id], |row| {
            Ok(Profile {
                id: row.get(0)?,
                name: row.get(1)?,
                payload: row.get(2)?,
                sort_order: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        }) {
            Ok(profile) => Ok(Some(profile)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    /// 保存配置方案（插入或更新全部受管字段）。
    pub fn save_profile(&self, profile: &Profile) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO profiles
             (id, name, payload, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                payload = excluded.payload,
                sort_order = excluded.sort_order,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at",
            params![
                profile.id,
                profile.name,
                profile.payload,
                profile.sort_order,
                profile.created_at,
                profile.updated_at,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 保存手动编辑；若它正是该分组的当前方案，标记为「已修改、待重新应用」。
    pub(crate) fn update_profile_config_record(
        &self,
        profile: &Profile,
        scope: &str,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        let transaction = conn
            .unchecked_transaction()
            .map_err(|error| AppError::Database(error.to_string()))?;
        transaction
            .execute(
                "UPDATE profiles SET name = ?2, payload = ?3, updated_at = ?4 WHERE id = ?1",
                params![
                    profile.id,
                    profile.name,
                    profile.payload,
                    profile.updated_at
                ],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        transaction
            .execute(
                "INSERT OR REPLACE INTO settings (key, value)
                 SELECT ?1, '1' WHERE EXISTS
                 (SELECT 1 FROM settings WHERE key = ?2 AND value = ?3)",
                params![
                    current_profile_stale_key(scope),
                    current_profile_key(scope),
                    profile.id
                ],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        transaction
            .commit()
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub(crate) fn is_profile_project(&self, path: &str) -> Result<bool, AppError> {
        Ok(self
            .get_profile_projects()?
            .iter()
            .any(|known| same_project_path(known, path)))
    }

    /// Registered project directories, in registration order.
    pub fn get_profile_projects(&self) -> Result<Vec<String>, AppError> {
        parse_project_registry(self.get_setting(PROFILE_PROJECTS_KEY)?)
    }

    fn update_profile_projects(
        &self,
        change: impl FnOnce(&mut Vec<String>) -> bool,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        let transaction = conn
            .unchecked_transaction()
            .map_err(|error| AppError::Database(error.to_string()))?;
        let mut projects =
            parse_project_registry(read_setting(&transaction, PROFILE_PROJECTS_KEY)?)?;
        if change(&mut projects) {
            transaction
                .execute(
                    "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                    params![
                        PROFILE_PROJECTS_KEY,
                        serde_json::to_string(&projects)
                            .map_err(|error| AppError::Config(error.to_string()))?
                    ],
                )
                .map_err(|error| AppError::Database(error.to_string()))?;
        }
        transaction
            .commit()
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// Register a normalized project directory; a known directory is kept as is.
    pub fn add_profile_project(&self, path: &str) -> Result<(), AppError> {
        self.update_profile_projects(|projects| push_project(projects, path))
    }

    /// Forget a project directory. Project files are never touched.
    pub fn remove_profile_project(&self, path: &str) -> Result<(), AppError> {
        self.update_profile_projects(|projects| {
            let before = projects.len();
            projects.retain(|known| !same_project_path(known, path));
            projects.len() != before
        })
    }

    /// 删除项目，返回是否实际删除了记录
    pub fn delete_profile(&self, id: &str) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let affected = conn
            .execute("DELETE FROM profiles WHERE id = ?1", params![id])
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(affected > 0)
    }

    /// 读取某分组当前激活的项目 id（未使用项目时为 None）
    pub fn get_current_profile_id(&self, scope: &str) -> Result<Option<String>, AppError> {
        self.get_setting(&current_profile_key(scope))
    }

    /// 设置某分组当前激活的方案 id；None 表示"不使用方案"（删除 key）。
    /// 总是清除「待重新应用」标记：新写入的指针代表一次完整应用。
    pub fn set_current_profile_id(&self, scope: &str, id: Option<&str>) -> Result<(), AppError> {
        self.set_current_profile(scope, id, false)
    }

    /// Point a scope at a Profile and record whether live configuration matches it.
    pub fn set_current_profile(
        &self,
        scope: &str,
        id: Option<&str>,
        stale: bool,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        let transaction = conn
            .unchecked_transaction()
            .map_err(|e| AppError::Database(e.to_string()))?;
        match id {
            Some(id) => transaction.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params![current_profile_key(scope), id],
            ),
            None => transaction.execute(
                "DELETE FROM settings WHERE key = ?1",
                params![current_profile_key(scope)],
            ),
        }
        .map_err(|e| AppError::Database(e.to_string()))?;
        if stale && id.is_some() {
            transaction.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, '1')",
                params![current_profile_stale_key(scope)],
            )
        } else {
            transaction.execute(
                "DELETE FROM settings WHERE key = ?1",
                params![current_profile_stale_key(scope)],
            )
        }
        .map_err(|e| AppError::Database(e.to_string()))?;
        transaction
            .commit()
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// Whether the current Profile of a scope was edited or only partly applied.
    pub fn is_current_profile_stale(&self, scope: &str) -> Result<bool, AppError> {
        Ok(self
            .get_setting(&current_profile_stale_key(scope))?
            .is_some())
    }

    /// Mark the current Profile stale; a no-op unless `profile_id` is current.
    pub fn mark_current_profile_stale(
        &self,
        scope: &str,
        profile_id: &str,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value)
             SELECT ?1, '1' WHERE EXISTS
             (SELECT 1 FROM settings WHERE key = ?2 AND value = ?3)",
            params![
                current_profile_stale_key(scope),
                current_profile_key(scope),
                profile_id
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// The current Profile only while live configuration still matches it.
    pub fn get_applied_profile_id(&self, scope: &str) -> Result<Option<String>, AppError> {
        if self.is_current_profile_stale(scope)? {
            return Ok(None);
        }
        self.get_current_profile_id(scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, name: &str, sort_order: Option<i64>) -> Profile {
        Profile {
            id: id.to_string(),
            name: name.to_string(),
            payload: r#"{"providers":{"claude":null,"codex":null}}"#.to_string(),
            sort_order,
            created_at: Some(1_000),
            updated_at: Some(1_000),
        }
    }

    #[test]
    fn test_profile_crud_roundtrip() -> Result<(), AppError> {
        let db = Database::memory()?;

        db.save_profile(&sample("a", "Dev", Some(2)))?;
        db.save_profile(&sample("b", "Draw", Some(1)))?;
        db.save_profile(&sample("c", "Misc", None))?;

        // sort_order 优先，NULL 排最后
        let all = db.get_all_profiles()?;
        assert_eq!(
            all.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "a", "c"]
        );

        let got = db.get_profile("a")?.expect("profile a exists");
        assert_eq!(got.name, "Dev");
        assert!(got.payload.contains("providers"));

        // 整行替换更新
        let mut updated = sample("a", "Dev Renamed", Some(2));
        updated.updated_at = Some(2_000);
        db.save_profile(&updated)?;
        let got = db.get_profile("a")?.expect("profile a exists");
        assert_eq!(got.name, "Dev Renamed");
        assert_eq!(got.updated_at, Some(2_000));

        assert!(db.delete_profile("a")?);
        assert!(!db.delete_profile("a")?);
        assert!(db.get_profile("a")?.is_none());
        Ok(())
    }

    #[test]
    fn project_registry_deduplicates_like_the_platform() -> Result<(), AppError> {
        let db = Database::memory()?;
        assert!(db.get_profile_projects()?.is_empty());
        db.add_profile_project("/Projects/Demo")?;
        db.add_profile_project("/Projects/Demo")?;
        db.add_profile_project("/projects/demo")?;
        #[cfg(windows)]
        assert_eq!(
            db.get_profile_projects()?,
            vec!["/Projects/Demo".to_string()]
        );
        #[cfg(not(windows))]
        assert_eq!(
            db.get_profile_projects()?,
            vec!["/Projects/Demo".to_string(), "/projects/demo".to_string()]
        );
        db.remove_profile_project("/missing")?;
        db.remove_profile_project("/Projects/Demo")?;
        #[cfg(windows)]
        assert!(db.get_profile_projects()?.is_empty());
        #[cfg(not(windows))]
        assert_eq!(
            db.get_profile_projects()?,
            vec!["/projects/demo".to_string()]
        );
        Ok(())
    }

    #[test]
    fn stale_flag_follows_the_current_profile() -> Result<(), AppError> {
        let db = Database::memory()?;
        db.mark_current_profile_stale("claude", "a")?;
        assert!(!db.is_current_profile_stale("claude")?);

        db.set_current_profile_id("claude", Some("a"))?;
        db.set_current_profile_id("codex", Some("b"))?;
        assert_eq!(db.get_applied_profile_id("claude")?, Some("a".to_string()));
        db.mark_current_profile_stale("claude", "other")?;
        assert!(!db.is_current_profile_stale("claude")?);
        db.mark_current_profile_stale("claude", "a")?;
        assert!(db.is_current_profile_stale("claude")?);
        assert_eq!(db.get_current_profile_id("claude")?, Some("a".to_string()));
        assert_eq!(db.get_applied_profile_id("claude")?, None);
        assert_eq!(db.get_applied_profile_id("codex")?, Some("b".to_string()));

        // Any new pointer, including the same Profile, is a complete application.
        db.set_current_profile_id("claude", Some("a"))?;
        assert_eq!(db.get_applied_profile_id("claude")?, Some("a".to_string()));
        db.set_current_profile("claude", Some("a"), true)?;
        assert_eq!(db.get_applied_profile_id("claude")?, None);
        db.set_current_profile_id("claude", None)?;
        assert!(!db.is_current_profile_stale("claude")?);
        db.set_current_profile("claude", None, true)?;
        assert!(!db.is_current_profile_stale("claude")?);

        // Saving an edit marks only the scope whose current Profile it is.
        db.save_profile(&sample("a", "A", None))?;
        db.set_current_profile_id("claude", Some("a"))?;
        db.update_profile_config_record(&sample("a", "A edited", None), "codex")?;
        assert!(!db.is_current_profile_stale("claude")?);
        db.update_profile_config_record(&sample("a", "A edited", None), "claude")?;
        assert!(db.is_current_profile_stale("claude")?);
        assert_eq!(db.get_current_profile_id("claude")?, Some("a".to_string()));
        Ok(())
    }

    #[test]
    fn test_current_profile_id_is_scoped() -> Result<(), AppError> {
        let db = Database::memory()?;

        assert_eq!(db.get_current_profile_id("claude")?, None);
        assert_eq!(db.get_current_profile_id("codex")?, None);

        // 两个分组的标记互不影响
        db.set_current_profile_id("claude", Some("a"))?;
        db.set_current_profile_id("codex", Some("b"))?;
        assert_eq!(db.get_current_profile_id("claude")?, Some("a".to_string()));
        assert_eq!(db.get_current_profile_id("codex")?, Some("b".to_string()));

        db.set_current_profile_id("claude", None)?;
        assert_eq!(db.get_current_profile_id("claude")?, None);
        assert_eq!(db.get_current_profile_id("codex")?, Some("b".to_string()));

        // 重复清除应幂等
        db.set_current_profile_id("claude", None)?;
        assert_eq!(db.get_current_profile_id("claude")?, None);
        Ok(())
    }
}
