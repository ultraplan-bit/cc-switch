//! 当前数据库结构与 DAO 的测试。
use super::*;
use crate::app_config::AppType;
use rusqlite::Connection;
use tempfile::NamedTempFile;

#[derive(Debug)]
struct ColumnInfo {
    r#type: String,
    notnull: i64,
    default: Option<String>,
}

fn get_column_info(conn: &Connection, table: &str, column: &str) -> ColumnInfo {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{table}\");"))
        .expect("prepare pragma");
    let mut rows = stmt.query([]).expect("query pragma");
    while let Some(row) = rows.next().expect("read row") {
        let column_name: String = row.get(1).expect("name");
        if column_name.eq_ignore_ascii_case(column) {
            return ColumnInfo {
                r#type: row.get::<_, String>(2).expect("type"),
                notnull: row.get::<_, i64>(3).expect("notnull"),
                default: row.get::<_, Option<String>>(4).ok().flatten(),
            };
        }
    }
    panic!("column {table}.{column} not found");
}

fn normalize_default(default: &Option<String>) -> Option<String> {
    default
        .as_ref()
        .map(|s| s.trim_matches('\'').trim_matches('"').to_string())
}

#[test]
fn deleted_default_skill_repo_is_not_restored() {
    let db = Database::memory().expect("create memory db");

    assert_eq!(db.init_default_skill_repos().expect("initialize repos"), 4);
    for repo in db.get_skill_repos().expect("get initialized repos") {
        db.delete_skill_repo(&repo.owner, &repo.name)
            .expect("delete repo");
    }
    assert!(db.get_skill_repos().expect("get deleted repos").is_empty());

    assert_eq!(
        db.init_default_skill_repos().expect("reinitialize repos"),
        0
    );
    assert!(db.get_skill_repos().expect("get repos").is_empty());
}

#[test]
fn existing_skill_repo_selection_is_not_supplemented() {
    let db = Database::memory().expect("create memory db");
    let default_store = crate::services::skill::SkillStore::default();
    db.save_skill_repo(&default_store.repos[0])
        .expect("save existing repo");

    assert_eq!(db.init_default_skill_repos().expect("initialize repos"), 0);
    assert_eq!(db.get_skill_repos().expect("get repos").len(), 1);
    assert!(db
        .get_bool_flag("default_skill_repos_initialized")
        .expect("get initialized flag"));
}

#[test]
fn schema_create_tables_include_pricing_model_columns() {
    let conn = Connection::open_in_memory().expect("open memory db");
    Database::create_tables_on_conn(&conn).expect("create tables");

    let multiplier = get_column_info(&conn, "proxy_config", "default_cost_multiplier");
    assert_eq!(multiplier.r#type, "TEXT");
    assert_eq!(multiplier.notnull, 1);
    assert_eq!(normalize_default(&multiplier.default).as_deref(), Some("1"));

    let pricing_source = get_column_info(&conn, "proxy_config", "pricing_model_source");
    assert_eq!(pricing_source.r#type, "TEXT");
    assert_eq!(pricing_source.notnull, 1);
    assert_eq!(
        normalize_default(&pricing_source.default).as_deref(),
        Some("response")
    );

    let request_model = get_column_info(&conn, "proxy_request_logs", "request_model");
    assert_eq!(request_model.r#type, "TEXT");
    assert_eq!(request_model.notnull, 0);
}

#[test]
fn schema_model_pricing_is_seeded_on_init() {
    let db = Database::memory().expect("create memory db");

    let conn = db.conn.lock().expect("lock conn");

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM model_pricing", [], |row| row.get(0))
        .expect("count pricing");

    assert!(
        count > 0,
        "模型定价数据应该在初始化时自动填充，实际数量: {}",
        count
    );

    // 验证包含 Claude 模型
    let claude_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_pricing WHERE model_id LIKE 'claude-%'",
            [],
            |row| row.get(0),
        )
        .expect("check claude");
    assert!(
        claude_count > 0,
        "应该包含 Claude 模型定价，实际数量: {}",
        claude_count
    );

    // 验证包含 GPT 模型
    let gpt_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_pricing WHERE model_id LIKE 'gpt-%'",
            [],
            |row| row.get(0),
        )
        .expect("check gpt");
    assert!(
        gpt_count > 0,
        "应该包含 GPT 模型定价，实际数量: {}",
        gpt_count
    );

    // 验证包含 Gemini 模型
    let gemini_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_pricing WHERE model_id LIKE 'gemini-%'",
            [],
            |row| row.get(0),
        )
        .expect("check gemini");
    assert!(
        gemini_count > 0,
        "应该包含 Gemini 模型定价，实际数量: {}",
        gemini_count
    );
}

#[test]
fn model_pricing_seed_covers_deepseek_v41_flash_aliases() {
    let db = Database::memory().expect("create memory db");
    let conn = db.conn.lock().expect("lock conn");

    // 官方定价页（2026-09-11）：deepseek-flash 是唯一推荐名，两个 legacy 名仍被接受
    // 但均由 V4.1-Flash 承接并按 Flash 价计费 → 四行同价（本表统一录高峰档）。
    // 查价前缀兜底是 LIKE '{id}-%'，只命中更长的行，任一行缺失都会静默按 0 计费。
    for model_id in [
        "deepseek-flash",
        "deepseek-v4.1-flash",
        "deepseek-v4-flash",
        "deepseek-v4-flash-0731",
        "deepseek-v4-flash-vision-exp",
    ] {
        let price: (String, String, String, String) = conn
            .query_row(
                "SELECT input_cost_per_million, output_cost_per_million,
                        cache_read_cost_per_million, cache_creation_cost_per_million
                 FROM model_pricing WHERE model_id = ?1",
                [model_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("query DeepSeek V4.1 Flash family price");
        assert_eq!(
            price,
            (
                "0.3".to_string(),
                "1.2".to_string(),
                "0.006".to_string(),
                "0".to_string()
            ),
            "{model_id}"
        );
    }
}

#[test]
fn model_pricing_seed_includes_claude_fable_5_2() {
    let db = Database::memory().expect("create memory db");
    let conn = db.conn.lock().expect("lock conn");

    let price: (String, String, String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
             FROM model_pricing WHERE model_id = 'claude-fable-5-2'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("query Fable 5.2 price");

    assert_eq!(
        price,
        (
            "8".to_string(),
            "40".to_string(),
            "0.20".to_string(),
            "10".to_string(),
        )
    );
}

#[test]
fn model_pricing_seed_includes_claude_5_1_and_standard_sonnet_5_prices() {
    let db = Database::memory().expect("create memory db");
    let conn = db.conn.lock().expect("lock conn");

    for model_id in ["claude-fable-5-1", "claude-mythos-5-1"] {
        let price: (String, String, String, String) = conn
            .query_row(
                "SELECT input_cost_per_million, output_cost_per_million,
                        cache_read_cost_per_million, cache_creation_cost_per_million
                 FROM model_pricing WHERE model_id = ?1",
                [model_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("query Fable 5.1 family price");
        // 缓存读 0.025x = $0.25，不是 Fable 5 的 $1
        assert_eq!(
            price,
            (
                "10".to_string(),
                "50".to_string(),
                "0.25".to_string(),
                "12.50".to_string(),
            ),
            "{model_id}"
        );
    }

    let sonnet: (String, String, String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
             FROM model_pricing WHERE model_id = 'claude-sonnet-5'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("query Sonnet 5 price");
    // $2/$10 介绍价已转为正式价（原定 2026-09-01 涨至 $3/$15 取消）
    assert_eq!(
        sonnet,
        (
            "2".to_string(),
            "10".to_string(),
            "0.20".to_string(),
            "2.50".to_string(),
        )
    );
}

#[test]
fn model_pricing_seed_includes_claude_opus_5_5() {
    let db = Database::memory().expect("create memory db");
    let conn = db.conn.lock().expect("lock conn");

    let price: (String, String, String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
             FROM model_pricing WHERE model_id = 'claude-opus-5-5'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("query Opus 5.5 price");

    // 缓存读 0.05x = $0.20：不是常规 0.1x 的 $0.40，也不是 Opus 5 的 $0.50
    assert_eq!(
        price,
        (
            "4".to_string(),
            "20".to_string(),
            "0.20".to_string(),
            "5".to_string(),
        )
    );
}

#[test]
fn model_pricing_seed_includes_claude_sonnet_5_5_and_haiku_5_5() {
    let db = Database::memory().expect("create memory db");
    let conn = db.conn.lock().expect("lock conn");

    let price_of = |model_id: &str| -> (String, String, String, String) {
        conn.query_row(
            "SELECT input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
             FROM model_pricing WHERE model_id = ?1",
            [model_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap_or_else(|e| panic!("query {model_id} price: {e}"))
    };

    // 缓存读 0.05x = $0.10：不是 Sonnet 5 的 $0.20
    assert_eq!(
        price_of("claude-sonnet-5-5"),
        (
            "2".to_string(),
            "10".to_string(),
            "0.10".to_string(),
            "2.50".to_string(),
        )
    );
    // 10 万 token 以内的标准档
    assert_eq!(
        price_of("claude-haiku-5-5"),
        (
            "0.10".to_string(),
            "0.50".to_string(),
            "0.01".to_string(),
            "0.125".to_string(),
        )
    );
}

#[test]
fn model_pricing_seed_includes_qwen4_max() {
    let db = Database::memory().expect("create memory db");
    let conn = db.conn.lock().expect("lock conn");

    let price: (String, String, String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
             FROM model_pricing WHERE model_id = 'qwen4-max'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("query Qwen4 Max price");

    assert_eq!(
        price,
        (
            "1.50".to_string(),
            "4.50".to_string(),
            "0.075".to_string(),
            "1.875".to_string(),
        )
    );
}

#[test]
fn model_pricing_seed_includes_gpt_6_astra() {
    let db = Database::memory().expect("create memory db");
    let conn = db.conn.lock().expect("lock conn");

    let price: (String, String, String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
             FROM model_pricing WHERE model_id = 'gpt-6-astra'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("query GPT-6 Astra price");

    assert_eq!(
        price,
        (
            "10".to_string(),
            "50".to_string(),
            "1".to_string(),
            "12.5".to_string(),
        )
    );
}

#[test]
fn model_pricing_seed_includes_glm_5_3_flash() {
    let db = Database::memory().expect("create memory db");
    let conn = db.conn.lock().expect("lock conn");

    let price: (String, String, String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
             FROM model_pricing WHERE model_id = 'glm-5.3-flash'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("query GLM-5.3-Flash price");

    assert_eq!(
        price,
        (
            "0.15".to_string(),
            "0.50".to_string(),
            "0.03".to_string(),
            "0".to_string(),
        )
    );
}

#[test]
fn model_pricing_seed_includes_gemini_4_neon() {
    let db = Database::memory().expect("create memory db");
    let conn = db.conn.lock().expect("lock conn");

    let price: (String, String, String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
             FROM model_pricing WHERE model_id = 'gemini-4-neon'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("query Gemini 4 Neon price");

    assert_eq!(
        price,
        (
            "3.00".to_string(),
            "15.00".to_string(),
            "0.15".to_string(),
            "0".to_string(),
        )
    );
}

#[test]
fn model_pricing_seed_includes_gemini_4_argon() {
    let db = Database::memory().expect("create memory db");
    let conn = db.conn.lock().expect("lock conn");

    let price: (String, String, String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
             FROM model_pricing WHERE model_id = 'gemini-4-argon'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("query Gemini 4 Argon price");

    assert_eq!(
        price,
        (
            "2.00".to_string(),
            "10.00".to_string(),
            "0.10".to_string(),
            "0".to_string(),
        )
    );
}

#[test]
fn model_pricing_seed_includes_gemini_4_carbon() {
    let db = Database::memory().expect("create memory db");
    let conn = db.conn.lock().expect("lock conn");

    let price: (String, String, String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
             FROM model_pricing WHERE model_id = 'gemini-4-carbon'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("query Gemini 4 Carbon price");

    assert_eq!(
        price,
        (
            "2.00".to_string(),
            "10.00".to_string(),
            "0.10".to_string(),
            "0".to_string(),
        )
    );
}

#[test]
fn model_pricing_seed_includes_gemini_3_8_flash() {
    let db = Database::memory().expect("create memory db");
    let conn = db.conn.lock().expect("lock conn");

    let price: (String, String, String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
             FROM model_pricing WHERE model_id = 'gemini-3.8-flash'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("query Gemini 3.8 Flash price");

    assert_eq!(
        price,
        (
            "0.75".to_string(),
            "3.75".to_string(),
            "0.075".to_string(),
            "0".to_string(),
        )
    );
}

#[test]
fn ensure_incremental_auto_vacuum_rebuilds_existing_file_db() {
    let temp = NamedTempFile::new().expect("create temp db file");
    let path = temp.path().to_path_buf();

    let conn = Connection::open(&path).expect("open temp db");
    conn.execute("PRAGMA auto_vacuum = NONE;", [])
        .expect("set none auto_vacuum");
    Database::create_tables_on_conn(&conn).expect("create tables");

    assert_eq!(
        Database::get_auto_vacuum_mode(&conn).expect("auto_vacuum before rebuild"),
        0,
        "existing file db should start with NONE auto_vacuum"
    );

    let rebuilt =
        Database::ensure_incremental_auto_vacuum_on_conn(&conn).expect("enable incremental mode");
    assert!(rebuilt, "existing db should require rebuild via VACUUM");
    drop(conn);

    let reopened = Connection::open(&path).expect("reopen temp db");
    assert_eq!(
        Database::get_auto_vacuum_mode(&reopened).expect("auto_vacuum after rebuild"),
        2,
        "file db should persist INCREMENTAL auto_vacuum after VACUUM rebuild"
    );
}

#[test]
fn incremental_vacuum_reclaims_entire_freelist() {
    let temp = NamedTempFile::new().expect("create temp db file");
    let conn = Connection::open(temp.path()).expect("open temp db");
    conn.execute("PRAGMA auto_vacuum = INCREMENTAL;", [])
        .expect("set incremental auto_vacuum");
    conn.execute_batch(
        "CREATE TABLE bulk (payload BLOB);
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 200)
         INSERT INTO bulk SELECT zeroblob(4096) FROM n;
         DELETE FROM bulk;",
    )
    .expect("fill and clear table");

    let freelist = |conn: &Connection| -> i64 {
        conn.query_row("PRAGMA freelist_count;", [], |row| row.get(0))
            .expect("read freelist_count")
    };
    assert!(
        freelist(&conn) > 100,
        "deleting rows should leave free pages"
    );

    Database::incremental_vacuum_on_conn(&conn).expect("incremental vacuum");
    assert_eq!(freelist(&conn), 0, "all free pages should be reclaimed");
}

/// v22 已随 v4.0.19 发布：从它升级必须补上 `mcp_servers.enabled_omp`，否则读 MCP 列表报
/// `no such column`。Skills 没有 omp 列（启用状态按原生目录现算）。
#[test]
fn fresh_install_has_dsh_copilot_and_workbuddy_flags_at_current_version() {
    // 全新安装直接创建当前结构并记录版本。
    let conn = Connection::open_in_memory().expect("open memory db");
    Database::create_tables_on_conn(&conn).expect("create tables");
    assert_eq!(Database::get_user_version(&conn).unwrap(), SCHEMA_VERSION);
    for table in ["mcp_servers", "skills"] {
        for column in ["enabled_dsh", "enabled_copilot", "enabled_workbuddy"] {
            assert!(
                Database::has_column(&conn, table, column).expect("inspect column"),
                "{table}.{column} missing on fresh install"
            );
        }
    }
}

#[test]
fn recovers_after_a_panic_poisons_the_connection_lock() {
    let db = std::sync::Arc::new(Database::memory().expect("memory db"));
    db.set_setting("kept", "1").expect("seed");

    let poisoner = std::sync::Arc::clone(&db);
    let joined = std::thread::spawn(move || {
        let conn = poisoner.conn.lock().expect("lock conn");
        conn.execute_batch("BEGIN; INSERT INTO settings (key, value) VALUES ('lost', '1');")
            .expect("open transaction");
        panic!("task panics while holding the lock");
    })
    .join();
    assert!(joined.is_err());
    assert!(db.conn.is_poisoned());

    // 中毒后仍能读写，半截事务被回滚
    assert_eq!(
        db.get_setting("kept")
            .expect("read after poison")
            .as_deref(),
        Some("1")
    );
    assert_eq!(db.get_setting("lost").expect("read after poison"), None);
    db.set_setting("after", "1").expect("write after poison");
    assert!(!db.conn.is_poisoned());
}

#[test]
fn keeps_the_lock_poisoned_when_rollback_fails() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};
    let db = std::sync::Arc::new(Database::memory().expect("memory db"));

    let poisoner = std::sync::Arc::clone(&db);
    let joined = std::thread::spawn(move || {
        let conn = poisoner.conn.lock().expect("lock conn");
        conn.execute_batch("BEGIN").expect("open transaction");
        conn.authorizer(Some(|ctx: AuthContext<'_>| match ctx.action {
            AuthAction::Transaction {
                operation: TransactionOperation::Rollback,
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }));
        panic!("task panics while holding the lock");
    })
    .join();
    assert!(joined.is_err());

    assert!(db.get_setting("kept").is_err());
    assert!(db.conn.is_poisoned());
}

#[tokio::test]
async fn proxy_dao_calls_survive_a_poisoned_lock() {
    let db = std::sync::Arc::new(Database::memory().expect("memory db"));
    db.set_proxy_flags_sync(&AppType::Claude, true, true)
        .expect("set flags");
    let mut proxy = db.get_global_proxy_config().await.expect("set port");
    proxy.listen_port = 25721;
    db.update_global_proxy_config(proxy)
        .await
        .expect("set port");

    // 每次调用前都重新毒化，确保每个调用点自己完成恢复
    let poison = || {
        let poisoner = std::sync::Arc::clone(&db);
        let joined = std::thread::spawn(move || {
            let _conn = poisoner.conn.lock().expect("lock conn");
            panic!("task panics while holding the lock");
        })
        .join();
        assert!(joined.is_err());
        assert!(db.conn.is_poisoned());
    };

    poison();
    db.set_proxy_flags_sync(&AppType::Claude, true, false)
        .expect("set flags after poison");
    poison();
    assert_eq!(
        db.get_proxy_flags_sync(&AppType::Claude),
        (true, false),
        "get_proxy_flags_sync after poison"
    );
    poison();
    assert_eq!(
        db.get_proxy_client_endpoint_sync().1,
        25721,
        "get_proxy_client_endpoint_sync after poison"
    );
    poison();
    db.set_pricing_model_source(&AppType::Claude, "request")
        .await
        .expect("set pricing source after poison");
    assert_eq!(
        db.get_pricing_model_source(&AppType::Claude)
            .await
            .expect("read pricing source"),
        "request"
    );
}

#[test]
fn schema_initializes_only_empty_databases_and_rejects_other_versions_without_writes() {
    for version in [0, SCHEMA_VERSION - 1, SCHEMA_VERSION + 1] {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE original (value TEXT); INSERT INTO original VALUES ('keep');
             CREATE TABLE profiles (id TEXT PRIMARY KEY, name TEXT NOT NULL, payload TEXT NOT NULL);
             INSERT INTO profiles VALUES ('old-profile', 'Original profile', '{\"preserved\":true}');",
        )
        .unwrap();
        Database::set_user_version(&conn, version).unwrap();
        assert!(Database::create_tables_on_conn(&conn).is_err());
        assert_eq!(Database::get_user_version(&conn).unwrap(), version);
        assert!(!Database::table_exists(&conn, "providers").unwrap());
        assert!(!Database::has_column(&conn, "profiles", "target").unwrap());
        assert_eq!(
            conn.query_row(
                "SELECT name, payload FROM profiles WHERE id = 'old-profile'",
                [],
                |row| { Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)) }
            )
            .unwrap(),
            (
                "Original profile".to_string(),
                "{\"preserved\":true}".to_string()
            )
        );
        assert_eq!(
            conn.query_row("SELECT value FROM original", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "keep"
        );
    }
    let conn = Connection::open_in_memory().unwrap();
    Database::create_tables_on_conn(&conn).unwrap();
    assert_eq!(Database::get_user_version(&conn).unwrap(), SCHEMA_VERSION);
    assert!(!Database::has_column(&conn, "profiles", "project_path").unwrap());
    let target = get_column_info(&conn, "profiles", "target");
    assert_eq!(target.r#type, "TEXT");
    assert_eq!(target.notnull, 1);
    assert_eq!(target.default, None);
    assert!(conn
        .execute(
            "INSERT INTO profiles (id, name, payload) VALUES ('missing', 'Missing target', '{}')",
            []
        )
        .is_err());
    for target in [None, Some(""), Some("unknown")] {
        assert!(conn.execute(
            "INSERT INTO profiles (id, name, target, payload) VALUES ('invalid', 'Invalid target', ?1, '{}')",
            [target],
        ).is_err());
    }
    for target in ["global", "project"] {
        conn.execute(
            "INSERT INTO profiles (id, name, target, payload) VALUES (?1, 'Current profile', ?1, '{}')",
            [target],
        ).unwrap();
    }
    Database::create_tables_on_conn(&conn).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM profiles", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn saving_a_profile_cannot_change_its_target_or_overwrite_its_other_fields() {
    let db = Database::memory().unwrap();
    for target in [ProfileTarget::Global, ProfileTarget::Project] {
        let original = Profile {
            id: target.as_str().to_string(),
            name: "Original".into(),
            target,
            payload: "{}".into(),
            sort_order: Some(3),
            created_at: Some(10),
            updated_at: Some(20),
        };
        db.save_profile(&original).unwrap();
        let mut changed = original.clone();
        changed.target = if target == ProfileTarget::Global {
            ProfileTarget::Project
        } else {
            ProfileTarget::Global
        };
        changed.name = "Overwritten".into();
        changed.payload = "{\"changed\":true}".into();
        changed.updated_at = Some(30);
        assert!(db.save_profile(&changed).is_err());
        let saved = db.get_profile(&original.id).unwrap().unwrap();
        assert_eq!(saved.target, original.target);
        assert_eq!(saved.name, original.name);
        assert_eq!(saved.payload, original.payload);
        assert_eq!(saved.sort_order, original.sort_order);
        assert_eq!(saved.created_at, original.created_at);
        assert_eq!(saved.updated_at, original.updated_at);
        changed.target = target;
        db.save_profile(&changed).unwrap();
        let saved = db.get_profile(&original.id).unwrap().unwrap();
        assert_eq!(saved.target, target);
        assert_eq!(saved.name, changed.name);
    }
}

#[test]
fn schema_rejects_per_agent_prompts_and_missing_application_table_without_repair() {
    for change in [
        "DROP TABLE prompts;
         CREATE TABLE prompts (
             id TEXT NOT NULL, app_type TEXT NOT NULL, name TEXT NOT NULL, content TEXT NOT NULL,
             description TEXT, enabled BOOLEAN NOT NULL DEFAULT 1, created_at INTEGER, updated_at INTEGER,
             PRIMARY KEY (id, app_type)
         );
         INSERT INTO prompts (id, app_type, name, content) VALUES ('keep', 'claude', 'Keep', 'original');",
        "DROP TABLE prompt_applications;",
        "DROP TABLE prompts;
         CREATE TABLE prompts (
             id TEXT, name TEXT NOT NULL, content TEXT NOT NULL,
             description TEXT, created_at INTEGER, updated_at INTEGER
         );",
    ] {
        let conn = Connection::open_in_memory().unwrap();
        Database::create_tables_on_conn(&conn).unwrap();
        conn.execute_batch(change).unwrap();
        let schema_before: String = conn
            .query_row("SELECT group_concat(sql, ';') FROM sqlite_master", [], |row| row.get(0))
            .unwrap();
        let error = Database::create_tables_on_conn(&conn).unwrap_err();
        assert!(error.to_string().contains("提示词格式"));
        assert_eq!(Database::get_user_version(&conn).unwrap(), SCHEMA_VERSION);
        assert_eq!(
            conn.query_row("SELECT group_concat(sql, ';') FROM sqlite_master", [], |row| row.get::<_, String>(0)).unwrap(),
            schema_before
        );
        if Database::has_column(&conn, "prompts", "app_type").unwrap() {
            assert_eq!(conn.query_row("SELECT content FROM prompts WHERE id = 'keep'", [], |row| row.get::<_, String>(0)).unwrap(), "original");
        }
    }
}
