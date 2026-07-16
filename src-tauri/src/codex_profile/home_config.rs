use crate::codex_config::{
    codex_config_path_for_home, CodexCatalogToolProfile, CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME,
};
use crate::codex_profile::{
    CODEX_MODEL_PROVIDERS_TABLE, CODEX_MODEL_PROVIDER_FIELD, CODEX_ROUTE_FIELD_BASE_URL,
    CODEX_ROUTE_FIELD_BEARER_TOKEN, CODEX_ROUTE_FIELD_WIRE_API, CODEX_ROUTE_LISTEN_HOST,
    CODEX_ROUTE_TOKEN_MISMATCH_DETAIL, CODEX_ROUTE_WIRE_API_RESPONSES, LEGACY_PROXY_MANAGED_TOKEN,
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
}

/// 持久化在 Profile 路由关系中的最小 Home 恢复信息。
#[derive(Serialize, Deserialize)]
struct CodexRouteBackup {
    previous_content: Option<Vec<u8>>,
    previous_fingerprint: String,
    target_fingerprint: String,
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
        let config = if current.content.as_deref() == Some(prepared.config_text.as_bytes()) {
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
        let projection = build_managed_route_projection(
            backup.previous_content.as_deref(),
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
        if current_state == projection.previous {
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

/// 返回文档中当前活动 provider 标识。
fn active_codex_provider_id(document: &DocumentMut) -> Option<String> {
    document
        .get(CODEX_MODEL_PROVIDER_FIELD)
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|provider_id| !provider_id.is_empty())
        .map(str::to_string)
}

/// 从准确路径读取字段 Item。
fn route_field_item<'a>(document: &'a DocumentMut, path: &CodexRouteFieldPath) -> Option<&'a Item> {
    match path {
        CodexRouteFieldPath::TopLevel(field) => document.get(field),
        CodexRouteFieldPath::Provider { provider_id, field } => document
            .get(CODEX_MODEL_PROVIDERS_TABLE)
            .and_then(Item::as_table)
            .and_then(|providers| providers.get(provider_id))
            .and_then(Item::as_table)
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

/// 记录 target 相比 previous 新建的 provider 表。
fn created_provider_tables(previous: &DocumentMut, target: &DocumentMut) -> Vec<String> {
    let previous_providers = previous
        .get(CODEX_MODEL_PROVIDERS_TABLE)
        .and_then(Item::as_table);
    target
        .get(CODEX_MODEL_PROVIDERS_TABLE)
        .and_then(Item::as_table)
        .map(|providers| {
            providers
                .iter()
                .filter(|(provider_id, item)| {
                    item.as_table().is_some()
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
                .and_then(Item::as_table)
                .is_some();
    let previous = CodexManagedRouteState {
        base_url: read_route_field_state(&previous_document, &base_url_path),
        wire_api: read_route_field_state(&previous_document, &wire_api_path),
        bearer_token: read_route_field_state(&previous_document, &bearer_token_path),
    };
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
                .and_then(Item::as_table_mut)
                .and_then(|providers| providers.get_mut(provider_id))
                .and_then(Item::as_table_mut)
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
        .and_then(Item::as_table_mut)
    {
        for provider_id in &projection.created_provider_tables {
            let should_remove = providers
                .get(provider_id)
                .and_then(Item::as_table)
                .map(toml_edit::Table::is_empty)
                .unwrap_or(false);
            if should_remove {
                providers.remove(provider_id);
            }
        }
    }
    let should_remove_parent = projection.created_model_providers_table
        && document
            .get(CODEX_MODEL_PROVIDERS_TABLE)
            .and_then(Item::as_table)
            .map(toml_edit::Table::is_empty)
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
    cleanup_created_provider_tables(current, projection);
    Ok(())
}

/// 读取活动 provider 中的字符串字段，缺失时回退顶层。
fn extract_active_codex_route_string(content: &str, field: &str) -> Option<String> {
    let document = content.parse::<DocumentMut>().ok()?;
    if let Some(provider_id) = active_codex_provider_id(&document) {
        if let Some(value) = document
            .get(CODEX_MODEL_PROVIDERS_TABLE)
            .and_then(Item::as_table)
            .and_then(|providers| providers.get(&provider_id))
            .and_then(Item::as_table)
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
