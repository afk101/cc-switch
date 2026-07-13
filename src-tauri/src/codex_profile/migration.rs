//! 旧版单例 Codex 配置向 Profile 的一次性迁移。

use crate::app_config::AppType;
use crate::codex_profile::{
    CodexProfile, CodexProfileRepository, HomePathCanonicalizer, DEFAULT_CODEX_PROFILE_ID,
    LEGACY_CODEX_ROUTE_PORT,
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

    /// 将旧单例 Codex 状态幂等映射到 Profile，不修改任何 Home 文件。
    pub fn migrate(&self, snapshot: LegacyCodexProfileSnapshot) -> Result<String, AppError> {
        let default_home = self.canonicalizer.canonicalize(&self.default_home)?;
        let actual_home = match snapshot.override_home.as_ref() {
            Some(override_home) => self.canonicalizer.canonicalize(override_home)?,
            None => default_home.clone(),
        };
        let actual_home_is_default = actual_home == default_home;
        let plan = self.plan_migration(&default_home, &actual_home, actual_home_is_default)?;
        self.apply_migration_plan(&plan, &snapshot)?;
        Ok(plan.selected_profile_id)
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
                }),
                None => Ok(LegacyCodexProfileMigrationPlan {
                    profiles_to_insert: vec![CodexProfile::default_profile(
                        default_home.to_string_lossy().into_owned(),
                        now,
                    )],
                    profiles_to_update: Vec::new(),
                    selected_profile_id: DEFAULT_CODEX_PROFILE_ID.to_string(),
                    should_apply_legacy_state: true,
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
        }
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
}
