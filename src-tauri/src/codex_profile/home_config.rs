use crate::codex_config::codex_config_path_for_home;
use crate::error::AppError;
use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Codex Home 配置文件的当前快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexLiveConfigSnapshot {
    config_path: PathBuf,
    content: Option<Vec<u8>>,
    fingerprint: String,
}

/// 由当前配置和目标路由配置组成的无副作用写入计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexRouteConfigPlan {
    home_path: PathBuf,
    previous: CodexLiveConfigSnapshot,
    target_content: Vec<u8>,
    target_fingerprint: String,
    /// 预留给路由配置构建器记录模型选择变化，文件写入阶段不解释该字段。
    model_changes: Vec<String>,
}

/// 持久化在 Profile 路由关系中的最小 Home 恢复信息。
#[derive(Serialize, Deserialize)]
struct CodexRouteBackup {
    previous_content: Option<Vec<u8>>,
    previous_fingerprint: String,
    target_fingerprint: String,
}

/// 读写单个 Home 的 `config.toml`，使原子替换失败可被行为测试注入。
pub trait CodexHomeFileOps: Send + Sync {
    fn read(&self, path: &Path) -> Result<Option<Vec<u8>>, AppError>;
    fn write_atomic(&self, path: &Path, content: &[u8]) -> Result<(), AppError>;
    fn remove_file(&self, path: &Path) -> Result<(), AppError>;
}

/// 基于本地文件系统的 Home 配置文件操作。
pub struct SystemCodexHomeFileOps;

impl CodexHomeFileOps for SystemCodexHomeFileOps {
    fn read(&self, path: &Path) -> Result<Option<Vec<u8>>, AppError> {
        if !path.exists() {
            return Ok(None);
        }
        fs::read(path)
            .map(Some)
            .map_err(|error| AppError::io(path, error))
    }

    fn write_atomic(&self, path: &Path, content: &[u8]) -> Result<(), AppError> {
        crate::config::atomic_write(path, content)
    }

    fn remove_file(&self, path: &Path) -> Result<(), AppError> {
        if path.exists() {
            fs::remove_file(path).map_err(|error| AppError::io(path, error))?;
        }
        Ok(())
    }
}

/// 只管理显式 Home 的 `config.toml`，不读取、写入、备份或复制 `auth.json`。
pub struct CodexHomeConfigService {
    file_ops: Arc<dyn CodexHomeFileOps>,
}

impl CodexHomeConfigService {
    /// 使用系统文件操作构造配置服务。
    pub fn system() -> Self {
        Self::new(Arc::new(SystemCodexHomeFileOps))
    }

    /// 使用可注入的文件操作构造配置服务。
    pub fn new(file_ops: Arc<dyn CodexHomeFileOps>) -> Self {
        Self { file_ops }
    }

    /// 读取显式 Home 的主配置及其内容指纹。
    pub fn inspect(&self, home: &Path) -> Result<CodexLiveConfigSnapshot, AppError> {
        let config_path = codex_config_path_for_home(home);
        let content = self.file_ops.read(&config_path)?;
        Ok(CodexLiveConfigSnapshot {
            config_path,
            fingerprint: fingerprint_content(content.as_deref()),
            content,
        })
    }

    /// 构造路由接管计划，不在此阶段写入任何文件。
    pub fn build_route_plan(
        &self,
        home: &Path,
        route_config: &str,
    ) -> Result<CodexRouteConfigPlan, AppError> {
        crate::codex_config::validate_config_toml(route_config)?;
        let previous = self.inspect(home)?;
        let target_content = route_config.as_bytes().to_vec();
        Ok(CodexRouteConfigPlan {
            home_path: home.to_path_buf(),
            target_fingerprint: fingerprint_content(Some(&target_content)),
            target_content,
            previous,
            model_changes: Vec::new(),
        })
    }

    /// 从当前 Home 配置构造指定 Profile 端口的接管计划。
    pub fn build_profile_route_plan(
        &self,
        home: &Path,
        listen_port: u16,
        provider: Option<&Provider>,
    ) -> Result<CodexRouteConfigPlan, AppError> {
        let current = self.inspect(home)?;
        let current_toml = current
            .content
            .as_deref()
            .map(std::str::from_utf8)
            .transpose()
            .map_err(|error| AppError::Config(format!("Codex config.toml 不是 UTF-8: {error}")))?
            .unwrap_or("");
        self.build_route_plan(
            home,
            &build_codex_profile_route_toml(current_toml, listen_port, provider),
        )
    }

    /// 在当前配置未被外部修改时，原子应用路由接管计划。
    pub fn apply_route_plan(&self, plan: &CodexRouteConfigPlan) -> Result<(), AppError> {
        let config_path = validated_plan_config_path(plan)?;
        let current = self.inspect(&plan.home_path)?;
        ensure_fingerprint(&plan.previous.fingerprint, &current.fingerprint)?;
        self.file_ops
            .write_atomic(&config_path, &plan.target_content)
    }

    /// 仅在仍由本计划接管时恢复接管前的配置，避免覆盖外部变更。
    pub fn restore(&self, plan: &CodexRouteConfigPlan) -> Result<(), AppError> {
        let config_path = validated_plan_config_path(plan)?;
        let current = self.inspect(&plan.home_path)?;
        ensure_fingerprint(&plan.target_fingerprint, &current.fingerprint)?;
        match &plan.previous.content {
            Some(content) => self.file_ops.write_atomic(&config_path, content),
            None => self.file_ops.remove_file(&config_path),
        }
    }

    /// 将计划的原始配置编码为 Profile 私有恢复备份。
    pub fn serialize_backup(&self, plan: &CodexRouteConfigPlan) -> Result<String, AppError> {
        serde_json::to_string(&CodexRouteBackup {
            previous_content: plan.previous.content.clone(),
            previous_fingerprint: plan.previous.fingerprint.clone(),
            target_fingerprint: plan.target_fingerprint.clone(),
        })
        .map_err(|error| AppError::JsonSerialize { source: error })
    }

    /// 仅当 Home 仍是该 Profile 接管版本时恢复其备份配置。
    pub fn restore_backup(&self, home: &Path, backup_json: &str) -> Result<(), AppError> {
        let backup: CodexRouteBackup = serde_json::from_str(backup_json)
            .map_err(|error| AppError::Config(format!("Codex 路由备份无效: {error}")))?;
        let current = self.inspect(home)?;
        if current.fingerprint == backup.previous_fingerprint {
            return Ok(());
        }
        ensure_fingerprint(&backup.target_fingerprint, &current.fingerprint)?;
        let config_path = codex_config_path_for_home(home);
        match backup.previous_content {
            Some(content) => self.file_ops.write_atomic(&config_path, &content),
            None => self.file_ops.remove_file(&config_path),
        }
    }
}

/// 根据指定 Profile 的本地端口构造纯 Codex 路由配置，不读取全局代理配置。
pub fn build_codex_profile_route_toml(
    toml_str: &str,
    listen_port: u16,
    provider: Option<&Provider>,
) -> String {
    let proxy_url = format!("http://127.0.0.1:{listen_port}/v1");
    let updated = crate::codex_config::update_codex_toml_field(toml_str, "base_url", &proxy_url)
        .unwrap_or_else(|_| toml_str.to_string());
    let mut updated =
        crate::codex_config::update_codex_toml_field(&updated, "wire_api", "responses")
            .unwrap_or(updated);

    if let Some(upstream_model) =
        provider.and_then(crate::proxy::providers::codex_provider_upstream_model)
    {
        updated = crate::codex_config::update_codex_toml_field(&updated, "model", &upstream_model)
            .unwrap_or(updated);
    }
    updated
}

/// 从计划的 Home 重派生配置路径，并拒绝任何不一致的内部数据。
fn validated_plan_config_path(plan: &CodexRouteConfigPlan) -> Result<PathBuf, AppError> {
    let config_path = codex_config_path_for_home(&plan.home_path);
    if config_path != plan.previous.config_path {
        return Err(AppError::InvalidInput(
            "Codex 路由配置计划的 Home 与配置路径不一致".to_string(),
        ));
    }
    Ok(config_path)
}

/// 为存在状态和原始字节共同计算稳定指纹，区分缺失文件与空文件。
fn fingerprint_content(content: Option<&[u8]>) -> String {
    let mut hasher = Sha256::new();
    match content {
        Some(content) => {
            hasher.update([1]);
            hasher.update(content);
        }
        None => hasher.update([0]),
    }
    format!("{:x}", hasher.finalize())
}

/// 校验实际配置仍与计划期望的指纹一致。
fn ensure_fingerprint(expected: &str, actual: &str) -> Result<(), AppError> {
    if expected == actual {
        return Ok(());
    }
    Err(AppError::CodexLiveConfigConflict {
        expected_fingerprint: expected.to_string(),
        actual_fingerprint: actual.to_string(),
    })
}

#[cfg(test)]
mod codex_home_config {
    use super::*;
    use crate::codex_config::{codex_auth_path_for_home, codex_config_path_for_home};
    use crate::error::AppError;
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    /// 通过拒绝原子替换模拟同目录 rename 失败。
    struct FailingRenameFileOps;

    impl CodexHomeFileOps for FailingRenameFileOps {
        fn read(&self, path: &Path) -> Result<Option<Vec<u8>>, AppError> {
            if path.exists() {
                fs::read(path)
                    .map(Some)
                    .map_err(|error| AppError::io(path, error))
            } else {
                Ok(None)
            }
        }

        fn write_atomic(&self, _path: &Path, _content: &[u8]) -> Result<(), AppError> {
            Err(AppError::IoContext {
                context: "模拟原子 rename 失败".to_string(),
                source: std::io::Error::other("rename failed"),
            })
        }

        fn remove_file(&self, _path: &Path) -> Result<(), AppError> {
            Ok(())
        }
    }

    /// 路由配置只能写入显式指定的 Home，且不得触碰任何认证文件。
    #[test]
    fn writes_route_config_only_to_explicit_home() -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let home_a = temp_dir.path().join("home-a");
        let home_b = temp_dir.path().join("home-b");
        fs::create_dir_all(&home_a).expect("创建 A Home");
        fs::create_dir_all(&home_b).expect("创建 B Home");
        fs::write(codex_auth_path_for_home(&home_a), b"{\"token\":\"a\"}").expect("写入 A 认证");
        fs::write(codex_auth_path_for_home(&home_b), b"{\"token\":\"b\"}").expect("写入 B 认证");
        fs::write(codex_config_path_for_home(&home_a), "model = \"a\"\n").expect("写入 A 配置");
        fs::write(codex_config_path_for_home(&home_b), "model = \"b\"\n").expect("写入 B 配置");
        let auth_a_before = fs::read(codex_auth_path_for_home(&home_a)).expect("读取 A 认证");
        let auth_b_before = fs::read(codex_auth_path_for_home(&home_b)).expect("读取 B 认证");
        let config_b_before = fs::read(codex_config_path_for_home(&home_b)).expect("读取 B 配置");

        let service = CodexHomeConfigService::system();
        let plan = service.build_route_plan(&home_a, "model = \"route-a-15722\"\n")?;
        service.apply_route_plan(&plan)?;

        assert_eq!(
            fs::read_to_string(codex_config_path_for_home(&home_a)).expect("读取 A 配置"),
            "model = \"route-a-15722\"\n"
        );
        assert_eq!(
            fs::read(codex_config_path_for_home(&home_b)).expect("读取 B 配置"),
            config_b_before
        );
        assert_eq!(
            fs::read(codex_auth_path_for_home(&home_a)).expect("读取 A 认证"),
            auth_a_before
        );
        assert_eq!(
            fs::read(codex_auth_path_for_home(&home_b)).expect("读取 B 认证"),
            auth_b_before
        );
        Ok(())
    }

    /// 原子替换失败时必须保留原配置与计划中保存的原始指纹。
    #[test]
    fn atomic_route_write_preserves_previous_config_on_failure() -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let home = temp_dir.path().join("home");
        fs::create_dir_all(&home).expect("创建 Home");
        let config_path = codex_config_path_for_home(&home);
        fs::write(&config_path, "model = \"before\"\n").expect("写入旧配置");
        let service = CodexHomeConfigService::new(Arc::new(FailingRenameFileOps));
        let plan = service.build_route_plan(&home, "model = \"route\"\n")?;
        let previous_fingerprint = plan.previous.fingerprint.clone();

        assert!(service.apply_route_plan(&plan).is_err());
        assert_eq!(
            fs::read_to_string(config_path).expect("读取旧配置"),
            "model = \"before\"\n"
        );
        assert_eq!(plan.previous.fingerprint, previous_fingerprint);
        Ok(())
    }

    /// 接管期间的外部变更必须以指纹冲突返回，不能被恢复流程覆盖。
    #[test]
    fn restore_rejects_external_live_config_change() -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let home = temp_dir.path().join("home");
        fs::create_dir_all(&home).expect("创建 Home");
        let config_path = codex_config_path_for_home(&home);
        fs::write(&config_path, "model = \"before\"\n").expect("写入旧配置");
        let service = CodexHomeConfigService::system();
        let plan = service.build_route_plan(&home, "model = \"route\"\n")?;
        service.apply_route_plan(&plan)?;
        fs::write(&config_path, "model = \"external\"\n").expect("写入外部配置");

        let error = service.restore(&plan).expect_err("外部变更必须拒绝恢复");
        assert!(matches!(
            error,
            AppError::CodexLiveConfigConflict {
                expected_fingerprint,
                actual_fingerprint,
            } if expected_fingerprint == plan.target_fingerprint && actual_fingerprint != expected_fingerprint
        ));
        assert_eq!(
            fs::read_to_string(config_path).expect("读取外部配置"),
            "model = \"external\"\n"
        );
        Ok(())
    }

    /// 即使内部计划被篡改为另一 Home 的路径，也不得跨 Home 写入配置。
    #[test]
    fn tampered_route_plan_cannot_write_another_home() -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let home_a = temp_dir.path().join("home-a");
        let home_b = temp_dir.path().join("home-b");
        fs::create_dir_all(&home_a).expect("创建 A Home");
        fs::create_dir_all(&home_b).expect("创建 B Home");
        let config_a = codex_config_path_for_home(&home_a);
        let config_b = codex_config_path_for_home(&home_b);
        fs::write(&config_a, "model = \"a\"\n").expect("写入 A 配置");
        fs::write(&config_b, "model = \"b\"\n").expect("写入 B 配置");
        let service = CodexHomeConfigService::system();
        let mut plan = service.build_route_plan(&home_a, "model = \"route\"\n")?;
        plan.previous.config_path = config_b.clone();

        assert!(matches!(
            service.apply_route_plan(&plan),
            Err(AppError::InvalidInput(_))
        ));
        assert_eq!(
            fs::read_to_string(config_a).expect("读取 A 配置"),
            "model = \"a\"\n"
        );
        assert_eq!(
            fs::read_to_string(config_b).expect("读取 B 配置"),
            "model = \"b\"\n"
        );
        Ok(())
    }
}
