# Codex Profile Route Isolation and Local Authentication Repair Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-executing-plans to implement this plan task-by-task.

**Goal:** Make the Codex header route switch strictly Profile-scoped, guarantee that each Profile Home and listener share the same private token, retire the old global Codex takeover lifecycle, and automatically repair currently enabled Profiles such as `/Users/qihoo/.codex-api`.

**Architecture:** Add a dedicated React route toggle whose mutation and cache key always include the selected Profile ID. Make `CodexProfileSecretStore` the single source of the local listener token and project that exact token into the explicit Home plan before starting the Profile runtime. Serialize startup as legacy-Codex retirement followed by Profile Home reconciliation/runtime restore; use fingerprint-only recovery metadata to close cross-resource crash windows without storing credentials.

**Tech Stack:** Rust, Tokio, rusqlite, Tauri v2, React, TypeScript, TanStack Query, Vitest, Testing Library.

---

## Implementation constraints

- Follow test-driven development for every behavior slice: add a failing test, run it and confirm the expected failure, implement the minimum fix, then rerun it.
- New TypeScript functions require JSDoc; new comments are Chinese. New Rust functions keep one objective and use Chinese doc comments for non-obvious lifecycle behavior.
- Put new lifecycle constants in `src-tauri/src/codex_profile/constants.rs`; do not inline operation/phase strings.
- Do not remove existing commented-out code or explanatory comments.
- Do not modify, copy, or delete any Profile `auth.json`, sessions, state database, or Home directory.
- Never log listener tokens, upstream keys, backup bodies, or authorization headers.
- Preserve Claude/Gemini takeover behavior; only Codex leaves the old application-global lifecycle.
- Focused tests must pass before each slice is committed. The final real-world verification must use `pnpm run dev:dump` and Computer Use.

## Task 1: Replace the Codex global header switch with a Profile-scoped switch

**Files:**

- Create: `src/components/codex/CodexProfileRouteToggle.tsx`
- Create: `src/components/codex/CodexProfileRouteToggle.test.tsx`
- Modify: `src/lib/query/codexProfiles.ts`
- Modify: `src/lib/query/codexProfiles.test.ts`
- Modify: `src/App.tsx`
- Test: existing App integration/fixture tests that cover the providers header

- [ ] **Step 1: Add failing Profile mutation tests**

Add `useEnableCodexProfileRoute` and a unified `useSetCodexProfileRouteEnabled`-level behavior test proving:

- enabling A calls `enableRoute("profile-a", "provider-a", [])`;
- disabling B calls `disableRoute("profile-b")`;
- each success invalidates only `codexProfileKeys.state(targetProfileId)`;
- neither mutation invalidates the other Profile or global proxy status.

Run:

```bash
pnpm vitest run src/lib/query/codexProfiles.test.ts
```

Expected before implementation: FAIL because the Profile enable mutation/hook does not exist.

- [ ] **Step 2: Add failing component tests**

Cover these cases in `CodexProfileRouteToggle.test.tsx`:

1. A enabled/B disabled render their own checked state.
2. switching from A to loading B does not retain A's checked state.
3. enabling uses B's `currentProviderId`, never a global provider.
4. disabling only passes B's Profile ID.
5. missing Profile/current provider cannot call an API and shows the scoped error.
6. the tooltip contains the selected Profile's Home/port and not the global proxy port.

Run:

```bash
pnpm vitest run src/components/codex/CodexProfileRouteToggle.test.tsx
```

Expected before implementation: FAIL because the component does not exist.

- [ ] **Step 3: Implement focused query mutations and the dedicated component**

Use a mutation input object containing `profileId`, `providerId`, and `enabled`. Keep API selection in a small documented adapter and cache invalidation in the query hook. The component owns only presentation, validation, and toast/error handling.

The component must not import `useProxyStatus`, `ProxyToggle`, `takeoverStatus`, or application-global provider settings.

- [ ] **Step 4: Wire the header by application scope**

In `App.tsx`:

- render `CodexProfileRouteToggle` for Codex;
- retain `ProxyToggle` for Claude/Gemini;
- do not render application-global `FailoverToggle` for Codex;
- derive Codex ProviderList running/takeover/active values from `codexProfileState`, while leaving other apps unchanged;
- retain the existing `enableLocalProxy` feature gate.

- [ ] **Step 5: Run focused frontend verification**

```bash
pnpm vitest run src/lib/query/codexProfiles.test.ts src/components/codex/CodexProfileRouteToggle.test.tsx src/components/codex/CodexHomeContextBar.test.tsx
pnpm exec tsc --noEmit
```

Expected: PASS.

- [ ] **Step 6: Commit the frontend slice**

```bash
git add src/components/codex/CodexProfileRouteToggle.tsx src/components/codex/CodexProfileRouteToggle.test.tsx src/lib/query/codexProfiles.ts src/lib/query/codexProfiles.test.ts src/App.tsx
git commit -m "fix(codex): scope route toggle to selected profile"
```

## Task 2: Project the exact Profile listener token into its Home

**Files:**

- Modify: `src-tauri/src/codex_profile/home_config.rs`
- Modify: `src-tauri/src/codex_profile/route_manager.rs`
- Modify: `src-tauri/src/codex_config.rs`
- Test: inline `codex_home_config` and route manager tests

- [ ] **Step 1: Add failing pure configuration tests**

Add tests proving:

- `build_codex_profile_route_toml` requires a listener token;
- an existing `PROXY_MANAGED` is replaced by the exact supplied token;
- an existing stale provider-scoped token is replaced;
- the token is written to the active model-provider table in valid TOML;
- a second Home and both Homes' `auth.json` remain byte-identical.

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml codex_home_config --lib
```

Expected before implementation: FAIL because the builder has no listener-token parameter and preserves the placeholder.

- [ ] **Step 2: Make the token a required plan input**

Expose the existing provider-aware Codex token setter only at crate scope. Change:

```rust
build_profile_route_plan(home, listen_port, provider, listener_token)
build_codex_profile_route_toml(toml, listen_port, provider, listener_token)
```

The builder must replace, not append, the effective `experimental_bearer_token`. It must not update `auth.json`.

- [ ] **Step 3: Add a failing manager ordering test**

Use a token-store fake returning a distinctive token and a Home fixture containing `PROXY_MANAGED`. Enable a Profile and assert:

- the runtime factory receives the distinctive token;
- the written Home contains that exact token;
- no placeholder remains;
- the route backup still restores the original bytes when disabled.

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml profile_listener_token --lib
```

Expected before implementation: FAIL because the plan is built before `ensure_token` and does not receive it.

- [ ] **Step 4: Reorder enable without weakening compensation**

Inside the Profile lock, ensure the token before plan construction and pass the same owned value to both the Home plan and runtime factory. Preserve the existing operation record, health check, backup, atomic Home apply, final route save, and reverse-order compensation semantics.

- [ ] **Step 5: Run focused Rust verification**

```bash
cargo test --manifest-path src-tauri/Cargo.toml codex_home_config --lib
cargo test --manifest-path src-tauri/Cargo.toml codex_profile::route_manager --lib
```

Expected: PASS.

- [ ] **Step 6: Commit the token slice**

```bash
git add src-tauri/src/codex_config.rs src-tauri/src/codex_profile/home_config.rs src-tauri/src/codex_profile/route_manager.rs
git commit -m "fix(codex): align profile home and listener token"
```

## Task 3: Retire the old global Codex takeover before Profile restore

**Files:**

- Modify: `src-tauri/src/codex_profile/constants.rs`
- Modify: `src-tauri/src/codex_profile/route_manager.rs`
- Modify: `src-tauri/src/database/dao/proxy.rs` or add a narrow Profile migration persistence method
- Modify: `src-tauri/src/services/proxy.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: focused migration/startup tests and existing proxy service tests

- [ ] **Step 1: Add failing retirement tests**

Cover both branches:

1. when at least one Profile route is enabled, retiring legacy Codex clears only the old Codex `proxy_config.enabled/live_takeover_active` and `proxy_live_backup`, without modifying any Home bytes;
2. when no Profile route is enabled, retirement uses the existing safe restore path before clearing old state;
3. Claude/Gemini rows and backups are byte/field identical;
4. startup proxy restore candidates exclude Codex but still include enabled Claude/Gemini;
5. a retirement failure prevents concurrent Profile restore and leaves retryable state.

Run the narrow test names introduced by the test module, for example:

```bash
cargo test --manifest-path src-tauri/Cargo.toml legacy_codex_takeover_retirement --lib
cargo test --manifest-path src-tauri/Cargo.toml restore_proxy_state_candidates --lib
```

Expected before implementation: FAIL because no retirement boundary exists and startup still enumerates Codex.

- [ ] **Step 2: Add a narrow idempotent retirement service**

Separate decisions from effects:

- a pure decision function reads whether any Profile route is enabled and whether legacy Codex state exists;
- a persistence operation clears only legacy Codex rows when a Profile already owns the Home;
- otherwise the service calls the existing safe `set_takeover_for_app("codex", false)` restore path;
- repeat execution is a no-op.

Do not infer that only the default Profile may use subscriptions or routes.

- [ ] **Step 3: Serialize startup ownership**

Remove the early independent Profile-restore spawn. In the existing startup async sequence:

1. retire legacy Codex takeover;
2. run generic crash recovery for remaining applications;
3. restore enabled Codex Profiles;
4. restore application-global proxy state for Claude/Gemini only.

If step 1 fails, log the failure and skip step 3 in that startup pass so two writers cannot race on a Codex Home. Keep the existing comments and update/add Chinese explanation rather than deleting them.

- [ ] **Step 4: Run retirement and proxy regression tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml legacy_codex_takeover_retirement --lib
cargo test --manifest-path src-tauri/Cargo.toml services::proxy --lib
cargo test --manifest-path src-tauri/Cargo.toml codex_profile --lib
```

Expected: PASS.

- [ ] **Step 5: Commit the retirement slice**

```bash
git add src-tauri/src/codex_profile src-tauri/src/database/dao/proxy.rs src-tauri/src/services/proxy.rs src-tauri/src/lib.rs
git commit -m "fix(codex): retire global takeover lifecycle"
```

## Task 4: Reconcile enabled Profile Home configuration during startup

**Files:**

- Modify: `src-tauri/src/codex_profile/constants.rs`
- Modify: `src-tauri/src/codex_profile/home_config.rs`
- Modify: `src-tauri/src/codex_profile/route_manager.rs`
- Test: inline route manager and Home configuration tests

- [ ] **Step 1: Add failing Home-backup rebase tests**

Add focused tests proving that rebasing a route backup:

- preserves the original `previous_content` and `previous_fingerprint`;
- changes only `target_fingerprint` to the repaired token configuration;
- rejects a current Home that matches neither the old target nor a recognized legacy-managed placeholder;
- never serializes a listener token into the backup metadata beyond the already existing Home bytes policy.

- [ ] **Step 2: Add failing startup reconciliation tests**

Cover:

1. enabled Profile with matching correct Home starts without a write;
2. enabled Profile with `PROXY_MANAGED` self-heals to the exact token and starts;
3. enabled Profile with a stale old listener token self-heals when its fingerprint matches the stored route target;
4. external Home edit is preserved and only that Profile gets `last_error`;
5. failure after Home apply compensates to the old target;
6. crash with Home already at the new target finalizes the rebased backup and clears recovery;
7. one failed Profile does not block a following Profile.

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml restoring_enabled_profile_home --lib
```

Expected before implementation: FAIL because startup only starts runtime and never inspects/applies Home.

- [ ] **Step 3: Add fingerprint-only reconcile recovery metadata**

Add constants for the operation and phases in `constants.rs`. Extend the recovery record with an optional reconcile transition that stores only old/new fingerprints. Existing serialized recovery JSON must remain readable through `serde(default)` and omit the field when unused.

- [ ] **Step 4: Implement idempotent reconcile-then-start**

Extract focused helpers for:

- deriving the desired Profile plan from the ensured token;
- classifying current Home as correct, old route-owned, legacy-managed, or external;
- rebasing the original backup target fingerprint;
- applying/finalizing/compensating reconciliation;
- starting and health-checking a runtime after Home ownership is converged.

Do not add one large branch to `restore_enabled_profiles`; keep per-Profile failure isolation intact.

- [ ] **Step 5: Run focused recovery verification**

```bash
cargo test --manifest-path src-tauri/Cargo.toml restoring_enabled_profile_home --lib
cargo test --manifest-path src-tauri/Cargo.toml codex_profile::route_manager --lib
```

Expected: PASS.

- [ ] **Step 6: Commit the recovery slice**

```bash
git add src-tauri/src/codex_profile/constants.rs src-tauri/src/codex_profile/home_config.rs src-tauri/src/codex_profile/route_manager.rs
git commit -m "fix(codex): reconcile enabled profile homes on startup"
```

## Task 5: Run the automated regression matrix

**Files:** all modified files

- [ ] **Step 1: Format and lint**

```bash
pnpm exec prettier --check src docs/superpowers/findings/2026-07-14-codex-profile-route-isolation-auth-findings.md docs/superpowers/specs/2026-07-14-codex-profile-route-isolation-auth-design.md docs/superpowers/plans/2026-07-14-codex-profile-route-isolation-auth.md
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
pnpm exec tsc --noEmit
```

- [ ] **Step 2: Run frontend tests**

```bash
pnpm vitest run src/lib/query/codexProfiles.test.ts src/components/codex
```

- [ ] **Step 3: Run Rust focused and full library tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml codex_profile --lib
cargo test --manifest-path src-tauri/Cargo.toml --lib
```

- [ ] **Step 4: Review the final diff**

```bash
git diff --check
git status --short
git diff --stat
```

Resolve only task-related failures. Record any environment-only blocker without weakening test assertions.

## Task 6: Close the real-world loop with `dev:dump` and Computer Use

**Files/Runtime:** real CC Switch dev app, `/Users/qihoo/.codex`, `/Users/qihoo/.codex-api`, `~/.cc-switch/logs`, `~/.cc-switch/logs/proxy-bodies`

- [ ] **Step 1: Capture safe pre-verification evidence**

Record only non-secret values: selected Profile IDs, route enabled flags, ports, process listeners, config/listener token lengths and hashes, and relevant file mtimes. Never print actual tokens or provider keys.

- [ ] **Step 2: Start the required build**

```bash
pnpm run dev:dump
```

Wait for both the frontend and rebuilt Rust backend to be ready. Confirm logs show legacy Codex retirement before Profile reconciliation/runtime restore.

- [ ] **Step 3: Verify UI isolation with Computer Use**

1. Select default `~/.codex`; verify its route switch reflects only the default Profile and official subscription remains usable.
2. Select `codex-api · /Users/qihoo/.codex-api`; verify the switch reflects only this Profile at port 15722.
3. Toggle `codex-api` off/on and switch back to default between operations; verify neither checked state, provider selection, Home config, nor error crosses Profile boundaries.
4. Verify the old OpenAI Official global warning does not appear when operating `codex-api`.

- [ ] **Step 4: Verify local and real request authentication**

- call `/v1/models` with the Home-configured bearer and expect 200;
- issue a real Codex `/v1/responses` request through `/Users/qihoo/.codex-api` and confirm it reaches the configured upstream rather than failing local authentication;
- compare only SHA-256/length of Home token and listener token and require equality;
- inspect Profile body-dump/log ownership and require the request to be attributed to the `codex-api` Profile.

- [ ] **Step 5: Verify restart stability**

Stop the dev app cleanly, restart `pnpm run dev:dump`, and repeat Profile selection plus `/v1/models` authentication. Confirm old global Codex `enabled/live_backup` does not reappear and `PROXY_MANAGED` is not written back.

- [ ] **Step 6: Diagnose before modifying if reality differs**

If any UI or request check fails, first add a narrowly scoped Chinese diagnostic log at the relevant lifecycle boundary, reproduce once, use the evidence to update the finding, then make a targeted TDD fix. Do not guess or repeat an unchanged failed operation.

- [ ] **Step 7: Final verification commit**

Update the finding with real verification evidence, run the affected focused tests once more, and commit only after every acceptance item passes.
