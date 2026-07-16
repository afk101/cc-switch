use crate::codex_profile::{CodexHomeConfigService, CodexModelCatalogProjectionPlan};
use crate::error::AppError;
use std::path::PathBuf;

/// 模型目录批处理中的单个 Profile 投影项。
#[derive(Debug)]
pub(crate) struct CodexCatalogProjectionEntry {
    pub profile_id: String,
    pub profile_name: String,
    pub home_path: PathBuf,
    pub plan: CodexModelCatalogProjectionPlan,
}

/// 已成功应用的模型目录批处理回执，用于后续事务补偿。
#[derive(Debug)]
pub(crate) struct AppliedCodexCatalogProjectionBatch {
    entries: Vec<CodexCatalogProjectionEntry>,
}

/// 按输入顺序应用模型目录投影，并在中途失败时反向补偿。
pub(crate) fn apply_catalog_projection_batch(
    home_config: &CodexHomeConfigService,
    entries: Vec<CodexCatalogProjectionEntry>,
) -> Result<AppliedCodexCatalogProjectionBatch, AppError> {
    for entry in &entries {
        if entry.plan.home_path() != entry.home_path {
            return Err(AppError::InvalidInput(format!(
                "Codex Profile {} ({}) 的模型目录计划与 Home 不一致: {}",
                entry.profile_id,
                entry.profile_name,
                entry.home_path.display()
            )));
        }
    }

    let mut applied = Vec::with_capacity(entries.len());
    for entry in entries {
        if let Err(primary) = home_config.apply_model_catalog_projection_plan(&entry.plan) {
            let applied_refs = applied.iter().collect::<Vec<_>>();
            let compensation =
                restore_applied_catalog_projections(home_config, &applied_refs).err();
            return Err(catalog_projection_apply_error(
                &entry,
                primary,
                compensation,
            ));
        }
        applied.push(entry);
    }
    Ok(AppliedCodexCatalogProjectionBatch { entries: applied })
}

/// 反向恢复所有已应用投影，并尽力收集每个未收敛项。
fn restore_applied_catalog_projections(
    home_config: &CodexHomeConfigService,
    applied: &[&CodexCatalogProjectionEntry],
) -> Result<(), AppError> {
    let failures = applied
        .iter()
        .rev()
        .filter_map(|entry| {
            home_config
                .restore_model_catalog_projection_plan(&entry.plan)
                .err()
                .map(|error| {
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

/// 在后续供应商提交失败时反向恢复整个成功批次。
pub(crate) fn restore_catalog_projection_batch(
    home_config: &CodexHomeConfigService,
    batch: &AppliedCodexCatalogProjectionBatch,
) -> Result<(), AppError> {
    let applied = batch.entries.iter().collect::<Vec<_>>();
    restore_applied_catalog_projections(home_config, &applied)
}

/// 构造批处理主失败与补偿结果，不包含供应商配置正文。
fn catalog_projection_apply_error(
    failed: &CodexCatalogProjectionEntry,
    primary: AppError,
    compensation: Option<AppError>,
) -> AppError {
    match compensation {
        Some(compensation) => AppError::Message(format!(
            "{}: Profile {} ({})，Home {}；主失败: {}；补偿失败: {}",
            crate::codex_profile::CODEX_CATALOG_SYNC_COMPENSATION_ERROR,
            failed.profile_id,
            failed.profile_name,
            failed.home_path.display(),
            primary,
            compensation
        )),
        None => AppError::Message(format!(
            "Codex Profile 模型目录同步失败: Profile {} ({})，Home {}: {}",
            failed.profile_id,
            failed.profile_name,
            failed.home_path.display(),
            primary
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_config::CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME;
    use crate::codex_profile::CodexHomeFileOps;
    use crate::provider::{Provider, ProviderMeta};
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    /// 构造包含共享模型目录的测试供应商。
    fn provider_with_catalog(id: &str, model: &str) -> Provider {
        let mut provider = Provider::with_id(
            id.to_string(),
            id.to_string(),
            serde_json::json!({
                "auth": {"OPENAI_API_KEY": "test-token"},
                "config": "model_provider = \"custom\"\n[model_providers.custom]\nbase_url = \"https://example.com/v1\"\nwire_api = \"responses\"\n",
                "modelCatalog": {"models": [{"model": model}]}
            }),
            None,
        );
        provider.meta = Some(ProviderMeta {
            api_format: Some("openai_responses".to_string()),
            ..Default::default()
        });
        provider
    }

    /// 在指定模型目录写入时注入失败。
    struct FailOnCatalogPathOps {
        fail_path: PathBuf,
    }

    impl CodexHomeFileOps for FailOnCatalogPathOps {
        fn read(&self, path: &Path) -> Result<Option<Vec<u8>>, AppError> {
            if path.exists() {
                fs::read(path)
                    .map(Some)
                    .map_err(|error| AppError::io(path, error))
            } else {
                Ok(None)
            }
        }

        fn write_atomic(&self, path: &Path, content: &[u8]) -> Result<(), AppError> {
            if path == self.fail_path {
                return Err(AppError::Config(format!(
                    "测试注入目录写入失败: {}",
                    path.display()
                )));
            }
            crate::config::atomic_write(path, content)
        }

        fn remove_file(&self, path: &Path) -> Result<(), AppError> {
            if path.exists() {
                fs::remove_file(path).map_err(|error| AppError::io(path, error))?;
            }
            Ok(())
        }
    }

    /// 第一个 Home 应用后制造外部修改，同时让第二个 Home 写入失败。
    struct InterfereBeforeCompensationOps {
        fail_path: PathBuf,
        interfere_path: PathBuf,
    }

    impl CodexHomeFileOps for InterfereBeforeCompensationOps {
        fn read(&self, path: &Path) -> Result<Option<Vec<u8>>, AppError> {
            if path.exists() {
                fs::read(path)
                    .map(Some)
                    .map_err(|error| AppError::io(path, error))
            } else {
                Ok(None)
            }
        }

        fn write_atomic(&self, path: &Path, content: &[u8]) -> Result<(), AppError> {
            if path == self.fail_path {
                return Err(AppError::Config(format!(
                    "测试注入目录写入失败: {}",
                    path.display()
                )));
            }
            crate::config::atomic_write(path, content)?;
            if path == self.interfere_path {
                fs::write(path, b"external-a").map_err(|error| AppError::io(path, error))?;
            }
            Ok(())
        }

        fn remove_file(&self, path: &Path) -> Result<(), AppError> {
            if path.exists() {
                fs::remove_file(path).map_err(|error| AppError::io(path, error))?;
            }
            Ok(())
        }
    }

    /// 构造两个 Home 的有序投影项。
    fn projection_entries(
        home_config: &CodexHomeConfigService,
        home_a: &Path,
        home_b: &Path,
    ) -> Vec<CodexCatalogProjectionEntry> {
        let provider = provider_with_catalog("shared", "new-model");
        vec![
            CodexCatalogProjectionEntry {
                profile_id: "profile-a".to_string(),
                profile_name: "Profile A".to_string(),
                home_path: home_a.to_path_buf(),
                plan: home_config
                    .build_model_catalog_projection_plan(home_a, &provider)
                    .expect("构造 A 投影"),
            },
            CodexCatalogProjectionEntry {
                profile_id: "profile-b".to_string(),
                profile_name: "Profile B".to_string(),
                home_path: home_b.to_path_buf(),
                plan: home_config
                    .build_model_catalog_projection_plan(home_b, &provider)
                    .expect("构造 B 投影"),
            },
        ]
    }

    #[test]
    fn second_home_failure_restores_first_home() {
        let home_a = tempfile::tempdir().expect("创建 A Home");
        let home_b = tempfile::tempdir().expect("创建 B Home");
        let path_a = home_a.path().join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
        let path_b = home_b.path().join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
        fs::write(&path_a, b"old-a").expect("写入 A 旧目录");
        fs::write(&path_b, b"old-b").expect("写入 B 旧目录");
        let home_config = CodexHomeConfigService::new(Arc::new(FailOnCatalogPathOps {
            fail_path: path_b.clone(),
        }));
        let entries = projection_entries(&home_config, home_a.path(), home_b.path());

        let error = apply_catalog_projection_batch(&home_config, entries)
            .expect_err("第二个 Home 写入失败必须返回错误");

        assert!(error.to_string().contains("profile-b"));
        assert_eq!(fs::read(&path_a).expect("重读 A 目录"), b"old-a");
        assert_eq!(fs::read(&path_b).expect("重读 B 目录"), b"old-b");
    }

    #[test]
    fn compensation_conflict_reports_primary_and_restore_failures() {
        let home_a = tempfile::tempdir().expect("创建 A Home");
        let home_b = tempfile::tempdir().expect("创建 B Home");
        let path_a = home_a.path().join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
        let path_b = home_b.path().join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
        fs::write(&path_a, b"old-a").expect("写入 A 旧目录");
        fs::write(&path_b, b"old-b").expect("写入 B 旧目录");
        let home_config = CodexHomeConfigService::new(Arc::new(InterfereBeforeCompensationOps {
            fail_path: path_b.clone(),
            interfere_path: path_a.clone(),
        }));
        let entries = projection_entries(&home_config, home_a.path(), home_b.path());

        let error = apply_catalog_projection_batch(&home_config, entries)
            .expect_err("补偿冲突必须返回错误");
        let message = error.to_string();

        assert!(message.contains("profile-b"));
        assert!(message.contains("Codex Live 配置已被外部修改"));
        assert_eq!(fs::read(&path_a).expect("重读 A 外部目录"), b"external-a");
        assert_eq!(fs::read(&path_b).expect("重读 B 目录"), b"old-b");
    }

    #[test]
    fn successful_batch_receipt_restores_all_homes() {
        let home_a = tempfile::tempdir().expect("创建 A Home");
        let home_b = tempfile::tempdir().expect("创建 B Home");
        let path_a = home_a.path().join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
        let path_b = home_b.path().join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
        fs::write(&path_a, b"old-a").expect("写入 A 旧目录");
        fs::write(&path_b, b"old-b").expect("写入 B 旧目录");
        let home_config = CodexHomeConfigService::system();
        let entries = projection_entries(&home_config, home_a.path(), home_b.path());
        let receipt =
            apply_catalog_projection_batch(&home_config, entries).expect("批量模型目录应用应成功");

        assert_ne!(fs::read(&path_a).expect("读取 A 新目录"), b"old-a");
        assert_ne!(fs::read(&path_b).expect("读取 B 新目录"), b"old-b");

        restore_catalog_projection_batch(&home_config, &receipt).expect("成功批次应可反向恢复");

        assert_eq!(fs::read(&path_a).expect("读取 A 恢复目录"), b"old-a");
        assert_eq!(fs::read(&path_b).expect("读取 B 恢复目录"), b"old-b");
    }

    #[test]
    fn invalid_second_entry_is_rejected_before_first_home_changes() {
        let home_a = tempfile::tempdir().expect("创建 A Home");
        let home_b = tempfile::tempdir().expect("创建 B Home");
        let unrelated_home = tempfile::tempdir().expect("创建无关 Home");
        let path_a = home_a.path().join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
        let path_b = home_b.path().join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
        fs::write(&path_a, b"old-a").expect("写入 A 旧目录");
        fs::write(&path_b, b"old-b").expect("写入 B 旧目录");
        let home_config = CodexHomeConfigService::system();
        let mut entries = projection_entries(&home_config, home_a.path(), home_b.path());
        entries[1].home_path = unrelated_home.path().to_path_buf();

        let error = apply_catalog_projection_batch(&home_config, entries)
            .expect_err("不一致的 Home 身份必须在写入前被拒绝");

        assert!(error.to_string().contains("计划与 Home 不一致"));
        assert_eq!(fs::read(&path_a).expect("读取 A 未变目录"), b"old-a");
        assert_eq!(fs::read(&path_b).expect("读取 B 未变目录"), b"old-b");
    }
}
