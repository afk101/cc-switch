# Codex Multi-Home Instance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-subagent-driven-development OR superpowers-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let CC Switch manage multiple equal-capability Codex Profiles, with globally shared providers and one independently isolated route runtime and port per `CODEX_HOME`.

**Architecture:** Add a Profile persistence and configuration layer that resolves every Codex filesystem operation from an explicit `profile_id`. Add a `CodexRouteManager` beside the existing singleton proxy service; it owns one isolated `RouteRuntime` per enabled Profile. Keep provider definitions global, while current provider, failover, runtime state, configuration backup, sessions, usage, MCP, and Skill associations are Profile-scoped.

**Tech Stack:** Rust, Tokio, Axum, rusqlite, Tauri v2, React, TypeScript, TanStack Query, Vitest, Testing Library.

---

## Implementation constraints

- Use test-driven development: add one failing behavior test, run it and observe the expected failure, then add the minimum production code.
- New Rust and TypeScript functions each handle one objective. Extract path normalization, port allocation, transaction planning, and query-key construction instead of growing existing functions.
- Put Rust constants in `src-tauri/src/codex_profile/constants.rs`; put frontend constants in `src/config/constants.ts`.
- Add Chinese comments for new non-obvious logic. Do not remove any existing commented-out code or explanatory comment.
- Do not change the singleton semantics for Claude, Gemini, OpenCode, OpenClaw, Hermes, or other applications.
- Never infer a Home from the process-global `CODEX_HOME` after a `profile_id` has entered the call chain.
- Never copy or delete a Profile's `auth.json`.
- Every task ends with its focused tests and a narrow commit. Run the full verification matrix only after all focused tasks pass.

## Target file structure

### Rust additions

- Create: `src-tauri/src/codex_profile/mod.rs`
- Create: `src-tauri/src/codex_profile/constants.rs`
- Create: `src-tauri/src/codex_profile/model.rs`
- Create: `src-tauri/src/codex_profile/repository.rs`
- Create: `src-tauri/src/codex_profile/migration.rs`
- Create: `src-tauri/src/codex_profile/home_config.rs`
- Create: `src-tauri/src/codex_profile/secret_store.rs`
- Create: `src-tauri/src/codex_profile/route_runtime.rs`
- Create: `src-tauri/src/codex_profile/route_manager.rs`
- Create: `src-tauri/src/commands/codex_profile.rs`
- Create: `src-tauri/src/database/dao/codex_profiles.rs`

### Rust modifications

- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/store.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/database/mod.rs`
- Modify: `src-tauri/src/database/dao/mod.rs`
- Modify: `src-tauri/src/database/schema.rs`
- Modify: `src-tauri/src/database/backup.rs`
- Modify: `src-tauri/src/codex_config.rs`
- Modify: `src-tauri/src/proxy/server.rs`
- Modify: `src-tauri/src/proxy/provider_router.rs`
- Modify: `src-tauri/src/proxy/body_dump.rs`
- Modify: `src-tauri/src/services/proxy.rs`
- Modify: `src-tauri/src/commands/provider.rs`
- Modify: `src-tauri/src/commands/config.rs`
- Modify: `src-tauri/src/commands/session_manager.rs`
- Modify: `src-tauri/src/commands/usage.rs`
- Modify: `src-tauri/src/commands/mcp.rs`
- Modify: `src-tauri/src/commands/skill.rs`
- Modify: `src-tauri/src/services/session_usage_codex.rs`
- Modify: `src-tauri/src/settings.rs`

### Frontend additions

- Create: `src/types/codexProfile.ts`
- Create: `src/lib/api/codexProfiles.ts`
- Create: `src/lib/query/codexProfiles.ts`
- Create: `src/components/codex/CodexHomeContextBar.tsx`
- Create: `src/components/codex/CodexProfileManagerDialog.tsx`
- Create: `src/components/codex/CodexHomeContextBar.test.tsx`
- Create: `src/components/codex/CodexProfileManagerDialog.test.tsx`

### Frontend modifications

- Modify: `src/types.ts`
- Modify: `src/lib/api/index.ts`
- Modify: `src/lib/api/providers.ts`
- Modify: `src/lib/query/index.ts`
- Modify: `src/lib/query/queries.ts`
- Modify: `src/lib/query/mutations.ts`
- Modify: `src/hooks/useProviderActions.ts`
- Modify: `src/App.tsx`
- Modify: `src/components/providers/ProviderList.tsx`
- Modify: `src/components/providers/ProviderCard.tsx`
- Modify: `src/components/settings/DirectorySettings.tsx`
- Modify: `src/config/constants.ts`

## Task 1: Add the v12 Profile schema and database DAO

**Files:**
- Create: `src-tauri/src/database/dao/codex_profiles.rs`
- Modify: `src-tauri/src/database/dao/mod.rs`
- Modify: `src-tauri/src/database/mod.rs`
- Modify: `src-tauri/src/database/schema.rs`
- Modify: `src-tauri/src/database/backup.rs`
- Test: `src-tauri/src/database/tests.rs`

- [ ] **Step 1: Write failing migration tests**

Add tests that open an in-memory v11 database, run schema migration, and assert the Profile tables, Profile-MCP/Profile-Skill relation tables, usage ownership columns, unique Home path, unique route port, and failover ordering constraints:

```rust
/// v11 升级后应创建 Codex Profile 相关表并进入 v12。
#[test]
fn migrates_v11_to_v12_codex_profile_schema() {
    let conn = rusqlite::Connection::open_in_memory().expect("打开内存数据库");
    Database::set_user_version(&conn, 11).expect("设置 v11");

    Database::apply_schema_migrations_on_conn(&conn).expect("迁移到 v12");

    assert_eq!(Database::get_user_version(&conn).expect("读取版本"), 12);
    assert!(table_exists(&conn, "codex_profiles"));
    assert!(table_exists(&conn, "codex_profile_routes"));
    assert!(table_exists(&conn, "codex_profile_failovers"));
    assert!(table_exists(&conn, "codex_profile_mcp_servers"));
    assert!(table_exists(&conn, "codex_profile_skills"));
    assert!(Database::has_column(&conn, "proxy_request_logs", "profile_id").expect("检查请求日志列"));
    assert!(Database::has_column(&conn, "session_log_sync", "profile_id").expect("检查会话同步列"));
}
```

Also insert duplicate `canonical_home_path`, duplicate `listen_port`, and duplicate `(profile_id, position)` rows and assert each insert fails.

- [ ] **Step 2: Run the focused migration tests and verify failure**

Run: `cargo test --manifest-path src-tauri/Cargo.toml migrates_v11_to_v12_codex_profile_schema --lib`

Expected: FAIL because `SCHEMA_VERSION` is still 11 and the new tables do not exist.

- [ ] **Step 3: Add schema constants and v12 migration**

Change `SCHEMA_VERSION` to `12`, add the `11 => migrate_v11_to_v12` branch, and create all five tables described in the spec. Store `provider_app_type='codex'` beside provider IDs so composite foreign keys can reference `providers(id, app_type)`, and add a `CHECK(provider_app_type = 'codex')`. Add indexes for `current_provider_id`, failover ordering, route enabled state, MCP/Skill reverse lookup, and Profile usage lookup.

Add nullable `profile_id` to `proxy_request_logs` and `session_log_sync`. Rebuild `usage_daily_rollups` using the existing v10 -> v11 migration pattern so `profile_id TEXT NOT NULL DEFAULT ''` is included in its primary key. Empty profile IDs retain all non-Codex and not-yet-migrated historical rows until Task 3 assigns legacy Codex data.

Do not read `AppSettings` inside the rusqlite schema migration. The schema migration only creates durable storage; Task 3 performs the settings-aware population after both database and settings are available.

- [ ] **Step 4: Add focused DAO methods**

Implement only persistence methods in `database/dao/codex_profiles.rs`:

```rust
impl Database {
    pub fn list_codex_profiles(&self) -> Result<Vec<CodexProfile>, AppError>;
    pub fn get_codex_profile(&self, profile_id: &str) -> Result<CodexProfile, AppError>;
    pub fn insert_codex_profile(&self, profile: &CodexProfile) -> Result<(), AppError>;
    pub fn update_codex_profile(&self, profile: &CodexProfile) -> Result<(), AppError>;
    pub fn delete_codex_profile(&self, profile_id: &str) -> Result<(), AppError>;
    pub fn get_codex_profile_route(&self, profile_id: &str) -> Result<CodexProfileRoute, AppError>;
    pub fn save_codex_profile_route(&self, route: &CodexProfileRoute) -> Result<(), AppError>;
    pub fn list_codex_profile_failovers(&self, profile_id: &str) -> Result<Vec<String>, AppError>;
    pub fn replace_codex_profile_failovers(&self, profile_id: &str, provider_ids: &[String]) -> Result<(), AppError>;
    pub fn list_codex_provider_profile_refs(&self, provider_id: &str) -> Result<Vec<CodexProfileRef>, AppError>;
}
```

Keep path validation, runtime control, and file I/O out of the DAO.

- [ ] **Step 5: Include Profile tables in database backup/import allowlists**

Add `codex_profiles`, `codex_profile_routes`, `codex_profile_failovers`, `codex_profile_mcp_servers`, and `codex_profile_skills` to the existing explicit table lists in `database/backup.rs`. The schema must not contain a local listener token column, so database backup/WebDAV/S3 paths cannot export that secret.

- [ ] **Step 6: Run database tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml database --lib`

Expected: PASS.

- [ ] **Step 7: Commit the schema slice**

```bash
git add src-tauri/src/database
git commit -m "feat(codex): add multi-home profile schema"
```

## Task 2: Add Profile domain constants, models, path validation, and port allocation

**Files:**
- Create: `src-tauri/src/codex_profile/constants.rs`
- Create: `src-tauri/src/codex_profile/model.rs`
- Create: `src-tauri/src/codex_profile/repository.rs`
- Create: `src-tauri/src/codex_profile/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: inline `#[cfg(test)]` modules in the new files

- [ ] **Step 1: Write failing path and port tests**

Add three named tests with complete fixtures:

- `canonical_home_path_rejects_symlink_duplicate`: use `tempfile` to create one real directory and a symlink to it, insert the real path, then assert validating the symlink returns `DuplicateCodexHome` with the existing Profile ID.
- `allocate_port_skips_reserved_and_bound_ports`: reserve `15722` in the fake repository, bind an OS listener to `15723`, and assert allocation returns `15724`.
- `default_profile_has_no_capability_restrictions`: construct the default Profile and assert both official and route operations pass domain validation, while only rebind/delete return `DefaultCodexProfileImmutable`.

- [ ] **Step 2: Run focused tests and verify failure**

Run: `cargo test --manifest-path src-tauri/Cargo.toml codex_profile --lib`

Expected: FAIL because the `codex_profile` module does not exist.

- [ ] **Step 3: Define constants in the required constant module**

Add:

```rust
pub const DEFAULT_CODEX_PROFILE_ID: &str = "codex-default";
pub const DEFAULT_CODEX_PROFILE_NAME: &str = "默认 Codex";
pub const LEGACY_CODEX_ROUTE_PORT: u16 = 15_721;
pub const FIRST_CUSTOM_CODEX_ROUTE_PORT: u16 = 15_722;
pub const CODEX_ROUTE_LISTEN_HOST: &str = "127.0.0.1";
pub const LOCAL_TOKEN_BYTES: usize = 32;
```

Do not duplicate these values in commands, UI components, or tests.

- [ ] **Step 4: Add immutable models and a repository facade**

Define `CodexProfile`, `CodexProfileRoute`, `CodexProfileRef`, `CodexProfileScope`, `CodexRuntimeStatus`, and `CodexProfileState`. Keep `is_default` out of capability decisions. `CodexProfileRepository` composes the database DAO with two focused collaborators:

```rust
pub trait HomePathCanonicalizer: Send + Sync {
    fn canonicalize(&self, input: &std::path::Path) -> Result<std::path::PathBuf, AppError>;
}

pub trait PortAvailability: Send + Sync {
    fn is_available(&self, host: &str, port: u16) -> Result<bool, AppError>;
}
```

Add `validate_new_home`, `create_profile`, `rename_profile`, `rebind_profile`, and `allocate_port`. `rebind_profile` rejects the default Profile and any running Profile. `delete_profile` is not added here; lifecycle deletion belongs to Task 6.

- [ ] **Step 5: Run focused tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml codex_profile --lib`

Expected: PASS.

- [ ] **Step 6: Commit the domain slice**

```bash
git add src-tauri/src/codex_profile src-tauri/src/lib.rs
git commit -m "feat(codex): add profile domain repository"
```

## Task 3: Add idempotent legacy Profile migration

**Files:**
- Create: `src-tauri/src/codex_profile/migration.rs`
- Modify: `src-tauri/src/settings.rs`
- Modify: `src-tauri/src/store.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: inline tests in `src-tauri/src/codex_profile/migration.rs`

- [ ] **Step 1: Write failing migration behavior tests**

Use temporary Homes and an in-memory database to prove:

1. no override creates only `codex-default` for `~/.codex`;
2. an override outside `~/.codex` creates default plus “原 Codex 配置” and selects the latter;
3. old current provider, route state, live backup, failover and port `15721` go to the old actual Home;
4. running migration twice produces identical rows;
5. no `auth.json`, `config.toml`, or session file is moved, copied, or rewritten.

- [ ] **Step 2: Run focused migration tests and verify failure**

Run: `cargo test --manifest-path src-tauri/Cargo.toml legacy_codex_profile_migration --lib`

Expected: FAIL because `CodexProfileMigrationService` does not exist.

- [ ] **Step 3: Add the migration service**

Implement:

```rust
pub struct LegacyCodexProfileSnapshot {
    pub override_home: Option<std::path::PathBuf>,
    pub current_provider_id: Option<String>,
    pub route_enabled: bool,
    pub failover_provider_ids: Vec<String>,
    pub live_backup_json: Option<String>,
}

impl CodexProfileMigrationService {
    /// 将旧单例 Codex 状态幂等映射到 Profile，不修改任何 Home 文件。
    pub fn migrate(&self, snapshot: LegacyCodexProfileSnapshot) -> Result<String, AppError>;
}
```

The returned string is the selected Profile ID. Add `selected_codex_profile_id: Option<String>` and a migration-completed marker to `AppSettings`. Continue reading legacy fields for migration only; new Profile mutations must not write them.

- [ ] **Step 4: Wire migration after database initialization and before runtime restore**

In application setup, build the legacy snapshot from settings/database, call `migrate`, persist the selected ID, then create `AppState`. Do not start or rewrite a route in this task.

- [ ] **Step 5: Run migration and settings tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml legacy_codex_profile_migration --lib`

Expected: PASS.

Run: `cargo test --manifest-path src-tauri/Cargo.toml settings --lib`

Expected: PASS.

- [ ] **Step 6: Commit the migration slice**

```bash
git add src-tauri/src/codex_profile/migration.rs src-tauri/src/settings.rs src-tauri/src/store.rs src-tauri/src/lib.rs
git commit -m "feat(codex): migrate legacy home into profiles"
```

## Task 4: Make Codex Home configuration operations explicit and transactional

**Files:**
- Create: `src-tauri/src/codex_profile/home_config.rs`
- Create: `src-tauri/src/codex_profile/secret_store.rs`
- Modify: `src-tauri/src/codex_config.rs`
- Test: inline tests in `src-tauri/src/codex_profile/home_config.rs` and `src-tauri/src/codex_profile/secret_store.rs`

- [ ] **Step 1: Write failing Home isolation and rollback tests**

Create two temporary Homes with different `auth.json` and `config.toml`, then add these complete behavior tests:

- `writes_route_config_only_to_explicit_home`: apply A's route plan, assert A's `config.toml` changed to A's port, B's `config.toml` stayed byte-identical, and both `auth.json` files stayed byte-identical.
- `atomic_route_write_preserves_previous_config_on_failure`: inject a `CodexHomeFileOps` whose rename fails, then assert A's previous live bytes and stored fingerprint are unchanged.
- `restore_rejects_external_live_config_change`: build a backup, externally replace live config, call restore, and assert `CodexLiveConfigConflict` contains expected and actual fingerprints without overwriting the external bytes.
- `profile_secret_store_is_local_and_profile_scoped`: create tokens for A/B, assert different values and paths, assert neither value exists in a database export, and on Unix assert token files use mode `0600`.
- `legacy_active_profile_keeps_compatible_token_without_home_write`: migrate an already-running `PROXY_MANAGED` Home, initialize its secret store, and assert the Home bytes are unchanged while the listener accepts the compatibility token.

- [ ] **Step 2: Run focused tests and verify failure**

Run: `cargo test --manifest-path src-tauri/Cargo.toml codex_home_config --lib`

Expected: FAIL because `CodexHomeConfigService` and `CodexProfileSecretStore` do not exist.

- [ ] **Step 3: Extract path-explicit primitives from `codex_config.rs`**

Add focused functions that take `&Path` and keep the old global wrappers temporarily for non-migrated callers:

```rust
pub fn codex_auth_path_for_home(home: &std::path::Path) -> std::path::PathBuf;
pub fn codex_config_path_for_home(home: &std::path::Path) -> std::path::PathBuf;
pub fn codex_models_dir_for_home(home: &std::path::Path) -> std::path::PathBuf;
pub fn write_codex_live_atomic_for_home(home: &std::path::Path, content: &str) -> Result<(), AppError>;
```

All new Profile code calls these explicit functions. Do not delete existing comments or wrappers while legacy callers remain.

- [ ] **Step 4: Implement `CodexHomeConfigService`**

Separate responsibilities behind `CodexHomeFileOps` so failures are testable. Implement `inspect`, `build_route_plan`, `apply_route_plan`, `restore`, and `fingerprint`. The plan is side-effect free and contains the previous bytes, target bytes, model changes, and expected fingerprint. Atomic writes use a temporary file in the same directory followed by rename.

Never read, write, back up, or copy `auth.json` in this service.

- [ ] **Step 5: Implement the local-only Profile secret store**

Add `create`, `read`, `rotate`, and `delete` methods. Derive the token path only from a validated Profile ID under the CC Switch secret root. Use a same-directory temporary file plus atomic rename; set owner-only permissions on Unix. `delete` removes only the token file and then removes the Profile secret directory only when empty.

For a migrated enabled Profile whose live config still contains `PROXY_MANAGED`, initialize that exact compatibility token without writing Home. New and explicitly rotated Profiles use 32 random bytes encoded as URL-safe text.

- [ ] **Step 6: Run focused tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml codex_home_config --lib`

Expected: PASS.

- [ ] **Step 7: Run secret store tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml codex_profile_secret_store --lib`

Expected: PASS.

- [ ] **Step 8: Commit the Home configuration slice**

```bash
git add src-tauri/src/codex_profile/home_config.rs src-tauri/src/codex_profile/secret_store.rs src-tauri/src/codex_config.rs
git commit -m "refactor(codex): scope config IO to explicit home"
```

## Task 5: Add an isolated RouteRuntime per Profile

**Files:**
- Create: `src-tauri/src/codex_profile/route_runtime.rs`
- Modify: `src-tauri/src/proxy/server.rs`
- Modify: `src-tauri/src/proxy/provider_router.rs`
- Modify: `src-tauri/src/proxy/body_dump.rs`
- Test: inline tests and existing proxy test modules

- [ ] **Step 1: Write failing runtime isolation tests**

Start two test runtimes with different scopes/providers and add three complete integration-style tests:

- `route_runtimes_isolate_codex_chat_history_for_identical_ids`: send different content through A and B while reusing the same `response_id` and `call_id`, then assert each mock upstream receives only its Profile's recovered tool history.
- `route_runtimes_isolate_breakers_and_failover`: force A's primary upstream past the failure threshold, then assert A selects its fallback while B's breaker snapshot and provider ID stay unchanged.
- `route_runtime_does_not_forward_local_token`: send A's local Bearer token to its listener, capture upstream headers, and assert the local token is absent while A provider authorization is present.

- [ ] **Step 2: Run isolation tests and verify failure**

Run: `cargo test --manifest-path src-tauri/Cargo.toml route_runtimes_isolate --lib`

Expected: FAIL because `RouteRuntime` and Profile-scoped server construction do not exist.

- [ ] **Step 3: Add Profile-scoped server construction**

Keep `ProxyServer::new()` for existing apps. Add a constructor that receives a fixed `CodexProfileScope`, listen address, local token, initial provider snapshot, and Profile failover list. The resulting server owns its own `ProviderRouter`, circuit breaker, `CodexChatHistoryStore`, status container, and in-flight counter.

Do not add a shared map inside `CodexChatHistoryStore`; independent runtime ownership is the primary isolation boundary.

- [ ] **Step 4: Implement `RouteRuntime` lifecycle**

Expose `start`, `health_check`, `swap_provider_snapshot`, `begin_draining`, `stop`, and `status`. `swap_provider_snapshot` changes only new requests; handlers clone the current immutable snapshot once at request start.

Bind only `127.0.0.1`. Validate the Profile local token before request transformation, remove that Authorization value, then let the provider adapter add upstream credentials.

- [ ] **Step 5: Namespace diagnostic logs**

Change body dump path construction to include a sanitized stable Profile ID, for example `proxy-bodies/<profile-id>/YYYYMMDD-...log`. Add `profile_id` and port to structured route lifecycle logs. Never log the local token.

- [ ] **Step 6: Run runtime and existing proxy tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml route_runtime --lib`

Expected: PASS.

Run: `cargo test --manifest-path src-tauri/Cargo.toml proxy:: --lib`

Expected: PASS.

- [ ] **Step 7: Commit the runtime slice**

```bash
git add src-tauri/src/codex_profile/route_runtime.rs src-tauri/src/proxy
git commit -m "feat(codex): isolate route runtime per profile"
```

## Task 6: Orchestrate activation, switching, shutdown, and deletion

**Files:**
- Create: `src-tauri/src/codex_profile/route_manager.rs`
- Modify: `src-tauri/src/store.rs`
- Modify: `src-tauri/src/services/proxy.rs`
- Test: inline tests in `src-tauri/src/codex_profile/route_manager.rs`

- [ ] **Step 1: Write failing lifecycle transaction tests**

Use fake repository, runtime factory, and Home config service to cover:

- listener failure produces no file/database change;
- config write failure stops the new runtime and restores old state;
- database save failure restores the file and old runtime/provider snapshot;
- provider switch keeps port/token and does not alter in-flight request snapshots;
- disabling restores config before draining listener;
- deleting a custom Profile stops/restores it and never removes the Home;
- restore failure prevents Profile deletion;
- startup restores each enabled Profile independently and records isolated errors.

- [ ] **Step 2: Run focused manager tests and verify failure**

Run: `cargo test --manifest-path src-tauri/Cargo.toml codex_route_manager --lib`

Expected: FAIL because `CodexRouteManager` does not exist.

- [ ] **Step 3: Implement per-Profile locking and activation plans**

Add a lock map keyed by `profile_id`. Different Profile locks may be held concurrently. Implement the exact ordering from the spec:

```text
validate -> lock -> build plan -> start + health check runtime
-> backup and atomic live write -> persist enabled state
```

Rollback executes in reverse order and aggregates rollback failures into the Profile's `last_error` without discarding the original error.

- [ ] **Step 4: Implement provider switching and graceful disable**

For switching, validate provider app type is Codex, swap the runtime snapshot, save `current_provider_id`, and restore the old snapshot if persistence fails. For disable, restore Home first, reject new requests, drain with a constant timeout from `constants.rs`, stop listener, then persist disabled.

- [ ] **Step 5: Add safe Profile deletion**

Reject the default Profile. If a custom Profile is enabled, call the same disable transaction. Delete only database relationships; do not call `remove_dir`, `remove_dir_all`, or delete any file under Home.

- [ ] **Step 6: Put the manager beside—not inside—the existing singleton server slot**

Add `codex_route_manager: CodexRouteManager` to `AppState`. Keep `ProxyService.server: Arc<RwLock<Option<ProxyServer>>>` for existing singleton app routing; remove Codex multi-Profile operations from that single slot without changing other app behavior.

- [ ] **Step 7: Run manager and existing service tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml codex_route_manager --lib`

Expected: PASS.

Run: `cargo test --manifest-path src-tauri/Cargo.toml services::proxy --lib`

Expected: PASS.

- [ ] **Step 8: Commit the lifecycle slice**

```bash
git add src-tauri/src/codex_profile/route_manager.rs src-tauri/src/store.rs src-tauri/src/services/proxy.rs
git commit -m "feat(codex): manage independent profile routes"
```

## Task 7: Expose Profile-aware Tauri commands and provider deletion guards

**Files:**
- Create: `src-tauri/src/commands/codex_profile.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/commands/provider.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: command tests beside the new command module

- [ ] **Step 1: Write failing command contract tests**

Cover list/create/rename/rebind/update-port/get-state/enable/switch/disable/delete. Assert every mutation targets the supplied Profile ID, unknown IDs return a typed error, and a provider referenced by one or more Profiles returns the referencing `CodexProfileRef` list instead of deleting.

- [ ] **Step 2: Run focused command tests and verify failure**

Run: `cargo test --manifest-path src-tauri/Cargo.toml commands::codex_profile --lib`

Expected: FAIL because the commands are not registered.

- [ ] **Step 3: Add Profile commands**

Register these Tauri commands:

```rust
list_codex_profiles
create_codex_profile
rename_codex_profile
rebind_codex_profile
update_codex_profile_port
get_codex_profile_state
enable_codex_profile_route
switch_codex_profile_provider
disable_codex_profile_route
delete_codex_profile
```

Commands validate DTO shape and delegate one objective to repository/manager services. They do not perform direct SQL, port binding, or file writes.

- [ ] **Step 4: Guard global provider deletion**

Before deleting a Codex provider, query Profile references. Return a structured `ProviderInUseByCodexProfiles { profiles }` error when non-empty. Keep deletion semantics for other app types unchanged.

- [ ] **Step 5: Emit Profile-specific events**

Emit `codex-profile-state-changed` with `profileId`, `status`, `providerId`, and `port`. Do not overload the existing app-level `provider-switched` event with ambiguous Codex Profile state.

- [ ] **Step 6: Run command and provider tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml commands::codex_profile --lib`

Expected: PASS.

Run: `cargo test --manifest-path src-tauri/Cargo.toml commands::provider --lib`

Expected: PASS.

- [ ] **Step 7: Commit the command slice**

```bash
git add src-tauri/src/commands src-tauri/src/lib.rs
git commit -m "feat(codex): expose profile route commands"
```

## Task 8: Scope Codex config, sessions, usage, MCP, and Skills to Profile

**Files:**
- Modify: `src-tauri/src/commands/config.rs`
- Modify: `src-tauri/src/commands/session_manager.rs`
- Modify: `src-tauri/src/commands/usage.rs`
- Modify: `src-tauri/src/commands/mcp.rs`
- Modify: `src-tauri/src/commands/skill.rs`
- Modify: `src-tauri/src/services/session_usage_codex.rs`
- Modify: `src-tauri/src/database/schema.rs` for tests and any corrective migration detail discovered while wiring the Task 1 association tables
- Modify: relevant DAO files under `src-tauri/src/database/dao/`
- Test: existing command/service tests plus new Profile isolation cases

- [ ] **Step 1: Inventory remaining implicit Home calls**

Run: `rg -n "get_codex_config_dir\(|get_codex_config_path\(|get_codex_auth_path\(" src-tauri/src/commands src-tauri/src/services src-tauri/src/session_manager src-tauri/src/mcp`

Expected: A concrete list of legacy Codex call sites to eliminate from Profile-aware paths. Save the list in the task notes; do not mechanically replace non-Codex code.

- [ ] **Step 2: Write failing cross-Home service tests**

For two temporary Homes, assert config export/import, session listing, usage scan, MCP enablement, and Skill enablement return or modify only the requested Profile. Use distinct sentinel files/records so accidental global fallback is visible.

- [ ] **Step 3: Run focused service tests and verify failure**

Run: `cargo test --manifest-path src-tauri/Cargo.toml codex_profile_scope --lib`

Expected: FAIL because commands still resolve the global Codex directory or global enablement relation.

- [ ] **Step 4: Add `profile_id` DTO fields and resolve once at command boundaries**

For Codex-specific calls, accept `profile_id`, load the Profile through `CodexProfileRepository`, and pass `&profile.home_path` to path-explicit service functions. Keep existing parameters for other apps.

MCP and Skill definitions remain global. Store Codex enablement in the Task 1 `(profile_id, definition_id)` association tables. Session and usage result DTOs include `profileId` so records from two Homes cannot be merged without an explicit aggregate view.

- [ ] **Step 5: Remove implicit global Home access from migrated paths**

Run the inventory command again.

Expected: No `get_codex_config_dir()` call remains in the migrated Codex command/service/session/MCP code paths. Any intentionally retained legacy migration wrapper has an adjacent Chinese comment stating that it is migration-only.

- [ ] **Step 6: Run focused and module tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml codex_profile_scope --lib`

Expected: PASS.

Run: `cargo test --manifest-path src-tauri/Cargo.toml session_manager --lib`

Expected: PASS.

- [ ] **Step 7: Commit the service isolation slice**

```bash
git add src-tauri/src/commands src-tauri/src/services src-tauri/src/session_manager src-tauri/src/mcp src-tauri/src/database
git commit -m "refactor(codex): scope home services by profile"
```

## Task 9: Add frontend Profile types, API, and cache isolation

**Files:**
- Create: `src/types/codexProfile.ts`
- Create: `src/lib/api/codexProfiles.ts`
- Create: `src/lib/query/codexProfiles.ts`
- Modify: `src/types.ts`
- Modify: `src/lib/api/index.ts`
- Modify: `src/lib/api/providers.ts`
- Modify: `src/lib/query/index.ts`
- Modify: `src/lib/query/queries.ts`
- Modify: `src/lib/query/mutations.ts`
- Modify: `src/config/constants.ts`
- Test: `src/lib/query/codexProfiles.test.ts`

- [ ] **Step 1: Write failing query-key and stale-response tests**

Assert Profile A and B have different keys, provider definitions remain shared, and a delayed A state response cannot replace B's selected state.

```typescript
it("按 Profile 隔离 Codex 状态查询键", () => {
  expect(codexProfileKeys.state("a")).not.toEqual(
    codexProfileKeys.state("b"),
  );
});
```

- [ ] **Step 2: Run focused frontend tests and verify failure**

Run: `pnpm exec vitest run src/lib/query/codexProfiles.test.ts`

Expected: FAIL because Profile API and keys do not exist.

- [ ] **Step 3: Add typed DTOs and API methods**

Model `CodexProfile`, `CodexProfileRoute`, `CodexProfileState`, `CodexProfileRef`, and runtime status values. Implement one API method per Tauri command. Every exported TypeScript function must have JSDoc, for example:

```typescript
/** 获取指定 Codex Profile 的当前路由与供应商状态。 */
export async function getCodexProfileState(
  profileId: string,
): Promise<CodexProfileState> {
  return await invoke("get_codex_profile_state", { profileId });
}
```

Add frontend display constants to `src/config/constants.ts`; do not duplicate `15721`/`15722` in components.

- [ ] **Step 4: Add Profile query keys and mutations**

Use keys such as:

```typescript
export const codexProfileKeys = {
  all: ["codex-profiles"] as const,
  state: (profileId: string) =>
    ["codex-profile-state", profileId] as const,
  refs: (providerId: string) =>
    ["codex-provider-profile-refs", providerId] as const,
};
```

Do not use `keepPreviousData` for the selected Profile's current provider or route status. Keep the global provider definition query as `['providers', 'codex']`, then combine it with the selected Profile state.

- [ ] **Step 5: Add Profile-aware provider calls without breaking other apps**

Keep `providersApi.switch(id, appId)` for non-Codex callers. Add a distinct `codexProfilesApi.switchProvider(profileId, providerId)` and route Codex UI actions through it. Do not add optional `profileId` arguments whose absence silently falls back to a global Home.

- [ ] **Step 6: Run focused tests and typecheck**

Run: `pnpm exec vitest run src/lib/query/codexProfiles.test.ts`

Expected: PASS.

Run: `pnpm typecheck`

Expected: PASS.

- [ ] **Step 7: Commit the frontend data slice**

```bash
git add src/types src/lib/api src/lib/query src/config/constants.ts
git commit -m "feat(codex): add profile frontend data layer"
```

## Task 10: Add the Home context bar and Profile manager

**Files:**
- Create: `src/components/codex/CodexHomeContextBar.tsx`
- Create: `src/components/codex/CodexProfileManagerDialog.tsx`
- Create: `src/components/codex/CodexHomeContextBar.test.tsx`
- Create: `src/components/codex/CodexProfileManagerDialog.test.tsx`
- Modify: `src/App.tsx`
- Modify: `src/components/settings/DirectorySettings.tsx`

- [ ] **Step 1: Write failing context bar tests**

Test that selecting Home B calls `onSelectProfile('b')`, shows B's path/port/status, renders all capabilities for default and custom Profiles, and displays a loading state instead of A's current provider while B is loading.

- [ ] **Step 2: Write failing manager tests**

Test add, rename, rebind, manual port validation, default Profile delete/path restrictions, custom Profile deletion warning, and “delete binding never deletes directory” wording. Assert custom Profiles can also choose official subscription; there must be no `isDefault`-based capability hiding.

- [ ] **Step 3: Run component tests and verify failure**

Run: `pnpm exec vitest run src/components/codex/CodexHomeContextBar.test.tsx src/components/codex/CodexProfileManagerDialog.test.tsx`

Expected: FAIL because the components do not exist.

- [ ] **Step 4: Implement the context bar**

The component receives data and callbacks; it does not call Tauri directly. Put query/mutation composition in `App.tsx` or a focused hook. The selected Home is the highest Codex UI scope and appears above provider controls.

- [ ] **Step 5: Implement the Profile manager**

Render name, canonical path, mode, port, provider, status, and last error. Disable default delete/path editing only. Before deleting a custom Profile, explain that CC Switch removes only the binding and never deletes Home files.

- [ ] **Step 6: Replace the single Codex directory setting**

In `DirectorySettings`, replace the active single `codex_config_dir` editor with an entry that opens Profile management. Preserve the old field and explanatory comments as migration-only compatibility; do not delete them.

- [ ] **Step 7: Run component tests and typecheck**

Run: `pnpm exec vitest run src/components/codex/CodexHomeContextBar.test.tsx src/components/codex/CodexProfileManagerDialog.test.tsx`

Expected: PASS.

Run: `pnpm typecheck`

Expected: PASS.

- [ ] **Step 8: Commit the Profile UI slice**

```bash
git add src/App.tsx src/components/codex src/components/settings/DirectorySettings.tsx
git commit -m "feat(codex): add home profile context UI"
```

## Task 11: Connect provider cards and route controls to the selected Profile

**Files:**
- Modify: `src/hooks/useProviderActions.ts`
- Modify: `src/components/providers/ProviderList.tsx`
- Modify: `src/components/providers/ProviderCard.tsx`
- Modify: `src/App.tsx`
- Modify: existing proxy/failover UI components used by `App.tsx`
- Test: relevant existing tests plus a new `src/components/providers/ProviderList.codexProfiles.test.tsx`

- [ ] **Step 1: Write failing selected-Profile action tests**

Test that switching provider, enabling/disabling route, changing failover, and deleting a provider all use the selected Profile. Test a provider shared by A and B displays both references and cannot be deleted until both bindings are removed.

- [ ] **Step 2: Run focused tests and verify failure**

Run: `pnpm exec vitest run src/components/providers/ProviderList.codexProfiles.test.tsx`

Expected: FAIL because the existing list only accepts one app-level `currentProviderId`.

- [ ] **Step 3: Extend provider presentation with Profile state**

Keep provider definitions global. For the Codex branch pass `selectedProfileState.currentProviderId`, Profile route/failover state, and `usedByProfiles` separately. Do not copy the provider list per Profile.

- [ ] **Step 4: Route all Codex actions through explicit Profile APIs**

In `useProviderActions`, require `selectedCodexProfileId` for Codex actions and fail fast if missing. Non-Codex actions continue to use existing app-level APIs. Add JSDoc to every newly extracted TypeScript function.

- [ ] **Step 5: Prevent stale Profile state in events and optimistic updates**

Handle `codex-profile-state-changed` by invalidating only the event's Profile key. If it is not currently selected, update its cache without changing the visible current provider.

- [ ] **Step 6: Run provider tests and typecheck**

Run: `pnpm exec vitest run src/components/providers/ProviderList.codexProfiles.test.tsx`

Expected: PASS.

Run: `pnpm typecheck`

Expected: PASS.

- [ ] **Step 7: Commit the integration UI slice**

```bash
git add src/App.tsx src/hooks/useProviderActions.ts src/components/providers
git commit -m "feat(codex): scope provider controls to selected home"
```

## Task 12: Add end-to-end isolation and recovery coverage

**Files:**
- Create: `src-tauri/tests/codex_multi_home_isolation.rs`
- Create: `src-tauri/tests/codex_multi_home_recovery.rs`
- Modify: `src-tauri/src/proxy/body_dump.rs` tests if needed
- Modify: test support modules only as required

- [ ] **Step 1: Add a real two-listener isolation test**

Start mock upstream A and B plus RouteRuntime A and B on separate ephemeral ports. Send concurrent requests with different prompts but the same `session_id`, `response_id`, and `call_id`. Assert upstream A receives only A content and upstream B receives only B content; responses and tool history match the correct Profile.

- [ ] **Step 2: Run the test before completing integration and verify failure**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test codex_multi_home_isolation -- --nocapture`

Expected: FAIL until all Profile-scoped handler wiring is complete, or PASS only if prior focused work already satisfies the full contract. Record the observed result; do not claim a red phase that did not occur.

- [ ] **Step 3: Add failure-containment cases**

Cover:

- A port collision does not stop B;
- A upstream failure opens only A breaker;
- A failover leaves B current provider unchanged;
- A runtime forced shutdown leaves B healthy;
- switching A during an SSE stream preserves A's in-flight provider snapshot;
- invalid local credentials produce 401 and no fallback;
- local token never reaches either upstream;
- body dumps are written under separate Profile directories.

- [ ] **Step 4: Add recovery transaction cases**

Inject listener start, health check, backup, atomic rename, database save, restore, drain timeout, and app restart failures one at a time. Assert each failure leaves a deterministic Profile state and never mutates the other Home.

- [ ] **Step 5: Run both integration suites**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test codex_multi_home_isolation -- --nocapture`

Expected: PASS.

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test codex_multi_home_recovery -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Commit the isolation coverage**

```bash
git add src-tauri/tests src-tauri/src/proxy/body_dump.rs
git commit -m "test(codex): cover multi-home route isolation"
```

## Task 13: Perform full verification and manual two-App acceptance

**Files:**
- Verify all implementation files
- Update: `docs/superpowers/findings/2026-07-13-codex-home-multi-instance-findings.md` only with actual verification evidence

- [ ] **Step 1: Check formatting and whitespace**

Run: `git diff --check`

Expected: PASS with no whitespace errors.

- [ ] **Step 2: Run all frontend unit tests**

Run: `pnpm test:unit`

Expected: PASS with zero failed tests.

- [ ] **Step 3: Run TypeScript typecheck**

Run: `pnpm typecheck`

Expected: PASS.

- [ ] **Step 4: Build the renderer**

Run: `pnpm build:renderer`

Expected: PASS.

- [ ] **Step 5: Run the full Rust library and integration test suite**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`

Expected: PASS with zero failed tests.

- [ ] **Step 6: Launch with detailed proxy body logs**

Run: `CC_SWITCH_DUMP_BODY=1 pnpm tauri dev`

Expected: The app launches; Profile A and B each show their configured independent listener and body dumps appear in separate Profile directories under `~/.cc-switch/logs/proxy-bodies`.

- [ ] **Step 7: Perform the user-owned two-App launch acceptance**

Using the user's existing method, launch two Codex Desktop/App instances with different `CODEX_HOME` values. CC Switch itself must not launch them. Verify:

1. custom Profile A can use official subscription with its own `auth.json`;
2. Profile A can route on its assigned port, for example `15722`;
3. Profile B can simultaneously route on its own port, for example `15723`;
4. simultaneous distinct prompts reach only their intended providers;
5. switching/failing Profile A leaves Profile B's stream and state unchanged;
6. deleting a test Profile leaves its Home directory and files intact.

- [ ] **Step 8: Inspect diagnostic evidence**

Inspect `~/.cc-switch/logs/proxy-bodies/<profile-id>/` and application logs. Confirm request IDs and payloads stay in the correct Profile namespace and no API Key, local token, or `auth.json` content appears.

- [ ] **Step 9: Record only observed results**

Append commands, pass/fail counts, manual Profile names/ports, and any incomplete acceptance item to the findings document. If Desktop/App multi-instance launch cannot be exercised in the implementation environment, state that limitation explicitly instead of promoting automated tests to full manual verification.

- [ ] **Step 10: Review final scope**

Run: `git status --short`

Run: `git diff --stat HEAD~13..HEAD`

Expected: Only intended Codex multi-Home implementation, tests, and approved documentation are present; no Home files or credentials are committed.

- [ ] **Step 11: Commit the verified documentation evidence**

```bash
git add docs/superpowers/findings/2026-07-13-codex-home-multi-instance-findings.md
git commit -m "docs(codex): record multi-home verification"
```
