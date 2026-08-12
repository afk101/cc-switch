use crate::codex_config::{
    codex_config_path_for_home, CodexCatalogToolProfile, CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME,
};
use crate::codex_profile::{
    CODEX_MODEL_PROVIDERS_TABLE, CODEX_MODEL_PROVIDER_FIELD,
    CODEX_ROUTE_BACKUP_MIN_SUPPORTED_VERSION, CODEX_ROUTE_BACKUP_VERSION,
    CODEX_ROUTE_FIELD_BASE_URL, CODEX_ROUTE_FIELD_BEARER_TOKEN, CODEX_ROUTE_FIELD_WIRE_API,
    CODEX_ROUTE_LEGACY_BACKUP_VERSION, CODEX_ROUTE_LISTEN_HOST,
    CODEX_ROUTE_OWNERSHIP_PROOF_BACKUP_VERSION, CODEX_ROUTE_TOKEN_MISMATCH_DETAIL,
    CODEX_ROUTE_TOKEN_PROOF_DOMAIN, CODEX_ROUTE_WIRE_API_RESPONSES, LEGACY_PROXY_MANAGED_TOKEN,
};
use crate::error::AppError;
use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use toml_edit::{DocumentMut, Item};

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

impl CodexDirectProviderConfigPlan {
    /// 返回计划所属的显式 Home，不暴露文件正文。
    pub fn home_path(&self) -> &Path {
        &self.config.home_path
    }
}

/// 单个 Profile Home 的模型目录投影计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexModelCatalogProjectionPlan {
    home_path: PathBuf,
    config: Option<CodexRouteConfigPlan>,
    model_catalog: Option<CodexAuxiliaryFilePlan>,
}

impl CodexModelCatalogProjectionPlan {
    /// 返回本次投影是否需要修改 Home 配置。
    pub fn config_changed(&self) -> bool {
        self.config.is_some()
    }

    /// 返回计划所属的显式 Home，不暴露文件正文。
    pub fn home_path(&self) -> &Path {
        &self.home_path
    }
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
    /// 当前活动连接路径已不再由该 Profile 路由持有。
    ExternalTakeover,
}

/// 启动阶段读取 Home 后得到的可判定状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexHomeRouteReadiness {
    /// 配置可读取且能定位当前活动连接路径。
    Readable,
    /// 配置缺失或结构损坏，必须保护性放弃路由所有权。
    ExternalTakeover,
}

/// 持久化在 Profile 路由关系中的最小 Home 恢复信息。
#[derive(Serialize, Deserialize)]
struct CodexRouteBackup {
    #[serde(default = "legacy_route_backup_version")]
    version: u8,
    previous_content: Option<Vec<u8>>,
    previous_fingerprint: String,
    target_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ownership_proof: Option<CodexRouteOwnershipProof>,
    #[serde(default)]
    previous_token_state: CodexRouteBackupTokenState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_token_origin: Option<CodexRouteBackupTokenOrigin>,
}

/// 只读取备份版本的最小 envelope，用于副作用前区分未知未来格式。
#[derive(Deserialize)]
struct CodexRouteBackupVersionEnvelope {
    #[serde(default = "legacy_route_backup_version")]
    version: u8,
}

/// 备份中接管前 listener token 的不可逆表达。
#[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CodexRouteBackupTokenState {
    /// 旧备份或普通上游 token 仍由原正文表达。
    #[default]
    Embedded,
    /// 接管前字段等于该 Profile 的 listener token，恢复时仅从私有密钥库取值。
    ListenerTokenReference,
}

/// listener token 引用在接管前配置中的准确来源路径。
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CodexRouteBackupTokenOrigin {
    /// token 位于配置顶层。
    TopLevel,
    /// token 位于指定 provider 的标准或 inline table 中。
    Provider(String),
}

/// 已脱敏的接管前正文及其不可逆 listener token 元数据。
struct CodexRedactedPreviousToken {
    content: Option<Vec<u8>>,
    state: CodexRouteBackupTokenState,
    origin: Option<CodexRouteBackupTokenOrigin>,
}

/// 不含凭证明文的活动路由字段所有权证明。
#[derive(Serialize, Deserialize)]
struct CodexRouteOwnershipProof {
    active_provider_id: Option<String>,
    base_url: String,
    wire_api: String,
    token_digest: String,
}

/// 单个严格路由字段在 Codex TOML 中的准确位置。
#[derive(Debug, Clone, PartialEq, Eq)]
enum CodexRouteFieldPath {
    TopLevel(&'static str),
    Provider {
        provider_id: String,
        field: &'static str,
    },
}

/// 将持久化 token 来源转换为字段定位路径。
fn token_origin_path(origin: &CodexRouteBackupTokenOrigin) -> CodexRouteFieldPath {
    match origin {
        CodexRouteBackupTokenOrigin::TopLevel => {
            CodexRouteFieldPath::TopLevel(CODEX_ROUTE_FIELD_BEARER_TOKEN)
        }
        CodexRouteBackupTokenOrigin::Provider(provider_id) => CodexRouteFieldPath::Provider {
            provider_id: provider_id.clone(),
            field: CODEX_ROUTE_FIELD_BEARER_TOKEN,
        },
    }
}

/// 严格路由字段的缺失状态或原始 TOML Item。
#[derive(Clone)]
enum CodexRouteFieldState {
    Missing,
    Present(Item),
}

impl PartialEq for CodexRouteFieldState {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Missing, Self::Missing) => true,
            (Self::Present(left), Self::Present(right)) => left.to_string() == right.to_string(),
            _ => false,
        }
    }
}

impl Eq for CodexRouteFieldState {}

/// 三个严格路由字段在单个文档中的状态。
#[derive(Clone, PartialEq, Eq)]
struct CodexManagedRouteState {
    base_url: CodexRouteFieldState,
    wire_api: CodexRouteFieldState,
    bearer_token: CodexRouteFieldState,
}

/// 从接管前配置推导出的字段路径、状态和临时表信息。
#[derive(Clone)]
struct CodexManagedRouteProjection {
    base_url_path: CodexRouteFieldPath,
    wire_api_path: CodexRouteFieldPath,
    bearer_token_path: CodexRouteFieldPath,
    previous: CodexManagedRouteState,
    target: CodexManagedRouteState,
    referenced_token_origin: Option<CodexRouteFieldPath>,
    created_provider_tables: Vec<String>,
    created_model_providers_table: bool,
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

    /// 区分可重试文件 I/O 与能够确定的外部接管状态。
    pub fn classify_profile_home_readiness(
        &self,
        home: &Path,
    ) -> Result<CodexHomeRouteReadiness, AppError> {
        let snapshot = self.inspect(home)?;
        let Some(content) = snapshot.content.as_deref() else {
            return Ok(CodexHomeRouteReadiness::ExternalTakeover);
        };
        if build_route_ownership_proof(content).is_err() {
            return Ok(CodexHomeRouteReadiness::ExternalTakeover);
        }
        Ok(CodexHomeRouteReadiness::Readable)
    }

    /// 构造路由接管计划，不在此阶段写入任何文件。
    pub fn build_route_plan(
        &self,
        home: &Path,
        route_config: &str,
    ) -> Result<CodexRouteConfigPlan, AppError> {
        let previous = self.inspect(home)?;
        self.build_route_plan_from_snapshot(home, previous, route_config)
    }

    /// 使用同一次 Home 快照构造配置计划，避免准备阶段重复读取覆盖外部变更。
    fn build_route_plan_from_snapshot(
        &self,
        home: &Path,
        previous: CodexLiveConfigSnapshot,
        route_config: &str,
    ) -> Result<CodexRouteConfigPlan, AppError> {
        crate::codex_config::validate_config_toml(route_config)?;
        let target_content = route_config.as_bytes().to_vec();
        Ok(CodexRouteConfigPlan {
            home_path: home.to_path_buf(),
            target_fingerprint: fingerprint_content(Some(&target_content)),
            target_content,
            previous,
            model_changes: Vec::new(),
        })
    }

    /// 构造目标 Home 的模型目录投影计划，不在准备阶段写入文件。
    pub fn build_model_catalog_projection_plan(
        &self,
        home: &Path,
        provider: &Provider,
    ) -> Result<CodexModelCatalogProjectionPlan, AppError> {
        let mut settings = provider.settings_config.clone();
        crate::codex_config::apply_codex_unified_session_bucket_to_settings(
            provider.category.as_deref(),
            &mut settings,
        )?;
        let source_config = settings
            .get("config")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let current = self.inspect(home)?;
        let home_config = current
            .content
            .as_deref()
            .map(std::str::from_utf8)
            .transpose()
            .map_err(|error| AppError::Config(format!("Codex config.toml 不是 UTF-8: {error}")))?
            .unwrap_or("");
        let catalog_profile = crate::proxy::providers::resolve_codex_catalog_tool_profile(provider);
        let prepared = crate::codex_config::prepare_codex_model_catalog_projection(
            &settings,
            source_config,
            home_config,
            catalog_profile,
        )?;
        let config_unchanged = current.content.as_deref() == Some(prepared.config_text.as_bytes())
            || (current.content.is_none() && prepared.config_text.is_empty());
        let config = if config_unchanged {
            None
        } else {
            Some(self.build_route_plan_from_snapshot(home, current, &prepared.config_text)?)
        };
        let model_catalog = self.build_model_catalog_file_plan(home, prepared.model_catalog)?;
        Ok(CodexModelCatalogProjectionPlan {
            home_path: home.to_path_buf(),
            config,
            model_catalog,
        })
    }

    /// 应用单个 Home 的模型目录投影，失败时恢复已写入的目录文件。
    pub fn apply_model_catalog_projection_plan(
        &self,
        plan: &CodexModelCatalogProjectionPlan,
    ) -> Result<(), AppError> {
        if let Some(catalog) = &plan.model_catalog {
            self.apply_auxiliary_plan(catalog)?;
        }
        if let Some(config) = &plan.config {
            if let Err(error) = self.apply_route_plan(config) {
                let compensation_error = plan
                    .model_catalog
                    .as_ref()
                    .and_then(|catalog| self.restore_auxiliary_plan(catalog).err());
                return match compensation_error {
                    Some(compensation) => Err(AppError::Message(format!(
                        "应用 Codex 模型目录投影失败: {error}；目录补偿失败: {compensation}"
                    ))),
                    None => Err(error),
                };
            }
        }
        Ok(())
    }

    /// 反向恢复单个 Home 的模型目录投影，先恢复配置再恢复目录文件。
    pub fn restore_model_catalog_projection_plan(
        &self,
        plan: &CodexModelCatalogProjectionPlan,
    ) -> Result<(), AppError> {
        if let Some(config) = &plan.config {
            self.restore(config)?;
        }
        if let Some(catalog) = &plan.model_catalog {
            self.restore_auxiliary_plan(catalog)?;
        }
        Ok(())
    }

    /// 为可选模型目录内容构造辅助文件计划。
    fn build_model_catalog_file_plan(
        &self,
        home: &Path,
        model_catalog: Option<serde_json::Value>,
    ) -> Result<Option<CodexAuxiliaryFilePlan>, AppError> {
        model_catalog
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
            .transpose()
    }

    /// 从当前 Home 配置构造指定 Profile 端口的接管计划。
    pub fn build_profile_route_plan(
        &self,
        home: &Path,
        listen_port: u16,
        provider: Option<&Provider>,
        listener_token: &str,
    ) -> Result<CodexRouteConfigPlan, AppError> {
        self.build_profile_route_plan_with_base_transform(
            home,
            listen_port,
            provider,
            listener_token,
            |current_toml| Ok(current_toml.to_string()),
        )
    }

    /// 从同一次 Home 快照变换基础 TOML 后构造 Profile 路由计划。
    pub(crate) fn build_profile_route_plan_with_base_transform<F>(
        &self,
        home: &Path,
        listen_port: u16,
        provider: Option<&Provider>,
        listener_token: &str,
        transform: F,
    ) -> Result<CodexRouteConfigPlan, AppError>
    where
        F: FnOnce(&str) -> Result<String, AppError>,
    {
        let current = self.inspect(home)?;
        let current_toml = current
            .content
            .as_deref()
            .map(std::str::from_utf8)
            .transpose()
            .map_err(|error| AppError::Config(format!("Codex config.toml 不是 UTF-8: {error}")))?
            .unwrap_or("");
        let base_toml = transform(current_toml)?;
        self.build_route_plan_from_snapshot(
            home,
            current,
            &build_codex_profile_route_toml(&base_toml, listen_port, provider, listener_token),
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
        let model_catalog = self.build_model_catalog_file_plan(home, prepared.model_catalog)?;
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
        serialize_route_backup(
            plan.previous.content.clone(),
            plan.previous.fingerprint.clone(),
            &plan.target_content,
            &plan.target_fingerprint,
        )
    }

    /// 为显式切换构造最新恢复基线；托管 Home 复用旧严格字段，外部 Home 采用当前配置。
    pub fn serialize_explicit_switch_backup(
        &self,
        plan: &CodexRouteConfigPlan,
        backup_json: Option<&str>,
        listen_port: u16,
        listener_token: &str,
    ) -> Result<String, AppError> {
        let Some(backup_json) = backup_json else {
            let current_proof = plan
                .previous
                .content
                .as_deref()
                .and_then(|content| build_route_ownership_proof(content).ok());
            let desired_proof = build_route_ownership_proof(&plan.target_content)?;
            if current_proof
                .as_ref()
                .is_some_and(|proof| route_proof_matches(proof, &desired_proof))
            {
                return Err(AppError::Config(
                    "Codex Profile 托管路由缺少接管前恢复备份".to_string(),
                ));
            }
            return self.serialize_backup(plan);
        };
        if self.classify_profile_reconcile(plan, backup_json, listen_port)?
            == CodexHomeReconcileOwnership::ExternalTakeover
        {
            return self.serialize_backup(plan);
        }
        let previous_content = build_latest_managed_baseline(
            plan.previous.content.as_deref(),
            backup_json,
            listen_port,
            listener_token,
        )?;
        let previous_fingerprint = fingerprint_content(previous_content.as_deref());
        serialize_route_backup(
            previous_content,
            previous_fingerprint,
            &plan.target_content,
            &plan.target_fingerprint,
        )
    }

    /// 仅当 Home 仍是该 Profile 接管版本时恢复其备份配置。
    pub fn restore_backup(&self, home: &Path, backup_json: &str) -> Result<(), AppError> {
        let backup = Self::decode_route_backup(backup_json)?;
        self.restore_decoded_backup(home, backup, None)
    }

    /// 在生命周期副作用前校验备份 envelope 可由当前版本安全消费。
    pub fn validate_route_backup(&self, backup_json: &str) -> Result<(), AppError> {
        Self::decode_route_backup(backup_json).map(|_| ())
    }

    /// 判断可解析 envelope 是否来自当前程序无法消费的未来版本。
    pub fn is_future_route_backup(&self, backup_json: &str) -> bool {
        serde_json::from_str::<CodexRouteBackupVersionEnvelope>(backup_json)
            .is_ok_and(|backup| backup.version > CODEX_ROUTE_BACKUP_VERSION)
    }

    /// 启用崩溃补偿按备份语义选择旧整份恢复或私有 token 引用恢复。
    pub fn restore_enable_recovery_backup(
        &self,
        home: &Path,
        backup_json: &str,
        listen_port: u16,
        listener_token: Option<&str>,
    ) -> Result<(), AppError> {
        let backup = Self::decode_route_backup(backup_json)?;
        if backup.previous_token_state == CodexRouteBackupTokenState::ListenerTokenReference {
            let listener_token = listener_token.ok_or_else(|| {
                AppError::Config("Codex Profile 本地路由凭证缺失，无法恢复启用残留".to_string())
            })?;
            return self.restore_profile_decoded_backup(home, backup, listen_port, listener_token);
        }
        self.restore_decoded_backup(home, backup, None)
    }

    /// 关闭 Profile 时只验证并恢复三个严格路由字段，保留其他当前配置。
    pub fn restore_profile_backup(
        &self,
        home: &Path,
        backup_json: &str,
        listen_port: u16,
        listener_token: &str,
    ) -> Result<(), AppError> {
        let backup = Self::decode_route_backup(backup_json)?;
        self.restore_profile_decoded_backup(home, backup, listen_port, listener_token)
    }

    /// 对已解码 Profile 备份执行字段级幂等恢复。
    fn restore_profile_decoded_backup(
        &self,
        home: &Path,
        backup: CodexRouteBackup,
        listen_port: u16,
        listener_token: &str,
    ) -> Result<(), AppError> {
        let previous_token_origin = resolve_previous_token_origin(&backup, listener_token)?;
        let projection = build_managed_route_projection(
            backup.previous_content.as_deref(),
            backup.previous_token_state,
            previous_token_origin.as_ref(),
            listen_port,
            listener_token,
        )?;
        let current = self.inspect(home)?;
        let Some(current_content) = current.content.as_deref() else {
            return if backup.previous_content.is_none() {
                Ok(())
            } else {
                Err(AppError::Config(
                    "Codex config.toml 已在 Profile 路由恢复前被删除".to_string(),
                ))
            };
        };
        let mut current_document = parse_codex_document(current_content, "当前")?;
        let current_state = read_managed_route_state(&current_document, &projection);
        if managed_route_restore_is_complete(&current_document, &current_state, &projection) {
            return Ok(());
        }
        ensure_managed_route_owned(&current_state, &projection.target, listen_port)?;
        apply_previous_managed_route_state(&mut current_document, &projection)?;
        let merged_content = current_document.to_string().into_bytes();

        let latest = self.file_ops.read(&current.config_path)?;
        ensure_fingerprint(
            &current.fingerprint,
            &fingerprint_content(latest.as_deref()),
        )?;
        if backup.previous_content.is_none()
            && std::str::from_utf8(&merged_content)
                .map(str::trim)
                .unwrap_or_default()
                .is_empty()
        {
            self.file_ops.remove_file(&current.config_path)
        } else {
            self.file_ops
                .write_atomic(&current.config_path, &merged_content)
        }
    }

    /// 按指纹和可选旧占位所有权证明恢复已解码备份。
    fn restore_decoded_backup(
        &self,
        home: &Path,
        backup: CodexRouteBackup,
        legacy_listen_port: Option<u16>,
    ) -> Result<(), AppError> {
        if backup.previous_token_state == CodexRouteBackupTokenState::ListenerTokenReference {
            return Err(AppError::Config(
                "Codex 路由备份需要 Profile 私有凭证才能恢复".to_string(),
            ));
        }
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
        let current_content = plan.previous.content.as_deref().ok_or_else(|| {
            AppError::Config("Codex Profile 路由所有权检查缺少 config.toml".to_string())
        })?;
        let current_proof = build_route_ownership_proof(current_content)?;
        let desired_proof = build_route_ownership_proof(&plan.target_content)?;
        let backup = Self::decode_route_backup(backup_json)?;
        if Self::is_legacy_managed_home(plan.previous.content.as_deref(), listen_port) {
            return Ok(CodexHomeReconcileOwnership::LegacyManaged);
        }
        if matches!(
            backup.version,
            CODEX_ROUTE_OWNERSHIP_PROOF_BACKUP_VERSION | CODEX_ROUTE_BACKUP_VERSION
        ) && backup.ownership_proof.is_some()
        {
            let Some(proof) = backup.ownership_proof.as_ref() else {
                return Ok(CodexHomeReconcileOwnership::ExternalTakeover);
            };
            if !route_proof_public_target_matches(&current_proof, proof) {
                return Ok(CodexHomeReconcileOwnership::ExternalTakeover);
            }
            if current_proof.token_digest == desired_proof.token_digest {
                if plan.previous.fingerprint == backup.target_fingerprint
                    && plan.previous.fingerprint != plan.target_fingerprint
                {
                    return Ok(CodexHomeReconcileOwnership::RouteOwned);
                }
                return Ok(CodexHomeReconcileOwnership::Current);
            }
            if current_proof.token_digest == proof.token_digest {
                return Ok(CodexHomeReconcileOwnership::RouteOwned);
            }
            return Ok(CodexHomeReconcileOwnership::ExternalTakeover);
        }
        if backup.version < CODEX_ROUTE_BACKUP_VERSION
            && route_proof_matches(&current_proof, &desired_proof)
        {
            return Ok(CodexHomeReconcileOwnership::Current);
        }
        if backup.version < CODEX_ROUTE_BACKUP_VERSION
            && plan.previous.fingerprint == backup.target_fingerprint
        {
            return Ok(CodexHomeReconcileOwnership::RouteOwned);
        }
        Ok(CodexHomeReconcileOwnership::ExternalTakeover)
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

    /// 将备份升级到指定目标计划，并同步更新字段级所有权证明。
    pub fn rebase_route_backup_to_plan(
        &self,
        backup_json: &str,
        plan: &CodexRouteConfigPlan,
    ) -> Result<String, AppError> {
        let mut backup = Self::decode_route_backup(backup_json)?;
        backup.version = CODEX_ROUTE_BACKUP_VERSION;
        backup.target_fingerprint = plan.target_fingerprint.clone();
        let proof = build_route_ownership_proof(&plan.target_content)?;
        let listener_token = extract_active_codex_route_string(
            std::str::from_utf8(&plan.target_content).map_err(|error| {
                AppError::Config(format!("Codex Profile 路由目标不是 UTF-8: {error}"))
            })?,
            CODEX_ROUTE_FIELD_BEARER_TOKEN,
        )
        .ok_or_else(|| AppError::Config("Codex Profile 路由目标缺少本地凭证".to_string()))?;
        let resolved_previous_token_origin =
            resolve_previous_token_origin(&backup, &listener_token)?;
        let redacted =
            if backup.previous_token_state == CodexRouteBackupTokenState::ListenerTokenReference {
                CodexRedactedPreviousToken {
                    content: backup.previous_content,
                    state: CodexRouteBackupTokenState::ListenerTokenReference,
                    origin: resolved_previous_token_origin,
                }
            } else {
                redact_previous_listener_token(backup.previous_content, &listener_token)?
            };
        backup.previous_content = redacted.content;
        backup.previous_token_state = redacted.state;
        backup.previous_token_origin = redacted.origin;
        backup.ownership_proof = Some(proof);
        serde_json::to_string(&backup).map_err(|source| AppError::JsonSerialize { source })
    }

    /// 读取显式 Home 的当前指纹，供崩溃恢复只比较所有权而不持久化正文。
    pub fn current_fingerprint(&self, home: &Path) -> Result<String, AppError> {
        Ok(self.inspect(home)?.fingerprint)
    }

    /// 解码路由备份，统一拒绝损坏的备份元数据。
    fn decode_route_backup(backup_json: &str) -> Result<CodexRouteBackup, AppError> {
        let backup: CodexRouteBackup = serde_json::from_str(backup_json)
            .map_err(|error| AppError::Config(format!("Codex 路由备份无效: {error}")))?;
        if !(CODEX_ROUTE_BACKUP_MIN_SUPPORTED_VERSION..=CODEX_ROUTE_BACKUP_VERSION)
            .contains(&backup.version)
        {
            return Err(AppError::Config(format!(
                "Codex 路由备份版本 {} 不受支持",
                backup.version
            )));
        }
        Ok(backup)
    }

    /// 仅识别 token 与 Profile 端口都匹配的旧全局占位配置。
    fn is_legacy_managed_home(content: Option<&[u8]>, listen_port: u16) -> bool {
        let Some(content) = content.and_then(|bytes| std::str::from_utf8(bytes).ok()) else {
            return false;
        };
        let expected_base_url = format!("http://{}:{listen_port}/v1", CODEX_ROUTE_LISTEN_HOST);
        crate::codex_config::extract_codex_experimental_bearer_token(content).as_deref()
            == Some(LEGACY_PROXY_MANAGED_TOKEN)
            && crate::codex_config::extract_codex_base_url(content).as_deref()
                == Some(expected_base_url.as_str())
            && extract_active_codex_route_string(content, CODEX_ROUTE_FIELD_WIRE_API).as_deref()
                == Some(CODEX_ROUTE_WIRE_API_RESPONSES)
    }
}

/// 将指定字节解析为保留格式的 Codex TOML 文档。
fn parse_codex_document(content: &[u8], context: &str) -> Result<DocumentMut, AppError> {
    let text = std::str::from_utf8(content).map_err(|error| {
        AppError::Config(format!("{context} Codex config.toml 不是 UTF-8: {error}"))
    })?;
    text.parse::<DocumentMut>()
        .map_err(|error| AppError::Config(format!("{context} Codex config.toml 无效: {error}")))
}

/// 为无版本字段的旧备份提供兼容版本号。
fn legacy_route_backup_version() -> u8 {
    CODEX_ROUTE_LEGACY_BACKUP_VERSION
}

/// 对 listener token 计算域分离摘要，避免与普通 SHA-256 摘要混用。
fn route_token_digest(listener_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(CODEX_ROUTE_TOKEN_PROOF_DOMAIN);
    hasher.update([0]);
    hasher.update(listener_token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// 从目标配置提取当前活动连接路径并生成脱敏所有权证明。
fn build_route_ownership_proof(
    target_content: &[u8],
) -> Result<CodexRouteOwnershipProof, AppError> {
    let target = parse_codex_document(target_content, "路由目标")?;
    let target_text = target.to_string();
    let base_url = extract_active_codex_route_string(&target_text, CODEX_ROUTE_FIELD_BASE_URL)
        .ok_or_else(|| AppError::Config("Codex Profile 路由目标缺少 base_url".to_string()))?;
    let wire_api = extract_active_codex_route_string(&target_text, CODEX_ROUTE_FIELD_WIRE_API)
        .ok_or_else(|| AppError::Config("Codex Profile 路由目标缺少 wire_api".to_string()))?;
    let listener_token = crate::codex_config::extract_codex_experimental_bearer_token(&target_text)
        .ok_or_else(|| AppError::Config("Codex Profile 路由目标缺少本地凭证".to_string()))?;
    Ok(CodexRouteOwnershipProof {
        active_provider_id: active_codex_provider_id(&target),
        base_url,
        wire_api,
        token_digest: route_token_digest(&listener_token),
    })
}

/// 判断两个证明是否指向同一个活动 provider 与公开路由目标。
fn route_proof_public_target_matches(
    actual: &CodexRouteOwnershipProof,
    expected: &CodexRouteOwnershipProof,
) -> bool {
    actual.active_provider_id == expected.active_provider_id
        && actual.base_url == expected.base_url
        && actual.wire_api == expected.wire_api
}

/// 判断活动连接路径及 listener token 是否完整一致。
fn route_proof_matches(
    actual: &CodexRouteOwnershipProof,
    expected: &CodexRouteOwnershipProof,
) -> bool {
    route_proof_public_target_matches(actual, expected)
        && actual.token_digest == expected.token_digest
}

/// 返回文档中当前活动 provider 标识。
fn active_codex_provider_id(document: &DocumentMut) -> Option<String> {
    document
        .get(CODEX_MODEL_PROVIDER_FIELD)
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|provider_id| !provider_id.is_empty())
        .map(str::to_string)
}

/// 返回当前实际生效且精确匹配 listener token 的来源路径。
fn active_listener_token_origin(
    document: &DocumentMut,
    listener_token: &str,
) -> Option<CodexRouteBackupTokenOrigin> {
    if let Some(provider_id) = active_codex_provider_id(document) {
        let provider_path = CodexRouteFieldPath::Provider {
            provider_id: provider_id.clone(),
            field: CODEX_ROUTE_FIELD_BEARER_TOKEN,
        };
        if route_field_item(document, &provider_path).and_then(Item::as_str) == Some(listener_token)
        {
            return Some(CodexRouteBackupTokenOrigin::Provider(provider_id));
        }
    }
    (document
        .get(CODEX_ROUTE_FIELD_BEARER_TOKEN)
        .and_then(Item::as_str)
        == Some(listener_token))
    .then_some(CodexRouteBackupTokenOrigin::TopLevel)
}

/// 用不可逆的旧正文指纹证明 token 来源，兼容没有路径字段的历史 v3。
fn resolve_previous_token_origin(
    backup: &CodexRouteBackup,
    listener_token: &str,
) -> Result<Option<CodexRouteBackupTokenOrigin>, AppError> {
    if backup.previous_token_state != CodexRouteBackupTokenState::ListenerTokenReference {
        return Ok(None);
    }
    let content = backup.previous_content.as_deref().ok_or_else(|| {
        AppError::Config("Codex 路由备份缺少可证明 token 来源的接管前配置".to_string())
    })?;
    if let Some(origin) = backup.previous_token_origin.as_ref() {
        if token_origin_can_be_active(content, origin, listener_token)? {
            return Ok(Some(origin.clone()));
        }
        return Err(AppError::Config(
            "Codex 路由备份的 token 来源证明不匹配".to_string(),
        ));
    }

    let document = parse_codex_document(content, "接管前")?;
    let mut candidates = vec![CodexRouteBackupTokenOrigin::TopLevel];
    if let Some(provider_id) = active_codex_provider_id(&document) {
        candidates.push(CodexRouteBackupTokenOrigin::Provider(provider_id));
    }
    let mut proven = candidates.into_iter().filter_map(|origin| {
        token_origin_matches_previous_fingerprint(
            content,
            &backup.previous_fingerprint,
            &origin,
            listener_token,
        )
        .ok()
        .filter(|matches| *matches)
        .map(|_| origin)
    });
    let origin = proven.next();
    if origin.is_none() || proven.next().is_some() {
        return Err(AppError::Config(
            "旧 Codex v3 路由备份的 token 来源无法唯一证明".to_string(),
        ));
    }
    Ok(origin)
}

/// 验证显式来源能在接管前结构中成为实际生效路径。
fn token_origin_can_be_active(
    redacted_content: &[u8],
    origin: &CodexRouteBackupTokenOrigin,
    listener_token: &str,
) -> Result<bool, AppError> {
    let mut document = parse_codex_document(redacted_content, "接管前")?;
    apply_route_field_state(
        &mut document,
        &token_origin_path(origin),
        &CodexRouteFieldState::Present(toml_edit::value(listener_token)),
    )?;
    Ok(active_listener_token_origin(&document, listener_token).as_ref() == Some(origin))
}

/// 通过重建候选字段并比较接管前指纹，证明来源路径而不持久化 token。
fn token_origin_matches_previous_fingerprint(
    redacted_content: &[u8],
    previous_fingerprint: &str,
    origin: &CodexRouteBackupTokenOrigin,
    listener_token: &str,
) -> Result<bool, AppError> {
    let mut document = parse_codex_document(redacted_content, "接管前")?;
    let path = token_origin_path(origin);
    apply_route_field_state(
        &mut document,
        &path,
        &CodexRouteFieldState::Present(toml_edit::value(listener_token)),
    )?;
    if active_listener_token_origin(&document, listener_token).as_ref() != Some(origin) {
        return Ok(false);
    }
    let content = document.to_string().into_bytes();
    Ok(fingerprint_content(Some(&content)) == previous_fingerprint)
}

/// 从准确路径读取字段 Item。
fn route_field_item<'a>(document: &'a DocumentMut, path: &CodexRouteFieldPath) -> Option<&'a Item> {
    match path {
        CodexRouteFieldPath::TopLevel(field) => document.get(field),
        CodexRouteFieldPath::Provider { provider_id, field } => document
            .get(CODEX_MODEL_PROVIDERS_TABLE)
            .and_then(Item::as_table_like)
            .and_then(|providers| providers.get(provider_id))
            .and_then(Item::as_table_like)
            .and_then(|provider| provider.get(field)),
    }
}

/// 在 target 中定位由现有构造器写入的字符串字段。
fn locate_route_field_path(
    target: &DocumentMut,
    field: &'static str,
    expected: &str,
) -> Result<CodexRouteFieldPath, AppError> {
    if let Some(provider_id) = active_codex_provider_id(target) {
        let path = CodexRouteFieldPath::Provider { provider_id, field };
        if route_field_item(target, &path).and_then(Item::as_str) == Some(expected) {
            return Ok(path);
        }
    }

    let path = CodexRouteFieldPath::TopLevel(field);
    if route_field_item(target, &path).and_then(Item::as_str) == Some(expected) {
        return Ok(path);
    }
    Err(AppError::Config(format!(
        "无法定位 Codex Profile 路由字段 {field}"
    )))
}

/// 从指定路径读取字段的缺失状态或原始 Item。
fn read_route_field_state(
    document: &DocumentMut,
    path: &CodexRouteFieldPath,
) -> CodexRouteFieldState {
    route_field_item(document, path)
        .cloned()
        .map(CodexRouteFieldState::Present)
        .unwrap_or(CodexRouteFieldState::Missing)
}

/// 按 projection 中三个独立路径读取严格路由状态。
fn read_managed_route_state(
    document: &DocumentMut,
    projection: &CodexManagedRouteProjection,
) -> CodexManagedRouteState {
    CodexManagedRouteState {
        base_url: read_route_field_state(document, &projection.base_url_path),
        wire_api: read_route_field_state(document, &projection.wire_api_path),
        bearer_token: read_route_field_state(document, &projection.bearer_token_path),
    }
}

/// 判断严格字段与不可逆 token 来源是否已经完整恢复。
fn managed_route_restore_is_complete(
    document: &DocumentMut,
    current: &CodexManagedRouteState,
    projection: &CodexManagedRouteProjection,
) -> bool {
    if current != &projection.previous {
        return false;
    }
    let Some(origin) = projection
        .referenced_token_origin
        .as_ref()
        .filter(|origin| *origin != &projection.bearer_token_path)
    else {
        return true;
    };
    read_route_field_state(document, origin) == projection.target.bearer_token
}

/// 记录 target 相比 previous 新建的 provider 表。
fn created_provider_tables(previous: &DocumentMut, target: &DocumentMut) -> Vec<String> {
    let previous_providers = previous
        .get(CODEX_MODEL_PROVIDERS_TABLE)
        .and_then(Item::as_table_like);
    target
        .get(CODEX_MODEL_PROVIDERS_TABLE)
        .and_then(Item::as_table_like)
        .map(|providers| {
            providers
                .iter()
                .filter(|(provider_id, item)| {
                    item.as_table_like().is_some()
                        && previous_providers
                            .map(|previous| !previous.contains_key(provider_id))
                            .unwrap_or(true)
                })
                .map(|(provider_id, _)| provider_id.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// 从接管前正文纯构造 target，并投影三个严格字段。
fn build_managed_route_projection(
    previous_content: Option<&[u8]>,
    previous_token_state: CodexRouteBackupTokenState,
    previous_token_origin: Option<&CodexRouteBackupTokenOrigin>,
    listen_port: u16,
    listener_token: &str,
) -> Result<CodexManagedRouteProjection, AppError> {
    let previous_document = match previous_content {
        Some(content) => parse_codex_document(content, "接管前")?,
        None => DocumentMut::new(),
    };
    let previous_text = previous_document.to_string();
    let target_text =
        build_codex_profile_route_toml(&previous_text, listen_port, None, listener_token);
    let target_document = parse_codex_document(target_text.as_bytes(), "路由目标")?;
    let expected_base_url = format!("http://{CODEX_ROUTE_LISTEN_HOST}:{listen_port}/v1");
    let base_url_path = locate_route_field_path(
        &target_document,
        CODEX_ROUTE_FIELD_BASE_URL,
        &expected_base_url,
    )?;
    let wire_api_path = locate_route_field_path(
        &target_document,
        CODEX_ROUTE_FIELD_WIRE_API,
        CODEX_ROUTE_WIRE_API_RESPONSES,
    )?;
    let bearer_token_path = locate_route_field_path(
        &target_document,
        CODEX_ROUTE_FIELD_BEARER_TOKEN,
        listener_token,
    )?;
    let created_provider_tables = created_provider_tables(&previous_document, &target_document);
    let created_model_providers_table =
        previous_document.get(CODEX_MODEL_PROVIDERS_TABLE).is_none()
            && target_document
                .get(CODEX_MODEL_PROVIDERS_TABLE)
                .and_then(Item::as_table_like)
                .is_some();
    let mut previous = CodexManagedRouteState {
        base_url: read_route_field_state(&previous_document, &base_url_path),
        wire_api: read_route_field_state(&previous_document, &wire_api_path),
        bearer_token: read_route_field_state(&previous_document, &bearer_token_path),
    };
    if previous_token_state == CodexRouteBackupTokenState::ListenerTokenReference
        && previous_token_origin.map(token_origin_path).as_ref() == Some(&bearer_token_path)
    {
        previous.bearer_token = CodexRouteFieldState::Present(toml_edit::value(listener_token));
    }
    let target = CodexManagedRouteState {
        base_url: read_route_field_state(&target_document, &base_url_path),
        wire_api: read_route_field_state(&target_document, &wire_api_path),
        bearer_token: read_route_field_state(&target_document, &bearer_token_path),
    };

    Ok(CodexManagedRouteProjection {
        base_url_path,
        wire_api_path,
        bearer_token_path,
        previous,
        target,
        referenced_token_origin: previous_token_origin.map(token_origin_path),
        created_provider_tables,
        created_model_providers_table,
    })
}

/// 返回严格字段的字符串值，非字符串与缺失均不转换。
fn route_field_state_string(state: &CodexRouteFieldState) -> Option<&str> {
    match state {
        CodexRouteFieldState::Missing => None,
        CodexRouteFieldState::Present(item) => item.as_str(),
    }
}

/// 为可公开的字段生成冲突详情。
fn describe_public_route_field_state(state: &CodexRouteFieldState) -> String {
    match state {
        CodexRouteFieldState::Missing => "缺失".to_string(),
        CodexRouteFieldState::Present(item) => item
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| "非字符串值".to_string()),
    }
}

/// 校验单个可公开字段仍等于路由目标。
fn ensure_public_route_field_owned(
    field: &str,
    current: &CodexRouteFieldState,
    expected: &str,
) -> Result<(), AppError> {
    if route_field_state_string(current) == Some(expected) {
        return Ok(());
    }
    Err(AppError::Config(format!(
        "Codex Profile 路由字段 {field} 冲突，期望 {expected}，实际 {}",
        describe_public_route_field_state(current)
    )))
}

/// 校验 current 三字段仍由指定 Profile 路由持有。
fn ensure_managed_route_owned(
    current: &CodexManagedRouteState,
    target: &CodexManagedRouteState,
    listen_port: u16,
) -> Result<(), AppError> {
    let expected_base_url = format!("http://{CODEX_ROUTE_LISTEN_HOST}:{listen_port}/v1");
    if route_field_state_string(&target.base_url) != Some(expected_base_url.as_str())
        || route_field_state_string(&target.wire_api) != Some(CODEX_ROUTE_WIRE_API_RESPONSES)
        || route_field_state_string(&target.bearer_token).is_none()
    {
        return Err(AppError::Config(
            "Codex Profile 路由目标缺少严格字段".to_string(),
        ));
    }
    ensure_public_route_field_owned(
        CODEX_ROUTE_FIELD_BASE_URL,
        &current.base_url,
        &expected_base_url,
    )?;
    ensure_public_route_field_owned(
        CODEX_ROUTE_FIELD_WIRE_API,
        &current.wire_api,
        CODEX_ROUTE_WIRE_API_RESPONSES,
    )?;

    let expected_token = route_field_state_string(&target.bearer_token)
        .ok_or_else(|| AppError::Config("Codex Profile 路由目标缺少本地凭证".to_string()))?;
    let current_token = route_field_state_string(&current.bearer_token);
    if current_token == Some(expected_token) || current_token == Some(LEGACY_PROXY_MANAGED_TOKEN) {
        return Ok(());
    }
    Err(AppError::Config(format!(
        "Codex Profile 路由字段 {CODEX_ROUTE_FIELD_BEARER_TOKEN} 冲突：{CODEX_ROUTE_TOKEN_MISMATCH_DETAIL}"
    )))
}

/// 将单个严格字段设置为接管前 Item 或删除。
fn apply_route_field_state(
    document: &mut DocumentMut,
    path: &CodexRouteFieldPath,
    previous: &CodexRouteFieldState,
) -> Result<(), AppError> {
    match path {
        CodexRouteFieldPath::TopLevel(field) => match previous {
            CodexRouteFieldState::Missing => {
                document.as_table_mut().remove(field);
            }
            CodexRouteFieldState::Present(item) => {
                document.as_table_mut().insert(field, item.clone());
            }
        },
        CodexRouteFieldPath::Provider { provider_id, field } => {
            let provider = document
                .get_mut(CODEX_MODEL_PROVIDERS_TABLE)
                .and_then(Item::as_table_like_mut)
                .and_then(|providers| providers.get_mut(provider_id))
                .and_then(Item::as_table_like_mut)
                .ok_or_else(|| {
                    AppError::Config(format!(
                        "Codex Profile 路由字段 {field} 的 provider 表不存在"
                    ))
                })?;
            match previous {
                CodexRouteFieldState::Missing => {
                    provider.remove(field);
                }
                CodexRouteFieldState::Present(item) => {
                    provider.insert(field, item.clone());
                }
            }
        }
    }
    Ok(())
}

/// 只清理由本次路由构造新建且恢复后仍为空的表。
fn cleanup_created_provider_tables(
    document: &mut DocumentMut,
    projection: &CodexManagedRouteProjection,
) {
    if let Some(providers) = document
        .get_mut(CODEX_MODEL_PROVIDERS_TABLE)
        .and_then(Item::as_table_like_mut)
    {
        for provider_id in &projection.created_provider_tables {
            let should_remove = providers
                .get(provider_id)
                .and_then(Item::as_table_like)
                .map(toml_edit::TableLike::is_empty)
                .unwrap_or(false);
            if should_remove {
                providers.remove(provider_id);
            }
        }
    }
    let should_remove_parent = projection.created_model_providers_table
        && document
            .get(CODEX_MODEL_PROVIDERS_TABLE)
            .and_then(Item::as_table_like)
            .map(toml_edit::TableLike::is_empty)
            .unwrap_or(false);
    if should_remove_parent {
        document.as_table_mut().remove(CODEX_MODEL_PROVIDERS_TABLE);
    }
}

/// 将三个严格路由字段恢复为接管前状态。
fn apply_previous_managed_route_state(
    current: &mut DocumentMut,
    projection: &CodexManagedRouteProjection,
) -> Result<(), AppError> {
    apply_route_field_state(
        current,
        &projection.base_url_path,
        &projection.previous.base_url,
    )?;
    apply_route_field_state(
        current,
        &projection.wire_api_path,
        &projection.previous.wire_api,
    )?;
    apply_route_field_state(
        current,
        &projection.bearer_token_path,
        &projection.previous.bearer_token,
    )?;
    if let Some(origin) = projection
        .referenced_token_origin
        .as_ref()
        .filter(|origin| *origin != &projection.bearer_token_path)
    {
        apply_route_field_state(current, origin, &projection.target.bearer_token)?;
    }
    cleanup_created_provider_tables(current, projection);
    Ok(())
}

/// 将旧接管前严格字段合并到当前 Home 的非路由字段，形成显式切换的新恢复基线。
fn build_latest_managed_baseline(
    current_content: Option<&[u8]>,
    backup_json: &str,
    listen_port: u16,
    listener_token: &str,
) -> Result<Option<Vec<u8>>, AppError> {
    let backup = CodexHomeConfigService::decode_route_backup(backup_json)?;
    let previous_token_origin = resolve_previous_token_origin(&backup, listener_token)?;
    let projection = build_managed_route_projection(
        backup.previous_content.as_deref(),
        backup.previous_token_state,
        previous_token_origin.as_ref(),
        listen_port,
        listener_token,
    )?;
    let current_content = current_content.ok_or_else(|| {
        AppError::Config("Codex Profile 托管路由缺少当前 config.toml".to_string())
    })?;
    let mut current_document = parse_codex_document(current_content, "当前")?;
    let current_state = read_managed_route_state(&current_document, &projection);
    ensure_managed_route_owned(&current_state, &projection.target, listen_port)?;
    apply_previous_managed_route_state(&mut current_document, &projection)?;
    let merged_content = current_document.to_string().into_bytes();
    if backup.previous_content.is_none()
        && std::str::from_utf8(&merged_content)
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
    {
        Ok(None)
    } else {
        Ok(Some(merged_content))
    }
}

/// 将恢复基线和路由目标编码为不含 listener token 明文的版本化备份。
fn serialize_route_backup(
    previous_content: Option<Vec<u8>>,
    previous_fingerprint: String,
    target_content: &[u8],
    target_fingerprint: &str,
) -> Result<String, AppError> {
    let ownership_proof = build_route_ownership_proof(target_content)?;
    let target_text = std::str::from_utf8(target_content)
        .map_err(|error| AppError::Config(format!("Codex Profile 路由目标不是 UTF-8: {error}")))?;
    let listener_token =
        extract_active_codex_route_string(target_text, CODEX_ROUTE_FIELD_BEARER_TOKEN)
            .ok_or_else(|| AppError::Config("Codex Profile 路由目标缺少本地凭证".to_string()))?;
    let redacted = redact_previous_listener_token(previous_content, &listener_token)?;
    serde_json::to_string(&CodexRouteBackup {
        version: CODEX_ROUTE_BACKUP_VERSION,
        previous_content: redacted.content,
        previous_fingerprint,
        target_fingerprint: target_fingerprint.to_string(),
        ownership_proof: Some(ownership_proof),
        previous_token_state: redacted.state,
        previous_token_origin: redacted.origin,
    })
    .map_err(|source| AppError::JsonSerialize { source })
}

/// 当接管前存在 Profile listener token 时，删除全部可逆字段并保留引用语义。
fn redact_previous_listener_token(
    previous_content: Option<Vec<u8>>,
    listener_token: &str,
) -> Result<CodexRedactedPreviousToken, AppError> {
    let Some(content) = previous_content else {
        return Ok(CodexRedactedPreviousToken {
            content: None,
            state: CodexRouteBackupTokenState::Embedded,
            origin: None,
        });
    };
    let mut document = parse_codex_document(&content, "接管前")?;
    let active_token_origin = active_listener_token_origin(&document, listener_token);
    let matching_paths = listener_token_paths(&document, listener_token);
    if matching_paths.is_empty() {
        return Ok(CodexRedactedPreviousToken {
            content: Some(content),
            state: CodexRouteBackupTokenState::Embedded,
            origin: None,
        });
    }
    let Some(active_token_origin) = active_token_origin else {
        return Err(AppError::Config(
            "Codex 路由备份包含无法无损表达的非活动 listener token".to_string(),
        ));
    };
    for path in matching_paths {
        apply_route_field_state(&mut document, &path, &CodexRouteFieldState::Missing)?;
    }
    Ok(CodexRedactedPreviousToken {
        content: Some(document.to_string().into_bytes()),
        state: CodexRouteBackupTokenState::ListenerTokenReference,
        origin: Some(active_token_origin),
    })
}

/// 找出配置中全部精确匹配 listener token 的可持久化路径。
fn listener_token_paths(document: &DocumentMut, listener_token: &str) -> Vec<CodexRouteFieldPath> {
    let mut paths = Vec::new();
    if document
        .get(CODEX_ROUTE_FIELD_BEARER_TOKEN)
        .and_then(Item::as_str)
        == Some(listener_token)
    {
        paths.push(CodexRouteFieldPath::TopLevel(
            CODEX_ROUTE_FIELD_BEARER_TOKEN,
        ));
    }
    if let Some(providers) = document
        .get(CODEX_MODEL_PROVIDERS_TABLE)
        .and_then(Item::as_table_like)
    {
        paths.extend(
            providers
                .iter()
                .filter(|(_, item)| {
                    item.as_table_like()
                        .and_then(|provider| provider.get(CODEX_ROUTE_FIELD_BEARER_TOKEN))
                        .and_then(Item::as_str)
                        == Some(listener_token)
                })
                .map(|(provider_id, _)| CodexRouteFieldPath::Provider {
                    provider_id: provider_id.to_string(),
                    field: CODEX_ROUTE_FIELD_BEARER_TOKEN,
                }),
        );
    }
    paths
}

/// 读取活动 provider 中的字符串字段，缺失时回退顶层。
fn extract_active_codex_route_string(content: &str, field: &str) -> Option<String> {
    let document = content.parse::<DocumentMut>().ok()?;
    if let Some(provider_id) = active_codex_provider_id(&document) {
        if let Some(value) = document
            .get(CODEX_MODEL_PROVIDERS_TABLE)
            .and_then(Item::as_table_like)
            .and_then(|providers| providers.get(&provider_id))
            .and_then(Item::as_table_like)
            .and_then(|provider| provider.get(field))
            .and_then(Item::as_str)
        {
            return Some(value.to_string());
        }
    }
    document
        .get(field)
        .and_then(Item::as_str)
        .map(str::to_string)
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
    let proxy_url = format!("http://{CODEX_ROUTE_LISTEN_HOST}:{listen_port}/v1");
    let updated = crate::codex_config::update_codex_toml_field(
        toml_str,
        CODEX_ROUTE_FIELD_BASE_URL,
        &proxy_url,
    )
    .unwrap_or_else(|_| toml_str.to_string());
    let mut updated = crate::codex_config::update_codex_toml_field(
        &updated,
        CODEX_ROUTE_FIELD_WIRE_API,
        CODEX_ROUTE_WIRE_API_RESPONSES,
    )
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
    use std::collections::VecDeque;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

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

    /// 在两次读取之间返回不同内容，用于模拟恢复窗口中的并发写入。
    struct ChangingReadFileOps {
        reads: Mutex<VecDeque<Option<Vec<u8>>>>,
        writes: AtomicUsize,
    }

    impl ChangingReadFileOps {
        /// 按给定顺序构造读取结果。
        fn new(reads: Vec<Option<Vec<u8>>>) -> Self {
            Self {
                reads: Mutex::new(reads.into()),
                writes: AtomicUsize::new(0),
            }
        }
    }

    impl CodexHomeFileOps for ChangingReadFileOps {
        fn read(&self, _path: &Path) -> Result<Option<Vec<u8>>, AppError> {
            self.reads
                .lock()
                .map_err(|error| AppError::Lock(error.to_string()))?
                .pop_front()
                .ok_or_else(|| AppError::Message("测试读取序列已耗尽".to_string()))
        }

        fn write_atomic(&self, _path: &Path, _content: &[u8]) -> Result<(), AppError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn remove_file(&self, _path: &Path) -> Result<(), AppError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// 构造包含共享模型目录的测试供应商。
    fn provider_with_catalog(id: &str, model: &str) -> Provider {
        let mut provider = Provider::with_id(
            id.to_string(),
            id.to_string(),
            serde_json::json!({
                "auth": {"OPENAI_API_KEY": "test-token"},
                "config": "model_provider = \"custom\"\n[model_providers.custom]\nbase_url = \"https://example.com/v1\"\nwire_api = \"responses\"\n",
                "modelCatalog": {"models": [{"model": model}]}
            }),
            None,
        );
        provider.meta = Some(crate::provider::ProviderMeta {
            api_format: Some("openai_responses".to_string()),
            ..Default::default()
        });
        provider
    }

    /// 构造不包含模型目录的测试供应商。
    fn provider_without_catalog(id: &str) -> Provider {
        Provider::with_id(
            id.to_string(),
            id.to_string(),
            serde_json::json!({"config": "model = \"gpt-official\"\n"}),
            None,
        )
    }

    /// 已有 CC Switch 指针时只需更新模型目录文件，不应重写 Home 配置。
    #[test]
    fn existing_pointer_updates_only_catalog_file() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        let original_config =
            "model_catalog_json = \"cc-switch-model-catalog.json\"\nkeep = true\n";
        fs::write(&config_path, original_config).expect("写入已有目录指针");
        let service = CodexHomeConfigService::system();
        let provider = provider_with_catalog("shared", "new-model");

        let plan = service.build_model_catalog_projection_plan(home.path(), &provider)?;

        assert!(!plan.config_changed());
        service.apply_model_catalog_projection_plan(&plan)?;
        assert_eq!(
            fs::read_to_string(&config_path).expect("重读 Home 配置"),
            original_config
        );
        assert!(
            fs::read_to_string(home.path().join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME))
                .expect("读取更新后的模型目录")
                .contains("new-model")
        );
        Ok(())
    }

    /// 无模型目录供应商只移除 CC Switch 指针，保留旧目录文件供后续复用。
    #[test]
    fn provider_without_catalog_keeps_old_catalog_file() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let catalog_path = home.path().join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
        fs::write(&catalog_path, b"old catalog").expect("写入旧模型目录");
        fs::write(
            codex_config_path_for_home(home.path()),
            "model_catalog_json = \"cc-switch-model-catalog.json\"\nkeep = true\n",
        )
        .expect("写入旧目录指针");
        let service = CodexHomeConfigService::system();

        let plan = service.build_model_catalog_projection_plan(
            home.path(),
            &provider_without_catalog("official"),
        )?;
        service.apply_model_catalog_projection_plan(&plan)?;

        assert_eq!(
            fs::read(&catalog_path).expect("读取旧模型目录"),
            b"old catalog"
        );
        let config = fs::read_to_string(codex_config_path_for_home(home.path()))
            .expect("读取移除指针后的配置");
        assert!(!config.contains("model_catalog_json"));
        assert!(config.contains("keep = true"));
        Ok(())
    }

    /// 计划构建后的外部配置修改必须被指纹校验拒绝。
    #[test]
    fn model_catalog_projection_plan_rejects_external_config_change() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        fs::write(&config_path, "keep = true\n").expect("写入初始配置");
        let service = CodexHomeConfigService::system();
        let plan = service.build_model_catalog_projection_plan(
            home.path(),
            &provider_with_catalog("shared", "new-model"),
        )?;
        fs::write(&config_path, "keep = false\n").expect("模拟外部修改");

        let error = service
            .apply_model_catalog_projection_plan(&plan)
            .expect_err("外部修改必须拒绝覆盖");

        assert!(matches!(error, AppError::CodexLiveConfigConflict { .. }));
        assert_eq!(
            fs::read_to_string(&config_path).expect("重读外部配置"),
            "keep = false\n"
        );
        assert!(!home
            .path()
            .join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME)
            .exists());
        Ok(())
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

    /// 接管前和当前文件都不存在时可幂等成功，只有当前被删除时必须拒绝恢复。
    #[test]
    fn profile_restore_distinguishes_absent_original_from_external_deletion() -> Result<(), AppError>
    {
        let absent_home = tempfile::tempdir().expect("创建空 Home");
        let service = CodexHomeConfigService::system();
        let absent_plan = service.build_profile_route_plan(
            absent_home.path(),
            15_722,
            None,
            "profile-listener-token",
        )?;
        let absent_backup = service.serialize_backup(&absent_plan)?;
        service.restore_profile_backup(
            absent_home.path(),
            &absent_backup,
            15_722,
            "profile-listener-token",
        )?;
        assert!(!codex_config_path_for_home(absent_home.path()).exists());

        let deleted_home = tempfile::tempdir().expect("创建有配置的 Home");
        let deleted_path = codex_config_path_for_home(deleted_home.path());
        fs::write(&deleted_path, "model = \"before\"\n").expect("写入接管前配置");
        let deleted_plan = service.build_profile_route_plan(
            deleted_home.path(),
            15_722,
            None,
            "profile-listener-token",
        )?;
        let deleted_backup = service.serialize_backup(&deleted_plan)?;
        fs::remove_file(&deleted_path).expect("模拟外部删除配置");

        assert!(service
            .restore_profile_backup(
                deleted_home.path(),
                &deleted_backup,
                15_722,
                "profile-listener-token",
            )
            .is_err());
        assert!(!deleted_path.exists());
        Ok(())
    }

    /// 非字符串严格字段不能被当作缺失，接管前 Item 应按原类型恢复。
    #[test]
    fn profile_restore_preserves_item_types_and_rejects_non_string_current_values(
    ) -> Result<(), AppError> {
        for (from, to) in [
            ("base_url = \"http://127.0.0.1:15722/v1\"", "base_url = 42"),
            ("wire_api = \"responses\"", "wire_api = [\"responses\"]"),
            (
                "experimental_bearer_token = \"profile-listener-token\"",
                "experimental_bearer_token = 42",
            ),
        ] {
            let home = tempfile::tempdir().expect("创建临时 Home");
            let path = codex_config_path_for_home(home.path());
            fs::write(&path, "model = \"before\"\n").expect("写入接管前配置");
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
            assert!(changed.contains(to), "测试夹具必须成功改写字段类型");
            fs::write(&path, &changed).expect("写入异常类型配置");

            assert!(service
                .restore_profile_backup(home.path(), &backup, 15_722, "profile-listener-token",)
                .is_err());
            assert_eq!(fs::read_to_string(&path).expect("重读配置"), changed);
        }

        let home = tempfile::tempdir().expect("创建原始 Item Home");
        let path = codex_config_path_for_home(home.path());
        fs::write(
            &path,
            "base_url = 7\nwire_api = \"chat\"\nexperimental_bearer_token = \"upstream-token\"\n",
        )
        .expect("写入非字符串原始 Item");
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
        let restored: toml::Value =
            toml::from_str(&fs::read_to_string(&path).expect("读取原始 Item 恢复结果"))
                .expect("解析恢复结果");
        assert_eq!(
            restored
                .get("base_url")
                .and_then(|value| value.as_integer()),
            Some(7)
        );
        Ok(())
    }

    /// 恢复校验后文件再次变化时必须拒绝写入。
    #[test]
    fn profile_restore_rejects_change_between_validation_and_write() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let path = codex_config_path_for_home(home.path());
        fs::write(&path, "model = \"before\"\n").expect("写入接管前配置");
        let system_service = CodexHomeConfigService::system();
        let plan = system_service.build_profile_route_plan(
            home.path(),
            15_722,
            None,
            "profile-listener-token",
        )?;
        let backup = system_service.serialize_backup(&plan)?;
        system_service.apply_route_plan(&plan)?;
        let current = fs::read(&path).expect("读取接管配置");
        let mut changed = current.clone();
        changed.extend_from_slice(b"\n[desktop]\nfollowUpQueueMode = \"queue\"\n");
        let file_ops = Arc::new(ChangingReadFileOps::new(vec![Some(current), Some(changed)]));
        let service = CodexHomeConfigService::new(file_ops.clone());

        assert!(service
            .restore_profile_backup(home.path(), &backup, 15_722, "profile-listener-token",)
            .is_err());
        assert_eq!(file_ops.writes.load(Ordering::SeqCst), 0);
        Ok(())
    }

    /// 字段级合并的原子替换失败时必须保留当前路由配置。
    #[test]
    fn profile_restore_preserves_current_config_on_atomic_write_failure() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let path = codex_config_path_for_home(home.path());
        fs::write(&path, "model = \"before\"\n").expect("写入接管前配置");
        let system_service = CodexHomeConfigService::system();
        let plan = system_service.build_profile_route_plan(
            home.path(),
            15_722,
            None,
            "profile-listener-token",
        )?;
        let backup = system_service.serialize_backup(&plan)?;
        system_service.apply_route_plan(&plan)?;
        let current = fs::read(&path).expect("读取接管配置");
        let service = CodexHomeConfigService::new(Arc::new(FailingRenameFileOps));

        assert!(service
            .restore_profile_backup(home.path(), &backup, 15_722, "profile-listener-token",)
            .is_err());
        assert_eq!(fs::read(&path).expect("重读当前配置"), current);
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
    fn profile_ownership_ignores_non_route_changes_and_hides_listener_token() -> Result<(), AppError>
    {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        fs::write(
            &config_path,
            r#"model_provider = "custom"
model = "before-model"

[model_providers.custom]
base_url = "https://upstream.example/v1"
wire_api = "responses"
experimental_bearer_token = "upstream-token"

[model_providers.inactive]
base_url = "https://inactive.example/v1"
wire_api = "chat"
"#,
        )
        .expect("写入接管前配置");
        let service = CodexHomeConfigService::system();
        let listener_token = "listener-token-must-not-be-persisted";
        let plan = service.build_profile_route_plan(home.path(), 15_722, None, listener_token)?;
        let backup = service.serialize_backup(&plan)?;
        service.apply_route_plan(&plan)?;

        let changed = fs::read_to_string(&config_path)
            .expect("读取接管配置")
            .replace("model = \"before-model\"", "model = \"latest-model\"")
            .replace(
                "https://inactive.example/v1",
                "https://changed-inactive.example/v1",
            )
            + "\n[desktop]\nfollowUpQueueMode = \"queue\"\n\n[plugins.example]\nenabled = true\n";
        fs::write(&config_path, changed).expect("写入非路由变化");
        let desired =
            service.build_profile_route_plan(home.path(), 15_722, None, listener_token)?;

        assert_eq!(
            service.classify_profile_reconcile(&desired, &backup, 15_722)?,
            CodexHomeReconcileOwnership::Current
        );
        let backup_json: serde_json::Value = serde_json::from_str(&backup).expect("解析备份");
        assert_eq!(backup_json["version"], CODEX_ROUTE_BACKUP_VERSION);
        assert!(backup_json.get("ownership_proof").is_some());
        assert!(!backup.contains(listener_token));
        Ok(())
    }

    /// 当前备份 writer 缺少严格字段时必须拒绝，不能伪装成历史 v1 备份。
    #[test]
    fn current_backup_writer_rejects_missing_strict_route_fields() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        let original = b"model = \"keep-user-model\"\n";
        fs::write(&config_path, original).expect("写入接管前配置");
        let service = CodexHomeConfigService::system();
        let plan = service.build_route_plan(
            home.path(),
            "model_provider = \"custom\"\n[model_providers.custom]\nwire_api = \"responses\"\nexperimental_bearer_token = \"listener-token\"\n",
        )?;

        let error = service
            .serialize_backup(&plan)
            .expect_err("缺少 base_url 的当前目标必须拒绝序列化");

        assert!(error.to_string().contains("缺少 base_url"));
        assert_eq!(fs::read(&config_path).expect("读取原配置"), original);
        Ok(())
    }

    /// 当前备份 writer 收到非 UTF-8 目标时必须传播错误，不能创建历史版本。
    #[test]
    fn current_backup_writer_rejects_non_utf8_route_target() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        let original = b"model = \"keep-user-model\"\n";
        fs::write(&config_path, original).expect("写入接管前配置");
        let service = CodexHomeConfigService::system();
        let mut plan = service.build_route_plan(
            home.path(),
            "base_url = \"http://127.0.0.1:15722/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"listener-token\"\n",
        )?;
        plan.target_content = vec![0xff, 0xfe];

        let error = service
            .serialize_backup(&plan)
            .expect_err("非 UTF-8 当前目标必须拒绝序列化");

        assert!(error.to_string().contains("不是 UTF-8"));
        assert_eq!(fs::read(&config_path).expect("读取原配置"), original);
        Ok(())
    }

    /// 当前备份 writer 缺 listener token 时必须传播错误，不能创建历史版本。
    #[test]
    fn current_backup_writer_rejects_missing_listener_token() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let service = CodexHomeConfigService::system();
        let plan = service.build_route_plan(
            home.path(),
            "base_url = \"http://127.0.0.1:15722/v1\"\nwire_api = \"responses\"\n",
        )?;

        let error = service
            .serialize_backup(&plan)
            .expect_err("缺 listener token 的当前目标必须拒绝序列化");

        assert!(error.to_string().contains("缺少本地凭证"));
        assert!(!codex_config_path_for_home(home.path()).exists());
        Ok(())
    }

    /// v2 字段级备份必须保持可读，并在重定位时消除可逆 listener token。
    #[test]
    fn rebasing_v2_backup_redacts_listener_token_and_preserves_restore_semantics(
    ) -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        let listener_token = "old-profile-listener-token";
        let original = format!(
            "base_url = \"https://user.example/v1\"\nwire_api = \"chat\"\nexperimental_bearer_token = \"{listener_token}\"\n"
        );
        fs::write(&config_path, &original).expect("写入 v2 接管前配置");
        let service = CodexHomeConfigService::system();
        let plan = service.build_profile_route_plan(home.path(), 15_722, None, listener_token)?;
        let mut v2: serde_json::Value =
            serde_json::from_str(&service.serialize_backup(&plan)?).expect("解析新备份");
        v2["version"] = serde_json::json!(CODEX_ROUTE_OWNERSHIP_PROOF_BACKUP_VERSION);
        v2["previous_content"] = serde_json::to_value(original.as_bytes()).expect("编码 v2 正文");
        v2.as_object_mut()
            .expect("备份为对象")
            .remove("previous_token_state");
        let v2 = serde_json::to_string(&v2).expect("编码 v2 备份");
        service.apply_route_plan(&plan)?;
        let desired =
            service.build_profile_route_plan(home.path(), 15_722, None, listener_token)?;

        assert_eq!(
            service.classify_profile_reconcile(&desired, &v2, 15_722)?,
            CodexHomeReconcileOwnership::Current
        );
        let upgraded = service.rebase_route_backup_to_plan(&v2, &desired)?;
        let upgraded_json: serde_json::Value =
            serde_json::from_str(&upgraded).expect("解析升级备份");
        let previous: Vec<u8> = serde_json::from_value(upgraded_json["previous_content"].clone())
            .expect("解码升级正文");
        assert_ne!(
            crate::codex_config::extract_codex_experimental_bearer_token(
                std::str::from_utf8(&previous).expect("UTF-8 备份")
            )
            .as_deref(),
            Some(listener_token)
        );
        assert_eq!(
            upgraded_json["previous_token_state"],
            "listener_token_reference"
        );

        let rebased_again = service.rebase_route_backup_to_plan(&upgraded, &desired)?;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&rebased_again).expect("解析再次重定位备份")
                ["previous_token_state"],
            "listener_token_reference"
        );
        service.restore_profile_backup(home.path(), &rebased_again, 15_722, listener_token)?;
        assert_eq!(
            fs::read_to_string(config_path).expect("读取恢复配置"),
            original
        );
        Ok(())
    }

    /// 旧 v3 无路径引用仅在接管前指纹能唯一证明来源时恢复，否则必须拒绝猜测。
    #[test]
    fn legacy_pathless_v3_token_origin_requires_unique_fingerprint_proof() -> Result<(), AppError> {
        let service = CodexHomeConfigService::system();
        let listener_token = "legacy-pathless-listener-token";

        let proven_home = tempfile::tempdir().expect("创建可证明旧 v3 Home");
        let proven_path = codex_config_path_for_home(proven_home.path());
        let proven_original = concat!(
            "model_provider = \"custom\"\n",
            "experimental_bearer_token = \"legacy-pathless-listener-token\"\n\n",
            "[model_providers.custom]\n",
            "base_url = \"https://upstream.example/v1\"\n",
            "wire_api = \"responses\"\n",
        );
        fs::write(&proven_path, proven_original).expect("写入可证明配置");
        let proven_plan =
            service.build_profile_route_plan(proven_home.path(), 15_722, None, listener_token)?;
        let mut pathless: serde_json::Value =
            serde_json::from_str(&service.serialize_backup(&proven_plan)?).expect("解析旧 v3");
        pathless
            .as_object_mut()
            .expect("备份为对象")
            .remove("previous_token_origin");
        let pathless = serde_json::to_string(&pathless).expect("编码旧 v3");
        service.apply_route_plan(&proven_plan)?;
        service.restore_profile_backup(proven_home.path(), &pathless, 15_722, listener_token)?;
        assert_eq!(
            fs::read(&proven_path).expect("读取证明恢复配置"),
            proven_original.as_bytes()
        );

        let ambiguous_home = tempfile::tempdir().expect("创建模糊旧 v3 Home");
        let ambiguous_path = codex_config_path_for_home(ambiguous_home.path());
        fs::write(&ambiguous_path, proven_original).expect("写入模糊配置");
        let ambiguous_plan = service.build_profile_route_plan(
            ambiguous_home.path(),
            15_723,
            None,
            listener_token,
        )?;
        let mut ambiguous: serde_json::Value =
            serde_json::from_str(&service.serialize_backup(&ambiguous_plan)?)
                .expect("解析模糊备份");
        ambiguous
            .as_object_mut()
            .expect("备份为对象")
            .remove("previous_token_origin");
        ambiguous["previous_fingerprint"] = serde_json::json!("not-a-provable-fingerprint");
        let ambiguous = serde_json::to_string(&ambiguous).expect("编码模糊备份");
        service.apply_route_plan(&ambiguous_plan)?;
        let routed_before = fs::read(&ambiguous_path).expect("读取拒绝前路由配置");

        let error = service
            .restore_profile_backup(ambiguous_home.path(), &ambiguous, 15_723, listener_token)
            .expect_err("无唯一证明的旧 v3 必须拒绝恢复");
        assert!(error.to_string().contains("无法唯一证明"));
        assert!(!error.to_string().contains(listener_token));
        assert_eq!(
            fs::read(ambiguous_path).expect("读取拒绝后路由配置"),
            routed_before
        );
        Ok(())
    }

    /// v3 备份必须脱敏全部 listener token 路径，并保留普通用户 token 与关闭恢复语义。
    #[test]
    fn backup_redacts_listener_token_from_all_paths_and_restores_without_data_loss(
    ) -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        let listener_token = "profile-listener-token-must-not-be-reversible";
        let ordinary_token = "ordinary-user-token-must-be-preserved";
        let original = format!(
            r#"model_provider = "active"
experimental_bearer_token = "{listener_token}"

[model_providers.active]
base_url = "https://active.example/v1"
wire_api = "responses"
experimental_bearer_token = "{listener_token}"

[model_providers.inactive]
base_url = "https://inactive.example/v1"
wire_api = "responses"
experimental_bearer_token = "{listener_token}"

[model_providers.ordinary]
base_url = "https://ordinary.example/v1"
wire_api = "responses"
experimental_bearer_token = "{ordinary_token}"
"#
        );
        fs::write(&config_path, &original).expect("写入多路径 token 配置");
        let service = CodexHomeConfigService::system();
        let plan = service.build_profile_route_plan(home.path(), 15_722, None, listener_token)?;
        let backup = service.serialize_backup(&plan)?;
        service.apply_route_plan(&plan)?;
        let desired =
            service.build_profile_route_plan(home.path(), 15_722, None, listener_token)?;
        let backup = service.rebase_route_backup_to_plan(&backup, &desired)?;
        let backup_json: serde_json::Value = serde_json::from_str(&backup).expect("解析备份");
        let previous: Vec<u8> = serde_json::from_value(backup_json["previous_content"].clone())
            .expect("解码接管前正文");

        assert!(!previous
            .windows(listener_token.len())
            .any(|window| window == listener_token.as_bytes()));
        assert!(previous
            .windows(ordinary_token.len())
            .any(|window| window == ordinary_token.as_bytes()));
        assert_eq!(
            backup_json["previous_token_origin"],
            serde_json::json!({"provider": "active"})
        );
        service.restore_profile_backup(home.path(), &backup, 15_722, listener_token)?;
        assert_eq!(
            fs::read_to_string(config_path).expect("读取关闭恢复配置"),
            original
        );
        Ok(())
    }

    /// inline table 中的全部 listener token 也必须不可逆处理，且不得破坏普通字段。
    #[test]
    fn backup_redacts_listener_token_from_inline_provider_tables() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        let listener_token = "profile-listener-token-inline-must-not-be-reversible";
        let ordinary_token = "ordinary-inline-token-must-be-preserved";
        let original = format!(
            r#"model_provider = "active"
experimental_bearer_token = "{listener_token}"
model_providers = {{ active = {{ base_url = "https://active.example/v1", wire_api = "responses", experimental_bearer_token = "{listener_token}", label = "keep-active" }}, inactive = {{ base_url = "https://inactive.example/v1", wire_api = "responses", experimental_bearer_token = "{listener_token}", note = "keep-inactive" }}, ordinary = {{ experimental_bearer_token = "{ordinary_token}", marker = "keep-ordinary" }} }}
"#
        );
        fs::write(&config_path, &original).expect("写入 inline provider token 配置");
        let service = CodexHomeConfigService::system();
        let plan = service.build_profile_route_plan(home.path(), 15_722, None, listener_token)?;
        let backup = service.serialize_backup(&plan)?;
        let backup_json: serde_json::Value = serde_json::from_str(&backup).expect("解析备份");
        let previous: Vec<u8> = serde_json::from_value(backup_json["previous_content"].clone())
            .expect("解码接管前正文");
        let previous_toml = String::from_utf8(previous).expect("备份正文应为 UTF-8");

        assert_eq!(backup_json["version"], CODEX_ROUTE_BACKUP_VERSION);
        assert_eq!(
            backup_json["previous_token_origin"],
            serde_json::json!({"provider": "active"})
        );
        assert!(!previous_toml.contains(listener_token));
        assert!(previous_toml.contains(ordinary_token));
        assert!(previous_toml.contains("model_providers = {"));
        assert!(previous_toml.contains("label = \"keep-active\""));
        assert!(previous_toml.contains("note = \"keep-inactive\""));
        assert!(previous_toml.contains("marker = \"keep-ordinary\""));
        service.apply_route_plan(&plan)?;
        service.restore_profile_backup(home.path(), &backup, 15_722, listener_token)?;
        assert_eq!(
            fs::read_to_string(config_path).expect("读取恢复后的 inline 配置"),
            original
        );
        Ok(())
    }

    /// inline 非活动 provider 独占 listener token 时必须拒绝有损备份。
    #[test]
    fn backup_fails_closed_for_listener_token_only_on_inline_inactive_provider(
    ) -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        let listener_token = "profile-listener-token-inline-inactive";
        let original = format!(
            r#"model_provider = "active"
model_providers = {{ active = {{ base_url = "https://active.example/v1", wire_api = "responses", experimental_bearer_token = "ordinary-active-token" }}, inactive = {{ experimental_bearer_token = "{listener_token}", note = "keep-inactive" }} }}
"#
        );
        fs::write(&config_path, &original).expect("写入 inline 非活动 token 配置");
        let service = CodexHomeConfigService::system();
        let plan = service.build_profile_route_plan(home.path(), 15_722, None, listener_token)?;

        let error = service
            .serialize_backup(&plan)
            .expect_err("必须拒绝有损备份");

        assert!(error.to_string().contains("无法无损表达"));
        assert_eq!(
            fs::read_to_string(config_path).expect("读取未修改配置"),
            original
        );
        Ok(())
    }

    /// 只有非活动路径残留 listener token 时无法无损恢复，必须拒绝持久化备份。
    #[test]
    fn backup_fails_closed_for_listener_token_only_on_inactive_path() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        let listener_token = "profile-listener-token-on-inactive-path";
        let original = format!(
            r#"model_provider = "active"

[model_providers.active]
base_url = "https://active.example/v1"
wire_api = "responses"
experimental_bearer_token = "ordinary-active-token"

[model_providers.inactive]
base_url = "https://inactive.example/v1"
wire_api = "responses"
experimental_bearer_token = "{listener_token}"
"#
        );
        fs::write(&config_path, &original).expect("写入非活动 listener token 配置");
        let service = CodexHomeConfigService::system();
        let plan = service.build_profile_route_plan(home.path(), 15_722, None, listener_token)?;

        let error = service
            .serialize_backup(&plan)
            .expect_err("必须拒绝有损备份");

        assert!(error.to_string().contains("无法无损表达"));
        assert_eq!(
            fs::read_to_string(config_path).expect("读取未修改配置"),
            original
        );
        Ok(())
    }

    /// 未知未来备份版本不得被关闭、补偿、切换基线或启动恢复入口消费。
    #[test]
    fn future_backup_version_fails_closed_at_every_home_service_entry() -> Result<(), AppError> {
        let listener_token = "profile-listener-token";
        for entry in ["disable", "enable_recovery", "switch_baseline", "startup"] {
            let home = tempfile::tempdir().expect("创建临时 Home");
            let config_path = codex_config_path_for_home(home.path());
            fs::write(
                &config_path,
                "model_provider = \"custom\"\nmodel = \"user-model\"\n\n[model_providers.custom]\nbase_url = \"https://user.example/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"user-token\"\n",
            )
            .expect("写入用户配置");
            let service = CodexHomeConfigService::system();
            let plan =
                service.build_profile_route_plan(home.path(), 15_722, None, listener_token)?;
            let backup = service.serialize_backup(&plan)?;
            let mut future: serde_json::Value =
                serde_json::from_str(&backup).expect("解析当前备份");
            future["version"] = serde_json::json!(4);
            let future = serde_json::to_string(&future).expect("编码未来备份");
            service.apply_route_plan(&plan)?;
            let before = fs::read(&config_path).expect("读取入口前配置");
            let current_plan =
                service.build_profile_route_plan(home.path(), 15_722, None, listener_token)?;

            let result = match entry {
                "disable" => {
                    service.restore_profile_backup(home.path(), &future, 15_722, listener_token)
                }
                "enable_recovery" => service.restore_enable_recovery_backup(
                    home.path(),
                    &future,
                    15_722,
                    Some(listener_token),
                ),
                "switch_baseline" => service
                    .serialize_explicit_switch_backup(
                        &current_plan,
                        Some(&future),
                        15_722,
                        listener_token,
                    )
                    .map(|_| ()),
                "startup" => service
                    .classify_profile_reconcile(&current_plan, &future, 15_722)
                    .map(|_| ()),
                _ => unreachable!(),
            };

            let error = result.expect_err("未知未来备份版本必须 fail closed");
            assert!(error.to_string().contains("版本"), "entry={entry}: {error}");
            assert_eq!(fs::read(&config_path).expect("读取入口后配置"), before);
        }
        Ok(())
    }

    /// token secret 重建后只能用备份摘要证明旧 token，不能依赖整文件指纹。
    #[test]
    fn profile_ownership_proves_stale_listener_token_after_non_route_change() -> Result<(), AppError>
    {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        fs::write(
            &config_path,
            "model_provider = \"custom\"\nmodel = \"before\"\n\n[model_providers.custom]\nbase_url = \"https://upstream.example/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"upstream-token\"\n",
        )
        .expect("写入接管前配置");
        let service = CodexHomeConfigService::system();
        let old_plan =
            service.build_profile_route_plan(home.path(), 15_722, None, "old-listener-token")?;
        let backup = service.serialize_backup(&old_plan)?;
        service.apply_route_plan(&old_plan)?;
        let changed = fs::read_to_string(&config_path)
            .expect("读取旧路由配置")
            .replace("model = \"before\"", "model = \"latest\"")
            + "\n[desktop]\nfollowUpQueueMode = \"queue\"\n";
        fs::write(&config_path, changed).expect("写入非路由变化");
        let desired =
            service.build_profile_route_plan(home.path(), 15_722, None, "new-listener-token")?;

        assert_eq!(
            service.classify_profile_reconcile(&desired, &backup, 15_722)?,
            CodexHomeReconcileOwnership::RouteOwned
        );
        Ok(())
    }

    /// v1 备份缺少字段级证明时，当前 listener token 仍能证明路由所有权。
    #[test]
    fn profile_ownership_upgrades_v1_backup_when_current_token_still_matches(
    ) -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        fs::write(
            &config_path,
            "model_provider = \"custom\"\nmodel = \"before\"\n\n[model_providers.custom]\nbase_url = \"https://upstream.example/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"upstream-token\"\n",
        )
        .expect("写入接管前配置");
        let service = CodexHomeConfigService::system();
        let listener_token = "current-listener-token";
        let plan = service.build_profile_route_plan(home.path(), 15_722, None, listener_token)?;
        let mut legacy_backup: serde_json::Value =
            serde_json::from_str(&service.serialize_backup(&plan)?).expect("解析新版备份");
        legacy_backup
            .as_object_mut()
            .expect("备份应为对象")
            .remove("version");
        legacy_backup
            .as_object_mut()
            .expect("备份应为对象")
            .remove("ownership_proof");
        let legacy_backup = serde_json::to_string(&legacy_backup).expect("编码 v1 备份");
        service.apply_route_plan(&plan)?;
        let changed = fs::read_to_string(&config_path)
            .expect("读取接管配置")
            .replace("model = \"before\"", "model = \"latest\"")
            + "\n[desktop]\nfollowUpQueueMode = \"queue\"\n";
        fs::write(&config_path, changed).expect("写入非路由变化");
        let desired =
            service.build_profile_route_plan(home.path(), 15_722, None, listener_token)?;

        assert_eq!(
            service.classify_profile_reconcile(&desired, &legacy_backup, 15_722)?,
            CodexHomeReconcileOwnership::Current
        );
        Ok(())
    }

    /// 当前活动连接路径的 selector 或严格字段变化必须分类为外部接管。
    #[test]
    fn profile_ownership_classifies_active_route_changes_as_external_takeover(
    ) -> Result<(), AppError> {
        let original = r#"model_provider = "custom"

[model_providers.custom]
base_url = "https://upstream.example/v1"
wire_api = "responses"
experimental_bearer_token = "upstream-token"

[model_providers.external]
base_url = "https://external.example/v1"
wire_api = "chat"
experimental_bearer_token = "external-token"
"#;
        let cases = [
            (
                "model_provider = \"custom\"",
                "model_provider = \"external\"",
            ),
            ("http://127.0.0.1:15722/v1", "https://external.example/v1"),
            ("wire_api = \"responses\"", "wire_api = \"chat\""),
            (
                "experimental_bearer_token = \"listener-token-aaaaaaaa\"",
                "experimental_bearer_token = \"external-token-bbbbbbb\"",
            ),
        ];

        for (from, to) in cases {
            let home = tempfile::tempdir().expect("创建临时 Home");
            let config_path = codex_config_path_for_home(home.path());
            fs::write(&config_path, original).expect("写入接管前配置");
            let service = CodexHomeConfigService::system();
            let plan = service.build_profile_route_plan(
                home.path(),
                15_722,
                None,
                "listener-token-aaaaaaaa",
            )?;
            let backup = service.serialize_backup(&plan)?;
            service.apply_route_plan(&plan)?;
            let changed = fs::read_to_string(&config_path)
                .expect("读取接管配置")
                .replacen(from, to, 1);
            assert!(changed.contains(to), "测试夹具必须改写活动连接路径");
            fs::write(&config_path, changed).expect("写入外部接管配置");
            let desired = service.build_profile_route_plan(
                home.path(),
                15_722,
                None,
                "listener-token-aaaaaaaa",
            )?;

            assert_eq!(
                service.classify_profile_reconcile(&desired, &backup, 15_722)?,
                CodexHomeReconcileOwnership::ExternalTakeover
            );
        }
        Ok(())
    }

    /// selector 即使改到相同本地三字段，也属于用户显式改变活动连接路径。
    #[test]
    fn profile_ownership_rejects_changed_selector_with_same_route_shape() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("创建临时 Home");
        let config_path = codex_config_path_for_home(home.path());
        fs::write(
            &config_path,
            "model_provider = \"custom\"\n\n[model_providers.custom]\nbase_url = \"https://upstream.example/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"upstream-token\"\n",
        )
        .expect("写入接管前配置");
        let service = CodexHomeConfigService::system();
        let plan = service.build_profile_route_plan(
            home.path(),
            15_722,
            None,
            "listener-token-aaaaaaaa",
        )?;
        let backup = service.serialize_backup(&plan)?;
        service.apply_route_plan(&plan)?;
        let routed = fs::read_to_string(&config_path).expect("读取接管配置");
        let current = routed.replace("model_provider = \"custom\"", "model_provider = \"other\"")
            + "\n[model_providers.other]\nbase_url = \"http://127.0.0.1:15722/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"listener-token-aaaaaaaa\"\n";
        fs::write(&config_path, current).expect("写入相同形状的新 selector");
        let desired = service.build_profile_route_plan(
            home.path(),
            15_722,
            None,
            "listener-token-aaaaaaaa",
        )?;

        assert_eq!(
            service.classify_profile_reconcile(&desired, &backup, 15_722)?,
            CodexHomeReconcileOwnership::ExternalTakeover
        );
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
        let rebased = service.rebase_route_backup_to_plan(&old_backup, &desired_plan)?;
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
        assert_ne!(
            rebased_json["ownership_proof"]["token_digest"],
            old_json["ownership_proof"]["token_digest"]
        );
        assert_eq!(rebased_json["version"], CODEX_ROUTE_BACKUP_VERSION);
        assert!(!rebased.contains("old-listener-token"));
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
        assert_eq!(
            service.classify_profile_reconcile(&external_plan, &backup, 15_722)?,
            CodexHomeReconcileOwnership::ExternalTakeover
        );
        assert!(service
            .restore_profile_backup(home.path(), &backup, 15_722, "new-token")
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
