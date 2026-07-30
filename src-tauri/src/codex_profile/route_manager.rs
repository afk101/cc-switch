//! Codex Profile 独立路由生命周期编排。

use crate::app_config::AppType;
use crate::codex_profile::catalog_sync::{
    apply_catalog_projection_batch, restore_catalog_projection_batch, CodexCatalogProjectionEntry,
};
use crate::codex_profile::{
    CodexHomeConfigService, CodexHomeReconcileOwnership, CodexModelCatalogProjectionPlan,
    CodexProfile, CodexProfileRoute, CodexProfileScope, CodexProfileSecretStore,
    CodexRouteConfigPlan, CodexRouteProviderSnapshot, CodexRouteRuntime, CodexRouteRuntimeFactory,
    CodexRuntimeStatus, CODEX_CATALOG_RECONCILE_ERROR_PREFIX,
    CODEX_CATALOG_SYNC_COMPENSATION_ERROR, CODEX_ROUTE_COMPENSATION_UNCONVERGED_ERROR,
    CODEX_ROUTE_RECOVERY_OPERATION_RECONCILE, CODEX_ROUTE_RECOVERY_PHASE_DELETE_DATABASE_FAILED,
    CODEX_ROUTE_RECOVERY_PHASE_DELETE_TOKEN_PENDING,
    CODEX_ROUTE_RECOVERY_PHASE_DISABLE_HOME_RESTORE_FAILED,
    CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOPPED, CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOP_FAILED,
    CODEX_ROUTE_RECOVERY_PHASE_ENABLE_HOME_APPLIED,
    CODEX_ROUTE_RECOVERY_PHASE_ENABLE_PERSIST_FAILED, CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED,
    CODEX_ROUTE_RECOVERY_PHASE_PREPARED, CODEX_ROUTE_RECOVERY_PHASE_RECONCILE_HOME_APPLIED,
    CODEX_ROUTE_RECOVERY_PHASE_RECONCILE_PREPARED,
    CODEX_ROUTE_RECOVERY_PHASE_SWITCH_FAILOVERS_SAVED,
    CODEX_ROUTE_RECOVERY_PHASE_SWITCH_ROUTE_SAVED,
    CODEX_ROUTE_RECOVERY_PHASE_SWITCH_RUNTIME_SWAPPED,
};
use crate::database::Database;
use crate::error::AppError;
use crate::provider::Provider;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

/// manager 所需的 Profile 路由持久化最小契约。
pub trait CodexProfileRoutePersistence: Send + Sync {
    fn list_profiles(&self) -> Result<Vec<CodexProfile>, AppError>;
    fn get_profile(&self, profile_id: &str) -> Result<CodexProfile, AppError>;
    fn get_route(&self, profile_id: &str) -> Result<Option<CodexProfileRoute>, AppError>;
    fn save_route(&self, route: &CodexProfileRoute) -> Result<(), AppError>;
    fn list_failovers(&self, profile_id: &str) -> Result<Vec<String>, AppError>;
    fn replace_failovers(&self, profile_id: &str, provider_ids: &[String]) -> Result<(), AppError>;
    fn get_provider(&self, provider_id: &str) -> Result<Option<Provider>, AppError>;
    fn save_provider_snapshot(&self, _provider: &Provider) -> Result<(), AppError> {
        Err(AppError::InvalidInput(
            "当前 Codex Profile 持久化适配器不支持供应商快照恢复".to_string(),
        ))
    }
    fn delete_profile(&self, profile_id: &str) -> Result<(), AppError>;
}

/// 数据库适配器只转发 manager 需要的 Profile 路由持久化操作。
impl CodexProfileRoutePersistence for Database {
    fn list_profiles(&self) -> Result<Vec<CodexProfile>, AppError> {
        self.list_codex_profiles()
    }
    fn get_profile(&self, profile_id: &str) -> Result<CodexProfile, AppError> {
        self.get_codex_profile(profile_id)
    }
    fn get_route(&self, profile_id: &str) -> Result<Option<CodexProfileRoute>, AppError> {
        self.get_codex_profile_route(profile_id)
    }
    fn save_route(&self, route: &CodexProfileRoute) -> Result<(), AppError> {
        self.save_codex_profile_route(route)
    }
    fn list_failovers(&self, profile_id: &str) -> Result<Vec<String>, AppError> {
        self.list_codex_profile_failovers(profile_id)
    }
    fn replace_failovers(&self, profile_id: &str, provider_ids: &[String]) -> Result<(), AppError> {
        self.replace_codex_profile_failovers(profile_id, provider_ids)
    }
    fn get_provider(&self, provider_id: &str) -> Result<Option<Provider>, AppError> {
        self.get_provider_by_id(provider_id, AppType::Codex.as_str())
    }
    fn save_provider_snapshot(&self, provider: &Provider) -> Result<(), AppError> {
        self.save_provider(AppType::Codex.as_str(), provider)
    }
    fn delete_profile(&self, profile_id: &str) -> Result<(), AppError> {
        self.delete_codex_profile(profile_id)
    }
}

/// manager 所需的本地 token 生命周期最小契约。
pub trait CodexProfileTokenStore: Send + Sync {
    fn read_token(&self, profile_id: &str) -> Result<Option<String>, AppError>;
    fn ensure_token(&self, profile_id: &str) -> Result<String, AppError>;
    fn delete_token(&self, profile_id: &str) -> Result<(), AppError>;
}

impl CodexProfileTokenStore for CodexProfileSecretStore {
    fn read_token(&self, profile_id: &str) -> Result<Option<String>, AppError> {
        self.read(profile_id)
    }
    fn ensure_token(&self, profile_id: &str) -> Result<String, AppError> {
        self.create(profile_id)
    }
    fn delete_token(&self, profile_id: &str) -> Result<(), AppError> {
        self.delete(profile_id)
    }
}

/// 每个 Profile 的路由生命周期管理器，不使用全局 ProxyService listener slot。
pub struct CodexRouteManager {
    persistence: Arc<dyn CodexProfileRoutePersistence>,
    home_config: Arc<CodexHomeConfigService>,
    secret_store: Arc<dyn CodexProfileTokenStore>,
    runtime_factory: Arc<dyn CodexRouteRuntimeFactory>,
    runtimes: Mutex<HashMap<String, Arc<dyn CodexRouteRuntime>>>,
    locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    catalog_sync_lock: AsyncMutex<()>,
}

/// 补偿记录只保存路由标识与开关状态，避免把 Home 正文或凭证写入数据库。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteRecoveryRecord {
    operation: String,
    before: RouteRecoverySnapshot,
    target: RouteRecoverySnapshot,
    phase: String,
    last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reconcile: Option<RouteReconcileTransition>,
}

/// 启动对账只持久化所有权指纹，不保存 token 或 Home 正文。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteReconcileTransition {
    old_target_fingerprint: String,
    new_target_fingerprint: String,
}

/// 可恢复的路由快照不包含 live backup、token 或认证配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteRecoverySnapshot {
    current_provider_id: Option<String>,
    enabled: bool,
    failover_ids: Vec<String>,
}

impl CodexRouteManager {
    /// 使用注入依赖创建 Profile 隔离路由管理器。
    pub fn new(
        persistence: Arc<dyn CodexProfileRoutePersistence>,
        home_config: Arc<CodexHomeConfigService>,
        secret_store: Arc<dyn CodexProfileTokenStore>,
        runtime_factory: Arc<dyn CodexRouteRuntimeFactory>,
    ) -> Self {
        Self {
            persistence,
            home_config,
            secret_store,
            runtime_factory,
            runtimes: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
            catalog_sync_lock: AsyncMutex::new(()),
        }
    }

    /// 判断是否存在任意已启用的 Profile 路由，用于启动时裁决 Codex Home 所有权。
    pub fn has_enabled_profile_routes(&self) -> Result<bool, AppError> {
        for profile in self.persistence.list_profiles()? {
            if self
                .persistence
                .get_route(&profile.id)?
                .map(|route| route.enabled)
                .unwrap_or(false)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// 返回主供应商引用匹配的 Profile，故障转移引用不参与目录扇出。
    fn profiles_using_primary_provider(
        &self,
        provider_id: &str,
    ) -> Result<Vec<CodexProfile>, AppError> {
        let mut profiles = Vec::new();
        for profile in self.persistence.list_profiles()? {
            let route = self.persistence.get_route(&profile.id)?;
            if route
                .as_ref()
                .and_then(|route| route.current_provider_id.as_deref())
                == Some(provider_id)
            {
                profiles.push(profile);
            }
        }
        profiles.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(profiles)
    }

    /// 在共享供应商提交前同步所有主引用 Home，并在提交失败时恢复双方状态。
    pub async fn with_provider_catalog_update<T, F>(
        &self,
        provider: &Provider,
        commit: F,
    ) -> Result<T, AppError>
    where
        F: FnOnce() -> Result<T, AppError>,
    {
        let _catalog_guard = self.catalog_sync_lock.lock().await;
        let profiles = self.profiles_using_primary_provider(&provider.id)?;
        let locks = profiles
            .iter()
            .map(|profile| self.profile_lock(&profile.id))
            .collect::<Result<Vec<_>, _>>()?;
        let mut _guards = Vec::with_capacity(locks.len());
        for lock in &locks {
            _guards.push(lock.lock().await);
        }

        let original = self
            .persistence
            .get_provider(&provider.id)?
            .ok_or_else(|| {
                AppError::InvalidInput(format!("Codex 供应商不存在: {}", provider.id))
            })?;
        let entries = profiles
            .iter()
            .map(|profile| {
                let home_path = std::path::PathBuf::from(&profile.canonical_home_path);
                Ok(CodexCatalogProjectionEntry {
                    profile_id: profile.id.clone(),
                    profile_name: profile.name.clone(),
                    plan: self
                        .home_config
                        .build_model_catalog_projection_plan(&home_path, provider)?,
                    home_path,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        let applied = apply_catalog_projection_batch(self.home_config.as_ref(), entries)?;

        match commit() {
            Ok(value) => Ok(value),
            Err(primary) => {
                let provider_restore = self.persistence.save_provider_snapshot(&original).err();
                let home_restore =
                    restore_catalog_projection_batch(self.home_config.as_ref(), &applied).err();
                Err(Self::catalog_update_compensation_error(
                    primary,
                    provider_restore,
                    home_restore,
                ))
            }
        }
    }

    /// 汇总供应商提交失败与双侧补偿结果，不包含供应商设置正文。
    fn catalog_update_compensation_error(
        primary: AppError,
        provider_restore: Option<AppError>,
        home_restore: Option<AppError>,
    ) -> AppError {
        match (provider_restore, home_restore) {
            (None, None) => primary,
            (provider_restore, home_restore) => AppError::Message(format!(
                "{CODEX_CATALOG_SYNC_COMPENSATION_ERROR}: {primary}; 供应商恢复: {}; Home 恢复: {}",
                provider_restore
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "成功".to_string()),
                home_restore
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "成功".to_string()),
            )),
        }
    }

    /// 启动指定 Profile 的独立监听器并原子接管其 Home 配置。
    pub async fn enable(
        &self,
        profile_id: &str,
        provider_id: &str,
        failover_ids: Vec<String>,
    ) -> Result<(), AppError> {
        self.enable_internal(profile_id, provider_id, Some(failover_ids))
            .await
    }

    /// 启用 Profile 路由并保留已经持久化的故障转移策略。
    pub async fn enable_preserving_failovers(
        &self,
        profile_id: &str,
        provider_id: &str,
    ) -> Result<(), AppError> {
        self.enable_internal(profile_id, provider_id, None).await
    }

    /// 在 Profile 锁内解析故障转移策略并执行启用流程。
    async fn enable_internal(
        &self,
        profile_id: &str,
        provider_id: &str,
        requested_failover_ids: Option<Vec<String>>,
    ) -> Result<(), AppError> {
        if provider_id.is_empty() {
            return Err(AppError::InvalidInput("Codex 供应商不能为空".to_string()));
        }
        self.validate_route_operation_before_lock(profile_id)?;
        if let Some(failover_ids) = requested_failover_ids.as_deref() {
            let _ = self.provider_snapshot(provider_id, failover_ids)?;
        }
        let _catalog_guard = self.catalog_sync_lock.lock().await;
        let lock = self.profile_lock(profile_id)?;
        let _guard = lock.lock().await;
        let profile = self.persistence.get_profile(profile_id)?;
        profile.validate_route_operation()?;
        self.recover_pending_locked(profile_id).await?;
        let failover_ids = self.resolve_failover_ids(profile_id, requested_failover_ids)?;
        let previous = self
            .persistence
            .get_route(profile_id)?
            .unwrap_or_else(|| Self::empty_route(&profile.id));
        let selected_provider = self
            .persistence
            .get_provider(provider_id)?
            .ok_or_else(|| AppError::InvalidInput("Codex 供应商不存在".to_string()))?;
        let snapshot = self.provider_snapshot(provider_id, &failover_ids)?;
        let token = self.secret_store.ensure_token(profile_id)?;
        let home = std::path::Path::new(&profile.canonical_home_path);
        let catalog_plan = self
            .home_config
            .build_model_catalog_projection_plan(home, &selected_provider)?;
        let target_snapshot = RouteRecoverySnapshot {
            current_provider_id: Some(provider_id.to_string()),
            enabled: true,
            failover_ids: failover_ids.clone(),
        };
        self.persist_operation(
            &previous,
            "enable",
            RouteRecoverySnapshot::from_route(
                &previous,
                self.persistence.list_failovers(profile_id)?,
            ),
            target_snapshot,
            CODEX_ROUTE_RECOVERY_PHASE_PREPARED,
            None,
        )?;
        self.home_config
            .apply_model_catalog_projection_plan(&catalog_plan)?;
        let plan = match self.home_config.build_profile_route_plan(
            home,
            profile.listen_port,
            Some(&selected_provider),
            &token,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                let compensation_error = self
                    .compensate_enable_side_effects(profile_id, None, None, &catalog_plan)
                    .await;
                let _ = self.persist_enable_recovery_error(
                    profile_id,
                    CODEX_ROUTE_RECOVERY_PHASE_PREPARED,
                    &error,
                );
                return Err(Self::compensation_failure_error(
                    &error,
                    compensation_error.as_ref(),
                ));
            }
        };
        let runtime = self.runtime_factory.create(
            CodexProfileScope {
                profile_id: profile.id.clone(),
                home_path: profile.canonical_home_path.clone().into(),
                port: profile.listen_port,
            },
            token,
            snapshot,
        );

        if let Err(error) = runtime.start().await {
            let operation_error = AppError::Message(error);
            let compensation_error = self
                .compensate_enable_side_effects(profile_id, None, None, &catalog_plan)
                .await;
            self.persist_enable_recovery_error(
                profile_id,
                CODEX_ROUTE_RECOVERY_PHASE_PREPARED,
                &operation_error,
            )?;
            return Err(Self::compensation_failure_error(
                &operation_error,
                compensation_error.as_ref(),
            ));
        }
        if let Err(error) =
            self.advance_operation(profile_id, CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED, None)
        {
            let compensation_error = self
                .compensate_enable_side_effects(profile_id, Some(&runtime), None, &catalog_plan)
                .await;
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        if !runtime.health_check().await {
            let error = AppError::Message("Codex Profile 路由健康检查失败".to_string());
            let mut compensation_errors = self
                .compensate_enable_side_effects(profile_id, Some(&runtime), None, &catalog_plan)
                .await
                .into_iter()
                .collect::<Vec<_>>();
            if let Err(persist_error) = self.persist_enable_recovery_error(
                profile_id,
                CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED,
                &error,
            ) {
                compensation_errors.push(persist_error);
            }
            let compensation_error = Self::combine_compensation_errors(compensation_errors);
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        let backup = match self.home_config.serialize_backup(&plan) {
            Ok(backup) => backup,
            Err(error) => {
                let compensation_error = self
                    .compensate_enable_side_effects(profile_id, Some(&runtime), None, &catalog_plan)
                    .await;
                return Err(Self::compensation_failure_error(
                    &error,
                    compensation_error.as_ref(),
                ));
            }
        };
        if let Err(error) = self.persist_enable_backup(profile_id, backup.clone()) {
            let compensation_error = self
                .compensate_enable_side_effects(profile_id, Some(&runtime), None, &catalog_plan)
                .await;
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        if let Err(error) = self.home_config.apply_route_plan(&plan) {
            let mut compensation_errors = self
                .compensate_enable_side_effects(profile_id, Some(&runtime), None, &catalog_plan)
                .await
                .into_iter()
                .collect::<Vec<_>>();
            if let Err(persist_error) = self.persist_enable_recovery_error(
                profile_id,
                CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED,
                &error,
            ) {
                compensation_errors.push(persist_error);
            }
            let compensation_error = Self::combine_compensation_errors(compensation_errors);
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        if let Err(error) = self.advance_operation(
            profile_id,
            CODEX_ROUTE_RECOVERY_PHASE_ENABLE_HOME_APPLIED,
            None,
        ) {
            let compensation_error = self
                .compensate_enable_side_effects(
                    profile_id,
                    Some(&runtime),
                    Some(&plan),
                    &catalog_plan,
                )
                .await;
            if let Some(compensation_error) = &compensation_error {
                let _ = self.persist_operation_error(
                    profile_id,
                    CODEX_ROUTE_RECOVERY_PHASE_ENABLE_HOME_APPLIED,
                    &compensation_error.to_string(),
                );
            }
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        let route = crate::codex_profile::CodexProfileRoute {
            profile_id: profile.id.clone(),
            current_provider_id: Some(provider_id.to_string()),
            enabled: true,
            live_backup_json: Some(backup),
            last_error: None,
            recovery_json: self.operation_json_from_route(profile_id)?,
            updated_at: Utc::now().timestamp_millis(),
        };
        if let Err(error) = self.persistence.save_route(&route) {
            let mut compensation_errors = self
                .compensate_enable_side_effects(
                    profile_id,
                    Some(&runtime),
                    Some(&plan),
                    &catalog_plan,
                )
                .await
                .into_iter()
                .collect::<Vec<_>>();
            if let Err(persistence_error) = self.persist_operation_error(
                profile_id,
                CODEX_ROUTE_RECOVERY_PHASE_ENABLE_PERSIST_FAILED,
                "启用路由持久化失败，拒绝继续变更",
            ) {
                compensation_errors.push(persistence_error);
            }
            let compensation_error = Self::combine_compensation_errors(compensation_errors);
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        self.clear_operation(&route)?;
        self.track_runtime(profile.id, runtime)?;
        Ok(())
    }

    /// 仅替换新请求的供应商快照；持久化失败时恢复旧快照。
    pub async fn switch_provider(
        &self,
        profile_id: &str,
        provider_id: &str,
        failover_ids: Vec<String>,
    ) -> Result<(), AppError> {
        self.switch_provider_internal(profile_id, provider_id, Some(failover_ids), None)
            .await
    }

    /// 切换供应商但保留 Profile 已持久化的故障转移策略。
    pub async fn switch_provider_preserving_failovers(
        &self,
        profile_id: &str,
        provider_id: &str,
    ) -> Result<(), AppError> {
        self.switch_provider_internal(profile_id, provider_id, None, None)
            .await
    }

    /// 使用命令层已合并公共配置的供应商设置执行 Profile 供应商选择。
    pub async fn switch_provider_with_effective_settings(
        &self,
        profile_id: &str,
        provider: Provider,
        failover_ids: Vec<String>,
    ) -> Result<(), AppError> {
        let provider_id = provider.id.clone();
        self.switch_provider_internal(profile_id, &provider_id, Some(failover_ids), Some(provider))
            .await
    }

    /// 使用已合并公共配置的供应商设置切换目标，同时保留现有故障转移策略。
    pub async fn switch_provider_with_effective_settings_preserving_failovers(
        &self,
        profile_id: &str,
        provider: Provider,
    ) -> Result<(), AppError> {
        let provider_id = provider.id.clone();
        self.switch_provider_internal(profile_id, &provider_id, None, Some(provider))
            .await
    }

    /// 在 Profile 锁内按权威路由状态分派直连写入或运行时热切换。
    async fn switch_provider_internal(
        &self,
        profile_id: &str,
        provider_id: &str,
        requested_failover_ids: Option<Vec<String>>,
        effective_provider: Option<Provider>,
    ) -> Result<(), AppError> {
        if provider_id.is_empty() {
            return Err(AppError::InvalidInput("Codex 供应商不能为空".to_string()));
        }
        self.validate_route_operation_before_lock(profile_id)?;
        if let Some(failover_ids) = requested_failover_ids.as_deref() {
            let _ = self.provider_snapshot(provider_id, failover_ids)?;
        }
        let _catalog_guard = self.catalog_sync_lock.lock().await;
        let lock = self.profile_lock(profile_id)?;
        let _guard = lock.lock().await;
        let profile = self.persistence.get_profile(profile_id)?;
        profile.validate_route_operation()?;
        let pending_lifecycle_operation = self
            .persistence
            .get_route(profile_id)?
            .and_then(|route| route.recovery_json)
            .and_then(|json| serde_json::from_str::<RouteRecoveryRecord>(&json).ok())
            .is_some_and(|recovery| recovery.operation != "switch");
        self.recover_pending_locked(profile_id).await?;
        if pending_lifecycle_operation {
            return Err(AppError::InvalidInput(
                "已完成上一次 Codex Profile 路由恢复，请重试供应商切换".to_string(),
            ));
        }
        let failover_ids = self.resolve_failover_ids(profile_id, requested_failover_ids)?;
        let snapshot = self.provider_snapshot(provider_id, &failover_ids)?;
        let route = self
            .persistence
            .get_route(profile_id)?
            .ok_or_else(|| AppError::InvalidInput("Codex Profile 路由不存在".to_string()))?;
        let selected_provider = match effective_provider {
            Some(provider) => provider,
            None => self
                .persistence
                .get_provider(provider_id)?
                .ok_or_else(|| AppError::InvalidInput("Codex 供应商不存在".to_string()))?,
        };
        if !route.enabled {
            return self.switch_direct_provider_locked(
                &profile,
                route,
                selected_provider,
                failover_ids,
            );
        }
        if selected_provider.category.as_deref() == Some("official") {
            return Err(AppError::InvalidInput(
                "官方订阅供应商不能经过 Profile 路由，请先关闭路由".to_string(),
            ));
        }
        let old_failovers = self.persistence.list_failovers(profile_id)?;
        let old_snapshot = self.provider_snapshot(
            route
                .current_provider_id
                .as_deref()
                .ok_or_else(|| AppError::InvalidInput("Codex Profile 未配置供应商".to_string()))?,
            &old_failovers,
        )?;
        let runtime = self.runtime(profile_id)?;
        let changed = crate::codex_profile::CodexProfileRoute {
            current_provider_id: Some(provider_id.to_string()),
            last_error: None,
            updated_at: Utc::now().timestamp_millis(),
            ..route.clone()
        };
        let recovery = RouteRecoveryRecord {
            operation: "switch".to_string(),
            before: RouteRecoverySnapshot::from_route(&route, old_failovers.clone()),
            target: RouteRecoverySnapshot::from_route(&changed, failover_ids.clone()),
            phase: CODEX_ROUTE_RECOVERY_PHASE_PREPARED.to_string(),
            last_error: None,
            reconcile: None,
        };
        let prepared = self.route_with_recovery(route.clone(), &recovery)?;
        let catalog_plan = self.home_config.build_model_catalog_projection_plan(
            std::path::Path::new(&profile.canonical_home_path),
            &selected_provider,
        )?;
        self.home_config
            .apply_model_catalog_projection_plan(&catalog_plan)?;
        if let Err(error) = self.persistence.save_route(&prepared) {
            let compensation_error = self
                .home_config
                .restore_model_catalog_projection_plan(&catalog_plan)
                .err();
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        runtime.swap_provider_snapshot(snapshot).await;
        if let Err(error) = self.advance_operation(
            profile_id,
            CODEX_ROUTE_RECOVERY_PHASE_SWITCH_RUNTIME_SWAPPED,
            None,
        ) {
            let compensation_error = self
                .restore_switch_and_catalog_locked(
                    profile_id,
                    &route,
                    &old_failovers,
                    old_snapshot,
                    &catalog_plan,
                )
                .await;
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        let changing = CodexProfileRoute {
            recovery_json: self.operation_json_from_route(profile_id)?,
            ..changed.clone()
        };
        if let Err(error) = self.persistence.save_route(&changing) {
            let compensation_error = self
                .restore_switch_and_catalog_locked(
                    profile_id,
                    &route,
                    &old_failovers,
                    old_snapshot,
                    &catalog_plan,
                )
                .await;
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        if let Err(error) = self.advance_operation(
            profile_id,
            CODEX_ROUTE_RECOVERY_PHASE_SWITCH_ROUTE_SAVED,
            None,
        ) {
            let compensation_error = self
                .restore_switch_and_catalog_locked(
                    profile_id,
                    &route,
                    &old_failovers,
                    old_snapshot,
                    &catalog_plan,
                )
                .await;
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        if let Err(error) = self
            .persistence
            .replace_failovers(profile_id, &failover_ids)
        {
            let compensation_error = self
                .restore_switch_and_catalog_locked(
                    profile_id,
                    &route,
                    &old_failovers,
                    old_snapshot,
                    &catalog_plan,
                )
                .await;
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        if let Err(error) = self.advance_operation(
            profile_id,
            CODEX_ROUTE_RECOVERY_PHASE_SWITCH_FAILOVERS_SAVED,
            None,
        ) {
            let compensation_error = self
                .restore_switch_and_catalog_locked(
                    profile_id,
                    &route,
                    &old_failovers,
                    old_snapshot,
                    &catalog_plan,
                )
                .await;
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        if let Err(error) = self.clear_operation(&changed) {
            let persistence_error = self
                .persist_operation_error(
                    profile_id,
                    CODEX_ROUTE_RECOVERY_PHASE_SWITCH_FAILOVERS_SAVED,
                    &error.to_string(),
                )
                .err();
            return Err(Self::compensation_failure_error(
                &error,
                persistence_error.as_ref(),
            ));
        }
        Ok(())
    }

    /// 在路由关闭态应用供应商直连配置，并在持久化失败时恢复 Home。
    fn switch_direct_provider_locked(
        &self,
        profile: &CodexProfile,
        route: CodexProfileRoute,
        provider: Provider,
        failover_ids: Vec<String>,
    ) -> Result<(), AppError> {
        let home = std::path::Path::new(&profile.canonical_home_path);
        let plan = self
            .home_config
            .build_direct_provider_plan(home, &provider)?;
        self.home_config.apply_direct_provider_plan(&plan)?;
        let changed = CodexProfileRoute {
            current_provider_id: Some(provider.id),
            enabled: false,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: Utc::now().timestamp_millis(),
            ..route.clone()
        };
        if let Err(error) = self.persistence.save_route(&changed) {
            let compensation_error = self.home_config.restore_direct_provider_plan(&plan).err();
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        if let Err(error) = self
            .persistence
            .replace_failovers(&profile.id, &failover_ids)
        {
            let route_compensation = self.persistence.save_route(&route).err();
            let home_compensation = self.home_config.restore_direct_provider_plan(&plan).err();
            let compensation_error = route_compensation.or(home_compensation);
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        Ok(())
    }

    /// 先恢复 Home，再立即拒绝新请求并停止监听器，最后持久化关闭状态。
    pub async fn disable(&self, profile_id: &str) -> Result<(), AppError> {
        self.validate_route_operation_before_lock(profile_id)?;
        let lock = self.profile_lock(profile_id)?;
        let _guard = lock.lock().await;
        let profile = self.persistence.get_profile(profile_id)?;
        profile.validate_route_operation()?;
        self.recover_pending_locked(profile_id).await?;
        let route = self
            .persistence
            .get_route(profile_id)?
            .ok_or_else(|| AppError::InvalidInput("Codex Profile 路由不存在".to_string()))?;
        self.disable_locked(profile_id, &profile, route).await
    }

    /// 使用既有 listener token 收敛关闭中的 Profile Home，不创建新凭证。
    fn restore_profile_home_for_disable(
        &self,
        profile: &CodexProfile,
        route: &CodexProfileRoute,
    ) -> Result<(), AppError> {
        let Some(backup) = route.live_backup_json.as_deref() else {
            return Ok(());
        };
        let listener_token = self.secret_store.read_token(&profile.id)?.ok_or_else(|| {
            AppError::Config("Codex Profile 本地路由凭证缺失，无法证明 Home 路由所有权".to_string())
        })?;
        self.home_config.restore_profile_backup(
            std::path::Path::new(&profile.canonical_home_path),
            backup,
            profile.listen_port,
            &listener_token,
        )
    }

    /// 在已持有 Profile 锁时执行可重试的关闭流程。
    async fn disable_locked(
        &self,
        profile_id: &str,
        profile: &crate::codex_profile::CodexProfile,
        route: crate::codex_profile::CodexProfileRoute,
    ) -> Result<(), AppError> {
        let failovers = self.persistence.list_failovers(profile_id)?;
        self.persist_operation(
            &route,
            "disable",
            RouteRecoverySnapshot::from_route(&route, failovers.clone()),
            RouteRecoverySnapshot {
                current_provider_id: route.current_provider_id.clone(),
                enabled: false,
                failover_ids: failovers,
            },
            CODEX_ROUTE_RECOVERY_PHASE_PREPARED,
            None,
        )?;
        if let Err(error) = self.restore_profile_home_for_disable(profile, &route) {
            self.persist_operation_error(
                profile_id,
                CODEX_ROUTE_RECOVERY_PHASE_DISABLE_HOME_RESTORE_FAILED,
                &error.to_string(),
            )?;
            return Err(error);
        }
        if let Ok(runtime) = self.runtime(profile_id) {
            if let Err(error) = Self::reject_new_requests_and_stop_runtime(runtime).await {
                self.persist_operation_error(
                    profile_id,
                    CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOP_FAILED,
                    &error,
                )?;
                return Err(AppError::Message(format!(
                    "Codex Profile 路由停止失败: {error}"
                )));
            }
        }
        self.advance_operation(profile_id, CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOPPED, None)?;
        let disabled = crate::codex_profile::CodexProfileRoute {
            enabled: false,
            last_error: None,
            recovery_json: self.operation_json_from_route(profile_id)?,
            updated_at: Utc::now().timestamp_millis(),
            ..route
        };
        if let Err(error) = self.persistence.save_route(&disabled) {
            self.persist_operation_error(
                profile_id,
                CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOPPED,
                &error.to_string(),
            )?;
            return Err(error);
        }
        self.clear_operation(&disabled)?;
        self.remove_runtime(profile_id)?;
        Ok(())
    }

    /// 删除自定义 Profile 的数据库关系与私有 token，绝不删除其 Home。
    pub async fn delete_custom_profile(&self, profile_id: &str) -> Result<(), AppError> {
        self.validate_delete_before_lock(profile_id)?;
        let lock = self.profile_lock(profile_id)?;
        let _guard = lock.lock().await;
        let profile = self.persistence.get_profile(profile_id)?;
        profile.validate_delete()?;
        self.recover_pending_locked(profile_id).await?;
        if let Some(route) = self.persistence.get_route(profile_id)? {
            if route.enabled {
                self.disable_locked(profile_id, &profile, route).await?;
            }
        }
        let route = self
            .persistence
            .get_route(profile_id)?
            .ok_or_else(|| AppError::InvalidInput("Codex Profile 路由不存在".to_string()))?;
        let snapshot =
            RouteRecoverySnapshot::from_route(&route, self.persistence.list_failovers(profile_id)?);
        self.persist_operation(
            &route,
            "delete",
            snapshot.clone(),
            snapshot,
            CODEX_ROUTE_RECOVERY_PHASE_DELETE_TOKEN_PENDING,
            None,
        )?;
        if let Err(error) = self.secret_store.delete_token(profile_id) {
            self.persist_operation_error(
                profile_id,
                CODEX_ROUTE_RECOVERY_PHASE_DELETE_TOKEN_PENDING,
                &error.to_string(),
            )?;
            return Err(error);
        }
        self.advance_operation(
            profile_id,
            CODEX_ROUTE_RECOVERY_PHASE_DELETE_DATABASE_FAILED,
            None,
        )?;
        if let Err(error) = self.persistence.delete_profile(profile_id) {
            self.persist_operation_error(
                profile_id,
                CODEX_ROUTE_RECOVERY_PHASE_DELETE_DATABASE_FAILED,
                &error.to_string(),
            )?;
            return Err(error);
        }
        Ok(())
    }

    /// 启动时逐个对账所有 Profile 的模型目录；单个文件失败不会阻断后续项。
    pub async fn reconcile_all_profile_catalogs(&self) -> Result<(), AppError> {
        let _catalog_guard = self.catalog_sync_lock.lock().await;
        let mut profiles = self.persistence.list_profiles()?;
        profiles.sort_by(|left, right| left.id.cmp(&right.id));

        for profile in profiles {
            let profile_lock = self.profile_lock(&profile.id)?;
            let _profile_guard = profile_lock.lock().await;
            let Some(mut route) = self.persistence.get_route(&profile.id)? else {
                continue;
            };
            let Some(provider_id) = route.current_provider_id.as_deref() else {
                continue;
            };

            let reconcile_result = (|| {
                let provider = self.persistence.get_provider(provider_id)?.ok_or_else(|| {
                    AppError::InvalidInput(format!("Codex 供应商不存在: {provider_id}"))
                })?;
                let plan = self.home_config.build_model_catalog_projection_plan(
                    std::path::Path::new(&profile.canonical_home_path),
                    &provider,
                )?;
                self.home_config.apply_model_catalog_projection_plan(&plan)
            })();

            match reconcile_result {
                Ok(()) => {
                    if route.last_error.as_deref().is_some_and(|error| {
                        error.starts_with(CODEX_CATALOG_RECONCILE_ERROR_PREFIX)
                    }) {
                        route.last_error = None;
                        if let Err(save_error) = self.persistence.save_route(&route) {
                            log::warn!(
                                "清理 Codex Profile {} 模型目录错误失败: {}",
                                profile.id,
                                save_error
                            );
                        }
                    }
                }
                Err(error) => {
                    route.last_error =
                        Some(format!("{CODEX_CATALOG_RECONCILE_ERROR_PREFIX}: {error}"));
                    if let Err(save_error) = self.persistence.save_route(&route) {
                        log::warn!(
                            "记录 Codex Profile {} 模型目录错误失败: {}",
                            profile.id,
                            save_error
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// 启动时逐个恢复已启用 Profile；单个失败会被隔离并记录。
    pub async fn restore_enabled_profiles(&self) -> Result<(), AppError> {
        for profile in self.persistence.list_profiles()? {
            let result = async {
                let lock = self.profile_lock(&profile.id)?;
                let _guard = lock.lock().await;
                let profile = self.persistence.get_profile(&profile.id)?;
                profile.validate_route_operation()?;
                self.recover_pending_locked(&profile.id).await?;
                let Some(route) = self.persistence.get_route(&profile.id)? else {
                    return Ok(());
                };
                if !route.enabled {
                    return Ok(());
                }
                let token = self.secret_store.ensure_token(&profile.id)?;
                let (snapshot, plan) = self.build_restore_plan(&profile, &route, &token)?;
                self.reconcile_enabled_home(&profile, &route, &plan)?;
                self.start_restored_runtime(&profile, token, snapshot).await
            }
            .await;
            if let Err(error) = result {
                if let Err(persist_error) =
                    self.persist_error_summary(&profile.id, "恢复失败", false)
                {
                    log::warn!(
                        "记录 Codex Profile {} 恢复错误失败: {}",
                        profile.id,
                        persist_error
                    );
                }
                log::warn!("恢复 Codex Profile {} 失败: {}", profile.id, error);
            }
        }
        Ok(())
    }

    /// 用同一份 listener token 构造 Home 目标与运行时供应商快照。
    fn build_restore_plan(
        &self,
        profile: &CodexProfile,
        route: &CodexProfileRoute,
        listener_token: &str,
    ) -> Result<(CodexRouteProviderSnapshot, CodexRouteConfigPlan), AppError> {
        let provider_id = route
            .current_provider_id
            .as_deref()
            .ok_or_else(|| AppError::InvalidInput("Codex Profile 未配置供应商".to_string()))?;
        let provider = self
            .persistence
            .get_provider(provider_id)?
            .ok_or_else(|| AppError::InvalidInput(format!("Codex 供应商不存在: {provider_id}")))?;
        let snapshot =
            self.provider_snapshot(provider_id, &self.persistence.list_failovers(&profile.id)?)?;
        let plan = self.home_config.build_profile_route_plan(
            std::path::Path::new(&profile.canonical_home_path),
            profile.listen_port,
            Some(&provider),
            listener_token,
        )?;
        Ok((snapshot, plan))
    }

    /// 在启动监听器前收敛 Home 所有权，外部编辑不会被覆盖。
    fn reconcile_enabled_home(
        &self,
        profile: &CodexProfile,
        route: &CodexProfileRoute,
        plan: &CodexRouteConfigPlan,
    ) -> Result<(), AppError> {
        let backup = route.live_backup_json.as_deref().ok_or_else(|| {
            AppError::InvalidInput("已启用 Codex Profile 缺少 Home 备份".to_string())
        })?;
        match self
            .home_config
            .classify_profile_reconcile(plan, backup, profile.listen_port)?
        {
            CodexHomeReconcileOwnership::Current => {
                self.finalize_current_home_backup(route, backup, plan.target_fingerprint())
            }
            CodexHomeReconcileOwnership::RouteOwned
            | CodexHomeReconcileOwnership::LegacyManaged => {
                self.apply_profile_reconcile(route, backup, plan)
            }
        }
    }

    /// Home 已正确时仅在 target 指纹变化后保存重定位的备份。
    fn finalize_current_home_backup(
        &self,
        route: &CodexProfileRoute,
        backup: &str,
        target_fingerprint: &str,
    ) -> Result<(), AppError> {
        let rebased = self
            .home_config
            .rebase_route_backup(backup, target_fingerprint)?;
        if rebased == backup {
            return Ok(());
        }
        self.persistence.save_route(&CodexProfileRoute {
            live_backup_json: Some(rebased),
            last_error: None,
            updated_at: Utc::now().timestamp_millis(),
            ..route.clone()
        })
    }

    /// 先记录指纹转换，再应用 Home，最后原子提交新备份目标；失败时恢复旧目标。
    fn apply_profile_reconcile(
        &self,
        route: &CodexProfileRoute,
        backup: &str,
        plan: &CodexRouteConfigPlan,
    ) -> Result<(), AppError> {
        self.persist_reconcile_operation(route, plan)?;
        self.home_config.apply_route_plan(plan)?;
        if let Err(error) = self.advance_operation(
            &route.profile_id,
            CODEX_ROUTE_RECOVERY_PHASE_RECONCILE_HOME_APPLIED,
            None,
        ) {
            return Err(self.compensate_reconcile_failure(plan, &error));
        }
        let rebased = match self
            .home_config
            .rebase_route_backup(backup, plan.target_fingerprint())
        {
            Ok(rebased) => rebased,
            Err(error) => return Err(self.compensate_reconcile_failure(plan, &error)),
        };
        let finalized = CodexProfileRoute {
            live_backup_json: Some(rebased),
            recovery_json: None,
            last_error: None,
            updated_at: Utc::now().timestamp_millis(),
            ..route.clone()
        };
        if let Err(error) = self.persistence.save_route(&finalized) {
            return Err(self.compensate_reconcile_failure(plan, &error));
        }
        Ok(())
    }

    /// 保存不含 token 和正文的启动对账记录。
    fn persist_reconcile_operation(
        &self,
        route: &CodexProfileRoute,
        plan: &CodexRouteConfigPlan,
    ) -> Result<(), AppError> {
        let snapshot = RouteRecoverySnapshot::from_route(
            route,
            self.persistence.list_failovers(&route.profile_id)?,
        );
        let recovery = RouteRecoveryRecord {
            operation: CODEX_ROUTE_RECOVERY_OPERATION_RECONCILE.to_string(),
            before: snapshot.clone(),
            target: snapshot,
            phase: CODEX_ROUTE_RECOVERY_PHASE_RECONCILE_PREPARED.to_string(),
            last_error: None,
            reconcile: Some(RouteReconcileTransition {
                old_target_fingerprint: plan.previous_fingerprint().to_string(),
                new_target_fingerprint: plan.target_fingerprint().to_string(),
            }),
        };
        self.persistence
            .save_route(&self.route_with_recovery(route.clone(), &recovery)?)
    }

    /// 对账持久化失败时恢复写入前 Home，并返回包含补偿结果的错误。
    fn compensate_reconcile_failure(
        &self,
        plan: &CodexRouteConfigPlan,
        operation_error: &AppError,
    ) -> AppError {
        let compensation_error = self.home_config.restore(plan).err();
        Self::compensation_failure_error(operation_error, compensation_error.as_ref())
    }

    /// Home 已收敛后启动并健康检查单个 Profile runtime。
    async fn start_restored_runtime(
        &self,
        profile: &CodexProfile,
        listener_token: String,
        snapshot: CodexRouteProviderSnapshot,
    ) -> Result<(), AppError> {
        let runtime = self.runtime_factory.create(
            CodexProfileScope {
                profile_id: profile.id.clone(),
                home_path: profile.canonical_home_path.clone().into(),
                port: profile.listen_port,
            },
            listener_token,
            snapshot,
        );
        runtime.start().await.map_err(AppError::Message)?;
        if !runtime.health_check().await {
            let stop_failed = Self::reject_new_requests_and_stop_runtime(runtime.clone())
                .await
                .is_err();
            self.persist_error_summary(&profile.id, "恢复健康检查失败", stop_failed)?;
            return Err(AppError::Message(
                "Codex Profile 路由健康检查失败".to_string(),
            ));
        }
        self.track_runtime(profile.id.clone(), runtime)
    }

    /// 返回 Profile 当前运行状态。
    pub async fn status(&self, profile_id: &str) -> Result<CodexRuntimeStatus, AppError> {
        Ok(self.runtime(profile_id)?.status().await)
    }

    /// 在同一把 Profile 生命周期锁内读取权威状态并执行元数据修改。
    pub async fn with_profile_metadata_lock<T, F>(
        &self,
        profile_id: &str,
        mutation: F,
    ) -> Result<T, AppError>
    where
        F: FnOnce(CodexRuntimeStatus) -> Result<T, AppError>,
    {
        let lock = self.profile_lock(profile_id)?;
        let _guard = lock.lock().await;
        let _ = self.persistence.get_profile(profile_id)?;
        self.recover_pending_locked(profile_id).await?;
        let route_enabled = self
            .persistence
            .get_route(profile_id)?
            .is_some_and(|route| route.enabled);
        let runtime_status = self.runtime_status_or_stopped(profile_id).await?;
        let effective_status = if route_enabled && !runtime_status.is_active() {
            CodexRuntimeStatus::Running
        } else {
            runtime_status
        };

        mutation(effective_status)
    }

    /// 未显式提交新策略时读取 Profile 当前故障转移列表。
    fn resolve_failover_ids(
        &self,
        profile_id: &str,
        requested_failover_ids: Option<Vec<String>>,
    ) -> Result<Vec<String>, AppError> {
        match requested_failover_ids {
            Some(failover_ids) => Ok(failover_ids),
            None => self.persistence.list_failovers(profile_id),
        }
    }

    fn provider_snapshot(
        &self,
        provider_id: &str,
        failover_ids: &[String],
    ) -> Result<CodexRouteProviderSnapshot, AppError> {
        let primary = self
            .persistence
            .get_provider(provider_id)?
            .ok_or_else(|| AppError::InvalidInput(format!("Codex 供应商不存在: {provider_id}")))?;
        let failovers = failover_ids
            .iter()
            .map(|id| {
                self.persistence
                    .get_provider(id)?
                    .ok_or_else(|| AppError::InvalidInput(format!("Codex 供应商不存在: {id}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CodexRouteProviderSnapshot::new(primary, failovers))
    }

    /// 读取已托管 runtime 的状态；没有 runtime 时返回关闭态，不吞掉锁错误。
    async fn runtime_status_or_stopped(
        &self,
        profile_id: &str,
    ) -> Result<CodexRuntimeStatus, AppError> {
        let runtime = self
            .runtimes
            .lock()
            .map_err(|error| AppError::Lock(error.to_string()))?
            .get(profile_id)
            .cloned();
        match runtime {
            Some(runtime) => Ok(runtime.status().await),
            None => Ok(CodexRuntimeStatus::Stopped),
        }
    }

    fn profile_lock(&self, profile_id: &str) -> Result<Arc<AsyncMutex<()>>, AppError> {
        Ok(self
            .locks
            .lock()
            .map_err(|e| AppError::Lock(e.to_string()))?
            .entry(profile_id.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone())
    }

    /// 在获取 Profile 锁前执行只读合法性校验，避免无效请求进入串行队列。
    fn validate_route_operation_before_lock(&self, profile_id: &str) -> Result<(), AppError> {
        self.persistence
            .get_profile(profile_id)?
            .validate_route_operation()?;
        let _ = self.persistence.get_route(profile_id)?;
        Ok(())
    }

    /// 在获取 Profile 锁前校验删除资格与当前路由读取能力。
    fn validate_delete_before_lock(&self, profile_id: &str) -> Result<(), AppError> {
        self.persistence
            .get_profile(profile_id)?
            .validate_delete()?;
        let _ = self.persistence.get_route(profile_id)?;
        Ok(())
    }
    fn runtime(&self, profile_id: &str) -> Result<Arc<dyn CodexRouteRuntime>, AppError> {
        self.runtimes
            .lock()
            .map_err(|e| AppError::Lock(e.to_string()))?
            .get(profile_id)
            .cloned()
            .ok_or_else(|| AppError::InvalidInput(format!("Codex Profile 未运行: {profile_id}")))
    }

    /// 将尚未成功停止的运行时纳入 manager，避免孤儿监听器被误判为已完成。
    fn track_runtime(
        &self,
        profile_id: String,
        runtime: Arc<dyn CodexRouteRuntime>,
    ) -> Result<(), AppError> {
        self.runtimes
            .lock()
            .map_err(|error| AppError::Lock(error.to_string()))?
            .insert(profile_id, runtime);
        Ok(())
    }

    /// 在停止和状态持久化都成功后移除 manager 对运行时的托管。
    fn remove_runtime(&self, profile_id: &str) -> Result<(), AppError> {
        self.runtimes
            .lock()
            .map_err(|error| AppError::Lock(error.to_string()))?
            .remove(profile_id);
        Ok(())
    }

    /// 立即拒绝新请求并停止监听器，不等待也不取消在途路由请求。
    ///
    /// 历史实现会等待在途请求排空至多 15 秒，并在超时后继续停止；用户显式关闭、
    /// 恢复和补偿现在统一采用即时停接。
    // 历史说明：停止监听器前先拒绝新请求并等待在途请求排空，超时状态会写入停止错误以便观测。
    async fn reject_new_requests_and_stop_runtime(
        runtime: Arc<dyn CodexRouteRuntime>,
    ) -> Result<(), String> {
        runtime.reject_new_requests().await;
        runtime.stop().await
    }

    /// 返回原始失败及补偿失败，避免调用方将未收敛状态误判为普通业务失败。
    fn compensation_failure_error(
        operation_error: &AppError,
        compensation_error: Option<&AppError>,
    ) -> AppError {
        match compensation_error {
            Some(compensation_error) => AppError::Message(format!(
                "{CODEX_ROUTE_COMPENSATION_UNCONVERGED_ERROR}: 原始错误: {operation_error}; 补偿错误: {compensation_error}"
            )),
            None => AppError::Message(operation_error.to_string()),
        }
    }

    /// 合并多个补偿失败，同时保留所有已执行补偿的脱敏错误。
    fn combine_compensation_errors(errors: Vec<AppError>) -> Option<AppError> {
        if errors.is_empty() {
            None
        } else {
            Some(AppError::Message(
                errors
                    .into_iter()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("；"),
            ))
        }
    }

    /// 恢复启用流程已经产生的 Home、目录与运行时副作用。
    async fn compensate_enable_side_effects(
        &self,
        profile_id: &str,
        runtime: Option<&Arc<dyn CodexRouteRuntime>>,
        route_plan: Option<&CodexRouteConfigPlan>,
        catalog_plan: &CodexModelCatalogProjectionPlan,
    ) -> Option<AppError> {
        let mut errors = Vec::new();
        if let Some(route_plan) = route_plan {
            if let Err(error) = self.home_config.restore(route_plan) {
                errors.push(error);
            }
        }
        if let Err(error) = self
            .home_config
            .restore_model_catalog_projection_plan(catalog_plan)
        {
            errors.push(error);
        }
        if let Some(runtime) = runtime {
            if let Err(error) = Self::reject_new_requests_and_stop_runtime(runtime.clone()).await {
                errors.push(AppError::Message(error));
                if let Err(error) = self.track_runtime(profile_id.to_string(), runtime.clone()) {
                    errors.push(error);
                }
            }
        }
        Self::combine_compensation_errors(errors)
    }

    /// 同时恢复已启用切换的运行时/数据库状态和目标 Home 模型目录。
    async fn restore_switch_and_catalog_locked(
        &self,
        profile_id: &str,
        route: &CodexProfileRoute,
        failovers: &[String],
        old_snapshot: CodexRouteProviderSnapshot,
        catalog_plan: &CodexModelCatalogProjectionPlan,
    ) -> Option<AppError> {
        let mut errors = Vec::new();
        if let Err(error) = self
            .restore_switch_locked(profile_id, route, failovers, old_snapshot)
            .await
        {
            errors.push(error);
        }
        if let Err(error) = self
            .home_config
            .restore_model_catalog_projection_plan(catalog_plan)
        {
            errors.push(error);
        }
        Self::combine_compensation_errors(errors)
    }

    /// 将恢复前置条件失败标记为未收敛，禁止调用方继续新的生命周期变更。
    fn recovery_unconverged_error(error: AppError) -> AppError {
        AppError::Message(format!(
            "{CODEX_ROUTE_COMPENSATION_UNCONVERGED_ERROR}: 无法安全恢复未完成的路由变更: {error}"
        ))
    }

    /// 创建未启用 Profile 的最小路由状态，供首次启用持久化操作记录。
    fn empty_route(profile_id: &str) -> CodexProfileRoute {
        CodexProfileRoute {
            profile_id: profile_id.to_string(),
            current_provider_id: None,
            enabled: false,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: Utc::now().timestamp_millis(),
        }
    }

    /// 在不可逆步骤前写入唯一且无敏感信息的操作记录。
    fn persist_operation(
        &self,
        route: &CodexProfileRoute,
        operation: &str,
        before: RouteRecoverySnapshot,
        target: RouteRecoverySnapshot,
        phase: &str,
        last_error: Option<String>,
    ) -> Result<(), AppError> {
        let recovery = RouteRecoveryRecord {
            operation: operation.to_string(),
            before,
            target,
            phase: phase.to_string(),
            last_error,
            reconcile: None,
        };
        let mut pending = self.route_with_recovery(route.clone(), &recovery)?;
        pending.last_error = recovery.last_error.clone();
        self.persistence.save_route(&pending)
    }

    /// 推进已存在操作记录的阶段，阶段写入失败时不执行后续副作用。
    fn advance_operation(
        &self,
        profile_id: &str,
        phase: &str,
        last_error: Option<String>,
    ) -> Result<(), AppError> {
        let route = self
            .persistence
            .get_route(profile_id)?
            .ok_or_else(|| AppError::InvalidInput("Codex Profile 路由不存在".to_string()))?;
        let json = route.recovery_json.as_deref().ok_or_else(|| {
            AppError::InvalidInput("Codex Profile 缺少生命周期操作记录".to_string())
        })?;
        let mut recovery: RouteRecoveryRecord = serde_json::from_str(json)
            .map_err(|_| AppError::InvalidInput("Codex Profile 路由补偿记录无效".to_string()))?;
        recovery.phase = phase.to_string();
        recovery.last_error = last_error.clone();
        let mut advanced = self.route_with_recovery(route, &recovery)?;
        advanced.last_error = last_error;
        self.persistence.save_route(&advanced)
    }

    /// 记录操作失败原因；内容仅来自运行时错误文本，不写入认证或 Home 正文。
    fn persist_operation_error(
        &self,
        profile_id: &str,
        phase: &str,
        error: &str,
    ) -> Result<(), AppError> {
        self.advance_operation(profile_id, phase, Some(Self::safe_error_summary(error)))
    }

    /// 将错误归纳为不可逆操作可安全持久化的摘要，禁止写入凭证、认证字段或本地路径。
    fn safe_error_summary(error: &str) -> String {
        let lower = error.to_ascii_lowercase();
        if lower.contains("token")
            || lower.contains("auth")
            || error.contains('/')
            || error.contains('\\')
        {
            "CODEX_PROFILE_OPERATION_SENSITIVE_FAILURE".to_string()
        } else {
            "CODEX_PROFILE_OPERATION_FAILURE".to_string()
        }
    }

    /// 读取当前唯一操作记录，以便最终路由状态保留同一记录直至清理。
    fn operation_json_from_route(&self, profile_id: &str) -> Result<Option<String>, AppError> {
        Ok(self
            .persistence
            .get_route(profile_id)?
            .and_then(|route| route.recovery_json))
    }

    /// 成功收敛后清除唯一操作记录及临时错误。
    fn clear_operation(&self, route: &CodexProfileRoute) -> Result<(), AppError> {
        self.persistence.save_route(&CodexProfileRoute {
            recovery_json: None,
            last_error: None,
            updated_at: Utc::now().timestamp_millis(),
            ..route.clone()
        })
    }

    /// 在改写 Home 前保存其可恢复备份，确保最终路由保存失败后仍能在后续操作中收敛。
    fn persist_enable_backup(&self, profile_id: &str, backup: String) -> Result<(), AppError> {
        let route = self
            .persistence
            .get_route(profile_id)?
            .ok_or_else(|| AppError::InvalidInput("Codex Profile 路由不存在".to_string()))?;
        self.persistence.save_route(&CodexProfileRoute {
            live_backup_json: Some(backup),
            updated_at: Utc::now().timestamp_millis(),
            ..route
        })
    }

    /// 持久化不含凭证的失败摘要，并明确标注是否仍有回滚动作待处理。
    fn persist_error_summary(
        &self,
        profile_id: &str,
        operation: &str,
        rollback_failed: bool,
    ) -> Result<(), AppError> {
        if let Some(route) = self.persistence.get_route(profile_id)? {
            let suffix = if rollback_failed {
                "；回滚未完全完成，请重试"
            } else {
                ""
            };
            self.persistence
                .save_route(&crate::codex_profile::CodexProfileRoute {
                    last_error: Some(format!("Codex Profile {operation}{suffix}")),
                    updated_at: Utc::now().timestamp_millis(),
                    ..route
                })?;
        }
        Ok(())
    }

    /// 在锁内恢复上次未完成的切换；失败时保留记录并拒绝后续生命周期变更。
    async fn recover_pending_locked(&self, profile_id: &str) -> Result<(), AppError> {
        let Some(route) = self
            .persistence
            .get_route(profile_id)
            .map_err(Self::recovery_unconverged_error)?
        else {
            return Ok(());
        };
        let Some(recovery_json) = route.recovery_json.clone() else {
            return Ok(());
        };
        let recovery: RouteRecoveryRecord = serde_json::from_str(&recovery_json).map_err(|_| {
            Self::recovery_unconverged_error(AppError::InvalidInput(
                "Codex Profile 路由补偿记录无效，拒绝继续变更".to_string(),
            ))
        })?;
        self.validate_recovery_record(&recovery).map_err(|_| {
            Self::recovery_unconverged_error(AppError::InvalidInput(
                "Codex Profile 路由补偿记录非法，拒绝继续变更".to_string(),
            ))
        })?;
        if recovery.operation == CODEX_ROUTE_RECOVERY_OPERATION_RECONCILE {
            return self.recover_reconcile_locked(profile_id, route, &recovery);
        }
        if recovery.operation == "enable" {
            return self
                .recover_enable_locked(profile_id, route, recovery, recovery_json)
                .await;
        }
        if recovery.operation == "delete" {
            self.secret_store.ensure_token(profile_id)?;
            let mut recovered = route;
            recovered.recovery_json = None;
            recovered.last_error = None;
            recovered.updated_at = Utc::now().timestamp_millis();
            return self.persistence.save_route(&recovered);
        }
        if Self::is_disable_recovery(&recovery.operation, &recovery.phase) {
            let profile = self.persistence.get_profile(profile_id)?;
            if let Err(error) = self.restore_profile_home_for_disable(&profile, &route) {
                self.persist_operation_error(
                    profile_id,
                    CODEX_ROUTE_RECOVERY_PHASE_DISABLE_HOME_RESTORE_FAILED,
                    &error.to_string(),
                )?;
                return Err(Self::recovery_unconverged_error(error));
            }
        }
        if Self::is_disable_recovery(&recovery.operation, &recovery.phase) {
            if let Ok(runtime) = self.runtime(profile_id) {
                if let Err(error) = Self::reject_new_requests_and_stop_runtime(runtime).await {
                    self.persist_operation_error(
                        profile_id,
                        CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOP_FAILED,
                        &error,
                    )?;
                    return Err(AppError::Message(format!(
                        "继续关闭 Codex Profile 路由失败: {error}"
                    )));
                }
            }
            let mut disabled = route;
            disabled.enabled = false;
            disabled.recovery_json = None;
            disabled.last_error = None;
            disabled.updated_at = Utc::now().timestamp_millis();
            self.persistence.save_route(&disabled)?;
            self.runtimes
                .lock()
                .map_err(|e| AppError::Lock(e.to_string()))?
                .remove(profile_id);
            return Ok(());
        }
        let original = recovery.before;
        let provider_id = original.current_provider_id.as_deref().ok_or_else(|| {
            Self::recovery_unconverged_error(AppError::InvalidInput(
                "补偿记录缺少原始供应商".to_string(),
            ))
        })?;
        let snapshot = self
            .provider_snapshot(provider_id, &original.failover_ids)
            .map_err(Self::recovery_unconverged_error)?;
        let runtime = self
            .runtime(profile_id)
            .map_err(Self::recovery_unconverged_error)?;
        runtime.swap_provider_snapshot(snapshot).await;
        let mut restored = route;
        restored.current_provider_id = original.current_provider_id.clone();
        restored.enabled = original.enabled;
        restored.recovery_json = Some(recovery_json);
        restored.last_error = Some("正在恢复未完成的路由变更".to_string());
        self.persistence.save_route(&restored)?;
        self.persistence
            .replace_failovers(profile_id, &original.failover_ids)?;
        restored.recovery_json = None;
        restored.last_error = None;
        restored.updated_at = Utc::now().timestamp_millis();
        self.persistence.save_route(&restored)
    }

    /// 根据 Home 当前指纹收敛崩溃中的启动对账，不需要读取或保存 listener token。
    fn recover_reconcile_locked(
        &self,
        profile_id: &str,
        route: CodexProfileRoute,
        recovery: &RouteRecoveryRecord,
    ) -> Result<(), AppError> {
        let transition = recovery.reconcile.as_ref().ok_or_else(|| {
            Self::recovery_unconverged_error(AppError::InvalidInput(
                "启动对账记录缺少指纹转换".to_string(),
            ))
        })?;
        let profile = self.persistence.get_profile(profile_id)?;
        let current = self
            .home_config
            .current_fingerprint(std::path::Path::new(&profile.canonical_home_path))?;
        let mut recovered = route;
        if current == transition.new_target_fingerprint {
            let backup = recovered.live_backup_json.as_deref().ok_or_else(|| {
                Self::recovery_unconverged_error(AppError::InvalidInput(
                    "启动对账缺少 Home 备份".to_string(),
                ))
            })?;
            recovered.live_backup_json = Some(
                self.home_config
                    .rebase_route_backup(backup, &transition.new_target_fingerprint)?,
            );
        } else if current != transition.old_target_fingerprint {
            self.persist_operation_error(profile_id, &recovery.phase, "启动对账 Home 指纹冲突")?;
            return Err(Self::recovery_unconverged_error(AppError::Message(
                "启动对账检测到外部 Home 修改".to_string(),
            )));
        }
        recovered.recovery_json = None;
        recovered.last_error = None;
        recovered.updated_at = Utc::now().timestamp_millis();
        self.persistence.save_route(&recovered)
    }

    /// 收敛未完成的启用：恢复 Home、停止已托管运行时，再回写启用前的路由快照。
    async fn recover_enable_locked(
        &self,
        profile_id: &str,
        route: CodexProfileRoute,
        recovery: RouteRecoveryRecord,
        recovery_json: String,
    ) -> Result<(), AppError> {
        if Self::enable_phase_requires_home_restore(&recovery.phase) {
            let Some(backup) = route.live_backup_json.as_deref() else {
                let error = AppError::Message("启用残留缺少 Home 备份".to_string());
                self.persist_enable_recovery_error(profile_id, &recovery.phase, &error)?;
                return Err(AppError::Message(format!(
                    "{CODEX_ROUTE_COMPENSATION_UNCONVERGED_ERROR}: 启用残留缺少 Home 备份"
                )));
            };
            let profile = self.persistence.get_profile(profile_id)?;
            if let Err(error) = self
                .home_config
                .restore_backup(std::path::Path::new(&profile.canonical_home_path), backup)
            {
                self.persist_enable_recovery_error(profile_id, &recovery.phase, &error)?;
                return Err(AppError::Message(format!(
                    "{CODEX_ROUTE_COMPENSATION_UNCONVERGED_ERROR}: 恢复 Codex Home 失败: {error}"
                )));
            }
        }
        if let Ok(runtime) = self.runtime(profile_id) {
            if let Err(error) = Self::reject_new_requests_and_stop_runtime(runtime).await {
                self.persist_enable_recovery_error(
                    profile_id,
                    &recovery.phase,
                    &AppError::Message(error.clone()),
                )?;
                return Err(AppError::Message(format!(
                    "{CODEX_ROUTE_COMPENSATION_UNCONVERGED_ERROR}: 停止 Codex Profile 路由失败: {error}"
                )));
            }
        }
        let mut restored = route;
        restored.current_provider_id = recovery.before.current_provider_id;
        restored.enabled = recovery.before.enabled;
        restored.live_backup_json = None;
        restored.recovery_json = Some(recovery_json);
        restored.last_error = Some("正在恢复未完成的启用".to_string());
        restored.updated_at = Utc::now().timestamp_millis();
        self.persistence.save_route(&restored)?;
        self.persistence
            .replace_failovers(profile_id, &recovery.before.failover_ids)?;
        self.clear_operation(&restored)?;
        self.remove_runtime(profile_id)
    }

    /// 判断启用阶段是否已经可能改写 Home，只有这些阶段才会尝试使用备份恢复。
    fn enable_phase_requires_home_restore(phase: &str) -> bool {
        matches!(
            phase,
            CODEX_ROUTE_RECOVERY_PHASE_ENABLE_HOME_APPLIED
                | CODEX_ROUTE_RECOVERY_PHASE_ENABLE_PERSIST_FAILED
        )
    }

    /// 判断恢复记录是否属于必须先恢复 Home 再收敛关闭状态的关闭流程。
    fn is_disable_recovery(operation: &str, phase: &str) -> bool {
        operation == "disable"
            && matches!(
                phase,
                CODEX_ROUTE_RECOVERY_PHASE_PREPARED
                    | CODEX_ROUTE_RECOVERY_PHASE_DISABLE_HOME_RESTORE_FAILED
                    | CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOP_FAILED
                    | CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOPPED
            )
    }

    /// Home 或运行时补偿失败时保留操作记录及经过脱敏的错误摘要。
    fn persist_enable_recovery_error(
        &self,
        profile_id: &str,
        phase: &str,
        error: &AppError,
    ) -> Result<(), AppError> {
        self.persist_operation_error(profile_id, phase, &error.to_string())
    }

    /// 校验恢复操作及阶段的合法组合；未知组合绝不允许执行副作用。
    fn validate_recovery_record(&self, recovery: &RouteRecoveryRecord) -> Result<(), AppError> {
        let valid = matches!(
            (recovery.operation.as_str(), recovery.phase.as_str()),
            ("enable", CODEX_ROUTE_RECOVERY_PHASE_PREPARED)
                | ("enable", CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED)
                | ("enable", CODEX_ROUTE_RECOVERY_PHASE_ENABLE_HOME_APPLIED)
                | ("enable", CODEX_ROUTE_RECOVERY_PHASE_ENABLE_PERSIST_FAILED)
                | ("disable", CODEX_ROUTE_RECOVERY_PHASE_PREPARED)
                | (
                    "disable",
                    CODEX_ROUTE_RECOVERY_PHASE_DISABLE_HOME_RESTORE_FAILED
                )
                | ("disable", CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOP_FAILED)
                | ("disable", CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOPPED)
                | ("delete", CODEX_ROUTE_RECOVERY_PHASE_DELETE_TOKEN_PENDING)
                | ("delete", CODEX_ROUTE_RECOVERY_PHASE_DELETE_DATABASE_FAILED)
                | ("switch", CODEX_ROUTE_RECOVERY_PHASE_PREPARED)
                | ("switch", CODEX_ROUTE_RECOVERY_PHASE_SWITCH_RUNTIME_SWAPPED)
                | ("switch", CODEX_ROUTE_RECOVERY_PHASE_SWITCH_ROUTE_SAVED)
                | ("switch", CODEX_ROUTE_RECOVERY_PHASE_SWITCH_FAILOVERS_SAVED)
                | (
                    CODEX_ROUTE_RECOVERY_OPERATION_RECONCILE,
                    CODEX_ROUTE_RECOVERY_PHASE_RECONCILE_PREPARED
                )
                | (
                    CODEX_ROUTE_RECOVERY_OPERATION_RECONCILE,
                    CODEX_ROUTE_RECOVERY_PHASE_RECONCILE_HOME_APPLIED
                )
        );
        let reconcile_shape_valid =
            if recovery.operation == CODEX_ROUTE_RECOVERY_OPERATION_RECONCILE {
                recovery.reconcile.is_some()
            } else {
                recovery.reconcile.is_none()
            };
        if valid && reconcile_shape_valid {
            Ok(())
        } else {
            Err(AppError::InvalidInput("非法恢复操作".to_string()))
        }
    }

    /// 将失败切换收敛回旧的运行时和持久化快照；无法收敛时保留补偿记录。
    async fn restore_switch_locked(
        &self,
        profile_id: &str,
        route: &CodexProfileRoute,
        failovers: &[String],
        old_snapshot: CodexRouteProviderSnapshot,
    ) -> Result<(), AppError> {
        let current = self.persistence.get_route(profile_id)?.ok_or_else(|| {
            AppError::InvalidInput("Codex Profile 路由不存在，无法补偿切换".to_string())
        })?;
        let persisted_recovery = current.recovery_json.ok_or_else(|| {
            AppError::InvalidInput("Codex Profile 缺少切换补偿记录，无法安全回滚".to_string())
        })?;
        let mut recovery: RouteRecoveryRecord =
            serde_json::from_str(&persisted_recovery).map_err(|_| {
                AppError::InvalidInput("Codex Profile 切换补偿记录无效，无法安全回滚".to_string())
            })?;
        recovery.last_error = Some("切换失败，正在恢复原路由".to_string());
        let mut restoring = self.route_with_recovery(route.clone(), &recovery)?;
        restoring.last_error = Some("切换失败，正在恢复原路由".to_string());
        let runtime = self.runtime(profile_id).map_err(|error| {
            AppError::Message(format!(
                "{CODEX_ROUTE_COMPENSATION_UNCONVERGED_ERROR}: 无法访问切换前运行时: {error}"
            ))
        })?;
        runtime.swap_provider_snapshot(old_snapshot).await;
        self.persistence.save_route(&restoring)?;
        self.persistence.replace_failovers(profile_id, failovers)?;
        restoring.recovery_json = None;
        restoring.last_error = None;
        restoring.updated_at = Utc::now().timestamp_millis();
        self.persistence.save_route(&restoring)
    }

    /// 将补偿记录编码到路由字段；编码失败不应开始任何不可逆变更。
    fn route_with_recovery(
        &self,
        mut route: CodexProfileRoute,
        recovery: &RouteRecoveryRecord,
    ) -> Result<CodexProfileRoute, AppError> {
        route.recovery_json = Some(
            serde_json::to_string(recovery)
                .map_err(|error| AppError::Message(format!("编码路由补偿记录失败: {error}")))?,
        );
        route.updated_at = Utc::now().timestamp_millis();
        Ok(route)
    }
}

impl RouteRecoverySnapshot {
    /// 从当前路由和故障转移列表创建不含敏感配置的恢复快照。
    fn from_route(route: &CodexProfileRoute, failover_ids: Vec<String>) -> Self {
        Self {
            current_provider_id: route.current_provider_id.clone(),
            enabled: route.enabled,
            failover_ids,
        }
    }
}

#[cfg(test)]
mod codex_route_manager {
    use super::*;
    use crate::codex_profile::{CodexProfile, CodexProfileRoute, CodexRouteRuntimeFuture};
    use crate::provider::Provider;
    use serde_json::json;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};

    struct FakeRuntime {
        provider: AsyncMutex<String>,
        port: u16,
    }

    /// 模拟仍有请求在途的运行时，用于证明关闭不会等待请求完成。
    struct InFlightRequestRuntime {
        stop_called: AtomicBool,
    }

    impl CodexRouteRuntime for InFlightRequestRuntime {
        fn start(&self) -> CodexRouteRuntimeFuture<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }

        fn health_check(&self) -> CodexRouteRuntimeFuture<'_, bool> {
            Box::pin(async { true })
        }

        fn swap_provider_snapshot(
            &self,
            _: CodexRouteProviderSnapshot,
        ) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async {})
        }

        fn reject_new_requests(&self) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async {})
        }

        fn stop(&self) -> CodexRouteRuntimeFuture<'_, Result<(), String>> {
            Box::pin(async move {
                self.stop_called.store(true, Ordering::SeqCst);
                Ok(())
            })
        }

        fn status(&self) -> CodexRouteRuntimeFuture<'_, CodexRuntimeStatus> {
            Box::pin(async { CodexRuntimeStatus::Running })
        }
    }

    /// 在关闭边界观察 Home、数据库状态和生命周期事件顺序的运行时替身。
    struct CloseOrderRuntime {
        db: Arc<Database>,
        profile_id: String,
        home_path: std::path::PathBuf,
        events: Mutex<Vec<&'static str>>,
        in_flight_request_active: AtomicBool,
    }

    impl CodexRouteRuntime for CloseOrderRuntime {
        fn start(&self) -> CodexRouteRuntimeFuture<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }

        fn health_check(&self) -> CodexRouteRuntimeFuture<'_, bool> {
            Box::pin(async { true })
        }

        fn swap_provider_snapshot(
            &self,
            _: CodexRouteProviderSnapshot,
        ) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async {})
        }

        fn reject_new_requests(&self) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async move {
                let config = fs::read_to_string(crate::codex_config::codex_config_path_for_home(
                    &self.home_path,
                ))
                .expect("停接前读取已恢复配置");
                assert!(config.contains("base_url = \"https://example.com/v1\""));
                assert!(!config.contains("http://127.0.0.1:16001/v1"));
                assert!(config.contains("[features]"));
                assert!(
                    self.db
                        .get_codex_profile_route(&self.profile_id)
                        .expect("停接前读取路由")
                        .expect("停接前路由存在")
                        .enabled
                );
                self.events.lock().expect("关闭事件锁").push("拒绝新请求");
            })
        }

        fn stop(&self) -> CodexRouteRuntimeFuture<'_, Result<(), String>> {
            Box::pin(async move {
                assert!(self.in_flight_request_active.load(Ordering::SeqCst));
                assert!(
                    self.db
                        .get_codex_profile_route(&self.profile_id)
                        .expect("停止前读取路由")
                        .expect("停止前路由存在")
                        .enabled
                );
                let mut events = self.events.lock().expect("关闭事件锁");
                assert_eq!(events.as_slice(), ["拒绝新请求"]);
                events.push("停止监听器");
                Ok(())
            })
        }

        fn status(&self) -> CodexRouteRuntimeFuture<'_, CodexRuntimeStatus> {
            Box::pin(async { CodexRuntimeStatus::Running })
        }
    }

    impl CodexRouteRuntime for FakeRuntime {
        fn start(&self) -> CodexRouteRuntimeFuture<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn health_check(&self) -> CodexRouteRuntimeFuture<'_, bool> {
            Box::pin(async { true })
        }
        fn swap_provider_snapshot(
            &self,
            snapshot: CodexRouteProviderSnapshot,
        ) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async move {
                *self.provider.lock().await = snapshot.providers()[0].id.clone();
            })
        }
        fn reject_new_requests(&self) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async {})
        }
        fn stop(&self) -> CodexRouteRuntimeFuture<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn status(&self) -> CodexRouteRuntimeFuture<'_, CodexRuntimeStatus> {
            Box::pin(async { CodexRuntimeStatus::Running })
        }
    }
    struct FakeFactory;
    impl CodexRouteRuntimeFactory for FakeFactory {
        fn create(
            &self,
            scope: CodexProfileScope,
            _: String,
            snapshot: CodexRouteProviderSnapshot,
        ) -> Arc<dyn CodexRouteRuntime> {
            Arc::new(FakeRuntime {
                provider: AsyncMutex::new(snapshot.providers()[0].id.clone()),
                port: scope.port,
            })
        }
    }

    /// 将真实数据库与可控的路由保存故障组合，验证补偿流程不会留下分裂状态。
    struct SaveFailingPersistence {
        db: Arc<Database>,
        save_count: AtomicUsize,
        fail_on_save: usize,
        fail_replace: bool,
    }

    /// 按保存序号连续拒绝写入，用于验证补偿失败不会被伪装成已收敛。
    struct SaveSequenceFailingPersistence {
        db: Arc<Database>,
        save_count: AtomicUsize,
        failed_saves: Vec<usize>,
    }

    impl CodexProfileRoutePersistence for SaveSequenceFailingPersistence {
        fn list_profiles(&self) -> Result<Vec<CodexProfile>, AppError> {
            self.db.list_codex_profiles()
        }
        fn get_profile(&self, profile_id: &str) -> Result<CodexProfile, AppError> {
            self.db.get_codex_profile(profile_id)
        }
        fn get_route(&self, profile_id: &str) -> Result<Option<CodexProfileRoute>, AppError> {
            self.db.get_codex_profile_route(profile_id)
        }
        fn save_route(&self, route: &CodexProfileRoute) -> Result<(), AppError> {
            let save_index = self.save_count.fetch_add(1, Ordering::SeqCst) + 1;
            if self.failed_saves.contains(&save_index) {
                return Err(AppError::Message(format!(
                    "模拟第 {save_index} 次路由保存失败"
                )));
            }
            self.db.save_codex_profile_route(route)
        }
        fn list_failovers(&self, profile_id: &str) -> Result<Vec<String>, AppError> {
            self.db.list_codex_profile_failovers(profile_id)
        }
        fn replace_failovers(
            &self,
            profile_id: &str,
            provider_ids: &[String],
        ) -> Result<(), AppError> {
            self.db
                .replace_codex_profile_failovers(profile_id, provider_ids)
        }
        fn get_provider(&self, provider_id: &str) -> Result<Option<Provider>, AppError> {
            self.db
                .get_provider_by_id(provider_id, AppType::Codex.as_str())
        }
        fn delete_profile(&self, profile_id: &str) -> Result<(), AppError> {
            self.db.delete_codex_profile(profile_id)
        }
    }

    impl CodexProfileRoutePersistence for SaveFailingPersistence {
        fn list_profiles(&self) -> Result<Vec<CodexProfile>, AppError> {
            self.db.list_codex_profiles()
        }
        fn get_profile(&self, profile_id: &str) -> Result<CodexProfile, AppError> {
            self.db.get_codex_profile(profile_id)
        }
        fn get_route(&self, profile_id: &str) -> Result<Option<CodexProfileRoute>, AppError> {
            self.db.get_codex_profile_route(profile_id)
        }
        fn save_route(&self, route: &CodexProfileRoute) -> Result<(), AppError> {
            if self.save_count.fetch_add(1, Ordering::SeqCst) + 1 == self.fail_on_save {
                return Err(AppError::Message("模拟路由保存失败".to_string()));
            }
            self.db.save_codex_profile_route(route)
        }
        fn list_failovers(&self, profile_id: &str) -> Result<Vec<String>, AppError> {
            self.db.list_codex_profile_failovers(profile_id)
        }
        fn replace_failovers(
            &self,
            profile_id: &str,
            provider_ids: &[String],
        ) -> Result<(), AppError> {
            if self.fail_replace {
                return Err(AppError::Message("模拟故障转移保存失败".to_string()));
            }
            self.db
                .replace_codex_profile_failovers(profile_id, provider_ids)
        }
        fn get_provider(&self, provider_id: &str) -> Result<Option<Provider>, AppError> {
            self.db
                .get_provider_by_id(provider_id, AppType::Codex.as_str())
        }
        fn delete_profile(&self, profile_id: &str) -> Result<(), AppError> {
            self.db.delete_codex_profile(profile_id)
        }
    }

    /// 仅拒绝删除主记录，模拟 token 已删除后的数据库故障。
    struct DeleteFailingPersistence {
        db: Arc<Database>,
    }

    impl CodexProfileRoutePersistence for DeleteFailingPersistence {
        fn list_profiles(&self) -> Result<Vec<CodexProfile>, AppError> {
            self.db.list_codex_profiles()
        }
        fn get_profile(&self, profile_id: &str) -> Result<CodexProfile, AppError> {
            self.db.get_codex_profile(profile_id)
        }
        fn get_route(&self, profile_id: &str) -> Result<Option<CodexProfileRoute>, AppError> {
            self.db.get_codex_profile_route(profile_id)
        }
        fn save_route(&self, route: &CodexProfileRoute) -> Result<(), AppError> {
            self.db.save_codex_profile_route(route)
        }
        fn list_failovers(&self, profile_id: &str) -> Result<Vec<String>, AppError> {
            self.db.list_codex_profile_failovers(profile_id)
        }
        fn replace_failovers(
            &self,
            profile_id: &str,
            provider_ids: &[String],
        ) -> Result<(), AppError> {
            self.db
                .replace_codex_profile_failovers(profile_id, provider_ids)
        }
        fn get_provider(&self, provider_id: &str) -> Result<Option<Provider>, AppError> {
            self.db
                .get_provider_by_id(provider_id, AppType::Codex.as_str())
        }
        fn delete_profile(&self, _: &str) -> Result<(), AppError> {
            Err(AppError::Message("模拟数据库删除失败".to_string()))
        }
    }

    /// 仅让指定 Profile 的路由读取失败，验证启动恢复可隔离单项故障。
    struct RouteReadFailingPersistence {
        db: Arc<Database>,
        failing_profile: String,
    }

    impl CodexProfileRoutePersistence for RouteReadFailingPersistence {
        fn list_profiles(&self) -> Result<Vec<CodexProfile>, AppError> {
            self.db.list_codex_profiles()
        }
        fn get_profile(&self, profile_id: &str) -> Result<CodexProfile, AppError> {
            self.db.get_codex_profile(profile_id)
        }
        fn get_route(&self, profile_id: &str) -> Result<Option<CodexProfileRoute>, AppError> {
            if profile_id == self.failing_profile {
                Err(AppError::Message("模拟路由读取失败".to_string()))
            } else {
                self.db.get_codex_profile_route(profile_id)
            }
        }
        fn save_route(&self, route: &CodexProfileRoute) -> Result<(), AppError> {
            self.db.save_codex_profile_route(route)
        }
        fn list_failovers(&self, profile_id: &str) -> Result<Vec<String>, AppError> {
            self.db.list_codex_profile_failovers(profile_id)
        }
        fn replace_failovers(
            &self,
            profile_id: &str,
            provider_ids: &[String],
        ) -> Result<(), AppError> {
            self.db
                .replace_codex_profile_failovers(profile_id, provider_ids)
        }
        fn get_provider(&self, provider_id: &str) -> Result<Option<Provider>, AppError> {
            self.db
                .get_provider_by_id(provider_id, AppType::Codex.as_str())
        }
        fn delete_profile(&self, profile_id: &str) -> Result<(), AppError> {
            self.db.delete_codex_profile(profile_id)
        }
    }

    /// 记录删除与补建次数，测试时不暴露任何真实凭证。
    struct TrackingTokenStore {
        ensured: AtomicUsize,
        deleted: AtomicUsize,
    }

    impl CodexProfileTokenStore for TrackingTokenStore {
        fn read_token(&self, _: &str) -> Result<Option<String>, AppError> {
            Ok(Some("test-local-token".to_string()))
        }

        fn ensure_token(&self, _: &str) -> Result<String, AppError> {
            self.ensured.fetch_add(1, Ordering::SeqCst);
            Ok("test-local-token".to_string())
        }
        fn delete_token(&self, _: &str) -> Result<(), AppError> {
            self.deleted.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// 模拟关闭时读取不到既有凭证，并记录是否错误调用了创建流程。
    struct MissingReadTokenStore {
        ensured: AtomicUsize,
    }

    impl CodexProfileTokenStore for MissingReadTokenStore {
        fn read_token(&self, _: &str) -> Result<Option<String>, AppError> {
            Ok(None)
        }

        fn ensure_token(&self, _: &str) -> Result<String, AppError> {
            self.ensured.fetch_add(1, Ordering::SeqCst);
            Ok("unexpected-new-token".to_string())
        }

        fn delete_token(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
    }

    /// 按 Profile 返回不同 token 并记录读取次数，用于验证关闭隔离。
    struct ProfileTokenMapStore {
        tokens: HashMap<String, String>,
        reads: Mutex<HashMap<String, usize>>,
    }

    impl ProfileTokenMapStore {
        /// 返回指定 Profile 的累计读取次数。
        fn read_count(&self, profile_id: &str) -> usize {
            self.reads
                .lock()
                .expect("token 读取计数锁")
                .get(profile_id)
                .copied()
                .unwrap_or_default()
        }
    }

    impl CodexProfileTokenStore for ProfileTokenMapStore {
        fn read_token(&self, profile_id: &str) -> Result<Option<String>, AppError> {
            let mut reads = self.reads.lock()?;
            *reads.entry(profile_id.to_string()).or_default() += 1;
            Ok(self.tokens.get(profile_id).cloned())
        }

        fn ensure_token(&self, profile_id: &str) -> Result<String, AppError> {
            self.tokens
                .get(profile_id)
                .cloned()
                .ok_or_else(|| AppError::Config("测试 Profile 缺少预置 listener token".to_string()))
        }

        fn delete_token(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
    }

    /// 记录启动对账的 Home 写入次数，同时复用真实原子文件操作。
    struct CountingHomeFileOps {
        writes: AtomicUsize,
    }

    /// 只拒绝指定模型目录写入，用于验证启动对账隔离单个 Profile。
    struct FailCatalogWriteOps {
        fail_path: std::path::PathBuf,
    }

    impl crate::codex_profile::CodexHomeFileOps for CountingHomeFileOps {
        fn read(&self, path: &Path) -> Result<Option<Vec<u8>>, AppError> {
            if path.exists() {
                fs::read(path)
                    .map(Some)
                    .map_err(|error| AppError::io(path, error))
            } else {
                Ok(None)
            }
        }

        fn write_atomic(&self, path: &Path, content: &[u8]) -> Result<(), AppError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            crate::config::atomic_write(path, content)
        }

        fn remove_file(&self, path: &Path) -> Result<(), AppError> {
            if path.exists() {
                fs::remove_file(path).map_err(|error| AppError::io(path, error))?;
            }
            Ok(())
        }
    }

    impl crate::codex_profile::CodexHomeFileOps for FailCatalogWriteOps {
        fn read(&self, path: &Path) -> Result<Option<Vec<u8>>, AppError> {
            if path.exists() {
                fs::read(path)
                    .map(Some)
                    .map_err(|error| AppError::io(path, error))
            } else {
                Ok(None)
            }
        }

        fn write_atomic(&self, path: &Path, content: &[u8]) -> Result<(), AppError> {
            if path == self.fail_path {
                return Err(AppError::Config("模拟模型目录写入失败".to_string()));
            }
            crate::config::atomic_write(path, content)
        }

        fn remove_file(&self, path: &Path) -> Result<(), AppError> {
            if path.exists() {
                fs::remove_file(path).map_err(|error| AppError::io(path, error))?;
            }
            Ok(())
        }
    }

    /// 创建带有效原始备份的已启用 Profile，返回其当前路由目标计划。
    fn prepare_enabled_profile_home(
        db: &Database,
        home_config: &CodexHomeConfigService,
        home: &Path,
        profile_id: &str,
        listen_port: u16,
        listener_token: &str,
    ) -> Result<CodexRouteConfigPlan, AppError> {
        use crate::codex_config::codex_config_path_for_home;

        let provider_id = format!("provider-{profile_id}");
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id(provider_id.clone(), provider_id.clone(), json!({}), None),
        )?;
        db.insert_codex_profile(&CodexProfile {
            id: profile_id.to_string(),
            name: profile_id.to_string(),
            canonical_home_path: home.display().to_string(),
            listen_port,
            created_at: 1,
            updated_at: 1,
        })?;
        fs::write(
            codex_config_path_for_home(home),
            format!(
                "model_provider = \"{provider_id}\"\n\n[model_providers.{provider_id}]\nname = \"Original\"\nbase_url = \"https://example.com/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"upstream-token\"\n"
            ),
        )
        .expect("写入原始 Home 配置");
        let provider = db
            .get_provider_by_id(&provider_id, AppType::Codex.as_str())?
            .expect("供应商已写入");
        let plan = home_config.build_profile_route_plan(
            home,
            listen_port,
            Some(&provider),
            listener_token,
        )?;
        let backup = home_config.serialize_backup(&plan)?;
        home_config.apply_route_plan(&plan)?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: profile_id.to_string(),
            current_provider_id: Some(provider_id),
            enabled: true,
            live_backup_json: Some(backup),
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        Ok(plan)
    }

    /// 构造包含路由模型目录的测试供应商。
    fn provider_with_route_catalog(id: &str, model: &str) -> Provider {
        let mut provider = Provider::with_id(
            id.to_string(),
            id.to_string(),
            json!({
                "auth": {"OPENAI_API_KEY": "test-token"},
                "config": "model_provider = \"custom\"\n[model_providers.custom]\nbase_url = \"https://example.com/v1\"\nwire_api = \"responses\"\n",
                "modelCatalog": {"models": [{"model": model}]}
            }),
            None,
        );
        provider.meta = Some(crate::provider::ProviderMeta {
            api_format: Some("openai_chat".to_string()),
            ..Default::default()
        });
        provider
    }

    /// 直接准备带模型目录的已启用 Profile，避免依赖待测启用流程。
    fn prepare_enabled_catalog_profile(
        db: &Database,
        home_config: &CodexHomeConfigService,
        home: &Path,
        profile_id: &str,
        listen_port: u16,
        provider: &Provider,
    ) -> Result<(), AppError> {
        db.insert_codex_profile(&CodexProfile {
            id: profile_id.to_string(),
            name: profile_id.to_string(),
            canonical_home_path: home.display().to_string(),
            listen_port,
            created_at: 1,
            updated_at: 1,
        })?;
        let catalog_plan = home_config.build_model_catalog_projection_plan(home, provider)?;
        home_config.apply_model_catalog_projection_plan(&catalog_plan)?;
        let route_plan = home_config.build_profile_route_plan(
            home,
            listen_port,
            Some(provider),
            "test-local-token",
        )?;
        let backup = home_config.serialize_backup(&route_plan)?;
        home_config.apply_route_plan(&route_plan)?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: profile_id.to_string(),
            current_provider_id: Some(provider.id.clone()),
            enabled: true,
            live_backup_json: Some(backup),
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        Ok(())
    }

    #[tokio::test]
    async fn enable_route_creates_missing_catalog_in_profile_home() -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("创建临时 Home");
        let provider = provider_with_route_catalog("provider-a", "model-a");
        db.save_provider(AppType::Codex.as_str(), &provider)?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16_001,
            created_at: 1,
            updated_at: 1,
        })?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: None,
            enabled: false,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        let manager = CodexRouteManager::new(
            db,
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        manager.enable("profile-a", "provider-a", vec![]).await?;

        let catalog = fs::read_to_string(
            home.path()
                .join(crate::codex_config::CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME),
        )
        .expect("读取 Profile 模型目录");
        let config =
            fs::read_to_string(crate::codex_config::codex_config_path_for_home(home.path()))
                .expect("读取 Profile 配置");
        assert!(catalog.contains("model-a"));
        assert!(config.contains("model_catalog_json = \"cc-switch-model-catalog.json\""));
        Ok(())
    }

    #[tokio::test]
    async fn enabled_switch_replaces_only_selected_profile_catalog() -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home_a = tempfile::tempdir().expect("创建 A Home");
        let home_b = tempfile::tempdir().expect("创建 B Home");
        let home_config = Arc::new(CodexHomeConfigService::system());
        let provider_a = provider_with_route_catalog("provider-a", "model-a");
        let provider_b = provider_with_route_catalog("provider-b", "model-b");
        db.save_provider(AppType::Codex.as_str(), &provider_a)?;
        db.save_provider(AppType::Codex.as_str(), &provider_b)?;
        prepare_enabled_catalog_profile(
            &db,
            &home_config,
            home_a.path(),
            "profile-a",
            16_001,
            &provider_a,
        )?;
        prepare_enabled_catalog_profile(
            &db,
            &home_config,
            home_b.path(),
            "profile-b",
            16_002,
            &provider_a,
        )?;
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );
        for profile_id in ["profile-a", "profile-b"] {
            manager.runtimes.lock()?.insert(
                profile_id.to_string(),
                Arc::new(FakeRuntime {
                    provider: AsyncMutex::new("provider-a".to_string()),
                    port: if profile_id == "profile-a" {
                        16_001
                    } else {
                        16_002
                    },
                }),
            );
        }

        manager
            .switch_provider("profile-a", "provider-b", vec![])
            .await?;

        let catalog_name = crate::codex_config::CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME;
        let catalog_a = fs::read_to_string(home_a.path().join(catalog_name)).expect("读取 A 目录");
        let catalog_b = fs::read_to_string(home_b.path().join(catalog_name)).expect("读取 B 目录");
        assert!(catalog_a.contains("model-b"));
        assert!(!catalog_a.contains("model-a"));
        assert!(catalog_b.contains("model-a"));
        assert!(!catalog_b.contains("model-b"));
        Ok(())
    }

    #[tokio::test]
    async fn enabled_switch_persistence_failure_restores_profile_catalog() -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("创建临时 Home");
        let home_config = Arc::new(CodexHomeConfigService::system());
        let provider_a = provider_with_route_catalog("provider-a", "model-a");
        let provider_b = provider_with_route_catalog("provider-b", "model-b");
        db.save_provider(AppType::Codex.as_str(), &provider_a)?;
        db.save_provider(AppType::Codex.as_str(), &provider_b)?;
        prepare_enabled_catalog_profile(
            &db,
            &home_config,
            home.path(),
            "profile-a",
            16_001,
            &provider_a,
        )?;
        let catalog_path = home
            .path()
            .join(crate::codex_config::CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
        let original_catalog = fs::read(&catalog_path).expect("读取初始模型目录");
        let manager = CodexRouteManager::new(
            Arc::new(SaveFailingPersistence {
                db,
                save_count: AtomicUsize::new(0),
                fail_on_save: 2,
                fail_replace: false,
            }),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );
        manager.runtimes.lock()?.insert(
            "profile-a".to_string(),
            Arc::new(FakeRuntime {
                provider: AsyncMutex::new("provider-a".to_string()),
                port: 16_001,
            }),
        );

        assert!(manager
            .switch_provider("profile-a", "provider-b", vec![])
            .await
            .is_err());

        assert_eq!(
            fs::read(&catalog_path).expect("重读补偿后的模型目录"),
            original_catalog
        );
        Ok(())
    }

    /// 准备共享供应商扇出测试的 Profile、主引用和初始模型目录。
    fn prepare_catalog_reference_profile(
        db: &Database,
        home_config: &CodexHomeConfigService,
        home: &Path,
        profile_id: &str,
        listen_port: u16,
        primary_provider: &Provider,
        failover_ids: &[String],
    ) -> Result<(), AppError> {
        db.insert_codex_profile(&CodexProfile {
            id: profile_id.to_string(),
            name: profile_id.to_string(),
            canonical_home_path: home.display().to_string(),
            listen_port,
            created_at: 1,
            updated_at: 1,
        })?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: profile_id.to_string(),
            current_provider_id: Some(primary_provider.id.clone()),
            enabled: false,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        db.replace_codex_profile_failovers(profile_id, failover_ids)?;
        let plan = home_config.build_model_catalog_projection_plan(home, primary_provider)?;
        home_config.apply_model_catalog_projection_plan(&plan)
    }

    #[tokio::test]
    async fn shared_provider_catalog_update_only_projects_primary_references(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home_a = tempfile::tempdir().expect("创建 A Home");
        let home_b = tempfile::tempdir().expect("创建 B Home");
        let home_c = tempfile::tempdir().expect("创建 C Home");
        let home_config = Arc::new(CodexHomeConfigService::system());
        let old_shared = provider_with_route_catalog("provider-shared", "old-shared");
        let new_shared = provider_with_route_catalog("provider-shared", "new-shared");
        let other = provider_with_route_catalog("provider-other", "other-model");
        db.save_provider(AppType::Codex.as_str(), &old_shared)?;
        db.save_provider(AppType::Codex.as_str(), &other)?;
        prepare_catalog_reference_profile(
            &db,
            &home_config,
            home_a.path(),
            "profile-a",
            16_001,
            &old_shared,
            &[],
        )?;
        prepare_catalog_reference_profile(
            &db,
            &home_config,
            home_b.path(),
            "profile-b",
            16_002,
            &other,
            &["provider-shared".to_string()],
        )?;
        prepare_catalog_reference_profile(
            &db,
            &home_config,
            home_c.path(),
            "profile-c",
            16_003,
            &old_shared,
            &[],
        )?;
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        manager
            .with_provider_catalog_update(&new_shared, || {
                db.save_provider(AppType::Codex.as_str(), &new_shared)
            })
            .await?;

        let catalog_name = crate::codex_config::CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME;
        let catalog_a = fs::read_to_string(home_a.path().join(catalog_name)).expect("读取 A 目录");
        let catalog_b = fs::read_to_string(home_b.path().join(catalog_name)).expect("读取 B 目录");
        let catalog_c = fs::read_to_string(home_c.path().join(catalog_name)).expect("读取 C 目录");
        assert!(catalog_a.contains("new-shared"));
        assert!(catalog_c.contains("new-shared"));
        assert!(catalog_b.contains("other-model"));
        assert!(!catalog_b.contains("new-shared"));
        Ok(())
    }

    #[tokio::test]
    async fn provider_catalog_commit_failure_restores_provider_and_homes() -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home_a = tempfile::tempdir().expect("创建 A Home");
        let home_c = tempfile::tempdir().expect("创建 C Home");
        let home_config = Arc::new(CodexHomeConfigService::system());
        let old_shared = provider_with_route_catalog("provider-shared", "old-shared");
        let new_shared = provider_with_route_catalog("provider-shared", "new-shared");
        db.save_provider(AppType::Codex.as_str(), &old_shared)?;
        prepare_catalog_reference_profile(
            &db,
            &home_config,
            home_a.path(),
            "profile-a",
            16_001,
            &old_shared,
            &[],
        )?;
        prepare_catalog_reference_profile(
            &db,
            &home_config,
            home_c.path(),
            "profile-c",
            16_003,
            &old_shared,
            &[],
        )?;
        let catalog_name = crate::codex_config::CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME;
        let original_a = fs::read(home_a.path().join(catalog_name)).expect("读取 A 原目录");
        let original_c = fs::read(home_c.path().join(catalog_name)).expect("读取 C 原目录");
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        let error = manager
            .with_provider_catalog_update(&new_shared, || {
                db.save_provider(AppType::Codex.as_str(), &new_shared)?;
                Err::<(), AppError>(AppError::Message("模拟供应商提交失败".to_string()))
            })
            .await
            .expect_err("供应商提交失败必须回滚");

        assert!(error.to_string().contains("模拟供应商提交失败"));
        assert_eq!(
            db.get_provider_by_id("provider-shared", AppType::Codex.as_str())?
                .expect("供应商已恢复")
                .settings_config,
            old_shared.settings_config
        );
        assert_eq!(
            fs::read(home_a.path().join(catalog_name)).expect("读取 A 恢复目录"),
            original_a
        );
        assert_eq!(
            fs::read(home_c.path().join(catalog_name)).expect("读取 C 恢复目录"),
            original_c
        );
        Ok(())
    }

    #[tokio::test]
    async fn reconcile_all_profile_catalogs_repairs_healthy_homes_and_isolates_failure(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home_a = tempfile::tempdir().expect("创建 A Home");
        let home_b = tempfile::tempdir().expect("创建 B Home");
        let home_c = tempfile::tempdir().expect("创建 C Home");
        let shared = provider_with_route_catalog("provider-shared", "new-shared");
        let stale_shared = provider_with_route_catalog("provider-shared", "stale-shared");
        let failing = provider_with_route_catalog("provider-failing", "failing-model");
        db.save_provider(AppType::Codex.as_str(), &shared)?;
        db.save_provider(AppType::Codex.as_str(), &failing)?;

        for (profile_id, home, provider_id, last_error, port) in [
            (
                "profile-a",
                home_a.path(),
                "provider-shared",
                Some(format!("{CODEX_CATALOG_RECONCILE_ERROR_PREFIX}: 旧错误")),
                16_001,
            ),
            (
                "profile-b",
                home_b.path(),
                "provider-shared",
                Some("其他生命周期错误".to_string()),
                16_002,
            ),
            ("profile-c", home_c.path(), "provider-failing", None, 16_003),
        ] {
            db.insert_codex_profile(&CodexProfile {
                id: profile_id.to_string(),
                name: profile_id.to_string(),
                canonical_home_path: home.display().to_string(),
                listen_port: port,
                created_at: 1,
                updated_at: 1,
            })?;
            db.save_codex_profile_route(&CodexProfileRoute {
                profile_id: profile_id.to_string(),
                current_provider_id: Some(provider_id.to_string()),
                enabled: false,
                live_backup_json: None,
                last_error,
                recovery_json: None,
                updated_at: 1,
            })?;
        }
        let setup_home_config = CodexHomeConfigService::system();
        let stale_plan =
            setup_home_config.build_model_catalog_projection_plan(home_b.path(), &stale_shared)?;
        setup_home_config.apply_model_catalog_projection_plan(&stale_plan)?;
        let fail_path = home_c
            .path()
            .join(crate::codex_config::CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::new(Arc::new(FailCatalogWriteOps {
                fail_path,
            }))),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        manager.reconcile_all_profile_catalogs().await?;

        let catalog_name = crate::codex_config::CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME;
        let catalog_a = fs::read_to_string(home_a.path().join(catalog_name)).expect("读取 A 目录");
        let catalog_b = fs::read_to_string(home_b.path().join(catalog_name)).expect("读取 B 目录");
        assert!(catalog_a.contains("new-shared"));
        assert!(catalog_b.contains("new-shared"));
        assert!(!catalog_b.contains("stale-shared"));
        assert!(db
            .get_codex_profile_route("profile-a")?
            .expect("A 路由")
            .last_error
            .is_none());
        assert_eq!(
            db.get_codex_profile_route("profile-b")?
                .expect("B 路由")
                .last_error
                .as_deref(),
            Some("其他生命周期错误")
        );
        let error_c = db
            .get_codex_profile_route("profile-c")?
            .expect("C 路由")
            .last_error
            .expect("C 应记录目录错误");
        assert!(error_c.starts_with(CODEX_CATALOG_RECONCILE_ERROR_PREFIX));
        assert!(error_c.contains("模拟模型目录写入失败"));
        assert!(!home_c.path().join(catalog_name).exists());
        assert!(manager.runtimes.lock()?.is_empty());
        Ok(())
    }

    /// 关闭态选择供应商只更新目标 Home 与供应商引用，不得创建 token 或启动路由。
    #[tokio::test]
    async fn switching_disabled_profile_preserves_route_state() -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Profile Home");
        let mut provider = Provider::with_id(
            "official".to_string(),
            "OpenAI Official".to_string(),
            json!({
                "auth": {"auth_mode": "chatgpt"},
                "config": "model = \"gpt-official\"\n"
            }),
            None,
        );
        provider.category = Some("official".to_string());
        db.save_provider(AppType::Codex.as_str(), &provider)?;
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id(
                "provider-failover".to_string(),
                "Provider Failover".to_string(),
                json!({}),
                None,
            ),
        )?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: None,
            enabled: false,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        db.replace_codex_profile_failovers("profile-a", &["provider-failover".to_string()])?;
        let token_store = Arc::new(TrackingTokenStore {
            ensured: AtomicUsize::new(0),
            deleted: AtomicUsize::new(0),
        });
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::system()),
            token_store.clone(),
            Arc::new(FakeFactory),
        );

        manager
            .switch_provider_with_effective_settings_preserving_failovers("profile-a", provider)
            .await?;

        let route = db.get_codex_profile_route("profile-a")?.expect("路由记录");
        assert!(!route.enabled);
        assert_eq!(route.current_provider_id.as_deref(), Some("official"));
        assert!(route.live_backup_json.is_none());
        assert_eq!(
            db.list_codex_profile_failovers("profile-a")?,
            vec!["provider-failover".to_string()]
        );
        assert_eq!(token_store.ensured.load(Ordering::SeqCst), 0);
        assert!(manager.runtimes.lock().expect("运行时锁").is_empty());
        let config =
            fs::read_to_string(crate::codex_config::codex_config_path_for_home(home.path()))
                .expect("读取直连配置");
        assert!(config.contains("gpt-official"));
        assert!(!config.contains("127.0.0.1:"));
        Ok(())
    }

    /// 即使 runtime 尚未恢复，持久化启用态也必须阻止 Profile 元数据修改。
    #[tokio::test]
    async fn profile_metadata_lock_treats_enabled_route_as_active_without_runtime(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: "/tmp/profile-a".to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: None,
            enabled: true,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        let manager = CodexRouteManager::new(
            db,
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );
        let mutation_executed = AtomicBool::new(false);

        let result = manager
            .with_profile_metadata_lock("profile-a", |runtime_status| {
                if runtime_status.is_active() {
                    return Err(AppError::InvalidInput(
                        "运行中的 Codex Profile 不可修改绑定".to_string(),
                    ));
                }
                mutation_executed.store(true, Ordering::SeqCst);
                Ok(())
            })
            .await;

        assert!(result.is_err());
        assert!(!mutation_executed.load(Ordering::SeqCst));
        Ok(())
    }

    /// 关闭态供应商引用保存失败时必须恢复 Home，避免配置与数据库分裂。
    #[tokio::test]
    async fn switching_disabled_profile_save_failure_restores_home() -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Profile Home");
        let config_path = crate::codex_config::codex_config_path_for_home(home.path());
        fs::write(&config_path, "model = \"before\"\n").expect("写入原配置");
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id(
                "provider-new".to_string(),
                "Provider New".to_string(),
                json!({"auth": {}, "config": "model = \"after\"\n"}),
                None,
            ),
        )?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: None,
            enabled: false,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        let manager = CodexRouteManager::new(
            Arc::new(SaveFailingPersistence {
                db: db.clone(),
                save_count: AtomicUsize::new(0),
                fail_on_save: 1,
                fail_replace: false,
            }),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        assert!(manager
            .switch_provider("profile-a", "provider-new", vec![])
            .await
            .is_err());

        assert_eq!(
            fs::read_to_string(config_path).expect("重读原配置"),
            "model = \"before\"\n"
        );
        let route = db.get_codex_profile_route("profile-a")?.expect("路由记录");
        assert!(!route.enabled);
        assert!(route.current_provider_id.is_none());
        Ok(())
    }

    /// 已启用路由不能热切到官方订阅供应商，必须先由用户显式关闭路由。
    #[tokio::test]
    async fn switching_enabled_profile_rejects_official_provider() -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id(
                "provider-old".to_string(),
                "Provider Old".to_string(),
                json!({}),
                None,
            ),
        )?;
        let mut official = Provider::with_id(
            "official".to_string(),
            "OpenAI Official".to_string(),
            json!({"auth": {"auth_mode": "chatgpt"}, "config": ""}),
            None,
        );
        official.category = Some("official".to_string());
        db.save_provider(AppType::Codex.as_str(), &official)?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: "/tmp/profile-a".to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: Some("provider-old".to_string()),
            enabled: true,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );
        let runtime = Arc::new(FakeRuntime {
            provider: AsyncMutex::new("provider-old".to_string()),
            port: 16001,
        });
        manager
            .runtimes
            .lock()
            .expect("运行时锁")
            .insert("profile-a".to_string(), runtime.clone());

        let error = manager
            .switch_provider("profile-a", "official", vec![])
            .await
            .expect_err("官方订阅不能经过路由");

        assert!(error.to_string().contains("请先关闭路由"));
        assert_eq!(*runtime.provider.lock().await, "provider-old");
        assert_eq!(
            db.get_codex_profile_route("profile-a")?
                .expect("路由记录")
                .current_provider_id
                .as_deref(),
            Some("provider-old")
        );
        Ok(())
    }

    /// 启动所有权判断必须基于任意 Profile 路由，而不是默认 Home 身份。
    #[test]
    fn legacy_codex_takeover_retirement_detects_any_enabled_profile_route() -> Result<(), AppError>
    {
        let db = Arc::new(Database::memory()?);
        db.insert_codex_profile(&CodexProfile {
            id: "custom-profile".to_string(),
            name: "自定义 Profile".to_string(),
            canonical_home_path: "/tmp/custom-profile".to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );
        assert!(!manager.has_enabled_profile_routes()?);

        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "custom-profile".to_string(),
            current_provider_id: None,
            enabled: true,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;

        assert!(manager.has_enabled_profile_routes()?);
        Ok(())
    }

    /// 启用 Profile 路由时，Home 与监听器必须使用同一份本地凭证，关闭后恢复原配置。
    #[tokio::test]
    async fn enabling_projects_profile_listener_token_and_disable_restores_home(
    ) -> Result<(), AppError> {
        use crate::codex_config::{
            codex_config_path_for_home, extract_codex_experimental_bearer_token,
        };

        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let original_config = r#"model_provider = "custom"

[model_providers.custom]
name = "Custom"
base_url = "https://example.com/v1"
wire_api = "responses"
experimental_bearer_token = "PROXY_MANAGED"
"#;
        fs::write(codex_config_path_for_home(home.path()), original_config).expect("写入原始配置");
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id(
                "provider-a".to_string(),
                "Provider A".to_string(),
                json!({}),
                None,
            ),
        )?;
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id(
                "provider-failover".to_string(),
                "Provider Failover".to_string(),
                json!({}),
                None,
            ),
        )?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        db.replace_codex_profile_failovers("profile-a", &["provider-failover".to_string()])?;
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        manager
            .enable_preserving_failovers("profile-a", "provider-a")
            .await?;

        let routed_config =
            fs::read_to_string(codex_config_path_for_home(home.path())).expect("读取路由配置");
        assert_eq!(
            extract_codex_experimental_bearer_token(&routed_config).as_deref(),
            Some("test-local-token")
        );
        assert_eq!(
            db.list_codex_profile_failovers("profile-a")?,
            vec!["provider-failover".to_string()]
        );

        manager.disable("profile-a").await?;
        assert_eq!(
            fs::read_to_string(codex_config_path_for_home(home.path())).expect("读取恢复配置"),
            original_config
        );
        Ok(())
    }

    /// 用户显式关闭必须立即停接，不得等待永不结束的在途路由请求。
    #[tokio::test]
    async fn disabling_route_does_not_wait_for_in_flight_request() -> Result<(), AppError> {
        use crate::codex_config::codex_config_path_for_home;

        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let home_config = Arc::new(CodexHomeConfigService::system());
        prepare_enabled_profile_home(
            &db,
            &home_config,
            home.path(),
            "profile-immediate-close",
            16_001,
            "test-local-token",
        )?;
        let config_path = codex_config_path_for_home(home.path());
        let routed_config = fs::read_to_string(&config_path).expect("读取接管配置");
        fs::write(
            &config_path,
            format!("{routed_config}\n[features]\njs_repl = true\n"),
        )
        .expect("模拟关闭前的非路由配置修改");
        let auth_path = home.path().join("auth.json");
        fs::write(&auth_path, "{\"auth\":\"unchanged\"}\n").expect("写入认证文件");
        let runtime = Arc::new(CloseOrderRuntime {
            db: db.clone(),
            profile_id: "profile-immediate-close".to_string(),
            home_path: home.path().to_path_buf(),
            events: Mutex::new(Vec::new()),
            in_flight_request_active: AtomicBool::new(true),
        });
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );
        manager.track_runtime("profile-immediate-close".to_string(), runtime.clone())?;

        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            manager.disable("profile-immediate-close"),
        )
        .await
        .expect("显式关闭不应等待在途路由请求")?;

        assert_eq!(
            runtime.events.lock().expect("关闭事件锁").as_slice(),
            ["拒绝新请求", "停止监听器"]
        );
        assert!(runtime.in_flight_request_active.load(Ordering::SeqCst));
        let restored = fs::read_to_string(config_path).expect("读取恢复配置");
        assert!(restored.contains("base_url = \"https://example.com/v1\""));
        assert!(!restored.contains("http://127.0.0.1:16001/v1"));
        assert!(restored.contains("[features]"));
        assert_eq!(
            fs::read_to_string(auth_path).expect("读取认证文件"),
            "{\"auth\":\"unchanged\"}\n"
        );
        let route = db
            .get_codex_profile_route("profile-immediate-close")?
            .expect("路由存在");
        assert!(!route.enabled);
        assert!(route.recovery_json.is_none());
        Ok(())
    }

    /// 关闭时缺少既有 token 必须保留补偿，且不得调用 ensure 创建新凭证。
    #[tokio::test]
    async fn disabling_route_with_missing_token_keeps_recovery_without_ensuring(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let home_config = Arc::new(CodexHomeConfigService::system());
        prepare_enabled_profile_home(
            &db,
            &home_config,
            home.path(),
            "profile-missing-token",
            16_001,
            "existing-listener-token",
        )?;
        let tokens = Arc::new(MissingReadTokenStore {
            ensured: AtomicUsize::new(0),
        });
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config,
            tokens.clone(),
            Arc::new(FakeFactory),
        );

        let error = manager
            .disable("profile-missing-token")
            .await
            .expect_err("缺少既有 token 必须拒绝关闭");

        assert!(!error.to_string().contains("existing-listener-token"));
        let route = db
            .get_codex_profile_route("profile-missing-token")?
            .expect("路由存在");
        assert!(route.enabled);
        assert!(route.recovery_json.is_some());
        assert_eq!(tokens.ensured.load(Ordering::SeqCst), 0);
        Ok(())
    }

    /// 正常关闭只恢复路由字段，必须保留 Desktop 段与关闭瞬间的模型。
    #[tokio::test]
    async fn disabling_route_preserves_desktop_fields_and_latest_model() -> Result<(), AppError> {
        use crate::codex_config::codex_config_path_for_home;

        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let home_config = Arc::new(CodexHomeConfigService::system());
        prepare_enabled_profile_home(
            &db,
            &home_config,
            home.path(),
            "profile-desktop",
            16_001,
            "test-local-token",
        )?;
        let path = codex_config_path_for_home(home.path());
        let routed = fs::read_to_string(&path).expect("读取路由配置");
        fs::write(
            &path,
            format!(
                "model = \"latest-model\"\n{routed}\n[desktop]\nfollowUpQueueMode = \"queue\"\n\n[plugins.\"computer-use@openai-bundled\"]\nenabled = true\n\n[mcp_servers.node_repl]\ncommand = \"node_repl\"\n"
            ),
        )
        .expect("模拟 Codex Desktop 写入");
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        manager.disable("profile-desktop").await?;

        let restored = fs::read_to_string(path).expect("读取恢复配置");
        assert!(restored.contains("model = \"latest-model\""));
        assert!(restored.contains("followUpQueueMode = \"queue\""));
        assert!(restored.contains("computer-use@openai-bundled"));
        assert!(restored.contains("mcp_servers.node_repl"));
        assert!(restored.contains("base_url = \"https://example.com/v1\""));
        assert!(!restored.contains("http://127.0.0.1:16001/v1"));
        let route = db
            .get_codex_profile_route("profile-desktop")?
            .expect("路由存在");
        assert!(!route.enabled);
        assert!(route.recovery_json.is_none());
        Ok(())
    }

    /// 关闭 A 只能读取和恢复 A 的 Home/token，不得触碰仍启用的 B。
    #[tokio::test]
    async fn disabling_route_isolates_other_profile_home_token_and_state() -> Result<(), AppError> {
        use crate::codex_config::codex_config_path_for_home;

        let db = Arc::new(Database::memory()?);
        let home_a = tempfile::tempdir().expect("A Home");
        let home_b = tempfile::tempdir().expect("B Home");
        let home_config = Arc::new(CodexHomeConfigService::system());
        prepare_enabled_profile_home(
            &db,
            &home_config,
            home_a.path(),
            "profile-a",
            16_001,
            "token-a",
        )?;
        prepare_enabled_profile_home(
            &db,
            &home_config,
            home_b.path(),
            "profile-b",
            16_002,
            "token-b",
        )?;
        let home_b_before =
            fs::read(codex_config_path_for_home(home_b.path())).expect("读取 B Home");
        let tokens = Arc::new(ProfileTokenMapStore {
            tokens: HashMap::from([
                ("profile-a".to_string(), "token-a".to_string()),
                ("profile-b".to_string(), "token-b".to_string()),
            ]),
            reads: Mutex::new(HashMap::new()),
        });
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config,
            tokens.clone(),
            Arc::new(FakeFactory),
        );

        manager.disable("profile-a").await?;

        assert_eq!(tokens.read_count("profile-a"), 1);
        assert_eq!(tokens.read_count("profile-b"), 0);
        assert_eq!(
            fs::read(codex_config_path_for_home(home_b.path())).expect("重读 B Home"),
            home_b_before
        );
        assert!(
            !db.get_codex_profile_route("profile-a")?
                .expect("A 路由")
                .enabled
        );
        assert!(
            db.get_codex_profile_route("profile-b")?
                .expect("B 路由")
                .enabled
        );
        Ok(())
    }

    /// 已是当前目标的 Home 启动时不得重复写入，只需恢复对应监听器。
    #[tokio::test]
    async fn restoring_enabled_profile_home_skips_write_when_target_is_current(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let file_ops = Arc::new(CountingHomeFileOps {
            writes: AtomicUsize::new(0),
        });
        let home_config = Arc::new(CodexHomeConfigService::new(file_ops.clone()));
        prepare_enabled_profile_home(
            &db,
            &home_config,
            home.path(),
            "profile-current",
            16_001,
            "test-local-token",
        )?;
        file_ops.writes.store(0, Ordering::SeqCst);
        let manager = CodexRouteManager::new(
            db,
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        manager.restore_enabled_profiles().await?;

        assert_eq!(file_ops.writes.load(Ordering::SeqCst), 0);
        assert_eq!(
            manager.status("profile-current").await?,
            CodexRuntimeStatus::Running
        );
        Ok(())
    }

    /// 旧占位 token 与备份指纹可证明的旧 listener token 都应自愈为当前本地凭证。
    #[tokio::test]
    async fn restoring_enabled_profile_home_repairs_legacy_and_stale_listener_tokens(
    ) -> Result<(), AppError> {
        use crate::codex_config::{
            codex_config_path_for_home, extract_codex_experimental_bearer_token,
        };

        let db = Arc::new(Database::memory()?);
        let home_config = Arc::new(CodexHomeConfigService::system());
        let legacy_home = tempfile::tempdir().expect("旧占位 Home");
        let stale_home = tempfile::tempdir().expect("旧 token Home");
        prepare_enabled_profile_home(
            &db,
            &home_config,
            legacy_home.path(),
            "profile-legacy",
            16_001,
            "old-listener-token",
        )?;
        fs::write(
            codex_config_path_for_home(legacy_home.path()),
            "model_provider = \"provider-profile-legacy\"\n\n[model_providers.provider-profile-legacy]\nname = \"Legacy\"\nbase_url = \"http://127.0.0.1:16001/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"PROXY_MANAGED\"\n",
        )
        .expect("写入旧占位配置");
        prepare_enabled_profile_home(
            &db,
            &home_config,
            stale_home.path(),
            "profile-stale",
            16_002,
            "old-listener-token",
        )?;
        let manager = CodexRouteManager::new(
            db,
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        manager.restore_enabled_profiles().await?;

        for (profile_id, home) in [
            ("profile-legacy", legacy_home.path()),
            ("profile-stale", stale_home.path()),
        ] {
            let content =
                fs::read_to_string(codex_config_path_for_home(home)).expect("读取自愈后的 Home");
            assert_eq!(
                extract_codex_experimental_bearer_token(&content).as_deref(),
                Some("test-local-token")
            );
            assert_eq!(
                manager.status(profile_id).await?,
                CodexRuntimeStatus::Running
            );
        }
        Ok(())
    }

    /// 旧全局占位覆盖 Profile 目标后，未完成关闭仍应恢复原始 Home 并收敛为关闭。
    #[tokio::test]
    async fn restoring_enabled_profile_home_completes_pending_disable_from_legacy_placeholder(
    ) -> Result<(), AppError> {
        use crate::codex_config::{
            codex_config_path_for_home, extract_codex_experimental_bearer_token,
        };

        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let home_config = Arc::new(CodexHomeConfigService::system());
        prepare_enabled_profile_home(
            &db,
            &home_config,
            home.path(),
            "profile-pending-disable",
            16_001,
            "old-listener-token",
        )?;
        fs::write(
            codex_config_path_for_home(home.path()),
            "model_provider = \"provider-profile-pending-disable\"\n\n[model_providers.provider-profile-pending-disable]\nname = \"Legacy\"\nbase_url = \"http://127.0.0.1:16001/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"PROXY_MANAGED\"\n",
        )
        .expect("写入旧全局占位配置");
        let mut route = db
            .get_codex_profile_route("profile-pending-disable")?
            .expect("路由存在");
        let before = RouteRecoverySnapshot::from_route(&route, vec![]);
        let mut target = before.clone();
        target.enabled = false;
        route.recovery_json = Some(
            serde_json::to_string(&RouteRecoveryRecord {
                operation: "disable".to_string(),
                before,
                target,
                phase: CODEX_ROUTE_RECOVERY_PHASE_DISABLE_HOME_RESTORE_FAILED.to_string(),
                last_error: Some("CODEX_PROFILE_OPERATION_FAILURE".to_string()),
                reconcile: None,
            })
            .expect("编码关闭恢复记录"),
        );
        db.save_codex_profile_route(&route)?;
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        manager.restore_enabled_profiles().await?;

        let restored =
            fs::read_to_string(codex_config_path_for_home(home.path())).expect("读取恢复后的 Home");
        assert_eq!(
            extract_codex_experimental_bearer_token(&restored).as_deref(),
            Some("upstream-token")
        );
        let route = db
            .get_codex_profile_route("profile-pending-disable")?
            .expect("路由仍存在");
        assert!(!route.enabled);
        assert!(route.recovery_json.is_none());
        Ok(())
    }

    /// pending 关闭遇到 Desktop 新增段时应字段级恢复并自动收敛。
    #[tokio::test]
    async fn pending_disable_preserves_codex_desktop_fields() -> Result<(), AppError> {
        use crate::codex_config::codex_config_path_for_home;

        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let home_config = Arc::new(CodexHomeConfigService::system());
        prepare_enabled_profile_home(
            &db,
            &home_config,
            home.path(),
            "profile-pending-desktop",
            16_001,
            "test-local-token",
        )?;
        let path = codex_config_path_for_home(home.path());
        let routed = fs::read_to_string(&path).expect("读取接管配置");
        fs::write(
            &path,
            format!(
                "js_repl = false\nmodel = \"latest-model\"\n{routed}\n[desktop]\nfollowUpQueueMode = \"queue\"\n\n[plugins.\"computer-use@openai-bundled\"]\nenabled = true\n\n[mcp_servers.node_repl]\ncommand = \"node_repl\"\n"
            ),
        )
        .expect("模拟 Codex Desktop 写入");
        let mut route = db
            .get_codex_profile_route("profile-pending-desktop")?
            .expect("路由存在");
        let before = RouteRecoverySnapshot::from_route(&route, vec![]);
        let mut target = before.clone();
        target.enabled = false;
        route.recovery_json = Some(
            serde_json::to_string(&RouteRecoveryRecord {
                operation: "disable".to_string(),
                before,
                target,
                phase: CODEX_ROUTE_RECOVERY_PHASE_DISABLE_HOME_RESTORE_FAILED.to_string(),
                last_error: Some("旧整文件指纹冲突".to_string()),
                reconcile: None,
            })
            .expect("编码关闭恢复记录"),
        );
        db.save_codex_profile_route(&route)?;
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        manager.restore_enabled_profiles().await?;

        let restored = fs::read_to_string(path).expect("读取恢复配置");
        assert!(restored.contains("js_repl = false"));
        assert!(restored.contains("model = \"latest-model\""));
        assert!(restored.contains("computer-use@openai-bundled"));
        assert!(restored.contains("mcp_servers.node_repl"));
        assert!(restored.contains("base_url = \"https://example.com/v1\""));
        assert!(!restored.contains("http://127.0.0.1:16001/v1"));
        let route = db
            .get_codex_profile_route("profile-pending-desktop")?
            .expect("路由存在");
        assert!(!route.enabled);
        assert!(route.recovery_json.is_none());
        Ok(())
    }

    /// 外部编辑必须原样保留并隔离失败，后续健康 Profile 仍能恢复监听器。
    #[tokio::test]
    async fn restoring_enabled_profile_home_preserves_external_edit_and_isolates_failure(
    ) -> Result<(), AppError> {
        use crate::codex_config::codex_config_path_for_home;

        let db = Arc::new(Database::memory()?);
        let home_config = Arc::new(CodexHomeConfigService::system());
        let bad_home = tempfile::tempdir().expect("外部编辑 Home");
        let good_home = tempfile::tempdir().expect("健康 Home");
        prepare_enabled_profile_home(
            &db,
            &home_config,
            bad_home.path(),
            "profile-bad-home",
            16_001,
            "old-listener-token",
        )?;
        prepare_enabled_profile_home(
            &db,
            &home_config,
            good_home.path(),
            "profile-good-home",
            16_002,
            "test-local-token",
        )?;
        let external = "model_provider = \"external\"\n\n[model_providers.external]\nname = \"External\"\nbase_url = \"https://external.example/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"external-token\"\n";
        fs::write(codex_config_path_for_home(bad_home.path()), external).expect("写入外部配置");
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config.clone(),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        manager.restore_enabled_profiles().await?;

        assert_eq!(
            fs::read_to_string(codex_config_path_for_home(bad_home.path())).expect("读取外部配置"),
            external
        );
        assert!(manager.status("profile-bad-home").await.is_err());
        assert!(db
            .get_codex_profile_route("profile-bad-home")?
            .expect("失败路由")
            .last_error
            .is_some());
        assert_eq!(
            manager.status("profile-good-home").await?,
            CodexRuntimeStatus::Running
        );
        Ok(())
    }

    /// Home 已写入但阶段保存失败时必须恢复旧路由目标，禁止留下无记录的新 token。
    #[tokio::test]
    async fn restoring_enabled_profile_home_compensates_when_phase_persistence_fails(
    ) -> Result<(), AppError> {
        use crate::codex_config::codex_config_path_for_home;

        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let home_config = Arc::new(CodexHomeConfigService::system());
        prepare_enabled_profile_home(
            &db,
            &home_config,
            home.path(),
            "profile-failing-save",
            16_001,
            "old-listener-token",
        )?;
        let old_content =
            fs::read(codex_config_path_for_home(home.path())).expect("读取旧路由目标");
        let manager = CodexRouteManager::new(
            Arc::new(SaveFailingPersistence {
                db: db.clone(),
                save_count: AtomicUsize::new(0),
                fail_on_save: 2,
                fail_replace: false,
            }),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        manager.restore_enabled_profiles().await?;

        assert_eq!(
            fs::read(codex_config_path_for_home(home.path())).expect("读取补偿后的 Home"),
            old_content
        );
        assert!(manager.status("profile-failing-save").await.is_err());
        let recovery_json = db
            .get_codex_profile_route("profile-failing-save")?
            .expect("失败路由仍存在")
            .recovery_json
            .expect("保留启动对账记录");
        assert!(!recovery_json.contains("test-local-token"));
        assert!(!recovery_json.contains("old-listener-token"));
        Ok(())
    }

    /// 崩溃后 Home 已是新目标时只需重定位备份并清除记录，不得回写旧 token。
    #[tokio::test]
    async fn restoring_enabled_profile_home_finalizes_applied_reconcile_after_crash(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let home_config = Arc::new(CodexHomeConfigService::system());
        let old_plan = prepare_enabled_profile_home(
            &db,
            &home_config,
            home.path(),
            "profile-crash",
            16_001,
            "old-listener-token",
        )?;
        let provider = db
            .get_provider_by_id("provider-profile-crash", AppType::Codex.as_str())?
            .expect("供应商存在");
        let desired_plan = home_config.build_profile_route_plan(
            home.path(),
            16_001,
            Some(&provider),
            "test-local-token",
        )?;
        home_config.apply_route_plan(&desired_plan)?;
        let mut route = db
            .get_codex_profile_route("profile-crash")?
            .expect("路由存在");
        let snapshot = RouteRecoverySnapshot::from_route(&route, vec![]);
        route.recovery_json = Some(
            serde_json::to_string(&RouteRecoveryRecord {
                operation: CODEX_ROUTE_RECOVERY_OPERATION_RECONCILE.to_string(),
                before: snapshot.clone(),
                target: snapshot,
                phase: CODEX_ROUTE_RECOVERY_PHASE_RECONCILE_HOME_APPLIED.to_string(),
                last_error: None,
                reconcile: Some(RouteReconcileTransition {
                    old_target_fingerprint: old_plan.target_fingerprint().to_string(),
                    new_target_fingerprint: desired_plan.target_fingerprint().to_string(),
                }),
            })
            .expect("编码启动对账记录"),
        );
        db.save_codex_profile_route(&route)?;
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config.clone(),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        manager.restore_enabled_profiles().await?;

        let route = db
            .get_codex_profile_route("profile-crash")?
            .expect("路由仍存在");
        assert!(route.recovery_json.is_none());
        let backup = route.live_backup_json.expect("备份仍存在");
        let current_plan = home_config.build_profile_route_plan(
            home.path(),
            16_001,
            Some(&provider),
            "test-local-token",
        )?;
        assert_eq!(
            home_config.classify_profile_reconcile(&current_plan, &backup, 16_001)?,
            CodexHomeReconcileOwnership::Current
        );
        assert_eq!(
            manager.status("profile-crash").await?,
            CodexRuntimeStatus::Running
        );
        Ok(())
    }

    /// 停止失败的运行时替身，用于验证关闭补偿记录。
    struct StopFailingRuntime;

    /// 仅拒绝 Home 原子写入，用于隔离启用 Home 写入失败路径。
    struct FailingHomeFileOps;

    /// 首次写入成功、后续写入失败，模拟 Home 已接管但补偿恢复失败。
    struct RestoreFailingHomeFileOps {
        writes: AtomicUsize,
    }

    impl crate::codex_profile::CodexHomeFileOps for FailingHomeFileOps {
        fn read(&self, path: &Path) -> Result<Option<Vec<u8>>, AppError> {
            if path.exists() {
                fs::read(path)
                    .map(Some)
                    .map_err(|error| AppError::io(path, error))
            } else {
                Ok(None)
            }
        }
        fn write_atomic(&self, _: &Path, _: &[u8]) -> Result<(), AppError> {
            Err(AppError::Message("模拟 Home 写入失败".to_string()))
        }
        fn remove_file(&self, _: &Path) -> Result<(), AppError> {
            Ok(())
        }
    }

    impl crate::codex_profile::CodexHomeFileOps for RestoreFailingHomeFileOps {
        fn read(&self, path: &Path) -> Result<Option<Vec<u8>>, AppError> {
            if path.exists() {
                fs::read(path)
                    .map(Some)
                    .map_err(|error| AppError::io(path, error))
            } else {
                Ok(None)
            }
        }
        fn write_atomic(&self, path: &Path, content: &[u8]) -> Result<(), AppError> {
            if self.writes.fetch_add(1, Ordering::SeqCst) == 0 {
                crate::config::atomic_write(path, content)
            } else {
                Err(AppError::Message("模拟 Home 恢复失败".to_string()))
            }
        }
        fn remove_file(&self, _: &Path) -> Result<(), AppError> {
            Err(AppError::Message("模拟 Home 恢复删除失败".to_string()))
        }
    }

    /// 可在测试中切换 Home 恢复失败状态，模拟修复外部文件问题后的重试。
    struct ToggleRestoreHomeFileOps {
        fail_writes: AtomicBool,
    }

    impl crate::codex_profile::CodexHomeFileOps for ToggleRestoreHomeFileOps {
        fn read(&self, path: &Path) -> Result<Option<Vec<u8>>, AppError> {
            if path.exists() {
                fs::read(path)
                    .map(Some)
                    .map_err(|error| AppError::io(path, error))
            } else {
                Ok(None)
            }
        }
        fn write_atomic(&self, path: &Path, content: &[u8]) -> Result<(), AppError> {
            if self.fail_writes.load(Ordering::SeqCst) {
                Err(AppError::Message("模拟 Home 恢复失败".to_string()))
            } else {
                crate::config::atomic_write(path, content)
            }
        }
        fn remove_file(&self, path: &Path) -> Result<(), AppError> {
            if self.fail_writes.load(Ordering::SeqCst) {
                Err(AppError::Message("模拟 Home 恢复删除失败".to_string()))
            } else if path.exists() {
                fs::remove_file(path).map_err(|error| AppError::io(path, error))
            } else {
                Ok(())
            }
        }
    }

    impl CodexRouteRuntime for StopFailingRuntime {
        fn start(&self) -> CodexRouteRuntimeFuture<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn health_check(&self) -> CodexRouteRuntimeFuture<'_, bool> {
            Box::pin(async { true })
        }
        fn swap_provider_snapshot(
            &self,
            _: CodexRouteProviderSnapshot,
        ) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async {})
        }
        fn reject_new_requests(&self) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async {})
        }
        fn stop(&self) -> CodexRouteRuntimeFuture<'_, Result<(), String>> {
            Box::pin(async { Err("模拟停止失败".to_string()) })
        }
        fn status(&self) -> CodexRouteRuntimeFuture<'_, CodexRuntimeStatus> {
            Box::pin(async {
                CodexRuntimeStatus::Failed {
                    message: "模拟停止失败".to_string(),
                }
            })
        }
    }

    /// 在恢复的首次路由读取后暂停，精确控制恢复与关闭的交错顺序。
    struct RestoreBlockingPersistence {
        db: Arc<Database>,
        route_reads: AtomicUsize,
        outer_read: Sender<()>,
        resume_restore: Mutex<Receiver<()>>,
    }

    impl CodexProfileRoutePersistence for RestoreBlockingPersistence {
        fn list_profiles(&self) -> Result<Vec<CodexProfile>, AppError> {
            self.db.list_codex_profiles()
        }
        fn get_profile(&self, profile_id: &str) -> Result<CodexProfile, AppError> {
            self.db.get_codex_profile(profile_id)
        }
        fn get_route(&self, profile_id: &str) -> Result<Option<CodexProfileRoute>, AppError> {
            if self.route_reads.fetch_add(1, Ordering::SeqCst) == 0 {
                let route = self.db.get_codex_profile_route(profile_id)?;
                self.outer_read.send(()).expect("恢复测试协调器仍在等待");
                self.resume_restore
                    .lock()
                    .expect("恢复测试协调锁")
                    .recv()
                    .expect("恢复测试继续信号");
                return Ok(route);
            }
            self.db.get_codex_profile_route(profile_id)
        }
        fn save_route(&self, route: &CodexProfileRoute) -> Result<(), AppError> {
            self.db.save_codex_profile_route(route)
        }
        fn list_failovers(&self, profile_id: &str) -> Result<Vec<String>, AppError> {
            self.db.list_codex_profile_failovers(profile_id)
        }
        fn replace_failovers(
            &self,
            profile_id: &str,
            provider_ids: &[String],
        ) -> Result<(), AppError> {
            self.db
                .replace_codex_profile_failovers(profile_id, provider_ids)
        }
        fn get_provider(&self, provider_id: &str) -> Result<Option<Provider>, AppError> {
            self.db
                .get_provider_by_id(provider_id, AppType::Codex.as_str())
        }
        fn delete_profile(&self, profile_id: &str) -> Result<(), AppError> {
            self.db.delete_codex_profile(profile_id)
        }
    }

    /// 记录恢复健康检查与停止动作的运行时替身。
    struct HealthRuntime {
        healthy: bool,
        stop_fails: bool,
        started: AtomicBool,
        stopped: AtomicBool,
    }

    impl CodexRouteRuntime for HealthRuntime {
        fn start(&self) -> CodexRouteRuntimeFuture<'_, Result<(), String>> {
            Box::pin(async move {
                self.started.store(true, Ordering::SeqCst);
                Ok(())
            })
        }
        fn health_check(&self) -> CodexRouteRuntimeFuture<'_, bool> {
            Box::pin(async move { self.healthy })
        }
        fn swap_provider_snapshot(
            &self,
            _: CodexRouteProviderSnapshot,
        ) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async {})
        }
        fn reject_new_requests(&self) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async {})
        }
        fn stop(&self) -> CodexRouteRuntimeFuture<'_, Result<(), String>> {
            Box::pin(async move {
                self.stopped.store(true, Ordering::SeqCst);
                if self.stop_fails {
                    Err("模拟停止失败".to_string())
                } else {
                    Ok(())
                }
            })
        }
        fn status(&self) -> CodexRouteRuntimeFuture<'_, CodexRuntimeStatus> {
            Box::pin(async { CodexRuntimeStatus::Running })
        }
    }

    /// 按 Profile 返回预设健康状态的运行时工厂。
    struct HealthFactory {
        runtimes: HashMap<String, Arc<HealthRuntime>>,
    }

    impl CodexRouteRuntimeFactory for HealthFactory {
        fn create(
            &self,
            scope: CodexProfileScope,
            _: String,
            _: CodexRouteProviderSnapshot,
        ) -> Arc<dyn CodexRouteRuntime> {
            self.runtimes
                .get(&scope.profile_id)
                .expect("已配置 Profile 运行时")
                .clone()
        }
    }

    /// 可切换停止结果并记录停接动作的运行时替身。
    // 历史说明：该替身原本用于记录排空过程。
    struct RecoveryRuntime {
        healthy: bool,
        stop_fails: AtomicBool,
        reject_new_requests_calls: AtomicUsize,
    }

    impl CodexRouteRuntime for RecoveryRuntime {
        fn start(&self) -> CodexRouteRuntimeFuture<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn health_check(&self) -> CodexRouteRuntimeFuture<'_, bool> {
            Box::pin(async move { self.healthy })
        }
        fn swap_provider_snapshot(
            &self,
            _: CodexRouteProviderSnapshot,
        ) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async {})
        }
        fn reject_new_requests(&self) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async move {
                self.reject_new_requests_calls
                    .fetch_add(1, Ordering::SeqCst);
            })
        }
        fn stop(&self) -> CodexRouteRuntimeFuture<'_, Result<(), String>> {
            Box::pin(async move {
                if self.stop_fails.load(Ordering::SeqCst) {
                    Err("模拟停止失败".to_string())
                } else {
                    Ok(())
                }
            })
        }
        fn status(&self) -> CodexRouteRuntimeFuture<'_, CodexRuntimeStatus> {
            Box::pin(async { CodexRuntimeStatus::Running })
        }
    }

    /// 复用同一可恢复运行时的工厂，方便验证失败后的后续收敛。
    struct RecoveryRuntimeFactory {
        runtime: Arc<RecoveryRuntime>,
    }

    impl CodexRouteRuntimeFactory for RecoveryRuntimeFactory {
        fn create(
            &self,
            _: CodexProfileScope,
            _: String,
            _: CodexRouteProviderSnapshot,
        ) -> Arc<dyn CodexRouteRuntime> {
            self.runtime.clone()
        }
    }

    /// 用于验证删除失败不会误删 Profile 记录的本地凭证替身。
    struct FailingDeleteTokenStore;

    impl CodexProfileTokenStore for FailingDeleteTokenStore {
        fn read_token(&self, _: &str) -> Result<Option<String>, AppError> {
            Ok(Some("test-local-token".to_string()))
        }

        fn ensure_token(&self, _: &str) -> Result<String, AppError> {
            Ok("test-local-token".to_string())
        }

        fn delete_token(&self, _: &str) -> Result<(), AppError> {
            Err(AppError::Message("模拟本地凭证删除失败".to_string()))
        }
    }

    /// 最终清理操作记录保存失败时，不能把切换伪装成完全成功。
    #[tokio::test]
    async fn switching_clear_operation_save_failure_keeps_pending_phase() -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        for id in ["provider-a", "provider-a-next", "provider-b"] {
            db.save_provider(
                AppType::Codex.as_str(),
                &Provider::with_id(id.to_string(), id.to_string(), json!({}), None),
            )?;
        }
        for (id, port) in [("profile-a", 16001), ("profile-b", 16002)] {
            db.insert_codex_profile(&CodexProfile {
                id: id.to_string(),
                name: id.to_string(),
                canonical_home_path: format!("/tmp/{id}"),
                listen_port: port,
                created_at: 1,
                updated_at: 1,
            })?;
            db.save_codex_profile_route(&CodexProfileRoute {
                profile_id: id.to_string(),
                current_provider_id: Some(if id == "profile-a" {
                    "provider-a".to_string()
                } else {
                    "provider-b".to_string()
                }),
                enabled: true,
                live_backup_json: None,
                last_error: None,
                recovery_json: None,
                updated_at: 1,
            })?;
        }
        let manager = CodexRouteManager::new(
            Arc::new(SaveFailingPersistence {
                db: db.clone(),
                save_count: AtomicUsize::new(0),
                fail_on_save: 6,
                fail_replace: false,
            }),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(CodexProfileSecretStore::with_root(
                tempfile::tempdir().expect("临时 token 目录").keep(),
            )),
            Arc::new(FakeFactory),
        );
        let runtime_a = Arc::new(FakeRuntime {
            provider: AsyncMutex::new("provider-a".to_string()),
            port: 16001,
        });
        let runtime_b = Arc::new(FakeRuntime {
            provider: AsyncMutex::new("provider-b".to_string()),
            port: 16002,
        });
        manager
            .runtimes
            .lock()
            .expect("运行时锁")
            .insert("profile-a".to_string(), runtime_a.clone());
        manager
            .runtimes
            .lock()
            .expect("运行时锁")
            .insert("profile-b".to_string(), runtime_b.clone());
        assert!(manager
            .switch_provider("profile-a", "provider-a-next", vec![])
            .await
            .is_err());
        assert_eq!(*runtime_a.provider.lock().await, "provider-a-next");
        assert_eq!(*runtime_b.provider.lock().await, "provider-b");
        assert_eq!(runtime_b.port, 16002);
        let route = db.get_codex_profile_route("profile-a")?.expect("路由记录");
        let recovery: serde_json::Value =
            serde_json::from_str(&route.recovery_json.expect("未清理的操作记录"))
                .expect("操作 JSON");
        assert_eq!(
            recovery["phase"],
            CODEX_ROUTE_RECOVERY_PHASE_SWITCH_FAILOVERS_SAVED
        );
        assert!(recovery["last_error"].is_string());
        assert!(manager
            .switch_provider("profile-a", "provider-a-next", vec![])
            .await
            .is_ok());
        Ok(())
    }

    /// 故障转移列表保存失败时，运行时回滚且数据库保留可拒绝的切换操作记录。
    #[tokio::test]
    async fn switching_failover_save_failure_keeps_recovery_and_rejects_next_mutation(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        for id in ["provider-old", "provider-new", "provider-failover"] {
            db.save_provider(
                AppType::Codex.as_str(),
                &Provider::with_id(id.to_string(), id.to_string(), json!({}), None),
            )?;
        }
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: "/tmp/profile-a".to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: Some("provider-old".to_string()),
            enabled: true,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        db.replace_codex_profile_failovers("profile-a", &["provider-failover".to_string()])?;
        let persistence = Arc::new(SaveFailingPersistence {
            db: db.clone(),
            save_count: AtomicUsize::new(0),
            fail_on_save: 0,
            fail_replace: true,
        });
        let manager = CodexRouteManager::new(
            persistence,
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(CodexProfileSecretStore::with_root(
                tempfile::tempdir().expect("临时 token 目录").keep(),
            )),
            Arc::new(FakeFactory),
        );
        let runtime = Arc::new(FakeRuntime {
            provider: AsyncMutex::new("provider-old".to_string()),
            port: 16001,
        });
        manager
            .runtimes
            .lock()
            .expect("运行时锁")
            .insert("profile-a".to_string(), runtime.clone());

        assert!(manager
            .switch_provider("profile-a", "provider-new", vec![])
            .await
            .is_err());
        assert_eq!(*runtime.provider.lock().await, "provider-old");
        let route = db
            .get_codex_profile_route("profile-a")?
            .expect("路由仍存在");
        let recovery: serde_json::Value =
            serde_json::from_str(&route.recovery_json.expect("切换操作记录")).expect("操作 JSON");
        assert_eq!(recovery["operation"], "switch");
        assert_eq!(
            recovery["phase"],
            CODEX_ROUTE_RECOVERY_PHASE_SWITCH_ROUTE_SAVED
        );
        assert!(recovery["last_error"].is_string());
        assert!(manager
            .switch_provider("profile-a", "provider-new", vec![])
            .await
            .is_err());
        Ok(())
    }

    /// 切换失败后连续持久化故障时，调用方必须收到未收敛错误且操作记录不得被清除。
    #[tokio::test]
    async fn switching_consecutive_persistence_failures_keep_operation_and_report_unconverged(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        for id in ["provider-old", "provider-new"] {
            db.save_provider(
                AppType::Codex.as_str(),
                &Provider::with_id(id.to_string(), id.to_string(), json!({}), None),
            )?;
        }
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: "/tmp/profile-a".to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: Some("provider-old".to_string()),
            enabled: true,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        let manager = CodexRouteManager::new(
            Arc::new(SaveSequenceFailingPersistence {
                db: db.clone(),
                save_count: AtomicUsize::new(0),
                failed_saves: vec![3, 4, 5],
            }),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );
        let runtime = Arc::new(FakeRuntime {
            provider: AsyncMutex::new("provider-old".to_string()),
            port: 16001,
        });
        manager
            .runtimes
            .lock()
            .expect("运行时锁")
            .insert("profile-a".to_string(), runtime.clone());

        let error = manager
            .switch_provider("profile-a", "provider-new", vec![])
            .await
            .expect_err("切换应失败");
        assert!(error.to_string().contains("补偿未收敛"));
        assert_eq!(*runtime.provider.lock().await, "provider-old");
        let route = db.get_codex_profile_route("profile-a")?.expect("路由记录");
        assert!(
            route.recovery_json.is_some(),
            "连续保存失败时不得清除操作记录"
        );
        assert!(manager
            .switch_provider("profile-a", "provider-new", vec![])
            .await
            .is_err());
        Ok(())
    }

    /// 启用写入路由失败后必须保存无敏感数据的补偿阶段，后续操作会先拒绝变更。
    #[tokio::test]
    async fn enabling_route_save_failure_persists_sanitized_recovery_phase() -> Result<(), AppError>
    {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id(
                "provider-a".to_string(),
                "Provider A".to_string(),
                json!({}),
                None,
            ),
        )?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let manager = CodexRouteManager::new(
            Arc::new(SaveFailingPersistence {
                db: db.clone(),
                save_count: AtomicUsize::new(0),
                fail_on_save: 5,
                fail_replace: false,
            }),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(CodexProfileSecretStore::with_root(
                tempfile::tempdir().expect("临时 token 目录").keep(),
            )),
            Arc::new(FakeFactory),
        );

        assert!(manager
            .enable("profile-a", "provider-a", vec![])
            .await
            .is_err());
        let route = db
            .get_codex_profile_route("profile-a")?
            .expect("已保存补偿记录");
        let recovery = route.recovery_json.expect("补偿记录");
        let parsed: serde_json::Value = serde_json::from_str(&recovery).expect("补偿 JSON");
        assert_eq!(
            parsed["phase"],
            CODEX_ROUTE_RECOVERY_PHASE_ENABLE_PERSIST_FAILED
        );
        assert_eq!(parsed["operation"], "enable");
        assert!(parsed["before"].is_object());
        assert!(parsed["target"].is_object());
        assert!(parsed["last_error"].is_string());
        assert!(!recovery.contains("test-local-token"));
        assert!(!recovery.contains(&home.path().display().to_string()));
        assert!(!recovery.contains("auth"));
        assert!(manager
            .switch_provider("profile-a", "provider-a", vec![])
            .await
            .is_err());
        Ok(())
    }

    /// 启用最终保存失败且 Home、监听器补偿都失败时，必须托管残留运行时并持久化安全恢复信息。
    #[tokio::test]
    async fn enabling_final_save_with_home_and_stop_failure_keeps_runtime_and_recovery(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id(
                "provider-a".to_string(),
                "Provider A".to_string(),
                json!({}),
                None,
            ),
        )?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let runtime = Arc::new(HealthRuntime {
            healthy: true,
            stop_fails: true,
            started: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        });
        let manager = CodexRouteManager::new(
            Arc::new(SaveFailingPersistence {
                db: db.clone(),
                save_count: AtomicUsize::new(0),
                fail_on_save: 5,
                fail_replace: false,
            }),
            Arc::new(CodexHomeConfigService::new(Arc::new(
                RestoreFailingHomeFileOps {
                    writes: AtomicUsize::new(0),
                },
            ))),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(HealthFactory {
                runtimes: HashMap::from([("profile-a".to_string(), runtime.clone())]),
            }),
        );

        assert!(manager
            .enable("profile-a", "provider-a", vec![])
            .await
            .is_err());
        let route = db.get_codex_profile_route("profile-a")?.expect("恢复记录");
        let recovery: serde_json::Value =
            serde_json::from_str(&route.recovery_json.expect("操作记录")).expect("操作 JSON");
        assert_eq!(recovery["operation"], "enable");
        assert_eq!(
            recovery["phase"],
            CODEX_ROUTE_RECOVERY_PHASE_ENABLE_PERSIST_FAILED
        );
        assert_eq!(recovery["last_error"], "CODEX_PROFILE_OPERATION_FAILURE");
        assert_eq!(
            manager.status("profile-a").await?,
            CodexRuntimeStatus::Running
        );
        assert!(runtime.stopped.load(Ordering::SeqCst));
        assert!(manager.disable("profile-a").await.is_err());
        Ok(())
    }

    /// 启用残留的 Home 与运行时可在后续启动恢复中收敛，并还原真实的原始 Home 配置。
    #[tokio::test]
    async fn restoring_enable_residue_restores_home_and_stops_tracked_runtime(
    ) -> Result<(), AppError> {
        use crate::codex_config::codex_config_path_for_home;

        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let home_config = Arc::new(CodexHomeConfigService::system());
        let config_path = codex_config_path_for_home(home.path());
        fs::write(&config_path, "model = \"before\"\n").expect("写入原始配置");
        let plan = home_config.build_route_plan(home.path(), "model = \"route\"\n")?;
        home_config.apply_route_plan(&plan)?;
        let backup = home_config.serialize_backup(&plan)?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let recovery = RouteRecoveryRecord {
            operation: "enable".to_string(),
            before: RouteRecoverySnapshot {
                current_provider_id: None,
                enabled: false,
                failover_ids: vec![],
            },
            target: RouteRecoverySnapshot {
                current_provider_id: Some("provider-a".to_string()),
                enabled: true,
                failover_ids: vec![],
            },
            phase: CODEX_ROUTE_RECOVERY_PHASE_ENABLE_HOME_APPLIED.to_string(),
            last_error: Some("CODEX_PROFILE_OPERATION_FAILURE".to_string()),
            reconcile: None,
        };
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: None,
            enabled: false,
            live_backup_json: Some(backup),
            last_error: recovery.last_error.clone(),
            recovery_json: Some(serde_json::to_string(&recovery).expect("编码恢复记录")),
            updated_at: 1,
        })?;
        let runtime = Arc::new(HealthRuntime {
            healthy: true,
            stop_fails: false,
            started: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
        });
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(HealthFactory {
                runtimes: HashMap::from([("profile-a".to_string(), runtime.clone())]),
            }),
        );
        manager
            .runtimes
            .lock()
            .expect("运行时锁")
            .insert("profile-a".to_string(), runtime.clone());

        manager.restore_enabled_profiles().await?;

        assert_eq!(
            fs::read_to_string(config_path).expect("读取恢复配置"),
            "model = \"before\"\n"
        );
        assert!(runtime.stopped.load(Ordering::SeqCst));
        let route = db.get_codex_profile_route("profile-a")?.expect("路由记录");
        assert!(!route.enabled);
        assert!(route.recovery_json.is_none());
        assert!(manager.status("profile-a").await.is_err());
        Ok(())
    }

    /// 启动阶段的备份持久化失败尚未改写 Home，恢复只能清理操作记录而不得覆盖当前 Home。
    #[tokio::test]
    async fn restoring_enable_started_without_backup_does_not_restore_home() -> Result<(), AppError>
    {
        use crate::codex_config::codex_config_path_for_home;

        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let config_path = codex_config_path_for_home(home.path());
        fs::write(&config_path, "model = \"current\"\n").expect("写入当前配置");
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let recovery = RouteRecoveryRecord {
            operation: "enable".to_string(),
            before: RouteRecoverySnapshot {
                current_provider_id: None,
                enabled: false,
                failover_ids: vec![],
            },
            target: RouteRecoverySnapshot {
                current_provider_id: Some("provider-a".to_string()),
                enabled: true,
                failover_ids: vec![],
            },
            phase: CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED.to_string(),
            last_error: Some("CODEX_PROFILE_OPERATION_FAILURE".to_string()),
            reconcile: None,
        };
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: None,
            enabled: false,
            live_backup_json: None,
            last_error: recovery.last_error.clone(),
            recovery_json: Some(serde_json::to_string(&recovery).expect("编码恢复记录")),
            updated_at: 1,
        })?;
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        manager.restore_enabled_profiles().await?;

        assert_eq!(
            fs::read_to_string(config_path).expect("读取当前配置"),
            "model = \"current\"\n"
        );
        assert!(db
            .get_codex_profile_route("profile-a")?
            .expect("路由记录")
            .recovery_json
            .is_none());
        Ok(())
    }

    /// 切换补偿无法读取运行时时必须保留原记录，绝不能写回旧数据库状态。
    #[tokio::test]
    async fn restoring_switch_without_runtime_keeps_recovery_and_reports_unconverged(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        for provider_id in ["provider-old", "provider-new"] {
            db.save_provider(
                AppType::Codex.as_str(),
                &Provider::with_id(
                    provider_id.to_string(),
                    provider_id.to_string(),
                    json!({}),
                    None,
                ),
            )?;
        }
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: "/tmp/profile-a".to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let recovery_json = serde_json::to_string(&RouteRecoveryRecord {
            operation: "switch".to_string(),
            before: RouteRecoverySnapshot {
                current_provider_id: Some("provider-old".to_string()),
                enabled: true,
                failover_ids: vec![],
            },
            target: RouteRecoverySnapshot {
                current_provider_id: Some("provider-new".to_string()),
                enabled: true,
                failover_ids: vec![],
            },
            phase: CODEX_ROUTE_RECOVERY_PHASE_SWITCH_RUNTIME_SWAPPED.to_string(),
            last_error: None,
            reconcile: None,
        })
        .expect("编码恢复记录");
        let route = CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: Some("provider-new".to_string()),
            enabled: true,
            live_backup_json: None,
            last_error: None,
            recovery_json: Some(recovery_json.clone()),
            updated_at: 1,
        };
        db.save_codex_profile_route(&route)?;
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        let error = manager
            .restore_switch_locked(
                "profile-a",
                &route,
                &[],
                manager.provider_snapshot("provider-old", &[])?,
            )
            .await
            .expect_err("运行时不可访问时补偿不得写回旧状态");

        assert!(error
            .to_string()
            .contains(CODEX_ROUTE_COMPENSATION_UNCONVERGED_ERROR));
        let persisted = db.get_codex_profile_route("profile-a")?.expect("路由记录");
        assert_eq!(
            persisted.current_provider_id.as_deref(),
            Some("provider-new")
        );
        assert_eq!(
            persisted.recovery_json.as_deref(),
            Some(recovery_json.as_str())
        );
        Ok(())
    }

    /// 公共切换入口遇到 prepared 阶段且缺失运行时的未完成切换时，必须保持持久化状态等待后续补偿。
    #[tokio::test]
    async fn switching_with_pending_switch_without_runtime_keeps_route_failovers_and_recovery(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        for provider_id in ["provider-old", "provider-new", "provider-requested"] {
            db.save_provider(
                AppType::Codex.as_str(),
                &Provider::with_id(
                    provider_id.to_string(),
                    provider_id.to_string(),
                    json!({}),
                    None,
                ),
            )?;
        }
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: "/tmp/profile-a".to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let recovery_json = serde_json::to_string(&RouteRecoveryRecord {
            operation: "switch".to_string(),
            before: RouteRecoverySnapshot {
                current_provider_id: Some("provider-old".to_string()),
                enabled: true,
                failover_ids: vec!["provider-old".to_string()],
            },
            target: RouteRecoverySnapshot {
                current_provider_id: Some("provider-new".to_string()),
                enabled: true,
                failover_ids: vec!["provider-new".to_string()],
            },
            phase: CODEX_ROUTE_RECOVERY_PHASE_PREPARED.to_string(),
            last_error: None,
            reconcile: None,
        })
        .expect("编码恢复记录");
        let route = CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: Some("provider-new".to_string()),
            enabled: true,
            live_backup_json: None,
            last_error: None,
            recovery_json: Some(recovery_json.clone()),
            updated_at: 1,
        };
        db.save_codex_profile_route(&route)?;
        db.replace_codex_profile_failovers("profile-a", &["provider-new".to_string()])?;
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        let error = manager
            .switch_provider("profile-a", "provider-requested", vec![])
            .await
            .expect_err("未收敛的切换不能继续执行新切换");

        assert!(error
            .to_string()
            .contains(CODEX_ROUTE_COMPENSATION_UNCONVERGED_ERROR));
        let recovered_route = db.get_codex_profile_route("profile-a")?.expect("路由记录");
        assert_eq!(
            recovered_route.current_provider_id,
            route.current_provider_id
        );
        assert_eq!(recovered_route.enabled, route.enabled);
        assert_eq!(recovered_route.live_backup_json, route.live_backup_json);
        assert_eq!(recovered_route.recovery_json, route.recovery_json);
        assert_eq!(recovered_route.last_error, route.last_error);
        assert_eq!(
            db.list_codex_profile_failovers("profile-a")?,
            vec!["provider-new".to_string()]
        );
        assert_eq!(
            db.get_codex_profile_route("profile-a")?
                .expect("路由记录")
                .recovery_json
                .as_deref(),
            Some(recovery_json.as_str())
        );
        Ok(())
    }

    /// 启动恢复遇到 prepared 阶段的切换残留时，不得改写 Home、停止运行时或伪装成已关闭。
    #[tokio::test]
    async fn restoring_prepared_switch_without_runtime_keeps_route_and_recovery(
    ) -> Result<(), AppError> {
        use crate::codex_config::codex_config_path_for_home;

        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let config_path = codex_config_path_for_home(home.path());
        fs::write(&config_path, "model = \"current\"\n").expect("写入当前配置");
        for provider_id in ["provider-old", "provider-new"] {
            db.save_provider(
                AppType::Codex.as_str(),
                &Provider::with_id(
                    provider_id.to_string(),
                    provider_id.to_string(),
                    json!({}),
                    None,
                ),
            )?;
        }
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let recovery_json = serde_json::to_string(&RouteRecoveryRecord {
            operation: "switch".to_string(),
            before: RouteRecoverySnapshot {
                current_provider_id: Some("provider-old".to_string()),
                enabled: true,
                failover_ids: vec![],
            },
            target: RouteRecoverySnapshot {
                current_provider_id: Some("provider-new".to_string()),
                enabled: true,
                failover_ids: vec![],
            },
            phase: CODEX_ROUTE_RECOVERY_PHASE_PREPARED.to_string(),
            last_error: None,
            reconcile: None,
        })
        .expect("编码恢复记录");
        let route = CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: Some("provider-new".to_string()),
            enabled: true,
            live_backup_json: None,
            last_error: None,
            recovery_json: Some(recovery_json.clone()),
            updated_at: 1,
        };
        db.save_codex_profile_route(&route)?;
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        manager.restore_enabled_profiles().await?;

        assert_eq!(
            fs::read_to_string(config_path).expect("读取当前配置"),
            "model = \"current\"\n"
        );
        let recovered_route = db.get_codex_profile_route("profile-a")?.expect("路由记录");
        assert_eq!(
            recovered_route.current_provider_id,
            route.current_provider_id
        );
        assert_eq!(recovered_route.enabled, route.enabled);
        assert_eq!(recovered_route.live_backup_json, route.live_backup_json);
        assert_eq!(recovered_route.recovery_json, route.recovery_json);
        assert!(recovered_route.last_error.is_some());
        assert!(manager.status("profile-a").await.is_err());
        Ok(())
    }

    /// 切换补偿记录解析失败时不得改写当前路由或清除原始记录。
    #[tokio::test]
    async fn restoring_switch_with_invalid_recovery_keeps_current_route_unchanged(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id(
                "provider-old".to_string(),
                "provider-old".to_string(),
                json!({}),
                None,
            ),
        )?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: "/tmp/profile-a".to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let route = CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: Some("provider-old".to_string()),
            enabled: true,
            live_backup_json: None,
            last_error: None,
            recovery_json: Some("{".to_string()),
            updated_at: 1,
        };
        db.save_codex_profile_route(&route)?;
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        assert!(manager
            .restore_switch_locked(
                "profile-a",
                &route,
                &[],
                manager.provider_snapshot("provider-old", &[])?,
            )
            .await
            .is_err());

        let persisted = db.get_codex_profile_route("profile-a")?.expect("路由记录");
        assert_eq!(persisted.current_provider_id, route.current_provider_id);
        assert_eq!(persisted.recovery_json, route.recovery_json);
        Ok(())
    }

    /// 健康检查失败且停止失败时，必须保留启用操作并拒绝同 Profile 的后续变更。
    #[tokio::test]
    async fn enabling_health_failure_with_stop_failure_keeps_operation_and_rejects_mutation(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id(
                "provider-a".to_string(),
                "Provider A".to_string(),
                json!({}),
                None,
            ),
        )?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let runtime = Arc::new(HealthRuntime {
            healthy: false,
            stop_fails: true,
            started: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        });
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(HealthFactory {
                runtimes: HashMap::from([("profile-a".to_string(), runtime)]),
            }),
        );

        assert!(manager
            .enable("profile-a", "provider-a", vec![])
            .await
            .is_err());
        let recovery: serde_json::Value = serde_json::from_str(
            &db.get_codex_profile_route("profile-a")?
                .expect("路由记录")
                .recovery_json
                .expect("操作记录"),
        )
        .expect("操作 JSON");
        assert_eq!(recovery["operation"], "enable");
        assert_eq!(recovery["phase"], CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED);
        assert_eq!(recovery["last_error"], "CODEX_PROFILE_OPERATION_FAILURE");
        assert!(manager
            .switch_provider("profile-a", "provider-a", vec![])
            .await
            .is_err());
        Ok(())
    }

    /// 启动阶段记录写入失败且停止失败后，恢复必须停接并移除被托管的运行时。
    // 历史说明：该场景原本要求恢复先排空再移除被托管的运行时。
    #[tokio::test]
    async fn enabling_started_persistence_failure_tracks_runtime_until_recovery_stops_it(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id("provider-a".to_string(), "A".to_string(), json!({}), None),
        )?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let runtime = Arc::new(RecoveryRuntime {
            healthy: true,
            stop_fails: AtomicBool::new(true),
            reject_new_requests_calls: AtomicUsize::new(0),
        });
        let manager = CodexRouteManager::new(
            Arc::new(SaveFailingPersistence {
                db: db.clone(),
                save_count: AtomicUsize::new(0),
                fail_on_save: 2,
                fail_replace: false,
            }),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(RecoveryRuntimeFactory {
                runtime: runtime.clone(),
            }),
        );

        assert!(manager
            .enable("profile-a", "provider-a", vec![])
            .await
            .is_err());
        assert_eq!(
            manager.status("profile-a").await?,
            CodexRuntimeStatus::Running
        );
        runtime.stop_fails.store(false, Ordering::SeqCst);

        manager.recover_pending_locked("profile-a").await?;

        assert_eq!(runtime.reject_new_requests_calls.load(Ordering::SeqCst), 2);
        assert!(manager.status("profile-a").await.is_err());
        Ok(())
    }

    /// 健康检查失败且停止失败后，恢复必须继续托管运行时并采用即时停接关闭流程。
    // 历史说明：该测试原本采用完整排空关闭流程。
    #[tokio::test]
    async fn enabling_health_failure_tracks_runtime_until_recovery_rejects_and_stops(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id("provider-a".to_string(), "A".to_string(), json!({}), None),
        )?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let runtime = Arc::new(RecoveryRuntime {
            healthy: false,
            stop_fails: AtomicBool::new(true),
            reject_new_requests_calls: AtomicUsize::new(0),
        });
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(RecoveryRuntimeFactory {
                runtime: runtime.clone(),
            }),
        );

        assert!(manager
            .enable("profile-a", "provider-a", vec![])
            .await
            .is_err());
        assert_eq!(
            manager.status("profile-a").await?,
            CodexRuntimeStatus::Running
        );
        runtime.stop_fails.store(false, Ordering::SeqCst);

        manager.recover_pending_locked("profile-a").await?;

        assert_eq!(runtime.reject_new_requests_calls.load(Ordering::SeqCst), 2);
        assert!(manager.status("profile-a").await.is_err());
        Ok(())
    }

    /// 启用残留补偿必须立即停接，不得等待在途路由请求。
    #[tokio::test]
    async fn recovering_enable_residue_does_not_wait_for_in_flight_request() -> Result<(), AppError>
    {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        db.insert_codex_profile(&CodexProfile {
            id: "profile-enable-residue".to_string(),
            name: "Enable Residue".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16_001,
            created_at: 1,
            updated_at: 1,
        })?;
        let recovery = RouteRecoveryRecord {
            operation: "enable".to_string(),
            before: RouteRecoverySnapshot {
                current_provider_id: None,
                enabled: false,
                failover_ids: vec![],
            },
            target: RouteRecoverySnapshot {
                current_provider_id: Some("provider-a".to_string()),
                enabled: true,
                failover_ids: vec![],
            },
            phase: CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED.to_string(),
            last_error: None,
            reconcile: None,
        };
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "profile-enable-residue".to_string(),
            current_provider_id: None,
            enabled: false,
            live_backup_json: None,
            last_error: None,
            recovery_json: Some(serde_json::to_string(&recovery).expect("编码恢复记录")),
            updated_at: 1,
        })?;
        let runtime = Arc::new(InFlightRequestRuntime {
            stop_called: AtomicBool::new(false),
        });
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );
        manager.track_runtime("profile-enable-residue".to_string(), runtime.clone())?;

        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            manager.recover_pending_locked("profile-enable-residue"),
        )
        .await
        .expect("启用残留补偿不应等待在途路由请求")?;

        assert!(runtime.stop_called.load(Ordering::SeqCst));
        assert!(manager.status("profile-enable-residue").await.is_err());
        let route = db
            .get_codex_profile_route("profile-enable-residue")?
            .expect("路由存在");
        assert!(!route.enabled);
        assert!(route.recovery_json.is_none());
        Ok(())
    }

    /// 历史排空超时会记录排空状态；即时停接后，停止失败仍必须保留恢复记录并返回错误。
    // 历史说明：恢复关闭即使排空超时也会调用等待，并在停止失败时返回可观测的排空状态。
    #[tokio::test]
    async fn recovering_enable_residue_keeps_recovery_when_stop_still_fails() -> Result<(), AppError>
    {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let recovery = RouteRecoveryRecord {
            operation: "enable".to_string(),
            before: RouteRecoverySnapshot {
                current_provider_id: None,
                enabled: false,
                failover_ids: vec![],
            },
            target: RouteRecoverySnapshot {
                current_provider_id: Some("provider-a".to_string()),
                enabled: true,
                failover_ids: vec![],
            },
            phase: CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED.to_string(),
            last_error: None,
            reconcile: None,
        };
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: None,
            enabled: false,
            live_backup_json: None,
            last_error: None,
            recovery_json: Some(serde_json::to_string(&recovery).expect("编码恢复记录")),
            updated_at: 1,
        })?;
        let runtime = Arc::new(RecoveryRuntime {
            healthy: true,
            stop_fails: AtomicBool::new(true),
            reject_new_requests_calls: AtomicUsize::new(0),
        });
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );
        manager.track_runtime("profile-a".to_string(), runtime.clone())?;

        let error = manager
            .recover_pending_locked("profile-a")
            .await
            .expect_err("停止失败时恢复记录不得被清除");

        assert_eq!(runtime.reject_new_requests_calls.load(Ordering::SeqCst), 1);
        assert!(error.to_string().contains("模拟停止失败"));
        assert!(db
            .get_codex_profile_route("profile-a")?
            .expect("路由记录")
            .recovery_json
            .is_some());
        Ok(())
    }

    /// 操作错误摘要不能把 token、认证字段或 Home 路径写入恢复记录。
    #[test]
    fn operation_error_summary_redacts_token_auth_and_home_path() {
        for sensitive in [
            "token=secret-value",
            "auth header: Bearer secret-value",
            "/Users/example/.codex/config.toml",
            "C:\\Users\\example\\config.toml",
        ] {
            let summary = CodexRouteManager::safe_error_summary(sensitive);
            assert!(!summary.contains("secret-value"));
            assert!(!summary.contains("/Users"));
            assert!(!summary.contains("C:\\Users"));
        }
    }

    /// 非法恢复记录必须在任何运行时或凭证副作用之前被拒绝并原样保留。
    #[test]
    fn recovery_record_validation_rejects_malformed_unknown_and_mismatched_states() {
        let manager = CodexRouteManager::new(
            Arc::new(Database::memory().expect("内存数据库")),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );
        let snapshot = RouteRecoverySnapshot {
            current_provider_id: None,
            enabled: false,
            failover_ids: vec![],
        };
        for record in [
            RouteRecoveryRecord {
                operation: "unknown".to_string(),
                before: snapshot.clone(),
                target: snapshot.clone(),
                phase: CODEX_ROUTE_RECOVERY_PHASE_PREPARED.to_string(),
                last_error: None,
                reconcile: None,
            },
            RouteRecoveryRecord {
                operation: "switch".to_string(),
                before: snapshot.clone(),
                target: snapshot.clone(),
                phase: CODEX_ROUTE_RECOVERY_PHASE_DELETE_TOKEN_PENDING.to_string(),
                last_error: None,
                reconcile: None,
            },
        ] {
            assert!(manager.validate_recovery_record(&record).is_err());
        }
        assert!(serde_json::from_str::<RouteRecoveryRecord>("{").is_err());
    }

    /// manager 入口遇到非法恢复 JSON 时不得启动运行时、创建 token 或改写原记录。
    #[tokio::test]
    async fn switching_with_invalid_recovery_keeps_json_and_has_no_side_effects(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id("provider-a".to_string(), "A".to_string(), json!({}), None),
        )?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "A".to_string(),
            canonical_home_path: "/tmp/a".to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let tokens = Arc::new(TrackingTokenStore {
            ensured: AtomicUsize::new(0),
            deleted: AtomicUsize::new(0),
        });
        for json in [
            "{",
            r#"{"operation":"unknown","phase":"prepared","before":{"current_provider_id":null,"enabled":false,"failover_ids":[]},"target":{"current_provider_id":null,"enabled":false,"failover_ids":[]},"last_error":null}"#,
            r#"{"operation":"switch","phase":"delete_token_pending","before":{"current_provider_id":null,"enabled":false,"failover_ids":[]},"target":{"current_provider_id":null,"enabled":false,"failover_ids":[]},"last_error":null}"#,
            r#"{"operation":"switch","phase":"prepared","before":{"current_provider_id":null,"enabled":false,"failover_ids":[],"token":"secret"},"target":{"current_provider_id":null,"enabled":false,"failover_ids":[]},"last_error":null,"auth":"secret","home":"secret"}"#,
        ] {
            db.save_codex_profile_route(&CodexProfileRoute {
                profile_id: "profile-a".to_string(),
                current_provider_id: Some("provider-a".to_string()),
                enabled: true,
                live_backup_json: None,
                last_error: None,
                recovery_json: Some(json.to_string()),
                updated_at: 1,
            })?;
            let manager = CodexRouteManager::new(
                db.clone(),
                Arc::new(CodexHomeConfigService::system()),
                tokens.clone(),
                Arc::new(FakeFactory),
            );
            let error = manager
                .switch_provider("profile-a", "provider-a", vec![])
                .await
                .expect_err("非法恢复记录不能继续切换");
            assert!(error
                .to_string()
                .contains(CODEX_ROUTE_COMPENSATION_UNCONVERGED_ERROR));
            assert_eq!(
                db.get_codex_profile_route("profile-a")?
                    .expect("路由")
                    .recovery_json
                    .as_deref(),
                Some(json)
            );
            assert!(manager.status("profile-a").await.is_err());
        }
        assert_eq!(tokens.ensured.load(Ordering::SeqCst), 0);
        assert_eq!(tokens.deleted.load(Ordering::SeqCst), 0);
        Ok(())
    }

    /// Home 写入失败且停止失败时，操作记录必须保留并阻断后续变更。
    #[tokio::test]
    async fn enabling_home_apply_failure_with_stop_failure_keeps_operation_and_rejects_mutation(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id(
                "provider-a".to_string(),
                "Provider A".to_string(),
                json!({}),
                None,
            ),
        )?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let runtime = Arc::new(HealthRuntime {
            healthy: true,
            stop_fails: true,
            started: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        });
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::new(Arc::new(FailingHomeFileOps))),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(HealthFactory {
                runtimes: HashMap::from([("profile-a".to_string(), runtime)]),
            }),
        );

        assert!(manager
            .enable("profile-a", "provider-a", vec![])
            .await
            .is_err());
        let recovery: serde_json::Value = serde_json::from_str(
            &db.get_codex_profile_route("profile-a")?
                .expect("路由记录")
                .recovery_json
                .expect("操作记录"),
        )
        .expect("操作 JSON");
        assert_eq!(recovery["operation"], "enable");
        assert_eq!(recovery["phase"], CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED);
        assert_eq!(recovery["last_error"], "CODEX_PROFILE_OPERATION_FAILURE");
        assert!(manager.disable("profile-a").await.is_err());
        Ok(())
    }

    /// 恢复在锁外读到启用状态后，若关闭先完成，就不能重新启动该 Profile。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restoring_profile_after_concurrent_disable_does_not_restart_runtime(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id(
                "provider-a".to_string(),
                "Provider A".to_string(),
                json!({}),
                None,
            ),
        )?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: "/tmp/profile-a".to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: Some("provider-a".to_string()),
            enabled: true,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        let (outer_read_tx, outer_read_rx) = channel();
        let (resume_tx, resume_rx) = channel();
        let persistence = Arc::new(RestoreBlockingPersistence {
            db: db.clone(),
            route_reads: AtomicUsize::new(0),
            outer_read: outer_read_tx,
            resume_restore: Mutex::new(resume_rx),
        });
        let manager = Arc::new(CodexRouteManager::new(
            persistence,
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(CodexProfileSecretStore::with_root(
                tempfile::tempdir().expect("临时 token 目录").keep(),
            )),
            Arc::new(FakeFactory),
        ));
        let existing_runtime = Arc::new(FakeRuntime {
            provider: AsyncMutex::new("provider-a".to_string()),
            port: 16001,
        });
        manager
            .runtimes
            .lock()
            .expect("运行时锁")
            .insert("profile-a".to_string(), existing_runtime);

        let restoring_manager = manager.clone();
        let restore_task =
            tokio::spawn(async move { restoring_manager.restore_enabled_profiles().await });
        outer_read_rx.recv().expect("恢复已在锁内读取路由");
        let disabling_manager = manager.clone();
        let disable_task =
            tokio::spawn(async move { disabling_manager.disable("profile-a").await });
        resume_tx.send(()).expect("允许恢复继续");
        restore_task.await.expect("恢复任务未 panic")?;
        disable_task.await.expect("关闭任务未 panic")?;

        assert!(
            !db.get_codex_profile_route("profile-a")?
                .expect("路由仍存在")
                .enabled
        );
        assert!(manager.status("profile-a").await.is_err());
        Ok(())
    }

    /// 含非法恢复记录的 Profile 不得有副作用，且不能影响其他 Profile 恢复。
    #[tokio::test]
    async fn restoring_invalid_recovery_isolated_from_healthy_profile() -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        let home_config = Arc::new(CodexHomeConfigService::system());
        let mut homes = Vec::new();
        let mut runtimes = HashMap::new();
        for (profile_id, healthy) in [("profile-bad", false), ("profile-good", true)] {
            let home = tempfile::tempdir().expect("临时 Profile Home");
            prepare_enabled_profile_home(
                &db,
                &home_config,
                home.path(),
                profile_id,
                if healthy { 16_002 } else { 16_001 },
                "test-local-token",
            )?;
            if !healthy {
                let mut route = db
                    .get_codex_profile_route(profile_id)?
                    .expect("失败路由已创建");
                route.recovery_json = Some(r#"{"operation":"unknown","phase":"prepared","before":{"current_provider_id":null,"enabled":false,"failover_ids":[]},"target":{"current_provider_id":null,"enabled":false,"failover_ids":[]},"last_error":null}"#.to_string());
                db.save_codex_profile_route(&route)?;
            }
            homes.push(home);
            runtimes.insert(
                profile_id.to_string(),
                Arc::new(HealthRuntime {
                    healthy,
                    stop_fails: false,
                    started: AtomicBool::new(false),
                    stopped: AtomicBool::new(false),
                }),
            );
        }
        let bad_runtime = runtimes.get("profile-bad").expect("失败运行时").clone();
        let good_runtime = runtimes.get("profile-good").expect("成功运行时").clone();
        let manager = CodexRouteManager::new(
            db,
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(HealthFactory { runtimes }),
        );

        manager.restore_enabled_profiles().await?;
        assert!(!bad_runtime.started.load(Ordering::SeqCst));
        assert!(!bad_runtime.stopped.load(Ordering::SeqCst));
        assert!(manager.status("profile-bad").await.is_err());
        assert!(good_runtime.started.load(Ordering::SeqCst));
        assert!(!good_runtime.stopped.load(Ordering::SeqCst));
        assert_eq!(
            manager.status("profile-good").await?,
            CodexRuntimeStatus::Running
        );
        Ok(())
    }

    /// 本地凭证删除失败时必须保留数据库关系，并写入可重试错误。
    #[tokio::test]
    async fn deleting_profile_keeps_database_record_when_token_delete_fails() -> Result<(), AppError>
    {
        let db = Arc::new(Database::memory()?);
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id(
                "provider-a".to_string(),
                "Provider A".to_string(),
                json!({}),
                None,
            ),
        )?;
        db.insert_codex_profile(&CodexProfile {
            id: "custom-profile".to_string(),
            name: "自定义 Profile".to_string(),
            canonical_home_path: "/tmp/custom-profile".to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "custom-profile".to_string(),
            current_provider_id: None,
            enabled: false,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        let manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(FailingDeleteTokenStore),
            Arc::new(FakeFactory),
        );

        assert!(manager
            .delete_custom_profile("custom-profile")
            .await
            .is_err());
        assert_eq!(db.get_codex_profile("custom-profile")?.id, "custom-profile");
        let route = db
            .get_codex_profile_route("custom-profile")?
            .expect("路由仍存在");
        assert!(route.last_error.is_some());
        assert!(route.recovery_json.is_some());
        Ok(())
    }

    /// 数据库删除失败时，已删除的 token 必须以无敏感补偿记录标记并在下次操作先补建。
    #[tokio::test]
    async fn deleting_profile_database_failure_records_recovery_and_recreates_token(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        db.insert_codex_profile(&CodexProfile {
            id: "custom-profile".to_string(),
            name: "自定义 Profile".to_string(),
            canonical_home_path: "/tmp/custom-profile".to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "custom-profile".to_string(),
            current_provider_id: None,
            enabled: false,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        let tokens = Arc::new(TrackingTokenStore {
            ensured: AtomicUsize::new(0),
            deleted: AtomicUsize::new(0),
        });
        let manager = CodexRouteManager::new(
            Arc::new(DeleteFailingPersistence { db: db.clone() }),
            Arc::new(CodexHomeConfigService::system()),
            tokens.clone(),
            Arc::new(FakeFactory),
        );

        assert!(manager
            .delete_custom_profile("custom-profile")
            .await
            .is_err());
        let route = db
            .get_codex_profile_route("custom-profile")?
            .expect("路由仍存在");
        let recovery = route.recovery_json.expect("删除补偿记录");
        let parsed: serde_json::Value = serde_json::from_str(&recovery).expect("补偿 JSON");
        assert_eq!(
            parsed["phase"],
            CODEX_ROUTE_RECOVERY_PHASE_DELETE_DATABASE_FAILED
        );
        assert_eq!(parsed["operation"], "delete");
        assert!(parsed["last_error"].is_string());
        assert!(!recovery.contains("test-local-token"));

        assert!(manager
            .delete_custom_profile("custom-profile")
            .await
            .is_err());
        assert!(tokens.ensured.load(Ordering::SeqCst) >= 1);
        assert!(tokens.deleted.load(Ordering::SeqCst) >= 2);
        Ok(())
    }

    /// token 删除后第二次保存（delete 阶段推进）失败，下一次操作必须先补建 token。
    #[tokio::test]
    async fn deleting_token_phase_save_failure_recovers_token_before_retry() -> Result<(), AppError>
    {
        let db = Arc::new(Database::memory()?);
        db.insert_codex_profile(&CodexProfile {
            id: "custom-profile".to_string(),
            name: "自定义".to_string(),
            canonical_home_path: "/tmp/custom".to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "custom-profile".to_string(),
            current_provider_id: None,
            enabled: false,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        let tokens = Arc::new(TrackingTokenStore {
            ensured: AtomicUsize::new(0),
            deleted: AtomicUsize::new(0),
        });
        let manager = CodexRouteManager::new(
            Arc::new(SaveFailingPersistence {
                db: db.clone(),
                save_count: AtomicUsize::new(0),
                fail_on_save: 2,
                fail_replace: false,
            }),
            Arc::new(CodexHomeConfigService::system()),
            tokens.clone(),
            Arc::new(FakeFactory),
        );
        assert!(manager
            .delete_custom_profile("custom-profile")
            .await
            .is_err());
        assert!(db
            .get_codex_profile_route("custom-profile")?
            .expect("路由")
            .recovery_json
            .is_some());
        manager.delete_custom_profile("custom-profile").await?;
        assert!(tokens.ensured.load(Ordering::SeqCst) >= 1);
        Ok(())
    }

    /// Home 已恢复而停止失败时必须留下可重试的关闭补偿阶段。
    #[tokio::test]
    async fn disabling_stop_failure_persists_recovery_after_home_restore() -> Result<(), AppError> {
        use crate::codex_config::codex_config_path_for_home;
        use std::fs;

        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        fs::write(
            codex_config_path_for_home(home.path()),
            "model = \"before\"\n",
        )
        .expect("写入初始配置");
        let home_config = Arc::new(CodexHomeConfigService::system());
        let plan =
            home_config.build_profile_route_plan(home.path(), 16_001, None, "test-local-token")?;
        home_config.apply_route_plan(&plan)?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: None,
            enabled: true,
            live_backup_json: Some(home_config.serialize_backup(&plan)?),
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );
        manager
            .runtimes
            .lock()
            .expect("运行时锁")
            .insert("profile-a".to_string(), Arc::new(StopFailingRuntime));

        assert!(manager.disable("profile-a").await.is_err());
        assert_eq!(
            fs::read_to_string(codex_config_path_for_home(home.path())).expect("读取恢复配置"),
            "model = \"before\"\n"
        );
        let recovery = db
            .get_codex_profile_route("profile-a")?
            .expect("路由存在")
            .recovery_json
            .expect("关闭补偿记录");
        let parsed: serde_json::Value = serde_json::from_str(&recovery).expect("补偿 JSON");
        assert_eq!(
            parsed["phase"],
            CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOP_FAILED
        );
        assert_eq!(parsed["operation"], "disable");
        assert!(parsed["last_error"].is_string());
        assert!(!recovery.contains(&home.path().display().to_string()));
        Ok(())
    }

    /// Home 已恢复但首次停止失败时，下一次补偿应识别 previous 状态并完成关闭。
    #[tokio::test]
    async fn pending_disable_retries_after_home_was_already_restored() -> Result<(), AppError> {
        use crate::codex_config::codex_config_path_for_home;

        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let home_config = Arc::new(CodexHomeConfigService::system());
        prepare_enabled_profile_home(
            &db,
            &home_config,
            home.path(),
            "profile-stop-retry",
            16_001,
            "test-local-token",
        )?;
        let runtime = Arc::new(RecoveryRuntime {
            healthy: true,
            stop_fails: AtomicBool::new(true),
            reject_new_requests_calls: AtomicUsize::new(0),
        });
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(RecoveryRuntimeFactory {
                runtime: runtime.clone(),
            }),
        );
        manager.track_runtime("profile-stop-retry".to_string(), runtime.clone())?;

        assert!(manager.disable("profile-stop-retry").await.is_err());
        let first_restored =
            fs::read(codex_config_path_for_home(home.path())).expect("读取首次恢复后的 Home");
        let recovery: RouteRecoveryRecord = serde_json::from_str(
            &db.get_codex_profile_route("profile-stop-retry")?
                .expect("路由存在")
                .recovery_json
                .expect("停止失败补偿记录"),
        )
        .expect("解析停止失败补偿记录");
        assert_eq!(
            recovery.phase,
            CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOP_FAILED
        );

        runtime.stop_fails.store(false, Ordering::SeqCst);
        manager.recover_pending_locked("profile-stop-retry").await?;

        assert_eq!(
            fs::read(codex_config_path_for_home(home.path())).expect("重读恢复后的 Home"),
            first_restored
        );
        let route = db
            .get_codex_profile_route("profile-stop-retry")?
            .expect("路由存在");
        assert!(!route.enabled);
        assert!(route.recovery_json.is_none());
        assert_eq!(runtime.reject_new_requests_calls.load(Ordering::SeqCst), 2);
        Ok(())
    }

    /// 未完成关闭的恢复同样必须立即停接，不得等待在途路由请求。
    #[tokio::test]
    async fn recovering_pending_disable_does_not_wait_for_in_flight_request() -> Result<(), AppError>
    {
        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let home_config = Arc::new(CodexHomeConfigService::system());
        prepare_enabled_profile_home(
            &db,
            &home_config,
            home.path(),
            "profile-pending-immediate-close",
            16_001,
            "test-local-token",
        )?;
        let mut route = db
            .get_codex_profile_route("profile-pending-immediate-close")?
            .expect("路由存在");
        let before = RouteRecoverySnapshot::from_route(&route, vec![]);
        let mut target = before.clone();
        target.enabled = false;
        route.recovery_json = Some(
            serde_json::to_string(&RouteRecoveryRecord {
                operation: "disable".to_string(),
                before,
                target,
                phase: CODEX_ROUTE_RECOVERY_PHASE_PREPARED.to_string(),
                last_error: None,
                reconcile: None,
            })
            .expect("编码关闭恢复记录"),
        );
        db.save_codex_profile_route(&route)?;
        let runtime = Arc::new(InFlightRequestRuntime {
            stop_called: AtomicBool::new(false),
        });
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );
        manager.track_runtime(
            "profile-pending-immediate-close".to_string(),
            runtime.clone(),
        )?;

        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            manager.recover_pending_locked("profile-pending-immediate-close"),
        )
        .await
        .expect("未完成关闭恢复不应等待在途路由请求")?;

        assert!(runtime.stop_called.load(Ordering::SeqCst));
        let route = db
            .get_codex_profile_route("profile-pending-immediate-close")?
            .expect("路由存在");
        assert!(!route.enabled);
        assert!(route.recovery_json.is_none());
        Ok(())
    }

    /// 关闭时 Home 恢复失败必须进入专用阶段，修复后恢复会重试 Home 并完成即时停接关闭。
    // 历史说明：该恢复流程原本在 Home 修复后完成排空关闭。
    #[tokio::test]
    async fn disabling_home_restore_failure_retries_home_before_recovery_shutdown(
    ) -> Result<(), AppError> {
        use crate::codex_config::codex_config_path_for_home;

        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let config_path = codex_config_path_for_home(home.path());
        fs::write(&config_path, "model = \"before\"\n").expect("写入初始配置");
        let file_ops = Arc::new(ToggleRestoreHomeFileOps {
            fail_writes: AtomicBool::new(false),
        });
        let home_config = Arc::new(CodexHomeConfigService::new(file_ops.clone()));
        let plan =
            home_config.build_profile_route_plan(home.path(), 16_001, None, "test-local-token")?;
        home_config.apply_route_plan(&plan)?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: None,
            enabled: true,
            live_backup_json: Some(home_config.serialize_backup(&plan)?),
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        let runtime = Arc::new(RecoveryRuntime {
            healthy: true,
            stop_fails: AtomicBool::new(false),
            reject_new_requests_calls: AtomicUsize::new(0),
        });
        let manager = CodexRouteManager::new(
            db.clone(),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );
        manager.track_runtime("profile-a".to_string(), runtime.clone())?;
        file_ops.fail_writes.store(true, Ordering::SeqCst);

        assert!(manager.disable("profile-a").await.is_err());
        let recovery: serde_json::Value = serde_json::from_str(
            &db.get_codex_profile_route("profile-a")?
                .expect("路由记录")
                .recovery_json
                .expect("关闭补偿记录"),
        )
        .expect("解析关闭补偿记录");
        assert_eq!(recovery["phase"], "disable_home_restore_failed");

        file_ops.fail_writes.store(false, Ordering::SeqCst);
        manager.recover_pending_locked("profile-a").await?;

        assert_eq!(
            fs::read_to_string(config_path).expect("读取恢复配置"),
            "model = \"before\"\n"
        );
        assert_eq!(runtime.reject_new_requests_calls.load(Ordering::SeqCst), 1);
        assert!(manager.status("profile-a").await.is_err());
        let route = db.get_codex_profile_route("profile-a")?.expect("路由记录");
        assert!(!route.enabled);
        assert!(route.recovery_json.is_none());
        Ok(())
    }

    /// 关闭操作仅持久化 prepared 后进程崩溃时，新 manager 必须恢复 Home 并收敛关闭状态。
    #[tokio::test]
    async fn restarting_after_prepared_disable_restores_home_and_clears_recovery(
    ) -> Result<(), AppError> {
        use crate::codex_config::codex_config_path_for_home;

        let db = Arc::new(Database::memory()?);
        let home = tempfile::tempdir().expect("临时 Home 目录");
        let config_path = codex_config_path_for_home(home.path());
        fs::write(&config_path, "model = \"before\"\n").expect("写入初始配置");
        let home_config = Arc::new(CodexHomeConfigService::system());
        let plan =
            home_config.build_profile_route_plan(home.path(), 16_001, None, "test-local-token")?;
        home_config.apply_route_plan(&plan)?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "A".to_string(),
            canonical_home_path: home.path().display().to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        let route = CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: None,
            enabled: true,
            live_backup_json: Some(home_config.serialize_backup(&plan)?),
            last_error: None,
            recovery_json: Some(
                serde_json::to_string(&RouteRecoveryRecord {
                    operation: "disable".to_string(),
                    before: RouteRecoverySnapshot {
                        current_provider_id: None,
                        enabled: true,
                        failover_ids: vec![],
                    },
                    target: RouteRecoverySnapshot {
                        current_provider_id: None,
                        enabled: false,
                        failover_ids: vec![],
                    },
                    phase: CODEX_ROUTE_RECOVERY_PHASE_PREPARED.to_string(),
                    last_error: None,
                    reconcile: None,
                })
                .expect("编码关闭操作记录"),
            ),
            updated_at: 1,
        };
        db.save_codex_profile_route(&route)?;

        let restarted_manager = CodexRouteManager::new(
            db.clone(),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        restarted_manager.restore_enabled_profiles().await?;

        assert_eq!(
            fs::read_to_string(config_path).expect("读取恢复配置"),
            "model = \"before\"\n"
        );
        let recovered_route = db.get_codex_profile_route("profile-a")?.expect("路由记录");
        assert!(!recovered_route.enabled);
        assert!(recovered_route.recovery_json.is_none());
        assert!(restarted_manager.status("profile-a").await.is_err());
        Ok(())
    }

    /// 停止成功但最终关闭状态保存失败时，下一次变更必须先收敛关闭操作。
    #[tokio::test]
    async fn disabling_final_save_failure_keeps_operation_before_next_mutation(
    ) -> Result<(), AppError> {
        let db = Arc::new(Database::memory()?);
        db.save_provider(
            AppType::Codex.as_str(),
            &Provider::with_id(
                "provider-a".to_string(),
                "Provider A".to_string(),
                json!({}),
                None,
            ),
        )?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "Profile A".to_string(),
            canonical_home_path: "/tmp/profile-a".to_string(),
            listen_port: 16001,
            created_at: 1,
            updated_at: 1,
        })?;
        db.save_codex_profile_route(&CodexProfileRoute {
            profile_id: "profile-a".to_string(),
            current_provider_id: None,
            enabled: true,
            live_backup_json: None,
            last_error: None,
            recovery_json: None,
            updated_at: 1,
        })?;
        let manager = CodexRouteManager::new(
            Arc::new(SaveFailingPersistence {
                db: db.clone(),
                save_count: AtomicUsize::new(0),
                fail_on_save: 3,
                fail_replace: false,
            }),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );
        manager.runtimes.lock().expect("运行时锁").insert(
            "profile-a".to_string(),
            Arc::new(FakeRuntime {
                provider: AsyncMutex::new("provider-a".to_string()),
                port: 16001,
            }),
        );

        assert!(manager.disable("profile-a").await.is_err());
        let recovery: serde_json::Value = serde_json::from_str(
            &db.get_codex_profile_route("profile-a")?
                .expect("路由记录")
                .recovery_json
                .expect("操作记录"),
        )
        .expect("操作 JSON");
        assert_eq!(recovery["operation"], "disable");
        assert_eq!(
            recovery["phase"],
            CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOPPED
        );
        assert!(manager
            .switch_provider("profile-a", "provider-a", vec![])
            .await
            .is_err());
        assert!(db
            .get_codex_profile_route("profile-a")?
            .expect("路由记录")
            .recovery_json
            .is_none());
        Ok(())
    }

    /// 单个 Profile 的 route 读取失败不能阻断后续已启用 Profile 的恢复。
    #[tokio::test]
    async fn restoring_route_read_failure_isolated_from_following_profile() -> Result<(), AppError>
    {
        let db = Arc::new(Database::memory()?);
        let home_config = Arc::new(CodexHomeConfigService::system());
        let mut homes = Vec::new();
        for profile_id in ["profile-bad", "profile-good"] {
            let home = tempfile::tempdir().expect("临时 Profile Home");
            prepare_enabled_profile_home(
                &db,
                &home_config,
                home.path(),
                profile_id,
                if profile_id == "profile-bad" {
                    16_001
                } else {
                    16_002
                },
                "test-local-token",
            )?;
            homes.push(home);
        }
        let manager = CodexRouteManager::new(
            Arc::new(RouteReadFailingPersistence {
                db,
                failing_profile: "profile-bad".to_string(),
            }),
            home_config,
            Arc::new(TrackingTokenStore {
                ensured: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }),
            Arc::new(FakeFactory),
        );

        manager.restore_enabled_profiles().await?;
        assert!(manager.status("profile-bad").await.is_err());
        assert_eq!(
            manager.status("profile-good").await?,
            CodexRuntimeStatus::Running
        );
        Ok(())
    }
}
