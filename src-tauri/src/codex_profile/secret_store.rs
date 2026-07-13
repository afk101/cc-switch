use crate::codex_profile::{
    MigratedEnabledCodexProfile, CODEX_PROFILE_SECRET_DIRECTORY,
    CODEX_PROFILE_SECRET_PARENT_DIRECTORY, CODEX_PROFILE_TOKEN_FILENAME,
    LEGACY_PROXY_MANAGED_TOKEN, LOCAL_TOKEN_BYTES,
};
use crate::config::get_app_config_dir;
use crate::error::AppError;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 仅存放在 CC Switch 私有目录中的 Profile 本地监听凭证。
pub struct CodexProfileSecretStore {
    secret_root: PathBuf,
}

impl CodexProfileSecretStore {
    /// 使用默认的 CC Switch 私有密钥根目录构造存储。
    pub fn new() -> Self {
        Self::with_root(
            get_app_config_dir()
                .join(CODEX_PROFILE_SECRET_PARENT_DIRECTORY)
                .join(CODEX_PROFILE_SECRET_DIRECTORY),
        )
    }

    /// 使用指定私有根目录构造存储，供测试和受控迁移使用。
    pub fn with_root(secret_root: PathBuf) -> Self {
        Self { secret_root }
    }

    /// 创建或读取 Profile 的本地监听凭证，不会接触 CODEX_HOME。
    pub fn create(&self, profile_id: &str) -> Result<String, AppError> {
        match self.read(profile_id)? {
            Some(token) => Ok(token),
            None => self.write_token(profile_id, &generate_token()?),
        }
    }

    /// 读取 Profile 的本地监听凭证；不存在时返回空。
    pub fn read(&self, profile_id: &str) -> Result<Option<String>, AppError> {
        let token_path = self.token_path(profile_id)?;
        if !token_path.exists() {
            return Ok(None);
        }
        fs::read_to_string(&token_path)
            .map(Some)
            .map_err(|error| AppError::io(&token_path, error))
    }

    /// 重新生成 Profile 的随机本地监听凭证。
    pub fn rotate(&self, profile_id: &str) -> Result<String, AppError> {
        self.write_token(profile_id, &generate_token()?)
    }

    /// 仅接受迁移服务的已启用证明来初始化旧路由兼容凭证，不会读取或写入 CODEX_HOME。
    pub fn initialize_migrated_enabled_proxy_token(
        &self,
        migrated_profile: &MigratedEnabledCodexProfile,
        live_config: &str,
    ) -> Result<String, AppError> {
        let profile_id = migrated_profile.profile_id();
        if let Some(token) = self.read(profile_id)? {
            return Ok(token);
        }
        if live_config.contains(LEGACY_PROXY_MANAGED_TOKEN) {
            return self.write_token(profile_id, LEGACY_PROXY_MANAGED_TOKEN);
        }
        self.create(profile_id)
    }

    /// 删除唯一的 token 文件；Profile 私有目录为空时才一并移除。
    pub fn delete(&self, profile_id: &str) -> Result<(), AppError> {
        let token_path = self.token_path(profile_id)?;
        if token_path.exists() {
            fs::remove_file(&token_path).map_err(|error| AppError::io(&token_path, error))?;
        }
        let profile_dir = self.profile_dir(profile_id)?;
        if profile_dir.exists()
            && fs::read_dir(&profile_dir)
                .map_err(|error| AppError::io(&profile_dir, error))?
                .next()
                .is_none()
        {
            fs::remove_dir(&profile_dir).map_err(|error| AppError::io(&profile_dir, error))?;
        }
        Ok(())
    }

    /// 返回经校验的 Profile 对应 token 路径。
    pub fn token_path(&self, profile_id: &str) -> Result<PathBuf, AppError> {
        Ok(self
            .profile_dir(profile_id)?
            .join(CODEX_PROFILE_TOKEN_FILENAME))
    }

    /// 写入 token 并以同目录临时文件 rename 保证替换原子性。
    fn write_token(&self, profile_id: &str, token: &str) -> Result<String, AppError> {
        let token_path = self.token_path(profile_id)?;
        let profile_dir = token_path
            .parent()
            .ok_or_else(|| AppError::Config("无效的 Codex Profile 密钥路径".to_string()))?;
        fs::create_dir_all(profile_dir).map_err(|error| AppError::io(profile_dir, error))?;
        let temporary_path = profile_dir.join(format!(
            ".{CODEX_PROFILE_TOKEN_FILENAME}.{}.tmp",
            uuid::Uuid::new_v4()
        ));
        write_private_file(&temporary_path, token.as_bytes())?;
        if let Err(error) = replace_token_file(&temporary_path, &token_path) {
            // rename 失败后不保留包含明文 token 的临时文件。
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }
        Ok(token.to_string())
    }

    /// 根据已验证的 Profile 标识派生其私有目录。
    fn profile_dir(&self, profile_id: &str) -> Result<PathBuf, AppError> {
        validate_profile_id(profile_id)?;
        Ok(self.secret_root.join(profile_id))
    }
}

impl Default for CodexProfileSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

/// 限制 Profile 标识为稳定文件名，防止离开私有密钥根目录。
fn validate_profile_id(profile_id: &str) -> Result<(), AppError> {
    if profile_id.is_empty()
        || !profile_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(AppError::InvalidInput(
            "Codex Profile 标识包含不安全字符".to_string(),
        ));
    }
    Ok(())
}

/// 用操作系统 CSPRNG 填满 32 个随机字节并编码为 URL-safe 本地监听凭证。
fn generate_token() -> Result<String, AppError> {
    let mut bytes = [0_u8; LOCAL_TOKEN_BYTES];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| AppError::Config(format!("生成 Codex 本地监听凭证失败: {error}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// 以 Unix owner-only 权限写入尚未公开的临时 token 文件。
fn write_private_file(path: &Path, content: &[u8]) -> Result<(), AppError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| AppError::io(path, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| AppError::io(path, error))?;
    }
    file.write_all(content)
        .map_err(|error| AppError::io(path, error))?;
    file.flush().map_err(|error| AppError::io(path, error))
}

/// 将同目录临时 token 文件替换到正式路径。
fn replace_token_file(temporary_path: &Path, token_path: &Path) -> Result<(), AppError> {
    #[cfg(windows)]
    if token_path.exists() {
        fs::remove_file(token_path).map_err(|error| AppError::io(token_path, error))?;
    }
    fs::rename(temporary_path, token_path).map_err(|error| AppError::IoContext {
        context: format!(
            "原子替换 Codex Profile 本地凭证失败: {} -> {}",
            temporary_path.display(),
            token_path.display()
        ),
        source: error,
    })
}

#[cfg(test)]
mod codex_profile_secret_store {
    use super::*;
    use crate::codex_profile::{
        CodexProfileMigrationService, CodexProfileRepository, HomePathCanonicalizer,
        LegacyCodexProfileSnapshot, SystemHomePathCanonicalizer, SystemPortAvailability,
    };
    use crate::database::Database;
    use crate::error::AppError;
    use base64::Engine;
    use std::fs;
    use std::sync::Arc;

    /// 本地监听凭证必须按 Profile 隔离，且不会出现在数据库导出内容中。
    #[test]
    fn profile_secret_store_is_local_and_profile_scoped() -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let store = CodexProfileSecretStore::with_root(temp_dir.path().join("secrets"));

        let token_a = store.create("profile-a")?;
        let token_b = store.create("profile-b")?;
        let path_a = store.token_path("profile-a")?;
        let path_b = store.token_path("profile-b")?;
        let database_export = Database::memory()?.export_sql_string()?;

        assert_ne!(token_a, token_b);
        assert_ne!(path_a, path_b);
        assert!(!database_export.contains(&token_a));
        assert!(!database_export.contains(&token_b));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path_a)
                    .expect("读取权限")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        Ok(())
    }

    /// 已启用旧路由的兼容 token 初始化不得写入其 Home 配置。
    #[test]
    fn legacy_active_profile_keeps_compatible_token_without_home_write() -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let home = temp_dir.path().join("home");
        fs::create_dir_all(&home).expect("创建 Home");
        let config_path = home.join("config.toml");
        let config = "[model_providers.cc_switch]\nexperimental_bearer_token = \"PROXY_MANAGED\"\n";
        fs::write(&config_path, config).expect("写入旧路由配置");
        let before = fs::read(&config_path).expect("读取旧路由配置");
        let store = CodexProfileSecretStore::with_root(temp_dir.path().join("secrets"));
        let default_home = temp_dir.path().join(".codex");
        fs::create_dir_all(&default_home).expect("创建默认 Home");
        let db = Arc::new(Database::memory()?);
        let canonicalizer: Arc<dyn HomePathCanonicalizer> = Arc::new(SystemHomePathCanonicalizer);
        let repository = CodexProfileRepository::new(
            db.clone(),
            canonicalizer.clone(),
            Arc::new(SystemPortAvailability),
        );
        let migration =
            CodexProfileMigrationService::new(db, repository, canonicalizer, default_home);
        let migration_result = migration.migrate_with_result(LegacyCodexProfileSnapshot {
            override_home: Some(home.clone()),
            current_provider_id: None,
            route_enabled: true,
            failover_provider_ids: Vec::new(),
            live_backup_json: None,
        })?;
        let migrated_profile_id = migration_result.selected_profile_id.clone();
        let migrated_profile = migration_result
            .migrated_enabled_profile()
            .expect("已启用的旧路由迁移应产生兼容初始化证明");

        let token = store.initialize_migrated_enabled_proxy_token(
            migrated_profile,
            &fs::read_to_string(&config_path).expect("读取 live 配置"),
        )?;

        assert_eq!(token, LEGACY_PROXY_MANAGED_TOKEN);
        assert_eq!(
            store.read(&migrated_profile_id)?,
            Some(LEGACY_PROXY_MANAGED_TOKEN.to_string())
        );
        assert_eq!(fs::read(config_path).expect("读取 Home 配置"), before);
        Ok(())
    }

    /// 非迁移 Profile 即使碰巧出现旧占位符，也必须使用新的随机本地凭证。
    #[test]
    fn non_migrated_profile_never_reuses_proxy_managed_placeholder() -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let store = CodexProfileSecretStore::with_root(temp_dir.path().join("secrets"));

        let unrelated_config = "experimental_bearer_token = \"PROXY_MANAGED\"\n";
        let token = store.create("new-profile")?;

        assert_ne!(token, LEGACY_PROXY_MANAGED_TOKEN);
        assert!(unrelated_config.contains(LEGACY_PROXY_MANAGED_TOKEN));
        assert_eq!(
            URL_SAFE_NO_PAD
                .decode(token.as_bytes())
                .expect("解码 token")
                .len(),
            LOCAL_TOKEN_BYTES
        );
        Ok(())
    }

    /// 旋转后的 token 仍为 32 随机字节的 URL-safe 编码，并只删除自己的文件。
    #[test]
    fn rotate_and_delete_keep_profile_secret_scope() -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let store = CodexProfileSecretStore::with_root(temp_dir.path().join("secrets"));
        let first = store.create("profile-a")?;
        let rotated = store.rotate("profile-a")?;
        let profile_dir = store
            .token_path("profile-a")?
            .parent()
            .expect("获取 Profile 目录")
            .to_path_buf();
        fs::write(profile_dir.join("unrelated-note"), "保留文件").expect("写入保留文件");

        assert_ne!(first, rotated);
        assert_eq!(
            URL_SAFE_NO_PAD
                .decode(rotated.as_bytes())
                .expect("解码 token")
                .len(),
            LOCAL_TOKEN_BYTES
        );
        store.delete("profile-a")?;
        assert!(!store.token_path("profile-a")?.exists());
        assert!(profile_dir.exists());
        assert!(profile_dir.join("unrelated-note").exists());
        Ok(())
    }

    /// 非法 Profile 标识不能影响私有根目录以外的路径。
    #[test]
    fn secret_store_rejects_unsafe_profile_id() {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let store = CodexProfileSecretStore::with_root(temp_dir.path().join("secrets"));

        assert!(matches!(
            store.create("../outside"),
            Err(AppError::InvalidInput(_))
        ));
    }
}
