//! Codex Profile 数据传输结构。

use crate::codex_profile::constants::{
    DEFAULT_CODEX_PROFILE_ID, DEFAULT_CODEX_PROFILE_NAME, LEGACY_CODEX_ROUTE_PORT,
};
use crate::error::AppError;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 一个独立 CODEX_HOME 的持久化描述。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodexProfile {
    pub id: String,
    pub name: String,
    pub canonical_home_path: String,
    pub listen_port: u16,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Profile 的当前供应商路由配置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodexProfileRoute {
    pub profile_id: String,
    pub current_provider_id: Option<String>,
    pub enabled: bool,
    pub updated_at: i64,
}

/// 引用某个全局供应商的 Profile 摘要。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodexProfileRef {
    pub id: String,
    pub name: String,
}

/// 监听器创建后不可变的 Codex Profile 请求作用域。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodexProfileScope {
    pub profile_id: String,
    pub home_path: PathBuf,
    pub port: u16,
}

/// 单个 Codex Profile 路由实例的生命周期状态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CodexRuntimeStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed { message: String },
}

impl CodexRuntimeStatus {
    /// 判断是否处于不能修改绑定信息的生命周期阶段。
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }
}

/// Profile 的持久化配置与运行时状态快照。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodexProfileState {
    pub profile: CodexProfile,
    pub route: Option<CodexProfileRoute>,
    pub runtime_status: CodexRuntimeStatus,
}

impl CodexProfile {
    /// 构造固定绑定 `~/.codex` 的默认 Profile。
    pub fn default_profile() -> Result<Self, AppError> {
        let home_dir = dirs::home_dir()
            .ok_or_else(|| AppError::Config("无法确定当前用户主目录".to_string()))?;
        let now = Utc::now().timestamp();

        Ok(Self {
            id: DEFAULT_CODEX_PROFILE_ID.to_string(),
            name: DEFAULT_CODEX_PROFILE_NAME.to_string(),
            canonical_home_path: home_dir.join(".codex").to_string_lossy().into_owned(),
            listen_port: LEGACY_CODEX_ROUTE_PORT,
            created_at: now,
            updated_at: now,
        })
    }

    /// 校验官方订阅操作；默认 Profile 与其他 Profile 权限相同。
    pub fn validate_official_operation(&self) -> Result<(), AppError> {
        Ok(())
    }

    /// 校验路由操作；默认 Profile 与其他 Profile 权限相同。
    pub fn validate_route_operation(&self) -> Result<(), AppError> {
        Ok(())
    }

    /// 校验 Profile 是否允许重新绑定 CODEX_HOME。
    pub fn validate_rebind(&self) -> Result<(), AppError> {
        self.validate_mutable()
    }

    /// 校验 Profile 是否允许删除。
    pub fn validate_delete(&self) -> Result<(), AppError> {
        self.validate_mutable()
    }

    /// 仅默认 Profile 的绑定与删除操作不可变。
    fn validate_mutable(&self) -> Result<(), AppError> {
        if self.id == DEFAULT_CODEX_PROFILE_ID {
            return Err(AppError::DefaultCodexProfileImmutable);
        }
        Ok(())
    }
}
