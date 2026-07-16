# Codex Multi-Home Model Catalog Synchronization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-subagent-driven-development OR superpowers-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep every Codex Profile Home's generated model catalog synchronized with its referenced shared provider without symlinks or cross-Home leakage.

**Architecture:** Keep `Provider.settings_config.modelCatalog` as the source of truth, build fingerprinted per-Home projection plans in `CodexHomeConfigService`, execute multi-Home batches with reverse-order compensation, and let `CodexRouteManager` serialize provider edits and Profile lifecycle changes with a single catalog lock plus sorted Profile locks. The default Home follows the same path as every custom Profile.

**Tech Stack:** Rust, Tauri 2 commands, Tokio mutexes, rusqlite-backed `Database`, `toml_edit`, serde_json, Cargo unit tests.

---

### Task 1: Split pure catalog generation from target-Home pointer projection

**Files:**
- Modify: `src-tauri/src/codex_config.rs:947-1058`
- Test: `src-tauri/src/codex_config.rs:2998-3535`

- [ ] **Step 1: Write failing projection-core tests**

Add tests that use a provider config as the catalog-generation source and a different target Home config as the pointer target:

```rust
#[test]
fn catalog_projection_uses_provider_config_but_only_mutates_home_pointer() {
    let settings = serde_json::json!({
        "config": "model_provider = \"custom\"\n[model_providers.custom]\nbase_url = \"https://upstream.example/v1\"\nwire_api = \"responses\"\n",
        "modelCatalog": {"models": [{"model": "shared-model"}]}
    });
    let home = "model = \"local-model\"\ncustom_user_field = \"keep\"\n";

    let prepared = prepare_codex_model_catalog_projection(
        &settings,
        settings["config"].as_str().unwrap(),
        home,
        CodexCatalogToolProfile::ProxyChat,
    )
    .unwrap();

    assert!(prepared.model_catalog.is_some());
    assert!(prepared.config_text.contains("custom_user_field = \"keep\""));
    assert!(prepared
        .config_text
        .contains("model_catalog_json = \"cc-switch-model-catalog.json\""));
    assert!(!prepared.config_text.contains("upstream.example"));
}

#[test]
fn empty_catalog_removes_only_cc_switch_pointer() {
    let prepared = prepare_codex_model_catalog_projection(
        &serde_json::json!({"config": "", "modelCatalog": {"models": []}}),
        "",
        "model_catalog_json = \"cc-switch-model-catalog.json\"\nkeep = true\n",
        CodexCatalogToolProfile::ProxyChat,
    )
    .unwrap();
    assert!(!prepared.config_text.contains("model_catalog_json"));
    assert!(prepared.config_text.contains("keep = true"));
}

#[test]
fn empty_catalog_preserves_user_managed_pointer() {
    let prepared = prepare_codex_model_catalog_projection(
        &serde_json::json!({"config": "", "modelCatalog": {"models": []}}),
        "",
        "model_catalog_json = \"my-catalog.json\"\n",
        CodexCatalogToolProfile::ProxyChat,
    )
    .unwrap();
    assert!(prepared.config_text.contains("my-catalog.json"));
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml catalog_projection_ --lib -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml empty_catalog_ --lib -- --nocapture
```

Expected: FAIL because `prepare_codex_model_catalog_projection` does not exist.

- [ ] **Step 3: Implement the pure projection function**

Add a focused function that separates the provider config used to synthesize catalog entries from the Home config whose pointer is changed:

```rust
pub fn prepare_codex_model_catalog_projection(
    settings: &Value,
    catalog_source_config: &str,
    home_config: &str,
    profile: CodexCatalogToolProfile,
) -> Result<PreparedCodexConfigWithModelCatalog, AppError> {
    let model_catalog = codex_model_catalog_from_settings(
        settings,
        catalog_source_config,
        profile,
    )?;
    let config_text = set_codex_model_catalog_json_field(
        home_config,
        model_catalog
            .as_ref()
            .map(|_| Path::new(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME)),
    )?;
    Ok(PreparedCodexConfigWithModelCatalog {
        config_text,
        model_catalog,
    })
}
```

Keep `prepare_codex_config_with_model_catalog` responsible for its existing direct/live behavior, including `web_search`. Reuse the new narrow helper only where a Profile Home must receive a catalog pointer without copying provider endpoint fields into its routed config.

- [ ] **Step 4: Run focused and existing catalog tests**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml catalog_projection_ --lib -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml codex_model_catalog --lib -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit the pure projection core**

```bash
git add src-tauri/src/codex_config.rs
git commit -m "refactor(codex): split model catalog projection core"
```

### Task 2: Add a fingerprinted single-Home catalog projection plan

**Files:**
- Modify: `src-tauri/src/codex_profile/home_config.rs:25-340`
- Modify: `src-tauri/src/codex_profile/mod.rs:20-40`
- Test: `src-tauri/src/codex_profile/home_config.rs:1465-1590`

- [ ] **Step 1: Write failing single-Home behavior tests**

Add these concrete helpers inside the existing `codex_home_config` test module:

```rust
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
    provider.meta = Some(crate::provider::ProviderMeta {
        api_format: Some("openai_responses".to_string()),
        ..Default::default()
    });
    provider
}

fn provider_without_catalog(id: &str) -> Provider {
    Provider::with_id(
        id.to_string(),
        id.to_string(),
        serde_json::json!({"config": "model = \"gpt-official\"\n"}),
        None,
    )
}
```

Then add tests for these exact behaviors:

```rust
#[test]
fn existing_pointer_updates_only_catalog_file() -> Result<(), AppError> {
    let home = tempfile::tempdir().unwrap();
    fs::write(
        codex_config_path_for_home(home.path()),
        "model_catalog_json = \"cc-switch-model-catalog.json\"\nkeep = true\n",
    )
    .expect("写入已有目录指针");
    let service = CodexHomeConfigService::system();
    let provider = provider_with_catalog("shared", "new-model");
    let plan = service.build_model_catalog_projection_plan(home.path(), &provider)?;
    assert!(!plan.config_changed());
    service.apply_model_catalog_projection_plan(&plan)?;
    assert!(fs::read_to_string(home.path().join(
        CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME,
    ))
    .expect("读取更新后的模型目录")
    .contains("new-model"));
    Ok(())
}

#[test]
fn provider_without_catalog_keeps_old_catalog_file() -> Result<(), AppError> {
    let home = tempfile::tempdir().unwrap();
    let catalog_path = home.path().join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
    fs::write(&catalog_path, b"old catalog").expect("写入旧模型目录");
    fs::write(
        codex_config_path_for_home(home.path()),
        "model_catalog_json = \"cc-switch-model-catalog.json\"\n",
    )
    .expect("写入旧目录指针");
    let service = CodexHomeConfigService::system();
    let plan = service.build_model_catalog_projection_plan(
        home.path(),
        &provider_without_catalog("official"),
    )?;
    service.apply_model_catalog_projection_plan(&plan)?;
    assert!(catalog_path.exists());
    assert!(!fs::read_to_string(codex_config_path_for_home(home.path()))
        .expect("读取移除指针后的配置")
        .contains("model_catalog_json"));
    Ok(())
}
```

Also add a test where `FailingFileOps` changes the file between prepare/apply and assert `AppError::CodexLiveConfigConflict`.

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml model_catalog_projection_plan --lib -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml existing_pointer_updates_only_catalog_file --lib -- --nocapture
```

Expected: FAIL because the plan API does not exist.

- [ ] **Step 3: Implement the plan type and narrow methods**

Add a public crate-local plan that owns only optional config mutation and optional catalog content mutation:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexModelCatalogProjectionPlan {
    home_path: PathBuf,
    config: Option<CodexRouteConfigPlan>,
    model_catalog: Option<CodexAuxiliaryFilePlan>,
}

impl CodexModelCatalogProjectionPlan {
    pub fn config_changed(&self) -> bool {
        self.config.is_some()
    }
}
```

Add methods with one responsibility each:

```rust
pub fn build_model_catalog_projection_plan(
    &self,
    home: &Path,
    provider: &Provider,
) -> Result<CodexModelCatalogProjectionPlan, AppError>;

pub fn apply_model_catalog_projection_plan(
    &self,
    plan: &CodexModelCatalogProjectionPlan,
) -> Result<(), AppError>;

pub fn restore_model_catalog_projection_plan(
    &self,
    plan: &CodexModelCatalogProjectionPlan,
) -> Result<(), AppError>;
```

Implementation rules:

- build the catalog from provider settings and provider config;
- project only the pointer into the current Home config;
- omit `config` when target bytes equal current bytes;
- write catalog before config;
- restore config before catalog so Codex never points at a restored-away target;
- leave the catalog plan absent when the provider has no catalog, preserving the old JSON file.

- [ ] **Step 4: Reuse the new plan in direct-provider construction**

Refactor duplicated auxiliary snapshot/apply/restore code in `CodexDirectProviderConfigPlan` to use the same helpers without changing direct config, auth, or `web_search` semantics. Preserve all explanatory comments.

- [ ] **Step 5: Run Home config tests**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml codex_profile::home_config --lib -- --nocapture
```

Expected: PASS, including the old auth-preservation and route-fingerprint tests.

- [ ] **Step 6: Commit the single-Home plan**

```bash
git add src-tauri/src/codex_profile/home_config.rs src-tauri/src/codex_profile/mod.rs
git commit -m "feat(codex): add Home model catalog projection plan"
```

### Task 3: Implement deterministic multi-Home batch application

**Files:**
- Create: `src-tauri/src/codex_profile/catalog_sync.rs`
- Modify: `src-tauri/src/codex_profile/constants.rs`
- Modify: `src-tauri/src/codex_profile/mod.rs`
- Test: `src-tauri/src/codex_profile/catalog_sync.rs`

- [ ] **Step 1: Write failing batch compensation tests**

Create a concrete injected file implementation that fails writes for the second Home's catalog path:

```rust
struct FailOnCatalogPathOps {
    fail_path: PathBuf,
}

impl CodexHomeFileOps for FailOnCatalogPathOps {
    fn read(&self, path: &Path) -> Result<Option<Vec<u8>>, AppError> {
        if path.exists() {
            fs::read(path).map(Some).map_err(|error| AppError::io(path, error))
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

#[test]
fn second_home_failure_restores_first_home() {
    let home_a = tempfile::tempdir().unwrap();
    let home_b = tempfile::tempdir().unwrap();
    let path_a = home_a.path().join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
    let path_b = home_b.path().join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
    fs::write(&path_a, b"old-a").unwrap();
    fs::write(&path_b, b"old-b").unwrap();
    let home_config = CodexHomeConfigService::new(Arc::new(FailOnCatalogPathOps {
        fail_path: path_b.clone(),
    }));
    let provider = provider_with_catalog("shared", "new-model");
    let entries = vec![
        CodexCatalogProjectionEntry {
            profile_id: "profile-a".to_string(),
            profile_name: "Profile A".to_string(),
            home_path: home_a.path().to_path_buf(),
            plan: home_config
                .build_model_catalog_projection_plan(home_a.path(), &provider)
                .unwrap(),
        },
        CodexCatalogProjectionEntry {
            profile_id: "profile-b".to_string(),
            profile_name: "Profile B".to_string(),
            home_path: home_b.path().to_path_buf(),
            plan: home_config
                .build_model_catalog_projection_plan(home_b.path(), &provider)
                .unwrap(),
        },
    ];

    let error = apply_catalog_projection_batch(&home_config, entries).unwrap_err();

    assert!(error.to_string().contains("profile-b"));
    assert_eq!(fs::read(&path_a).unwrap(), b"old-a");
    assert_eq!(fs::read(&path_b).unwrap(), b"old-b");
}
```

Define `provider_with_catalog` in this test module with the same concrete Provider construction used in Task 2. Add a second injected implementation that changes `path_a` immediately after its successful target write; assert the resulting error string contains both `profile-b` and `Codex Live 配置已被外部修改`.

- [ ] **Step 2: Run the batch tests and verify they fail**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml catalog_sync --lib -- --nocapture
```

Expected: FAIL because the batch module does not exist.

- [ ] **Step 3: Implement the batch executor**

Add stable sanitized error prefixes to `constants.rs`:

```rust
/// 模型目录批量同步补偿未收敛时使用的安全错误前缀。
pub const CODEX_CATALOG_SYNC_COMPENSATION_ERROR: &str =
    "Codex Profile 模型目录同步失败，补偿未收敛";
/// 启动模型目录对账错误的稳定前缀，仅用于识别并清理本类错误。
pub const CODEX_CATALOG_RECONCILE_ERROR_PREFIX: &str =
    "Codex Profile 模型目录对账失败";
```

Define entries that retain sanitized identity alongside the plan:

```rust
pub(crate) struct CodexCatalogProjectionEntry {
    pub profile_id: String,
    pub profile_name: String,
    pub home_path: PathBuf,
    pub plan: CodexModelCatalogProjectionPlan,
}

pub(crate) struct AppliedCodexCatalogProjectionBatch {
    entries: Vec<CodexCatalogProjectionEntry>,
}
```

Implement three focused functions:

```rust
pub(crate) fn apply_catalog_projection_batch(
    home_config: &CodexHomeConfigService,
    entries: Vec<CodexCatalogProjectionEntry>,
) -> Result<AppliedCodexCatalogProjectionBatch, AppError>;

fn restore_applied_catalog_projections(
    home_config: &CodexHomeConfigService,
    applied: &[&CodexCatalogProjectionEntry],
) -> Result<(), AppError>;

pub(crate) fn restore_catalog_projection_batch(
    home_config: &CodexHomeConfigService,
    batch: &AppliedCodexCatalogProjectionBatch,
) -> Result<(), AppError>;
```

Apply entries in input order and return a receipt owning every successfully applied plan. On a mid-batch failure, restore the already-applied slice in reverse order. `restore_catalog_projection_batch` restores a successful receipt in reverse order when a later provider commit fails. Build errors containing only Profile ID/name, Home path, primary failure, and compensation failure.

- [ ] **Step 4: Run batch and formatting checks**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml catalog_sync --lib -- --nocapture
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

Expected: PASS.

- [ ] **Step 5: Commit the batch executor**

```bash
git add src-tauri/src/codex_profile/catalog_sync.rs src-tauri/src/codex_profile/constants.rs src-tauri/src/codex_profile/mod.rs
git commit -m "feat(codex): apply catalog projections as a batch"
```

### Task 4: Synchronize Profile enable and provider switch lifecycles

**Files:**
- Modify: `src-tauri/src/codex_profile/route_manager.rs:100-590`
- Test: `src-tauri/src/codex_profile/route_manager.rs:1900-5200`

- [ ] **Step 1: Write failing route lifecycle tests**

Add this helper to the existing route manager test module:

```rust
fn provider_with_route_catalog(id: &str, model: &str) -> Provider {
    let mut provider = Provider::with_id(
        id.to_string(),
        id.to_string(),
        json!({
            "auth": {"OPENAI_API_KEY": "test-token"},
            "config": "model_provider = \"custom\"\n[model_providers.custom]\nbase_url = \"https://example.com/v1\"\nwire_api = \"responses\"\n",
            "modelCatalog": {"models": [{"model": model}]}
        }),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        api_format: Some("openai_chat".to_string()),
        ..Default::default()
    });
    provider
}
```

Use the existing `Database::memory`, `FakeFactory`, `TrackingTokenStore`, and `prepare_enabled_profile_home` helpers to add tests proving:

```rust
#[tokio::test]
async fn enable_route_creates_missing_catalog_in_profile_home() -> Result<(), AppError> {
    let db = Arc::new(Database::memory()?);
    let home = tempfile::tempdir().unwrap();
    let provider = provider_with_route_catalog("provider-a", "model-a");
    db.save_provider(AppType::Codex.as_str(), &provider)?;
    db.insert_codex_profile(&CodexProfile {
        id: "profile-a".to_string(),
        name: "Profile A".to_string(),
        canonical_home_path: home.path().display().to_string(),
        listen_port: 16_001,
        created_at: 1,
        updated_at: 1,
    })?;
    db.save_codex_profile_route(&CodexProfileRoute {
        profile_id: "profile-a".to_string(),
        current_provider_id: None,
        enabled: false,
        live_backup_json: None,
        last_error: None,
        recovery_json: None,
        updated_at: 1,
    })?;
    let manager = CodexRouteManager::new(
        db,
        Arc::new(CodexHomeConfigService::system()),
        Arc::new(TrackingTokenStore {
            ensured: AtomicUsize::new(0),
            deleted: AtomicUsize::new(0),
        }),
        Arc::new(FakeFactory),
    );

    manager.enable("profile-a", "provider-a", vec![]).await?;
    let catalog = fs::read_to_string(
        home.path().join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME),
    )
    .expect("读取 Profile 模型目录");
    let config = fs::read_to_string(codex_config_path_for_home(home.path()))
        .expect("读取 Profile 配置");
    assert!(catalog.contains("model-a"));
    assert!(config.contains("model_catalog_json = \"cc-switch-model-catalog.json\""));
    Ok(())
}
```

For the two-enabled-Profile switch test, create two temp Homes with `prepare_enabled_profile_home`, save `provider-a` and `provider-b` from `provider_with_route_catalog`, switch only `profile-a`, and assert A contains `model-b` while B retains `model-a`. Add persistence-failure and runtime-swap-failure assertions that the catalog returns to its original bytes.

- [ ] **Step 2: Run tests and verify the missing behavior**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml enable_route_creates_missing_catalog --lib -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml enabled_switch_replaces_only_selected_profile_catalog --lib -- --nocapture
```

Expected: FAIL because enabled routes do not project catalogs.

- [ ] **Step 3: Add the catalog synchronization lock**

Add one outer lock to `CodexRouteManager`:

```rust
catalog_sync_lock: AsyncMutex<()>,
```

Initialize it in `new()`. Acquire it before a Profile lock in `enable_internal` and `switch_provider_internal`. Do not acquire it from helpers that are called after the outer lock is already held.

- [ ] **Step 4: Integrate projection plans into enable and switch**

For enable:

```rust
let catalog_plan = self
    .home_config
    .build_model_catalog_projection_plan(home, &selected_provider)?;
self.home_config
    .apply_model_catalog_projection_plan(&catalog_plan)?;
```

If runtime start, health check, Home route write, route save, or failover save fails, include `restore_model_catalog_projection_plan` in the existing compensation result.

For enabled switch, apply the target catalog before `swap_provider_snapshot`; restore it from every existing switch compensation exit. For disabled switch, retain the existing direct plan but ensure it uses the shared catalog plan helpers.

- [ ] **Step 5: Run route manager regressions**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml codex_profile::route_manager --lib -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit route lifecycle integration**

```bash
git add src-tauri/src/codex_profile/route_manager.rs
git commit -m "fix(codex): project catalogs across Profile route changes"
```

### Task 5: Fan out shared provider edits transactionally

**Files:**
- Modify: `src-tauri/src/codex_profile/route_manager.rs`
- Modify: `src-tauri/src/commands/provider.rs:45-80`
- Modify: `src-tauri/src/services/provider/mod.rs:2138-2370`
- Test: `src-tauri/src/codex_profile/route_manager.rs`
- Test: `src-tauri/src/commands/provider.rs`

- [ ] **Step 1: Write failing reference-filter and rollback tests**

Add a manager test with these routes:

```text
profile-a: current provider-shared
profile-b: current provider-other, failover provider-shared
profile-c: current provider-shared
```

Call the provider-update projection method with a commit closure that succeeds. Assert only A and C update. Then use a closure that saves the new provider and returns an injected error; assert A/C and the provider DB row all return to old values.

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml shared_provider_catalog_update --lib -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml provider_catalog_commit_failure --lib -- --nocapture
```

Expected: FAIL because no provider-update projection transaction exists.

- [ ] **Step 3: Add a no-write Codex provider update preflight**

Extract the normalization already performed at the start of `ProviderService::update` into a reusable method:

```rust
pub(crate) fn prepare_codex_provider_update(
    state: &AppState,
    original_id: Option<&str>,
    mut provider: Provider,
) -> Result<Provider, AppError> {
    let original_id = original_id.unwrap_or(provider.id.as_str());
    if original_id != provider.id {
        return Err(AppError::Message(
            "Only additive-mode providers support changing provider key".to_string(),
        ));
    }
    Self::validate_provider_settings(&AppType::Codex, &provider)?;
    normalize_provider_common_config_for_storage(
        state.db.as_ref(),
        &AppType::Codex,
        &mut provider,
    )?;
    Self::normalize_usage_script_credential_overrides(&AppType::Codex, &mut provider);
    Ok(provider)
}
```

Call this helper from the normal Codex branch inside `ProviderService::update` as well, so the preflight and commit cannot drift. The method performs no DB or filesystem writes.

- [ ] **Step 4: Implement main-reference filtering and sorted locking**

Add a private query over the existing persistence contract:

```rust
fn profiles_using_primary_provider(
    &self,
    provider_id: &str,
) -> Result<Vec<CodexProfile>, AppError> {
    let mut profiles = Vec::new();
    for profile in self.persistence.list_profiles()? {
        let route = self.persistence.get_route(&profile.id)?;
        if route
            .as_ref()
            .and_then(|route| route.current_provider_id.as_deref())
            == Some(provider_id)
        {
            profiles.push(profile);
        }
    }
    profiles.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(profiles)
}
```

Do not use `list_codex_provider_profile_refs`, because that query intentionally includes failovers.

Acquire the catalog lock, create all Profile lock `Arc`s in sorted order, and then await their guards in that same order before preparing any file plan.

- [ ] **Step 5: Implement the update transaction wrapper**

Add a public async manager method that accepts the incoming provider and a synchronous commit closure:

```rust
pub async fn with_provider_catalog_update<T, F>(
    &self,
    provider: &Provider,
    commit: F,
) -> Result<T, AppError>
where
    F: FnOnce() -> Result<T, AppError>,
{
    let _catalog_guard = self.catalog_sync_lock.lock().await;
    let profiles = self.profiles_using_primary_provider(&provider.id)?;
    let locks = profiles
        .iter()
        .map(|profile| self.profile_lock(&profile.id))
        .collect::<Result<Vec<_>, _>>()?;
    let mut guards = Vec::with_capacity(locks.len());
    for lock in &locks {
        guards.push(lock.lock().await);
    }

    let original = self
        .persistence
        .get_provider(&provider.id)?
        .ok_or_else(|| AppError::InvalidInput(
            format!("Codex 供应商不存在: {}", provider.id)
        ))?;
    let entries = profiles
        .iter()
        .map(|profile| {
            let home_path = PathBuf::from(&profile.canonical_home_path);
            Ok(CodexCatalogProjectionEntry {
                profile_id: profile.id.clone(),
                profile_name: profile.name.clone(),
                plan: self
                    .home_config
                    .build_model_catalog_projection_plan(&home_path, provider)?,
                home_path,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    let applied = apply_catalog_projection_batch(self.home_config.as_ref(), entries)?;

    match commit() {
        Ok(value) => Ok(value),
        Err(primary) => {
            let provider_restore = self
                .persistence
                .save_provider_snapshot(&original)
                .err();
            let home_restore = restore_catalog_projection_batch(
                self.home_config.as_ref(),
                &applied,
            )
            .err();
            Err(catalog_update_compensation_error(
                primary,
                provider_restore,
                home_restore,
            ))
        }
    }
}

fn catalog_update_compensation_error(
    primary: AppError,
    provider_restore: Option<AppError>,
    home_restore: Option<AppError>,
) -> AppError {
    match (provider_restore, home_restore) {
        (None, None) => primary,
        (provider_restore, home_restore) => AppError::Message(format!(
            "{CODEX_CATALOG_SYNC_COMPENSATION_ERROR}: {primary}; \
             供应商恢复: {}; Home 恢复: {}",
            provider_restore
                .map(|error| error.to_string())
                .unwrap_or_else(|| "成功".to_string()),
            home_restore
                .map(|error| error.to_string())
                .unwrap_or_else(|| "成功".to_string()),
        )),
    }
}
```

Capture the original provider before applying files. If `commit()` fails, restore the provider row through the persistence adapter and reverse-restore the Home batch. Extend `CodexProfileRoutePersistence` with a narrowly named `save_provider_snapshot(&Provider)` method backed by `Database::save_provider(AppType::Codex.as_str(), provider)`.

Return a combined error when DB or Home compensation fails. Do not include provider settings or file contents.

- [ ] **Step 6: Make `update_provider` async and route Codex edits through the wrapper**

Change only the command signature and Codex branch:

```rust
#[tauri::command]
pub async fn update_provider(
    state: State<'_, AppState>,
    app: String,
    provider: Provider,
    #[allow(non_snake_case)] originalId: Option<String>,
) -> Result<bool, String> {
    let app_type = AppType::from_str(&app).map_err(|error| error.to_string())?;
    if app_type != AppType::Codex {
        return ProviderService::update(
            state.inner(),
            app_type,
            originalId.as_deref(),
            provider,
        )
        .map_err(|error| error.to_string());
    }

    let prepared_provider = ProviderService::prepare_codex_provider_update(
        state.inner(),
        originalId.as_deref(),
        provider,
    )
    .map_err(|error| error.to_string())?;
    let projection_provider = prepared_provider.clone();
    state
        .codex_route_manager
        .with_provider_catalog_update(&projection_provider, || {
            ProviderService::update(
                state.inner(),
                AppType::Codex,
                originalId.as_deref(),
                prepared_provider,
            )
        })
        .await
        .map_err(|error| error.to_string())
}
```

Keep non-Codex behavior byte-for-byte equivalent. The same prepared Provider instance drives both Home projections and the final provider commit.

- [ ] **Step 7: Run provider and route manager tests**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml commands::provider --lib -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml shared_provider_catalog_update --lib -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml provider_catalog_commit_failure --lib -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit shared-provider fan-out**

```bash
git add src-tauri/src/codex_profile/route_manager.rs src-tauri/src/commands/provider.rs src-tauri/src/services/provider/mod.rs
git commit -m "fix(codex): sync shared catalogs to referenced Homes"
```

### Task 6: Reconcile all Profile catalogs during application startup

**Files:**
- Modify: `src-tauri/src/codex_profile/route_manager.rs:740-815`
- Modify: `src-tauri/src/lib.rs:1100-1195`
- Test: `src-tauri/src/codex_profile/route_manager.rs`

- [ ] **Step 1: Write failing startup reconciliation tests**

Add one test with three Profiles:

- A references a catalog provider and its file is missing;
- B references the same provider and its file is stale;
- C references another provider and its injected write fails.

Assert A is created, B is refreshed, C receives a sanitized `last_error`, and the method returns without blocking A/B.

- [ ] **Step 2: Run the startup test and verify it fails**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml reconcile_all_profile_catalogs --lib -- --nocapture
```

Expected: FAIL because startup only restores enabled route configs/runtimes.

- [ ] **Step 3: Implement per-Profile startup reconciliation**

Add:

```rust
pub async fn reconcile_all_profile_catalogs(&self) -> Result<(), AppError> {
    let _catalog_guard = self.catalog_sync_lock.lock().await;
    let mut profiles = self.persistence.list_profiles()?;
    profiles.sort_by(|left, right| left.id.cmp(&right.id));

    for profile in profiles {
        let profile_lock = self.profile_lock(&profile.id)?;
        let _profile_guard = profile_lock.lock().await;
        let Some(mut route) = self.persistence.get_route(&profile.id)? else {
            continue;
        };
        let Some(provider_id) = route.current_provider_id.as_deref() else {
            continue;
        };

        let reconcile_result = (|| {
            let provider = self
                .persistence
                .get_provider(provider_id)?
                .ok_or_else(|| AppError::InvalidInput(
                    format!("Codex 供应商不存在: {provider_id}")
                ))?;
            let plan = self.home_config.build_model_catalog_projection_plan(
                Path::new(&profile.canonical_home_path),
                &provider,
            )?;
            self.home_config
                .apply_model_catalog_projection_plan(&plan)
        })();

        match reconcile_result {
            Ok(()) => {
                if route
                    .last_error
                    .as_deref()
                    .is_some_and(|error| {
                        error.starts_with(CODEX_CATALOG_RECONCILE_ERROR_PREFIX)
                    })
                {
                    route.last_error = None;
                    if let Err(save_error) = self.persistence.save_route(&route) {
                        log::warn!(
                            "清理 Codex Profile {} 模型目录错误失败: {}",
                            profile.id,
                            save_error
                        );
                    }
                }
            }
            Err(error) => {
                route.last_error = Some(format!(
                    "{CODEX_CATALOG_RECONCILE_ERROR_PREFIX}: {error}"
                ));
                if let Err(save_error) = self.persistence.save_route(&route) {
                    log::warn!(
                        "记录 Codex Profile {} 模型目录错误失败: {}",
                        profile.id,
                        save_error
                    );
                }
            }
        }
    }
    Ok(())
}
```

For each Profile with a `current_provider_id`, load the shared provider and build/apply a single-Home projection plan. On failure, set `last_error` to `format!("{CODEX_CATALOG_RECONCILE_ERROR_PREFIX}: {error}")`. On success, clear `last_error` only when it starts with `CODEX_CATALOG_RECONCILE_ERROR_PREFIX`; preserve every other lifecycle error. Continue with the next Profile.

Do not start, stop, or swap a runtime in this method.

- [ ] **Step 4: Invoke catalog reconciliation before runtime restoration**

In Tauri startup setup, call:

```rust
if let Err(error) = state.codex_route_manager.reconcile_all_profile_catalogs().await {
    log::warn!("Codex Profile 模型目录启动对账失败: {error}");
}
```

Place it before the existing enabled-Profile runtime restoration call so each restarted Codex instance sees repaired disk state independently of listener restoration.

- [ ] **Step 5: Run startup and migration regressions**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml reconcile_all_profile_catalogs --lib -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml codex_profile::migration --lib -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit startup self-healing**

```bash
git add src-tauri/src/codex_profile/route_manager.rs src-tauri/src/lib.rs
git commit -m "fix(codex): reconcile Profile catalogs on startup"
```

### Task 7: Run full focused verification and update release documentation

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `docs/release-notes/v3.17.0-zh.md`
- Modify: `docs/release-notes/v3.17.0-en.md`
- Modify: `docs/release-notes/v3.17.0-ja.md`

- [ ] **Step 1: Add release notes**

Document these user-visible facts in all four files:

- every Codex Profile Home gets its own generated catalog;
- shared provider edits fan out to all primary-reference Homes;
- no symlink is required;
- Codex must be restarted to reload the catalog;
- switching to a provider without a catalog removes the pointer but leaves the JSON file.

- [ ] **Step 2: Run Rust formatting and focused suites**

Run:

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml codex_profile::home_config --lib -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml codex_profile::catalog_sync --lib -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml codex_profile::route_manager --lib -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml commands::provider --lib -- --nocapture
```

Expected: all commands PASS.

- [ ] **Step 3: Run the complete backend library suite**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib
```

Expected: PASS with zero failed tests.

- [ ] **Step 4: Check diffs and accidental secrets**

Run:

```bash
git diff --check
git diff -- src-tauri/src/codex_config.rs src-tauri/src/codex_profile src-tauri/src/commands/provider.rs src-tauri/src/services/provider/mod.rs src-tauri/src/lib.rs CHANGELOG.md docs/release-notes
rg -n "OPENAI_API_KEY\s*=|experimental_bearer_token\s*=|Bearer [A-Za-z0-9_-]{20,}" src-tauri/src/codex_profile/catalog_sync.rs src-tauri/src/codex_profile/route_manager.rs docs/release-notes CHANGELOG.md
```

Expected: no whitespace errors, no unrelated changes, and no real credential values.

- [ ] **Step 5: Commit verification documentation**

```bash
git add CHANGELOG.md docs/release-notes/v3.17.0-zh.md docs/release-notes/v3.17.0-en.md docs/release-notes/v3.17.0-ja.md
git commit -m "docs(codex): document multi-Home catalog sync"
```
