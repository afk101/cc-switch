//! 旧版单例 Codex 配置向 Profile 的一次性迁移。

use crate::app_config::AppType;
use crate::codex_profile::{
    CodexProfile, CodexProfileRepository, CodexProfileRoute, HomePathCanonicalizer,
    DEFAULT_CODEX_PROFILE_ID, LEGACY_CODEX_ROUTE_PORT,
};
use crate::database::Database;
use crate::error::AppError;
use chrono::Utc;
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

        let (default_profile, default_profile_created) =
            self.ensure_default_profile(&default_home, actual_home_is_default)?;
        if actual_home_is_default {
            if default_profile_created {
                self.apply_legacy_state(&default_profile.id, &snapshot)?;
            }
            return Ok(default_profile.id);
        }

        let (actual_profile, actual_profile_created) =
            self.ensure_legacy_override_profile(&actual_home)?;
        self.assign_legacy_port(&default_profile.id, &actual_profile.id)?;
        if actual_profile_created {
            self.apply_legacy_state(&actual_profile.id, &snapshot)?;
        }
        Ok(actual_profile.id)
    }

    /// 查找或创建默认 Profile；外部 override 场景为旧端口预留一个临时可用端口。
    fn ensure_default_profile(
        &self,
        default_home: &std::path::Path,
        actual_home_is_default: bool,
    ) -> Result<(CodexProfile, bool), AppError> {
        if let Some(profile) = self.find_profile_by_home(default_home)? {
            return Ok((profile, false));
        }

        if actual_home_is_default {
            return self
                .repository
                .create_default_profile(default_home)
                .map(|profile| (profile, true));
        }

        let now = Utc::now().timestamp();
        let profile = CodexProfile {
            id: DEFAULT_CODEX_PROFILE_ID.to_string(),
            name: crate::codex_profile::DEFAULT_CODEX_PROFILE_NAME.to_string(),
            canonical_home_path: default_home.to_string_lossy().into_owned(),
            listen_port: self.repository.allocate_port()?,
            created_at: now,
            updated_at: now,
        };
        self.db.create_codex_profile_with_empty_route(&profile)?;
        Ok((profile, true))
    }

    /// 查找或创建旧 override 对应的“原 Codex 配置”Profile。
    fn ensure_legacy_override_profile(
        &self,
        actual_home: &std::path::Path,
    ) -> Result<(CodexProfile, bool), AppError> {
        if let Some(profile) = self.find_profile_by_home(actual_home)? {
            return Ok((profile, false));
        }
        self.repository
            .create_profile("原 Codex 配置", actual_home)
            .map(|profile| (profile, true))
    }

    /// 将旧版固定端口归属实际旧 Home，避免默认 Profile 抢占旧监听器。
    fn assign_legacy_port(
        &self,
        default_profile_id: &str,
        actual_profile_id: &str,
    ) -> Result<(), AppError> {
        let actual_profile = self.db.get_codex_profile(actual_profile_id)?;
        if actual_profile.listen_port == LEGACY_CODEX_ROUTE_PORT {
            return Ok(());
        }

        let mut default_profile = self.db.get_codex_profile(default_profile_id)?;
        if default_profile.listen_port == LEGACY_CODEX_ROUTE_PORT {
            default_profile.listen_port = self.repository.allocate_port()?;
            default_profile.updated_at = Utc::now().timestamp();
            self.db.update_codex_profile(&default_profile)?;
        }

        let mut actual_profile = actual_profile;
        actual_profile.listen_port = LEGACY_CODEX_ROUTE_PORT;
        actual_profile.updated_at = Utc::now().timestamp();
        self.db.update_codex_profile(&actual_profile)
    }

    /// 将旧单例状态写入本次新建的 Profile。
    fn apply_legacy_state(
        &self,
        profile_id: &str,
        snapshot: &LegacyCodexProfileSnapshot,
    ) -> Result<(), AppError> {
        let route = CodexProfileRoute {
            profile_id: profile_id.to_string(),
            current_provider_id: snapshot.current_provider_id.clone(),
            enabled: snapshot.route_enabled,
            updated_at: Utc::now().timestamp(),
        };
        self.db.save_codex_profile_route(&route)?;
        self.db
            .replace_codex_profile_failovers(profile_id, &snapshot.failover_provider_ids)?;
        Ok(())
    }

    /// 按规范化 Home 路径定位已有 Profile，避免重复创建。
    fn find_profile_by_home(
        &self,
        canonical_home: &std::path::Path,
    ) -> Result<Option<CodexProfile>, AppError> {
        let canonical_home = canonical_home.to_string_lossy();
        Ok(self
            .db
            .list_codex_profiles()?
            .into_iter()
            .find(|profile| profile.canonical_home_path == canonical_home))
    }
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
        assert_eq!(
            db.get_codex_profile_route(&selected_profile_id)?
                .expect("读取旧 Home 路由"),
            crate::codex_profile::CodexProfileRoute {
                profile_id: selected_profile_id.clone(),
                current_provider_id: Some("current-provider".to_string()),
                enabled: true,
                updated_at: db
                    .get_codex_profile_route(&selected_profile_id)?
                    .expect("再次读取旧 Home 路由")
                    .updated_at,
            }
        );
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
}
