use crate::codex_profile::constants::{CODEX_ROUTE_LISTEN_HOST, FIRST_CUSTOM_CODEX_ROUTE_PORT};
use crate::codex_profile::{CodexProfile, CodexProfileRoute, CodexRuntimeStatus};
use crate::database::Database;
use crate::error::AppError;
use chrono::Utc;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 将用户输入的 CODEX_HOME 归一化为可比较路径。
pub trait HomePathCanonicalizer: Send + Sync {
    fn canonicalize(&self, input: &Path) -> Result<PathBuf, AppError>;
}

/// 检查端口是否可在指定地址绑定。
pub trait PortAvailability: Send + Sync {
    fn is_available(&self, host: &str, port: u16) -> Result<bool, AppError>;
}

/// 使用操作系统文件系统解析真实路径。
pub struct SystemHomePathCanonicalizer;

impl HomePathCanonicalizer for SystemHomePathCanonicalizer {
    fn canonicalize(&self, input: &Path) -> Result<PathBuf, AppError> {
        std::fs::canonicalize(input).map_err(|error| AppError::io(input, error))
    }
}

/// 使用临时 TCP 绑定确认端口未被其他进程占用。
pub struct SystemPortAvailability;

impl PortAvailability for SystemPortAvailability {
    fn is_available(&self, host: &str, port: u16) -> Result<bool, AppError> {
        match TcpListener::bind((host, port)) {
            Ok(listener) => {
                drop(listener);
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => Ok(false),
            Err(error) => Err(AppError::IoContext {
                context: format!("检查 Codex 路由端口 {host}:{port} 失败"),
                source: error,
            }),
        }
    }
}

/// Profile 持久化、路径唯一性校验与端口分配的门面。
pub struct CodexProfileRepository {
    db: Arc<Database>,
    canonicalizer: Arc<dyn HomePathCanonicalizer>,
    port_availability: Arc<dyn PortAvailability>,
}

impl CodexProfileRepository {
    /// 组合数据库与可替换的路径、端口协作者。
    pub fn new(
        db: Arc<Database>,
        canonicalizer: Arc<dyn HomePathCanonicalizer>,
        port_availability: Arc<dyn PortAvailability>,
    ) -> Self {
        Self {
            db,
            canonicalizer,
            port_availability,
        }
    }

    /// 校验新 CODEX_HOME 的规范化路径尚未被其他 Profile 使用。
    pub fn validate_new_home(&self, input: &Path) -> Result<PathBuf, AppError> {
        let canonical_path = self.canonicalizer.canonicalize(input)?;
        self.ensure_home_is_available(&canonical_path, None)?;
        Ok(canonical_path)
    }

    /// 创建新的 Profile、初始空路由记录与固定监听端口。
    pub fn create_profile(&self, name: &str, home_path: &Path) -> Result<CodexProfile, AppError> {
        let profile_name = validate_profile_name(name)?;
        let canonical_home = self.validate_new_home(home_path)?;
        let now = Utc::now().timestamp();
        let profile = CodexProfile {
            id: uuid::Uuid::new_v4().to_string(),
            name: profile_name,
            canonical_home_path: canonical_home.to_string_lossy().into_owned(),
            listen_port: self.allocate_port()?,
            created_at: now,
            updated_at: now,
        };

        self.db.insert_codex_profile(&profile)?;
        if let Err(error) = self.save_initial_route(&profile.id, now) {
            let _ = self.db.delete_codex_profile(&profile.id);
            return Err(error);
        }
        Ok(profile)
    }

    /// 更新 Profile 的显示名称，不改变 Home、端口或运行状态。
    pub fn rename_profile(&self, profile_id: &str, name: &str) -> Result<CodexProfile, AppError> {
        let mut profile = self.db.get_codex_profile(profile_id)?;
        profile.name = validate_profile_name(name)?;
        profile.updated_at = Utc::now().timestamp();
        self.db.update_codex_profile(&profile)?;
        Ok(profile)
    }

    /// 在非默认且未运行时重新绑定 Profile 的 CODEX_HOME。
    pub fn rebind_profile(
        &self,
        profile_id: &str,
        home_path: &Path,
        runtime_status: CodexRuntimeStatus,
    ) -> Result<CodexProfile, AppError> {
        let mut profile = self.db.get_codex_profile(profile_id)?;
        profile.validate_rebind()?;
        if runtime_status.is_active() {
            return Err(AppError::InvalidInput(
                "运行中的 Codex Profile 不可重新绑定 CODEX_HOME".to_string(),
            ));
        }

        let canonical_home = self.canonicalizer.canonicalize(home_path)?;
        self.ensure_home_is_available(&canonical_home, Some(profile_id))?;
        profile.canonical_home_path = canonical_home.to_string_lossy().into_owned();
        profile.updated_at = Utc::now().timestamp();
        self.db.update_codex_profile(&profile)?;
        Ok(profile)
    }

    /// 从自定义端口起始值搜索未被数据库或操作系统占用的端口。
    pub fn allocate_port(&self) -> Result<u16, AppError> {
        let reserved_ports = self.reserved_ports()?;
        for port in FIRST_CUSTOM_CODEX_ROUTE_PORT..=u16::MAX {
            if reserved_ports.contains(&port) {
                continue;
            }
            if self
                .port_availability
                .is_available(CODEX_ROUTE_LISTEN_HOST, port)?
            {
                return Ok(port);
            }
        }
        Err(AppError::Config("没有可分配的 Codex 路由端口".to_string()))
    }

    /// 校验给定规范化路径未被除可选自身外的 Profile 占用。
    fn ensure_home_is_available(
        &self,
        canonical_home: &Path,
        excluded_profile_id: Option<&str>,
    ) -> Result<(), AppError> {
        let canonical_home = canonical_home.to_string_lossy();
        let existing_profile = self
            .db
            .list_codex_profiles()?
            .into_iter()
            .find(|profile| {
                profile.canonical_home_path == canonical_home
                    && Some(profile.id.as_str()) != excluded_profile_id
            });

        match existing_profile {
            Some(profile) => Err(AppError::DuplicateCodexHome {
                profile_id: profile.id,
            }),
            None => Ok(()),
        }
    }

    /// 收集已持久化的监听端口，防止自动分配与其他 Profile 冲突。
    fn reserved_ports(&self) -> Result<std::collections::HashSet<u16>, AppError> {
        Ok(self
            .db
            .list_codex_profiles()?
            .into_iter()
            .map(|profile| profile.listen_port)
            .collect())
    }

    /// 为新 Profile 创建关闭状态的空路由记录。
    fn save_initial_route(&self, profile_id: &str, updated_at: i64) -> Result<(), AppError> {
        self.db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: profile_id.to_string(),
            current_provider_id: None,
            enabled: false,
            updated_at,
        })
    }
}

/// 校验并规范化用户输入的 Profile 显示名称。
fn validate_profile_name(name: &str) -> Result<String, AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput(
            "Codex Profile 名称不能为空".to_string(),
        ));
    }
    Ok(name.to_string())
}

#[cfg(test)]
fn test_profile(id: &str, listen_port: u16) -> CodexProfile {
    CodexProfile {
        id: id.to_string(),
        name: id.to_string(),
        canonical_home_path: format!("/tmp/{id}"),
        listen_port,
        created_at: 1,
        updated_at: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::{CodexProfileRepository, SystemHomePathCanonicalizer, SystemPortAvailability};
    use crate::database::Database;
    use crate::error::AppError;
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::Arc;

    #[cfg(unix)]
    #[test]
    fn canonical_home_path_rejects_symlink_duplicate() -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let real_home = temp_dir.path().join("real-home");
        let symlink_home = temp_dir.path().join("linked-home");
        std::fs::create_dir(&real_home).expect("创建真实 Home");
        std::os::unix::fs::symlink(&real_home, &symlink_home).expect("创建 Home 软链接");

        let db = Arc::new(Database::memory()?);
        let repository = CodexProfileRepository::new(
            db,
            Arc::new(SystemHomePathCanonicalizer),
            Arc::new(SystemPortAvailability),
        );
        repository.create_profile("工作", &real_home)?;

        assert!(matches!(
            repository.validate_new_home(&symlink_home),
            Err(AppError::DuplicateCodexHome { profile_id }) if !profile_id.is_empty()
        ));
        Ok(())
    }

    #[test]
    fn allocate_port_skips_reserved_and_bound_ports() -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let repository = CodexProfileRepository::new(
            db.clone(),
            Arc::new(SystemHomePathCanonicalizer),
            Arc::new(SystemPortAvailability),
        );
        db.insert_codex_profile(&super::test_profile("reserved", 15_722))?;
        let _bound_port = TcpListener::bind("127.0.0.1:15723").expect("占用测试端口");

        assert_eq!(repository.allocate_port()?, 15_724);
        Ok(())
    }

    #[test]
    fn default_profile_has_no_capability_restrictions() -> Result<(), AppError> {
        let profile = super::CodexProfile::default_profile()?;

        assert!(profile.validate_official_operation().is_ok());
        assert!(profile.validate_route_operation().is_ok());
        assert!(matches!(
            profile.validate_rebind(),
            Err(AppError::DefaultCodexProfileImmutable)
        ));
        assert!(matches!(
            profile.validate_delete(),
            Err(AppError::DefaultCodexProfileImmutable)
        ));
        Ok(())
    }

    #[test]
    fn rebind_rejects_running_profile() -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let old_home = temp_dir.path().join("old");
        let new_home = temp_dir.path().join("new");
        std::fs::create_dir(&old_home).expect("创建旧 Home");
        std::fs::create_dir(&new_home).expect("创建新 Home");
        let repository = CodexProfileRepository::new(
            Arc::new(Database::memory()?),
            Arc::new(SystemHomePathCanonicalizer),
            Arc::new(SystemPortAvailability),
        );
        let profile = repository.create_profile("工作", &old_home)?;

        assert!(matches!(
            repository.rebind_profile(&profile.id, Path::new(&new_home), super::CodexRuntimeStatus::Running),
            Err(AppError::InvalidInput(_))
        ));
        Ok(())
    }

    #[test]
    fn create_rename_and_rebind_profile_persists_canonical_home() -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let old_home = temp_dir.path().join("old");
        let new_home = temp_dir.path().join("new");
        std::fs::create_dir(&old_home).expect("创建旧 Home");
        std::fs::create_dir(&new_home).expect("创建新 Home");
        let repository = CodexProfileRepository::new(
            Arc::new(Database::memory()?),
            Arc::new(SystemHomePathCanonicalizer),
            Arc::new(SystemPortAvailability),
        );
        let profile = repository.create_profile(" 工作 ", &old_home)?;

        let renamed = repository.rename_profile(&profile.id, "新名称")?;
        let rebound = repository.rebind_profile(
            &profile.id,
            &new_home,
            super::CodexRuntimeStatus::Stopped,
        )?;

        assert_eq!(renamed.name, "新名称");
        assert_eq!(
            rebound.canonical_home_path,
            std::fs::canonicalize(&new_home)
                .expect("规范化新 Home")
                .to_string_lossy()
        );
        Ok(())
    }
}
