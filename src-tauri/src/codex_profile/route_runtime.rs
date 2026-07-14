//! 每个 Codex Profile 独占的本地路由运行时。
//!
//! 此模块不复用全局代理的监听器、路由器或会话历史，避免不同 `CODEX_HOME`
//! 之间因相同 response / tool call 标识而串话。

use crate::codex_profile::{CodexProfileScope, CodexRuntimeStatus};
use crate::database::Database;
use crate::provider::Provider;
use crate::proxy::{server::ProxyServer, ProxyError, ProxyServerInfo, ProxyStatus};
use std::sync::Arc;
use tokio::sync::RwLock;

/// 一个 Profile 在某个时刻可用于新请求的供应商快照。
#[derive(Clone)]
pub struct CodexRouteProviderSnapshot {
    primary: Provider,
    failovers: Vec<Provider>,
}

impl CodexRouteProviderSnapshot {
    /// 使用主供应商和按优先级排列的备用供应商创建快照。
    pub fn new(primary: Provider, failovers: Vec<Provider>) -> Self {
        Self { primary, failovers }
    }

    /// 返回本次请求可尝试的供应商顺序。
    pub fn providers(&self) -> Vec<Provider> {
        let mut providers = Vec::with_capacity(self.failovers.len() + 1);
        providers.push(self.primary.clone());
        providers.extend(self.failovers.clone());
        providers
    }
}

/// Profile 路由运行时的最小隔离内核。
///
/// 监听器生命周期会在同一类型中继续组合；此处先确保每个实例持有独立 ProviderRouter，
/// 从根源隔离熔断器和供应商快照。
pub struct RouteRuntime {
    scope: CodexProfileScope,
    server: Arc<ProxyServer>,
    status: Arc<RwLock<CodexRuntimeStatus>>,
}

impl RouteRuntime {
    /// 创建一个绑定到固定 Profile、端口和本地凭证的独立路由运行时。
    pub fn new(
        db: Arc<Database>,
        scope: CodexProfileScope,
        local_token: String,
        snapshot: CodexRouteProviderSnapshot,
    ) -> Self {
        let server = Arc::new(ProxyServer::new_for_codex_profile(
            db,
            scope.clone(),
            local_token,
            snapshot.providers(),
        ));
        Self {
            scope,
            server,
            status: Arc::new(RwLock::new(CodexRuntimeStatus::Stopped)),
        }
    }

    /// 启动固定在该 Profile loopback 端口的监听器。
    pub async fn start(&self) -> Result<ProxyServerInfo, ProxyError> {
        *self.status.write().await = CodexRuntimeStatus::Starting;
        match self.server.start().await {
            Ok(info) => {
                *self.status.write().await = CodexRuntimeStatus::Running;
                log::info!(
                    "[CodexRouteRuntime] Profile {} 已监听 127.0.0.1:{}",
                    self.scope.profile_id,
                    info.port
                );
                Ok(info)
            }
            Err(error) => {
                *self.status.write().await = CodexRuntimeStatus::Failed {
                    message: error.to_string(),
                };
                Err(error)
            }
        }
    }

    /// 返回此 Profile 监听器的当前健康状态。
    pub async fn health_check(&self) -> bool {
        self.server.get_status().await.running
    }

    /// 替换仅供新请求使用的供应商快照。
    pub async fn swap_provider_snapshot(&self, snapshot: CodexRouteProviderSnapshot) {
        self.server
            .swap_codex_profile_providers(snapshot.providers())
            .await;
    }

    /// 拒绝新请求并保留已进入转发链路的请求完成。
    pub async fn begin_draining(&self) {
        self.server.begin_profile_draining();
        *self.status.write().await = CodexRuntimeStatus::Stopping;
        log::info!(
            "[CodexRouteRuntime] Profile {} 开始排空端口 {}",
            self.scope.profile_id,
            self.scope.port
        );
    }

    /// 停止该 Profile 专属监听器。
    pub async fn stop(&self) -> Result<(), ProxyError> {
        *self.status.write().await = CodexRuntimeStatus::Stopping;
        let result = self.server.stop().await;
        *self.status.write().await = match &result {
            Ok(()) => CodexRuntimeStatus::Stopped,
            Err(error) => CodexRuntimeStatus::Failed {
                message: error.to_string(),
            },
        };
        result
    }

    /// 返回运行时生命周期状态，不暴露内部可变状态。
    pub async fn status(&self) -> CodexRuntimeStatus {
        self.status.read().await.clone()
    }

    /// 返回 Profile 独占的代理统计。
    pub async fn proxy_status(&self) -> ProxyStatus {
        self.server.get_status().await
    }

    /// 为测试构造不绑定端口的独立路由器实例。
    #[cfg(test)]
    fn for_test(db: Arc<Database>, snapshot: CodexRouteProviderSnapshot) -> Self {
        Self::new(
            db,
            CodexProfileScope {
                profile_id: "test-profile".to_string(),
                home_path: std::path::PathBuf::from("/tmp/test-codex-home"),
                port: 0,
            },
            "test-local-token".to_string(),
            snapshot,
        )
    }

    /// 记录当前 Profile 内某供应商的一次失败。
    #[cfg(test)]
    async fn record_provider_failure(&self, provider_id: &str) {
        for _ in 0..5 {
            let _ = self
                .server
                .provider_router()
                .record_result(provider_id, "codex", false, false, None)
                .await;
        }
    }

    /// 返回当前请求会尝试的供应商标识。
    #[cfg(test)]
    async fn select_provider_ids(&self) -> Vec<String> {
        self.server
            .provider_router()
            .select_providers("codex")
            .await
            .expect("Profile 路由器应有可用供应商")
            .into_iter()
            .map(|provider| provider.id)
            .collect()
    }

    /// 返回该 Profile 独占的 Codex 历史存储，仅供测试验证隔离边界。
    #[cfg(test)]
    fn codex_chat_history(
        &self,
    ) -> Arc<crate::proxy::providers::codex_chat_history::CodexChatHistoryStore> {
        self.server.codex_chat_history()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::provider::Provider;
    use serde_json::json;
    use std::sync::Arc;

    /// A Profile 的熔断记录与候选供应商不能影响 B Profile。
    #[tokio::test]
    async fn route_runtimes_isolate_breakers_and_failover() {
        let db = Arc::new(Database::memory().expect("创建内存数据库"));
        let primary_a = Provider::with_id(
            "provider-a".to_string(),
            "A 主".to_string(),
            json!({}),
            None,
        );
        let fallback_a = Provider::with_id(
            "provider-a-fallback".to_string(),
            "A 备".to_string(),
            json!({}),
            None,
        );
        let primary_b = Provider::with_id(
            "provider-b".to_string(),
            "B 主".to_string(),
            json!({}),
            None,
        );

        let runtime_a = RouteRuntime::for_test(
            db.clone(),
            CodexRouteProviderSnapshot::new(primary_a.clone(), vec![fallback_a.clone()]),
        );
        let runtime_b = RouteRuntime::for_test(
            db,
            CodexRouteProviderSnapshot::new(primary_b.clone(), Vec::new()),
        );

        runtime_a.record_provider_failure(&primary_a.id).await;

        assert_eq!(runtime_a.select_provider_ids().await, vec![fallback_a.id]);
        assert_eq!(runtime_b.select_provider_ids().await, vec![primary_b.id]);
    }

    /// 相同 response/call 标识在不同 Profile 只能恢复各自的 tool-call 历史。
    #[tokio::test]
    async fn route_runtimes_isolate_codex_chat_history_for_identical_ids() {
        let db = Arc::new(Database::memory().expect("创建内存数据库"));
        let runtime_a = RouteRuntime::for_test(
            db.clone(),
            CodexRouteProviderSnapshot::new(
                Provider::with_id("provider-a".to_string(), "A".to_string(), json!({}), None),
                Vec::new(),
            ),
        );
        let runtime_b = RouteRuntime::for_test(
            db,
            CodexRouteProviderSnapshot::new(
                Provider::with_id("provider-b".to_string(), "B".to_string(), json!({}), None),
                Vec::new(),
            ),
        );

        runtime_a
            .codex_chat_history()
            .record_response(&json!({
                "id": "response-shared",
                "output": [{
                    "type": "function_call",
                    "call_id": "call-shared",
                    "name": "only-a",
                    "arguments": "{}"
                }]
            }))
            .await;
        runtime_b
            .codex_chat_history()
            .record_response(&json!({
                "id": "response-shared",
                "output": [{
                    "type": "function_call",
                    "call_id": "call-shared",
                    "name": "only-b",
                    "arguments": "{}"
                }]
            }))
            .await;

        let mut request_a = json!({
            "previous_response_id": "response-shared",
            "input": [{"type": "function_call_output", "call_id": "call-shared", "output": "ok"}]
        });
        let mut request_b = request_a.clone();
        assert_eq!(
            runtime_a
                .codex_chat_history()
                .enrich_request(&mut request_a)
                .await,
            1
        );
        assert_eq!(
            runtime_b
                .codex_chat_history()
                .enrich_request(&mut request_b)
                .await,
            1
        );
        assert_eq!(request_a["input"][0]["name"], "only-a");
        assert_eq!(request_b["input"][0]["name"], "only-b");
    }

    /// 替换快照只影响替换后的选择结果，已经取得的请求候选列表保持不变。
    #[tokio::test]
    async fn route_runtime_swaps_snapshot_for_new_requests_only() {
        let db = Arc::new(Database::memory().expect("创建内存数据库"));
        let provider_a =
            Provider::with_id("provider-a".to_string(), "A".to_string(), json!({}), None);
        let provider_b =
            Provider::with_id("provider-b".to_string(), "B".to_string(), json!({}), None);
        let runtime = RouteRuntime::for_test(
            db,
            CodexRouteProviderSnapshot::new(provider_a.clone(), Vec::new()),
        );

        let request_started_before_swap = runtime.select_provider_ids().await;
        runtime
            .swap_provider_snapshot(CodexRouteProviderSnapshot::new(
                provider_b.clone(),
                Vec::new(),
            ))
            .await;

        assert_eq!(request_started_before_swap, vec![provider_a.id]);
        assert_eq!(runtime.select_provider_ids().await, vec![provider_b.id]);
    }
}
