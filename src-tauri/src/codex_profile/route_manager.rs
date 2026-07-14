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
        let lock = self.profile_lock(profile_id)?;
        let _guard = lock.lock().await;
        let snapshot = self.provider_snapshot(provider_id, &failover_ids)?;
        let route = self
            .persistence
            .get_route(profile_id)?
            .ok_or_else(|| AppError::InvalidInput("Codex Profile 路由不存在".to_string()))?;
        let old_snapshot = self.provider_snapshot(
            route
                .current_provider_id
                .as_deref()
                .ok_or_else(|| AppError::InvalidInput("Codex Profile 未配置供应商".to_string()))?,
            &self.persistence.list_failovers(profile_id)?,
        )?;
        let runtime = self.runtime(profile_id)?;
        runtime.swap_provider_snapshot(snapshot).await;
        let changed = crate::codex_profile::CodexProfileRoute {
            current_provider_id: Some(provider_id.to_string()),
            last_error: None,
            updated_at: Utc::now().timestamp_millis(),
            ..route.clone()
        };
        if let Err(error) = self.persistence.save_route(&changed) {
            runtime.swap_provider_snapshot(old_snapshot).await;
            self.persist_error(profile_id)?;
            return Err(error);
        }
        if let Err(error) = self
            .persistence
            .replace_failovers(profile_id, &failover_ids)
        {
            runtime.swap_provider_snapshot(old_snapshot).await;
            let restore_error = self.persistence.save_route(&route).err();
            self.persist_switch_rollback_error(profile_id, restore_error.as_ref())?;
            return Err(error);
        }
        Ok(())
    }

    /// 先恢复 Home，再拒绝新请求并排空、停止监听器，最后持久化关闭状态。
    pub async fn disable(&self, profile_id: &str) -> Result<(), AppError> {
        let lock = self.profile_lock(profile_id)?;
        let _guard = lock.lock().await;
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

    /// 切换回滚失败时保留运行时与持久化状态不一致的可重试摘要。
    fn persist_switch_rollback_error(
        &self,
        profile_id: &str,
        restore_error: Option<&AppError>,
    ) -> Result<(), AppError> {
        let operation = if restore_error.is_some() {
            "切换故障转移持久化失败，数据库回滚失败，请重试"
        } else {
            "切换故障转移持久化失败，运行时已回滚"
        };
        self.persist_error_summary(profile_id, operation, restore_error.is_some())
    }
}

#[cfg(test)]
mod codex_route_manager {
    use super::*;
    use crate::codex_profile::{CodexProfile, CodexProfileRoute, CodexRouteRuntimeFuture};
    use crate::provider::Provider;
    use serde_json::json;

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
        assert!(db
            .get_codex_profile_route("custom-profile")?
            .and_then(|route| route.last_error)
            .is_some());
        Ok(())
    }
}
