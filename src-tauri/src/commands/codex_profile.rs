//! Codex Profile 的窄 Tauri 命令。

use crate::app_config::AppType;
use crate::codex_profile::{
    CodexProfile, CodexProfileRef, CodexProfileRepository, CodexProfileState, CodexRuntimeStatus,
    SystemHomePathCanonicalizer, SystemPortAvailability,
};
use crate::database::Database;
use crate::error::AppError;
use crate::store::AppState;
use chrono::Utc;
use std::path::Path;
use std::sync::Arc;
use tauri::State;

/// 为命令层创建只负责 Profile 元数据的 Repository。
fn profile_repository(state: &AppState) -> CodexProfileRepository {
    CodexProfileRepository::new(
        state.db.clone(),
        Arc::new(SystemHomePathCanonicalizer),
        Arc::new(SystemPortAvailability),
    )
}

/// 仅更新指定 Profile 的监听端口，默认 Profile 与自定义 Profile 都允许修改端口。
fn update_codex_profile_port_internal(
    db: &Database,
    profile_id: &str,
    listen_port: u16,
) -> Result<(), AppError> {
    if listen_port == 0 {
        return Err(AppError::InvalidInput(
            "Codex Profile 监听端口必须大于 0".to_string(),
        ));
    }
    let mut profile = db.get_codex_profile(profile_id)?;
    let port_in_use = db
        .list_codex_profiles()?
        .into_iter()
        .any(|candidate| candidate.id != profile_id && candidate.listen_port == listen_port);
    if port_in_use {
        return Err(AppError::InvalidInput(format!(
            "Codex Profile 监听端口已被占用: {listen_port}"
        )));
    }
    profile.listen_port = listen_port;
    profile.updated_at = Utc::now().timestamp();
    db.update_codex_profile(&profile)
}

/// 列出全部 Codex Profile；供应商定义仍通过既有全局供应商接口读取。
#[tauri::command]
pub fn list_codex_profiles(state: State<'_, AppState>) -> Result<Vec<CodexProfile>, String> {
    state
        .db
        .list_codex_profiles()
        .map_err(|error| error.to_string())
}

/// 创建一个使用独立 CODEX_HOME 与独立监听端口的自定义 Profile。
#[tauri::command]
pub fn create_codex_profile(
    state: State<'_, AppState>,
    name: String,
    #[allow(non_snake_case)] homePath: String,
    #[allow(non_snake_case)] listenPort: Option<u16>,
) -> Result<CodexProfile, String> {
    profile_repository(&state)
        .create_profile(&name, Path::new(&homePath), listenPort)
        .map_err(|error| error.to_string())
}

/// 修改指定 Profile 的显示名称。
#[tauri::command]
pub fn rename_codex_profile(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] profileId: String,
    name: String,
) -> Result<CodexProfile, String> {
    profile_repository(&state)
        .rename_profile(&profileId, &name)
        .map_err(|error| error.to_string())
}

/// 原子修改指定 Profile 的名称、CODEX_HOME 与监听端口。
#[tauri::command]
pub async fn update_codex_profile(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] profileId: String,
    name: String,
    #[allow(non_snake_case)] homePath: String,
    #[allow(non_snake_case)] listenPort: u16,
) -> Result<CodexProfile, String> {
    let repository = profile_repository(&state);
    state
        .codex_route_manager
        .with_profile_metadata_lock(&profileId, |runtime_status| {
            repository.update_profile(
                &profileId,
                &name,
                Path::new(&homePath),
                listenPort,
                runtime_status,
            )
        })
        .await
        .map_err(|error| error.to_string())
}

/// 在未运行时重绑自定义 Profile 的 CODEX_HOME。
#[tauri::command]
pub async fn rebind_codex_profile(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] profileId: String,
    #[allow(non_snake_case)] homePath: String,
) -> Result<CodexProfile, String> {
    let repository = profile_repository(&state);
    state
        .codex_route_manager
        .with_profile_metadata_lock(&profileId, |runtime_status| {
            repository.rebind_profile(&profileId, Path::new(&homePath), runtime_status)
        })
        .await
        .map_err(|error| error.to_string())
}

/// 修改指定 Profile 的监听端口；运行中的监听器必须先关闭以避免端口状态分裂。
#[tauri::command]
pub async fn update_codex_profile_port(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] profileId: String,
    #[allow(non_snake_case)] listenPort: u16,
) -> Result<bool, String> {
    state
        .codex_route_manager
        .with_profile_metadata_lock(&profileId, |runtime_status| {
            if runtime_status.is_active() {
                return Err(AppError::InvalidInput(
                    "运行中的 Codex Profile 不可修改监听端口".to_string(),
                ));
            }
            update_codex_profile_port_internal(&state.db, &profileId, listenPort)
        })
        .await
        .map(|_| true)
        .map_err(|error| error.to_string())
}

/// 读取一个 Profile 的持久化路由与当前运行时状态。
#[tauri::command]
pub async fn get_codex_profile_state(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] profileId: String,
) -> Result<CodexProfileState, String> {
    let profile = state
        .db
        .get_codex_profile(&profileId)
        .map_err(|error| error.to_string())?;
    let route = state
        .db
        .get_codex_profile_route(&profileId)
        .map_err(|error| error.to_string())?;
    let runtime_status = state
        .codex_route_manager
        .status(&profileId)
        .await
        .unwrap_or(CodexRuntimeStatus::Stopped);
    Ok(CodexProfileState {
        profile,
        route,
        runtime_status,
    })
}

/// 列出仍引用指定全局供应商的 Codex Profile，供删除保护与前端提示使用。
#[tauri::command]
pub fn list_codex_provider_profile_refs(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] providerId: String,
) -> Result<Vec<CodexProfileRef>, String> {
    state
        .db
        .list_codex_provider_profile_refs(&providerId)
        .map_err(|error| error.to_string())
}

/// 为指定 Profile 启用独立路由并接管其 Home 配置。
#[tauri::command]
pub async fn enable_codex_profile_route(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] profileId: String,
    #[allow(non_snake_case)] providerId: String,
    #[allow(non_snake_case)] failoverIds: Option<Vec<String>>,
) -> Result<bool, String> {
    let result = match failoverIds {
        Some(failover_ids) => {
            state
                .codex_route_manager
                .enable(&profileId, &providerId, failover_ids)
                .await
        }
        None => {
            state
                .codex_route_manager
                .enable_preserving_failovers(&profileId, &providerId)
                .await
        }
    };
    result.map(|_| true).map_err(|error| error.to_string())
}

/// 为指定运行中 Profile 切换供应商快照。
#[tauri::command]
pub async fn switch_codex_profile_provider(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] profileId: String,
    #[allow(non_snake_case)] providerId: String,
    #[allow(non_snake_case)] failoverIds: Option<Vec<String>>,
) -> Result<bool, String> {
    let mut provider = state
        .db
        .get_provider_by_id(&providerId, AppType::Codex.as_str())
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Codex 供应商不存在".to_string())?;
    provider.settings_config =
        crate::services::provider::build_effective_settings_with_common_config(
            state.db.as_ref(),
            &AppType::Codex,
            &provider,
        )
        .map_err(|error| error.to_string())?;
    let result = match failoverIds {
        Some(failover_ids) => {
            state
                .codex_route_manager
                .switch_provider_with_effective_settings(&profileId, provider, failover_ids)
                .await
        }
        None => {
            state
                .codex_route_manager
                .switch_provider_with_effective_settings_preserving_failovers(&profileId, provider)
                .await
        }
    };
    result.map(|_| true).map_err(|error| error.to_string())
}

/// 关闭指定 Profile 路由并恢复其 Home 配置。
#[tauri::command]
pub async fn disable_codex_profile_route(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] profileId: String,
) -> Result<bool, String> {
    state
        .codex_route_manager
        .disable(&profileId)
        .await
        .map(|_| true)
        .map_err(|error| error.to_string())
}

/// 删除自定义 Profile 的绑定及私有 token，绝不删除其 Home 目录。
#[tauri::command]
pub async fn delete_codex_profile(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] profileId: String,
) -> Result<bool, String> {
    state
        .codex_route_manager
        .delete_custom_profile(&profileId)
        .await
        .map(|_| true)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use crate::codex_profile::CodexProfile;
    use crate::database::Database;
    use crate::error::AppError;

    /// 更新端口只能影响被明确指定的 Profile。
    #[test]
    fn update_port_changes_only_supplied_profile() -> Result<(), AppError> {
        let db = Database::memory()?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-a".to_string(),
            name: "A".to_string(),
            canonical_home_path: "/tmp/codex-a".to_string(),
            listen_port: 15_722,
            created_at: 1,
            updated_at: 1,
        })?;
        db.insert_codex_profile(&CodexProfile {
            id: "profile-b".to_string(),
            name: "B".to_string(),
            canonical_home_path: "/tmp/codex-b".to_string(),
            listen_port: 15_723,
            created_at: 1,
            updated_at: 1,
        })?;

        super::update_codex_profile_port_internal(&db, "profile-a", 15_730)?;

        assert_eq!(db.get_codex_profile("profile-a")?.listen_port, 15_730);
        assert_eq!(db.get_codex_profile("profile-b")?.listen_port, 15_723);
        Ok(())
    }
}
