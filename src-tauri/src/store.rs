use crate::codex_profile::{
    CodexHomeConfigService, CodexProfileSecretStore, CodexRouteManager,
    SystemCodexRouteRuntimeFactory,
};
use crate::database::Database;
use crate::services::{ProxyService, UsageCache};
use std::sync::Arc;

/// 全局应用状态
pub struct AppState {
    pub db: Arc<Database>,
    pub proxy_service: ProxyService,
    pub codex_route_manager: CodexRouteManager,
    pub usage_cache: Arc<UsageCache>,
}

impl AppState {
    /// 创建新的应用状态
    pub fn new(db: Arc<Database>) -> Self {
        let proxy_service = ProxyService::new(db.clone());
        let codex_route_manager = CodexRouteManager::new(
            db.clone(),
            Arc::new(CodexHomeConfigService::system()),
            Arc::new(CodexProfileSecretStore::new()),
            Arc::new(SystemCodexRouteRuntimeFactory::new(db.clone())),
        );

        Self {
            db,
            proxy_service,
            codex_route_manager,
            usage_cache: Arc::new(UsageCache::new()),
        }
    }
}
