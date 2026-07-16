use crate::codex_config::{
    codex_config_path_for_home, CodexCatalogToolProfile, CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME,
};
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

/// 模型目录辅助文件的原内容与目标内容。
#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexAuxiliaryFilePlan {
    path: PathBuf,
    previous_content: Option<Vec<u8>>,
    previous_fingerprint: String,
    target_content: Vec<u8>,
    target_fingerprint: String,
}

/// 单个 Profile Home 的供应商直连配置计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexDirectProviderConfigPlan {
    config: CodexRouteConfigPlan,
    model_catalog: Option<CodexAuxiliaryFilePlan>,
}

impl CodexRouteConfigPlan {
    /// 返回计划写入后的目标指纹，不暴露目标配置正文。
    pub fn target_fingerprint(&self) -> &str {
        &self.target_fingerprint
    }

    /// 返回计划构造时看到的 Home 指纹，不暴露当前配置正文。
    pub fn previous_fingerprint(&self) -> &str {
        &self.previous.fingerprint
    }
}

/// 启动修复时对当前 Home 的安全所有权分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexHomeReconcileOwnership {
    /// Home 已经是当前 token 对应的目标配置。
    Current,
    /// Home 与路由备份记录的旧 target 完全匹配。
    RouteOwned,
    /// Home 是端口匹配的旧 `PROXY_MANAGED` 配置。
    LegacyManaged,
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
        listener_token: &str,
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
            &build_codex_profile_route_toml(current_toml, listen_port, provider, listener_token),
        )
    }

    /// 构造指定 Profile Home 的供应商直连计划，不读取或写入该 Home 的认证文件。
    pub fn build_direct_provider_plan(
        &self,
        home: &Path,
        provider: &Provider,
    ) -> Result<CodexDirectProviderConfigPlan, AppError> {
        let mut settings = provider.settings_config.clone();
        crate::codex_config::apply_codex_unified_session_bucket_to_settings(
            provider.category.as_deref(),
            &mut settings,
        )?;
        let config_text = settings
            .get("config")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let direct_config = if provider.category.as_deref() == Some("official") {
            config_text.to_string()
        } else {
            let auth = settings.get("auth").unwrap_or(&serde_json::Value::Null);
            crate::codex_config::prepare_codex_provider_live_config(auth, config_text)?
        };
        let catalog_profile = CodexCatalogToolProfile::from_api_format(
            provider
                .meta
                .as_ref()
                .and_then(|meta| meta.api_format.as_deref()),
        );
        let prepared = crate::codex_config::prepare_codex_config_with_model_catalog(
            &settings,
            &direct_config,
            catalog_profile,
        )?;
        let config = self.build_route_plan(home, &prepared.config_text)?;
        let model_catalog = prepared
            .model_catalog
            .map(|catalog| {
                serde_json::to_vec_pretty(&catalog)
                    .map_err(|source| AppError::JsonSerialize { source })
            })
            .transpose()?
            .map(|target_content| {
                let path = home.join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
                let previous_content = self.file_ops.read(&path)?;
                Ok::<CodexAuxiliaryFilePlan, AppError>(CodexAuxiliaryFilePlan {
                    previous_fingerprint: fingerprint_content(previous_content.as_deref()),
                    target_fingerprint: fingerprint_content(Some(&target_content)),
                    path,
                    previous_content,
                    target_content,
                })
            })
            .transpose()?;
        Ok(CodexDirectProviderConfigPlan {
            config,
            model_catalog,
        })
    }

    /// 原子应用供应商直连计划，失败时不改变计划构造时的 Home 内容。
    pub fn apply_direct_provider_plan(
        &self,
        plan: &CodexDirectProviderConfigPlan,
    ) -> Result<(), AppError> {
        if let Some(catalog) = &plan.model_catalog {
            self.apply_auxiliary_plan(catalog)?;
        }
        if let Err(error) = self.apply_route_plan(&plan.config) {
            let compensation_error = plan
                .model_catalog
                .as_ref()
                .and_then(|catalog| self.restore_auxiliary_plan(catalog).err());
            return match compensation_error {
                Some(compensation) => Err(AppError::Message(format!(
                    "应用 Codex 直连配置失败: {error}；模型目录补偿失败: {compensation}"
                ))),
                None => Err(error),
            };
        }
        Ok(())
    }

    /// 仅在 Home 仍由该直连计划持有时恢复应用前配置。
    pub fn restore_direct_provider_plan(
        &self,
        plan: &CodexDirectProviderConfigPlan,
    ) -> Result<(), AppError> {
        self.restore(&plan.config)?;
        if let Some(catalog) = &plan.model_catalog {
            self.restore_auxiliary_plan(catalog)?;
        }
        Ok(())
    }

    /// 校验模型目录未被外部修改后原子写入目标内容。
    fn apply_auxiliary_plan(&self, plan: &CodexAuxiliaryFilePlan) -> Result<(), AppError> {
        let current = self.file_ops.read(&plan.path)?;
        ensure_fingerprint(
            &plan.previous_fingerprint,
            &fingerprint_content(current.as_deref()),
        )?;
        self.file_ops.write_atomic(&plan.path, &plan.target_content)
    }

    /// 仅在模型目录仍属于当前计划时恢复原内容。
    fn restore_auxiliary_plan(&self, plan: &CodexAuxiliaryFilePlan) -> Result<(), AppError> {
        let current = self.file_ops.read(&plan.path)?;
        ensure_fingerprint(
            &plan.target_fingerprint,
            &fingerprint_content(current.as_deref()),
        )?;
        match &plan.previous_content {
            Some(content) => self.file_ops.write_atomic(&plan.path, content),
            None => self.file_ops.remove_file(&plan.path),
        }
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
        let backup = Self::decode_route_backup(backup_json)?;
        self.restore_decoded_backup(home, backup, None)
    }

    /// 关闭 Profile 时兼容同端口的旧全局占位配置，其他外部修改仍拒绝覆盖。
    pub fn restore_profile_backup(
        &self,
        home: &Path,
        backup_json: &str,
        listen_port: u16,
    ) -> Result<(), AppError> {
        let backup = Self::decode_route_backup(backup_json)?;
        self.restore_decoded_backup(home, backup, Some(listen_port))
    }

    /// 按指纹和可选旧占位所有权证明恢复已解码备份。
    fn restore_decoded_backup(
        &self,
        home: &Path,
        backup: CodexRouteBackup,
        legacy_listen_port: Option<u16>,
    ) -> Result<(), AppError> {
        let current = self.inspect(home)?;
        if current.fingerprint == backup.previous_fingerprint {
            return Ok(());
        }
        let legacy_managed = legacy_listen_port
            .map(|listen_port| {
                Self::is_legacy_managed_home(current.content.as_deref(), listen_port)
            })
            .unwrap_or(false);
        if !legacy_managed {
            ensure_fingerprint(&backup.target_fingerprint, &current.fingerprint)?;
        }
        let config_path = codex_config_path_for_home(home);
        match backup.previous_content {
            Some(content) => self.file_ops.write_atomic(&config_path, &content),
            None => self.file_ops.remove_file(&config_path),
        }
    }

    /// 判断当前 Home 是否可由已启用 Profile 安全修复，外部编辑一律返回冲突。
    pub fn classify_profile_reconcile(
        &self,
        plan: &CodexRouteConfigPlan,
        backup_json: &str,
        listen_port: u16,
    ) -> Result<CodexHomeReconcileOwnership, AppError> {
        if plan.previous.fingerprint == plan.target_fingerprint {
            return Ok(CodexHomeReconcileOwnership::Current);
        }
        let backup = Self::decode_route_backup(backup_json)?;
        if plan.previous.fingerprint == backup.target_fingerprint {
            return Ok(CodexHomeReconcileOwnership::RouteOwned);
        }
        if Self::is_legacy_managed_home(plan.previous.content.as_deref(), listen_port) {
            return Ok(CodexHomeReconcileOwnership::LegacyManaged);
        }
        Err(AppError::CodexLiveConfigConflict {
            expected_fingerprint: backup.target_fingerprint,
            actual_fingerprint: plan.previous.fingerprint.clone(),
        })
    }

    /// 保留最初 Home 快照，只把路由备份的 target 指纹重定位到新配置。
    pub fn rebase_route_backup(
        &self,
        backup_json: &str,
        target_fingerprint: &str,
    ) -> Result<String, AppError> {
        let mut backup = Self::decode_route_backup(backup_json)?;
        backup.target_fingerprint = target_fingerprint.to_string();
        serde_json::to_string(&backup).map_err(|source| AppError::JsonSerialize { source })
    }

    /// 读取显式 Home 的当前指纹，供崩溃恢复只比较所有权而不持久化正文。
    pub fn current_fingerprint(&self, home: &Path) -> Result<String, AppError> {
        Ok(self.inspect(home)?.fingerprint)
    }

    /// 解码路由备份，统一拒绝损坏的备份元数据。
    fn decode_route_backup(backup_json: &str) -> Result<CodexRouteBackup, AppError> {
        serde_json::from_str(backup_json)
            .map_err(|error| AppError::Config(format!("Codex 路由备份无效: {error}")))
    }

    /// 仅识别 token 与 Profile 端口都匹配的旧全局占位配置。
    fn is_legacy_managed_home(content: Option<&[u8]>, listen_port: u16) -> bool {
        let Some(content) = content.and_then(|bytes| std::str::from_utf8(bytes).ok()) else {
            return false;
        };
        let expected_base_url = format!(
            "http://{}:{listen_port}/v1",
            crate::codex_profile::CODEX_ROUTE_LISTEN_HOST
        );
        crate::codex_config::extract_codex_experimental_bearer_token(content).as_deref()
            == Some(crate::codex_profile::LEGACY_PROXY_MANAGED_TOKEN)
            && crate::codex_config::extract_codex_base_url(content).as_deref()
                == Some(expected_base_url.as_str())
    }
}

/// 根据指定 Profile 的本地端口构造纯 Codex 路由配置，不读取全局代理配置。
pub fn build_codex_profile_route_toml(
    toml_str: &str,
    listen_port: u16,
    provider: Option<&Provider>,
    listener_token: &str,
) -> String {
    let updated = build_codex_route_toml_base(toml_str, listen_port, provider);
    crate::codex_config::set_codex_experimental_bearer_token(&updated, listener_token)
        .unwrap_or(updated)
}

/// 构造 Codex 本地路由共享字段，不决定旧全局接管或 Profile 的凭证策略。
pub(crate) fn build_codex_route_toml_base(
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

    /// Profile 路由配置必须用 listener token 替换旧全局占位符。
    #[test]
    fn profile_route_config_replaces_proxy_managed_with_listener_token() {
        let input = r#"model_provider = "custom"

[model_providers.custom]
name = "Custom"
base_url = "http://127.0.0.1:15721/v1"
wire_api = "responses"
experimental_bearer_token = "PROXY_MANAGED"
"#;

        let updated = build_codex_profile_route_toml(input, 15_722, None, "profile-listener-token");

        assert_eq!(
            crate::codex_config::extract_codex_experimental_bearer_token(&updated).as_deref(),
            Some("profile-listener-token")
        );
        assert!(!updated.contains("PROXY_MANAGED"));
        assert!(updated.contains("http://127.0.0.1:15722/v1"));
    }

    /// 应用 Profile token 时只能改写显式 Home 的 config.toml。
    #[test]
    fn profile_route_plan_writes_token_only_to_explicit_home() -> Result<(), AppError> {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let home_a = temp_dir.path().join("home-a");
        let home_b = temp_dir.path().join("home-b");
        fs::create_dir_all(&home_a).expect("创建 A Home");
        fs::create_dir_all(&home_b).expect("创建 B Home");
        let config = "model_provider = \"custom\"\n\n[model_providers.custom]\nname = \"Custom\"\nbase_url = \"https://example.com/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"PROXY_MANAGED\"\n";
        fs::write(codex_config_path_for_home(&home_a), config).expect("写入 A 配置");
        fs::write(codex_config_path_for_home(&home_b), config).expect("写入 B 配置");
        fs::write(codex_auth_path_for_home(&home_a), b"{\"token\":\"a\"}").expect("写入 A 认证");
        fs::write(codex_auth_path_for_home(&home_b), b"{\"token\":\"b\"}").expect("写入 B 认证");
        let config_b_before = fs::read(codex_config_path_for_home(&home_b)).expect("读取 B 配置");
        let auth_a_before = fs::read(codex_auth_path_for_home(&home_a)).expect("读取 A 认证");
        let auth_b_before = fs::read(codex_auth_path_for_home(&home_b)).expect("读取 B 认证");
        let service = CodexHomeConfigService::system();

        let plan =
            service.build_profile_route_plan(&home_a, 15_722, None, "profile-listener-token")?;
        service.apply_route_plan(&plan)?;

        let config_a =
            fs::read_to_string(codex_config_path_for_home(&home_a)).expect("读取 A 配置");
        assert_eq!(
            crate::codex_config::extract_codex_experimental_bearer_token(&config_a).as_deref(),
            Some("profile-listener-token")
        );
        assert_eq!(
            fs::read(codex_config_path_for_home(&home_b)).expect("重读 B 配置"),
            config_b_before
        );
        assert_eq!(
            fs::read(codex_auth_path_for_home(&home_a)).expect("重读 A 认证"),
            auth_a_before
        );
        assert_eq!(
            fs::read(codex_auth_path_for_home(&home_b)).expect("重读 B 认证"),
            auth_b_before
        );
        Ok(())
    }

    /// Profile 恢复只回写路由字段，必须保留 Codex Desktop 新增配置与最新模型。
    #[test]
    fn profile_restore_preserves_current_non_route_fields() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let path = codex_config_path_for_home(home.path());
        let original = r#"model_provider = "custom"
model = "before-model"

[model_providers.custom]
name = "Custom"
base_url = "https://upstream.example/v1"
wire_api = "chat"
experimental_bearer_token = "upstream-token"

[features]
hooks = true
"#;
        fs::write(&path, original).expect("写入接管前配置");
        let service = CodexHomeConfigService::system();
        let plan = service.build_profile_route_plan(
            home.path(),
            15_722,
            None,
            "profile-listener-token",
        )?;
        let backup = service.serialize_backup(&plan)?;
        service.apply_route_plan(&plan)?;

        let routed = fs::read_to_string(&path).expect("读取接管配置");
        let current = routed.replace("model = \"before-model\"", "model = \"latest-model\"")
            + r#"
[desktop]
followUpQueueMode = "queue"

[plugins."computer-use@openai-bundled"]
enabled = true

[mcp_servers.node_repl]
command = "/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node_repl"
"#;
        fs::write(&path, current).expect("模拟 Codex Desktop 写入");

        service.restore_profile_backup(home.path(), &backup, 15_722, "profile-listener-token")?;

        let restored = fs::read_to_string(&path).expect("读取恢复配置");
        let parsed: toml::Value = toml::from_str(&restored).expect("解析恢复配置");
        assert_eq!(
            parsed.get("model").and_then(|value| value.as_str()),
            Some("latest-model")
        );
        assert_eq!(
            crate::codex_config::extract_codex_base_url(&restored).as_deref(),
            Some("https://upstream.example/v1")
        );
        assert!(restored.contains("computer-use@openai-bundled"));
        assert!(restored.contains("mcp_servers.node_repl"));
        Ok(())
    }

    /// 接管前不存在路由字段时，恢复应只删除路由字段且重复调用保持字节不变。
    #[test]
    fn profile_restore_removes_absent_route_fields_and_is_idempotent() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let path = codex_config_path_for_home(home.path());
        fs::write(&path, "model = \"before\"\n\n[features]\nhooks = true\n")
            .expect("写入接管前配置");
        let service = CodexHomeConfigService::system();
        let plan = service.build_profile_route_plan(
            home.path(),
            15_722,
            None,
            "profile-listener-token",
        )?;
        let backup = service.serialize_backup(&plan)?;
        service.apply_route_plan(&plan)?;
        let current = fs::read_to_string(&path).expect("读取接管配置")
            + "\n[desktop]\nfollowUpQueueMode = \"queue\"\n";
        fs::write(&path, current).expect("模拟 Desktop 写入");

        service.restore_profile_backup(home.path(), &backup, 15_722, "profile-listener-token")?;
        let once = fs::read(&path).expect("读取第一次恢复结果");
        service.restore_profile_backup(home.path(), &backup, 15_722, "profile-listener-token")?;

        let restored = fs::read_to_string(&path).expect("读取恢复配置");
        assert_eq!(fs::read(&path).expect("读取幂等恢复结果"), once);
        assert!(restored.contains("model = \"before\""));
        assert!(restored.contains("followUpQueueMode = \"queue\""));
        assert!(!restored.contains("base_url"));
        assert!(!restored.contains("wire_api"));
        assert!(!restored.contains("experimental_bearer_token"));
        Ok(())
    }

    /// 三个严格字段任一被外部修改时都必须拒绝恢复，token 错误不得泄漏值。
    #[test]
    fn profile_restore_rejects_each_managed_field_conflict_without_token_leak(
    ) -> Result<(), AppError> {
        let cases = [
            (
                "http://127.0.0.1:15722/v1",
                "https://external.example/v1",
                false,
            ),
            ("wire_api = \"responses\"", "wire_api = \"chat\"", false),
            (
                "experimental_bearer_token = \"profile-listener-token\"",
                "experimental_bearer_token = \"external-token\"",
                true,
            ),
        ];

        for (from, to, token_case) in cases {
            let home = tempfile::tempdir().expect("创建临时 Home");
            let path = codex_config_path_for_home(home.path());
            let original = r#"model_provider = "custom"

[model_providers.custom]
name = "Custom"
base_url = "https://upstream.example/v1"
wire_api = "chat"
experimental_bearer_token = "upstream-token"
"#;
            fs::write(&path, original).expect("写入接管前配置");
            let service = CodexHomeConfigService::system();
            let plan = service.build_profile_route_plan(
                home.path(),
                15_722,
                None,
                "profile-listener-token",
            )?;
            let backup = service.serialize_backup(&plan)?;
            service.apply_route_plan(&plan)?;
            let changed = fs::read_to_string(&path)
                .expect("读取接管配置")
                .replace(from, to);
            assert!(changed.contains(to), "测试夹具必须成功修改目标字段");
            fs::write(&path, &changed).expect("写入外部修改");

            let error = service
                .restore_profile_backup(home.path(), &backup, 15_722, "profile-listener-token")
                .expect_err("外部修改必须拒绝恢复");
            let message = error.to_string();
            if token_case {
                assert!(!message.contains("profile-listener-token"));
                assert!(!message.contains("external-token"));
            }
            assert_eq!(fs::read_to_string(&path).expect("重读外部配置"), changed);
        }
        Ok(())
    }

    /// 无 model_provider 时三个路由字段都应在顶层恢复。
    #[test]
    fn profile_restore_handles_top_level_route_fields() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let path = codex_config_path_for_home(home.path());
        let original = "model = \"before\"\nbase_url = \"https://upstream.example/v1\"\nwire_api = \"chat\"\nexperimental_bearer_token = \"upstream-token\"\n";
        fs::write(&path, original).expect("写入顶层配置");
        let service = CodexHomeConfigService::system();
        let plan = service.build_profile_route_plan(
            home.path(),
            15_722,
            None,
            "profile-listener-token",
        )?;
        let backup = service.serialize_backup(&plan)?;
        service.apply_route_plan(&plan)?;

        service.restore_profile_backup(home.path(), &backup, 15_722, "profile-listener-token")?;

        assert_eq!(fs::read_to_string(path).expect("读取恢复配置"), original);
        Ok(())
    }

    /// 保留 provider 的 base/wire 与顶层 token 必须按各自路径恢复。
    #[test]
    fn profile_restore_handles_split_reserved_provider_paths() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let path = codex_config_path_for_home(home.path());
        let original = r#"model_provider = "openai"
experimental_bearer_token = "upstream-token"

[model_providers.openai]
name = "OpenAI"
base_url = "https://upstream.example/v1"
wire_api = "chat"

[model_providers.other]
base_url = "https://other.example/v1"
wire_api = "responses"
"#;
        fs::write(&path, original).expect("写入分离路径配置");
        let service = CodexHomeConfigService::system();
        let plan = service.build_profile_route_plan(
            home.path(),
            15_722,
            None,
            "profile-listener-token",
        )?;
        let backup = service.serialize_backup(&plan)?;
        service.apply_route_plan(&plan)?;

        service.restore_profile_backup(home.path(), &backup, 15_722, "profile-listener-token")?;

        assert_eq!(fs::read_to_string(path).expect("读取恢复配置"), original);
        Ok(())
    }

    /// 官方供应商直连计划只写目标 Home 配置，不得覆盖该 Home 自己的订阅认证。
    #[test]
    fn direct_official_provider_plan_preserves_profile_auth() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        let auth_path = codex_auth_path_for_home(home.path());
        fs::write(&config_path, "model = \"before\"\n").expect("写入旧配置");
        fs::write(
            &auth_path,
            b"{\"auth_mode\":\"chatgpt\",\"token\":\"home-token\"}",
        )
        .expect("写入 Profile 认证");
        let auth_before = fs::read(&auth_path).expect("读取 Profile 认证");
        let mut provider = Provider::with_id(
            "official".to_string(),
            "OpenAI Official".to_string(),
            serde_json::json!({
                "auth": {"auth_mode": "chatgpt", "token": "provider-token"},
                "config": "model = \"gpt-official\"\n"
            }),
            None,
        );
        provider.category = Some("official".to_string());
        let service = CodexHomeConfigService::system();

        let plan = service.build_direct_provider_plan(home.path(), &provider)?;
        service.apply_direct_provider_plan(&plan)?;

        assert_eq!(
            fs::read(&auth_path).expect("重读 Profile 认证"),
            auth_before
        );
        let config = fs::read_to_string(&config_path).expect("读取直连配置");
        assert!(config.contains("model = \"gpt-official\""));
        assert!(!config.contains("127.0.0.1:"));
        assert!(!config.contains("experimental_bearer_token"));
        Ok(())
    }

    /// 第三方供应商直连计划把 API key 投影到 config，同时保留 Profile 认证。
    #[test]
    fn direct_third_party_provider_plan_projects_token_into_config() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        let auth_path = codex_auth_path_for_home(home.path());
        fs::write(&config_path, "model = \"before\"\n").expect("写入旧配置");
        fs::write(&auth_path, b"{\"auth_mode\":\"chatgpt\"}").expect("写入认证");
        let auth_before = fs::read(&auth_path).expect("读取认证");
        let provider = Provider::with_id(
            "third-party".to_string(),
            "Third Party".to_string(),
            serde_json::json!({
                "auth": {"OPENAI_API_KEY": "upstream-token"},
                "config": "model_provider = \"custom\"\n\n[model_providers.custom]\nname = \"Third Party\"\nbase_url = \"https://example.com/v1\"\nwire_api = \"responses\"\n"
            }),
            None,
        );
        let service = CodexHomeConfigService::system();

        let plan = service.build_direct_provider_plan(home.path(), &provider)?;
        service.apply_direct_provider_plan(&plan)?;

        let config = fs::read_to_string(config_path).expect("读取直连配置");
        assert_eq!(
            crate::codex_config::extract_codex_experimental_bearer_token(&config).as_deref(),
            Some("upstream-token")
        );
        assert_eq!(fs::read(auth_path).expect("重读认证"), auth_before);
        Ok(())
    }

    /// 直连模型目录必须写入目标 Profile Home，不得写入其他 Home。
    #[test]
    fn direct_provider_model_catalog_is_scoped_to_profile_home() -> Result<(), AppError> {
        let target_home = tempfile::tempdir().expect("目标 Profile Home");
        let other_home = tempfile::tempdir().expect("其他 Profile Home");
        let mut provider = Provider::with_id(
            "native".to_string(),
            "Native Responses".to_string(),
            serde_json::json!({
                "auth": {"OPENAI_API_KEY": "upstream-token"},
                "config": "model_provider = \"custom\"\n[model_providers.custom]\nname = \"Native\"\nbase_url = \"https://example.com/v1\"\nwire_api = \"responses\"\n",
                "modelCatalog": {"models": [{"model": "native-model"}]}
            }),
            None,
        );
        provider.meta = Some(crate::provider::ProviderMeta {
            api_format: Some("openai_responses".to_string()),
            ..Default::default()
        });
        let service = CodexHomeConfigService::system();

        let plan = service.build_direct_provider_plan(target_home.path(), &provider)?;
        service.apply_direct_provider_plan(&plan)?;

        let catalog_path = target_home
            .path()
            .join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
        assert!(catalog_path.exists());
        assert!(!other_home
            .path()
            .join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME)
            .exists());
        let config = fs::read_to_string(codex_config_path_for_home(target_home.path()))
            .expect("读取目标 Home 配置");
        assert!(config.contains("model_catalog_json = \"cc-switch-model-catalog.json\""));
        Ok(())
    }

    /// 启动修复重定位备份时只能更新 target 指纹，不能丢失最初的 Home 快照。
    #[test]
    fn restoring_enabled_profile_home_rebases_backup_without_serializing_new_token(
    ) -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        fs::write(
            &config_path,
            "model_provider = \"custom\"\n\n[model_providers.custom]\nname = \"Custom\"\nbase_url = \"https://example.com/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"upstream-token\"\n",
        )
        .expect("写入原始配置");
        let service = CodexHomeConfigService::system();
        let old_plan =
            service.build_profile_route_plan(home.path(), 15_722, None, "old-listener-token")?;
        let old_backup = service.serialize_backup(&old_plan)?;
        service.apply_route_plan(&old_plan)?;
        let desired_plan =
            service.build_profile_route_plan(home.path(), 15_722, None, "new-listener-token")?;

        assert_eq!(
            service.classify_profile_reconcile(&desired_plan, &old_backup, 15_722)?,
            CodexHomeReconcileOwnership::RouteOwned
        );
        let rebased =
            service.rebase_route_backup(&old_backup, desired_plan.target_fingerprint())?;
        let old_json: serde_json::Value = serde_json::from_str(&old_backup).expect("解析旧备份");
        let rebased_json: serde_json::Value = serde_json::from_str(&rebased).expect("解析新备份");

        assert_eq!(
            rebased_json["previous_content"],
            old_json["previous_content"]
        );
        assert_eq!(
            rebased_json["previous_fingerprint"],
            old_json["previous_fingerprint"]
        );
        assert_eq!(
            rebased_json["target_fingerprint"],
            desired_plan.target_fingerprint()
        );
        assert!(!rebased.contains("new-listener-token"));
        Ok(())
    }

    /// 只有匹配当前 Profile 端口的旧占位配置可自愈，外部编辑必须被拒绝。
    #[test]
    fn restoring_enabled_profile_home_accepts_legacy_placeholder_and_rejects_external_edit(
    ) -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        let original = "model_provider = \"custom\"\n\n[model_providers.custom]\nname = \"Custom\"\nbase_url = \"https://example.com/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"upstream-token\"\n";
        fs::write(&config_path, original).expect("写入原始配置");
        let service = CodexHomeConfigService::system();
        let old_plan = service.build_profile_route_plan(home.path(), 15_722, None, "old-token")?;
        let backup = service.serialize_backup(&old_plan)?;
        fs::write(
            &config_path,
            "model_provider = \"custom\"\n\n[model_providers.custom]\nname = \"Custom\"\nbase_url = \"http://127.0.0.1:15722/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"PROXY_MANAGED\"\n",
        )
        .expect("写入旧占位配置");
        let legacy_plan =
            service.build_profile_route_plan(home.path(), 15_722, None, "new-token")?;
        assert_eq!(
            service.classify_profile_reconcile(&legacy_plan, &backup, 15_722)?,
            CodexHomeReconcileOwnership::LegacyManaged
        );

        fs::write(
            &config_path,
            "model_provider = \"custom\"\n\n[model_providers.custom]\nname = \"Custom\"\nbase_url = \"https://external.example/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"external-token\"\n",
        )
        .expect("写入外部编辑");
        let external_plan =
            service.build_profile_route_plan(home.path(), 15_722, None, "new-token")?;
        assert!(service
            .classify_profile_reconcile(&external_plan, &backup, 15_722)
            .is_err());
        assert!(service
            .restore_profile_backup(home.path(), &backup, 15_722)
            .is_err());
        assert_eq!(
            fs::read_to_string(&config_path).expect("重读外部配置"),
            "model_provider = \"custom\"\n\n[model_providers.custom]\nname = \"Custom\"\nbase_url = \"https://external.example/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"external-token\"\n"
        );
        Ok(())
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
