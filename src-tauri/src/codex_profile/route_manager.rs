//! Codex Profile 独立路由生命周期编排。

use crate::app_config::AppType;
use crate::codex_profile::{
    CodexHomeConfigService, CodexProfile, CodexProfileRoute, CodexProfileScope,
    CodexProfileSecretStore, CodexRouteProviderSnapshot, CodexRouteRuntime,
    CodexRouteRuntimeFactory, CodexRuntimeStatus,
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
struct RouteRecoveryRecord {
    original: RouteRecoverySnapshot,
    target: RouteRecoverySnapshot,
    phase: String,
    error_summary: Option<String>,
}

/// 可恢复的路由快照不包含 live backup、token 或认证配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        let lock = self.profile_lock(profile_id)?;
        let _guard = lock.lock().await;
        self.recover_pending_locked(profile_id).await?;
        let profile = self.persistence.get_profile(profile_id)?;
        profile.validate_route_operation()?;
        let snapshot = self.provider_snapshot(provider_id, &failover_ids)?;
        let plan = self.home_config.build_profile_route_plan(
            std::path::Path::new(&profile.canonical_home_path),
            profile.listen_port,
            self.persistence.get_provider(provider_id)?.as_ref(),
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
            self.persist_error(profile_id)?;
            return Err(AppError::Message(error));
        }
        if !runtime.health_check().await {
            let rollback_failed = runtime.stop().await.is_err();
            self.persist_error_summary(profile_id, "健康检查失败", rollback_failed)?;
            return Err(AppError::Message(
                "Codex Profile 路由健康检查失败".to_string(),
            ));
        }
        if let Err(error) = self.home_config.apply_route_plan(&plan) {
            let rollback_failed = runtime.stop().await.is_err();
            self.persist_error_summary(profile_id, "写入 Home 配置失败", rollback_failed)?;
            return Err(error);
        }
        let backup = self.home_config.serialize_backup(&plan)?;
        let route = crate::codex_profile::CodexProfileRoute {
            profile_id: profile.id.clone(),
            current_provider_id: Some(provider_id.to_string()),
            enabled: true,
            live_backup_json: Some(backup),
            last_error: None,
            recovery_json: None,
            updated_at: Utc::now().timestamp_millis(),
        };
        if let Err(error) = self.persistence.save_route(&route) {
            let restore_failed = self.home_config.restore(&plan).is_err();
            let stop_failed = runtime.stop().await.is_err();
            self.persist_error_summary(
                profile_id,
                "持久化启用状态失败",
                restore_failed || stop_failed,
            )?;
            return Err(error);
        }
        self.runtimes
            .lock()
            .map_err(|e| AppError::Lock(e.to_string()))?
            .insert(profile.id, runtime);
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
        let lock = self.profile_lock(profile_id)?;
        let _guard = lock.lock().await;
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
            original: RouteRecoverySnapshot::from_route(&route, old_failovers.clone()),
            target: RouteRecoverySnapshot::from_route(&changed, failover_ids.clone()),
            phase: "prepared".to_string(),
            error_summary: None,
        };
        let prepared = self.route_with_recovery(route.clone(), &recovery)?;
        self.persistence.save_route(&prepared)?;
        runtime.swap_provider_snapshot(snapshot).await;
        let changing = self.route_with_recovery(changed.clone(), &recovery)?;
        if let Err(error) = self.persistence.save_route(&changing) {
            self.restore_switch_locked(profile_id, &route, &old_failovers, old_snapshot, &recovery)
                .await;
            return Err(error);
        }
        if let Err(error) = self
            .persistence
            .replace_failovers(profile_id, &failover_ids)
        {
            self.restore_switch_locked(profile_id, &route, &old_failovers, old_snapshot, &recovery)
                .await;
            return Err(error);
        }
        self.persistence.save_route(&changed)?;
        Ok(())
    }

    /// 先恢复 Home，再拒绝新请求并排空、停止监听器，最后持久化关闭状态。
    pub async fn disable(&self, profile_id: &str) -> Result<(), AppError> {
        let lock = self.profile_lock(profile_id)?;
        let _guard = lock.lock().await;
        self.recover_pending_locked(profile_id).await?;
        let profile = self.persistence.get_profile(profile_id)?;
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
        if let Some(backup) = &route.live_backup_json {
            self.home_config
                .restore_backup(std::path::Path::new(&profile.canonical_home_path), backup)?;
        }
        if let Ok(runtime) = self.runtime(profile_id) {
            runtime.begin_draining().await;
            let drained = runtime.wait_for_drain().await;
            if let Err(error) = runtime.stop().await {
                self.persist_error(profile_id)?;
                return Err(AppError::Message(format!(
                    "Codex Profile 路由停止失败（已排空: {drained}）: {error}"
                )));
            }
        }
        self.persistence
            .save_route(&crate::codex_profile::CodexProfileRoute {
                enabled: false,
                last_error: None,
                updated_at: Utc::now().timestamp_millis(),
                ..route
            })?;
        self.runtimes
            .lock()
            .map_err(|e| AppError::Lock(e.to_string()))?
            .remove(profile_id);
        Ok(())
    }

    /// 删除自定义 Profile 的数据库关系与私有 token，绝不删除其 Home。
    pub async fn delete_custom_profile(&self, profile_id: &str) -> Result<(), AppError> {
        let lock = self.profile_lock(profile_id)?;
        let _guard = lock.lock().await;
        self.recover_pending_locked(profile_id).await?;
        let profile = self.persistence.get_profile(profile_id)?;
        profile.validate_delete()?;
        if let Some(route) = self.persistence.get_route(profile_id)? {
            if route.enabled {
                self.disable_locked(profile_id, &profile, route).await?;
            }
        }
        if let Err(error) = self.secret_store.delete_token(profile_id) {
            self.persist_error_summary(profile_id, "删除本地凭证失败，请重试", false)?;
            return Err(error);
        }
        self.persistence.delete_profile(profile_id)
    }

    /// 启动时逐个恢复已启用 Profile；单个失败会被隔离并记录。
    pub async fn restore_enabled_profiles(&self) -> Result<(), AppError> {
        for profile in self.persistence.list_profiles()? {
            let Some(route) = self.persistence.get_route(&profile.id)? else {
                continue;
            };
            if !route.enabled {
                continue;
            }
            let result = async {
                let lock = self.profile_lock(&profile.id)?;
                let _guard = lock.lock().await;
                self.recover_pending_locked(&profile.id).await?;
                let route = self.persistence.get_route(&profile.id)?.ok_or_else(|| {
                    AppError::InvalidInput("Codex Profile 路由不存在".to_string())
                })?;
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
    fn runtime(&self, profile_id: &str) -> Result<Arc<dyn CodexRouteRuntime>, AppError> {
        self.runtimes
            .lock()
            .map_err(|e| AppError::Lock(e.to_string()))?
            .get(profile_id)
            .cloned()
            .ok_or_else(|| AppError::InvalidInput(format!("Codex Profile 未运行: {profile_id}")))
    }
    fn persist_error(&self, profile_id: &str) -> Result<(), AppError> {
        self.persist_error_summary(profile_id, "路由操作失败", false)
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
        let Some(route) = self.persistence.get_route(profile_id)? else {
            return Ok(());
        };
        let Some(recovery_json) = route.recovery_json.clone() else {
            return Ok(());
        };
        let recovery: RouteRecoveryRecord = serde_json::from_str(&recovery_json).map_err(|_| {
            AppError::InvalidInput("Codex Profile 路由补偿记录无效，拒绝继续变更".to_string())
        })?;
        let original = recovery.original;
        let mut restored = route;
        restored.current_provider_id = original.current_provider_id.clone();
        restored.enabled = original.enabled;
        restored.recovery_json = Some(recovery_json);
        restored.last_error = Some("正在恢复未完成的路由变更".to_string());
        self.persistence.save_route(&restored)?;
        self.persistence
            .replace_failovers(profile_id, &original.failover_ids)?;
        if let Ok(runtime) = self.runtime(profile_id) {
            let provider_id = original
                .current_provider_id
                .as_deref()
                .ok_or_else(|| AppError::InvalidInput("补偿记录缺少原始供应商".to_string()))?;
            runtime
                .swap_provider_snapshot(
                    self.provider_snapshot(provider_id, &original.failover_ids)?,
                )
                .await;
        }
        restored.recovery_json = None;
        restored.last_error = None;
        restored.updated_at = Utc::now().timestamp_millis();
        self.persistence.save_route(&restored)
    }

    /// 将失败切换收敛回旧的运行时和持久化快照；无法收敛时保留补偿记录。
    async fn restore_switch_locked(
        &self,
        profile_id: &str,
        route: &CodexProfileRoute,
        failovers: &[String],
        old_snapshot: CodexRouteProviderSnapshot,
        recovery: &RouteRecoveryRecord,
    ) {
        let mut restoring = self
            .route_with_recovery(route.clone(), recovery)
            .unwrap_or_else(|_| route.clone());
        restoring.last_error = Some("切换失败，正在恢复原路由".to_string());
        let persistence_result = self
            .persistence
            .save_route(&restoring)
            .and_then(|_| self.persistence.replace_failovers(profile_id, failovers));
        if let Ok(runtime) = self.runtime(profile_id) {
            runtime.swap_provider_snapshot(old_snapshot).await;
        }
        if persistence_result.is_ok() {
            restoring.recovery_json = None;
            restoring.last_error = None;
            restoring.updated_at = Utc::now().timestamp_millis();
            let _ = self.persistence.save_route(&restoring);
        }
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
                Ok(())
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

    fn manager(db: Arc<Database>) -> CodexRouteManager {
        CodexRouteManager::new(
            db,
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(CodexProfileSecretStore::with_root(
                tempfile::tempdir().expect("临时目录").keep(),
            )),
            Arc::new(FakeFactory),
        )
    }

    /// 切换 A 只能替换 A 的新请求快照，B 的供应商与端口保持不变。
    #[tokio::test]
    async fn switching_profile_a_does_not_change_profile_b_snapshot() -> Result<(), AppError> {
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
        let manager = manager(db);
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
        manager
            .switch_provider("profile-a", "provider-a-next", vec![])
            .await?;
        assert_eq!(*runtime_a.provider.lock().await, "provider-a-next");
        assert_eq!(*runtime_b.provider.lock().await, "provider-b");
        assert_eq!(runtime_b.port, 16002);
        Ok(())
    }

    /// 保存切换中的目标路由失败时，运行时与数据库都必须恢复为原始快照。
    #[tokio::test]
    async fn switching_route_save_failure_restores_runtime_and_database_snapshot(
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
            fail_on_save: 2,
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
        assert_eq!(route.current_provider_id.as_deref(), Some("provider-old"));
        assert!(route.recovery_json.is_none());
        assert_eq!(
            db.list_codex_profile_failovers("profile-a")?,
            vec!["provider-failover"]
        );
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
        outer_read_rx.recv().expect("恢复已读取锁外路由");
        manager.disable("profile-a").await?;
        resume_tx.send(()).expect("允许恢复继续");
        restore_task.await.expect("恢复任务未 panic")?;

        assert!(
            !db.get_codex_profile_route("profile-a")?
                .expect("路由仍存在")
                .enabled
        );
        assert!(manager.status("profile-a").await.is_err());
        Ok(())
    }

    /// 一个 Profile 恢复健康检查失败必须停止自身，不能影响其他 Profile 成功恢复。
    #[tokio::test]
    async fn restoring_unhealthy_profile_stops_it_and_keeps_healthy_profile_running(
    ) -> Result<(), AppError> {
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
                recovery_json: None,
                updated_at: 1,
            })?;
            runtimes.insert(
                profile_id.to_string(),
                Arc::new(HealthRuntime {
                    healthy,
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
        assert!(bad_runtime.started.load(Ordering::SeqCst));
        assert!(bad_runtime.stopped.load(Ordering::SeqCst));
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
        assert!(route
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("删除本地凭证失败")));
        assert!(route.recovery_json.is_none());
        Ok(())
    }
}
