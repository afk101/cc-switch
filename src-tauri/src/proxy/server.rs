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
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Router,
};
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::{oneshot, Notify, RwLock};
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
    /// Profile 进入排空阶段后拒绝新请求，已有请求仍由其独立 in-flight 计数完成。
    pub(crate) route_draining: Arc<AtomicBool>,
    /// Profile 已通过认证并进入处理链路的请求数。
    pub(crate) profile_in_flight: Arc<AtomicUsize>,
    /// 请求完成时唤醒排空等待者。
    pub(crate) profile_drain_notify: Arc<Notify>,
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

impl ProxyServer {
    pub fn new(
        config: ProxyConfig,
        db: Arc<Database>,
        app_handle: Option<tauri::AppHandle>,
    ) -> Self {
        // 创建共享的 ProviderRouter（熔断器状态将跨所有请求保持）
        let provider_router = Arc::new(ProviderRouter::new(db.clone()));
        // 创建故障转移切换管理器
        let failover_manager = Arc::new(FailoverSwitchManager::new(db.clone()));

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
            route_draining: Arc::new(AtomicBool::new(false)),
            profile_in_flight: Arc::new(AtomicUsize::new(0)),
            profile_drain_notify: Arc::new(Notify::new()),
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
    /// ProviderRouter、Codex history、状态容器和 in-flight 计数载体。
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
        let failover_manager = Arc::new(FailoverSwitchManager::new(db.clone()));
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
            route_draining: Arc::new(AtomicBool::new(false)),
            profile_in_flight: Arc::new(AtomicUsize::new(0)),
            profile_drain_notify: Arc::new(Notify::new()),
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
                .map_err(|e| ProxyError::BindFailed(format!("无效的地址: {e}")))?;

        // 创建关闭通道
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        // 构建路由
        let app = self.build_router();

        // 绑定监听器
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| ProxyError::BindFailed(e.to_string()))?;
        let local_addr = listener
            .local_addr()
            .map_err(|e| ProxyError::BindFailed(e.to_string()))?;
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
            // OpenAI Responses API (Codex CLI，支持带前缀和不带前缀)
            .route("/responses", post(handlers::handle_responses))
            .route("/v1/responses", post(handlers::handle_responses))
            .route("/v1/v1/responses", post(handlers::handle_responses))
            .route("/codex/v1/responses", post(handlers::handle_responses))
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
                track_profile_request,
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

    /// 热更新熔断器配置
    ///
    /// 将新配置应用到所有已创建的熔断器实例
    pub async fn update_circuit_breaker_configs(
        &self,
        config: super::circuit_breaker::CircuitBreakerConfig,
    ) {
        self.state.provider_router.update_all_configs(config).await;
    }

    pub async fn update_circuit_breaker_config_for_app(
        &self,
        app_type: &str,
        config: super::circuit_breaker::CircuitBreakerConfig,
    ) {
        self.state
            .provider_router
            .update_app_configs(app_type, config)
            .await;
    }

    /// 重置指定 Provider 的熔断器
    pub async fn reset_provider_circuit_breaker(&self, provider_id: &str, app_type: &str) {
        self.state
            .provider_router
            .reset_provider_breaker(provider_id, app_type)
            .await;
    }

    /// 用新快照替换 Profile 路由器后续请求的候选供应商。
    pub(crate) async fn swap_codex_profile_providers(&self, providers: Vec<Provider>) {
        self.state
            .provider_router
            .replace_codex_profile_providers(providers)
            .await;
    }

    /// 进入排空阶段，仅拒绝该 Profile 后续进入的请求。
    pub(crate) fn begin_profile_draining(&self) {
        self.state.route_draining.store(true, Ordering::Release);
    }

    /// 等待已进入 Profile 转发链路的请求完成，超时交由调用方继续关闭监听器。
    pub(crate) async fn wait_for_profile_drain(&self, timeout: std::time::Duration) -> bool {
        let wait = async {
            loop {
                // 先注册 waiter 再读取计数，避免最后一个请求在两者之间完成而漏掉通知。
                let notified = self.state.profile_drain_notify.notified();
                if self.state.profile_in_flight.load(Ordering::Acquire) == 0 {
                    return;
                }
                notified.await;
            }
        };
        tokio::time::timeout(timeout, wait).await.is_ok()
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
    if state.route_draining.load(Ordering::Acquire) {
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

/// 仅在本地凭证校验通过后计入 Profile 请求，并在响应完成后通知排空等待者。
async fn track_profile_request(
    State(state): State<ProxyState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    state.profile_in_flight.fetch_add(1, Ordering::AcqRel);
    if state.route_draining.load(Ordering::Acquire) {
        state.profile_in_flight.fetch_sub(1, Ordering::AcqRel);
        state.profile_drain_notify.notify_waiters();
        return ProxyError::NotRunning.into_response();
    }
    let response = next.run(request).await;
    state.profile_in_flight.fetch_sub(1, Ordering::AcqRel);
    state.profile_drain_notify.notify_waiters();
    response
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
mod tests {
    use super::*;

    /// 排空必须等待已进入链路的 Profile 请求结束。
    #[tokio::test]
    async fn profile_drain_waits_for_in_flight_request_completion() {
        let server = ProxyServer::new(
            ProxyConfig::default(),
            Arc::new(Database::memory().expect("内存数据库")),
            None,
        );
        server.state.profile_in_flight.store(1, Ordering::Release);
        let in_flight = server.state.profile_in_flight.clone();
        let notify = server.state.profile_drain_notify.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            in_flight.fetch_sub(1, Ordering::AcqRel);
            notify.notify_waiters();
        });

        assert!(
            server
                .wait_for_profile_drain(std::time::Duration::from_millis(100))
                .await
        );
    }

    /// 最后一个请求在 waiter 注册前完成时，排空检查不得等待超时通知。
    #[tokio::test]
    async fn profile_drain_succeeds_when_last_request_finishes_before_waiter_registration() {
        let server = ProxyServer::new(
            ProxyConfig::default(),
            Arc::new(Database::memory().expect("内存数据库")),
            None,
        );
        server.state.profile_in_flight.store(1, Ordering::Release);
        server
            .state
            .profile_in_flight
            .fetch_sub(1, Ordering::AcqRel);
        server.state.profile_drain_notify.notify_waiters();

        assert!(
            server
                .wait_for_profile_drain(std::time::Duration::ZERO)
                .await
        );
    }

    /// 排空超时后必须返回 false，调用方据此继续停止监听器。
    #[tokio::test]
    async fn profile_drain_returns_false_after_timeout() {
        let server = ProxyServer::new(
            ProxyConfig::default(),
            Arc::new(Database::memory().expect("内存数据库")),
            None,
        );
        server.state.profile_in_flight.store(1, Ordering::Release);

        assert!(
            !server
                .wait_for_profile_drain(std::time::Duration::from_millis(10))
                .await
        );
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
