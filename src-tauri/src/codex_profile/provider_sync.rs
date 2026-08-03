use crate::codex_profile::{
    CodexDirectProviderConfigPlan, CodexHomeConfigService, CodexModelCatalogProjectionPlan,
};
use crate::error::AppError;
use std::path::{Path, PathBuf};

/// 单个 Profile Home 在共享供应商保存中的目标投影。
#[derive(Debug)]
pub(crate) enum CodexProviderHomeProjectionPlan {
    Direct(CodexDirectProviderConfigPlan),
    Catalog(CodexModelCatalogProjectionPlan),
}

impl CodexProviderHomeProjectionPlan {
    /// 返回计划所属的 Home。
    fn home_path(&self) -> &Path {
        match self {
            Self::Direct(plan) => plan.home_path(),
            Self::Catalog(plan) => plan.home_path(),
        }
    }

    /// 应用计划。
    fn apply(&self, home_config: &CodexHomeConfigService) -> Result<(), AppError> {
        match self {
            Self::Direct(plan) => home_config.apply_direct_provider_plan(plan),
            Self::Catalog(plan) => home_config.apply_model_catalog_projection_plan(plan),
        }
    }

    /// 恢复计划应用前的 Home。
    fn restore(&self, home_config: &CodexHomeConfigService) -> Result<(), AppError> {
        match self {
            Self::Direct(plan) => home_config.restore_direct_provider_plan(plan),
            Self::Catalog(plan) => home_config.restore_model_catalog_projection_plan(plan),
        }
    }
}

/// 共享供应商保存中的单个 Profile Home 投影项。
#[derive(Debug)]
pub(crate) struct CodexProviderHomeProjectionEntry {
    pub profile_id: String,
    pub profile_name: String,
    pub home_path: PathBuf,
    pub plan: CodexProviderHomeProjectionPlan,
}

/// 已成功应用的 Profile Home 投影批次。
#[derive(Debug)]
pub(crate) struct AppliedCodexProviderHomeProjectionBatch {
    entries: Vec<CodexProviderHomeProjectionEntry>,
}

/// 按输入顺序应用 Profile Home 投影，并在中途失败时反向补偿。
pub(crate) fn apply_provider_home_projection_batch(
    home_config: &CodexHomeConfigService,
    entries: Vec<CodexProviderHomeProjectionEntry>,
) -> Result<AppliedCodexProviderHomeProjectionBatch, AppError> {
    for entry in &entries {
        if entry.plan.home_path() != entry.home_path {
            return Err(AppError::InvalidInput(format!(
                "Codex Profile {} ({}) 的供应商计划与 Home 不一致: {}",
                entry.profile_id,
                entry.profile_name,
                entry.home_path.display()
            )));
        }
    }

    let mut applied = Vec::with_capacity(entries.len());
    for entry in entries {
        if let Err(primary) = entry.plan.apply(home_config) {
            let compensation = restore_provider_home_projections(home_config, &applied).err();
            return Err(provider_home_projection_apply_error(
                &entry,
                primary,
                compensation,
            ));
        }
        applied.push(entry);
    }
    Ok(AppliedCodexProviderHomeProjectionBatch { entries: applied })
}

/// 在后续供应商提交失败时反向恢复整个成功批次。
pub(crate) fn restore_provider_home_projection_batch(
    home_config: &CodexHomeConfigService,
    batch: &AppliedCodexProviderHomeProjectionBatch,
) -> Result<(), AppError> {
    restore_provider_home_projections(home_config, &batch.entries)
}

/// 反向恢复已经应用的 Profile Home 投影。
fn restore_provider_home_projections(
    home_config: &CodexHomeConfigService,
    entries: &[CodexProviderHomeProjectionEntry],
) -> Result<(), AppError> {
    let failures = entries
        .iter()
        .rev()
        .filter_map(|entry| {
            entry.plan.restore(home_config).err().map(|error| {
                format!(
                    "Profile {} ({})，Home {}: {}",
                    entry.profile_id,
                    entry.profile_name,
                    entry.home_path.display(),
                    error
                )
            })
        })
        .collect::<Vec<_>>();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(AppError::Message(failures.join("；")))
    }
}

/// 汇总 Profile Home 主失败与补偿结果，不包含供应商配置正文。
fn provider_home_projection_apply_error(
    failed: &CodexProviderHomeProjectionEntry,
    primary: AppError,
    compensation: Option<AppError>,
) -> AppError {
    match compensation {
        Some(compensation) => AppError::Message(format!(
            "Codex Profile 供应商同步失败并且补偿未收敛: Profile {} ({})，Home {}；主失败: {}；补偿失败: {}",
            failed.profile_id,
            failed.profile_name,
            failed.home_path.display(),
            primary,
            compensation
        )),
        None => AppError::Message(format!(
            "Codex Profile 供应商同步失败: Profile {} ({})，Home {}: {}",
            failed.profile_id,
            failed.profile_name,
            failed.home_path.display(),
            primary
        )),
    }
}
