use serde_json::{json, Value};

use crate::error::AppError;
use crate::services::{model_pricing, PromptService, ProviderService};
use crate::settings;
use crate::store::AppState;

use crate::codex_profile::CodexExplicitSyncResult;

/// 显式同步完成后的统一命令层结果，数据库恢复成功不会被后置失败推翻。
pub(crate) struct PostOperationSyncResult {
    pub profile_sync: Option<CodexExplicitSyncResult>,
    pub warning: Option<String>,
}

/// 在阻塞线程中同步依赖文件系统和数据库的派生状态。
fn run_blocking_post_import_sync(app_state: &AppState) -> Vec<String> {
    let mut failures = Vec::new();

    if let Err(error) = ProviderService::sync_current_to_live(app_state) {
        failures.push(format!("live configuration: {error}"));
    }
    if let Err(error) = PromptService::sync_all_to_live(app_state) {
        failures.push(format!("prompts: {error}"));
    }
    if let Err(error) = model_pricing::sync_local_model_pricing(&app_state.db) {
        failures.push(format!("model pricing: {error}"));
    }
    failures
}

/// 执行手动 Sync、Import 与云恢复共用的后置同步合同。
pub(crate) async fn run_post_import_sync(state: &AppState) -> PostOperationSyncResult {
    let blocking_state = state.clone();
    let mut failures = match tauri::async_runtime::spawn_blocking(move || {
        run_blocking_post_import_sync(&blocking_state)
    })
    .await
    {
        Ok(failures) => failures,
        Err(error) => vec![format!("blocking synchronization task: {error}")],
    };

    // Profile 投影最后执行，确保旧 global Live 同步不会覆盖默认 Profile 自己的主供应商。
    let profile_sync = state
        .codex_route_manager
        .sync_managed_profiles_explicit(state.db.as_ref())
        .await
        .ok();
    if profile_sync.is_none() {
        failures.push("Codex Profile projection".to_string());
    }
    if let Err(error) = settings::reload_settings() {
        failures.push(format!("settings cache: {error}"));
    }
    match state.db.get_log_config() {
        Ok(log_config) => log::set_max_level(log_config.to_level_filter()),
        Err(error) => {
            log::set_max_level(log::LevelFilter::Info);
            failures.push(format!("runtime log level: {error}"));
        }
    }
    state.usage_cache.invalidate_all();

    if !failures.is_empty() {
        log::warn!("后置同步未完全完成: {}", failures.join("; "));
    }
    let warning = (!failures.is_empty()).then(post_sync_warning);
    PostOperationSyncResult {
        profile_sync,
        warning,
    }
}

fn post_sync_warning() -> String {
    AppError::localized(
        "sync.post_operation_sync_failed",
        "后置同步未完全完成，请修复配置或文件权限后重试",
        "Post-operation synchronization did not fully complete; fix the configuration or file permissions and retry.",
    )
    .to_string()
}

pub(crate) fn attach_warning(mut value: Value, warning: Option<String>) -> Value {
    if let Some(message) = warning {
        if let Some(obj) = value.as_object_mut() {
            obj.insert("warning".to_string(), Value::String(message));
        }
    }
    value
}

/// 将结构化 Profile 同步结果附加到既有成功 payload，不改变导入或恢复成功语义。
pub(crate) fn attach_profile_sync_result(
    mut value: Value,
    profile_sync: CodexExplicitSyncResult,
) -> Value {
    let warnings = serde_json::to_value(&profile_sync.warnings).unwrap_or_else(|_| json!([]));
    let profile_sync_value = serde_json::to_value(profile_sync).unwrap_or_else(|_| {
        json!({
            "status": "completed_with_warnings",
            "synchronizedCount": 0,
            "skippedCount": 0,
            "warnings": [],
            "outcomes": []
        })
    });
    if let Some(object) = value.as_object_mut() {
        object.insert("warnings".to_string(), warnings);
        object.insert("profileSync".to_string(), profile_sync_value);
    }
    value
}

/// 将统一后置同步结果附加到既有操作 payload。
pub(crate) fn attach_post_operation_sync_result(
    mut value: Value,
    sync_result: PostOperationSyncResult,
) -> Value {
    if let Some(profile_sync) = sync_result.profile_sync {
        value = attach_profile_sync_result(value, profile_sync);
    }
    attach_warning(value, sync_result.warning)
}

pub(crate) fn success_payload_with_warning(backup_id: String, warning: Option<String>) -> Value {
    attach_warning(
        json!({
            "success": true,
            "message": "SQL imported successfully",
            "backupId": backup_id
        }),
        warning,
    )
}

#[cfg(test)]
mod tests {
    use super::{attach_profile_sync_result, attach_warning};
    use crate::codex_profile::{
        CodexExplicitProfileSyncOutcome, CodexExplicitSyncResult, CodexProfileSyncWarning,
    };
    use serde_json::json;

    #[test]
    fn attach_warning_adds_warning_without_dropping_existing_fields() {
        let payload = json!({ "status": "downloaded" });
        let updated = attach_warning(payload, Some("post sync warning".to_string()));
        assert_eq!(
            updated.get("status").and_then(|v| v.as_str()),
            Some("downloaded")
        );
        assert_eq!(
            updated.get("warning").and_then(|v| v.as_str()),
            Some("post sync warning")
        );
    }

    #[test]
    fn attach_profile_sync_result_preserves_restore_success_and_exposes_structured_warnings() {
        let profile_warning = CodexProfileSyncWarning {
            profile_id: "profile-failed".to_string(),
            profile_name: "Failed Profile".to_string(),
            home_path: "/tmp/profile-failed".to_string(),
            reason_code: "apply_failed".to_string(),
        };
        let sync_result = CodexExplicitSyncResult {
            status: "completed_with_warnings".to_string(),
            synchronized_count: 1,
            skipped_count: 0,
            warnings: vec![profile_warning],
            outcomes: vec![CodexExplicitProfileSyncOutcome {
                profile_id: "profile-failed".to_string(),
                profile_name: "Failed Profile".to_string(),
                home_path: "/tmp/profile-failed".to_string(),
                status: "failed".to_string(),
                reason_code: Some("apply_failed".to_string()),
            }],
        };

        let payload = attach_profile_sync_result(
            json!({ "success": true, "backupId": "safety-backup" }),
            sync_result,
        );

        assert_eq!(payload["success"], true);
        assert_eq!(payload["backupId"], "safety-backup");
        assert_eq!(payload["profileSync"]["status"], "completed_with_warnings");
        assert_eq!(payload["warnings"][0]["profileId"], "profile-failed");
        assert_eq!(payload["warnings"][0]["reasonCode"], "apply_failed");
        assert!(payload["warnings"][0].get("reason").is_none());
        assert_eq!(
            payload["profileSync"]["outcomes"][0]["reasonCode"],
            "apply_failed"
        );
    }
}
