//! 当前数据库结构的初始化与版本校验。

use super::{lock_conn, Database, SCHEMA_VERSION};
use crate::error::AppError;
use rusqlite::Connection;

impl Database {
    /// 创建所有数据库表
    pub(crate) fn create_tables(&self) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        Self::create_tables_on_conn(&conn)
    }

    /// 仅初始化空数据库；已有数据库必须使用当前格式。
    pub(crate) fn create_tables_on_conn(conn: &Connection) -> Result<(), AppError> {
        let version = Self::get_user_version(conn)?;
        if Self::has_user_tables(conn)? {
            return Self::validate_schema_version_on_conn(conn);
        }
        if version != 0 {
            return Err(AppError::Database("数据库缺少当前格式的表结构".into()));
        }
        Self::in_savepoint(conn, "create_tables", || {
            Self::create_tables_on_conn_unbatched(conn)?;
            Self::set_user_version(conn, SCHEMA_VERSION)
        })
    }

    /// 在 SAVEPOINT 里执行一组写入：成功 RELEASE，失败 ROLLBACK TO 再 RELEASE。
    ///
    /// 自动提交模式下每条语句单独一个事务，都要建 / 删一次日志文件并落盘；建表和
    /// 二百多条内置定价逐条提交，在 Windows 上新库要多花约 6 秒（首次启动、每个用到
    /// 数据库的测试都会碰上）。合成一个事务只落一次盘。外面已有事务时 SAVEPOINT 会嵌套。
    pub(crate) fn in_savepoint<T>(
        conn: &Connection,
        name: &str,
        f: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        conn.execute_batch(&format!("SAVEPOINT {name};"))
            .map_err(|e| AppError::Database(format!("开启 savepoint {name} 失败: {e}")))?;
        match f() {
            Ok(value) => {
                conn.execute_batch(&format!("RELEASE {name};"))
                    .map_err(|e| AppError::Database(format!("提交 savepoint {name} 失败: {e}")))?;
                Ok(value)
            }
            Err(err) => {
                let _ = conn.execute_batch(&format!("ROLLBACK TO {name}; RELEASE {name};"));
                Err(err)
            }
        }
    }

    fn create_tables_on_conn_unbatched(conn: &Connection) -> Result<(), AppError> {
        // 1. Providers 表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS providers (
                id TEXT NOT NULL,
                app_type TEXT NOT NULL,
                name TEXT NOT NULL,
                settings_config TEXT NOT NULL,
                website_url TEXT,
                category TEXT,
                created_at INTEGER,
                sort_index INTEGER,
                notes TEXT,
                icon TEXT,
                icon_color TEXT,
                meta TEXT NOT NULL DEFAULT '{}',
                is_current BOOLEAN NOT NULL DEFAULT 0,
                in_failover_queue BOOLEAN NOT NULL DEFAULT 0,
                cost_multiplier TEXT NOT NULL DEFAULT '1.0',
                limit_daily_usd TEXT,
                limit_monthly_usd TEXT,
                provider_type TEXT,
                PRIMARY KEY (id, app_type)
            )",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        // 2. Provider Endpoints 表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS provider_endpoints (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider_id TEXT NOT NULL,
                app_type TEXT NOT NULL,
                url TEXT NOT NULL,
                added_at INTEGER,
                FOREIGN KEY (provider_id, app_type) REFERENCES providers(id, app_type) ON DELETE CASCADE
            )",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        // 3. MCP Servers 表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS mcp_servers (
            id TEXT PRIMARY KEY, name TEXT NOT NULL, server_config TEXT NOT NULL,
            description TEXT, homepage TEXT, docs TEXT, tags TEXT NOT NULL DEFAULT '[]',
            enabled_claude BOOLEAN NOT NULL DEFAULT 0, enabled_codex BOOLEAN NOT NULL DEFAULT 0,
            enabled_gemini BOOLEAN NOT NULL DEFAULT 0, enabled_grokbuild BOOLEAN NOT NULL DEFAULT 0,
            enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
            enabled_mcode BOOLEAN NOT NULL DEFAULT 0,
            enabled_agy BOOLEAN NOT NULL DEFAULT 0,
            enabled_kimi BOOLEAN NOT NULL DEFAULT 0,
            enabled_dsh BOOLEAN NOT NULL DEFAULT 0,
            enabled_copilot BOOLEAN NOT NULL DEFAULT 0,
            enabled_workbuddy BOOLEAN NOT NULL DEFAULT 0,
            enabled_hermes BOOLEAN NOT NULL DEFAULT 0,
            enabled_pi BOOLEAN NOT NULL DEFAULT 0,
            enabled_omp BOOLEAN NOT NULL DEFAULT 0
        )",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        // 4. Prompts 表
        conn.execute("CREATE TABLE IF NOT EXISTS prompts (
            id TEXT NOT NULL, app_type TEXT NOT NULL, name TEXT NOT NULL, content TEXT NOT NULL,
            description TEXT, enabled BOOLEAN NOT NULL DEFAULT 1, created_at INTEGER, updated_at INTEGER,
            PRIMARY KEY (id, app_type)
        )", []).map_err(|e| AppError::Database(e.to_string()))?;

        // 5. Skills 表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS skills (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            directory TEXT NOT NULL,
            repo_owner TEXT,
            repo_name TEXT,
            repo_branch TEXT DEFAULT 'main',
            readme_url TEXT,
            enabled_claude BOOLEAN NOT NULL DEFAULT 0,
            enabled_codex BOOLEAN NOT NULL DEFAULT 0,
            enabled_gemini BOOLEAN NOT NULL DEFAULT 0,
            enabled_grokbuild BOOLEAN NOT NULL DEFAULT 0,
            enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
            enabled_mcode BOOLEAN NOT NULL DEFAULT 0,
            enabled_agy BOOLEAN NOT NULL DEFAULT 0,
            enabled_kimi BOOLEAN NOT NULL DEFAULT 0,
            enabled_dsh BOOLEAN NOT NULL DEFAULT 0,
            enabled_copilot BOOLEAN NOT NULL DEFAULT 0,
            enabled_workbuddy BOOLEAN NOT NULL DEFAULT 0,
            enabled_hermes BOOLEAN NOT NULL DEFAULT 0,
            installed_at INTEGER NOT NULL DEFAULT 0,
            content_hash TEXT,
            updated_at INTEGER NOT NULL DEFAULT 0,
            repo_host TEXT
        )",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        // 6. Skill Repos 表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS skill_repos (
            owner TEXT NOT NULL, name TEXT NOT NULL, branch TEXT NOT NULL DEFAULT 'main',
            enabled BOOLEAN NOT NULL DEFAULT 1, host TEXT NOT NULL DEFAULT 'github.com',
            PRIMARY KEY (owner, name)
        )",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        // 7. Settings 表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        // 8. Proxy Config 表（app_type 主键）
        conn.execute("CREATE TABLE IF NOT EXISTS proxy_config (
            app_type TEXT PRIMARY KEY CHECK (app_type IN ('claude','codex','gemini','grokbuild')),
            live_takeover_active INTEGER NOT NULL DEFAULT 0,
            proxy_enabled INTEGER NOT NULL DEFAULT 0, listen_address TEXT NOT NULL DEFAULT '127.0.0.1',
            listen_port INTEGER NOT NULL DEFAULT 15721, enable_logging INTEGER NOT NULL DEFAULT 1,
            enabled INTEGER NOT NULL DEFAULT 0, auto_failover_enabled INTEGER NOT NULL DEFAULT 0,
            max_retries INTEGER NOT NULL DEFAULT 3, streaming_first_byte_timeout INTEGER NOT NULL DEFAULT 60,
            streaming_idle_timeout INTEGER NOT NULL DEFAULT 120, non_streaming_timeout INTEGER NOT NULL DEFAULT 600,
            circuit_failure_threshold INTEGER NOT NULL DEFAULT 4, circuit_success_threshold INTEGER NOT NULL DEFAULT 2,
            circuit_timeout_seconds INTEGER NOT NULL DEFAULT 60, circuit_error_rate_threshold REAL NOT NULL DEFAULT 0.6,
            circuit_min_requests INTEGER NOT NULL DEFAULT 10,
            default_cost_multiplier TEXT NOT NULL DEFAULT '1',
            pricing_model_source TEXT NOT NULL DEFAULT 'response',
            created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        )", []).map_err(|e| AppError::Database(e.to_string()))?;

        // 初始化各 Agent 的路由配置。
        conn.execute(
            "INSERT OR IGNORE INTO proxy_config (app_type, max_retries,
            streaming_first_byte_timeout, streaming_idle_timeout, non_streaming_timeout,
            circuit_failure_threshold, circuit_success_threshold, circuit_timeout_seconds,
            circuit_error_rate_threshold, circuit_min_requests)
            VALUES ('claude', 6, 90, 180, 600, 8, 3, 90, 0.7, 15)",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute(
            "INSERT OR IGNORE INTO proxy_config (app_type, max_retries,
            streaming_first_byte_timeout, streaming_idle_timeout, non_streaming_timeout,
            circuit_failure_threshold, circuit_success_threshold, circuit_timeout_seconds,
            circuit_error_rate_threshold, circuit_min_requests)
            VALUES ('codex', 3, 60, 120, 600, 4, 2, 60, 0.6, 10)",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute(
            "INSERT OR IGNORE INTO proxy_config (app_type, max_retries,
            streaming_first_byte_timeout, streaming_idle_timeout, non_streaming_timeout,
            circuit_failure_threshold, circuit_success_threshold, circuit_timeout_seconds,
            circuit_error_rate_threshold, circuit_min_requests)
            VALUES ('gemini', 5, 60, 120, 600, 4, 2, 60, 0.6, 10)",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute(
            "INSERT OR IGNORE INTO proxy_config (app_type, max_retries,
            streaming_first_byte_timeout, streaming_idle_timeout, non_streaming_timeout,
            circuit_failure_threshold, circuit_success_threshold, circuit_timeout_seconds,
            circuit_error_rate_threshold, circuit_min_requests)
            VALUES ('grokbuild', 3, 60, 120, 600, 4, 2, 60, 0.6, 10)",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        // 9. Provider Health 表
        conn.execute("CREATE TABLE IF NOT EXISTS provider_health (
            provider_id TEXT NOT NULL, app_type TEXT NOT NULL, is_healthy INTEGER NOT NULL DEFAULT 1,
            consecutive_failures INTEGER NOT NULL DEFAULT 0, last_success_at TEXT, last_failure_at TEXT,
            last_error TEXT, updated_at TEXT NOT NULL,
            PRIMARY KEY (provider_id, app_type),
            FOREIGN KEY (provider_id, app_type) REFERENCES providers(id, app_type) ON DELETE CASCADE
        )", []).map_err(|e| AppError::Database(e.to_string()))?;

        // 10. Proxy Request Logs 表
        // pricing_model = 写入时实际用于计价的模型名（pricing_model_source 解析结果），
        // 未计价的错误行使用空字符串。
        conn.execute("CREATE TABLE IF NOT EXISTS proxy_request_logs (
            request_id TEXT PRIMARY KEY, provider_id TEXT NOT NULL, app_type TEXT NOT NULL, model TEXT NOT NULL,
            request_model TEXT,
            pricing_model TEXT,
            input_tokens INTEGER NOT NULL DEFAULT 0, output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0, cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            input_token_semantics INTEGER NOT NULL DEFAULT 0,
            input_cost_usd TEXT NOT NULL DEFAULT '0', output_cost_usd TEXT NOT NULL DEFAULT '0',
            cache_read_cost_usd TEXT NOT NULL DEFAULT '0', cache_creation_cost_usd TEXT NOT NULL DEFAULT '0',
            total_cost_usd TEXT NOT NULL DEFAULT '0', latency_ms INTEGER NOT NULL, first_token_ms INTEGER,
            duration_ms INTEGER, status_code INTEGER NOT NULL, error_message TEXT, session_id TEXT,
            provider_type TEXT, is_streaming INTEGER NOT NULL DEFAULT 0,
            cost_multiplier TEXT NOT NULL DEFAULT '1.0', created_at INTEGER NOT NULL,
            data_source TEXT NOT NULL DEFAULT 'proxy'
        )", []).map_err(|e| AppError::Database(e.to_string()))?;

        conn.execute("CREATE INDEX IF NOT EXISTS idx_request_logs_provider ON proxy_request_logs(provider_id, app_type)", [])
            .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_request_logs_created_at ON proxy_request_logs(created_at)", [])
            .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_model ON proxy_request_logs(model)",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_session ON proxy_request_logs(session_id)",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_status ON proxy_request_logs(status_code)",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Self::create_request_logs_usage_indexes_if_supported(conn)?;

        // 11. Model Pricing 表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS model_pricing (
            model_id TEXT PRIMARY KEY, display_name TEXT NOT NULL,
            input_cost_per_million TEXT NOT NULL, output_cost_per_million TEXT NOT NULL,
            cache_read_cost_per_million TEXT NOT NULL DEFAULT '0',
            cache_creation_cost_per_million TEXT NOT NULL DEFAULT '0'
        )",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        // 12. Stream Check Logs 表
        conn.execute("CREATE TABLE IF NOT EXISTS stream_check_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT, provider_id TEXT NOT NULL, provider_name TEXT NOT NULL,
            app_type TEXT NOT NULL, status TEXT NOT NULL, success INTEGER NOT NULL, message TEXT NOT NULL,
            response_time_ms INTEGER, http_status INTEGER, model_used TEXT,
            retry_count INTEGER DEFAULT 0, tested_at INTEGER NOT NULL
        )", []).map_err(|e| AppError::Database(e.to_string()))?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_stream_check_logs_provider
             ON stream_check_logs(app_type, provider_id, tested_at DESC)",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        // 注意：circuit_breaker_config 已合并到 proxy_config 表中

        // 16. Proxy Live Backup 表 (Live 配置备份)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS proxy_live_backup (
            app_type TEXT PRIMARY KEY, original_config TEXT NOT NULL, backed_up_at TEXT NOT NULL
        )",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        // 17. Usage Daily Rollups 表 (日聚合统计)
        // request_model 保留路由接管的「客户端别名 → 真实模型」映射维度，
        // pricing_model 保留写入时的计价基准（request 计价模式下与 model 分叉），
        // 保留明细清理后的计费审计信息。
        conn.execute(
            "CREATE TABLE IF NOT EXISTS usage_daily_rollups (
                date TEXT NOT NULL,
                app_type TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                model TEXT NOT NULL,
                request_model TEXT NOT NULL DEFAULT '',
                pricing_model TEXT NOT NULL DEFAULT '',
                request_count INTEGER NOT NULL DEFAULT 0,
                success_count INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                input_token_semantics INTEGER NOT NULL DEFAULT 0,
                total_cost_usd TEXT NOT NULL DEFAULT '0',
                avg_latency_ms INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (date, app_type, provider_id, model, request_model, pricing_model)
            )",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        // 18. Session Log Sync 表 (会话日志同步状态)
        //
        // last_byte_offset：Claude 路径的字节游标（seek 增量读）；NULL 表示
        // 尚无字节游标（初次扫描或非 Claude 路径行），此时全量读。
        // last_tail_fingerprint：游标边界前尾部字节的指纹，用于识别文件被
        // 外部重写（同尺寸/更大的替换无法靠 size 检测）；NULL 表示无指纹
        // 可校验，按纯追加处理。
        conn.execute(
            "CREATE TABLE IF NOT EXISTS session_log_sync (
                file_path TEXT PRIMARY KEY,
                last_modified INTEGER NOT NULL,
                last_line_offset INTEGER NOT NULL DEFAULT 0,
                last_synced_at INTEGER NOT NULL,
                last_byte_offset INTEGER,
                last_tail_fingerprint INTEGER,
                parser_state TEXT
            )",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        // Session detail rows are pruned after rollup, so request IDs needed
        // for fork/rewrite deduplication live in a compact durable ledger.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS session_usage_dedup (
                data_source TEXT NOT NULL,
                request_id TEXT NOT NULL,
                semantic_id TEXT NOT NULL,
                has_entry_id INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (data_source, request_id)
            )",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_session_usage_dedup_semantic
             ON session_usage_dedup(data_source, semantic_id, has_entry_id)",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        // 19. Profiles 表（用途固定为全局或项目，payload 按 app 分槽快照
        //     供应商/MCP/Skills/Prompt；各应用分组的 current 标记在 settings 表）
        conn.execute(
            "CREATE TABLE IF NOT EXISTS profiles (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                target TEXT NOT NULL CHECK (target IN ('global', 'project')),
                payload TEXT NOT NULL,
                sort_order INTEGER,
                created_at INTEGER,
                updated_at INTEGER
            )",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        // 为故障转移队列创建索引（基于 providers 表）
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_providers_failover
             ON providers(app_type, in_failover_queue, sort_index)",
            [],
        );

        Ok(())
    }

    /// 插入默认模型定价数据
    /// 格式: (model_id, display_name, input, output, cache_read, cache_creation)
    /// 注意: model_id 使用短横线格式（如 claude-haiku-4-5），与 API 返回的模型名称标准化后一致
    fn seed_model_pricing(conn: &Connection) -> Result<(), AppError> {
        let pricing_data = [
            // Claude Fable 5.2（2026-11-11 核对官方标准价；缓存写入为 5 分钟 TTL）
            // 来源：https://www.anthropic.com/pricing#api；fast mode 为 2x 标准价，不入表。
            (
                "claude-fable-5-2",
                "Claude Fable 5.2",
                "8",
                "40",
                "0.20",
                "10",
            ),
            // Claude Fable 5.1 / Mythos 5.1（2026-09-01 发布；同 Fable 5 价，
            // 但缓存读为 0.025x = $0.25，非 Fable 5 的 $1）
            (
                "claude-fable-5-1",
                "Claude Fable 5.1",
                "10",
                "50",
                "0.25",
                "12.50",
            ),
            (
                "claude-mythos-5-1",
                "Claude Mythos 5.1",
                "10",
                "50",
                "0.25",
                "12.50",
            ),
            // Claude Fable 5（Opus 之上的新档）
            (
                "claude-fable-5",
                "Claude Fable 5",
                "10",
                "50",
                "1.00",
                "12.50",
            ),
            (
                "claude-mythos-5",
                "Claude Mythos 5",
                "10",
                "50",
                "1.00",
                "12.50",
            ),
            // Claude Opus 5.5（2026-09-23 发布；缓存读为 0.05x = $0.20，非常规 0.1x 的
            // $0.40，也非 Opus 5 的 $0.50；fast mode $8/$40 不入表）
            ("claude-opus-5-5", "Claude Opus 5.5", "4", "20", "0.20", "5"),
            // Claude Opus 5（与 Opus 4.8 同价位；fast mode $10/$50 不入表）
            ("claude-opus-5", "Claude Opus 5", "5", "25", "0.50", "6.25"),
            // Claude 4.8 系列
            (
                "claude-opus-4-8",
                "Claude Opus 4.8",
                "5",
                "25",
                "0.50",
                "6.25",
            ),
            // Claude Sonnet 5.5（2026-09-28 发布；同 Sonnet 5 价，但缓存读为
            // 0.05x = $0.10，非 Sonnet 5 的 $0.20）
            (
                "claude-sonnet-5-5",
                "Claude Sonnet 5.5",
                "2",
                "10",
                "0.10",
                "2.50",
            ),
            // Claude Haiku 5.5（2026-10-07 发布；按提示词长度分档，表内只存
            // 10 万 token 以内的标准档，超过时 $0.50/$2.50 的高档不入表）
            (
                "claude-haiku-5-5",
                "Claude Haiku 5.5",
                "0.10",
                "0.50",
                "0.01",
                "0.125",
            ),
            // Claude Sonnet 5（官方定价页 2026-09 确认：$2/$10 介绍价转为正式价，
            // 原定 09-01 涨至 $3/$15 取消）
            (
                "claude-sonnet-5",
                "Claude Sonnet 5",
                "2",
                "10",
                "0.20",
                "2.50",
            ),
            // Claude 4.7 系列
            (
                "claude-opus-4-7",
                "Claude Opus 4.7",
                "5",
                "25",
                "0.50",
                "6.25",
            ),
            // Claude 4.6 系列（裸 id 行覆盖无日期后缀的日志变体，与 dated 行同价）
            (
                "claude-opus-4-6",
                "Claude Opus 4.6",
                "5",
                "25",
                "0.50",
                "6.25",
            ),
            (
                "claude-sonnet-4-6",
                "Claude Sonnet 4.6",
                "3",
                "15",
                "0.30",
                "3.75",
            ),
            (
                "claude-opus-4-6-20260206",
                "Claude Opus 4.6",
                "5",
                "25",
                "0.50",
                "6.25",
            ),
            (
                "claude-sonnet-4-6-20260217",
                "Claude Sonnet 4.6",
                "3",
                "15",
                "0.30",
                "3.75",
            ),
            // Claude 4.5 系列
            (
                "claude-opus-4-5-20251101",
                "Claude Opus 4.5",
                "5",
                "25",
                "0.50",
                "6.25",
            ),
            (
                "claude-sonnet-4-5-20250929",
                "Claude Sonnet 4.5",
                "3",
                "15",
                "0.30",
                "3.75",
            ),
            (
                "claude-haiku-4-5-20251001",
                "Claude Haiku 4.5",
                "1",
                "5",
                "0.10",
                "1.25",
            ),
            // Claude 4 系列 (Legacy Models)
            (
                "claude-opus-4-20250514",
                "Claude Opus 4",
                "15",
                "75",
                "1.50",
                "18.75",
            ),
            (
                "claude-opus-4-1-20250805",
                "Claude Opus 4.1",
                "15",
                "75",
                "1.50",
                "18.75",
            ),
            (
                "claude-sonnet-4-20250514",
                "Claude Sonnet 4",
                "3",
                "15",
                "0.30",
                "3.75",
            ),
            // Claude 3.5 系列
            (
                "claude-3-5-haiku-20241022",
                "Claude 3.5 Haiku",
                "0.80",
                "4",
                "0.08",
                "1",
            ),
            (
                "claude-3-5-sonnet-20241022",
                "Claude 3.5 Sonnet",
                "3",
                "15",
                "0.30",
                "3.75",
            ),
            // GPT-6 系列（Astra 2026-09-04 发布，1.05M 窗口；Sol / Luna 2026-09-22 发布）
            // 2026-09-23 核对官方价页 + 模型页 + models.dev：录入 Standard 短上下文价，
            // cache read 0.1×、cache write 1.25× 输入价。>272K 长上下文档（输入与缓存 2×、输出 1.5×）、
            // Batch/Flex、Fast mode、区域加价本表无法表达，与 gpt-5.5 同样忽略。
            // effort 档 low/medium/high/xhigh 由查价剥后缀回落到本行；max 不在剥离列表
            //（会与 *-max 真 id 撞名），不另加后缀行。
            ("gpt-6-astra", "GPT-6 Astra", "10", "50", "1", "12.5"),
            // GPT-6.2 Sol / Astra（2026-11-17 发布）：2026-11-17 核对官方模型页 + 价页 + models.dev，
            // 取 Standard ≤272K 短上下文价，cache write 取 30 分钟档（12 小时档为 2 倍）。
            // 长上下文档、Batch/Fast/Ultrafast 与 astra-law 配置本表无法表达，同 GPT-6 一样忽略。
            // https://developers.openai.com/api/docs/models/gpt-6.2-sol
            // https://developers.openai.com/api/docs/models/gpt-6.2-astra
            ("gpt-6.2-sol", "GPT-6.2 Sol", "2", "10", "0.10", "2.50"),
            ("gpt-6.2-astra", "GPT-6.2 Astra", "8", "40", "0.40", "10"),
            // GPT-6.1 Sol: Standard short-context pricing; cached input is 0.05× input.
            // https://developers.openai.com/api/docs/models/gpt-6.1-sol
            ("gpt-6.1-sol", "GPT-6.1 Sol", "2", "10", "0.10", "2.50"),
            // GPT-6.1 Terra（2026-10-26 发布）：2026-11-02 核对官方模型页 + 价页，取 Standard 短上下文价，
            // cache write 取 30 分钟档（12 小时档为 2 倍）。
            // https://developers.openai.com/api/docs/models/gpt-6.1-terra
            ("gpt-6.1-terra", "GPT-6.1 Terra", "1", "5", "0.05", "1.25"),
            ("gpt-6-sol", "GPT-6 Sol", "2", "10", "0.20", "2.50"),
            ("gpt-6-luna", "GPT-6 Luna", "0.10", "0.50", "0.01", "0.125"),
            // GPT-5.6 系列（Sol / Terra / Luna，2026-06 发布）
            // 5.6 家族起 cache write 收 1.25× 输入价（此前 GPT 模型写缓存免费，勿回填旧系列）
            // 2026-09-06 审计：Sol 改促销价 4/20/0.40/5（OpenAI 价页原文"至少持续到 2026-11-21"），
            // 挂牌价 5/30/0.50/6.25。录促销价、不进豁免表：促销结束 models.dev 更新后审计会自动报出。
            ("gpt-5.6-sol", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            // 2026-07-30 OpenAI 降价：luna -80%、terra -20%，sol 不变（Fast mode 2× 价不入表）
            ("gpt-5.6-terra", "GPT-5.6 Terra", "2", "12", "0.20", "2.50"),
            (
                "gpt-5.6-luna",
                "GPT-5.6 Luna",
                "0.20",
                "1.20",
                "0.02",
                "0.25",
            ),
            // GPT-5.6 Cyber（Daybreak 计划的网安模型，需 Trusted Access；2026-09-23 核对官方价页）。
            // 别名 gpt-daybreak-red-latest / gpt-daybreak-blue-latest 当前分别指向 gpt-5.6-cyber /
            // gpt-5.6-sol，官方明说别名改指向时价格随之改变，故别名不入表。
            (
                "gpt-5.6-cyber",
                "GPT-5.6 Cyber",
                "12.50",
                "75",
                "1.25",
                "15.625",
            ),
            // 裸名 gpt-5.6 是 sol 的官方别名；effort 后缀对齐 gpt-5.5 系列的记账形态。
            // 查价先精确匹配 id 再剥 effort 后缀，这些行必须与 sol 同步改价，否则旧价会压过基础行。
            ("gpt-5.6", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            ("gpt-5.6-low", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            ("gpt-5.6-medium", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            ("gpt-5.6-high", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            ("gpt-5.6-xhigh", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            ("gpt-5.6-minimal", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            // GPT-5.5 系列
            ("gpt-5.5", "GPT-5.5", "5", "30", "0.50", "0"),
            ("gpt-5.5-low", "GPT-5.5", "5", "30", "0.50", "0"),
            ("gpt-5.5-medium", "GPT-5.5", "5", "30", "0.50", "0"),
            ("gpt-5.5-high", "GPT-5.5", "5", "30", "0.50", "0"),
            ("gpt-5.5-xhigh", "GPT-5.5", "5", "30", "0.50", "0"),
            ("gpt-5.5-minimal", "GPT-5.5", "5", "30", "0.50", "0"),
            // GPT-5.4 系列
            ("gpt-5.4", "GPT-5.4", "2.50", "15", "0.25", "0"),
            ("gpt-5.4-mini", "GPT-5.4 Mini", "0.75", "4.50", "0.075", "0"),
            ("gpt-5.4-nano", "GPT-5.4 Nano", "0.20", "1.25", "0.02", "0"),
            // GPT-5.2 系列
            ("gpt-5.2", "GPT-5.2", "1.75", "14", "0.175", "0"),
            ("gpt-5.2-low", "GPT-5.2", "1.75", "14", "0.175", "0"),
            ("gpt-5.2-medium", "GPT-5.2", "1.75", "14", "0.175", "0"),
            ("gpt-5.2-high", "GPT-5.2", "1.75", "14", "0.175", "0"),
            ("gpt-5.2-xhigh", "GPT-5.2", "1.75", "14", "0.175", "0"),
            ("gpt-5.2-codex", "GPT-5.2 Codex", "1.75", "14", "0.175", "0"),
            (
                "gpt-5.2-codex-low",
                "GPT-5.2 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            (
                "gpt-5.2-codex-medium",
                "GPT-5.2 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            (
                "gpt-5.2-codex-high",
                "GPT-5.2 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            (
                "gpt-5.2-codex-xhigh",
                "GPT-5.2 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            // GPT-5.3 Codex 系列
            ("gpt-5.3-codex", "GPT-5.3 Codex", "1.75", "14", "0.175", "0"),
            (
                "gpt-5.3-codex-spark",
                "GPT-5.3 Codex Spark",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            (
                "gpt-5.3-codex-low",
                "GPT-5.3 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            (
                "gpt-5.3-codex-medium",
                "GPT-5.3 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            (
                "gpt-5.3-codex-high",
                "GPT-5.3 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            (
                "gpt-5.3-codex-xhigh",
                "GPT-5.3 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            // GPT-5.1 系列
            ("gpt-5.1", "GPT-5.1", "1.25", "10", "0.125", "0"),
            ("gpt-5.1-low", "GPT-5.1", "1.25", "10", "0.125", "0"),
            ("gpt-5.1-medium", "GPT-5.1", "1.25", "10", "0.125", "0"),
            ("gpt-5.1-high", "GPT-5.1", "1.25", "10", "0.125", "0"),
            ("gpt-5.1-minimal", "GPT-5.1", "1.25", "10", "0.125", "0"),
            ("gpt-5.1-codex", "GPT-5.1 Codex", "1.25", "10", "0.125", "0"),
            (
                "gpt-5.1-codex-mini",
                "GPT-5.1 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gpt-5.1-codex-max",
                "GPT-5.1 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gpt-5.1-codex-max-high",
                "GPT-5.1 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gpt-5.1-codex-max-xhigh",
                "GPT-5.1 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            // GPT-5 系列
            ("gpt-5", "GPT-5", "1.25", "10", "0.125", "0"),
            ("gpt-5-low", "GPT-5", "1.25", "10", "0.125", "0"),
            ("gpt-5-medium", "GPT-5", "1.25", "10", "0.125", "0"),
            ("gpt-5-high", "GPT-5", "1.25", "10", "0.125", "0"),
            ("gpt-5-minimal", "GPT-5", "1.25", "10", "0.125", "0"),
            ("gpt-5-codex", "GPT-5 Codex", "1.25", "10", "0.125", "0"),
            ("gpt-5-codex-low", "GPT-5 Codex", "1.25", "10", "0.125", "0"),
            (
                "gpt-5-codex-medium",
                "GPT-5 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gpt-5-codex-high",
                "GPT-5 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gpt-5-codex-mini",
                "GPT-5 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gpt-5-codex-mini-medium",
                "GPT-5 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gpt-5-codex-mini-high",
                "GPT-5 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            // OpenAI Reasoning 系列
            ("o3", "OpenAI o3", "2", "8", "0.50", "0"),
            ("o4-mini", "OpenAI o4-mini", "1.10", "4.40", "0.275", "0"),
            // GPT-4.1 系列
            ("gpt-4.1", "GPT-4.1", "2", "8", "0.50", "0"),
            ("gpt-4.1-mini", "GPT-4.1 Mini", "0.40", "1.60", "0.10", "0"),
            ("gpt-4.1-nano", "GPT-4.1 Nano", "0.10", "0.40", "0.025", "0"),
            // OpenAI 文本模型补全（2026-09-23）：各模型页的标准 token 价。
            // 来源：https://developers.openai.com/api/docs/models/<model_id>
            // 已按 /api/docs/deprecations 排除弃用模型；不含工具调用费及多模态价格。
            // Pro 系列未提供缓存折扣，与 o3-pro 一样将未支持的缓存价格记为 0。
            // 有意不收：chat-latest 是滚动别名（改指向即改价）；gpt-rosalind-research 官方
            // 2026-10-05 才开始计费且仅限 Trusted Access，提前入表会给免费期用量记账。
            ("gpt-5.5-pro", "GPT-5.5 Pro", "30", "180", "0", "0"),
            ("gpt-5.4-pro", "GPT-5.4 Pro", "30", "180", "0", "0"),
            ("gpt-5.2-pro", "GPT-5.2 Pro", "21", "168", "0", "0"),
            ("gpt-4o", "GPT-4o", "2.50", "10", "1.25", "0"),
            ("gpt-4o-mini", "GPT-4o Mini", "0.15", "0.60", "0.075", "0"),
            // Gemini 4 系列
            // 来源：https://ai.google.dev/gemini-api/docs/pricing#gemini-4-neon
            // 标准档（Global）：输入 3.00 / 输出 15.00（含思考）/ 缓存读 0.15（仅隐式缓存，写入记 0），
            // 与 models.dev google/gemini-4-neon 一致。
            (
                "gemini-4-neon",
                "Gemini 4 Neon",
                "3.00",
                "15.00",
                "0.15",
                "0",
            ),
            // 来源：https://ai.google.dev/gemini-api/docs/pricing#gemini-4-argon
            // 录的是标准档介绍价（2026-10-15 抓取）：输入 2.00 / 输出 10.00 / 上下文缓存读 0.10
            // （缓存仅支持隐式，不支持显式写入，cache_creation 记 0，与其他 Gemini 条目口径一致）。
            // ⚠️ 介绍价 2026-11-30 到期，2026-12-01 起恢复挂牌价 4.00/20.00/0.20。
            // 到期后需走 seed + repair 双写改回；刻意不进 audit-ignore.json，
            // 由 /jason-update-model 审计的 A 段在到期后报出提醒，口径同 3.6/3.7/3.8 Flash。
            // 免费层级不可用；批量档（输入 1.00→2.00 / 输出 5.00→10.00）与 Flex 档不入表，
            // 仓库现有口径仅录标准档。
            (
                "gemini-4-argon",
                "Gemini 4 Argon",
                "2.00",
                "10.00",
                "0.10",
                "0",
            ),
            // Gemini 4 Carbon（2026-11-11 核对官方价页 + models.dev）：与 Argon 同口径录标准档介绍价
            // 2.00/10.00/0.10/0（仅支持隐式缓存）；2026-11-30 到期，2026-12-01 起恢复挂牌价
            // 4.00/20.00/0.20，刻意不进 audit-ignore.json。
            // 来源：https://ai.google.dev/gemini-api/docs/pricing#gemini-4-carbon
            (
                "gemini-4-carbon",
                "Gemini 4 Carbon",
                "2.00",
                "10.00",
                "0.10",
                "0",
            ),
            // Hydrogen executes Orchestrator steps; standard API-equivalent estimate, not plan quota.
            // https://aistudio.google.com/docs/pricing#gemini-4-hydrogen (2026-11-13)
            // Output includes thinking; implicit cache only, so no cache-write charge.
            (
                "gemini-4-hydrogen",
                "Gemini 4 Hydrogen",
                "1.50",
                "7.50",
                "0.075",
                "0",
            ),
            // Gemini 3.8 系列（2026-09-02 发布，1M 窗口）
            // 介绍价 0.75/3.75/0.075 至 2026-12-31，2027-01-01 起挂牌价 1.50/7.50/0.15；口径同 3.7 Flash，勿加豁免。
            (
                "gemini-3.8-flash",
                "Gemini 3.8 Flash",
                "0.75",
                "3.75",
                "0.075",
                "0",
            ),
            // Gemini 3.7 系列
            // 录的是介绍价（官方公告 + ai.google.dev 价表 + models.dev 三源一致）。
            // ⚠️ 介绍价 2026-12-31 到期，2027-01-01 起恢复挂牌价 1.50/7.50/0.15（3.6/3.8 Flash 同此规则）。
            // 到期后需走 seed + repair 双写改回；届时 models.dev 会先更新，
            // /jason-update-model 审计的 A 段会自动报出这一行作为提醒——
            // 因此这一行刻意不进 audit-ignore.json，勿加豁免（会屏蔽掉该提醒）。
            (
                "gemini-3.7-flash",
                "Gemini 3.7 Flash",
                "0.75",
                "3.75",
                "0.075",
                "0",
            ),
            // Gemini 3.6 系列
            // 2026-09-06 审计：Google 价页已把 3.6 Flash 也改成介绍价 0.75/3.75/0.075（至 2026-12-31），
            // 2027-01-01 起恢复挂牌价 1.50/7.50/0.15。与 3.7/3.8 Flash 同口径，刻意不进 audit-ignore.json。
            (
                "gemini-3.6-flash",
                "Gemini 3.6 Flash",
                "0.75",
                "3.75",
                "0.075",
                "0",
            ),
            // Gemini 3.5 系列
            (
                "gemini-3.5-flash",
                "Gemini 3.5 Flash",
                "1.50",
                "9.00",
                "0.15",
                "0",
            ),
            (
                "gemini-3.5-flash-lite",
                "Gemini 3.5 Flash Lite",
                "0.30",
                "2.50",
                "0.03",
                "0",
            ),
            // Gemini 3.1 系列
            (
                "gemini-3.1-pro-preview",
                "Gemini 3.1 Pro Preview",
                "2",
                "12",
                "0.20",
                "0",
            ),
            (
                "gemini-3.1-flash-lite",
                "Gemini 3.1 Flash Lite",
                "0.25",
                "1.50",
                "0.025",
                "0",
            ),
            (
                "gemini-3.1-flash-lite-preview",
                "Gemini 3.1 Flash Lite Preview",
                "0.25",
                "1.50",
                "0.025",
                "0",
            ),
            // Gemini 3 系列
            (
                "gemini-3-pro-preview",
                "Gemini 3 Pro Preview",
                "2",
                "12",
                "0.2",
                "0",
            ),
            (
                "gemini-3-flash-preview",
                "Gemini 3 Flash Preview",
                "0.5",
                "3",
                "0.05",
                "0",
            ),
            // Gemini 2.5 系列
            (
                "gemini-2.5-pro",
                "Gemini 2.5 Pro",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gemini-2.5-flash",
                "Gemini 2.5 Flash",
                "0.3",
                "2.5",
                "0.03",
                "0",
            ),
            (
                "gemini-2.5-flash-lite",
                "Gemini 2.5 Flash Lite",
                "0.10",
                "0.40",
                "0.01",
                "0",
            ),
            // Gemini 2.0 系列
            (
                "gemini-2.0-flash",
                "Gemini 2.0 Flash",
                "0.10",
                "0.40",
                "0.025",
                "0",
            ),
            // StepFun 系列：CNY 按 1 USD ≈ 7.14 CNY 折算，保留两位小数。
            // Step 5 Preview 官方输入 / 输出 / 缓存读取：7 / 20 / 0.70 元（缓存读取 2026-10-14
            // 起由 0.35 元上调；此前已按量调用过的老用户仍按 0.35 元计，表里取现行挂牌价）。
            (
                "step-5-preview",
                "Step 5 Preview",
                "0.98",
                "2.80",
                "0.10",
                "0",
            ),
            (
                "step-3.7-flash",
                "Step 3.7 Flash",
                "0.19",
                "1.13",
                "0.04",
                "0",
            ),
            (
                "step-3.5-flash",
                "Step 3.5 Flash",
                "0.10",
                "0.30",
                "0.02",
                "0",
            ),
            (
                "step-3.5-flash-2603",
                "Step 3.5 Flash 2603",
                "0.10",
                "0.30",
                "0.02",
                "0",
            ),
            // ====== 国产模型 (USD/1M tokens) ======
            // Doubao (字节跳动)
            // Seed 2.1 系列（2026-06 火山引擎官方 list 价，CNY 按 ~7.14 折算）：
            //   pro   输入 6 元 / 输出 30 元 / 命中 1.2 元
            //   turbo 输入 3 元 / 输出 15 元 / 命中 0.6 元
            // 「缓存存储 0.017 元/M/小时」是按时长计费的存储费，与本表 cache_creation（按 token 写入价）口径不同，置 0。
            (
                "doubao-seed-2-1-pro",
                "Doubao Seed 2.1 Pro",
                "0.84",
                "4.2",
                "0.17",
                "0",
            ),
            (
                "doubao-seed-2-1-turbo",
                "Doubao Seed 2.1 Turbo",
                "0.42",
                "2.1",
                "0.08",
                "0",
            ),
            (
                "doubao-seed-code",
                "Doubao Seed Code",
                "0.17",
                "1.11",
                "0.02",
                "0",
            ),
            (
                "doubao-seed-2-0-pro",
                "Doubao Seed 2.0 Pro",
                "0.47",
                "2.37",
                "0.09",
                "0",
            ),
            (
                "doubao-seed-2-0-code",
                "Doubao Seed 2.0 Code",
                "0.47",
                "2.37",
                "0.09",
                "0",
            ),
            (
                "doubao-seed-2-0-code-preview-latest",
                "Doubao Seed 2.0 Code Preview",
                "0.47",
                "2.37",
                "0.09",
                "0",
            ),
            (
                "doubao-seed-2-0-lite",
                "Doubao Seed 2.0 Lite",
                "0.08",
                "0.50",
                "0.017",
                "0",
            ),
            (
                "doubao-seed-2-0-mini",
                "Doubao Seed 2.0 Mini",
                "0.03",
                "0.31",
                "0.0056",
                "0",
            ),
            // DeepSeek 系列
            (
                "deepseek-v3.2",
                "DeepSeek V3.2",
                "0.28",
                "0.42",
                "0.028",
                "0",
            ),
            (
                "deepseek-v3.1",
                "DeepSeek V3.1",
                "0.55",
                "1.67",
                "0.055",
                "0",
            ),
            ("deepseek-v3", "DeepSeek V3", "0.28", "1.11", "0.028", "0"),
            // ── DeepSeek V4 系列：2026-08-16 16:00 UTC 起改为峰谷双档计价 ──
            // 官方价页（api-docs.deepseek.com/quick_start/pricing，中英一致）直接挂 USD，
            // 不再需要 CNY 折算。高峰时段 = 北京时间 9:00-12:00 与 14:00-18:00
            // （= UTC 01:00-04:00、06:00-10:00），共 7h/天；其余 17h 为空闲档。
            //
            // 🔴 本表每模型仅一行、无时段维度，**统一录高峰档**（Jason 2026-08-18 拍板）：
            //   ① 官方措辞是「空闲价为高峰价的一半」，高峰档才是基准挂牌价；
            //   ② 高峰时段正是中文用户的工作时间，是 AI 编程主力时段。
            //   代价=夜间/凌晨用量高估一倍。勿按「阶梯取低档」惯例改成空闲档。
            //
            // input=缓存未命中价，cache_read=缓存命中价；DeepSeek 不单收 cache write → 0。
            //
            // ── 2026-09-11：V4 Flash 退役，三个 id 全部由 DeepSeek-V4.1-Flash 承接 ──
            // 官方价页原文：legacy names `deepseek-v4-flash` / `deepseek-v4-flash-vision-exp`
            // 仍被接受，但「the corresponding models have been retired」，请求由 V4.1-Flash 服务
            // 并按 Flash 价计费 → 三者同价。V4.1 Flash 高峰档 0.3/1.2/0.006（空闲档 0.15/0.6/0.003
            // 恰为一半；models.dev 录的正是空闲档，故审计 A 段会长期报这几行，属预期）。
            // deepseek-flash 是官方当前唯一推荐名，必须单列：查价前缀兜底是 LIKE '{id}-%'，
            // 只命中更长的行，短 id 匹配不到 deepseek-v4-flash，缺行即静默按 0 计费。
            //
            // 🔴 deepseek-chat / deepseek-reasoner 停在 V4 Flash 高峰档不动（2026-09-11 复核）：
            // 官方文档站已全站搜不到这两个 id、models.dev 第一方条目也已删除 —— 无权威源可证
            // 「跟随 V4.1 Flash 降价」或「已下线」任一方向，按无源不动原则保留旧值。
            (
                "deepseek-chat",
                "DeepSeek Chat",
                "0.44",
                "1.32",
                "0.014",
                "0",
            ),
            (
                "deepseek-reasoner",
                "DeepSeek Reasoner",
                "0.44",
                "1.32",
                "0.014",
                "0",
            ),
            (
                "deepseek-flash",
                "DeepSeek V4.1 Flash",
                "0.3",
                "1.2",
                "0.006",
                "0",
            ),
            // 第三方上游（OpenCode Go、OpenRouter 的 deepseek/ 命名空间、Hermes 预设等）用带版本号的
            // deepseek-v4.1-flash。点号版本既剥不成 deepseek-flash，前缀兜底也只命中更长的行，
            // 不单列会显示「未定价」并按 0 计费
            (
                "deepseek-v4.1-flash",
                "DeepSeek V4.1 Flash",
                "0.3",
                "1.2",
                "0.006",
                "0",
            ),
            (
                "deepseek-v4-flash",
                "DeepSeek V4 Flash",
                "0.3",
                "1.2",
                "0.006",
                "0",
            ),
            // 部分上游（如阿里百炼）回传 4 位 MMDD 日期变体。查价的
            // strip_model_date_suffix 只剥 ISO / 8 位 YYYYMMDD / 6 位 YYMMDD，
            // 剥不到裸 id，前缀兜底也只匹配更长的行 —— 不补别名会静默按 0 计费
            (
                "deepseek-v4-flash-0731",
                "DeepSeek V4 Flash",
                "0.3",
                "1.2",
                "0.006",
                "0",
            ),
            // 旧视觉实验名，官方定价页明示「仍被接受、由 V4.1-Flash 承接并按 Flash 价计费」。
            // 官方安装脚本 ≤1.2.0 写过这个 id，存量供应商仍在用；前缀兜底匹配不到更短的
            // deepseek-v4-flash，不单列会静默按 0 计费
            (
                "deepseek-v4-flash-vision-exp",
                "DeepSeek V4 Flash Vision Exp",
                "0.3",
                "1.2",
                "0.006",
                "0",
            ),
            // V4 Pro 高峰档 1.32/3.96/0.044（CNY 9/27/0.3）。官方 2026-09-12 撤回了「09-14 起
            // 路由到 V4.1 Flash」的公告，定价页注(2)：9 月 14 日之后继续提供 V4 Pro API，
            // 计费方式保持不变（2026-09-15 复核）。
            // 🔴 勿按未生效的厂商公告提前改价：v3.20.3 曾因此发出 0.3/1.2/0.006 错价。
            (
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "1.32",
                "3.96",
                "0.044",
                "0",
            ),
            // Kimi (月之暗面)
            (
                "kimi-k2-thinking",
                "Kimi K2 Thinking",
                "0.55",
                "2.20",
                "0.10",
                "0",
            ),
            ("kimi-k2-0905", "Kimi K2", "0.55", "2.20", "0.10", "0"),
            (
                "kimi-k2-turbo",
                "Kimi K2 Turbo",
                "1.11",
                "8.06",
                "0.14",
                "0",
            ),
            ("kimi-k2.5", "Kimi K2.5", "0.60", "3.00", "0.10", "0"),
            ("kimi-k2.6", "Kimi K2.6", "0.95", "4.00", "0.16", "0"),
            (
                "kimi-k2.7-code",
                "Kimi K2.7 Code",
                "0.95",
                "4.00",
                "0.19",
                "0",
            ),
            // HighSpeed 加速档=本体 2 倍价（Kimi 官方一贯模式，同 K2 Turbo）
            (
                "kimi-k2.7-code-highspeed",
                "Kimi K2.7 Code HighSpeed",
                "1.90",
                "8.00",
                "0.38",
                "0",
            ),
            // 2026-10-19 官方价页：K3 缓存写入按 TTL 计费，默认 5 分钟档 $3（1 小时档 $6，取默认档）
            ("kimi-k3", "Kimi K3", "3.00", "15.00", "0.30", "3.00"),
            // Kimi For Coding 套餐里 K3 的裸名（无 kimi- 前缀），同标准 list 价
            ("k3", "Kimi K3", "3.00", "15.00", "0.30", "3.00"),
            // 2026-10-24 官方价页：K3.1 Pro 缓存写入取默认 5 分钟档（1 小时档为 2 倍）。
            // HighSpeed 只有输入/输出/缓存写翻倍，缓存读仍是 $0.20，别按 K2.7 的 2 倍规律改
            (
                "kimi-k3.1-pro",
                "Kimi K3.1 Pro",
                "2.00",
                "10.00",
                "0.20",
                "2.00",
            ),
            (
                "kimi-k3.1-pro-highspeed",
                "Kimi K3.1 Pro HighSpeed",
                "4.00",
                "20.00",
                "0.20",
                "4.00",
            ),
            // Kimi Code 套餐里的裸名（无 kimi- 前缀），同标准 list 价
            ("k3.1-pro", "Kimi K3.1 Pro", "2.00", "10.00", "0.20", "2.00"),
            (
                "k3.1-pro-highspeed",
                "Kimi K3.1 Pro HighSpeed",
                "4.00",
                "20.00",
                "0.20",
                "4.00",
            ),
            // 腾讯混元 (Tencent Hunyuan)（官方 CNY 1/4/0.25 按 1 USD ≈ 7.14 折算；Hy3 阶梯计价取最低档）
            ("hunyuan-hy3", "Hunyuan Hy3", "0.14", "0.56", "0.035", "0"),
            ("hy3", "Hunyuan Hy3", "0.14", "0.56", "0.035", "0"),
            // Hy4 preview：官方广州地域 CNY 6/18/0.3（1823/130055，2026-09-11 版）按 7.14 折算，无阶梯
            (
                "hy4-preview",
                "Hunyuan Hy4 Preview",
                "0.84",
                "2.52",
                "0.042",
                "0",
            ),
            // MiniMax 系列
            // 2026-09-06 审计：官方按量价页（platform.minimax.io/docs/guides/pricing-paygo）
            // M2 / M2.1 / M2.5 均为 0.3/1.2/0.03/0.375，models.dev 一致；旧值 0.27/0.95 与 0.15 为早期误录。
            (
                "minimax-m2.1",
                "MiniMax M2.1",
                "0.30",
                "1.20",
                "0.03",
                "0.375",
            ),
            (
                "minimax-m2.1-lightning",
                "MiniMax M2.1 Lightning",
                "0.27",
                "2.33",
                "0.03",
                "0",
            ),
            ("minimax-m2", "MiniMax M2", "0.30", "1.20", "0.03", "0.375"),
            (
                "minimax-m2.5",
                "MiniMax M2.5",
                "0.30",
                "1.20",
                "0.03",
                "0.375",
            ),
            (
                "minimax-m2.5-lightning",
                "MiniMax M2.5 Lightning",
                "0.30",
                "2.40",
                "0.03",
                "0",
            ),
            (
                "minimax-m2.7",
                "MiniMax M2.7",
                "0.30",
                "1.20",
                "0.06",
                "0.375",
            ),
            (
                "minimax-m2.7-highspeed",
                "MiniMax M2.7 Highspeed",
                "0.60",
                "2.40",
                "0.06",
                "0.375",
            ),
            ("minimax-m3", "MiniMax M3", "0.30", "1.20", "0.06", "0"),
            (
                "minimax-m3.1-flash",
                "MiniMax M3.1 Flash",
                "0.15",
                "0.60",
                "0.015",
                "0",
            ),
            // GLM (智谱)
            ("glm-4.7", "GLM-4.7", "0.6", "2.2", "0.11", "0"),
            ("glm-4.6", "GLM-4.6", "0.6", "2.2", "0.11", "0"),
            ("glm-5", "GLM-5", "1", "3.2", "0.2", "0"),
            ("glm-5.1", "GLM-5.1", "1.4", "4.4", "0.26", "0"),
            ("glm-5.2", "GLM-5.2", "1.4", "4.4", "0.26", "0"),
            ("glm-5.3", "GLM-5.3", "1.4", "4.4", "0.26", "0"),
            (
                "glm-5.3-flash",
                "GLM-5.3-Flash",
                "0.15",
                "0.50",
                "0.03",
                "0",
            ),
            (
                "glm-5.3-flashx",
                "GLM-5.3-FlashX",
                "0.37",
                "1.25",
                "0.075",
                "0",
            ),
            ("glm-5-turbo", "GLM-5-Turbo", "1.2", "4", "0.24", "0"),
            ("glm-5v-turbo", "GLM-5V-Turbo", "1.2", "4", "0.24", "0"),
            // MiMo (小米)
            (
                "mimo-v2-flash",
                "MiMo V2 Flash",
                "0.09",
                "0.29",
                "0.009",
                "0",
            ),
            ("mimo-v2-pro", "MiMo V2 Pro", "0.435", "0.87", "0.0036", "0"),
            ("mimo-v2.5", "MiMo V2.5", "0.14", "0.28", "0.0028", "0"),
            (
                "mimo-v2.5-pro",
                "MiMo V2.5 Pro",
                "0.435",
                "0.87",
                "0.0036",
                "0",
            ),
            // Qwen 系列 (阿里巴巴)
            // 2026-11-16：https://www.qwencloud.com/models/qwen4-max（新加坡，USD/百万 token）；
            // 无阶梯，思考/非思考同价；缓存写入取默认 5 分钟档（1 小时档 $3）。
            ("qwen4-max", "Qwen4 Max", "1.50", "4.50", "0.075", "1.875"),
            ("qwen3.8-max", "Qwen3.8 Max", "2", "6", "0.25", "2.50"),
            // 2026-09-06：阿里国际站价页 0.15/0.47 全区间（0<Token≤1M）平价、无阶梯；
            // 缓存两列官方只注明"非常规比例"未给数字，取 models.dev（与 qwen3.8-max 同口径）
            (
                "qwen3.8-flash",
                "Qwen3.8 Flash",
                "0.15",
                "0.47",
                "0.016",
                "0.20",
            ),
            // 2026-09-15：开放权重两款，取阿里国际站（新加坡）模型页单价，无阶梯；缓存两列为
            // 隐式缓存命中价与显式缓存创建价，与 qwen3.8-max 同口径
            (
                "qwen3.8-2.4t-a95b",
                "Qwen3.8 2.4T A95B",
                "2",
                "6",
                "0.25",
                "2.50",
            ),
            ("qwen3.8-27b", "Qwen3.8 27B", "0.50", "3", "0.10", "0.625"),
            ("qwen3.7-max", "Qwen3.7 Max", "2.50", "7.50", "0.25", "0"),
            ("qwen3.7-plus", "Qwen3.7 Plus", "0.40", "1.60", "0.08", "0"),
            // 2026-10-19：阿里国际站（新加坡）价页。qwen3.7-flash 阶梯计价取最低档（≤32K），
            // 缓存两列按 qwen3.8 同口径（隐式命中 20%、显式创建 125%）；qwen3.6-27b 无阶梯、
            // 价页未列缓存价；qwen3.8-omni-flash 取文本价，价页只列缓存命中价
            (
                "qwen3.7-flash",
                "Qwen3.7 Flash",
                "0.03",
                "0.13",
                "0.006",
                "0.0375",
            ),
            ("qwen3.6-27b", "Qwen3.6 27B", "0.60", "3.60", "0", "0"),
            (
                "qwen3.8-omni-flash",
                "Qwen3.8 Omni Flash",
                "0.15",
                "0.47",
                "0.016",
                "0",
            ),
            (
                "qwen3.6-plus",
                "Qwen3.6 Plus",
                "0.325",
                "1.95",
                "0.065",
                "0",
            ),
            (
                "qwen3.6-flash",
                "Qwen3.6 Flash",
                "0.1875",
                "1.125",
                "0.0375",
                "0",
            ),
            ("qwen3.5-plus", "Qwen3.5 Plus", "0.26", "1.56", "0.052", "0"),
            ("qwen3-max", "Qwen3 Max", "0.78", "3.90", "0", "0"),
            (
                "qwen3-235b-a22b",
                "Qwen3 235B-A22B",
                "0.70",
                "8.40",
                "0",
                "0",
            ),
            (
                "qwen3-coder-plus",
                "Qwen3 Coder Plus",
                "0.65",
                "3.25",
                "0.13",
                "0",
            ),
            (
                "qwen3-coder-480b",
                "Qwen3 Coder 480B",
                "0.65",
                "3.25",
                "0",
                "0",
            ),
            (
                "qwen3-coder-480b-a35b-instruct",
                "Qwen3 Coder 480B-A35B Instruct",
                "0.65",
                "3.25",
                "0",
                "0",
            ),
            (
                "qwen3-coder-flash",
                "Qwen3 Coder Flash",
                "0.195",
                "0.975",
                "0.039",
                "0",
            ),
            (
                "qwen3-coder-next",
                "Qwen3 Coder Next",
                "0.12",
                "0.75",
                "0",
                "0",
            ),
            ("qwq-plus", "QwQ Plus", "0.80", "2.40", "0", "0"),
            ("qwq-32b", "QwQ 32B", "0.20", "0.60", "0", "0"),
            ("qwen3-32b", "Qwen3 32B", "0.16", "0.64", "0", "0"),
            // Grok 系列 (xAI)
            // 4.5/4.6/4.7 均为分档计价：prompt ≥200K 时单价翻倍（4/12，cached 亦翻倍）。
            // 本表无档位列，统一取基础档（<200K），与其它分档厂商口径一致
            ("grok-4.7", "Grok 4.7", "2", "6", "0.50", "0"),
            ("grok-4.6", "Grok 4.6", "2", "6", "0.50", "0"),
            ("grok-4.5", "Grok 4.5", "2", "6", "0.30", "0"),
            // Grok CLI 官方 OAuth 态 modelUsage 上报的内部别名。定价由
            // costUsdTicks（1 tick = 1e-10 USD）双轮实测反推：input/output 与
            // grok-4.5 同为 2/6，cache read 同为 0.30
            ("grok-4.5-build", "Grok 4.5 Build", "2", "6", "0.30", "0"),
            // grok-4.6-build 同理：查价层不剥 -build，且 grok- 不走前缀匹配，
            // 必须单独 seed，否则会话导入只能用 CLI 自报 total、分项成本为 0
            ("grok-4.6-build", "Grok 4.6 Build", "2", "6", "0.50", "0"),
            ("grok-4.7-build", "Grok 4.7 Build", "2", "6", "0.50", "0"),
            // docs.x.ai：Grok 4.7 Fast（仅 Cursor / Grok Build）<200k 档 $4 / $12，cached $1
            (
                "grok-4.7-build-fast",
                "Grok 4.7 Build Fast",
                "4",
                "12",
                "1",
                "0",
            ),
            ("grok-4.3", "Grok 4.3", "1.25", "2.50", "0.20", "0"),
            (
                "grok-4.20-0309-reasoning",
                "Grok 4.20 Reasoning",
                "1.25",
                "2.50",
                "0.20",
                "0",
            ),
            (
                "grok-4.20-0309-non-reasoning",
                "Grok 4.20",
                "1.25",
                "2.50",
                "0.20",
                "0",
            ),
            (
                "grok-4-1-fast-reasoning",
                "Grok 4.1 Fast Reasoning",
                "0.20",
                "0.50",
                "0.05",
                "0",
            ),
            (
                "grok-4-1-fast-non-reasoning",
                "Grok 4.1 Fast",
                "0.20",
                "0.50",
                "0.05",
                "0",
            ),
            ("grok-4", "Grok 4", "3", "15", "0.75", "0"),
            (
                "grok-code-fast-1",
                "Grok Build 0.1 (Code Fast Alias)",
                "1",
                "2",
                "0.20",
                "0",
            ),
            ("grok-build-0.1", "Grok Build 0.1", "1", "2", "0.20", "0"),
            ("grok-3", "Grok 3", "3", "15", "0.75", "0"),
            ("grok-3-mini", "Grok 3 Mini", "0.25", "0.50", "0.075", "0"),
            // Mistral 系列
            (
                "mistral-medium-3.5",
                "Mistral Medium 3.5",
                "1.50",
                "7.50",
                "0",
                "0",
            ),
            (
                "mistral-small-4",
                "Mistral Small 4",
                "0.10",
                "0.30",
                "0.01",
                "0",
            ),
            (
                "devstral-small-2-2512",
                "Devstral Small 2",
                "0.10",
                "0.30",
                "0.01",
                "0",
            ),
            (
                "magistral-small",
                "Magistral Small",
                "0.50",
                "1.50",
                "0",
                "0",
            ),
            ("codestral-2508", "Codestral", "0.30", "0.90", "0.03", "0"),
            (
                "devstral-small-1.1",
                "Devstral Small 1.1",
                "0.07",
                "0.28",
                "0.01",
                "0",
            ),
            ("devstral-2-2512", "Devstral 2", "0.40", "2", "0.04", "0"),
            (
                "devstral-medium",
                "Devstral Medium",
                "0.40",
                "2",
                "0.04",
                "0",
            ),
            (
                "mistral-large-3-2512",
                "Mistral Large 3",
                "0.50",
                "1.50",
                "0.05",
                "0",
            ),
            (
                "mistral-medium-3.1",
                "Mistral Medium 3.1",
                "0.40",
                "2",
                "0.04",
                "0",
            ),
            (
                "mistral-small-3.2-24b",
                "Mistral Small 3.2",
                "0.075",
                "0.20",
                "0.01",
                "0",
            ),
            ("magistral-medium", "Magistral Medium", "2", "5", "0", "0"),
            // Cohere 系列
            ("command-a", "Cohere Command A", "2.50", "10", "0", "0"),
            (
                "command-r-plus",
                "Cohere Command R+",
                "2.50",
                "10",
                "0",
                "0",
            ),
            ("command-r", "Cohere Command R", "0.15", "0.60", "0", "0"),
            // OpenAI 补充
            ("o3-pro", "OpenAI o3-pro", "20", "80", "0", "0"),
            ("o3-mini", "OpenAI o3-mini", "1.10", "4.40", "0.55", "0"),
            ("o1", "OpenAI o1", "15", "60", "7.50", "0"),
            ("o1-mini", "OpenAI o1-mini", "0.55", "2.20", "0.55", "0"),
            ("codex-mini", "Codex Mini", "0.75", "3", "0.025", "0"),
            ("gpt-5-mini", "GPT-5 Mini", "0.25", "2", "0.025", "0"),
            ("gpt-5-nano", "GPT-5 Nano", "0.05", "0.40", "0.005", "0"),
            // MiMo 2.6：2026-09-23 核对官方标准价格，单位 USD / 百万 tokens。
            (
                "mimo-v2.6-pro",
                "MiMo V2.6 Pro",
                "0.435",
                "0.87",
                "0.0036",
                "0",
            ),
            (
                "mimo-v2.6-flash",
                "MiMo V2.6 Flash",
                "0.14",
                "0.28",
                "0.0028",
                "0",
            ),
            (
                "mimo-v2.6-pro-ultraspeed",
                "MiMo V2.6 Pro UltraSpeed",
                "4.35",
                "8.7",
                "0.036",
                "0",
            ),
        ];

        let mut stmt = conn
            .prepare(
                "INSERT OR IGNORE INTO model_pricing (
                    model_id, display_name, input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .map_err(|e| AppError::Database(format!("准备模型定价语句失败: {e}")))?;
        for (model_id, display_name, input, output, cache_read, cache_creation) in pricing_data {
            stmt.execute(rusqlite::params![
                model_id,
                display_name,
                input,
                output,
                cache_read,
                cache_creation
            ])
            .map_err(|e| AppError::Database(format!("插入模型定价失败: {e}")))?;
        }

        log::info!("已插入 {} 条默认模型定价数据", pricing_data.len());
        Ok(())
    }

    /// 确保模型定价表具备默认数据
    pub fn ensure_model_pricing_seeded(&self) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        Self::ensure_model_pricing_seeded_on_conn(&conn)
    }

    pub(crate) fn ensure_model_pricing_seeded_on_conn(conn: &Connection) -> Result<(), AppError> {
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_model_pricing_nocase ON model_pricing(model_id COLLATE NOCASE)",
            [],
        ).map_err(|e| AppError::Database(e.to_string()))?;

        Self::in_savepoint(conn, "seed_model_pricing", || {
            Self::seed_model_pricing(conn)
        })
    }

    // --- 辅助方法 ---

    pub(crate) fn get_user_version(conn: &Connection) -> Result<i32, AppError> {
        conn.query_row("PRAGMA user_version;", [], |row| row.get(0))
            .map_err(|e| AppError::Database(format!("读取 user_version 失败: {e}")))
    }

    pub(crate) fn set_user_version(conn: &Connection, version: i32) -> Result<(), AppError> {
        if version < 0 {
            return Err(AppError::Database("user_version 不能为负数".to_string()));
        }
        let sql = format!("PRAGMA user_version = {version};");
        conn.execute(&sql, [])
            .map_err(|e| AppError::Database(format!("写入 user_version 失败: {e}")))?;
        Ok(())
    }

    fn create_request_logs_usage_indexes_if_supported(conn: &Connection) -> Result<(), AppError> {
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_app_created_at
             ON proxy_request_logs(app_type, created_at DESC)",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_dedup_lookup_expr
             ON proxy_request_logs(app_type, COALESCE(data_source, 'proxy'), input_tokens,
                                   output_tokens, cache_read_tokens, created_at,
                                   cache_creation_tokens)",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    pub(crate) fn validate_schema_version_on_conn(conn: &Connection) -> Result<(), AppError> {
        let version = Self::get_user_version(conn)?;
        if version != SCHEMA_VERSION {
            return Err(AppError::Database(format!(
                "不支持数据库格式版本 {version}；当前仅支持 {SCHEMA_VERSION}，不会转换或修改此数据库。"
            )));
        }
        if !Self::has_column(conn, "profiles", "target")? {
            return Err(AppError::Database(
                "数据库缺少当前格式必需的 Profile target 列；原数据库保持不变".into(),
            ));
        }
        let invalid_target: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM profiles WHERE target IS NULL OR target NOT IN ('global', 'project'))",
            [],
            |row| row.get(0),
        ).map_err(|error| AppError::Database(error.to_string()))?;
        if invalid_target {
            return Err(AppError::Database(
                "数据库含不支持的 Profile 用途；原数据库保持不变".into(),
            ));
        }
        Ok(())
    }

    fn validate_identifier(s: &str, kind: &str) -> Result<(), AppError> {
        if s.is_empty() {
            return Err(AppError::Database(format!("{kind} 不能为空")));
        }
        if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(AppError::Database(format!(
                "非法{kind}: {s}，仅允许字母、数字和下划线"
            )));
        }
        Ok(())
    }

    pub(crate) fn table_exists(conn: &Connection, table: &str) -> Result<bool, AppError> {
        Self::validate_identifier(table, "表名")?;

        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .map_err(|e| AppError::Database(format!("读取表名失败: {e}")))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| AppError::Database(format!("查询表名失败: {e}")))?;
        while let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
            let name: String = row
                .get(0)
                .map_err(|e| AppError::Database(format!("解析表名失败: {e}")))?;
            if name.eq_ignore_ascii_case(table) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn has_column(
        conn: &Connection,
        table: &str,
        column: &str,
    ) -> Result<bool, AppError> {
        Self::validate_identifier(table, "表名")?;
        Self::validate_identifier(column, "列名")?;

        let sql = format!("PRAGMA table_info(\"{table}\");");
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AppError::Database(format!("读取表结构失败: {e}")))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| AppError::Database(format!("查询表结构失败: {e}")))?;
        while let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
            let name: String = row
                .get(1)
                .map_err(|e| AppError::Database(format!("读取列名失败: {e}")))?;
            if name.eq_ignore_ascii_case(column) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workbuddy_fresh_install_has_columns_and_persists_flags_independently() -> Result<(), AppError>
    {
        let db = Database::memory()?;
        {
            let conn = db.conn.lock().unwrap();
            for table in ["mcp_servers", "skills"] {
                assert!(Database::has_column(&conn, table, "enabled_workbuddy")?);
                assert!(Database::has_column(&conn, table, "enabled_dsh")?);
            }
            assert!(Database::has_column(&conn, "mcp_servers", "enabled_omp")?);
        }
        let server = serde_json::from_value(serde_json::json!({"id":"m", "name":"m", "server":{"command":"echo"}, "apps":{"workbuddy":true,"dsh":true,"omp":true,"kimi":true}})).unwrap();
        db.save_mcp_server(&server)?;
        let saved = db.get_all_mcp_servers()?;
        assert!(saved["m"].apps.workbuddy && saved["m"].apps.dsh && saved["m"].apps.omp);
        db.update_mcp_server_app_enabled("m", &crate::app_config::AppType::WorkBuddy, false)?;
        let saved = db.get_all_mcp_servers()?;
        assert!(!saved["m"].apps.workbuddy);
        assert!(saved["m"].apps.dsh);
        assert!(saved["m"].apps.omp);
        assert!(saved["m"].apps.kimi);
        db.update_mcp_server_app_enabled("m", &crate::app_config::AppType::Dsh, false)?;
        db.update_mcp_server_app_enabled("m", &crate::app_config::AppType::Omp, false)?;
        db.update_mcp_server_app_enabled("m", &crate::app_config::AppType::WorkBuddy, true)?;
        let saved = db.get_all_mcp_servers()?;
        assert!(
            saved["m"].apps.workbuddy
                && !saved["m"].apps.dsh
                && !saved["m"].apps.omp
                && saved["m"].apps.kimi
        );
        let skill = serde_json::from_value(serde_json::json!({"id":"s", "name":"s", "description":"", "directory":"s", "apps":{"workbuddy":true,"dsh":true,"claude":true}, "installedAt":0})).unwrap();
        db.save_skill(&skill)?;
        let saved = db.get_installed_skill("s")?.unwrap();
        assert!(saved.apps.workbuddy && saved.apps.dsh && saved.apps.claude);
        let mut apps = saved.apps.clone();
        apps.workbuddy = false;
        db.update_skill_apps("s", &apps)?;
        let saved = &db.get_all_installed_skills()?["s"];
        assert!(!saved.apps.workbuddy && saved.apps.dsh && saved.apps.claude);
        let mut apps = saved.apps.clone();
        apps.dsh = false;
        apps.workbuddy = true;
        db.update_skill_apps("s", &apps)?;
        let saved = &db.get_all_installed_skills()?["s"];
        assert!(saved.apps.workbuddy && !saved.apps.dsh && saved.apps.claude);
        Ok(())
    }

    #[test]
    fn copilot_clean_database_persists_mcp_and_skill_flags_independently() -> Result<(), AppError> {
        let db = Database::memory()?;
        let server = serde_json::from_value(serde_json::json!({"id":"m", "name":"m", "server":{"command":"echo"}, "apps":{"copilot":true,"kimi":true}})).unwrap();
        db.save_mcp_server(&server)?;
        db.update_mcp_server_app_enabled("m", &crate::app_config::AppType::Copilot, false)?;
        let saved = db.get_all_mcp_servers()?;
        assert!(!saved["m"].apps.copilot);
        assert!(saved["m"].apps.kimi);
        let skill = serde_json::from_value(serde_json::json!({"id":"s", "name":"s", "description":"", "directory":"s", "apps":{"copilot":true,"claude":true}, "installedAt":0})).unwrap();
        db.save_skill(&skill)?;
        let saved = db.get_installed_skill("s")?.unwrap();
        assert!(saved.apps.copilot && saved.apps.claude);
        let mut apps = saved.apps.clone();
        apps.copilot = false;
        db.update_skill_apps("s", &apps)?;
        let saved = db.get_all_installed_skills()?;
        assert!(!saved["s"].apps.copilot && saved["s"].apps.claude);
        Ok(())
    }

    #[test]
    fn kimi_clean_database_persists_mcp_and_skill_flags_independently() -> Result<(), AppError> {
        let db = Database::memory()?;
        let server = serde_json::from_value(serde_json::json!({"id":"m", "name":"m", "server":{"command":"echo"}, "apps":{"kimi":true,"codex":true}})).unwrap();
        db.save_mcp_server(&server)?;
        db.update_mcp_server_app_enabled("m", &crate::app_config::AppType::Kimi, false)?;
        let saved = db.get_all_mcp_servers()?;
        assert!(!saved["m"].apps.kimi);
        assert!(saved["m"].apps.codex);
        let skill = serde_json::from_value(serde_json::json!({"id":"s", "name":"s", "description":"", "directory":"s", "apps":{"kimi":true,"claude":true}, "installedAt":0})).unwrap();
        db.save_skill(&skill)?;
        let saved = db.get_installed_skill("s")?.unwrap();
        assert!(saved.apps.kimi && saved.apps.claude);
        assert!(db.get_all_installed_skills()?["s"].apps.kimi);
        Ok(())
    }

    #[test]
    fn agy_clean_database_persists_independent_mcp_and_skill_flags() -> Result<(), AppError> {
        let db = Database::memory()?;
        let mut server: crate::app_config::McpServer = serde_json::from_value(serde_json::json!({"id":"m", "name":"m", "server":{"command":"echo"}, "apps":{"gemini":true,"agy":true}})).unwrap();
        db.save_mcp_server(&server)?;
        server.apps.agy = false;
        db.update_mcp_server_app_enabled("m", &crate::app_config::AppType::Agy, false)?;
        assert_eq!(db.get_all_mcp_servers()?["m"].apps, server.apps);
        let skill: crate::app_config::InstalledSkill = serde_json::from_value(serde_json::json!({"id":"s", "name":"s", "directory":"s", "installedAt":0, "apps":{"agy":true,"gemini":true}})).unwrap();
        db.save_skill(&skill)?;
        assert!(db.get_installed_skill("s")?.unwrap().apps.agy);
        let mut apps = skill.apps;
        apps.agy = false;
        db.update_skill_apps("s", &apps)?;
        let read = db.get_all_installed_skills()?;
        assert!(!read["s"].apps.agy);
        assert!(read["s"].apps.gemini);
        Ok(())
    }
}
