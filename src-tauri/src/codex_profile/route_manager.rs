//! Codex Profile 独立路由生命周期编排。

use crate::app_config::AppType;
use crate::codex_profile::{
    CodexHomeConfigService, CodexProfile, CodexProfileRoute, CodexProfileScope,
    CodexProfileSecretStore, CodexRouteProviderSnapshot, CodexRouteRuntime,
    CodexRouteRuntimeFactory, CodexRuntimeStatus, CODEX_ROUTE_COMPENSATION_UNCONVERGED_ERROR,
    CODEX_ROUTE_RECOVERY_PHASE_DELETE_DATABASE_FAILED,
    CODEX_ROUTE_RECOVERY_PHASE_DELETE_TOKEN_PENDING,
    CODEX_ROUTE_RECOVERY_PHASE_DISABLE_HOME_RESTORE_FAILED,
    CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOPPED, CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOP_FAILED,
    CODEX_ROUTE_RECOVERY_PHASE_ENABLE_HOME_APPLIED,
    CODEX_ROUTE_RECOVERY_PHASE_ENABLE_PERSIST_FAILED, CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED,
    CODEX_ROUTE_RECOVERY_PHASE_PREPARED, CODEX_ROUTE_RECOVERY_PHASE_SWITCH_FAILOVERS_SAVED,
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
    fn delete_profile(&self, profile_id: &str) -> Result<(), AppError> {
        self.delete_codex_profile(profile_id)
    }
}

/// manager 所需的本地 token 生命周期最小契约。
pub trait CodexProfileTokenStore: Send + Sync {
    fn ensure_token(&self, profile_id: &str) -> Result<String, AppError>;
    fn delete_token(&self, profile_id: &str) -> Result<(), AppError>;
}

impl CodexProfileTokenStore for CodexProfileSecretStore {
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
        }
    }

    /// 启动指定 Profile 的独立监听器并原子接管其 Home 配置。
    pub async fn enable(
        &self,
        profile_id: &str,
        provider_id: &str,
        failover_ids: Vec<String>,
    ) -> Result<(), AppError> {
        if provider_id.is_empty() {
            return Err(AppError::InvalidInput("Codex 供应商不能为空".to_string()));
        }
        self.validate_route_operation_before_lock(profile_id)?;
        let _ = self.provider_snapshot(provider_id, &failover_ids)?;
        let lock = self.profile_lock(profile_id)?;
        let _guard = lock.lock().await;
        let profile = self.persistence.get_profile(profile_id)?;
        profile.validate_route_operation()?;
        self.recover_pending_locked(profile_id).await?;
        let previous = self
            .persistence
            .get_route(profile_id)?
            .unwrap_or_else(|| Self::empty_route(&profile.id));
        let snapshot = self.provider_snapshot(provider_id, &failover_ids)?;
        let plan = self.home_config.build_profile_route_plan(
            std::path::Path::new(&profile.canonical_home_path),
            profile.listen_port,
            self.persistence.get_provider(provider_id)?.as_ref(),
        )?;
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
        let token = self.secret_store.ensure_token(profile_id)?;
        let runtime = self.runtime_factory.create(
            CodexProfileScope {
                profile_id: profile.id.clone(),
                home_path: profile.canonical_home_path.into(),
                port: profile.listen_port,
            },
            token,
            snapshot,
        );

        if let Err(error) = runtime.start().await {
            self.persist_operation_error(profile_id, CODEX_ROUTE_RECOVERY_PHASE_PREPARED, &error)?;
            return Err(AppError::Message(error));
        }
        if let Err(error) =
            self.advance_operation(profile_id, CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED, None)
        {
            let stop_error = runtime.stop().await.err();
            if let Some(stop_error) = stop_error {
                self.track_runtime(profile.id.clone(), runtime.clone())?;
                let _ = self.persist_operation_error(
                    profile_id,
                    CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED,
                    &stop_error,
                );
            }
            return Err(error);
        }
        if !runtime.health_check().await {
            let error = "Codex Profile 路由健康检查失败";
            let stop_error = runtime.stop().await.err();
            if stop_error.is_some() {
                self.track_runtime(profile.id.clone(), runtime.clone())?;
            }
            self.persist_operation_error(
                profile_id,
                CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED,
                &stop_error.unwrap_or_else(|| error.to_string()),
            )?;
            return Err(AppError::Message(
                "Codex Profile 路由健康检查失败".to_string(),
            ));
        }
        let backup = self.home_config.serialize_backup(&plan)?;
        if let Err(error) = self.persist_enable_backup(profile_id, backup.clone()) {
            if runtime.stop().await.is_err() {
                self.track_runtime(profile.id.clone(), runtime)?;
            }
            return Err(error);
        }
        if let Err(error) = self.home_config.apply_route_plan(&plan) {
            let stop_error = runtime.stop().await.err();
            if stop_error.is_some() {
                self.track_runtime(profile.id.clone(), runtime)?;
            }
            self.persist_operation_error(
                profile_id,
                CODEX_ROUTE_RECOVERY_PHASE_ENABLE_STARTED,
                &stop_error.unwrap_or_else(|| error.to_string()),
            )?;
            return Err(error);
        }
        if let Err(error) = self.advance_operation(
            profile_id,
            CODEX_ROUTE_RECOVERY_PHASE_ENABLE_HOME_APPLIED,
            None,
        ) {
            let restore_error = self.home_config.restore(&plan).err();
            let stop_error = runtime.stop().await.err();
            let compensation_error = restore_error.map(|error| error.to_string()).or(stop_error);
            if let Some(compensation_error) = compensation_error {
                let _ = self.persist_operation_error(
                    profile_id,
                    CODEX_ROUTE_RECOVERY_PHASE_ENABLE_HOME_APPLIED,
                    &compensation_error,
                );
            }
            return Err(error);
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
            let restore_error = self.home_config.restore(&plan).err();
            let stop_error = runtime.stop().await.err();
            let cleanup_failed = restore_error.is_some() || stop_error.is_some();
            let persistence_error = self
                .persist_operation_error(
                    profile_id,
                    CODEX_ROUTE_RECOVERY_PHASE_ENABLE_PERSIST_FAILED,
                    "启用路由持久化失败，拒绝继续变更",
                )
                .err();
            if cleanup_failed {
                self.track_runtime(profile.id.clone(), runtime)?;
            }
            return Err(Self::compensation_failure_error(
                &error,
                persistence_error.as_ref(),
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
        if provider_id.is_empty() {
            return Err(AppError::InvalidInput("Codex 供应商不能为空".to_string()));
        }
        self.validate_route_operation_before_lock(profile_id)?;
        let _ = self.provider_snapshot(provider_id, &failover_ids)?;
        let lock = self.profile_lock(profile_id)?;
        let _guard = lock.lock().await;
        self.persistence
            .get_profile(profile_id)?
            .validate_route_operation()?;
        self.recover_pending_locked(profile_id).await?;
        let snapshot = self.provider_snapshot(provider_id, &failover_ids)?;
        let route = self
            .persistence
            .get_route(profile_id)?
            .ok_or_else(|| AppError::InvalidInput("Codex Profile 路由不存在".to_string()))?;
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
        };
        let prepared = self.route_with_recovery(route.clone(), &recovery)?;
        self.persistence.save_route(&prepared)?;
        runtime.swap_provider_snapshot(snapshot).await;
        if let Err(error) = self.advance_operation(
            profile_id,
            CODEX_ROUTE_RECOVERY_PHASE_SWITCH_RUNTIME_SWAPPED,
            None,
        ) {
            let compensation_error = self
                .restore_switch_locked(profile_id, &route, &old_failovers, old_snapshot)
                .await
                .err();
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
                .restore_switch_locked(profile_id, &route, &old_failovers, old_snapshot)
                .await
                .err();
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        self.advance_operation(
            profile_id,
            CODEX_ROUTE_RECOVERY_PHASE_SWITCH_ROUTE_SAVED,
            None,
        )?;
        if let Err(error) = self
            .persistence
            .replace_failovers(profile_id, &failover_ids)
        {
            let compensation_error = self
                .restore_switch_locked(profile_id, &route, &old_failovers, old_snapshot)
                .await
                .err();
            return Err(Self::compensation_failure_error(
                &error,
                compensation_error.as_ref(),
            ));
        }
        self.advance_operation(
            profile_id,
            CODEX_ROUTE_RECOVERY_PHASE_SWITCH_FAILOVERS_SAVED,
            None,
        )?;
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

    /// 先恢复 Home，再拒绝新请求并排空、停止监听器，最后持久化关闭状态。
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
        if let Some(backup) = &route.live_backup_json {
            if let Err(error) = self
                .home_config
                .restore_backup(std::path::Path::new(&profile.canonical_home_path), backup)
            {
                self.persist_operation_error(
                    profile_id,
                    CODEX_ROUTE_RECOVERY_PHASE_DISABLE_HOME_RESTORE_FAILED,
                    &error.to_string(),
                )?;
                return Err(error);
            }
        }
        if let Ok(runtime) = self.runtime(profile_id) {
            if let Err(error) = Self::drain_and_stop_runtime(runtime).await {
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
                let provider_id = route.current_provider_id.as_deref().ok_or_else(|| {
                    AppError::InvalidInput("Codex Profile 未配置供应商".to_string())
                })?;
                let snapshot = self.provider_snapshot(
                    provider_id,
                    &self.persistence.list_failovers(&profile.id)?,
                )?;
                let token = self.secret_store.ensure_token(&profile.id)?;
                let runtime = self.runtime_factory.create(
                    CodexProfileScope {
                        profile_id: profile.id.clone(),
                        home_path: profile.canonical_home_path.clone().into(),
                        port: profile.listen_port,
                    },
                    token,
                    snapshot,
                );
                runtime.start().await.map_err(AppError::Message)?;
                if !runtime.health_check().await {
                    let stop_failed = runtime.stop().await.is_err();
                    self.persist_error_summary(&profile.id, "恢复健康检查失败", stop_failed)?;
                    return Err(AppError::Message(
                        "Codex Profile 路由健康检查失败".to_string(),
                    ));
                }
                self.runtimes
                    .lock()
                    .map_err(|e| AppError::Lock(e.to_string()))?
                    .insert(profile.id.clone(), runtime);
                Ok(())
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

    /// 返回 Profile 当前运行状态。
    pub async fn status(&self, profile_id: &str) -> Result<CodexRuntimeStatus, AppError> {
        Ok(self.runtime(profile_id)?.status().await)
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

    /// 停止监听器前先拒绝新请求并等待在途请求排空，超时状态会写入停止错误以便观测。
    async fn drain_and_stop_runtime(runtime: Arc<dyn CodexRouteRuntime>) -> Result<(), String> {
        runtime.begin_draining().await;
        let drained = runtime.wait_for_drain().await;
        runtime
            .stop()
            .await
            .map_err(|error| format!("{error}（已排空: {drained}）"))
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
        if recovery.phase == CODEX_ROUTE_RECOVERY_PHASE_DISABLE_HOME_RESTORE_FAILED {
            if let Some(backup) = route.live_backup_json.as_deref() {
                let profile = self.persistence.get_profile(profile_id)?;
                if let Err(error) = self
                    .home_config
                    .restore_backup(std::path::Path::new(&profile.canonical_home_path), backup)
                {
                    self.persist_operation_error(
                        profile_id,
                        CODEX_ROUTE_RECOVERY_PHASE_DISABLE_HOME_RESTORE_FAILED,
                        &error.to_string(),
                    )?;
                    return Err(Self::recovery_unconverged_error(error));
                }
            }
        }
        if recovery.phase == CODEX_ROUTE_RECOVERY_PHASE_DISABLE_HOME_RESTORE_FAILED
            || recovery.phase == CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOP_FAILED
            || recovery.phase == CODEX_ROUTE_RECOVERY_PHASE_DISABLE_STOPPED
        {
            if let Ok(runtime) = self.runtime(profile_id) {
                if let Err(error) = Self::drain_and_stop_runtime(runtime).await {
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
            if let Err(error) = Self::drain_and_stop_runtime(runtime).await {
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
        );
        if valid {
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
        fn begin_draining(&self) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async {})
        }
        fn wait_for_drain(&self) -> CodexRouteRuntimeFuture<'_, bool> {
            Box::pin(async { true })
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
        fn ensure_token(&self, _: &str) -> Result<String, AppError> {
            self.ensured.fetch_add(1, Ordering::SeqCst);
            Ok("test-local-token".to_string())
        }
        fn delete_token(&self, _: &str) -> Result<(), AppError> {
            self.deleted.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
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
        fn begin_draining(&self) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async {})
        }
        fn wait_for_drain(&self) -> CodexRouteRuntimeFuture<'_, bool> {
            Box::pin(async { true })
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
        fn begin_draining(&self) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async {})
        }
        fn wait_for_drain(&self) -> CodexRouteRuntimeFuture<'_, bool> {
            Box::pin(async { true })
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

    /// 可切换停止结果并记录排空过程的运行时替身。
    struct RecoveryRuntime {
        healthy: bool,
        stop_fails: AtomicBool,
        drain_waits: AtomicUsize,
        drain_result: bool,
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
        fn begin_draining(&self) -> CodexRouteRuntimeFuture<'_, ()> {
            Box::pin(async {})
        }
        fn wait_for_drain(&self) -> CodexRouteRuntimeFuture<'_, bool> {
            Box::pin(async move {
                self.drain_waits.fetch_add(1, Ordering::SeqCst);
                self.drain_result
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

    /// 公共切换入口遇到缺失运行时的未完成切换时，必须保持持久化状态等待后续补偿。
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
            phase: CODEX_ROUTE_RECOVERY_PHASE_SWITCH_RUNTIME_SWAPPED.to_string(),
            last_error: None,
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
        assert_eq!(
            db.get_codex_profile_route("profile-a")?.expect("路由记录"),
            route
        );
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

    /// 启动阶段记录写入失败且停止失败后，恢复必须排空并移除被托管的运行时。
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
            drain_waits: AtomicUsize::new(0),
            drain_result: true,
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

        assert_eq!(runtime.drain_waits.load(Ordering::SeqCst), 1);
        assert!(manager.status("profile-a").await.is_err());
        Ok(())
    }

    /// 健康检查失败且停止失败后，恢复必须继续托管运行时并采用完整排空关闭流程。
    #[tokio::test]
    async fn enabling_health_failure_tracks_runtime_until_recovery_drains_and_stops(
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
            drain_waits: AtomicUsize::new(0),
            drain_result: true,
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

        assert_eq!(runtime.drain_waits.load(Ordering::SeqCst), 1);
        assert!(manager.status("profile-a").await.is_err());
        Ok(())
    }

    /// 恢复关闭即使排空超时也必须调用等待，并在停止失败时返回可观测的排空状态。
    #[tokio::test]
    async fn recovering_enable_residue_reports_drain_timeout_when_stop_still_fails(
    ) -> Result<(), AppError> {
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
            drain_waits: AtomicUsize::new(0),
            drain_result: false,
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

        assert_eq!(runtime.drain_waits.load(Ordering::SeqCst), 1);
        assert!(error.to_string().contains("已排空: false"));
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
            },
            RouteRecoveryRecord {
                operation: "switch".to_string(),
                before: snapshot.clone(),
                target: snapshot.clone(),
                phase: CODEX_ROUTE_RECOVERY_PHASE_DELETE_TOKEN_PENDING.to_string(),
                last_error: None,
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
        let mut runtimes = HashMap::new();
        for (profile_id, healthy) in [("profile-bad", false), ("profile-good", true)] {
            db.save_provider(
                AppType::Codex.as_str(),
                &Provider::with_id(
                    format!("provider-{profile_id}"),
                    profile_id.to_string(),
                    json!({}),
                    None,
                ),
            )?;
            db.insert_codex_profile(&CodexProfile {
                id: profile_id.to_string(),
                name: profile_id.to_string(),
                canonical_home_path: format!("/tmp/{profile_id}"),
                listen_port: if healthy { 16002 } else { 16001 },
                created_at: 1,
                updated_at: 1,
            })?;
            db.save_codex_profile_route(&CodexProfileRoute {
                profile_id: profile_id.to_string(),
                current_provider_id: Some(format!("provider-{profile_id}")),
                enabled: true,
                live_backup_json: None,
                last_error: None,
                recovery_json: (!healthy).then(|| r#"{"operation":"unknown","phase":"prepared","before":{"current_provider_id":null,"enabled":false,"failover_ids":[]},"target":{"current_provider_id":null,"enabled":false,"failover_ids":[]},"last_error":null}"#.to_string()),
                updated_at: 1,
            })?;
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
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(CodexProfileSecretStore::with_root(
                tempfile::tempdir().expect("临时 token 目录").keep(),
            )),
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
        let plan = home_config.build_route_plan(home.path(), "model = \"route\"\n")?;
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

    /// 关闭时 Home 恢复失败必须进入专用阶段，修复后恢复会重试 Home 并完成排空关闭。
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
        let plan = home_config.build_route_plan(home.path(), "model = \"route\"\n")?;
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
            drain_waits: AtomicUsize::new(0),
            drain_result: true,
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
        assert_eq!(runtime.drain_waits.load(Ordering::SeqCst), 1);
        assert!(manager.status("profile-a").await.is_err());
        let route = db.get_codex_profile_route("profile-a")?.expect("路由记录");
        assert!(!route.enabled);
        assert!(route.recovery_json.is_none());
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
        for profile_id in ["profile-bad", "profile-good"] {
            let provider_id = format!("provider-{profile_id}");
            db.save_provider(
                AppType::Codex.as_str(),
                &Provider::with_id(provider_id.clone(), provider_id.clone(), json!({}), None),
            )?;
            db.insert_codex_profile(&CodexProfile {
                id: profile_id.to_string(),
                name: profile_id.to_string(),
                canonical_home_path: format!("/tmp/{profile_id}"),
                listen_port: if profile_id == "profile-bad" {
                    16001
                } else {
                    16002
                },
                created_at: 1,
                updated_at: 1,
            })?;
            db.save_codex_profile_route(&CodexProfileRoute {
                profile_id: profile_id.to_string(),
                current_provider_id: Some(provider_id),
                enabled: true,
                live_backup_json: None,
                last_error: None,
                recovery_json: None,
                updated_at: 1,
            })?;
        }
        let manager = CodexRouteManager::new(
            Arc::new(RouteReadFailingPersistence {
                db,
                failing_profile: "profile-bad".to_string(),
            }),
            Arc::new(CodexHomeConfigService::system()),
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
