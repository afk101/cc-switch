//! Codex Profile 的领域模型与持久化编排。
//!
//! 除 `home_config` 外，本模块的持久化域代码不读写 Home 配置文件，也不控制监听器运行时。

mod constants;
mod home_config;
mod migration;
mod model;
mod repository;
mod route_manager;
mod route_runtime;
mod secret_store;

pub use constants::*;
pub(crate) use home_config::build_codex_route_toml_base;
pub use home_config::{
    build_codex_profile_route_toml, CodexHomeConfigService, CodexHomeFileOps,
    CodexHomeReconcileOwnership, CodexLiveConfigSnapshot, CodexRouteConfigPlan,
    SystemCodexHomeFileOps,
};
pub use migration::{
    CodexProfileMigrationResult, CodexProfileMigrationService, LegacyCodexProfileSnapshot,
    MigratedEnabledCodexProfile,
};
pub use model::{
    CodexProfile, CodexProfileRef, CodexProfileRoute, CodexProfileScope, CodexProfileState,
    CodexRuntimeStatus,
};
pub use repository::{
    CodexProfileRepository, HomePathCanonicalizer, PortAvailability, SystemHomePathCanonicalizer,
    SystemPortAvailability,
};
pub use route_manager::{CodexProfileRoutePersistence, CodexProfileTokenStore, CodexRouteManager};
pub use route_runtime::{
    CodexRouteProviderSnapshot, CodexRouteRuntime, CodexRouteRuntimeFactory,
    CodexRouteRuntimeFuture, RouteRuntime, SystemCodexRouteRuntimeFactory,
};
pub use secret_store::CodexProfileSecretStore;
