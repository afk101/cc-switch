//! Codex Profile 数据传输结构。

use serde::{Deserialize, Serialize};

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
