//! Codex Profile 独立路由生命周期编排。

use crate::app_config::AppType;
use crate::codex_profile::{
    CodexHomeConfigService, CodexProfileScope, CodexProfileSecretStore, CodexRouteProviderSnapshot,
    CodexRouteRuntime, CodexRouteRuntimeFactory, CodexRuntimeStatus,
};
use crate::database::Database;
use crate::error::AppError;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

/// 每个 Profile 的路由生命周期管理器，不使用全局 ProxyService listener slot。
pub struct CodexRouteManager {
    db: Arc<Database>,
    home_config: Arc<CodexHomeConfigService>,
    secret_store: Arc<CodexProfileSecretStore>,
    runtime_factory: Arc<dyn CodexRouteRuntimeFactory>,
    runtimes: Mutex<HashMap<String, Arc<dyn CodexRouteRuntime>>>,
    locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl CodexRouteManager {
    /// 使用注入依赖创建 Profile 隔离路由管理器。
    pub fn new(
        db: Arc<Database>,
        home_config: Arc<CodexHomeConfigService>,
        secret_store: Arc<CodexProfileSecretStore>,
        runtime_factory: Arc<dyn CodexRouteRuntimeFactory>,
    ) -> Self {
        Self {
            db,
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
        let profile = self.db.get_codex_profile(profile_id)?;
        profile.validate_route_operation()?;
        let snapshot = self.provider_snapshot(provider_id, &failover_ids)?;
        let plan = self.home_config.build_profile_route_plan(
            std::path::Path::new(&profile.canonical_home_path),
            profile.listen_port,
            self.db
                .get_provider_by_id(provider_id, AppType::Codex.as_str())?
                .as_ref(),
        )?;
        let token = self.secret_store.create(profile_id)?;
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
            let _ = runtime.stop().await;
            self.persist_error(profile_id)?;
            return Err(AppError::Message(
                "Codex Profile 路由健康检查失败".to_string(),
            ));
        }
        if let Err(error) = self.home_config.apply_route_plan(&plan) {
            let _ = runtime.stop().await;
            self.persist_error(profile_id)?;
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
        if let Err(error) = self.db.save_codex_profile_route(&route) {
            let _ = self.home_config.restore(&plan);
            let _ = runtime.stop().await;
            self.persist_error(profile_id)?;
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
            .db
            .get_codex_profile_route(profile_id)?
            .ok_or_else(|| AppError::InvalidInput("Codex Profile 路由不存在".to_string()))?;
        let old_snapshot = self.provider_snapshot(
            route
                .current_provider_id
                .as_deref()
                .ok_or_else(|| AppError::InvalidInput("Codex Profile 未配置供应商".to_string()))?,
            &self.db.list_codex_profile_failovers(profile_id)?,
        )?;
        let runtime = self.runtime(profile_id)?;
        runtime.swap_provider_snapshot(snapshot).await;
        let changed = crate::codex_profile::CodexProfileRoute {
            current_provider_id: Some(provider_id.to_string()),
            last_error: None,
            updated_at: Utc::now().timestamp_millis(),
            ..route.clone()
        };
        if let Err(error) = self.db.save_codex_profile_route(&changed) {
            runtime.swap_provider_snapshot(old_snapshot).await;
            self.persist_error(profile_id)?;
            return Err(error);
        }
        if let Err(error) = self
            .db
            .replace_codex_profile_failovers(profile_id, &failover_ids)
        {
            runtime.swap_provider_snapshot(old_snapshot).await;
            let _ = self.db.save_codex_profile_route(&route);
            self.persist_error(profile_id)?;
            return Err(error);
        }
        Ok(())
    }

    /// 先恢复 Home，再拒绝新请求并排空、停止监听器，最后持久化关闭状态。
    pub async fn disable(&self, profile_id: &str) -> Result<(), AppError> {
        let lock = self.profile_lock(profile_id)?;
        let _guard = lock.lock().await;
        let profile = self.db.get_codex_profile(profile_id)?;
        let route = self
            .db
            .get_codex_profile_route(profile_id)?
            .ok_or_else(|| AppError::InvalidInput("Codex Profile 路由不存在".to_string()))?;
        if let Some(backup) = &route.live_backup_json {
            self.home_config
                .restore_backup(std::path::Path::new(&profile.canonical_home_path), backup)?;
        }
        if let Ok(runtime) = self.runtime(profile_id) {
            runtime.begin_draining().await;
            runtime.stop().await.map_err(AppError::Message)?;
        }
        self.db
            .save_codex_profile_route(&crate::codex_profile::CodexProfileRoute {
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
        let profile = self.db.get_codex_profile(profile_id)?;
        profile.validate_delete()?;
        if self
            .db
            .get_codex_profile_route(profile_id)?
            .is_some_and(|route| route.enabled)
        {
            self.disable(profile_id).await?;
        }
        self.db.delete_codex_profile(profile_id)?;
        self.secret_store.delete(profile_id)
    }

    /// 启动时逐个恢复已启用 Profile；单个失败会被隔离并记录。
    pub async fn restore_enabled_profiles(&self) -> Result<(), AppError> {
        for profile in self.db.list_codex_profiles()? {
            let Some(route) = self.db.get_codex_profile_route(&profile.id)? else {
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
                    &self.db.list_codex_profile_failovers(&profile.id)?,
                )?;
                let token = self.secret_store.create(&profile.id)?;
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
            if result.is_err() {
                let _ = self.persist_error(&profile.id);
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
            .db
            .get_provider_by_id(provider_id, AppType::Codex.as_str())?
            .ok_or_else(|| AppError::InvalidInput(format!("Codex 供应商不存在: {provider_id}")))?;
        let failovers = failover_ids
            .iter()
            .map(|id| {
                self.db
                    .get_provider_by_id(id, AppType::Codex.as_str())?
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
        if let Some(route) = self.db.get_codex_profile_route(profile_id)? {
            self.db
                .save_codex_profile_route(&crate::codex_profile::CodexProfileRoute {
                    last_error: Some("Codex Profile 路由操作失败，请查看日志".to_string()),
                    updated_at: Utc::now().timestamp_millis(),
                    ..route
                })?;
        }
        Ok(())
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
}
