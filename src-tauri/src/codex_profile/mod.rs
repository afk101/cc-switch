//! Codex Profile 的领域模型与持久化编排。
//!
//! 本模块不读写 Home 配置文件，也不控制监听器运行时。

mod constants;
mod migration;
mod model;
mod repository;

pub use constants::*;
pub use migration::{CodexProfileMigrationService, LegacyCodexProfileSnapshot};
pub use model::{
    CodexProfile, CodexProfileRef, CodexProfileRoute, CodexProfileScope, CodexProfileState,
    CodexRuntimeStatus,
};
pub use repository::{
    CodexProfileRepository, HomePathCanonicalizer, PortAvailability, SystemHomePathCanonicalizer,
    SystemPortAvailability,
};
