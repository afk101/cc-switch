//! 旧版单例 Codex 配置向 Profile 的一次性迁移。

use crate::app_config::AppType;
use crate::codex_profile::{
    CodexProfile, CodexProfileRepository, HomePathCanonicalizer,
    CODEX_LEGACY_TOKEN_PENDING_PROFILE_SETTING, DEFAULT_CODEX_PROFILE_ID, LEGACY_CODEX_ROUTE_PORT,
};
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use chrono::Utc;
use rusqlite::params;
use std::path::PathBuf;
use std::sync::Arc;

/// 旧单例 Codex 状态在迁移时刻的只读快照。
#[derive(Debug, Clone)]
pub struct LegacyCodexProfileSnapshot {
    pub override_home: Option<PathBuf>,
    pub current_provider_id: Option<String>,
    pub route_enabled: bool,
    pub failover_provider_ids: Vec<String>,
    pub live_backup_json: Option<String>,
}

impl LegacyCodexProfileSnapshot {
    /// 从旧 settings 与数据库读取一次迁移所需的单例状态。
    pub fn from_legacy_state(
        db: &Database,
        override_home: Option<PathBuf>,
        current_provider_id: Option<String>,
    ) -> Result<Self, AppError> {
        let current_provider_id =
            current_provider_id.or(db.get_current_provider(AppType::Codex.as_str())?);
        let route_enabled =
            futures::executor::block_on(db.get_proxy_config_for_app(AppType::Codex.as_str()))?
                .enabled;
        let failover_provider_ids = db
            .get_failover_queue(AppType::Codex.as_str())?
            .into_iter()
            .map(|item| item.provider_id)
            .collect();
        let live_backup_json =
            futures::executor::block_on(db.get_live_backup(AppType::Codex.as_str()))?
                .map(|backup| backup.original_config);

        Ok(Self {
            override_home,
            current_provider_id,
            route_enabled,
            failover_provider_ids,
            live_backup_json,
        })
    }
}

/// 已完成旧单例迁移且当时已启用路由的 Profile 证明。
///
/// 该类型只能由迁移服务产生，避免普通新 Profile 误用旧 `PROXY_MANAGED` 兼容凭证。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigratedEnabledCodexProfile {
    profile_id: String,
}

impl MigratedEnabledCodexProfile {
    /// 返回已由迁移服务绑定的 Profile 标识。
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }
}

/// 旧单例迁移的结果，保留后续兼容初始化所需的受控证明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexProfileMigrationResult {
    pub selected_profile_id: String,
    migrated_enabled_profile: Option<MigratedEnabledCodexProfile>,
}

impl CodexProfileMigrationResult {
    /// 仅在本次实际迁移了已启用旧路由时返回兼容 token 初始化证明。
    pub fn migrated_enabled_profile(&self) -> Option<&MigratedEnabledCodexProfile> {
        self.migrated_enabled_profile.as_ref()
    }
}

/// 将旧单例 Codex 状态映射为 Profile 的服务。
pub struct CodexProfileMigrationService {
    db: Arc<Database>,
    repository: CodexProfileRepository,
    canonicalizer: Arc<dyn HomePathCanonicalizer>,
    default_home: PathBuf,
}

/// 一次旧单例迁移所需的数据库写入计划。
struct LegacyCodexProfileMigrationPlan {
    profiles_to_insert: Vec<CodexProfile>,
    profiles_to_update: Vec<CodexProfile>,
    selected_profile_id: String,
    should_apply_legacy_state: bool,
    actual_home: PathBuf,
}

impl CodexProfileMigrationService {
    /// 用数据库、统一路径仓库与默认 Home 构造迁移服务。
    pub fn new(
        db: Arc<Database>,
        repository: CodexProfileRepository,
        canonicalizer: Arc<dyn HomePathCanonicalizer>,
        default_home: PathBuf,
    ) -> Self {
        Self {
            db,
            repository,
            canonicalizer,
            default_home,
        }
    }

    /// 将旧单例 Codex 状态幂等映射到 Profile，不创建或改写任何 Codex 配置文件。
    pub fn migrate(&self, snapshot: LegacyCodexProfileSnapshot) -> Result<String, AppError> {
        Ok(self.migrate_with_result(snapshot)?.selected_profile_id)
    }

    /// 将旧单例状态迁移为 Profile，并返回仅供受控兼容初始化使用的迁移结果。
    pub fn migrate_with_result(
        &self,
        snapshot: LegacyCodexProfileSnapshot,
    ) -> Result<CodexProfileMigrationResult, AppError> {
        let override_home = snapshot
            .override_home
            .as_ref()
            .map(|home| self.canonicalizer.canonicalize(home))
            .transpose()?;
        let default_home = self.canonicalize_default_home()?;
        let actual_home = override_home.unwrap_or_else(|| default_home.clone());
        let actual_home_is_default = actual_home == default_home;
        let plan = self.plan_migration(&default_home, &actual_home, actual_home_is_default)?;
        self.apply_migration_plan(&plan, &snapshot)?;
        Ok(CodexProfileMigrationResult {
            selected_profile_id: plan.selected_profile_id.clone(),
            migrated_enabled_profile: (plan.should_apply_legacy_state && snapshot.route_enabled)
                .then(|| MigratedEnabledCodexProfile {
                    profile_id: plan.selected_profile_id,
                }),
        })
    }

    /// 确保内置默认 Home 可被登记；仅创建目录本身，不创建或改写任何 Codex 文件。
    fn canonicalize_default_home(&self) -> Result<PathBuf, AppError> {
        if !self.default_home.exists() {
            std::fs::create_dir_all(&self.default_home)
                .map_err(|error| AppError::io(&self.default_home, error))?;
        }
        self.canonicalizer.canonicalize(&self.default_home)
    }

    /// 计算默认与旧实际 Home 的 Profile 写入计划，不在此阶段修改数据库。
    fn plan_migration(
        &self,
        default_home: &std::path::Path,
        actual_home: &std::path::Path,
        actual_home_is_default: bool,
    ) -> Result<LegacyCodexProfileMigrationPlan, AppError> {
        let existing_profiles = self.db.list_codex_profiles()?;
        let existing_default = find_profile_by_home(&existing_profiles, default_home);
        let now = Utc::now().timestamp();
        if actual_home_is_default {
            return match existing_default {
                Some(profile) => Ok(LegacyCodexProfileMigrationPlan {
                    profiles_to_insert: Vec::new(),
                    profiles_to_update: Vec::new(),
                    selected_profile_id: profile.id,
                    should_apply_legacy_state: false,
                    actual_home: actual_home.to_path_buf(),
                }),
                None => Ok(LegacyCodexProfileMigrationPlan {
                    profiles_to_insert: vec![CodexProfile::default_profile(
                        default_home.to_string_lossy().into_owned(),
                        now,
                    )],
                    profiles_to_update: Vec::new(),
                    selected_profile_id: DEFAULT_CODEX_PROFILE_ID.to_string(),
                    should_apply_legacy_state: true,
                    actual_home: actual_home.to_path_buf(),
                }),
            };
        }

        let existing_actual = find_profile_by_home(&existing_profiles, actual_home);
        let mut profiles_to_insert = Vec::new();
        let mut profiles_to_update = Vec::new();
        let default_profile = match existing_default {
            Some(mut profile) => {
                if profile.listen_port == LEGACY_CODEX_ROUTE_PORT {
                    profile.listen_port = self.repository.allocate_port()?;
                    profile.updated_at = now;
                    profiles_to_update.push(profile.clone());
                }
                profile
            }
            None => {
                let mut profile =
                    CodexProfile::default_profile(default_home.to_string_lossy().into_owned(), now);
                profile.listen_port = self.repository.allocate_port()?;
                profiles_to_insert.push(profile.clone());
                profile
            }
        };

        let (actual_profile, should_apply_legacy_state) = match existing_actual {
            Some(mut profile) => {
                if profile.listen_port != LEGACY_CODEX_ROUTE_PORT {
                    profile.listen_port = LEGACY_CODEX_ROUTE_PORT;
                    profile.updated_at = now;
                    profiles_to_update.push(profile.clone());
                }
                (profile, false)
            }
            None => {
                let profile = CodexProfile {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: "原 Codex 配置".to_string(),
                    canonical_home_path: actual_home.to_string_lossy().into_owned(),
                    listen_port: LEGACY_CODEX_ROUTE_PORT,
                    created_at: now,
                    updated_at: now,
                };
                profiles_to_insert.push(profile.clone());
                (profile, true)
            }
        };

        debug_assert_ne!(default_profile.id, actual_profile.id);
        Ok(LegacyCodexProfileMigrationPlan {
            profiles_to_insert,
            profiles_to_update,
            selected_profile_id: actual_profile.id,
            should_apply_legacy_state,
            actual_home: actual_home.to_path_buf(),
        })
    }

    /// 在同一事务中写入新 Profile、旧路由状态、故障转移与 Live 备份归属。
    fn apply_migration_plan(
        &self,
        plan: &LegacyCodexProfileMigrationPlan,
        snapshot: &LegacyCodexProfileSnapshot,
    ) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.db.conn);
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Database(error.to_string()))?;

        for profile in &plan.profiles_to_update {
            update_profile_in_transaction(&tx, profile)?;
        }
        for profile in &plan.profiles_to_insert {
            insert_profile_with_empty_route(&tx, profile)?;
        }
        if plan.should_apply_legacy_state {
            save_legacy_route_in_transaction(&tx, &plan.selected_profile_id, snapshot)?;
            replace_legacy_failovers_in_transaction(
                &tx,
                &plan.selected_profile_id,
                &snapshot.failover_provider_ids,
            )?;
            if snapshot.live_backup_json.is_some() {
                tx.execute("DELETE FROM proxy_live_backup WHERE app_type = 'codex'", [])
                    .map_err(|error| AppError::Database(error.to_string()))?;
            }
            if snapshot.route_enabled {
                tx.execute(
                    "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                    params![
                        CODEX_LEGACY_TOKEN_PENDING_PROFILE_SETTING,
                        plan.selected_profile_id
                    ],
                )
                .map_err(|error| AppError::Database(error.to_string()))?;
            }
        }
        backfill_legacy_history_in_transaction(&tx, &plan.selected_profile_id, &plan.actual_home)?;
        tx.commit()
            .map_err(|error| AppError::Database(error.to_string()))
    }
}

/// 按规范化 Home 路径从已加载 Profile 中查找记录。
fn find_profile_by_home(
    profiles: &[CodexProfile],
    canonical_home: &std::path::Path,
) -> Option<CodexProfile> {
    let canonical_home = canonical_home.to_string_lossy();
    profiles
        .iter()
        .find(|profile| profile.canonical_home_path == canonical_home)
        .cloned()
}

/// 在迁移事务内更新 Profile 的端口与时间戳。
fn update_profile_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    profile: &CodexProfile,
) -> Result<(), AppError> {
    tx.execute(
        "UPDATE codex_profiles
         SET name = ?1, canonical_home_path = ?2, listen_port = ?3, updated_at = ?4
         WHERE id = ?5",
        params![
            profile.name,
            profile.canonical_home_path,
            profile.listen_port,
            profile.updated_at,
            profile.id,
        ],
    )
    .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(())
}

/// 在迁移事务内创建 Profile 与其初始空路由。
fn insert_profile_with_empty_route(
    tx: &rusqlite::Transaction<'_>,
    profile: &CodexProfile,
) -> Result<(), AppError> {
    tx.execute(
        "INSERT INTO codex_profiles
         (id, name, canonical_home_path, listen_port, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            profile.id,
            profile.name,
            profile.canonical_home_path,
            profile.listen_port,
            profile.created_at,
            profile.updated_at,
        ],
    )
    .map_err(|error| AppError::Database(error.to_string()))?;
    tx.execute(
        "INSERT INTO codex_profile_routes
         (profile_id, current_provider_id, provider_app_type, enabled, live_backup_json, updated_at)
         VALUES (?1, NULL, 'codex', 0, NULL, ?2)",
        params![profile.id, profile.updated_at],
    )
    .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(())
}

/// 在迁移事务内保存旧单例路由状态及其 Live 配置备份。
fn save_legacy_route_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    profile_id: &str,
    snapshot: &LegacyCodexProfileSnapshot,
) -> Result<(), AppError> {
    tx.execute(
        "UPDATE codex_profile_routes
         SET current_provider_id = ?1, provider_app_type = 'codex', enabled = ?2,
             live_backup_json = ?3, updated_at = ?4
         WHERE profile_id = ?5",
        params![
            snapshot.current_provider_id,
            snapshot.route_enabled,
            snapshot.live_backup_json,
            Utc::now().timestamp(),
            profile_id,
        ],
    )
    .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(())
}

/// 在迁移事务内按旧队列顺序保存 Profile 故障转移供应商。
fn replace_legacy_failovers_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    profile_id: &str,
    provider_ids: &[String],
) -> Result<(), AppError> {
    for (position, provider_id) in provider_ids.iter().enumerate() {
        tx.execute(
            "INSERT INTO codex_profile_failovers
             (profile_id, position, provider_id, provider_app_type)
             VALUES (?1, ?2, ?3, 'codex')",
            params![profile_id, position, provider_id],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
    }
    Ok(())
}

/// 回填能可靠识别为旧单例 Codex 的未归属历史；无法判断的行保持空归属。
fn backfill_legacy_history_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    profile_id: &str,
    actual_home: &std::path::Path,
) -> Result<(), AppError> {
    tx.execute(
        "UPDATE proxy_request_logs
         SET profile_id = ?1
         WHERE app_type = 'codex' AND (profile_id IS NULL OR profile_id = '')",
        [profile_id],
    )
    .map_err(|error| AppError::Database(error.to_string()))?;
    // 与目标 Profile 聚合键冲突时沿用日聚合逻辑合并计数、Token、成本，
    // 平均延迟按请求数加权；完成后删除空桶，避免同一历史被重复统计。
    tx.execute(
        "INSERT INTO usage_daily_rollups
            (date, app_type, provider_id, model, request_model, pricing_model,
             request_count, success_count, input_tokens, output_tokens,
             cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms, profile_id)
         SELECT date, app_type, provider_id, model, request_model, pricing_model,
                request_count, success_count, input_tokens, output_tokens,
                cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms, ?1
         FROM usage_daily_rollups
         WHERE app_type = 'codex' AND profile_id = ''
         ON CONFLICT(date, app_type, provider_id, model, request_model, pricing_model, profile_id)
         DO UPDATE SET
             request_count = usage_daily_rollups.request_count + excluded.request_count,
             success_count = usage_daily_rollups.success_count + excluded.success_count,
             input_tokens = usage_daily_rollups.input_tokens + excluded.input_tokens,
             output_tokens = usage_daily_rollups.output_tokens + excluded.output_tokens,
             cache_read_tokens = usage_daily_rollups.cache_read_tokens + excluded.cache_read_tokens,
             cache_creation_tokens = usage_daily_rollups.cache_creation_tokens + excluded.cache_creation_tokens,
             total_cost_usd = CAST(
                 COALESCE(CAST(usage_daily_rollups.total_cost_usd AS REAL), 0)
                 + COALESCE(CAST(excluded.total_cost_usd AS REAL), 0)
                 AS TEXT
             ),
             avg_latency_ms = CASE
                 WHEN usage_daily_rollups.request_count + excluded.request_count > 0 THEN
                     (usage_daily_rollups.avg_latency_ms * usage_daily_rollups.request_count
                      + excluded.avg_latency_ms * excluded.request_count)
                     / (usage_daily_rollups.request_count + excluded.request_count)
                 ELSE 0
             END",
        [profile_id],
    )
    .map_err(|error| AppError::Database(error.to_string()))?;
    tx.execute(
        "DELETE FROM usage_daily_rollups
         WHERE app_type = 'codex' AND profile_id = ''",
        [],
    )
    .map_err(|error| AppError::Database(error.to_string()))?;
    backfill_legacy_session_sync_in_transaction(tx, profile_id, actual_home)
}

/// 仅按会话文件位于旧实际 Home 的 sessions/ 或 archived_sessions/ 下回填归属。
fn backfill_legacy_session_sync_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    profile_id: &str,
    actual_home: &std::path::Path,
) -> Result<(), AppError> {
    let file_paths = {
        let mut statement = tx
            .prepare(
                "SELECT file_path FROM session_log_sync
                 WHERE profile_id IS NULL OR profile_id = ''",
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        let file_paths = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| AppError::Database(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| AppError::Database(error.to_string()))?;
        file_paths
    };
    let session_root = actual_home.join("sessions");
    let archived_root = actual_home.join("archived_sessions");

    for file_path in file_paths {
        // 旧记录的字符串路径可能含有 /var 与 /private/var 等别名；仅在文件仍存在、
        // 规范化后明确位于当前 Home 的会话目录时才归属，无法确认的记录必须保持空归属。
        let Ok(path) = std::fs::canonicalize(&file_path) else {
            continue;
        };
        if path.starts_with(&session_root) || path.starts_with(&archived_root) {
            tx.execute(
                "UPDATE session_log_sync SET profile_id = ?1
                 WHERE file_path = ?2 AND (profile_id IS NULL OR profile_id = '')",
                params![profile_id, file_path],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CodexProfileMigrationService, LegacyCodexProfileSnapshot};
    use crate::codex_profile::{
        CodexProfileRepository, HomePathCanonicalizer, SystemHomePathCanonicalizer,
        SystemPortAvailability, DEFAULT_CODEX_PROFILE_ID, LEGACY_CODEX_ROUTE_PORT,
    };
    use crate::database::Database;
    use crate::error::AppError;
    use std::path::Path;
    use std::sync::Arc;

    /// 构造迁移测试服务，使默认 Home 可由临时目录稳定控制。
    fn migration_service(db: Arc<Database>, default_home: &Path) -> CodexProfileMigrationService {
        let canonicalizer: Arc<dyn HomePathCanonicalizer> = Arc::new(SystemHomePathCanonicalizer);
        let repository = CodexProfileRepository::new(
            db.clone(),
            canonicalizer.clone(),
            Arc::new(SystemPortAvailability),
        );
        CodexProfileMigrationService::new(db, repository, canonicalizer, default_home.to_path_buf())
    }

    /// 没有 override 时只创建默认 Profile，且迁移绝不触碰 Home 中已有文件。
    #[test]
    fn legacy_codex_profile_migration_creates_only_default_profile_without_override(
    ) -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let default_home = temp_dir.path().join(".codex");
        std::fs::create_dir(&default_home).expect("创建默认 Home");
        let auth_path = default_home.join("auth.json");
        let config_path = default_home.join("config.toml");
        let session_path = default_home.join("sessions.jsonl");
        std::fs::write(&auth_path, "{\"token\":\"keep\"}").expect("写入认证文件");
        std::fs::write(&config_path, "model_provider = \"keep\"").expect("写入配置文件");
        std::fs::write(&session_path, "session").expect("写入会话文件");
        let before = [
            std::fs::read(&auth_path).expect("读取认证文件"),
            std::fs::read(&config_path).expect("读取配置文件"),
            std::fs::read(&session_path).expect("读取会话文件"),
        ];

        let db = Arc::new(Database::memory()?);
        let selected_profile_id =
            migration_service(db.clone(), &default_home).migrate(LegacyCodexProfileSnapshot {
                override_home: None,
                current_provider_id: None,
                route_enabled: false,
                failover_provider_ids: Vec::new(),
                live_backup_json: None,
            })?;

        let profiles = db.list_codex_profiles()?;
        assert_eq!(selected_profile_id, DEFAULT_CODEX_PROFILE_ID);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id, DEFAULT_CODEX_PROFILE_ID);
        assert_eq!(profiles[0].listen_port, LEGACY_CODEX_ROUTE_PORT);
        assert_eq!(
            profiles[0].canonical_home_path,
            std::fs::canonicalize(&default_home)
                .expect("规范化默认 Home")
                .to_string_lossy()
        );
        assert_eq!(
            before,
            [
                std::fs::read(&auth_path).expect("再次读取认证文件"),
                std::fs::read(&config_path).expect("再次读取配置文件"),
                std::fs::read(&session_path).expect("再次读取会话文件"),
            ]
        );
        Ok(())
    }

    /// 未启用的旧路由迁移不得产生旧占位符兼容凭证的初始化证明。
    #[test]
    fn disabled_legacy_migration_does_not_issue_compatibility_token_proof() -> Result<(), AppError>
    {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let default_home = temp_dir.path().join(".codex");
        std::fs::create_dir(&default_home).expect("创建默认 Home");
        let db = Arc::new(Database::memory()?);

        let result = migration_service(db, &default_home).migrate_with_result(
            LegacyCodexProfileSnapshot {
                override_home: None,
                current_provider_id: None,
                route_enabled: false,
                failover_provider_ids: Vec::new(),
                live_backup_json: None,
            },
        )?;

        assert!(result.migrated_enabled_profile().is_none());
        Ok(())
    }

    /// 旧 override Home 缺失时必须返回其路径相关的错误，迁移不得代为创建该目录。
    #[test]
    fn legacy_codex_profile_migration_rejects_missing_override_home() -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let default_home = temp_dir.path().join(".codex");
        let missing_override_home = temp_dir.path().join("missing-work-codex");
        let db = Arc::new(Database::memory()?);

        let error = migration_service(db, &default_home)
            .migrate(LegacyCodexProfileSnapshot {
                override_home: Some(missing_override_home.clone()),
                current_provider_id: None,
                route_enabled: false,
                failover_provider_ids: Vec::new(),
                live_backup_json: None,
            })
            .expect_err("缺失的 override Home 不得被自动创建");

        match error {
            AppError::Io { path, .. } => {
                assert_eq!(path, missing_override_home.to_string_lossy());
            }
            other => panic!("应返回包含 override 路径的 IO 错误，实际为: {other}"),
        }
        assert!(
            !missing_override_home.exists(),
            "不得创建缺失的 override Home"
        );
        assert!(
            !default_home.exists(),
            "缺失 override 时不得提前创建默认 Home"
        );
        Ok(())
    }

    /// 外部 override 的旧状态必须归入“原 Codex 配置”，并且重复迁移不能覆盖新数据。
    #[test]
    fn legacy_codex_profile_migration_assigns_legacy_state_to_override_home_idempotently(
    ) -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let default_home = temp_dir.path().join(".codex");
        let override_home = temp_dir.path().join("work-codex");
        std::fs::create_dir(&default_home).expect("创建默认 Home");
        std::fs::create_dir(&override_home).expect("创建旧 override Home");

        let db = Arc::new(Database::memory()?);
        {
            let conn = db.conn.lock().expect("获取数据库锁");
            conn.execute_batch(
                "INSERT INTO providers (id, app_type, name, settings_config, meta) VALUES
                    ('current-provider', 'codex', '当前供应商', '{}', '{}'),
                    ('failover-provider', 'codex', '故障转移供应商', '{}', '{}');",
            )
            .expect("写入旧供应商");
        }
        futures::executor::block_on(db.save_live_backup("codex", "{\"config\":\"legacy\"}"))?;
        let service = migration_service(db.clone(), &default_home);
        let snapshot = LegacyCodexProfileSnapshot {
            override_home: Some(override_home.clone()),
            current_provider_id: Some("current-provider".to_string()),
            route_enabled: true,
            failover_provider_ids: vec!["failover-provider".to_string()],
            live_backup_json: Some("{\"config\":\"legacy\"}".to_string()),
        };

        let selected_profile_id = service.migrate(snapshot.clone())?;
        let selected_profile = db.get_codex_profile(&selected_profile_id)?;
        assert_eq!(selected_profile.name, "原 Codex 配置");
        assert_eq!(selected_profile.listen_port, LEGACY_CODEX_ROUTE_PORT);
        assert_eq!(
            selected_profile.canonical_home_path,
            std::fs::canonicalize(&override_home)
                .expect("规范化旧 override Home")
                .to_string_lossy()
        );
        assert_eq!(db.list_codex_profiles()?.len(), 2);
        let migrated_route = db
            .get_codex_profile_route(&selected_profile_id)?
            .expect("读取旧 Home 路由");
        assert_eq!(
            migrated_route.current_provider_id.as_deref(),
            Some("current-provider")
        );
        assert!(migrated_route.enabled);
        assert_eq!(
            migrated_route.live_backup_json.as_deref(),
            Some("{\"config\":\"legacy\"}")
        );
        assert!(futures::executor::block_on(db.get_live_backup("codex"))?.is_none());
        assert_eq!(
            db.list_codex_profile_failovers(&selected_profile_id)?,
            vec!["failover-provider".to_string()]
        );

        let mut changed_route = db
            .get_codex_profile_route(&selected_profile_id)?
            .expect("读取待修改路由");
        changed_route.enabled = false;
        changed_route.current_provider_id = None;
        db.save_codex_profile_route(&changed_route)?;
        db.replace_codex_profile_failovers(&selected_profile_id, &[])?;

        assert_eq!(service.migrate(snapshot)?, selected_profile_id);
        assert_eq!(db.list_codex_profiles()?.len(), 2);
        assert_eq!(
            db.get_codex_profile_route(&selected_profile_id)?
                .expect("读取重复迁移后的路由"),
            changed_route
        );
        assert!(db
            .list_codex_profile_failovers(&selected_profile_id)?
            .is_empty());
        Ok(())
    }

    /// 首次迁移失败必须回滚，修复旧 Provider 后可完整重试所有旧状态。
    #[test]
    fn legacy_codex_profile_migration_retries_after_invalid_provider_without_losing_state(
    ) -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let default_home = temp_dir.path().join(".codex");
        let override_home = temp_dir.path().join("work-codex");
        std::fs::create_dir(&default_home).expect("创建默认 Home");
        std::fs::create_dir(&override_home).expect("创建旧 override Home");
        let db = Arc::new(Database::memory()?);
        {
            let conn = db.conn.lock().expect("获取数据库锁");
            conn.execute(
                "INSERT INTO providers (id, app_type, name, settings_config, meta)
                 VALUES ('failover-provider', 'codex', '故障转移供应商', '{}', '{}')",
                [],
            )
            .expect("写入故障转移供应商");
        }
        futures::executor::block_on(db.save_live_backup("codex", "{\"config\":\"legacy\"}"))?;
        let snapshot = LegacyCodexProfileSnapshot {
            override_home: Some(override_home),
            current_provider_id: Some("missing-provider".to_string()),
            route_enabled: true,
            failover_provider_ids: vec!["failover-provider".to_string()],
            live_backup_json: Some("{\"config\":\"legacy\"}".to_string()),
        };
        let service = migration_service(db.clone(), &default_home);

        assert!(service.migrate(snapshot.clone()).is_err());
        assert!(
            db.list_codex_profiles()?.is_empty(),
            "失败迁移不得残留半成品 Profile"
        );
        assert!(futures::executor::block_on(db.get_live_backup("codex"))?.is_some());

        {
            let conn = db.conn.lock().expect("获取数据库锁");
            conn.execute(
                "INSERT INTO providers (id, app_type, name, settings_config, meta)
                 VALUES ('missing-provider', 'codex', '修复后的供应商', '{}', '{}')",
                [],
            )
            .expect("修复当前供应商");
        }
        let selected_profile_id = service.migrate(snapshot)?;
        let route = db
            .get_codex_profile_route(&selected_profile_id)?
            .expect("读取重试后的路由");
        assert_eq!(
            route.current_provider_id.as_deref(),
            Some("missing-provider")
        );
        assert!(route.enabled);
        assert_eq!(
            route.live_backup_json.as_deref(),
            Some("{\"config\":\"legacy\"}")
        );
        assert_eq!(
            db.list_codex_profile_failovers(&selected_profile_id)?,
            vec!["failover-provider".to_string()]
        );
        assert!(futures::executor::block_on(db.get_live_backup("codex"))?.is_none());
        Ok(())
    }

    /// 旧 Codex 历史只归属实际旧 Home，其他应用和其他 Home 的数据必须保持未归属。
    #[test]
    fn legacy_codex_profile_migration_backfills_only_identifiable_legacy_history_idempotently(
    ) -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let default_home = temp_dir.path().join(".codex");
        let override_home = temp_dir.path().join("work-codex");
        let other_home = temp_dir.path().join("other-codex");
        std::fs::create_dir(&override_home).expect("创建旧 override Home");
        std::fs::create_dir(&other_home).expect("创建其他 Home");
        let legacy_session_path = override_home.join("sessions/2026/07/13/legacy.jsonl");
        let other_session_path = other_home.join("sessions/2026/07/13/other.jsonl");
        let missing_session_path = override_home.join("sessions/2026/07/13/missing.jsonl");
        std::fs::create_dir_all(legacy_session_path.parent().expect("旧会话文件应有父目录"))
            .expect("创建旧会话目录");
        std::fs::create_dir_all(other_session_path.parent().expect("其他会话文件应有父目录"))
            .expect("创建其他会话目录");
        std::fs::write(&legacy_session_path, "{}").expect("创建旧会话文件");
        std::fs::write(&other_session_path, "{}").expect("创建其他会话文件");
        let legacy_session_path = std::fs::canonicalize(&legacy_session_path)
            .expect("规范化旧会话文件")
            .to_string_lossy()
            .to_string();
        let other_session_path = std::fs::canonicalize(&other_session_path)
            .expect("规范化其他会话文件")
            .to_string_lossy()
            .to_string();
        let missing_session_path = missing_session_path.to_string_lossy().to_string();
        let db = Arc::new(Database::memory()?);
        {
            let conn = db.conn.lock().expect("获取数据库锁");
            conn.execute_batch(
                "INSERT INTO providers (id, app_type, name, settings_config, meta) VALUES
                    ('legacy-provider', 'codex', '旧 Codex 供应商', '{}', '{}');
                 INSERT INTO proxy_request_logs
                    (request_id, provider_id, app_type, model, latency_ms, status_code, created_at)
                 VALUES
                    ('codex-legacy', 'legacy-provider', 'codex', 'gpt-5', 1, 200, 1),
                    ('claude-other', 'claude-provider', 'claude', 'claude', 1, 200, 1);
                 INSERT INTO usage_daily_rollups
                    (date, app_type, provider_id, model, request_model, pricing_model)
                 VALUES
                    ('2026-07-13', 'codex', 'legacy-provider', 'gpt-5', '', ''),
                    ('2026-07-13', 'claude', 'claude-provider', 'claude', '', '');",
            )
            .expect("写入混合历史数据");
            conn.execute(
                "INSERT INTO session_log_sync
                    (file_path, last_modified, last_line_offset, last_synced_at)
                 VALUES (?1, 1, 0, 1), (?2, 1, 0, 1), (?3, 1, 0, 1)",
                [
                    legacy_session_path.clone(),
                    other_session_path.clone(),
                    missing_session_path.clone(),
                ],
            )
            .expect("写入混合会话状态");
        }
        let snapshot = LegacyCodexProfileSnapshot {
            override_home: Some(override_home.clone()),
            current_provider_id: Some("legacy-provider".to_string()),
            route_enabled: true,
            failover_provider_ids: Vec::new(),
            live_backup_json: None,
        };
        let service = migration_service(db.clone(), &default_home);

        let selected_profile_id = service.migrate(snapshot.clone())?;
        assert!(default_home.is_dir(), "默认 Home 不存在时应只创建目录");
        for file_name in ["auth.json", "config.toml", "sessions"] {
            assert!(
                !default_home.join(file_name).exists(),
                "迁移不得创建 {file_name}"
            );
        }
        assert_history_ownership(
            &db,
            &selected_profile_id,
            &legacy_session_path,
            &other_session_path,
            &missing_session_path,
        )?;

        assert_eq!(service.migrate(snapshot)?, selected_profile_id);
        assert_history_ownership(
            &db,
            &selected_profile_id,
            &legacy_session_path,
            &other_session_path,
            &missing_session_path,
        )
    }

    /// 旧空 Profile 聚合桶与目标桶冲突时，迁移必须按现有聚合语义合并并删除旧桶。
    #[test]
    fn legacy_codex_profile_migration_merges_conflicting_legacy_rollup_idempotently(
    ) -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let default_home = temp_dir.path().join(".codex");
        let override_home = temp_dir.path().join("work-codex");
        std::fs::create_dir(&default_home).expect("创建默认 Home");
        std::fs::create_dir(&override_home).expect("创建旧 override Home");
        let db = Arc::new(Database::memory()?);
        {
            let conn = db.conn.lock().expect("获取数据库锁");
            conn.execute(
                "INSERT INTO providers (id, app_type, name, settings_config, meta)
                 VALUES ('legacy-provider', 'codex', '旧 Codex 供应商', '{}', '{}')",
                [],
            )
            .expect("写入旧供应商");
        }
        let snapshot = LegacyCodexProfileSnapshot {
            override_home: Some(override_home),
            current_provider_id: Some("legacy-provider".to_string()),
            route_enabled: false,
            failover_provider_ids: Vec::new(),
            live_backup_json: None,
        };
        let service = migration_service(db.clone(), &default_home);
        let selected_profile_id = service.migrate(snapshot.clone())?;
        {
            let conn = db.conn.lock().expect("获取数据库锁");
            conn.execute(
                "INSERT INTO usage_daily_rollups
                    (date, app_type, provider_id, model, request_model, pricing_model,
                     request_count, success_count, input_tokens, output_tokens,
                     cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms, profile_id)
                 VALUES
                    ('2026-07-13', 'codex', 'legacy-provider', 'gpt-5', '', '',
                     2, 1, 10, 20, 4, 5, '0.25', 100, ?1)",
                [&selected_profile_id],
            )
            .expect("写入目标聚合桶");
            conn.execute(
                "INSERT INTO usage_daily_rollups
                    (date, app_type, provider_id, model, request_model, pricing_model,
                     request_count, success_count, input_tokens, output_tokens,
                     cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms, profile_id)
                 VALUES
                    ('2026-07-13', 'codex', 'legacy-provider', 'gpt-5', '', '',
                     3, 2, 30, 40, 6, 7, '0.75', 200, '')",
                [],
            )
            .expect("写入冲突的旧聚合桶");
        }

        assert_eq!(service.migrate(snapshot.clone())?, selected_profile_id);
        assert_merged_rollup(&db, &selected_profile_id)?;

        assert_eq!(service.migrate(snapshot)?, selected_profile_id);
        assert_merged_rollup(&db, &selected_profile_id)
    }

    /// 断言混合历史数据只被归属到旧实际 Home 对应的 Profile。
    fn assert_history_ownership(
        db: &Database,
        selected_profile_id: &str,
        legacy_session_path: &str,
        other_session_path: &str,
        missing_session_path: &str,
    ) -> Result<(), AppError> {
        let conn = db.conn.lock().expect("获取数据库锁");
        let codex_request_profile: Option<String> = conn.query_row(
            "SELECT profile_id FROM proxy_request_logs WHERE request_id = 'codex-legacy'",
            [],
            |row| row.get(0),
        )?;
        let claude_request_profile: Option<String> = conn.query_row(
            "SELECT profile_id FROM proxy_request_logs WHERE request_id = 'claude-other'",
            [],
            |row| row.get(0),
        )?;
        let codex_rollup_profile: String = conn.query_row(
            "SELECT profile_id FROM usage_daily_rollups
             WHERE app_type = 'codex' AND provider_id = 'legacy-provider'",
            [],
            |row| row.get(0),
        )?;
        let claude_rollup_profile: String = conn.query_row(
            "SELECT profile_id FROM usage_daily_rollups
             WHERE app_type = 'claude' AND provider_id = 'claude-provider'",
            [],
            |row| row.get(0),
        )?;
        let legacy_session_profile: Option<String> = conn.query_row(
            "SELECT profile_id FROM session_log_sync WHERE file_path = ?1",
            [legacy_session_path],
            |row| row.get(0),
        )?;
        let other_session_profile: Option<String> = conn.query_row(
            "SELECT profile_id FROM session_log_sync WHERE file_path = ?1",
            [other_session_path],
            |row| row.get(0),
        )?;
        let missing_session_profile: Option<String> = conn.query_row(
            "SELECT profile_id FROM session_log_sync WHERE file_path = ?1",
            [missing_session_path],
            |row| row.get(0),
        )?;

        assert_eq!(codex_request_profile.as_deref(), Some(selected_profile_id));
        assert!(claude_request_profile.is_none());
        assert_eq!(codex_rollup_profile, selected_profile_id);
        assert!(claude_rollup_profile.is_empty());
        assert_eq!(legacy_session_profile.as_deref(), Some(selected_profile_id));
        assert!(other_session_profile.is_none());
        assert!(missing_session_profile.is_none());
        Ok(())
    }

    /// 断言冲突聚合桶已被精确合并，且不会遗留空 Profile 桶。
    fn assert_merged_rollup(db: &Database, selected_profile_id: &str) -> Result<(), AppError> {
        let conn = db.conn.lock().expect("获取数据库锁");
        let merged: (i64, i64, i64, i64, i64, i64, f64, i64) = conn.query_row(
            "SELECT request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens,
                    CAST(total_cost_usd AS REAL), avg_latency_ms
             FROM usage_daily_rollups
             WHERE date = '2026-07-13' AND app_type = 'codex'
               AND provider_id = 'legacy-provider' AND model = 'gpt-5'
               AND profile_id = ?1",
            [selected_profile_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )?;
        let empty_rollup_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM usage_daily_rollups
             WHERE date = '2026-07-13' AND app_type = 'codex'
               AND provider_id = 'legacy-provider' AND model = 'gpt-5'
               AND profile_id = ''",
            [],
            |row| row.get(0),
        )?;

        assert_eq!(merged, (5, 3, 40, 60, 10, 12, 1.0, 160));
        assert_eq!(empty_rollup_count, 0, "合并后不得保留空 Profile 桶");
        Ok(())
    }
}
