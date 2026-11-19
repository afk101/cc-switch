//! HTTP代理服务器
//!
//! 基于Axum的HTTP服务器，处理代理请求
//!
//! Uses a manual hyper HTTP/1.1 accept loop with `preserve_header_case(true)` so
//! that the original header-name casing from the CLI client is captured in a
//! `HeaderCaseMap` extension.  This map is later forwarded to the upstream via
//! the hyper-based HTTP client, producing wire-level header casing identical to
//! a direct (non-proxied) CLI request.

use super::{
    browser_guard,
    failover_switch::FailoverSwitchManager,
    handlers,
    log_codes::srv as log_srv,
    provider_router::ProviderRouter,
    providers::{codex_chat_history::CodexChatHistoryStore, gemini_shadow::GeminiShadowStore},
    types::*,
    ProxyError,
};
use crate::database::Database;
use crate::{codex_profile::CodexProfileScope, provider::Provider};
use axum::{
    extract::{DefaultBodyLimit, State},
    http::header::AUTHORIZATION,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Router,
};
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{oneshot, RwLock};
use tokio::task::JoinHandle;

/// Profile 监听器停止流程的可观察状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileListenerState {
    Running,
    StopRequested,
    Stopped,
    StopFailed,
}

/// 代理服务器状态（共享）
#[derive(Clone)]
pub struct ProxyState {
    pub db: Arc<Database>,
    pub config: Arc<RwLock<ProxyConfig>>,
    pub status: Arc<RwLock<ProxyStatus>>,
    pub start_time: Arc<RwLock<Option<std::time::Instant>>>,
    /// 每个应用类型当前使用的 provider (app_type -> (provider_id, provider_name))
    pub current_providers: Arc<RwLock<std::collections::HashMap<String, (String, String)>>>,
    /// 共享的 ProviderRouter（持有熔断器状态，跨请求保持）
    pub provider_router: Arc<ProviderRouter>,
    /// Gemini Native shadow state，用于 thoughtSignature / tool call 回放
    pub gemini_shadow: Arc<GeminiShadowStore>,
    /// Codex Chat bridge history，用于恢复 previous_response_id 指向的 tool call
    pub codex_chat_history: Arc<CodexChatHistoryStore>,
    /// AppHandle，用于发射事件和更新托盘菜单
    pub app_handle: Option<tauri::AppHandle>,
    /// 故障转移切换管理器
    pub failover_manager: Arc<FailoverSwitchManager>,
    /// Profile 路由固定的作用域；空值代表保留原有全局代理语义。
    pub codex_profile_scope: Option<CodexProfileScope>,
    /// Profile 监听器本地认证凭证；只用于入站校验，绝不写入日志或转发上游。
    pub(crate) local_codex_token: Option<Arc<str>>,
    /// Profile 路由关闭开始后拒绝新请求，已进入转发链路的请求不受影响。
    // 历史说明：旧 route_draining 门禁配合独立 in-flight 计数完成排空。
    // 历史说明：旧 profile_in_flight 记录已通过认证并进入处理链路的请求数。
    // 历史说明：旧 profile_drain_notify 在请求完成时唤醒排空等待者。
    pub(crate) profile_rejecting_new_requests: Arc<AtomicBool>,
}

/// 代理HTTP服务器
pub struct ProxyServer {
    config: ProxyConfig,
    state: ProxyState,
    shutdown_tx: Arc<RwLock<Option<oneshot::Sender<()>>>>,
    /// 服务器任务句柄，用于等待服务器实际关闭
    server_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
    /// 停止超时后仍保留 join handle，后续 stop 会继续等待同一任务。
    profile_listener_state: Arc<RwLock<ProfileListenerState>>,
}

#[cfg(test)]
impl ProxyState {
    /// 测试用：用这个数据库、其余都是默认值的状态。
    pub(crate) fn for_test(db: Arc<Database>) -> Self {
        ProxyServer::new(ProxyConfig::default(), db, None).state
    }
}

impl ProxyServer {
    pub fn new(
        config: ProxyConfig,
        db: Arc<Database>,
        app_handle: Option<tauri::AppHandle>,
    ) -> Self {
        // 创建共享的 ProviderRouter（熔断器状态将跨所有请求保持）
        let provider_router = Arc::new(ProviderRouter::new(db.clone()));
        // 创建故障转移切换管理器
        let failover_manager = Arc::new(FailoverSwitchManager::new());

        let state = ProxyState {
            db,
            config: Arc::new(RwLock::new(config.clone())),
            status: Arc::new(RwLock::new(ProxyStatus::default())),
            start_time: Arc::new(RwLock::new(None)),
            current_providers: Arc::new(RwLock::new(std::collections::HashMap::new())),
            provider_router,
            gemini_shadow: Arc::new(GeminiShadowStore::default()),
            codex_chat_history: Arc::new(CodexChatHistoryStore::default()),
            app_handle,
            failover_manager,
            codex_profile_scope: None,
            local_codex_token: None,
            profile_rejecting_new_requests: Arc::new(AtomicBool::new(false)),
        };

        Self {
            config,
            state,
            shutdown_tx: Arc::new(RwLock::new(None)),
            server_handle: Arc::new(RwLock::new(None)),
            profile_listener_state: Arc::new(RwLock::new(ProfileListenerState::Stopped)),
        }
    }

    /// 创建只服务一个 Codex Profile 的代理服务器。
    ///
    /// 此构造器不影响 `ProxyServer::new` 的全局单例语义。每次调用都会新建
    /// ProviderRouter、Codex history、状态容器和请求停接门禁。
    // 历史说明：这里原本还会新建 in-flight 计数载体。
    pub(crate) fn new_for_codex_profile(
        db: Arc<Database>,
        scope: CodexProfileScope,
        local_token: String,
        providers: Vec<Provider>,
    ) -> Self {
        let config = ProxyConfig {
            listen_address: "127.0.0.1".to_string(),
            listen_port: scope.port,
            ..ProxyConfig::default()
        };
        let provider_router =
            Arc::new(ProviderRouter::new_for_codex_profile(db.clone(), providers));
        let failover_manager = Arc::new(FailoverSwitchManager::new());
        let state = ProxyState {
            db,
            config: Arc::new(RwLock::new(config.clone())),
            status: Arc::new(RwLock::new(ProxyStatus::default())),
            start_time: Arc::new(RwLock::new(None)),
            current_providers: Arc::new(RwLock::new(std::collections::HashMap::new())),
            provider_router,
            gemini_shadow: Arc::new(GeminiShadowStore::default()),
            codex_chat_history: Arc::new(CodexChatHistoryStore::default()),
            // Profile 路由无需也不能同步全局 UI/托盘当前供应商。
            app_handle: None,
            failover_manager,
            codex_profile_scope: Some(scope),
            local_codex_token: Some(Arc::<str>::from(local_token)),
            profile_rejecting_new_requests: Arc::new(AtomicBool::new(false)),
        };

        Self {
            config,
            state,
            shutdown_tx: Arc::new(RwLock::new(None)),
            server_handle: Arc::new(RwLock::new(None)),
            profile_listener_state: Arc::new(RwLock::new(ProfileListenerState::Stopped)),
        }
    }

    pub async fn start(&self) -> Result<ProxyServerInfo, ProxyError> {
        // 检查是否已在运行
        if self.shutdown_tx.read().await.is_some() {
            return Err(ProxyError::AlreadyRunning);
        }

        let addr: SocketAddr =
            format!("{}:{}", self.config.listen_address, self.config.listen_port)
                .parse()
                .map_err(|e| ProxyError::BindFailed {
                    message: format!("无效的地址: {e}"),
                    kind: Some(std::io::ErrorKind::InvalidInput),
                })?;

        // 创建关闭通道
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        // 构建路由
        let app = self.build_router();

        // 绑定监听器
        let listener =
            tokio::net::TcpListener::bind(&addr)
                .await
                .map_err(|e| ProxyError::BindFailed {
                    message: e.to_string(),
                    kind: Some(e.kind()),
                })?;
        let local_addr = listener.local_addr().map_err(|e| ProxyError::BindFailed {
            message: e.to_string(),
            kind: Some(e.kind()),
        })?;
        let actual_port = local_addr.port();

        log::info!("[{}] 代理服务器启动于 {local_addr}", log_srv::STARTED);

        // 只有原有全局代理可更新系统代理检测端口；Profile 路由彼此完全独立。
        if self.state.codex_profile_scope.is_none() {
            crate::proxy::http_client::set_proxy_port(actual_port);
        }

        // 保存关闭句柄
        *self.shutdown_tx.write().await = Some(shutdown_tx);

        // 更新状态
        let mut status = self.state.status.write().await;
        status.running = true;
        status.address = self.config.listen_address.clone();
        status.port = actual_port;
        drop(status);

        // 记录启动时间
        *self.state.start_time.write().await = Some(std::time::Instant::now());

        // 启动服务器 — 使用手动 hyper HTTP/1.1 accept loop
        // 开启 preserve_header_case 以捕获客户端请求头的原始大小写
        let state = self.state.clone();
        let handle = tokio::spawn(async move {
            let mut shutdown_rx = shutdown_rx;
            loop {
                tokio::select! {
                    result = listener.accept() => {
                        let (stream, _remote_addr) = match result {
                            Ok(v) => v,
                            Err(e) => {
                                log::error!("[{SRV}] accept 失败: {e}", SRV = log_srv::ACCEPT_ERR);
                                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                                continue;
                            }
                        };

                        let app = app.clone();
                        tokio::spawn(async move {
                            // Peek raw TCP bytes to capture original header casing
                            // before hyper parses (and lowercases) the header names.
                            let original_cases = {
                                let mut peek_buf = vec![0u8; 8192];
                                match stream.peek(&mut peek_buf).await {
                                    Ok(n) => {
                                        let cases = super::hyper_client::OriginalHeaderCases::from_raw_bytes(&peek_buf[..n]);
                                        log::debug!(
                                            "[ProxyServer] Peeked {} bytes, captured {} header casings",
                                            n, cases.cases.len()
                                        );
                                        cases
                                    }
                                    Err(e) => {
                                        log::debug!("[ProxyServer] peek failed (non-fatal): {e}");
                                        super::hyper_client::OriginalHeaderCases::default()
                                    }
                                }
                            };

                            // service_fn 将 axum Router（tower::Service）桥接到 hyper
                            let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                                let mut router = app.clone();
                                let cases = original_cases.clone();
                                async move {
                                    // 将 hyper::body::Incoming 转为 axum::body::Body，保留 extensions
                                    let (mut parts, body) = req.into_parts();

                                    // Insert our own header case map alongside hyper's internal one
                                    parts.extensions.insert(cases);

                                    let body = axum::body::Body::new(body);
                                    let axum_req = http::Request::from_parts(parts, body);
                                    <Router as tower::Service<http::Request<axum::body::Body>>>::call(&mut router, axum_req).await
                                }
                            });

                            if let Err(e) = hyper::server::conn::http1::Builder::new()
                                .preserve_header_case(true)
                                .serve_connection(TokioIo::new(stream), service)
                                .await
                            {
                                // Connection reset / broken pipe 等在代理场景下很常见，debug 级别
                                log::debug!("[{SRV}] connection error: {e}", SRV = log_srv::CONN_ERR);
                            }
                        });
                    }
                    _ = &mut shutdown_rx => {
                        break;
                    }
                }
            }

            // 服务器停止后更新状态
            state.status.write().await.running = false;
            *state.start_time.write().await = None;
        });

        // 保存服务器任务句柄
        *self.server_handle.write().await = Some(handle);
        *self.profile_listener_state.write().await = ProfileListenerState::Running;

        Ok(ProxyServerInfo {
            address: self.config.listen_address.clone(),
            port: actual_port,
            started_at: chrono::Utc::now().to_rfc3339(),
        })
    }

    pub async fn stop(&self) -> Result<(), ProxyError> {
        self.stop_with_timeout(std::time::Duration::from_secs(5))
            .await
    }

    /// 请求停止并等待同一个服务器任务；超时不会丢弃 join handle。
    async fn stop_with_timeout(&self, timeout: std::time::Duration) -> Result<(), ProxyError> {
        // 1. 首次停止发送关闭信号；之后必须继续等待同一个 join handle。
        if matches!(
            *self.profile_listener_state.read().await,
            ProfileListenerState::Running
        ) {
            if let Some(tx) = self.shutdown_tx.write().await.take() {
                let _ = tx.send(());
                *self.profile_listener_state.write().await = ProfileListenerState::StopRequested;
            } else {
                *self.profile_listener_state.write().await = ProfileListenerState::StopFailed;
                return Err(ProxyError::NotRunning);
            }
        }

        // 2. 等待服务器任务结束（带 5 秒超时保护）
        let mut handle_guard = self.server_handle.write().await;
        if let Some(handle) = handle_guard.as_mut() {
            match tokio::time::timeout(timeout, handle).await {
                Ok(Ok(())) => {
                    handle_guard.take();
                    *self.profile_listener_state.write().await = ProfileListenerState::Stopped;
                    log::info!("[{}] 代理服务器已完全停止", log_srv::STOPPED);
                    Ok(())
                }
                Ok(Err(e)) => {
                    handle_guard.take();
                    *self.profile_listener_state.write().await = ProfileListenerState::StopFailed;
                    log::warn!("[{}] 代理服务器任务异常终止: {e}", log_srv::TASK_ERROR);
                    Err(ProxyError::StopFailed(e.to_string()))
                }
                Err(_) => {
                    *self.profile_listener_state.write().await =
                        ProfileListenerState::StopRequested;
                    log::warn!(
                        "[{}] 代理服务器停止超时（5秒），强制继续",
                        log_srv::STOP_TIMEOUT
                    );
                    Err(ProxyError::StopTimeout)
                }
            }
        } else {
            match *self.profile_listener_state.read().await {
                ProfileListenerState::Stopped => Ok(()),
                ProfileListenerState::StopFailed => {
                    Err(ProxyError::StopFailed("监听器停止失败".to_string()))
                }
                _ => Err(ProxyError::NotRunning),
            }
        }
    }

    /// 返回 Profile 监听器停止流程的可观察状态。
    pub async fn profile_listener_state(&self) -> ProfileListenerState {
        *self.profile_listener_state.read().await
    }

    pub async fn get_status(&self) -> ProxyStatus {
        let mut status = self.state.status.read().await.clone();

        // 计算运行时间
        if let Some(start) = *self.state.start_time.read().await {
            status.uptime_seconds = start.elapsed().as_secs();
        }

        // 从 current_providers HashMap 获取每个应用类型当前正在使用的 provider
        let current_providers = self.state.current_providers.read().await;
        status.active_targets = current_providers
            .iter()
            .map(|(app_type, (provider_id, provider_name))| ActiveTarget {
                app_type: app_type.clone(),
                provider_id: provider_id.clone(),
                provider_name: provider_name.clone(),
            })
            .collect();

        status
    }

    /// 更新某个应用类型当前“目标供应商”（用于 UI 展示 active_targets）
    ///
    /// 注意：这不代表该供应商一定已经处理过请求，而是用于“热切换/启用故障转移立即切 P1”
    /// 等场景下，让 UI 能立刻反映最新目标。
    pub async fn set_active_target(&self, app_type: &str, provider_id: &str, provider_name: &str) {
        let mut current_providers = self.state.current_providers.write().await;
        current_providers.insert(
            app_type.to_string(),
            (provider_id.to_string(), provider_name.to_string()),
        );
    }

    fn build_router(&self) -> Router {
        if self.state.codex_profile_scope.is_some() {
            return self.build_codex_profile_router();
        }

        Router::new()
            // 健康检查
            .route("/health", get(handlers::health_check))
            .route("/status", get(handlers::get_status))
            // Claude API (支持带前缀和不带前缀两种格式)
            .route("/v1/messages", post(handlers::handle_messages))
            .route("/claude/v1/messages", post(handlers::handle_messages))
            // Claude Desktop 3P 本地 gateway（独立 provider namespace）
            .route(
                "/claude-desktop/v1/models",
                get(handlers::handle_claude_desktop_models),
            )
            .route(
                "/claude-desktop/v1/messages",
                post(handlers::handle_claude_desktop_messages),
            )
            // OpenAI Chat Completions API (Codex CLI，支持带前缀和不带前缀)
            .route("/chat/completions", post(handlers::handle_chat_completions))
            .route(
                "/v1/chat/completions",
                post(handlers::handle_chat_completions),
            )
            .route(
                "/v1/v1/chat/completions",
                post(handlers::handle_chat_completions),
            )
            .route(
                "/codex/v1/chat/completions",
                post(handlers::handle_chat_completions),
            )
            // OpenAI Models API (Codex CLI reachability check)
            .route("/models", get(handlers::handle_models))
            .route("/v1/models", get(handlers::handle_models))
            // OpenAI Responses API (Codex CLI，支持带前缀和不带前缀)。GET 是 WebSocket
            // 握手：Codex 内置的 openai（代理的官方路由）会先试 WebSocket。
            .route(
                "/responses",
                post(handlers::handle_responses).get(handlers::handle_responses_websocket),
            )
            .route(
                "/v1/responses",
                post(handlers::handle_responses).get(handlers::handle_responses_websocket),
            )
            .route(
                "/v1/v1/responses",
                post(handlers::handle_responses).get(handlers::handle_responses_websocket),
            )
            .route(
                "/codex/v1/responses",
                post(handlers::handle_responses).get(handlers::handle_responses_websocket),
            )
            // Grok Build uses the Responses protocol but has an independent
            // provider namespace and failover queue.
            .route(
                "/grokbuild/v1/responses",
                post(handlers::handle_grokbuild_responses),
            )
            // OpenAI Responses Compact API (Codex CLI 远程压缩，透传)
            .route(
                "/responses/compact",
                post(handlers::handle_responses_compact),
            )
            .route(
                "/v1/responses/compact",
                post(handlers::handle_responses_compact),
            )
            .route(
                "/v1/v1/responses/compact",
                post(handlers::handle_responses_compact),
            )
            .route(
                "/codex/v1/responses/compact",
                post(handlers::handle_responses_compact),
            )
            .route(
                "/grokbuild/v1/responses/compact",
                post(handlers::handle_grokbuild_responses_compact),
            )
            // Codex standalone Alpha Search API. All local aliases normalize to
            // the selected provider's canonical sibling `/alpha/search` route.
            .route("/alpha/search", post(handlers::handle_alpha_search))
            .route("/v1/alpha/search", post(handlers::handle_alpha_search))
            .route("/v1/v1/alpha/search", post(handlers::handle_alpha_search))
            .route(
                "/codex/v1/alpha/search",
                post(handlers::handle_alpha_search),
            )
            // Codex built-in ImageGen still calls the legacy OpenAI Images API.
            .route(
                "/images/generations",
                post(handlers::handle_images_generations),
            )
            .route(
                "/v1/images/generations",
                post(handlers::handle_images_generations),
            )
            .route(
                "/v1/v1/images/generations",
                post(handlers::handle_images_generations),
            )
            .route(
                "/codex/v1/images/generations",
                post(handlers::handle_images_generations),
            )
            // Codex ImageGen posts to `/images/edits` when it references existing images.
            .route("/images/edits", post(handlers::handle_images_edits))
            .route("/v1/images/edits", post(handlers::handle_images_edits))
            .route("/v1/v1/images/edits", post(handlers::handle_images_edits))
            .route(
                "/codex/v1/images/edits",
                post(handlers::handle_images_edits),
            )
            // Gemini API (支持带前缀和不带前缀)
            //
            // 用 `any(..)` 覆盖所有 HTTP 方法：除了 POST `:generateContent` /
            // `:streamGenerateContent` / `:countTokens` 之外，Gemini SDK / CLI 还会发
            // GET `/models`、GET `/models/<id>` 等只读端点。如果只挂 POST，这些 GET
            // 请求会在路由层 404，绕过本地代理的统计、整流和故障转移。
            .route("/v1beta/*path", any(handlers::handle_gemini))
            .route("/gemini/v1beta/*path", any(handlers::handle_gemini))
            // Gemini 的 GA 版本也叫 /v1，给原 SDK 留一条出口
            .route("/gemini/v1/*path", any(handlers::handle_gemini))
            // 提高默认请求体大小限制（避免 413 Payload Too Large）
            .layer(DefaultBodyLimit::max(200 * 1024 * 1024))
            // 拒绝网页发来的请求（最外层，读请求体之前）：见 browser_guard
            .layer(middleware::from_fn_with_state(
                self.state.clone(),
                browser_guard::reject_browser_requests,
            ))
            .with_state(self.state.clone())
    }

    /// 构建仅允许 Codex API 的 Profile 路由。
    fn build_codex_profile_router(&self) -> Router {
        let protected_routes = Router::new()
            .route("/models", get(profile_models))
            .route("/v1/models", get(profile_models))
            .route("/chat/completions", post(handlers::handle_chat_completions))
            .route(
                "/v1/chat/completions",
                post(handlers::handle_chat_completions),
            )
            .route(
                "/v1/v1/chat/completions",
                post(handlers::handle_chat_completions),
            )
            .route(
                "/codex/v1/chat/completions",
                post(handlers::handle_chat_completions),
            )
            .route("/responses", post(handlers::handle_responses))
            .route("/v1/responses", post(handlers::handle_responses))
            .route("/v1/v1/responses", post(handlers::handle_responses))
            .route("/codex/v1/responses", post(handlers::handle_responses))
            .route(
                "/responses/compact",
                post(handlers::handle_responses_compact),
            )
            .route(
                "/v1/responses/compact",
                post(handlers::handle_responses_compact),
            )
            .route(
                "/v1/v1/responses/compact",
                post(handlers::handle_responses_compact),
            )
            .route(
                "/codex/v1/responses/compact",
                post(handlers::handle_responses_compact),
            )
            .route_layer(axum::middleware::from_fn_with_state(
                self.state.clone(),
                reject_new_profile_request_after_auth,
            ))
            .route_layer(axum::middleware::from_fn_with_state(
                self.state.clone(),
                validate_profile_local_token,
            ));

        Router::new()
            .route("/health", get(handlers::health_check))
            .route("/status", get(handlers::get_status))
            .merge(protected_routes)
            .layer(DefaultBodyLimit::max(200 * 1024 * 1024))
            .with_state(self.state.clone())
    }

    /// 在不重启服务的情况下更新运行时配置
    pub async fn apply_runtime_config(&self, config: &ProxyConfig) {
        *self.state.config.write().await = config.clone();
    }

    pub async fn update_circuit_breaker_config_for_app(
        &self,
        app_type: &crate::app_config::AppType,
        config: super::circuit_breaker::CircuitBreakerConfig,
    ) {
        self.state
            .provider_router
            .update_app_configs(app_type, config)
            .await;
    }

    /// 用新快照替换 Profile 路由器后续请求的候选供应商。
    pub(crate) async fn swap_codex_profile_providers(&self, providers: Vec<Provider>) {
        self.state
            .provider_router
            .replace_codex_profile_providers(providers)
            .await;
    }

    /// 立即拒绝该 Profile 后续进入的请求，不等待也不取消已经进入转发链路的请求。
    // 历史说明：旧 begin_profile_draining 仅拒绝该 Profile 后续进入的请求。
    // 历史说明：旧 wait_for_profile_drain 等待已进入转发链路的请求完成，超时后由调用方继续关闭监听器。
    // 历史说明：等待实现先注册 waiter 再读取计数，避免最后一个请求在两者之间完成而漏掉通知。
    pub(crate) fn reject_new_profile_requests(&self) {
        self.state
            .profile_rejecting_new_requests
            .store(true, Ordering::Release);
    }

    /// 返回此服务器独占的 ProviderRouter。
    #[cfg(test)]
    pub(crate) fn provider_router(&self) -> Arc<ProviderRouter> {
        self.state.provider_router.clone()
    }

    /// 返回此服务器独占的 Codex tool-call 历史存储。
    #[cfg(test)]
    pub(crate) fn codex_chat_history(&self) -> Arc<CodexChatHistoryStore> {
        self.state.codex_chat_history.clone()
    }
}

/// Profile 路由的模型探测响应不读取默认 `CODEX_HOME`，避免跨 Home 配置泄漏。
async fn profile_models() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({"models": []}))
}

/// 校验 Profile 专属 Bearer 凭证，并在进入 handler 前删除本地凭证。
async fn validate_profile_local_token(
    State(state): State<ProxyState>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    if state.profile_rejecting_new_requests.load(Ordering::Acquire) {
        return ProxyError::NotRunning.into_response();
    }

    let Some(token) = state.local_codex_token.as_deref() else {
        return ProxyError::AuthError("Codex Profile 本地监听凭证缺失".to_string()).into_response();
    };

    if !validate_and_strip_profile_token(request.headers_mut(), token) {
        return ProxyError::AuthError("Codex Profile 本地监听凭证无效".to_string()).into_response();
    }

    next.run(request).await
}

/// 在认证后再次检查停接门禁，封闭认证检查与进入 handler 之间的并发窗口。
// 历史说明：旧中间件仅在本地凭证校验通过后计入 Profile 请求，并在响应完成后通知排空等待者。
async fn reject_new_profile_request_after_auth(
    State(state): State<ProxyState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if state.profile_rejecting_new_requests.load(Ordering::Acquire) {
        return ProxyError::NotRunning.into_response();
    }
    next.run(request).await
}

/// 校验并剥离 Profile 本地凭证，使后续 Provider adapter 接管上游认证。
fn validate_and_strip_profile_token(headers: &mut axum::http::HeaderMap, token: &str) -> bool {
    let expected = format!("Bearer {token}");
    let valid = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|actual| actual == expected);
    if valid {
        // 本地凭证只证明请求来自所属 CODEX_HOME；Provider adapter 会添加真正的上游认证。
        headers.remove(AUTHORIZATION);
    }
    valid
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    use axum::body::Body;
    use http::{Request, StatusCode};
    use tokio::sync::Notify;
    use tower::Service;

    /// 为 Profile 模型端点构造携带本地凭证的请求。
    fn profile_models_request() -> Request<Body> {
        Request::builder()
            .uri("/models")
            .header(AUTHORIZATION, "Bearer local-listener-token")
            .body(Body::empty())
            .expect("构造 Profile 模型请求")
    }

    /// 历史排空测试覆盖等待完成、通知竞争与超时；即时停接改为验证关闭边界拒绝新请求。
    // 历史说明：排空必须等待已进入链路的 Profile 请求结束。
    // 历史说明：最后一个请求在 waiter 注册前完成时，排空检查不得等待超时通知。
    // 历史说明：排空超时后必须返回 false，调用方据此继续停止监听器。
    #[tokio::test]
    async fn profile_route_rejects_new_requests_after_close_starts() {
        let server = ProxyServer::new_for_codex_profile(
            Arc::new(Database::memory().expect("内存数据库")),
            CodexProfileScope {
                profile_id: "profile-a".to_string(),
                home_path: std::path::PathBuf::from("/tmp/profile-a"),
                port: 16_001,
            },
            "local-listener-token".to_string(),
            vec![],
        );
        let mut router = server.build_codex_profile_router();

        let before_close = router
            .call(profile_models_request())
            .await
            .expect("关闭前请求");
        assert_eq!(before_close.status(), StatusCode::OK);

        server.reject_new_profile_requests();

        let after_close = router
            .call(profile_models_request())
            .await
            .expect("关闭后请求");
        assert_eq!(after_close.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// 停止超时后必须保留同一 join handle，第二次停止可完成。
    #[tokio::test]
    async fn profile_stop_timeout_keeps_join_handle_for_retry() {
        let server = ProxyServer::new(
            ProxyConfig::default(),
            Arc::new(Database::memory().expect("内存数据库")),
            None,
        );
        let (shutdown_tx, _shutdown_rx) = oneshot::channel();
        let release = Arc::new(Notify::new());
        let task_release = release.clone();
        *server.shutdown_tx.write().await = Some(shutdown_tx);
        *server.server_handle.write().await = Some(tokio::spawn(async move {
            task_release.notified().await;
        }));
        *server.profile_listener_state.write().await = ProfileListenerState::Running;

        assert!(matches!(
            server.stop_with_timeout(std::time::Duration::ZERO).await,
            Err(ProxyError::StopTimeout)
        ));
        assert_eq!(
            server.profile_listener_state().await,
            ProfileListenerState::StopRequested
        );
        release.notify_waiters();
        assert!(server
            .stop_with_timeout(std::time::Duration::from_secs(1))
            .await
            .is_ok());
        assert_eq!(
            server.profile_listener_state().await,
            ProfileListenerState::Stopped
        );
    }

    /// 本地 Bearer 凭证必须在进入上游转发前被剥离。
    #[test]
    fn profile_local_token_is_validated_and_not_forwarded_upstream() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            "Bearer local-listener-token".parse().unwrap(),
        );

        assert!(validate_and_strip_profile_token(
            &mut headers,
            "local-listener-token"
        ));
        assert!(headers.get(AUTHORIZATION).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::AppType;
    use crate::provider::{Provider, ProviderMeta};
    use crate::AppError;
    use axum::http::{header, HeaderMap, StatusCode};
    use serde_json::{json, Value};
    use tokio::sync::Mutex;

    #[derive(Debug)]
    struct CapturedRequest {
        path_and_query: String,
        authorization: Option<String>,
        body: Value,
    }

    /// Windows 上监听 socket 不能被子进程继承：从 CC Switch 拉起的终端、CLI 一旦继承它，
    /// 退出后端口仍被占着，下次启动报 os error 10048（#8552）。mio 1.2.1 起才带
    /// WSA_FLAG_NO_HANDLE_INHERIT，这里防止 Cargo.lock 退回旧版本。
    #[cfg(windows)]
    #[tokio::test]
    async fn listener_socket_is_not_inheritable_on_windows() {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT};

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind listener");
        let mut flags = 0u32;
        let ok = unsafe { GetHandleInformation(listener.as_raw_socket() as HANDLE, &mut flags) };
        assert_ne!(ok, 0, "GetHandleInformation failed");
        assert_eq!(
            flags & HANDLE_FLAG_INHERIT,
            0,
            "listener socket is inheritable"
        );
    }

    /// Codex 内置的 openai 先用 WebSocket 连 Responses：握手回 426 它才立刻改走 HTTP。
    #[tokio::test]
    async fn responses_websocket_handshake_is_answered_with_upgrade_required() {
        let db = Arc::new(Database::memory().expect("memory database"));
        let proxy = ProxyServer::new(
            ProxyConfig {
                listen_port: 0,
                ..ProxyConfig::default()
            },
            db,
            None,
        );
        let proxy_info = proxy.start().await.expect("start test proxy");
        let client = reqwest::Client::new();
        for path in [
            "/responses",
            "/v1/responses",
            "/v1/v1/responses",
            "/codex/v1/responses",
        ] {
            let response = client
                .get(format!("http://127.0.0.1:{}{path}", proxy_info.port))
                .header(header::CONNECTION, "Upgrade")
                .header(header::UPGRADE, "websocket")
                .header("sec-websocket-version", "13")
                .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                .send()
                .await
                .expect("websocket handshake");
            assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED, "{path}");
        }
        proxy.stop().await.expect("stop test proxy");
    }

    /// 网页发来的请求（no-cors POST、WebSocket、DNS rebinding）在读请求体之前就被拒绝。
    #[tokio::test]
    async fn browser_originated_requests_are_rejected() {
        let db = Arc::new(Database::memory().expect("memory database"));
        let proxy = ProxyServer::new(
            ProxyConfig {
                listen_port: 0,
                ..ProxyConfig::default()
            },
            db,
            None,
        );
        let info = proxy.start().await.expect("start test proxy");
        let base = format!("http://127.0.0.1:{}", info.port);
        let client = reqwest::Client::builder().no_proxy().build().unwrap();

        let no_cors_post = client
            .post(format!("{base}/v1/messages"))
            .header(header::ORIGIN, "https://evil.example")
            .header(header::CONTENT_TYPE, "text/plain")
            .body(r#"{"model":"x","max_tokens":1,"messages":[]}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(no_cors_post.status(), StatusCode::FORBIDDEN);

        let websocket = client
            .get(format!("{base}/v1/responses"))
            .header(header::ORIGIN, "https://evil.example")
            .header(header::CONNECTION, "Upgrade")
            .header(header::UPGRADE, "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .send()
            .await
            .unwrap();
        assert_eq!(websocket.status(), StatusCode::FORBIDDEN);

        let rebinding = client
            .get(format!("{base}/status"))
            .header(header::HOST, format!("rebind.evil.example:{}", info.port))
            .send()
            .await
            .unwrap();
        assert_eq!(rebinding.status(), StatusCode::FORBIDDEN);

        let cli = client.get(format!("{base}/health")).send().await.unwrap();
        assert_eq!(cli.status(), StatusCode::OK);

        // Claude Desktop 由网关 token 把关，带 Origin 也交给它自己的鉴权（401 而非 403）
        let desktop = client
            .get(format!("{base}/claude-desktop/v1/models"))
            .header(header::ORIGIN, "app://claude")
            .send()
            .await
            .unwrap();
        assert_eq!(desktop.status(), StatusCode::UNAUTHORIZED);

        proxy.stop().await.expect("stop test proxy");
    }

    /// Regression for #3787: a provider accidentally pointing at the proxy must
    /// fail before it creates another request, even with a dynamically bound port.
    #[tokio::test]
    async fn codex_upstream_self_forwarding_is_rejected() {
        let db = Arc::new(Database::memory().expect("memory database"));
        let proxy = ProxyServer::new(
            ProxyConfig {
                listen_port: 0,
                ..ProxyConfig::default()
            },
            db.clone(),
            None,
        );
        let info = proxy.start().await.expect("start proxy");
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap();
        let loopback = std::net::Ipv4Addr::LOCALHOST.to_string();
        for (host, full_url, stream) in
            [(loopback.as_str(), false, true), ("localhost", true, false)]
        {
            let mut provider = Provider::with_id(
                "self-forwarding".to_string(),
                "Self forwarding".to_string(),
                json!({
                    "base_url": format!("http://{host}:{}/v1{}", info.port, if full_url { "/responses" } else { "" }),
                    "auth": {"OPENAI_API_KEY": "upstream-secret"},
                }),
                None,
            );
            provider.meta = Some(ProviderMeta {
                is_full_url: Some(full_url),
                ..ProviderMeta::default()
            });
            db.save_provider(&AppType::Codex, &provider).unwrap();
            db.set_current_provider(&AppType::Codex, &provider.id)
                .unwrap();
            let before = proxy.get_status().await.total_requests;
            let request = client
                .post(format!(
                    "http://{}:{}/v1/responses",
                    info.address, info.port
                ))
                .json(&json!({
                    "model": "gpt-5",
                    "stream": stream,
                    "input": [{"role": "user", "content": "x".repeat(256 * 1024)}],
                }));
            // Bound the regression itself if the guard is ever removed: stop
            // after eight internal requests instead of letting the loop grow.
            let response = tokio::select! {
                response = request.send() => response.expect("send request"),
                _ = async {
                    while proxy.get_status().await.total_requests < before + 8 {
                        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    }
                } => {
                    proxy.stop().await.unwrap();
                    panic!("provider request looped back into the proxy");
                }
            };
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let error: Value = response.json().await.unwrap();
            assert!(error.to_string().contains("own proxy listener"), "{error}");
            assert_eq!(proxy.get_status().await.total_requests, before + 1);
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while proxy.get_status().await.active_connections != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("rejected request releases its connection");
        }
        proxy.stop().await.unwrap();
    }

    /// A base URL pasted as a complete endpoint with the full-URL switch left off
    /// must derive the sibling standalone endpoint instead of having the
    /// standalone path appended to it.
    #[tokio::test]
    async fn codex_standalone_endpoints_derive_from_pasted_full_base_url() {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let capture_handler = {
            let captured = captured.clone();
            move |request: axum::extract::Request| {
                let captured = captured.clone();
                async move {
                    let (parts, _body) = request.into_parts();
                    captured.lock().await.push(CapturedRequest {
                        path_and_query: parts
                            .uri
                            .path_and_query()
                            .map(|value| value.as_str().to_string())
                            .unwrap_or_else(|| parts.uri.path().to_string()),
                        authorization: parts
                            .headers
                            .get(header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .map(ToString::to_string),
                        body: Value::Null,
                    });

                    (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "application/json")],
                        r#"{"created":1,"data":[{"b64_json":"aW1hZ2U="}],"usage":{"input_tokens":7,"output_tokens":11,"total_tokens":18}}"#,
                    )
                }
            }
        };
        let mock_app = Router::new()
            .route("/v1/images/generations", post(capture_handler.clone()))
            .route("/v1/images/edits", post(capture_handler.clone()))
            .route("/Gateway/v1/images/edits", post(capture_handler.clone()))
            .route("/v1/alpha/search", post(capture_handler));
        let mock_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind mock upstream");
        let mock_addr = mock_listener.local_addr().expect("mock upstream address");
        let mock_handle = tokio::spawn(async move {
            axum::serve(mock_listener, mock_app)
                .await
                .expect("serve mock upstream");
        });

        let db = Arc::new(Database::memory().expect("memory database"));
        let proxy = ProxyServer::new(
            ProxyConfig {
                listen_port: 0,
                ..ProxyConfig::default()
            },
            db.clone(),
            None,
        );
        let proxy_info = proxy.start().await.expect("start test proxy");
        let client = reqwest::Client::new();

        let cases = [
            (
                "pasted-mixed-case-chat-completions",
                format!("http://{mock_addr}/v1/Chat/Completions?api-version=CaseValue"),
                "/v1/images/edits",
                "/v1/images/edits?api-version=CaseValue&client_version=0.145.0",
            ),
            (
                "pasted-mixed-case-images-generations",
                format!("http://{mock_addr}/Gateway/v1/Images/Generations/?api-version=CaseValue#fragment"),
                "/v1/images/edits",
                "/Gateway/v1/images/edits?api-version=CaseValue&client_version=0.145.0",
            ),
            (
                "pasted-mixed-case-images-edits",
                format!("http://{mock_addr}/v1/Images/Edits/"),
                "/v1/images/generations",
                "/v1/images/generations?client_version=0.145.0",
            ),
            (
                "pasted-mixed-case-responses-compact",
                format!("http://{mock_addr}/v1/Responses/Compact/"),
                "/v1/alpha/search",
                "/v1/alpha/search?client_version=0.145.0",
            ),
            (
                "pasted-chat-completions",
                format!("http://{mock_addr}/v1/chat/completions"),
                "/v1/images/generations",
                "/v1/images/generations?client_version=0.145.0",
            ),
            (
                "pasted-chat-completions",
                format!("http://{mock_addr}/v1/chat/completions"),
                "/v1/images/edits",
                "/v1/images/edits?client_version=0.145.0",
            ),
            (
                "pasted-images-generations",
                format!("http://{mock_addr}/v1/images/generations?api-version=test"),
                "/v1/images/edits",
                "/v1/images/edits?api-version=test&client_version=0.145.0",
            ),
            (
                "pasted-responses",
                format!("http://{mock_addr}/v1/responses"),
                "/v1/alpha/search",
                "/v1/alpha/search?client_version=0.145.0",
            ),
        ];

        for (provider_id, base_url, local_path, expected_upstream) in cases {
            let provider = Provider::with_id(
                provider_id.to_string(),
                provider_id.to_string(),
                json!({
                    "base_url": base_url,
                    "auth": {"OPENAI_API_KEY": "upstream-secret"}
                }),
                None,
            );
            db.save_provider(&AppType::Codex, &provider)
                .expect("save pasted base URL provider");
            db.set_current_provider(&AppType::Codex, &provider.id)
                .expect("select pasted base URL provider");

            let response = client
                .post(format!(
                    "http://127.0.0.1:{}{local_path}?client_version=0.145.0",
                    proxy_info.port
                ))
                .header(header::AUTHORIZATION, "Bearer client-secret")
                .json(&json!({"model": "gpt-image-1", "prompt": "pasted base URL"}))
                .send()
                .await
                .expect("send images request");

            assert_eq!(
                response.status(),
                StatusCode::OK,
                "{local_path} via {base_url}"
            );
            let request = captured
                .lock()
                .await
                .pop()
                .expect("upstream request captured");
            assert_eq!(
                request.path_and_query, expected_upstream,
                "{local_path} via {base_url}"
            );
            assert_eq!(
                request.authorization.as_deref(),
                Some("Bearer upstream-secret")
            );
        }

        proxy.stop().await.expect("stop test proxy");
        mock_handle.abort();
    }

    /// Codex 对第三方原生 Responses 上游发起 V2 压缩（`/responses`，input 末尾是
    /// `compaction_trigger`）时，上游只回普通消息，代理要补上 Codex 必需的 compaction 条目。
    /// `/responses/compact` 是另一套接口，同样的请求体原样透传，不补条目。
    #[tokio::test]
    async fn codex_compaction_item_is_added_on_responses_but_not_on_compact() {
        const UPSTREAM_SSE: &str = "event: response.output_item.done\n\
data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_1\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"summary so far\"}]}}\n\n\
event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"output\":[]}}\n\n";
        let paths = Arc::new(Mutex::new(Vec::<String>::new()));
        let handler = {
            let paths = paths.clone();
            move |request: axum::extract::Request| {
                let paths = paths.clone();
                async move {
                    paths.lock().await.push(request.uri().path().to_string());
                    (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "text/event-stream")],
                        UPSTREAM_SSE,
                    )
                }
            }
        };
        let mock_app = Router::new()
            .route("/v1/responses", post(handler.clone()))
            .route("/v1/responses/compact", post(handler));
        let mock_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind mock upstream");
        let mock_addr = mock_listener.local_addr().expect("mock upstream address");
        let mock_handle = tokio::spawn(async move {
            axum::serve(mock_listener, mock_app)
                .await
                .expect("serve mock upstream");
        });

        let db = Arc::new(Database::memory().expect("memory database"));
        let provider = Provider::with_id(
            "relay".to_string(),
            "Relay".to_string(),
            json!({
                "base_url": format!("http://{mock_addr}/v1"),
                "auth": {"OPENAI_API_KEY": "upstream-secret"}
            }),
            None,
        );
        db.save_provider(&AppType::Codex, &provider)
            .expect("save provider");
        db.set_current_provider(&AppType::Codex, &provider.id)
            .expect("select provider");
        let proxy = ProxyServer::new(
            ProxyConfig {
                listen_port: 0,
                ..ProxyConfig::default()
            },
            db,
            None,
        );
        let proxy_info = proxy.start().await.expect("start test proxy");
        let client = reqwest::Client::new();
        let compaction_body = json!({
            "model": "gpt-5",
            "stream": true,
            "input": [
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]},
                {"type": "compaction_trigger"}
            ]
        });

        let mut bodies = Vec::new();
        for path in ["/v1/responses", "/v1/responses/compact"] {
            let response = client
                .post(format!("http://127.0.0.1:{}{path}", proxy_info.port))
                .json(&compaction_body)
                .send()
                .await
                .expect("send compaction request");
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            bodies.push(response.text().await.expect("read response body"));
        }
        assert_eq!(
            *paths.lock().await,
            vec![
                "/v1/responses".to_string(),
                "/v1/responses/compact".to_string()
            ]
        );

        let compaction_items = |body: &str| {
            body.split("\n\n")
                .filter_map(|block| {
                    block
                        .lines()
                        .find_map(|line| line.strip_prefix("data: "))
                        .and_then(|data| serde_json::from_str::<Value>(data).ok())
                })
                .filter(|event| {
                    event["type"] == "response.output_item.done"
                        && event["item"]["type"] == "compaction"
                })
                .count()
        };
        assert_eq!(compaction_items(&bodies[0]), 1, "{}", bodies[0]);
        assert!(
            bodies[0].contains("ccswitch-compaction-v1:"),
            "{}",
            bodies[0]
        );
        assert_eq!(compaction_items(&bodies[1]), 0, "{}", bodies[1]);
        assert!(bodies[1].contains("summary so far"), "{}", bodies[1]);

        proxy.stop().await.expect("stop test proxy");
        mock_handle.abort();
    }

    #[tokio::test]
    async fn codex_images_generation_aliases_forward_and_record_usage() {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let mock_app = Router::new().route(
            "/v1/images/generations",
            post({
                let captured = captured.clone();
                move |request: axum::extract::Request| {
                    let captured = captured.clone();
                    async move {
                        let (parts, body) = request.into_parts();
                        let body = axum::body::to_bytes(body, 1024 * 1024)
                            .await
                            .expect("read mock request body");
                        captured.lock().await.push(CapturedRequest {
                            path_and_query: parts
                                .uri
                                .path_and_query()
                                .map(|value| value.as_str().to_string())
                                .unwrap_or_else(|| parts.uri.path().to_string()),
                            authorization: parts
                                .headers
                                .get(header::AUTHORIZATION)
                                .and_then(|value| value.to_str().ok())
                                .map(ToString::to_string),
                            body: serde_json::from_slice(&body).expect("parse mock request body"),
                        });

                        (
                            StatusCode::OK,
                            [(header::CONTENT_TYPE, "application/json")],
                            r#"{"created":1,"data":[{"b64_json":"aW1hZ2U="}],"usage":{"input_tokens":7,"output_tokens":11,"total_tokens":18}}"#,
                        )
                    }
                }
            }),
        );
        let mock_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind mock upstream");
        let mock_addr = mock_listener.local_addr().expect("mock upstream address");
        let mock_handle = tokio::spawn(async move {
            axum::serve(mock_listener, mock_app)
                .await
                .expect("serve mock upstream");
        });

        let db = Arc::new(Database::memory().expect("memory database"));
        let provider = Provider::with_id(
            "images-api-upstream".to_string(),
            "Images API Upstream".to_string(),
            json!({
                "base_url": format!("http://{mock_addr}/v1"),
                "auth": {"OPENAI_API_KEY": "upstream-secret"}
            }),
            None,
        );
        db.save_provider(&AppType::Codex, &provider)
            .expect("save test provider");
        db.set_current_provider(&AppType::Codex, &provider.id)
            .expect("select test provider");

        let proxy = ProxyServer::new(
            ProxyConfig {
                listen_port: 0,
                enable_logging: true,
                ..ProxyConfig::default()
            },
            db.clone(),
            None,
        );
        let proxy_info = proxy.start().await.expect("start test proxy");
        let client = reqwest::Client::new();
        let aliases = [
            "/images/generations",
            "/v1/images/generations",
            "/v1/v1/images/generations",
            "/codex/v1/images/generations",
        ];

        for (index, path) in aliases.iter().enumerate() {
            let response = client
                .post(format!(
                    "http://127.0.0.1:{}{}?client_version=0.145.0",
                    proxy_info.port, path
                ))
                .header(header::AUTHORIZATION, "Bearer client-secret")
                .json(&json!({
                    "model": "gpt-image-1",
                    "prompt": format!("image generation alias {index}")
                }))
                .send()
                .await
                .expect("send images request");

            assert_eq!(response.status(), StatusCode::OK, "alias {path}");
            assert_eq!(
                response.text().await.expect("read proxy response"),
                r#"{"created":1,"data":[{"b64_json":"aW1hZ2U="}],"usage":{"input_tokens":7,"output_tokens":11,"total_tokens":18}}"#,
                "alias {path}"
            );
        }

        let (log_count, input_tokens, output_tokens): (i64, i64, i64) =
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    let totals = {
                        let conn = crate::database::lock_conn!(db.conn);
                        match conn.query_row(
                            "SELECT COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0)
                             FROM proxy_request_logs WHERE provider_id = ?1",
                            ["images-api-upstream"],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        ) {
                            Ok(totals) => totals,
                            Err(error) => panic!("query images usage logs: {error}"),
                        }
                    };

                    if totals.0 == aliases.len() as i64 {
                        break Ok::<_, AppError>(totals);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("images usage logs were not recorded")
            .expect("read images usage logs");
        assert_eq!(log_count, aliases.len() as i64);
        assert_eq!(input_tokens, 7 * aliases.len() as i64);
        assert_eq!(output_tokens, 11 * aliases.len() as i64);

        let mut full_url_provider = Provider::with_id(
            "images-api-full-url".to_string(),
            "Images API Full URL".to_string(),
            json!({
                "base_url": format!("http://{mock_addr}/v1/responses?api-version=test"),
                "auth": {"OPENAI_API_KEY": "full-url-secret"}
            }),
            None,
        );
        full_url_provider.meta = Some(ProviderMeta {
            is_full_url: Some(true),
            ..ProviderMeta::default()
        });
        db.save_provider(&AppType::Codex, &full_url_provider)
            .expect("save full URL images provider");
        db.set_current_provider(&AppType::Codex, &full_url_provider.id)
            .expect("select full URL images provider");

        let response = client
            .post(format!(
                "http://127.0.0.1:{}/v1/images/generations?client_version=0.145.0",
                proxy_info.port
            ))
            .header(header::AUTHORIZATION, "Bearer client-secret")
            .json(&json!({
                "model": "gpt-image-1",
                "prompt": "image generation full URL"
            }))
            .send()
            .await
            .expect("send full URL images request");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.text().await.expect("read full URL image response"),
            r#"{"created":1,"data":[{"b64_json":"aW1hZ2U="}],"usage":{"input_tokens":7,"output_tokens":11,"total_tokens":18}}"#
        );

        let captured = captured.lock().await;
        assert_eq!(captured.len(), aliases.len() + 1);
        for (index, request) in captured.iter().take(aliases.len()).enumerate() {
            assert_eq!(
                request.path_and_query,
                "/v1/images/generations?client_version=0.145.0"
            );
            assert_eq!(
                request.authorization.as_deref(),
                Some("Bearer upstream-secret")
            );
            assert_eq!(request.body["model"], "gpt-image-1");
            assert_eq!(
                request.body["prompt"],
                format!("image generation alias {index}")
            );
        }

        let full_url_request = captured.last().expect("full URL image request captured");
        assert_eq!(
            full_url_request.path_and_query,
            "/v1/images/generations?api-version=test&client_version=0.145.0"
        );
        assert_eq!(
            full_url_request.authorization.as_deref(),
            Some("Bearer full-url-secret")
        );
        assert_eq!(full_url_request.body["model"], "gpt-image-1");
        assert_eq!(full_url_request.body["prompt"], "image generation full URL");

        proxy.stop().await.expect("stop test proxy");
        mock_handle.abort();
    }

    #[tokio::test]
    async fn codex_images_edit_aliases_forward_and_record_usage() {
        // Real Images API responses carry `input_tokens_details` with text/image
        // splits; the shared Codex usage parser must tolerate them.
        const UPSTREAM_BODY: &str = r#"{"created":1,"data":[{"b64_json":"ZWRpdA=="}],"usage":{"input_tokens":13,"output_tokens":17,"total_tokens":30,"input_tokens_details":{"text_tokens":5,"image_tokens":8}}}"#;
        const IMAGE_DATA_URL: &str = "data:image/png;base64,Zm9v";

        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let mock_app = Router::new().route(
            "/v1/images/edits",
            post({
                let captured = captured.clone();
                move |request: axum::extract::Request| {
                    let captured = captured.clone();
                    async move {
                        let (parts, body) = request.into_parts();
                        let body = axum::body::to_bytes(body, 1024 * 1024)
                            .await
                            .expect("read mock request body");
                        captured.lock().await.push(CapturedRequest {
                            path_and_query: parts
                                .uri
                                .path_and_query()
                                .map(|value| value.as_str().to_string())
                                .unwrap_or_else(|| parts.uri.path().to_string()),
                            authorization: parts
                                .headers
                                .get(header::AUTHORIZATION)
                                .and_then(|value| value.to_str().ok())
                                .map(ToString::to_string),
                            body: serde_json::from_slice(&body).expect("parse mock request body"),
                        });

                        (
                            StatusCode::OK,
                            [(header::CONTENT_TYPE, "application/json")],
                            UPSTREAM_BODY,
                        )
                    }
                }
            }),
        );
        let mock_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind mock upstream");
        let mock_addr = mock_listener.local_addr().expect("mock upstream address");
        let mock_handle = tokio::spawn(async move {
            axum::serve(mock_listener, mock_app)
                .await
                .expect("serve mock upstream");
        });

        let db = Arc::new(Database::memory().expect("memory database"));
        let provider = Provider::with_id(
            "images-edit-upstream".to_string(),
            "Images Edit Upstream".to_string(),
            json!({
                "base_url": format!("http://{mock_addr}/v1"),
                "auth": {"OPENAI_API_KEY": "upstream-secret"}
            }),
            None,
        );
        db.save_provider(&AppType::Codex, &provider)
            .expect("save test provider");
        db.set_current_provider(&AppType::Codex, &provider.id)
            .expect("select test provider");

        let proxy = ProxyServer::new(
            ProxyConfig {
                listen_port: 0,
                enable_logging: true,
                ..ProxyConfig::default()
            },
            db.clone(),
            None,
        );
        let proxy_info = proxy.start().await.expect("start test proxy");
        let client = reqwest::Client::new();
        let aliases = [
            "/images/edits",
            "/v1/images/edits",
            "/v1/v1/images/edits",
            "/codex/v1/images/edits",
        ];

        for (index, path) in aliases.iter().enumerate() {
            let response = client
                .post(format!(
                    "http://127.0.0.1:{}{}?client_version=0.145.0",
                    proxy_info.port, path
                ))
                .header(header::AUTHORIZATION, "Bearer client-secret")
                .json(&json!({
                    "model": "gpt-image-2",
                    "prompt": format!("image edit alias {index}"),
                    "images": [{"image_url": IMAGE_DATA_URL}]
                }))
                .send()
                .await
                .expect("send images edit request");

            assert_eq!(response.status(), StatusCode::OK, "alias {path}");
            assert_eq!(
                response.text().await.expect("read proxy response"),
                UPSTREAM_BODY,
                "alias {path}"
            );
        }

        let (log_count, input_tokens, output_tokens): (i64, i64, i64) =
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    let totals = {
                        let conn = crate::database::lock_conn!(db.conn);
                        match conn.query_row(
                            "SELECT COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0)
                             FROM proxy_request_logs WHERE provider_id = ?1",
                            ["images-edit-upstream"],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        ) {
                            Ok(totals) => totals,
                            Err(error) => panic!("query images edit usage logs: {error}"),
                        }
                    };

                    if totals.0 == aliases.len() as i64 {
                        break Ok::<_, AppError>(totals);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("images edit usage logs were not recorded")
            .expect("read images edit usage logs");
        assert_eq!(log_count, aliases.len() as i64);
        assert_eq!(input_tokens, 13 * aliases.len() as i64);
        assert_eq!(output_tokens, 17 * aliases.len() as i64);

        // A full URL pasted for the sibling generations route must derive
        // `/images/edits` instead of posting edit payloads to generations.
        let mut full_url_provider = Provider::with_id(
            "images-edit-full-url".to_string(),
            "Images Edit Full URL".to_string(),
            json!({
                "base_url": format!("http://{mock_addr}/v1/images/generations?api-version=test"),
                "auth": {"OPENAI_API_KEY": "full-url-secret"}
            }),
            None,
        );
        full_url_provider.meta = Some(ProviderMeta {
            is_full_url: Some(true),
            ..ProviderMeta::default()
        });
        db.save_provider(&AppType::Codex, &full_url_provider)
            .expect("save full URL images edit provider");
        db.set_current_provider(&AppType::Codex, &full_url_provider.id)
            .expect("select full URL images edit provider");

        let response = client
            .post(format!(
                "http://127.0.0.1:{}/v1/images/edits?client_version=0.145.0",
                proxy_info.port
            ))
            .header(header::AUTHORIZATION, "Bearer client-secret")
            .json(&json!({
                "model": "gpt-image-2",
                "prompt": "image edit full URL",
                "images": [{"image_url": IMAGE_DATA_URL}]
            }))
            .send()
            .await
            .expect("send full URL images edit request");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .text()
                .await
                .expect("read full URL image edit response"),
            UPSTREAM_BODY
        );

        let captured = captured.lock().await;
        assert_eq!(captured.len(), aliases.len() + 1);
        for (index, request) in captured.iter().take(aliases.len()).enumerate() {
            assert_eq!(
                request.path_and_query,
                "/v1/images/edits?client_version=0.145.0"
            );
            assert_eq!(
                request.authorization.as_deref(),
                Some("Bearer upstream-secret")
            );
            assert_eq!(request.body["model"], "gpt-image-2");
            assert_eq!(request.body["prompt"], format!("image edit alias {index}"));
            assert_eq!(request.body["images"][0]["image_url"], IMAGE_DATA_URL);
        }

        let full_url_request = captured
            .last()
            .expect("full URL image edit request captured");
        assert_eq!(
            full_url_request.path_and_query,
            "/v1/images/edits?api-version=test&client_version=0.145.0"
        );
        assert_eq!(
            full_url_request.authorization.as_deref(),
            Some("Bearer full-url-secret")
        );
        assert_eq!(full_url_request.body["model"], "gpt-image-2");
        assert_eq!(full_url_request.body["prompt"], "image edit full URL");
        assert_eq!(
            full_url_request.body["images"][0]["image_url"],
            IMAGE_DATA_URL
        );

        proxy.stop().await.expect("stop test proxy");
        mock_handle.abort();
    }

    #[tokio::test]
    async fn alpha_search_routes_forward_to_canonical_upstream() {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let mock_app = Router::new().route(
            "/v1/alpha/search",
            post({
                let captured = captured.clone();
                move |request: axum::extract::Request| {
                    let captured = captured.clone();
                    async move {
                        let (parts, body) = request.into_parts();
                        let body = axum::body::to_bytes(body, 1024 * 1024)
                            .await
                            .expect("read mock request body");
                        captured.lock().await.push(CapturedRequest {
                            path_and_query: parts
                                .uri
                                .path_and_query()
                                .map(|value| value.as_str().to_string())
                                .unwrap_or_else(|| parts.uri.path().to_string()),
                            authorization: parts
                                .headers
                                .get(header::AUTHORIZATION)
                                .and_then(|value| value.to_str().ok())
                                .map(ToString::to_string),
                            body: serde_json::from_slice(&body).expect("parse mock request body"),
                        });

                        let mut headers = HeaderMap::new();
                        headers.insert(
                            header::CONTENT_TYPE,
                            "application/json".parse().expect("content type"),
                        );
                        headers.insert(
                            "x-upstream-request-id",
                            "search-1".parse().expect("request id"),
                        );
                        (
                            StatusCode::ACCEPTED,
                            headers,
                            r#"{"encrypted_output":"ciphertext"}"#,
                        )
                    }
                }
            }),
        );
        let mock_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind mock upstream");
        let mock_addr = mock_listener.local_addr().expect("mock upstream address");
        let mock_handle = tokio::spawn(async move {
            axum::serve(mock_listener, mock_app)
                .await
                .expect("serve mock upstream");
        });

        let db = Arc::new(Database::memory().expect("memory database"));
        let provider = Provider::with_id(
            "alpha-search-upstream".to_string(),
            "Alpha Search Upstream".to_string(),
            json!({
                "base_url": format!("http://{mock_addr}/v1"),
                "auth": {"OPENAI_API_KEY": "upstream-secret"}
            }),
            None,
        );
        db.save_provider(&AppType::Codex, &provider)
            .expect("save test provider");
        db.set_current_provider(&AppType::Codex, &provider.id)
            .expect("select test provider");

        let proxy = ProxyServer::new(
            ProxyConfig {
                listen_port: 0,
                enable_logging: false,
                ..ProxyConfig::default()
            },
            db.clone(),
            None,
        );
        let proxy_info = proxy.start().await.expect("start test proxy");
        let client = reqwest::Client::new();
        let aliases = [
            "/alpha/search",
            "/v1/alpha/search",
            "/v1/v1/alpha/search",
            "/codex/v1/alpha/search",
        ];

        for (index, path) in aliases.iter().enumerate() {
            let response = client
                .post(format!(
                    "http://127.0.0.1:{}{}?client_version=0.144.6",
                    proxy_info.port, path
                ))
                .header(header::AUTHORIZATION, "Bearer client-secret")
                .json(&json!({
                    "id": format!("search-{index}"),
                    "model": "gpt-5.6-sol",
                    "commands": {"search_query": [{"q": "test"}]}
                }))
                .send()
                .await
                .expect("send alpha search request");

            assert_eq!(response.status(), StatusCode::ACCEPTED, "alias {path}");
            assert_eq!(
                response
                    .headers()
                    .get("x-upstream-request-id")
                    .and_then(|value| value.to_str().ok()),
                Some("search-1"),
                "alias {path}"
            );
            assert_eq!(
                response.text().await.expect("read proxy response"),
                r#"{"encrypted_output":"ciphertext"}"#,
                "alias {path}"
            );
        }

        // Full-URL providers were the known flaw in the original PR: without a
        // sibling-endpoint rewrite, this request would be posted back to
        // `/v1/responses` instead of `/v1/alpha/search`.
        let mut full_url_provider = Provider::with_id(
            "alpha-search-full-url".to_string(),
            "Alpha Search Full URL".to_string(),
            json!({
                "base_url": format!("http://{mock_addr}/v1/responses?api-version=test"),
                "auth": {"OPENAI_API_KEY": "full-url-secret"}
            }),
            None,
        );
        full_url_provider.meta = Some(ProviderMeta {
            is_full_url: Some(true),
            ..ProviderMeta::default()
        });
        db.save_provider(&AppType::Codex, &full_url_provider)
            .expect("save full URL provider");
        db.set_current_provider(&AppType::Codex, &full_url_provider.id)
            .expect("select full URL provider");

        let response = client
            .post(format!(
                "http://127.0.0.1:{}/v1/alpha/search?client_version=0.144.6",
                proxy_info.port
            ))
            .header(header::AUTHORIZATION, "Bearer client-secret")
            .json(&json!({
                "id": "search-full-url",
                "model": "gpt-5.6-sol",
                "commands": {"search_query": [{"q": "full URL"}]}
            }))
            .send()
            .await
            .expect("send full URL alpha search request");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(
            response.text().await.expect("read full URL response"),
            r#"{"encrypted_output":"ciphertext"}"#
        );

        proxy.stop().await.expect("stop test proxy");
        mock_handle.abort();

        let captured = captured.lock().await;
        assert_eq!(captured.len(), aliases.len() + 1);
        for (index, request) in captured.iter().take(aliases.len()).enumerate() {
            assert_eq!(
                request.path_and_query,
                "/v1/alpha/search?client_version=0.144.6"
            );
            assert_eq!(
                request.authorization.as_deref(),
                Some("Bearer upstream-secret")
            );
            assert_eq!(request.body["id"], format!("search-{index}"));
            assert_eq!(request.body["model"], "gpt-5.6-sol");
            assert_eq!(request.body["commands"]["search_query"][0]["q"], "test");
        }

        let full_url_request = captured.last().expect("full URL request captured");
        assert_eq!(
            full_url_request.path_and_query,
            "/v1/alpha/search?api-version=test&client_version=0.144.6"
        );
        assert_eq!(
            full_url_request.authorization.as_deref(),
            Some("Bearer full-url-secret")
        );
        assert_eq!(full_url_request.body["id"], "search-full-url");
        assert_eq!(
            full_url_request.body["commands"]["search_query"][0]["q"],
            "full URL"
        );
    }
}
